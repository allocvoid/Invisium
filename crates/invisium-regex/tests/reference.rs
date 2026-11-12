//! Differential test for the ECMAScript matching semantics. Random small patterns are matched
//! by the engine and by `Reference`, a direct transcription of the specification's
//! continuation-passing matcher (RepeatMatcher, capture reset, the empty check, backward
//! matching in lookbehind, backreferences). The two must agree on every capture.

mod common;

use common::{spans, u16s, NO};
use invisium_regex::{Flags, Regex};
use std::cell::Cell;

#[derive(Clone, Debug)]
enum Node {
    Char(u16),
    Any,
    Class(Vec<u16>, bool),
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    Group(Box<Node>, usize),
    Repeat { body: Box<Node>, min: u32, max: Option<u32>, greedy: bool, paren_index: usize, paren_count: usize },
    Look { body: Box<Node>, behind: bool, negate: bool },
    BackRef(usize),
    Start,
    End,
    WordB,
    NotWordB,
}

#[derive(Clone)]
struct State {
    end: usize,
    caps: Vec<Option<(usize, usize)>>,
}

type Cont<'a> = &'a mut dyn FnMut(State) -> Option<State>;

const REFERENCE_LIMIT: u32 = 300_000;

struct Reference<'t> {
    text: &'t [u16],
    work: Cell<u32>,
}

impl Reference<'_> {
    fn is_word(&self, i: Option<usize>) -> bool {
        i.and_then(|i| self.text.get(i)).is_some_and(|&u| u < 128 && ((u as u8).is_ascii_alphanumeric() || u == b'_' as u16))
    }

    /// One character matcher, forwards or backwards.
    fn character(&self, back: bool, x: State, c: Cont, accept: impl Fn(u16) -> bool) -> Option<State> {
        let at = if back { x.end.checked_sub(1)? } else { x.end };
        let unit = *self.text.get(at)?;
        if !accept(unit) {
            return None;
        }
        c(State { end: if back { at } else { at + 1 }, caps: x.caps })
    }

    fn run(&self, node: &Node, back: bool, x: State, c: Cont) -> Option<State> {
        self.work.set(self.work.get() + 1);
        if self.work.get() > REFERENCE_LIMIT {
            return None;
        }
        match node {
            Node::Char(ch) => self.character(back, x, c, |u| u == *ch),
            Node::Any => self.character(back, x, c, |u| u != b'\n' as u16),
            Node::Class(set, negate) => self.character(back, x, c, |u| set.contains(&u) != *negate),
            Node::Seq(items) => self.seq(items, back, x, c),
            Node::Alt(alts) => alts.iter().find_map(|alt| self.run(alt, back, x.clone(), &mut *c)),
            Node::Group(body, index) => {
                let entered = x.end;
                self.run(body, back, x, &mut |mut y: State| {
                    y.caps[*index] = Some(if back { (y.end, entered) } else { (entered, y.end) });
                    c(y)
                })
            }
            Node::Repeat { body, min, max, greedy, paren_index, paren_count } => {
                self.repeat(body, *min, *max, *greedy, (*paren_index, *paren_count), back, x, c)
            }
            Node::Look { body, behind, negate } => {
                let inner = self.run(body, *behind, x.clone(), &mut Some);
                match (inner, negate) {
                    (Some(y), false) => c(State { end: x.end, caps: y.caps }),
                    (None, true) => c(x),
                    _ => None,
                }
            }
            Node::BackRef(n) => {
                let Some((s, e)) = x.caps[*n] else { return c(x) };
                let len = e - s;
                let (from, to) = if back { (x.end.checked_sub(len)?, x.end) } else { (x.end, x.end + len) };
                if self.text.get(from..to)? != &self.text[s..e] {
                    return None;
                }
                c(State { end: if back { from } else { to }, caps: x.caps })
            }
            Node::Start => (x.end == 0).then_some(x).and_then(c),
            Node::End => (x.end == self.text.len()).then_some(x).and_then(c),
            Node::WordB | Node::NotWordB => {
                let boundary = self.is_word(x.end.checked_sub(1)) != self.is_word(Some(x.end));
                (boundary == matches!(node, Node::WordB)).then_some(x).and_then(c)
            }
        }
    }

    fn seq(&self, items: &[Node], back: bool, x: State, c: Cont) -> Option<State> {
        let split = if back { items.split_last() } else { items.split_first() };
        let Some((next, rest)) = split else { return c(x) };
        self.run(next, back, x, &mut |y| self.seq(rest, back, y, &mut *c))
    }

    /// RepeatMatcher, step for step.
    #[allow(clippy::too_many_arguments)]
    fn repeat(&self, body: &Node, min: u32, max: Option<u32>, greedy: bool, parens: (usize, usize), back: bool, x: State, c: Cont) -> Option<State> {
        if max == Some(0) {
            return c(x);
        }
        let entered = x.end;
        let d = |y: State, c: Cont| {
            if min == 0 && y.end == entered {
                return None;
            }
            self.repeat(body, min.saturating_sub(1), max.map(|m| m - 1), greedy, parens, back, y, c)
        };
        let mut xr = x.clone();
        for k in parens.0 + 1..=parens.0 + parens.1 {
            xr.caps[k] = None;
        }
        if min != 0 {
            return self.run(body, back, xr, &mut |y| d(y, &mut *c));
        }
        if !greedy {
            return c(x).or_else(|| self.run(body, back, xr, &mut |y| d(y, &mut *c)));
        }
        self.run(body, back, xr, &mut |y| d(y, &mut *c)).or_else(|| c(x))
    }

    /// The leftmost match, in the same notation as `common::spans`.
    fn exec(&self, root: &Node, groups: usize) -> String {
        for start in 0..=self.text.len() {
            let x = State { end: start, caps: vec![None; groups + 1] };
            if let Some(mut y) = self.run(root, false, x, &mut Some) {
                y.caps[0] = Some((start, y.end));
                let parts: Vec<String> = y.caps.iter().map(|c| c.map_or("-".to_string(), |(s, e)| format!("{s},{e}"))).collect();
                return parts.join(" ");
            }
        }
        NO.to_string()
    }
}

