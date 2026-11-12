//! The backtracking instruction loop. All state that must survive a failed branch lives on the
//! explicit `Frame` stack, so neither the pattern nor the subject can deepen the call stack.

use crate::exec::{charge, Frame, Matcher};
use crate::program::{Inst, NO_REG};
use crate::unicode::{is_high_surrogate, is_line_terminator, is_low_surrogate};
use crate::{ExecError, NONE};

impl Matcher<'_> {
    /// Try to match with the match starting exactly at `start`. On success the capture slots
    /// are in `self.caps`; on failure they are all unset again.
    pub(crate) fn run(&mut self, start: usize, steps: &mut u64) -> Result<bool, ExecError> {
        let prog = self.prog;
        let text = self.text;
        let len = text.len();
        let unicode = prog.unicode;
        let (mut pc, mut pos) = (0usize, start);
        self.stack.clear();
        loop {
            charge(steps, 1)?;
            let Some(&inst) = prog.insts.get(pc) else { return Ok(false) };
            let ok = match inst {
                Inst::Match => {
                    if let [first, second, ..] = self.caps.as_mut_slice() {
                        *first = start as u32;
                        *second = pos as u32;
                    }
                    return Ok(true);
                }
                Inst::Char(c) => {
                    let hit = pos < len && text[pos] as u32 == c;
                    pos += hit as usize;
                    hit
                }
                Inst::CharBack(c) => {
                    let hit = pos > 0 && text[pos - 1] as u32 == c;
                    pos -= hit as usize;
                    hit
                }
                Inst::Any | Inst::AnyAll | Inst::Set(_) => match self.read(pos) {
                    Some((c, next)) if self.class_inst(inst, c) => {
                        pos = next;
                        true
                    }
                    _ => false,
                },
                Inst::AnyBack | Inst::AnyAllBack | Inst::SetBack(_) => match self.read_back(pos) {
                    Some((c, next)) if self.class_inst(inst, c) => {
                        pos = next;
                        true
                    }
                    _ => false,
                },
                Inst::Jmp(target) => {
                    pc = target as usize;
                    continue;
                }
                Inst::SplitNext(target) => {
                    self.push(Frame::Choice { pc: target, pos: pos as u32 })?;
                    true
                }
                Inst::SplitJump(target) => {
                    self.push(Frame::Choice { pc: pc as u32 + 1, pos: pos as u32 })?;
                    pc = target as usize;
                    continue;
                }
                Inst::Save(slot) => {
                    self.set_cap(slot as usize, pos as u32)?;
                    true
                }
                Inst::ResetCaps(from, to) => {
                    let (a, b) = (2 * from as usize, 2 * to as usize);
                    charge(steps, b.saturating_sub(a) as u64)?;
                    for slot in a..b {
                        if self.caps.get(slot).is_some_and(|&v| v != NONE) {
                            self.set_cap(slot, NONE)?;
                        }
                    }
                    true
                }
                Inst::LineStart => pos == 0,
                Inst::LineEnd => pos == len,
                Inst::LineStartMulti => pos == 0 || self.line_terminator_at(pos - 1),
                Inst::LineEndMulti => pos == len || self.line_terminator_at(pos),
                Inst::WordBoundary => self.at_word_boundary(pos),
                Inst::NotWordBoundary => !self.at_word_boundary(pos),
                Inst::BackRef(n) | Inst::BackRefBack(n) => {
                    let slot = |i: usize| self.caps.get(i).copied().unwrap_or(NONE);
                    let (s, e) = (slot(2 * n as usize), slot(2 * n as usize + 1));
                    if s == NONE || e == NONE {
                        // A group that has not matched (yet) is the empty string.
                        true
                    } else {
                        charge(steps, e.saturating_sub(s) as u64)?;
                        match self.backref(s as usize, e as usize, pos, matches!(inst, Inst::BackRefBack(_))) {
                            Some(next) => {
                                pos = next;
                                true
                            }
                            None => false,
                        }
                    }
                }
                Inst::LookStart(cont) => {
                    self.push(Frame::Look { cont, pos: pos as u32, negate: false })?;
                    true
                }
                Inst::NegLookStart(cont) => {
                    self.push(Frame::Look { cont, pos: pos as u32, negate: true })?;
                    true
                }
                Inst::LookEnd => match self.look_end(steps)? {
                    Some(entered_at) => {
                        pos = entered_at;
                        true
                    }
                    None => false,
                },
                Inst::Rep(index) => {
                    let Some(&rep) = prog.reps.get(index as usize) else { return Ok(false) };
                    // Greedy: take as many as allowed. Lazy: only the mandatory ones.
                    let want = if rep.greedy { rep.max } else { rep.min };
                    let (mut count, mut p, mut limit) = (0u32, pos, pos);
                    while count < want {
                        let Some((c, next)) = (if rep.back { self.read_back(p) } else { self.read(p) }) else { break };
                        if !self.atom_matches(rep.atom, c) {
                            break;
                        }
                        p = next;
                        count += 1;
                        if count == rep.min {
                            limit = p;
                        }
                    }
                    charge(steps, count as u64)?;
                    if count < rep.min {
                        false
                    } else {
                        if rep.greedy && p != limit {
                            let (pc, pos, limit) = (pc as u32, p as u32, limit as u32);
                            self.push(if rep.back { Frame::GreedyBack { pc, pos, limit } } else { Frame::Greedy { pc, pos, limit } })?;
                        } else if !rep.greedy && rep.min < rep.max {
                            self.push(Frame::Lazy { pc: pc as u32, pos: p as u32, count })?;
                        }
                        pos = p;
                        true
                    }
                }
                Inst::CounterInit(reg) => {
                    self.set_reg(reg as usize, 0)?;
                    true
                }
                Inst::PosSave(reg) => {
                    self.set_reg(reg as usize, pos as u32)?;
                    true
                }
                Inst::EmptyCheck(reg) => self.reg(reg) != pos as u32,
                Inst::LoopHead(index) => {
                    let Some(&l) = prog.loops.get(index as usize) else { return Ok(false) };
                    let count = self.reg(l.reg);
                    if count < l.min {
                        true
                    } else if count >= l.max {
                        pc = l.exit as usize;
                        continue;
                    } else if l.greedy {
                        self.push(Frame::Choice { pc: l.exit, pos: pos as u32 })?;
                        true
                    } else {
                        self.push(Frame::Choice { pc: pc as u32 + 1, pos: pos as u32 })?;
                        pc = l.exit as usize;
                        continue;
                    }
                }
                Inst::LoopTail(index) => {
                    let Some(&l) = prog.loops.get(index as usize) else { return Ok(false) };
                    let count = self.reg(l.reg);
                    // Once the minimum is met, an iteration that consumed nothing is rejected.
                    if l.pos_reg != NO_REG && count >= l.min && self.reg(l.pos_reg) == pos as u32 {
                        false
                    } else {
                        self.set_reg(l.reg as usize, count.saturating_add(1))?;
                        pc = l.head as usize;
                        continue;
                    }
                }
            };
            if ok {
                pc += 1;
                continue;
            }
            // Backtrack: undo writes until a frame offers another way forward.
            loop {
                let Some(frame) = self.stack.pop() else { return Ok(false) };
                match frame {
                    Frame::Cap { slot, old } => self.restore_cap(slot, old),
                    Frame::Reg { reg, old } => self.restore_reg(reg, old),
                    Frame::Choice { pc: p, pos: q } => {
                        (pc, pos) = (p as usize, q as usize);
                        break;
                    }
                    Frame::Greedy { pc: p, pos: q, limit } => {
                        let (q, limit) = (q as usize, limit as usize);
                        let pair = unicode && q >= limit + 2 && is_low_surrogate(text[q - 1] as u32) && is_high_surrogate(text[q - 2] as u32);
                        let q = q - if pair { 2 } else { 1 };
                        if q > limit {
                            self.stack.push(Frame::Greedy { pc: p, pos: q as u32, limit: limit as u32 });
                        }
                        (pc, pos) = (p as usize + 1, q);
                        break;
                    }
                    Frame::GreedyBack { pc: p, pos: q, limit } => {
                        let (q, limit) = (q as usize, limit as usize);
                        let pair = unicode && q + 2 <= limit && is_high_surrogate(text[q] as u32) && is_low_surrogate(text[q + 1] as u32);
                        let q = q + if pair { 2 } else { 1 };
                        if q < limit {
                            self.stack.push(Frame::GreedyBack { pc: p, pos: q as u32, limit: limit as u32 });
                        }
                        (pc, pos) = (p as usize + 1, q);
                        break;
                    }
                    Frame::Lazy { pc: p, pos: q, count } => {
                        let Some(Inst::Rep(index)) = prog.insts.get(p as usize) else { continue };
                        let Some(&rep) = prog.reps.get(*index as usize) else { continue };
                        let read = if rep.back { self.read_back(q as usize) } else { self.read(q as usize) };
                        let Some((c, next)) = read else { continue };
                        if !self.atom_matches(rep.atom, c) {
                            continue;
                        }
                        if count + 1 < rep.max {
                            self.stack.push(Frame::Lazy { pc: p, pos: next as u32, count: count + 1 });
                        }
                        (pc, pos) = (p as usize + 1, next);
                        break;
                    }
                    Frame::Look { cont, pos: q, negate } => {
                        // The body failed, which is success for a negative lookaround.
                        if negate {
                            (pc, pos) = (cont as usize, q as usize);
                            break;
                        }
                    }
                }
            }
        }
    }

    fn class_inst(&self, inst: Inst, c: u32) -> bool {
        match inst {
            Inst::Any | Inst::AnyBack => !is_line_terminator(c),
            Inst::Set(i) | Inst::SetBack(i) => self.prog.sets.get(i as usize).is_some_and(|s| s.contains(c)),
            _ => true,
        }
    }

    fn reg(&self, reg: u32) -> u32 {
        self.regs.get(reg as usize).copied().unwrap_or(0)
    }

    fn set_reg(&mut self, reg: usize, value: u32) -> Result<(), ExecError> {
        let Some(&old) = self.regs.get(reg) else { return Ok(()) };
        self.push(Frame::Reg { reg: reg as u32, old })?;
        self.regs[reg] = value;
        Ok(())
    }

    fn restore_reg(&mut self, reg: u32, old: u32) {
        if let Some(slot) = self.regs.get_mut(reg as usize) {
            *slot = old;
        }
    }

    fn set_cap(&mut self, slot: usize, value: u32) -> Result<(), ExecError> {
        let Some(&old) = self.caps.get(slot) else { return Ok(()) };
        self.push(Frame::Cap { slot: slot as u32, old })?;
        self.caps[slot] = value;
        Ok(())
    }

    fn restore_cap(&mut self, slot: u32, old: u32) {
        if let Some(cell) = self.caps.get_mut(slot as usize) {
            *cell = old;
        }
    }

    /// The body of a lookaround matched. Positive: drop its choice points (it is atomic) but
    /// keep the capture undo records, and return the position it was entered at. Negative:
    /// undo everything it did and return `None` (the lookaround fails).
    fn look_end(&mut self, steps: &mut u64) -> Result<Option<usize>, ExecError> {
        let Some(mark) = self.stack.iter().rposition(|f| matches!(f, Frame::Look { .. })) else {
            return Ok(None);
        };
        let Some(&Frame::Look { pos, negate, .. }) = self.stack.get(mark) else { return Ok(None) };
        charge(steps, ((self.stack.len() - mark) / 8) as u64)?;
        if negate {
            while self.stack.len() > mark {
                match self.stack.pop() {
                    Some(Frame::Cap { slot, old }) => self.restore_cap(slot, old),
                    Some(Frame::Reg { reg, old }) => self.restore_reg(reg, old),
                    _ => {}
                }
            }
            return Ok(None);
        }
        let mut keep = mark;
        for i in mark + 1..self.stack.len() {
            let frame = self.stack[i];
            if matches!(frame, Frame::Cap { .. }) {
                self.stack[keep] = frame;
                keep += 1;
            }
        }
        self.stack.truncate(keep);
        Ok(Some(pos as usize))
    }
}
