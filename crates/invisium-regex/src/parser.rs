//! Pattern parser: ECMAScript syntax (with the Annex B leniency when `u`/`v` is absent) to the
//! arena AST. Nesting is handled with an explicit stack of open groups, so pattern depth never
//! becomes call depth.

use crate::ast::{Ast, Node, NodeId, INF};
use crate::charset::CharSet;
use crate::{Flags, SyntaxError};
use std::collections::HashMap;

/// Deepest allowed nesting of parenthesised groups.
const MAX_NESTING: usize = 1000;
/// Capture-group indices must fit the 16-bit operands of the reset instruction.
const MAX_GROUPS: u32 = 0xFFFE;
/// Cap on AST nodes, so a huge pattern is refused before it costs much memory. A program
/// within the compiler's size cap never needs more.
const MAX_NODES: usize = 1 << 19;

pub(crate) fn parse(pattern: &[u16], flags: Flags) -> Result<Ast, SyntaxError> {
    let unicode = flags.unicode || flags.unicode_sets;
    let (total_groups, has_names) = prescan(pattern);
    let mut p = Parser {
        pat: pattern,
        pos: 0,
        unicode,
        icase: flags.ignore_case,
        vmode: flags.unicode_sets,
        named: unicode || has_names,
        total_groups,
        nodes: Vec::new(),
        nullable: Vec::new(),
        sets: Vec::new(),
        set_index: HashMap::new(),
        group_count: 0,
        names: Vec::new(),
        name_index: HashMap::new(),
        named_refs: Vec::new(),
    };
    let root = p.parse_pattern()?;
    p.resolve_named_refs()?;
    Ok(Ast { nodes: p.nodes, nullable: p.nullable, sets: p.sets, root, group_count: p.group_count, names: p.names })
}

/// Count the capturing groups of the whole pattern and note whether any is named. Both are
/// needed before parsing: `\N` is a backreference only if group N exists somewhere, and `\k`
/// is a named reference only if the pattern has a named group.
fn prescan(pat: &[u16]) -> (u32, bool) {
    let at = |i: usize| pat.get(i).copied().unwrap_or(0);
    let (mut groups, mut named, mut in_class, mut i) = (0u32, false, false, 0usize);
    while i < pat.len() {
        let c = at(i);
        if c == b'\\' as u16 {
            i += 2;
            continue;
        }
        if in_class {
            in_class = c != b']' as u16;
        } else if c == b'[' as u16 {
            in_class = true;
        } else if c == b'(' as u16 {
            if at(i + 1) != b'?' as u16 {
                groups = groups.saturating_add(1);
            } else if at(i + 2) == b'<' as u16 && at(i + 3) != b'=' as u16 && at(i + 3) != b'!' as u16 {
                groups = groups.saturating_add(1);
                named = true;
            }
        }
        i += 1;
    }
    (groups, named)
}

enum Kind {
    Root,
    Capture(u32),
    NonCapture,
    Look { behind: bool, negate: bool },
}

/// An open group: finished alternatives plus the terms of the alternative being read.
struct Frame {
    kind: Kind,
    alts: Vec<NodeId>,
    terms: Vec<NodeId>,
    start: usize,
    groups_before: u32,
}

pub(crate) struct Parser<'a> {
    pub(crate) pat: &'a [u16],
    pub(crate) pos: usize,
    pub(crate) unicode: bool,
    pub(crate) icase: bool,
    pub(crate) vmode: bool,
    /// `\k` is a named backreference (Unicode mode, or the pattern has a named group).
    pub(crate) named: bool,
    pub(crate) total_groups: u32,
    nodes: Vec<Node>,
    nullable: Vec<bool>,
    sets: Vec<CharSet>,
    set_index: HashMap<CharSet, u32>,
    group_count: u32,
    names: Vec<(String, usize)>,
    name_index: HashMap<String, u32>,
    pub(crate) named_refs: Vec<(NodeId, String, usize)>,
}