struct Gen {
    state: u64,
    groups: usize,
}

impl Gen {
    fn below(&mut self, n: u32) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        ((self.state >> 32) % n as u64) as u32
    }

    fn alt(&mut self, depth: u32) -> Node {
        let count = if self.below(4) == 0 { 2 } else { 1 };
        let mut alts: Vec<Node> = (0..count).map(|_| self.seq(depth)).collect();
        if alts.len() == 1 {
            alts.remove(0)
        } else {
            Node::Alt(alts)
        }
    }

    fn seq(&mut self, depth: u32) -> Node {
        let count = 1 + self.below(3);
        Node::Seq((0..count).map(|_| self.term(depth)).collect())
    }

    fn term(&mut self, depth: u32) -> Node {
        let before = self.groups;
        let letter = |g: &mut Gen| b"abc"[g.below(3) as usize] as u16;
        let kind = self.below(if depth == 0 { 9 } else { 14 });
        let (atom, quantifiable) = match kind {
            0..=3 => (Node::Char(letter(self)), true),
            4 => (Node::Any, true),
            5 => (Node::Class(vec![letter(self), letter(self)], self.below(3) == 0), true),
            6 => (Node::BackRef(1 + self.below(3) as usize), true),
            7 => (Node::Char(letter(self)), true),
            8 => ([Node::Start, Node::End, Node::WordB, Node::NotWordB][self.below(4) as usize].clone(), false),
            9..=11 => {
                self.groups += 1;
                let index = self.groups;
                (Node::Group(Box::new(self.alt(depth - 1)), index), true)
            }
            12 => (Node::Seq(vec![self.alt(depth - 1)]), true),
            _ => {
                let (behind, negate) = (self.below(2) == 0, self.below(3) == 0);
                (Node::Look { body: Box::new(self.alt(depth - 1)), behind, negate }, false)
            }
        };
        if !quantifiable || self.below(5) > 1 {
            return atom;
        }
        let (min, max) = [(0, None), (1, None), (0, Some(1)), (2, Some(2)), (1, Some(3)), (0, Some(2)), (2, None)][self.below(7) as usize];
        Node::Repeat { body: Box::new(atom), min, max, greedy: self.below(3) != 0, paren_index: before, paren_count: self.groups - before }
    }
}

