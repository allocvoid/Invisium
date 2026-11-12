//! HTML serialisation (`innerHTML` / `outerHTML` getters).

use crate::atom::{a, Atom};
use crate::dom::{Document, NodeData, NodeId, NO_NODE};

fn is_void(t: Atom) -> bool {
    matches!(
        t,
        a::AREA | a::BASE | a::BR | a::COL | a::EMBED | a::HR | a::IMG | a::INPUT | a::LINK | a::META | a::PARAM | a::SOURCE | a::TRACK | a::WBR
    )
}

fn is_raw_text(t: Atom) -> bool {
    matches!(t, a::SCRIPT | a::STYLE | a::XMP | a::IFRAME | a::NOEMBED | a::NOFRAMES | a::PLAINTEXT)
}

fn esc_text(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            c => out.push(c),
        }
    }
}

fn esc_attr(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            c => out.push(c),
        }
    }
}

fn open_tag(doc: &Document, n: NodeId, out: &mut String) {
    let Some(el) = doc.element(n) else { return };
    out.push('<');
    out.push_str(el.tag.as_str());
    for at in &el.attrs {
        out.push(' ');
        out.push_str(at.name.as_str());
        out.push_str("=\"");
        esc_attr(&at.value, out);
        out.push('"');
    }
    out.push('>');
}

/// Serialise the children of `root` (`inner`) or `root` itself. Iterative: depth is page-controlled.
pub fn serialize(doc: &Document, root: NodeId, inner: bool, scripting: bool) -> String {
    let mut out = String::new();
    // (node, entering)
    let mut stack: Vec<(NodeId, bool)> = Vec::new();
    if inner {
        let mut kids: Vec<NodeId> = doc.children(root).collect();
        kids.reverse();
        stack.extend(kids.into_iter().map(|k| (k, true)));
    } else {
        stack.push((root, true));
    }
    while let Some((n, entering)) = stack.pop() {
        if !entering {
            out.push_str("</");
            out.push_str(doc.tag(n).as_str());
            out.push('>');
            continue;
        }
        match &doc.node(n).data {
            NodeData::Text(t) => {
                let p = doc.parent(n);
                let ptag = if p != NO_NODE { doc.tag(p) } else { Atom::NONE };
                if is_raw_text(ptag) || (ptag == a::NOSCRIPT && scripting) {
                    out.push_str(t);
                } else {
                    esc_text(t, &mut out);
                }
            }
            NodeData::Comment(t) => {
                out.push_str("<!--");
                out.push_str(t);
                out.push_str("-->");
            }
            NodeData::Doctype => out.push_str("<!DOCTYPE html>"),
            NodeData::Document => {
                let mut kids: Vec<NodeId> = doc.children(n).collect();
                kids.reverse();
                stack.extend(kids.into_iter().map(|k| (k, true)));
            }
            NodeData::Element(el) => {
                open_tag(doc, n, &mut out);
                if is_void(el.tag) {
                    continue;
                }
                stack.push((n, false));
                let mut kids: Vec<NodeId> = doc.children(n).collect();
                kids.reverse();
                stack.extend(kids.into_iter().map(|k| (k, true)));
            }
        }
        if out.len() > 64 << 20 {
            break; // serialisation bombs
        }
    }
    out
}
