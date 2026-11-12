//! Pattern AST, stored in an arena so that neither building, walking nor dropping it recurses.

use crate::charset::CharSet;

pub(crate) type NodeId = u32;

/// "No upper bound" for a quantifier.
pub(crate) const INF: u32 = u32::MAX;

#[derive(Debug)]
pub(crate) enum Node {
    Empty,
    /// One code unit (or one code point in Unicode mode).
    Char(u32),
    /// `.`
    Any,
    /// A character class: an index into `Ast::sets`, taken before case closure and negation.
    Class { set: u32, negate: bool },
    Seq(Vec<NodeId>),
    Alt(Vec<NodeId>),
    /// A capturing group with its 1-based index.
    Capture(NodeId, u32),
    Repeat {
        child: NodeId,
        min: u32,
        max: u32,
        greedy: bool,
        /// Capture groups `cap_from + 1 ..= cap_to` live inside `child`.
        cap_from: u32,
        cap_to: u32,
    },
    Look { child: NodeId, behind: bool, negate: bool },
    LineStart,
    LineEnd,
    WordBoundary,
    NotWordBoundary,
    BackRef(u32),
}

#[derive(Debug)]
pub(crate) struct Ast {
    pub nodes: Vec<Node>,
    /// Parallel to `nodes`: can the node match the empty string?
    pub nullable: Vec<bool>,
    /// The distinct character sets used by `Node::Class`.
    pub sets: Vec<CharSet>,
    pub root: NodeId,
    pub group_count: u32,
    pub names: Vec<(String, usize)>,
}

impl Ast {
    pub(crate) fn node(&self, id: NodeId) -> &Node {
        // Ids are produced by the parser and always in range; anything else reads as an empty node.
        self.nodes.get(id as usize).unwrap_or(&Node::Empty)
    }

    pub(crate) fn set(&self, index: u32) -> Option<&CharSet> {
        self.sets.get(index as usize)
    }

    pub(crate) fn is_nullable(&self, id: NodeId) -> bool {
        self.nullable.get(id as usize).copied().unwrap_or(true)
    }
}
