//! Start-position prefilters: a literal prefix, or the set of code units a match can begin with.

use crate::ast::{Ast, Node, NodeId};
use crate::charset::{CharSet, SetBuilder, SetData};
use crate::compile::class_set;
use crate::program::Prefilter;
use crate::unicode::{case_closure, is_surrogate, split_surrogates};
use crate::Flags;

/// Give up on the first-character analysis below this nesting depth (it recurses).
const MAX_DEPTH: u32 = 48;

/// `(prefilter, anchored)` for the pattern; `anchored` means it can only match at index 0.
pub(crate) fn prefilter(ast: &Ast, flags: Flags) -> (Prefilter, bool) {
    let a = Analysis { ast, icase: flags.ignore_case, unicode: flags.unicode || flags.unicode_sets };
    let root = ast.root;
    let items: &[NodeId] = match ast.node(root) {
        Node::Seq(items) => items,
        _ => std::slice::from_ref(&root),
    };
    let anchored = !flags.multiline && items.first().is_some_and(|&i| matches!(ast.node(i), Node::LineStart));
    let literal = a.literal_prefix(items);
    if !literal.is_empty() {
        return (Prefilter::Literal(literal), anchored);
    }
    let mut set = SetBuilder::new();
    if a.first(root, 0, &mut set) != Some(false) {
        return (Prefilter::None, anchored);
    }
    let units = a.first_units(&set.finish());
    if units.ranges() == [(0, 0xFFFF)] {
        return (Prefilter::None, anchored);
    }
    (Prefilter::First(SetData::new(&units)), anchored)
}

struct Analysis<'a> {
    ast: &'a Ast,
    icase: bool,
    unicode: bool,
}

impl Analysis<'_> {
    /// The code units every match must start with (leading case-sensitive literals).
    fn literal_prefix(&self, items: &[NodeId]) -> Vec<u16> {
        let mut out = Vec::new();
        for &id in items {
            let Node::Char(c) = self.ast.node(id) else { break };
            let c = *c;
            if self.icase && case_closure(&CharSet::single(c), self.unicode).single_member().is_none() {
                break;
            }
            if c > 0xFFFF {
                let (hi, lo) = split_surrogates(c);
                out.push(hi as u16);
                out.push(lo as u16);
            } else if self.unicode && is_surrogate(c) {
                // A lone surrogate must not be found inside a pair; leave it to the matcher.
                break;
            } else {
                out.push(c as u16);
            }
            if out.len() >= 64 {
                break;
            }
        }
        out
    }

    /// Add the characters `id` can start with to `out`. Returns whether the node can match
    /// without consuming anything, or `None` when no useful answer exists.
    fn first(&self, id: NodeId, depth: u32, out: &mut SetBuilder) -> Option<bool> {
        if depth > MAX_DEPTH {
            return None;
        }
        match self.ast.node(id) {
            Node::Empty | Node::LineStart | Node::LineEnd | Node::WordBoundary | Node::NotWordBoundary => Some(true),
            // A lookaround consumes nothing; whatever follows it supplies the first character.
            Node::Look { .. } => Some(true),
            Node::Char(c) => {
                if self.icase {
                    out.add_set(&case_closure(&CharSet::single(*c), self.unicode));
                } else {
                    out.add(*c);
                }
                Some(false)
            }
            Node::Any | Node::BackRef(_) => None,
            Node::Class { set, negate } => {
                out.add_set(&class_set(self.ast.set(*set)?, *negate, self.icase, self.unicode));
                Some(false)
            }
            Node::Seq(items) => {
                for &item in items {
                    if !self.first(item, depth + 1, out)? {
                        return Some(false);
                    }
                }
                Some(true)
            }
            Node::Alt(alts) => {
                let mut nullable = false;
                for &alt in alts {
                    nullable |= self.first(alt, depth + 1, out)?;
                }
                Some(nullable)
            }
            Node::Capture(child, _) => self.first(*child, depth + 1, out),
            Node::Repeat { child, min, max, .. } => {
                if *max == 0 {
                    return Some(true);
                }
                let nullable = self.first(*child, depth + 1, out)?;
                Some(nullable || *min == 0)
            }
        }
    }

    /// Turn a set of first characters into the set of first code units.
    fn first_units(&self, set: &CharSet) -> CharSet {
        let mut units = CharSet::new();
        for &(lo, hi) in set.ranges() {
            if lo <= 0xFFFF {
                units.add_range(lo, hi.min(0xFFFF));
            }
            if hi > 0xFFFF && self.unicode {
                // A supplementary character starts with its high surrogate.
                units.add_range(split_surrogates(lo.max(0x10000)).0, split_surrogates(hi).0);
            }
        }
        units.normalize();
        units
    }
}
