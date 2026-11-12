//! AST → instruction program. The walk uses an explicit task stack (no recursion); jump
//! targets are labels until a final pass turns them into instruction indices.

use crate::analyze;
use crate::ast::{Ast, Node, NodeId, INF};
use crate::charset::{CharSet, SetData};
use crate::program::{Atom, Inst, LoopInfo, Program, Rep, NO_REG};
use crate::unicode::{case_closure, is_surrogate};
use crate::{Flags, SyntaxError};
use std::collections::HashMap;

/// Cap on the memory of a compiled program.
const MAX_PROGRAM_BYTES: usize = 1 << 20;

enum Task {
    /// Compile a node, reading forwards (`false`) or backwards (`true`).
    Node(NodeId, bool),
    Emit(Inst),
    /// Bind a label to the next instruction.
    Bind(u32),
}

pub(crate) fn compile(ast: &Ast, flags: Flags) -> Result<Program, SyntaxError> {
    let unicode = flags.unicode || flags.unicode_sets;
    let mut c = Compiler {
        ast,
        icase: flags.ignore_case,
        unicode,
        multiline: flags.multiline,
        dot_all: flags.dot_all,
        insts: Vec::new(),
        sets: Vec::new(),
        set_index: HashMap::new(),
        class_index: HashMap::new(),
        char_index: HashMap::new(),
        reps: Vec::new(),
        loops: Vec::new(),
        regs: 0,
        labels: Vec::new(),
        bytes: std::mem::size_of::<Program>(),
    };
    let mut tasks = vec![Task::Node(ast.root, false)];
    while let Some(task) = tasks.pop() {
        match task {
            Task::Node(id, back) => c.node(id, back, &mut tasks)?,
            Task::Emit(inst) => c.emit(inst)?,
            Task::Bind(label) => c.bind(label),
        }
    }
    c.emit(Inst::Match)?;
    c.resolve_labels();
    let (prefilter, anchored) = analyze::prefilter(ast, flags);
    Ok(Program {
        insts: c.insts,
        sets: c.sets,
        reps: c.reps,
        loops: c.loops,
        regs: c.regs as usize,
        groups: ast.group_count as usize,
        unicode,
        icase: flags.ignore_case,
        prefilter,
        anchored,
    })
}

/// The set a class node matches: case closure first, then negation.
pub(crate) fn class_set(set: &CharSet, negate: bool, icase: bool, unicode: bool) -> CharSet {
    let closed = if icase { case_closure(set, unicode) } else { set.clone() };
    if negate {
        closed.negated()
    } else {
        closed
    }
}

struct Compiler<'a> {
    ast: &'a Ast,
    icase: bool,
    unicode: bool,
    multiline: bool,
    dot_all: bool,
    insts: Vec<Inst>,
    sets: Vec<SetData>,
    /// Final set → index in `sets`.
    set_index: HashMap<CharSet, u32>,
    /// Source class (AST set index, negated) → index in `sets`.
    class_index: HashMap<(u32, bool), u32>,
    char_index: HashMap<u32, Atom>,
    reps: Vec<Rep>,
    loops: Vec<LoopInfo>,
    regs: u32,
    labels: Vec<u32>,
    bytes: usize,
}

