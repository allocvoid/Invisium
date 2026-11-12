//! Document tree: the element structure is read in full before drawing, so references
//! (gradients, `<use>`) may point forwards.

use super::xml::{Event, Reader};
use super::{attr, MAX_DEPTH, MAX_ELEMENTS};
use std::borrow::Cow;
use std::collections::HashMap;

/// "No node" in the `first` / `next` links.
pub const NONE: u32 = u32::MAX;

pub struct Node<'a> {
    /// Lower-case local name.
    pub name: String,
    pub attrs: Vec<(&'a str, Cow<'a, str>)>,
    pub first: u32,
    pub next: u32,
}

pub struct Doc<'a> {
    /// `nodes[0]` is the root `<svg>`.
    pub nodes: Vec<Node<'a>>,
    ids: HashMap<String, u32>,
}

impl<'a> Doc<'a> {
    /// Build the tree. `None` unless the first element is `<svg>`.
    pub fn parse(source: &'a str) -> Option<Doc<'a>> {
        let mut reader = Reader::new(source);
        let Some(Event::Start { name, attrs, self_closing }) = reader.next() else { return None };
        if !name.eq_ignore_ascii_case("svg") {
            return None;
        }
        let mut doc = Doc { nodes: Vec::new(), ids: HashMap::new() };
        doc.add(name, attrs);
        if self_closing {
            return Some(doc);
        }
        // (open element, its last child so far)
        let mut stack: Vec<(u32, u32)> = vec![(0, NONE)];
        let mut skip = 0usize;
        for ev in reader {
            match ev {
                Event::End => {
                    if skip > 0 {
                        skip -= 1;
                    } else {
                        stack.pop();
                        if stack.is_empty() {
                            break; // the root closed
                        }
                    }
                }
                Event::Start { name, attrs, self_closing } => {
                    if skip > 0 {
                        if !self_closing {
                            skip += 1;
                        }
                        continue;
                    }
                    if doc.nodes.len() >= MAX_ELEMENTS {
                        break;
                    }
                    let depth = stack.len();
                    let Some(top) = stack.last_mut() else { break };
                    let idx = doc.add(name, attrs);
                    if top.1 == NONE {
                        doc.nodes[top.0 as usize].first = idx;
                    } else {
                        doc.nodes[top.1 as usize].next = idx;
                    }
                    top.1 = idx;
                    if !self_closing {
                        if depth < MAX_DEPTH {
                            stack.push((idx, NONE));
                        } else {
                            skip = 1;
                        }
                    }
                }
            }
        }
        Some(doc)
    }

    fn add(&mut self, name: &str, attrs: Vec<(&'a str, Cow<'a, str>)>) -> u32 {
        let idx = self.nodes.len() as u32;
        if let Some(id) = attr(&attrs, "id").map(str::trim).filter(|s| !s.is_empty()) {
            self.ids.entry(id.to_string()).or_insert(idx);
        }
        self.nodes.push(Node { name: name.to_ascii_lowercase(), attrs, first: NONE, next: NONE });
        idx
    }

    pub fn node(&self, idx: u32) -> &Node<'a> {
        &self.nodes[idx as usize]
    }

    pub fn by_id(&self, id: &str) -> Option<u32> {
        self.ids.get(id.trim()).copied()
    }

    pub fn is_gradient(&self, idx: u32) -> bool {
        matches!(self.node(idx).name.as_str(), "lineargradient" | "radialgradient")
    }

    /// The gradient element with this id, if there is one.
    pub fn gradient(&self, id: &str) -> Option<u32> {
        self.by_id(id).filter(|&i| self.is_gradient(i))
    }

    /// Target of the element's `href` / `xlink:href` (`#id` form only).
    pub fn href(&self, idx: u32) -> Option<u32> {
        let attrs = &self.node(idx).attrs;
        let v = attr(attrs, "href").or_else(|| attr(attrs, "xlink:href"))?;
        self.by_id(v.trim().strip_prefix('#')?)
    }

    pub fn children(&self, idx: u32) -> impl Iterator<Item = u32> + '_ {
        let mut cur = self.node(idx).first;
        std::iter::from_fn(move || {
            if cur == NONE {
                return None;
            }
            let i = cur;
            cur = self.node(i).next;
            Some(i)
        })
    }
}
