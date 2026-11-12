//! The compiled form of a pattern: a flat instruction program plus side tables.

use crate::charset::SetData;

/// "No register" marker in [`LoopInfo::pos_reg`].
pub(crate) const NO_REG: u32 = u32::MAX;

/// One matcher instruction (8 bytes). Jump operands are instruction indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Inst {
    Match,
    /// One code unit, reading forwards / backwards.
    Char(u32),
    CharBack(u32),
    /// Any character but a line terminator.
    Any,
    AnyBack,
    /// Any character at all (`.` with the `s` flag).
    AnyAll,
    AnyAllBack,
    /// One character from `sets[i]`.
    Set(u32),
    SetBack(u32),
    Jmp(u32),
    /// Continue with the next instruction; resume at the target on backtrack.
    SplitNext(u32),
    /// Continue at the target; resume at the next instruction on backtrack.
    SplitJump(u32),
    /// Record the position in a capture slot.
    Save(u32),
    /// Unset both slots of the groups `from .. to`.
    ResetCaps(u16, u16),
    LineStart,
    LineEnd,
    LineStartMulti,
    LineEndMulti,
    WordBoundary,
    NotWordBoundary,
    BackRef(u32),
    BackRefBack(u32),
    /// Begin a positive lookaround; the operand is the instruction after its `LookEnd`.
    LookStart(u32),
    /// Begin a negative lookaround.
    NegLookStart(u32),
    /// The body of a lookaround matched.
    LookEnd,
    /// A quantified single character, described by `reps[i]`.
    Rep(u32),
    /// Zero a loop counter register.
    CounterInit(u32),
    /// Top of a counted loop (`loops[i]`): run the body, or leave.
    LoopHead(u32),
    /// Bottom of a counted loop: count the iteration and go back to the head.
    LoopTail(u32),
    /// Remember the position at the start of a loop iteration.
    PosSave(u32),
    /// Fail if the iteration that started at the remembered position matched nothing.
    EmptyCheck(u32),
}

/// The single character a [`Rep`] repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Atom {
    Unit(u32),
    Set(u32),
    Any,
    AnyAll,
}

/// A quantified single character: no per-iteration backtrack state is needed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rep {
    pub atom: Atom,
    pub min: u32,
    pub max: u32,
    pub greedy: bool,
    pub back: bool,
}

/// A loop with an iteration counter.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LoopInfo {
    pub reg: u32,
    /// Register holding the iteration's start position, or `NO_REG` if the body cannot be empty.
    pub pos_reg: u32,
    pub min: u32,
    pub max: u32,
    pub greedy: bool,
    pub head: u32,
    pub exit: u32,
}

/// A cheap test that rules out most start positions before the matcher runs.
#[derive(Debug)]
pub(crate) enum Prefilter {
    None,
    /// Every match starts with these code units.
    Literal(Vec<u16>),
    /// Every match starts with a code unit from this set.
    First(SetData),
}

#[derive(Debug)]
pub(crate) struct Program {
    pub insts: Vec<Inst>,
    pub sets: Vec<SetData>,
    pub reps: Vec<Rep>,
    pub loops: Vec<LoopInfo>,
    /// Number of counter / position registers.
    pub regs: usize,
    /// Number of capture groups, not counting group 0.
    pub groups: usize,
    pub unicode: bool,
    pub icase: bool,
    pub prefilter: Prefilter,
    /// The pattern can only match at the very start of the text.
    pub anchored: bool,
}

impl Program {
    pub(crate) fn memory_bytes(&self) -> usize {
        let prefilter = match &self.prefilter {
            Prefilter::None => 0,
            Prefilter::Literal(units) => units.len() * 2,
            Prefilter::First(set) => set.memory_bytes(),
        };
        std::mem::size_of::<Program>()
            + self.insts.len() * std::mem::size_of::<Inst>()
            + self.sets.iter().map(SetData::memory_bytes).sum::<usize>()
            + self.reps.len() * std::mem::size_of::<Rep>()
            + self.loops.len() * std::mem::size_of::<LoopInfo>()
            + prefilter
    }
}

#[cfg(test)]
mod tests {
    /// The size caps (1 MiB of program, 32 MiB of backtrack stack) assume these layouts.
    #[test]
    fn compact_layouts() {
        assert_eq!(std::mem::size_of::<super::Inst>(), 8);
        assert_eq!(std::mem::size_of::<crate::exec::Frame>(), 16);
    }
}