impl Compiler<'_> {
    fn grow(&mut self, bytes: usize) -> Result<(), SyntaxError> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > MAX_PROGRAM_BYTES {
            return Err(SyntaxError::new("regular expression too large", 0));
        }
        Ok(())
    }

    fn emit(&mut self, inst: Inst) -> Result<(), SyntaxError> {
        self.grow(std::mem::size_of::<Inst>())?;
        self.insts.push(inst);
        Ok(())
    }

    fn label(&mut self) -> u32 {
        self.labels.push(0);
        (self.labels.len() - 1) as u32
    }

    fn bind(&mut self, label: u32) {
        let here = self.insts.len() as u32;
        if let Some(slot) = self.labels.get_mut(label as usize) {
            *slot = here;
        }
    }

    fn reg(&mut self) -> u32 {
        self.regs += 1;
        self.regs - 1
    }

    fn resolve_labels(&mut self) {
        let labels = &self.labels;
        let target = |t: u32| labels.get(t as usize).copied().unwrap_or(0);
        for inst in &mut self.insts {
            match inst {
                Inst::Jmp(t) | Inst::SplitNext(t) | Inst::SplitJump(t) | Inst::LookStart(t) | Inst::NegLookStart(t) => {
                    *t = target(*t);
                }
                _ => {}
            }
        }
        for l in &mut self.loops {
            l.exit = target(l.exit);
        }
    }

    fn intern(&mut self, set: CharSet) -> Result<u32, SyntaxError> {
        if let Some(&i) = self.set_index.get(&set) {
            return Ok(i);
        }
        let data = SetData::new(&set);
        self.grow(data.memory_bytes())?;
        let i = self.sets.len() as u32;
        self.sets.push(data);
        self.set_index.insert(set, i);
        Ok(i)
    }

    fn class(&mut self, set: u32, negate: bool) -> Result<u32, SyntaxError> {
        if let Some(&i) = self.class_index.get(&(set, negate)) {
            return Ok(i);
        }
        let empty = CharSet::new();
        let source = self.ast.set(set).unwrap_or(&empty);
        let i = self.intern(class_set(source, negate, self.icase, self.unicode))?;
        self.class_index.insert((set, negate), i);
        Ok(i)
    }

    /// How to match the literal character `c`: a plain code unit when possible, otherwise a
    /// set (case variants, or a code point that is not one plain unit in Unicode mode).
    fn char_atom(&mut self, c: u32) -> Result<Atom, SyntaxError> {
        if let Some(&atom) = self.char_index.get(&c) {
            return Ok(atom);
        }
        let mut atom = Atom::Unit(c);
        if self.icase {
            let closure = case_closure(&CharSet::single(c), self.unicode);
            if closure.single_member().is_none() {
                atom = Atom::Set(self.intern(closure)?);
            }
        }
        if atom == Atom::Unit(c) && self.unicode && (c > 0xFFFF || is_surrogate(c)) {
            atom = Atom::Set(self.intern(CharSet::single(c))?);
        }
        self.char_index.insert(c, atom);
        Ok(atom)
    }

    /// The node as a single-character atom, if it is one.
    fn simple_atom(&mut self, id: NodeId) -> Result<Option<Atom>, SyntaxError> {
        let ast = self.ast;
        Ok(match ast.node(id) {
            Node::Char(c) => Some(self.char_atom(*c)?),
            Node::Any => Some(if self.dot_all { Atom::AnyAll } else { Atom::Any }),
            Node::Class { set, negate } => Some(Atom::Set(self.class(*set, *negate)?)),
            _ => None,
        })
    }

    fn node(&mut self, id: NodeId, back: bool, tasks: &mut Vec<Task>) -> Result<(), SyntaxError> {
        let ast = self.ast;
        if let Some(atom) = self.simple_atom(id)? {
            return self.emit(match (atom, back) {
                (Atom::Unit(c), false) => Inst::Char(c),
                (Atom::Unit(c), true) => Inst::CharBack(c),
                (Atom::Set(i), false) => Inst::Set(i),
                (Atom::Set(i), true) => Inst::SetBack(i),
                (Atom::Any, false) => Inst::Any,
                (Atom::Any, true) => Inst::AnyBack,
                (Atom::AnyAll, false) => Inst::AnyAll,
                (Atom::AnyAll, true) => Inst::AnyAllBack,
            });
        }
        match ast.node(id) {
            Node::Empty | Node::Char(_) | Node::Any | Node::Class { .. } => {}
            Node::Seq(items) => {
                // Tasks pop in reverse, so push the item to run first last.
                if back {
                    tasks.extend(items.iter().map(|&i| Task::Node(i, back)));
                } else {
                    tasks.extend(items.iter().rev().map(|&i| Task::Node(i, back)));
                }
            }
            Node::Alt(alts) => {
                let end = self.label();
                tasks.push(Task::Bind(end));
                for (k, &alt) in alts.iter().enumerate().rev() {
                    if k + 1 == alts.len() {
                        tasks.push(Task::Node(alt, back));
                        continue;
                    }
                    let next = self.label();
                    tasks.push(Task::Bind(next));
                    tasks.push(Task::Emit(Inst::Jmp(end)));
                    tasks.push(Task::Node(alt, back));
                    tasks.push(Task::Emit(Inst::SplitNext(next)));
                }
            }
            Node::Capture(child, index) => {
                // Backwards, the end of the group is reached first.
                let (first, second) = if back { (2 * index + 1, 2 * index) } else { (2 * index, 2 * index + 1) };
                self.emit(Inst::Save(first))?;
                tasks.push(Task::Emit(Inst::Save(second)));
                tasks.push(Task::Node(*child, back));
            }
            Node::Look { child, behind, negate } => {
                let end = self.label();
                self.emit(if *negate { Inst::NegLookStart(end) } else { Inst::LookStart(end) })?;
                tasks.push(Task::Bind(end));
                tasks.push(Task::Emit(Inst::LookEnd));
                tasks.push(Task::Node(*child, *behind));
            }
            Node::LineStart => self.emit(if self.multiline { Inst::LineStartMulti } else { Inst::LineStart })?,
            Node::LineEnd => self.emit(if self.multiline { Inst::LineEndMulti } else { Inst::LineEnd })?,
            Node::WordBoundary => self.emit(Inst::WordBoundary)?,
            Node::NotWordBoundary => self.emit(Inst::NotWordBoundary)?,
            Node::BackRef(n) => self.emit(if back { Inst::BackRefBack(*n) } else { Inst::BackRef(*n) })?,
            Node::Repeat { child, min, max, greedy, cap_from, cap_to } => {
                self.repeat(*child, (*min, *max), *greedy, (*cap_from, *cap_to), back, tasks)?;
            }
        }
        Ok(())
    }

    fn repeat(
        &mut self,
        child: NodeId,
        (min, max): (u32, u32),
        greedy: bool,
        (cap_from, cap_to): (u32, u32),
        back: bool,
        tasks: &mut Vec<Task>,
    ) -> Result<(), SyntaxError> {
        if max == 0 {
            return Ok(());
        }
        if let Some(atom) = self.simple_atom(child)? {
            self.grow(std::mem::size_of::<Rep>())?;
            self.reps.push(Rep { atom, min, max, greedy, back });
            return self.emit(Inst::Rep((self.reps.len() - 1) as u32));
        }
        let nullable = self.ast.is_nullable(child);
        // Captures inside the body are unset at the start of every iteration; a body that can
        // run only once has nothing to unset.
        let reset = (cap_to > cap_from && max > 1).then(|| Inst::ResetCaps((cap_from + 1) as u16, (cap_to + 1) as u16));
        let body = Task::Node(child, back);
        match (min, max) {
            (1, 1) => tasks.push(body),
            (0, 1) | (0, INF) => {
                let (head, exit) = (self.label(), self.label());
                self.bind(head);
                self.emit(if greedy { Inst::SplitNext(exit) } else { Inst::SplitJump(exit) })?;
                if let Some(reset) = reset {
                    self.emit(reset)?;
                }
                tasks.push(Task::Bind(exit));
                if max == INF {
                    tasks.push(Task::Emit(Inst::Jmp(head)));
                }
                // An optional iteration that matches nothing is rejected.
                if nullable {
                    let reg = self.reg();
                    self.emit(Inst::PosSave(reg))?;
                    tasks.push(Task::Emit(Inst::EmptyCheck(reg)));
                }
                tasks.push(body);
            }
            (1, INF) if !nullable => {
                let head = self.label();
                self.bind(head);
                if let Some(reset) = reset {
                    self.emit(reset)?;
                }
                tasks.push(Task::Emit(if greedy { Inst::SplitJump(head) } else { Inst::SplitNext(head) }));
                tasks.push(body);
            }
            _ => {
                let reg = self.reg();
                let pos_reg = if nullable { self.reg() } else { NO_REG };
                let exit = self.label();
                self.emit(Inst::CounterInit(reg))?;
                self.grow(std::mem::size_of::<LoopInfo>())?;
                let head = self.insts.len() as u32;
                self.loops.push(LoopInfo { reg, pos_reg, min, max, greedy, head, exit });
                let index = (self.loops.len() - 1) as u32;
                self.emit(Inst::LoopHead(index))?;
                if let Some(reset) = reset {
                    self.emit(reset)?;
                }
                if nullable {
                    self.emit(Inst::PosSave(pos_reg))?;
                }
                tasks.push(Task::Bind(exit));
                tasks.push(Task::Emit(Inst::LoopTail(index)));
                tasks.push(body);
            }
        }
        Ok(())
    }
}
