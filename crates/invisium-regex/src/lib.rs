//! Invisium regular-expression engine. Owned by the REGEX track — see `docs/agents/REGEX.md`
//! for the contract.
//!
//! Pipeline: `parser` (pattern → arena AST) → `compile` (AST → instruction program) → `exec`
//! (backtracking matcher with an explicit stack, a step budget and a memory cap).
#![forbid(unsafe_code)]

mod analyze;
mod ast;
mod charset;
mod compile;
mod escape;
mod exec;
mod parser;
mod program;
mod props;
mod unicode;
mod vm;

/// The flags of a regular expression (`/pattern/flags`).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Flags {
    /// `d`
    pub has_indices: bool,
    /// `g` (carried, not interpreted here)
    pub global: bool,
    /// `i`
    pub ignore_case: bool,
    /// `m`
    pub multiline: bool,
    /// `s`
    pub dot_all: bool,
    /// `u` (also set by `v`)
    pub unicode: bool,
    /// `v` (accepted; set-operation syntax is not supported)
    pub unicode_sets: bool,
    /// `y`
    pub sticky: bool,
}

impl Flags {
    /// `None` for an unknown or repeated flag letter, or for `u` together with `v`.
    pub fn parse(flags: &str) -> Option<Flags> {
        let mut out = Flags::default();
        let mut saw_u = false;
        for ch in flags.chars() {
            let slot = match ch {
                'd' => &mut out.has_indices,
                'g' => &mut out.global,
                'i' => &mut out.ignore_case,
                'm' => &mut out.multiline,
                's' => &mut out.dot_all,
                'u' => &mut saw_u,
                'v' => &mut out.unicode_sets,
                'y' => &mut out.sticky,
                _ => return None,
            };
            if *slot {
                return None;
            }
            *slot = true;
        }
        if saw_u && out.unicode_sets {
            return None;
        }
        out.unicode = saw_u || out.unicode_sets;
        Some(out)
    }

    /// Canonical order "dgimsuvy" (what `RegExp.prototype.flags` returns).
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        let mut s = String::new();
        if self.has_indices {
            s.push('d');
        }
        if self.global {
            s.push('g');
        }
        if self.ignore_case {
            s.push('i');
        }
        if self.multiline {
            s.push('m');
        }
        if self.dot_all {
            s.push('s');
        }
        if self.unicode_sets {
            s.push('v');
        } else if self.unicode {
            s.push('u');
        }
        if self.sticky {
            s.push('y');
        }
        s
    }
}

/// A pattern that is not a valid regular expression. `offset` is in pattern code units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    pub offset: usize,
}

impl SyntaxError {
    pub(crate) fn new(message: &str, offset: usize) -> SyntaxError {
        SyntaxError { message: message.to_string(), offset }
    }
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid regular expression: {} (at offset {})", self.message, self.offset)
    }
}

impl std::error::Error for SyntaxError {}

/// Why a match attempt was abandoned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecError {
    /// The caller's step budget ran out.
    StepLimit,
    /// The backtrack stack reached its memory cap.
    StackLimit,
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ExecError::StepLimit => "regular expression step limit exceeded",
            ExecError::StackLimit => "regular expression backtrack stack limit exceeded",
        })
    }
}

impl std::error::Error for ExecError {}

/// "Unset" marker for a capture slot.
pub(crate) const NONE: u32 = u32::MAX;

/// The result of a successful match: group 0 is the whole match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Captures {
    /// Two slots (start, end) per group, `NONE` when the group did not participate.
    slots: Vec<u32>,
}

impl Captures {
    /// `group_count() + 1`.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.slots.len() / 2
    }

    /// `[start, end)` of a group in code units, `None` if it is unset or out of range.
    pub fn get(&self, group: usize) -> Option<(usize, usize)> {
        let i = group.checked_mul(2)?;
        let start = *self.slots.get(i)?;
        let end = *self.slots.get(i + 1)?;
        if start == NONE || end == NONE {
            return None;
        }
        Some((start as usize, end as usize))
    }
}

/// A compiled regular expression. Immutable, `Send + Sync`.
#[derive(Debug)]
pub struct Regex {
    program: program::Program,
    flags: Flags,
    names: Vec<(String, usize)>,
}

impl Regex {
    /// Compile `pattern` (UTF-16 code units) with `flags`.
    pub fn new(pattern: &[u16], flags: Flags) -> Result<Regex, SyntaxError> {
        let ast = parser::parse(pattern, flags)?;
        let program = compile::compile(&ast, flags)?;
        Ok(Regex { program, flags, names: ast.names })
    }

    pub fn flags(&self) -> Flags {
        self.flags
    }

    /// Number of capture groups, not counting group 0.
    pub fn group_count(&self) -> usize {
        self.program.groups
    }

    /// (name, 1-based group index) for every named group, in source order.
    pub fn group_names(&self) -> &[(String, usize)] {
        &self.names
    }

    /// Find the leftmost match at or after code-unit index `start` (with `sticky`: only at
    /// `start`). `start > text.len()` is `Ok(None)`. `global` and `lastIndex` are the caller's
    /// business. `steps` is the remaining budget; it is decremented as the matcher works.
    ///
    /// One step is one matcher instruction. Instructions that do more work are charged for
    /// it: a single-character loop costs a step per character, a backreference a step per
    /// code unit compared. Skipping to the next possible start position costs nothing.
    /// At zero the result is `ExecError::StepLimit` and `*steps` is left at 0.
    ///
    /// In Unicode mode a `start` between the halves of a surrogate pair is moved back to the
    /// pair's first half, as the specification requires, so a match may begin at `start - 1`.
    pub fn exec(&self, text: &[u16], start: usize, steps: &mut u64) -> Result<Option<Captures>, ExecError> {
        let slots = exec::exec(&self.program, text, start, self.flags.sticky, steps)?;
        Ok(slots.map(|slots| Captures { slots }))
    }

    /// Approximate heap and inline size of the compiled expression.
    pub fn memory_bytes(&self) -> usize {
        let names: usize = self.names.iter().map(|(n, _)| n.len() + std::mem::size_of::<(String, usize)>()).sum();
        std::mem::size_of::<Regex>() + self.program.memory_bytes() + names
    }
}