/// Replace references to groups that do not exist (they would be octal escapes).
fn fix_backrefs(node: &mut Node, groups: usize) {
    match node {
        Node::BackRef(n) if *n > groups => *node = Node::Char(b'a' as u16),
        Node::Seq(items) | Node::Alt(items) => items.iter_mut().for_each(|n| fix_backrefs(n, groups)),
        Node::Group(body, _) | Node::Repeat { body, .. } | Node::Look { body, .. } => fix_backrefs(body, groups),
        _ => {}
    }
}

fn print(node: &Node, out: &mut String) {
    let wrapped = |n: &Node, out: &mut String| {
        out.push_str("(?:");
        print(n, out);
        out.push(')');
    };
    match node {
        Node::Char(c) => out.push(*c as u8 as char),
        Node::Any => out.push('.'),
        Node::Class(set, negate) => {
            out.push_str(if *negate { "[^" } else { "[" });
            out.extend(set.iter().map(|&u| u as u8 as char));
            out.push(']');
        }
        Node::Seq(items) => items.iter().for_each(|item| match item {
            Node::Alt(_) | Node::Seq(_) => wrapped(item, out),
            _ => print(item, out),
        }),
        Node::Alt(alts) => {
            for (i, alt) in alts.iter().enumerate() {
                if i > 0 {
                    out.push('|');
                }
                print(alt, out);
            }
        }
        Node::Group(body, _) => {
            out.push('(');
            print(body, out);
            out.push(')');
        }
        Node::Repeat { body, min, max, greedy, .. } => {
            match **body {
                Node::Char(_) | Node::Any | Node::Class(..) | Node::Group(..) | Node::BackRef(_) => print(body, out),
                _ => wrapped(body, out),
            }
            match (min, max) {
                (0, None) => out.push('*'),
                (1, None) => out.push('+'),
                (0, Some(1)) => out.push('?'),
                (n, None) => out.push_str(&format!("{{{n},}}")),
                (n, Some(m)) if n == m => out.push_str(&format!("{{{n}}}")),
                (n, Some(m)) => out.push_str(&format!("{{{n},{m}}}")),
            }
            if !greedy {
                out.push('?');
            }
        }
        Node::Look { body, behind, negate } => {
            out.push_str(match (behind, negate) {
                (false, false) => "(?=",
                (false, true) => "(?!",
                (true, false) => "(?<=",
                (true, true) => "(?<!",
            });
            print(body, out);
            out.push(')');
        }
        Node::BackRef(n) => out.push_str(&format!("\\{n}")),
        Node::Start => out.push('^'),
        Node::End => out.push('$'),
        Node::WordB => out.push_str("\\b"),
        Node::NotWordB => out.push_str("\\B"),
    }
}

#[test]
fn engine_agrees_with_the_specification_transcription() {
    let mut gen = Gen { state: 0x1234_5678_9ABC_DEF1, groups: 0 };
    let (mut compared, mut mismatches) = (0u32, Vec::new());
    for _ in 0..30_000 {
        gen.groups = 0;
        let mut root = gen.alt(3);
        let groups = gen.groups;
        fix_backrefs(&mut root, groups);
        let mut pattern = String::new();
        print(&root, &mut pattern);
        let re = Regex::new(&u16s(&pattern), Flags::default()).unwrap_or_else(|e| panic!("/{pattern}/: {e}"));
        assert_eq!(re.group_count(), groups, "/{pattern}/");
        for _ in 0..3 {
            let len = gen.below(9);
            let subject: Vec<u16> = (0..len).map(|_| b"aabbc \n"[gen.below(7) as usize] as u16).collect();
            let reference = Reference { text: &subject, work: Cell::new(0) };
            let expected = reference.exec(&root, groups);
            let mut steps = 2_000_000u64;
            let Ok(result) = re.exec(&subject, 0, &mut steps) else { continue };
            if reference.work.get() > REFERENCE_LIMIT {
                continue;
            }
            compared += 1;
            let got = result.map_or(NO.to_string(), |caps| spans(&caps));
            if got != expected && mismatches.len() < 12 {
                mismatches.push(format!("  /{pattern}/ on {:?}: specification {expected:?}, engine {got:?}", String::from_utf16_lossy(&subject)));
            }
        }
    }
    assert!(mismatches.is_empty(), "disagreements:\n{}", mismatches.join("\n"));
    assert!(compared > 80_000, "only {compared} comparisons ran");
}
