//! Match driver: the scan over start positions, the prefilter, and the matcher's helpers.
//! The instruction loop itself is in `vm.rs`.

use crate::program::{Atom, Prefilter, Program};
use crate::unicode::{canon_legacy, canonicalize, combine_surrogates, is_ascii_word, is_high_surrogate, is_line_terminator, is_low_surrogate};
use crate::{ExecError, NONE};

/// One entry of the backtrack stack (16 bytes).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Frame {
    /// Resume at `pc` with the position `pos`.
    Choice { pc: u32, pos: u32 },
    /// Undo a capture-slot write.
    Cap { slot: u32, old: u32 },
    /// Undo a register write.
    Reg { reg: u32, old: u32 },
    /// A greedy single-character loop (the `Rep` at `pc`, reading forwards) that can still
    /// give characters back, from `pos` down to `limit`.
    Greedy { pc: u32, pos: u32, limit: u32 },
    /// The same for a loop reading backwards (gives back by moving forwards, up to `limit`).
    GreedyBack { pc: u32, pos: u32, limit: u32 },
    /// A lazy single-character loop that has matched `count` characters and may take more.
    Lazy { pc: u32, pos: u32, count: u32 },
    /// Start of a lookaround entered at `pos`; `cont` is the instruction after it.
    Look { cont: u32, pos: u32, negate: bool },
}

/// The backtrack stack may hold 32 MiB of frames.
const MAX_FRAMES: usize = (32 << 20) / std::mem::size_of::<Frame>();

pub(crate) struct Matcher<'a> {
    pub(crate) prog: &'a Program,
    pub(crate) text: &'a [u16],
    pub(crate) caps: Vec<u32>,
    pub(crate) regs: Vec<u32>,
    pub(crate) stack: Vec<Frame>,
}

/// Find the leftmost match at or after `start`; the result is the capture slots.
pub(crate) fn exec(prog: &Program, text: &[u16], start: usize, sticky: bool, steps: &mut u64) -> Result<Option<Vec<u32>>, ExecError> {
    if start > text.len() {
        return Ok(None);
    }
    // Positions are stored as u32 with u32::MAX reserved for "unset".
    if text.len() >= NONE as usize {
        return Err(ExecError::StackLimit);
    }
    let mut m = Matcher { prog, text, caps: vec![NONE; 2 * (prog.groups + 1)], regs: vec![0; prog.regs], stack: Vec::new() };
    let mut at = start;
    // In Unicode mode matching starts at the character containing `start`.
    if prog.unicode && m.mid_pair(at) {
        at -= 1;
    }
    loop {
        if !sticky {
            match m.next_candidate(at) {
                Some(p) => at = p,
                None => return Ok(None),
            }
        }
        if prog.anchored && at > 0 {
            return Ok(None);
        }
        if m.run(at, steps)? {
            return Ok(Some(std::mem::take(&mut m.caps)));
        }
        if sticky || at >= text.len() {
            return Ok(None);
        }
        at = m.read(at).map_or(at + 1, |(_, next)| next);
    }
}

pub(crate) fn charge(steps: &mut u64, n: u64) -> Result<(), ExecError> {
    if *steps < n {
        *steps = 0;
        return Err(ExecError::StepLimit);
    }
    *steps -= n;
    Ok(())
}