impl Parser<'_> {
    pub(crate) fn peek(&self) -> Option<u16> {
        self.pat.get(self.pos).copied()
    }

    pub(crate) fn peek_at(&self, ahead: usize) -> Option<u16> {
        self.pat.get(self.pos.checked_add(ahead)?).copied()
    }

    pub(crate) fn peek_is(&self, ch: char) -> bool {
        self.peek() == Some(ch as u16)
    }

    pub(crate) fn eat(&mut self, ch: char) -> bool {
        let hit = self.peek_is(ch);
        if hit {
            self.pos += 1;
        }
        hit
    }

    pub(crate) fn err<T>(&self, message: &str, offset: usize) -> Result<T, SyntaxError> {
        Err(SyntaxError::new(message, offset))
    }

    pub(crate) fn add(&mut self, node: Node, nullable: bool) -> NodeId {
        self.nodes.push(node);
        self.nullable.push(nullable);
        (self.nodes.len() - 1) as NodeId
    }

    /// Add a class node; equal sets are stored once (`set` must be normalized).
    pub(crate) fn add_class(&mut self, set: CharSet, negate: bool) -> NodeId {
        let index = match self.set_index.get(&set) {
            Some(&index) => index,
            None => {
                let index = self.sets.len() as u32;
                self.sets.push(set.clone());
                self.set_index.insert(set, index);
                index
            }
        };
        self.add(Node::Class { set: index, negate }, false)
    }

    fn nullable(&self, id: NodeId) -> bool {
        self.nullable.get(id as usize).copied().unwrap_or(true)
    }

    fn parse_pattern(&mut self) -> Result<NodeId, SyntaxError> {
        let root = Frame { kind: Kind::Root, alts: Vec::new(), terms: Vec::new(), start: 0, groups_before: 0 };
        let mut stack = vec![root];
        while let Some(c) = self.peek() {
            if self.nodes.len() > MAX_NODES {
                return self.err("regular expression too large", self.pos);
            }
            let c = char::from_u32(c as u32).unwrap_or('\u{FFFD}');
            match c {
                '|' => {
                    self.pos += 1;
                    let terms = stack.last_mut().map(|top| std::mem::take(&mut top.terms)).unwrap_or_default();
                    let seq = self.make_seq(terms);
                    if let Some(top) = stack.last_mut() {
                        top.alts.push(seq);
                    }
                }
                '(' => {
                    let start = self.pos;
                    self.pos += 1;
                    if stack.len() > MAX_NESTING {
                        return self.err("too many nested groups", start);
                    }
                    let groups_before = self.group_count;
                    let kind = self.parse_group_kind(start)?;
                    stack.push(Frame { kind, alts: Vec::new(), terms: Vec::new(), start, groups_before });
                }
                ')' => {
                    if stack.len() < 2 {
                        return self.err("unmatched ')'", self.pos);
                    }
                    self.pos += 1;
                    let Some(frame) = stack.pop() else { break };
                    let body = self.finish_alternatives(frame.alts, frame.terms);
                    let body_nullable = self.nullable(body);
                    let (node, quantifiable) = match frame.kind {
                        Kind::Root | Kind::NonCapture => (body, true),
                        Kind::Capture(index) => (self.add(Node::Capture(body, index), body_nullable), true),
                        Kind::Look { behind, negate } => {
                            // Annex B: a lookahead may be quantified, but only without `u`.
                            (self.add(Node::Look { child: body, behind, negate }, true), !behind && !self.unicode)
                        }
                    };
                    let node = self.parse_quantifier(node, quantifiable, frame.groups_before)?;
                    if let Some(top) = stack.last_mut() {
                        top.terms.push(node);
                    }
                }
                _ => {
                    let groups_before = self.group_count;
                    let (node, quantifiable) = self.parse_atom(c)?;
                    let node = self.parse_quantifier(node, quantifiable, groups_before)?;
                    if let Some(top) = stack.last_mut() {
                        top.terms.push(node);
                    }
                }
            }
        }
        if stack.len() != 1 {
            let start = stack.last().map_or(0, |f| f.start);
            return self.err("unterminated group", start);
        }
        match stack.pop() {
            Some(frame) => Ok(self.finish_alternatives(frame.alts, frame.terms)),
            None => Ok(self.add(Node::Empty, true)),
        }
    }

    fn make_seq(&mut self, terms: Vec<NodeId>) -> NodeId {
        match terms.as_slice() {
            [] => self.add(Node::Empty, true),
            [only] => *only,
            _ => {
                let nullable = terms.iter().all(|&t| self.nullable(t));
                self.add(Node::Seq(terms), nullable)
            }
        }
    }

    fn finish_alternatives(&mut self, mut alts: Vec<NodeId>, terms: Vec<NodeId>) -> NodeId {
        let last = self.make_seq(terms);
        if alts.is_empty() {
            return last;
        }
        alts.push(last);
        let nullable = alts.iter().any(|&a| self.nullable(a));
        self.add(Node::Alt(alts), nullable)
    }

    /// Called just after `(`.
    fn parse_group_kind(&mut self, start: usize) -> Result<Kind, SyntaxError> {
        let mut name = None;
        if self.eat('?') {
            if self.eat(':') {
                return Ok(Kind::NonCapture);
            } else if self.eat('=') {
                return Ok(Kind::Look { behind: false, negate: false });
            } else if self.eat('!') {
                return Ok(Kind::Look { behind: false, negate: true });
            } else if self.eat('<') {
                if self.eat('=') {
                    return Ok(Kind::Look { behind: true, negate: false });
                } else if self.eat('!') {
                    return Ok(Kind::Look { behind: true, negate: true });
                }
                name = Some(self.parse_group_name()?);
            } else {
                return self.err("invalid group", start);
            }
        }
        if self.group_count >= MAX_GROUPS {
            return self.err("too many capture groups", start);
        }
        self.group_count += 1;
        let index = self.group_count;
        if let Some(name) = name {
            if self.name_index.insert(name.clone(), index).is_some() {
                return self.err("duplicate capture group name", start);
            }
            self.names.push((name, index as usize));
        }
        Ok(Kind::Capture(index))
    }

    fn parse_atom(&mut self, c: char) -> Result<(NodeId, bool), SyntaxError> {
        let start = self.pos;
        let simple = match c {
            '^' => Some((Node::LineStart, true, false)),
            '$' => Some((Node::LineEnd, true, false)),
            '.' => Some((Node::Any, false, true)),
            _ => None,
        };
        if let Some((node, nullable, quantifiable)) = simple {
            self.pos += 1;
            return Ok((self.add(node, nullable), quantifiable));
        }
        match c {
            '[' => return Ok((self.parse_class()?, true)),
            '\\' => return self.parse_atom_escape(),
            '*' | '+' | '?' => return self.err("nothing to repeat", start),
            '{' if self.unicode => return self.err("lone quantifier brackets", start),
            '{' if self.braces().is_some() => return self.err("nothing to repeat", start),
            '}' if self.unicode => return self.err("lone quantifier brackets", start),
            ']' if self.unicode => return self.err("unmatched ']'", start),
            _ => {}
        }
        let ch = self.next_char().unwrap_or(0);
        Ok((self.add(Node::Char(ch), false), true))
    }

    /// Wrap `node` in a `Repeat` if a quantifier follows it.
    fn parse_quantifier(&mut self, node: NodeId, quantifiable: bool, groups_before: u32) -> Result<NodeId, SyntaxError> {
        let start = self.pos;
        let (min, max) = if self.peek_is('*') {
            (0, INF)
        } else if self.peek_is('+') {
            (1, INF)
        } else if self.peek_is('?') {
            (0, 1)
        } else if self.peek_is('{') {
            match self.braces() {
                Some((min, max, end)) => {
                    self.pos = end - 1;
                    (min, max)
                }
                // Not a quantifier: the atom parser decides (literal, or an error with `u`).
                None => return Ok(node),
            }
        } else {
            return Ok(node);
        };
        self.pos += 1;
        if !quantifiable {
            return self.err("nothing to repeat", start);
        }
        if min > max {
            return self.err("numbers out of order in {} quantifier", start);
        }
        let greedy = !self.eat('?');
        let nullable = min == 0 || self.nullable(node);
        let repeat = Node::Repeat { child: node, min, max, greedy, cap_from: groups_before, cap_to: self.group_count };
        Ok(self.add(repeat, nullable))
    }

    /// With `pos` at `{`: `(min, max, index after the closing brace)` if a well-formed
    /// `{n}`, `{n,}` or `{n,m}` starts here.
    fn braces(&self) -> Option<(u32, u32, usize)> {
        let (min, mut i) = self.decimal(self.pos + 1)?;
        let at = |i: usize| self.pat.get(i).copied().unwrap_or(0);
        if at(i) == b'}' as u16 {
            return Some((min, min, i + 1));
        }
        if at(i) != b',' as u16 {
            return None;
        }
        i += 1;
        if at(i) == b'}' as u16 {
            return Some((min, INF, i + 1));
        }
        let (max, i) = self.decimal(i)?;
        if at(i) == b'}' as u16 {
            return Some((min, max, i + 1));
        }
        None
    }

    /// One or more decimal digits starting at `from`: `(value clamped below INF, end index)`.
    pub(crate) fn decimal(&self, from: usize) -> Option<(u32, usize)> {
        let mut value = 0u64;
        let mut i = from;
        while let Some(d) = self.pat.get(i).and_then(|&c| char::from_u32(c as u32)).and_then(|c| c.to_digit(10)) {
            value = (value * 10 + d as u64).min((INF - 1) as u64);
            i += 1;
        }
        if i == from {
            return None;
        }
        Some((value as u32, i))
    }

    fn resolve_named_refs(&mut self) -> Result<(), SyntaxError> {
        for (id, name, offset) in std::mem::take(&mut self.named_refs) {
            let Some(&index) = self.name_index.get(&name) else {
                return self.err("invalid named reference", offset);
            };
            if let Some(slot) = self.nodes.get_mut(id as usize) {
                *slot = Node::BackRef(index);
            }
        }
        Ok(())
    }
}