impl Matcher<'_> {
    /// The first position at or after `at` where the prefilter allows a match to start.
    fn next_candidate(&self, at: usize) -> Option<usize> {
        let text = self.text;
        match &self.prog.prefilter {
            Prefilter::None => Some(at),
            Prefilter::Literal(units) => {
                let first = *units.first()?;
                let mut i = at;
                loop {
                    let j = i + text.get(i..)?.iter().position(|&u| u == first)?;
                    if text.get(j..)?.starts_with(units) {
                        return Some(j);
                    }
                    i = j + 1;
                }
            }
            Prefilter::First(set) => {
                let mut i = at;
                while let Some(&u) = text.get(i) {
                    if set.contains(u as u32) && !(self.prog.unicode && self.mid_pair(i)) {
                        return Some(i);
                    }
                    i += 1;
                }
                None
            }
        }
    }

    /// Is `i` between the two halves of a surrogate pair?
    pub(crate) fn mid_pair(&self, i: usize) -> bool {
        i > 0
            && self.text.get(i).is_some_and(|&lo| is_low_surrogate(lo as u32))
            && self.text.get(i - 1).is_some_and(|&hi| is_high_surrogate(hi as u32))
    }

    /// The character starting at `pos` and the position after it.
    #[inline]
    pub(crate) fn read(&self, pos: usize) -> Option<(u32, usize)> {
        let c = *self.text.get(pos)? as u32;
        if self.prog.unicode && is_high_surrogate(c) {
            if let Some(&lo) = self.text.get(pos + 1) {
                if is_low_surrogate(lo as u32) {
                    return Some((combine_surrogates(c, lo as u32), pos + 2));
                }
            }
        }
        Some((c, pos + 1))
    }

    /// The character ending at `pos` and the position before it.
    #[inline]
    pub(crate) fn read_back(&self, pos: usize) -> Option<(u32, usize)> {
        let c = *self.text.get(pos.checked_sub(1)?)? as u32;
        if self.prog.unicode && is_low_surrogate(c) && pos >= 2 {
            if let Some(&hi) = self.text.get(pos - 2) {
                if is_high_surrogate(hi as u32) {
                    return Some((combine_surrogates(hi as u32, c), pos - 2));
                }
            }
        }
        Some((c, pos - 1))
    }

    #[inline]
    pub(crate) fn atom_matches(&self, atom: Atom, c: u32) -> bool {
        match atom {
            Atom::Unit(u) => c == u,
            Atom::Set(i) => self.prog.sets.get(i as usize).is_some_and(|s| s.contains(c)),
            Atom::Any => !is_line_terminator(c),
            Atom::AnyAll => true,
        }
    }

    fn is_word(&self, i: Option<usize>) -> bool {
        let Some(&u) = i.and_then(|i| self.text.get(i)) else { return false };
        let c = u as u32;
        // With `iu`, U+017F and U+212A fold to ASCII letters and count as word characters.
        is_ascii_word(c) || (self.prog.icase && self.prog.unicode && (c == 0x17F || c == 0x212A))
    }

    /// Is `pos` between a word character and a non-word character (or a text edge)?
    pub(crate) fn at_word_boundary(&self, pos: usize) -> bool {
        self.is_word(pos.checked_sub(1)) != self.is_word(Some(pos))
    }

    pub(crate) fn line_terminator_at(&self, i: usize) -> bool {
        self.text.get(i).is_some_and(|&u| is_line_terminator(u as u32))
    }

    /// Match the text of the capture `[s, e)` at `pos` (ending at `pos` when `back`).
    /// Returns the new position.
    pub(crate) fn backref(&self, s: usize, e: usize, pos: usize, back: bool) -> Option<usize> {
        let n = e.checked_sub(s)?;
        let (a, b) = if back { (pos.checked_sub(n)?, pos) } else { (pos, pos.checked_add(n)?) };
        let cap = self.text.get(s..e)?;
        let sub = self.text.get(a..b)?;
        let unicode = self.prog.unicode;
        // In Unicode mode the compared text must be whole characters.
        if unicode && (self.mid_pair(a) || self.mid_pair(b)) {
            return None;
        }
        if !self.prog.icase {
            if cap != sub {
                return None;
            }
        } else if unicode {
            let (mut i, mut j) = (s, a);
            while i < e {
                let (c1, ni) = self.read(i)?;
                let (c2, nj) = self.read(j)?;
                if ni - i != nj - j || (c1 != c2 && canonicalize(c1, true) != canonicalize(c2, true)) {
                    return None;
                }
                i = ni;
                j = nj;
            }
        } else if cap.iter().zip(sub).any(|(&x, &y)| x != y && canon_legacy(x as u32) != canon_legacy(y as u32)) {
            return None;
        }
        Some(if back { a } else { b })
    }

    #[inline]
    pub(crate) fn push(&mut self, frame: Frame) -> Result<(), ExecError> {
        if self.stack.len() == self.stack.capacity() {
            self.grow_stack()?;
        }
        self.stack.push(frame);
        Ok(())
    }

    #[cold]
    fn grow_stack(&mut self) -> Result<(), ExecError> {
        let len = self.stack.len();
        if len >= MAX_FRAMES {
            return Err(ExecError::StackLimit);
        }
        let target = (len * 2).clamp(64, MAX_FRAMES);
        self.stack.try_reserve_exact(target - len).map_err(|_| ExecError::StackLimit)
    }
}
