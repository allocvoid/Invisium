//! HTML parser: tokenizer + tree builder.
//!
//! The tree builder follows the shape of the WHATWG algorithm (implied `<html>/<head>/<body>`,
//! implied end tags, `<p>` auto-closing, table structure repair with foster parenting, and
//! reconstruction of active formatting elements) without reproducing every error-recovery corner.

pub mod entities;
pub mod tokenizer;

use crate::atom::{a, Atom};
use crate::dom::{Document, NodeId, DOCUMENT_NODE, NO_NODE};
use tokenizer::{Token, Tokenizer};

struct Fmt {
    node: NodeId, // NO_NODE = scope marker
    tag: Atom,
}

struct Builder<'a> {
    doc: Document,
    tok: Tokenizer<'a>,
    open: Vec<NodeId>,
    fmt: Vec<Fmt>,
    html: NodeId,
    head: NodeId,
    body: NodeId,
    foreign: u32,
    skip_newline: bool,
    seen_doctype: bool,
}

fn is_void(t: Atom) -> bool {
    matches!(
        t,
        a::AREA | a::BASE | a::BR | a::COL | a::EMBED | a::HR | a::IMG | a::INPUT | a::LINK | a::META
            | a::PARAM | a::SOURCE | a::TRACK | a::WBR
    )
}

fn is_formatting(t: Atom) -> bool {
    matches!(
        t,
        a::A | a::B | a::BIG | a::CODE | a::EM | a::FONT | a::I | a::NOBR | a::S | a::SMALL | a::STRIKE
            | a::STRONG | a::TT | a::U
    )
}

fn is_heading(t: Atom) -> bool {
    matches!(t, a::H1 | a::H2 | a::H3 | a::H4 | a::H5 | a::H6)
}

/// Start tags that close an open `<p>`.
fn closes_p(t: Atom) -> bool {
    matches!(
        t,
        a::ADDRESS | a::ARTICLE | a::ASIDE | a::BLOCKQUOTE | a::CENTER | a::DETAILS | a::DIALOG | a::DIR
            | a::DIV | a::DL | a::FIELDSET | a::FIGCAPTION | a::FIGURE | a::FOOTER | a::HEADER | a::HGROUP
            | a::MAIN | a::MENU | a::NAV | a::OL | a::P | a::SECTION | a::SUMMARY | a::UL | a::PRE
            | a::LISTING | a::FORM | a::TABLE | a::HR | a::PLAINTEXT | a::XMP | a::H1 | a::H2 | a::H3
            | a::H4 | a::H5 | a::H6 | a::LI | a::DD | a::DT
    )
}

fn is_special(t: Atom) -> bool {
    closes_p(t)
        || is_void(t)
        || matches!(
            t,
            a::APPLET | a::BODY | a::BUTTON | a::CAPTION | a::COLGROUP | a::FRAME | a::FRAMESET | a::HEAD
                | a::HTML | a::IFRAME | a::MARQUEE | a::NOEMBED | a::NOFRAMES | a::NOSCRIPT | a::OBJECT
                | a::SCRIPT | a::SELECT | a::STYLE | a::TBODY | a::TD | a::TEMPLATE | a::TEXTAREA
                | a::TFOOT | a::TH | a::THEAD | a::TITLE | a::TR
        )
}

fn implied_end(t: Atom) -> bool {
    matches!(t, a::DD | a::DT | a::LI | a::OPTGROUP | a::OPTION | a::P | a::RB | a::RP | a::RT | a::RTC)
}

fn head_allowed(t: Atom) -> bool {
    matches!(t, a::BASE | a::LINK | a::META | a::TITLE | a::NOSCRIPT | a::NOFRAMES | a::STYLE | a::SCRIPT | a::TEMPLATE)
}

fn is_table_part(t: Atom) -> bool {
    matches!(t, a::CAPTION | a::COLGROUP | a::COL | a::TBODY | a::THEAD | a::TFOOT | a::TR | a::TD | a::TH)
}

fn is_table_ctx(t: Atom) -> bool {
    matches!(t, a::TABLE | a::TBODY | a::THEAD | a::TFOOT | a::TR)
}

#[derive(Clone, Copy, PartialEq)]
enum Scope {
    Default,
    Button,
    ListItem,
    Table,
}

fn all_ws(s: &str) -> bool {
    s.bytes().all(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0c))
}

impl<'a> Builder<'a> {
    fn top(&self) -> NodeId {
        *self.open.last().unwrap_or(&DOCUMENT_NODE)
    }

    fn top_tag(&self) -> Atom {
        self.doc.tag(self.top())
    }

    fn pop(&mut self) -> NodeId {
        let n = self.open.pop().unwrap_or(NO_NODE);
        if n != NO_NODE {
            let t = self.doc.tag(n);
            if t == a::SVG || t == a::MATH {
                self.foreign = self.foreign.saturating_sub(1);
            }
        }
        n
    }

    fn ensure_html(&mut self) {
        if self.html == NO_NODE {
            self.html = self.doc.create_element(a::HTML, Vec::new());
            self.doc.append_child(DOCUMENT_NODE, self.html);
            self.open.push(self.html);
            if !self.seen_doctype {
                self.doc.quirks = true;
            }
        }
    }

    fn ensure_head(&mut self) {
        self.ensure_html();
        if self.head == NO_NODE {
            self.head = self.doc.create_element(a::HEAD, Vec::new());
            self.doc.append_child(self.html, self.head);
            self.open.push(self.head);
        }
    }

    fn ensure_body(&mut self) {
        self.ensure_head();
        if self.body == NO_NODE {
            while self.open.len() > 1 {
                self.pop();
            }
            self.body = self.doc.create_element(a::BODY, Vec::new());
            self.doc.append_child(self.html, self.body);
            self.open.push(self.body);
        }
    }

    fn merge_attrs(&mut self, node: NodeId, attrs: Vec<(Atom, String)>) {
        if let Some(el) = self.doc.element_mut(node) {
            for (name, value) in attrs {
                if !el.has_attr(name) {
                    el.set_attr(name, &value);
                }
            }
        }
    }

    /// Where the next node goes. In a table context, content that is not table structure is
    /// "foster parented": inserted just before the table.
    fn insertion_point(&self, foster: bool) -> (NodeId, NodeId) {
        let cur = self.top();
        if foster && is_table_ctx(self.doc.tag(cur)) {
            for (i, &n) in self.open.iter().enumerate().rev() {
                if self.doc.tag(n) == a::TABLE {
                    let p = self.doc.parent(n);
                    if p != NO_NODE {
                        return (p, n);
                    }
                    return (self.open[i.saturating_sub(1)], NO_NODE);
                }
            }
        }
        (cur, NO_NODE)
    }

    fn insert(&mut self, name: Atom, attrs: Vec<(Atom, String)>, foster: bool) -> NodeId {
        let (parent, before) = self.insertion_point(foster);
        let n = self.doc.create_element(name, attrs);
        self.doc.insert_before(parent, n, before);
        self.open.push(n);
        if name == a::SVG || name == a::MATH {
            self.foreign += 1;
        }
        n
    }

    fn insert_void(&mut self, name: Atom, attrs: Vec<(Atom, String)>, foster: bool) {
        self.insert(name, attrs, foster);
        self.pop();
    }

    fn insert_text(&mut self, text: &str) {
        let (parent, before) = self.insertion_point(true);
        if before == NO_NODE {
            self.doc.append_text(parent, text);
        } else {
            let prev = self.doc.node(before).prev;
            if prev != NO_NODE {
                if let crate::dom::NodeData::Text(t) = &mut self.doc.nodes[prev as usize].data {
                    t.push_str(text);
                    return;
                }
            }
            let n = self.doc.create_text(text.to_owned());
            self.doc.insert_before(parent, n, before);
        }
    }

    fn in_scope(&self, target: Atom, scope: Scope) -> bool {
        for &n in self.open.iter().rev() {
            let t = self.doc.tag(n);
            if t == target {
                return true;
            }
            let boundary = match scope {
                Scope::Table => matches!(t, a::HTML | a::TABLE | a::TEMPLATE),
                _ => {
                    matches!(
                        t,
                        a::APPLET | a::CAPTION | a::HTML | a::TABLE | a::TD | a::TH | a::MARQUEE | a::OBJECT
                            | a::TEMPLATE
                    ) || (scope == Scope::Button && t == a::BUTTON)
                        || (scope == Scope::ListItem && (t == a::OL || t == a::UL))
                }
            };
            if boundary {
                return false;
            }
        }
        false
    }

    fn generate_implied(&mut self, except: Atom) {
        loop {
            let t = self.top_tag();
            if t != except && implied_end(t) {
                self.pop();
            } else {
                break;
            }
        }
    }

    /// Pop until an element with tag `t` has been popped.
    fn pop_until(&mut self, t: Atom) {
        while self.open.len() > 1 {
            let n = self.pop();
            let tag = self.doc.tag(n);
            if tag == a::TD || tag == a::TH || tag == a::CAPTION {
                self.clear_fmt_to_marker();
            }
            if tag == t {
                break;
            }
        }
    }

    /// Pop until the current node's tag is one of `set` (never pops `<html>`).
    fn pop_to_ctx(&mut self, set: &[Atom]) {
        while self.open.len() > 1 && !set.contains(&self.top_tag()) {
            let n = self.pop();
            let tag = self.doc.tag(n);
            if tag == a::TD || tag == a::TH || tag == a::CAPTION {
                self.clear_fmt_to_marker();
            }
        }
    }

    fn close_p(&mut self) {
        if self.in_scope(a::P, Scope::Button) {
            self.generate_implied(a::P);
            self.pop_until(a::P);
        }
    }

    fn clear_fmt_to_marker(&mut self) {
        while let Some(e) = self.fmt.pop() {
            if e.node == NO_NODE {
                break;
            }
        }
    }

    fn push_fmt(&mut self, node: NodeId, tag: Atom) {
        // Noah's Ark clause (simplified): at most three open entries per tag since the last marker.
        let mut count = 0;
        let mut earliest = None;
        for (i, e) in self.fmt.iter().enumerate().rev() {
            if e.node == NO_NODE {
                break;
            }
            if e.tag == tag {
                count += 1;
                earliest = Some(i);
            }
        }
        if count >= 3 {
            if let Some(i) = earliest {
                self.fmt.remove(i);
            }
        }
        self.fmt.push(Fmt { node, tag });
    }

    fn reconstruct_fmt(&mut self) {
        let Some(last) = self.fmt.last() else { return };
        if last.node == NO_NODE || self.open.contains(&last.node) {
            return;
        }
        let mut i = self.fmt.len() - 1;
        while i > 0 {
            let e = &self.fmt[i - 1];
            if e.node == NO_NODE || self.open.contains(&e.node) {
                break;
            }
            i -= 1;
        }
        for j in i..self.fmt.len() {
            let old = self.fmt[j].node;
            let tag = self.fmt[j].tag;
            let attrs: Vec<(Atom, String)> = self
                .doc
                .element(old)
                .map(|e| e.attrs.iter().map(|x| (x.name, x.value.to_string())).collect())
                .unwrap_or_default();
            let n = self.insert(tag, attrs, true);
            self.fmt[j].node = n;
        }
    }

    fn text(&mut self, mut t: &str) {
        if self.skip_newline {
            self.skip_newline = false;
            if let Some(rest) = t.strip_prefix('\n') {
                t = rest;
            }
            if t.is_empty() {
                return;
            }
        }
        if self.body == NO_NODE {
            let top = self.top_tag();
            if self.html != NO_NODE && top != a::HTML && top != a::HEAD {
                // inside <title>, <style>, <script>, <noscript> in the head
                let cur = self.top();
                self.doc.append_text(cur, t);
                return;
            }
            if all_ws(t) {
                return;
            }
            self.ensure_body();
        }
        if is_table_ctx(self.top_tag()) && all_ws(t) {
            return;
        }
        self.reconstruct_fmt();
        self.insert_text(t);
    }

    fn start(&mut self, name: Atom, attrs: Vec<(Atom, String)>, self_closing: bool) {
        self.skip_newline = false;
        if self.body == NO_NODE {
            match name {
                a::HTML => {
                    self.ensure_html();
                    let h = self.html;
                    self.merge_attrs(h, attrs);
                    return;
                }
                a::HEAD => {
                    self.ensure_head();
                    let h = self.head;
                    self.merge_attrs(h, attrs);
                    return;
                }
                a::BODY => {
                    self.ensure_body();
                    let b = self.body;
                    self.merge_attrs(b, attrs);
                    return;
                }
                a::FRAMESET | a::FRAME => return,
                _ if head_allowed(name) => {
                    self.ensure_head();
                    if self.top_tag() == a::HTML {
                        // </head> was seen; late head content still belongs in the head
                        let h = self.head;
                        self.open.push(h);
                    }
                    if is_void(name) {
                        self.insert_void(name, attrs, false);
                    } else {
                        self.insert(name, attrs, false);
                    }
                    return;
                }
                _ => {
                    self.ensure_body();
                }
            }
        }
        self.start_in_body(name, attrs, self_closing);
    }

    fn start_in_body(&mut self, name: Atom, attrs: Vec<(Atom, String)>, self_closing: bool) {
        if self.foreign > 0 {
            // Inside <svg>/<math>: generic XML-ish nesting, self-closing syntax is honoured.
            self.tok.cancel_raw();
            let n = self.insert(name, attrs, false);
            if self_closing {
                debug_assert_eq!(self.top(), n);
                self.pop();
            }
            return;
        }
        match name {
            a::HTML => {
                let h = self.html;
                self.merge_attrs(h, attrs);
            }
            a::BODY => {
                let b = self.body;
                self.merge_attrs(b, attrs);
            }
            a::HEAD | a::FRAMESET | a::FRAME => {}
            _ if is_table_part(name) => self.table_part(name, attrs),
            a::LI => {
                for i in (0..self.open.len()).rev() {
                    let t = self.doc.tag(self.open[i]);
                    if t == a::LI {
                        self.generate_implied(a::LI);
                        self.pop_until(a::LI);
                        break;
                    }
                    if is_special(t) && !matches!(t, a::ADDRESS | a::DIV | a::P) {
                        break;
                    }
                }
                self.close_p();
                self.insert(name, attrs, true);
            }
            a::DD | a::DT => {
                for i in (0..self.open.len()).rev() {
                    let t = self.doc.tag(self.open[i]);
                    if t == a::DD || t == a::DT {
                        self.generate_implied(t);
                        self.pop_until(t);
                        break;
                    }
                    if is_special(t) && !matches!(t, a::ADDRESS | a::DIV | a::P) {
                        break;
                    }
                }
                self.close_p();
                self.insert(name, attrs, true);
            }
            a::HR => {
                self.close_p();
                self.insert_void(name, attrs, true);
            }
            _ if is_heading(name) => {
                self.close_p();
                if is_heading(self.top_tag()) {
                    self.pop();
                }
                self.insert(name, attrs, true);
            }
            a::PRE | a::LISTING => {
                self.close_p();
                self.insert(name, attrs, true);
                self.skip_newline = true;
            }
            _ if closes_p(name) => {
                self.close_p();
                self.insert(name, attrs, true);
            }
            a::A => {
                let open_a = self.fmt.iter().rev().take_while(|e| e.node != NO_NODE).any(|e| e.tag == a::A);
                if open_a {
                    self.end_formatting(a::A);
                }
                self.reconstruct_fmt();
                let n = self.insert(name, attrs, true);
                self.push_fmt(n, name);
            }
            _ if is_formatting(name) => {
                self.reconstruct_fmt();
                let n = self.insert(name, attrs, true);
                self.push_fmt(n, name);
            }
            a::BUTTON => {
                if self.in_scope(a::BUTTON, Scope::Default) {
                    self.generate_implied(Atom::NONE);
                    self.pop_until(a::BUTTON);
                }
                self.reconstruct_fmt();
                self.insert(name, attrs, true);
            }
            a::APPLET | a::MARQUEE | a::OBJECT => {
                self.reconstruct_fmt();
                self.insert(name, attrs, true);
                self.fmt.push(Fmt { node: NO_NODE, tag: Atom::NONE });
            }
            a::TEXTAREA => {
                self.insert(name, attrs, true);
                self.skip_newline = true;
            }
            a::OPTION | a::OPTGROUP => {
                if self.top_tag() == a::OPTION {
                    self.pop();
                }
                if name == a::OPTGROUP && self.top_tag() == a::OPTGROUP {
                    self.pop();
                }
                self.insert(name, attrs, true);
            }
            a::SELECT => {
                if self.in_scope(a::SELECT, Scope::Default) {
                    self.pop_until(a::SELECT);
                } else {
                    self.reconstruct_fmt();
                    self.insert(name, attrs, true);
                }
            }
            _ if is_void(name) => {
                self.reconstruct_fmt();
                // hidden inputs stay inside the table instead of being foster parented
                let hidden_input = name == a::INPUT
                    && attrs.iter().any(|(n, v)| *n == a::TYPE && v.eq_ignore_ascii_case("hidden"));
                self.insert_void(name, attrs, !hidden_input);
            }
            a::SCRIPT | a::STYLE | a::TEMPLATE | a::TITLE | a::NOSCRIPT => {
                self.insert(name, attrs, false);
            }
            _ => {
                if name.as_str() == "image" {
                    return self.start_in_body(a::IMG, attrs, self_closing);
                }
                self.reconstruct_fmt();
                let n = self.insert(name, attrs, true);
                if self_closing && (name == a::SVG || name == a::MATH) {
                    debug_assert_eq!(self.top(), n);
                    self.pop();
                }
            }
        }
    }

    fn table_part(&mut self, name: Atom, attrs: Vec<(Atom, String)>) {
        if !self.in_scope(a::TABLE, Scope::Table) {
            return; // stray table structure outside a table is ignored
        }
        match name {
            a::CAPTION | a::COLGROUP => {
                self.pop_to_ctx(&[a::TABLE]);
                self.insert(name, attrs, false);
                if name == a::CAPTION {
                    self.fmt.push(Fmt { node: NO_NODE, tag: Atom::NONE });
                }
            }
            a::COL => {
                self.pop_to_ctx(&[a::TABLE, a::COLGROUP]);
                if self.top_tag() == a::TABLE {
                    self.insert(a::COLGROUP, Vec::new(), false);
                }
                self.insert_void(name, attrs, false);
            }
            a::TBODY | a::THEAD | a::TFOOT => {
                self.pop_to_ctx(&[a::TABLE]);
                self.insert(name, attrs, false);
            }
            a::TR => {
                self.pop_to_ctx(&[a::TABLE, a::TBODY, a::THEAD, a::TFOOT]);
                if self.top_tag() == a::TABLE {
                    self.insert(a::TBODY, Vec::new(), false);
                }
                self.insert(name, attrs, false);
            }
            _ => {
                // td / th
                self.pop_to_ctx(&[a::TABLE, a::TBODY, a::THEAD, a::TFOOT, a::TR]);
                if self.top_tag() == a::TABLE {
                    self.insert(a::TBODY, Vec::new(), false);
                }
                if self.top_tag() != a::TR {
                    self.insert(a::TR, Vec::new(), false);
                }
                self.insert(name, attrs, false);
                self.fmt.push(Fmt { node: NO_NODE, tag: Atom::NONE });
            }
        }
    }

    /// Simplified adoption agency: close a formatting element.
    fn end_formatting(&mut self, name: Atom) {
        let idx = self
            .fmt
            .iter()
            .enumerate()
            .rev()
            .take_while(|(_, e)| e.node != NO_NODE)
            .find(|(_, e)| e.tag == name)
            .map(|(i, _)| i);
        let Some(idx) = idx else {
            return self.end_other(name);
        };
        let node = self.fmt[idx].node;
        let Some(pos) = self.open.iter().rposition(|&n| n == node) else {
            self.fmt.remove(idx);
            return;
        };
        let misnested = self.open[pos + 1..].iter().any(|&n| is_special(self.doc.tag(n)));
        if misnested {
            // e.g. <b><div>x</b>y</div>: keep the block open, just end the formatting scope.
            self.open.remove(pos);
        } else {
            while self.open.len() > pos {
                self.pop();
            }
        }
        self.fmt.remove(idx);
    }

    fn end_other(&mut self, name: Atom) {
        for i in (1..self.open.len()).rev() {
            let t = self.doc.tag(self.open[i]);
            if t == name {
                self.generate_implied(name);
                while self.open.len() > i {
                    self.pop();
                }
                return;
            }
            if is_special(t) {
                return;
            }
        }
    }

    fn end(&mut self, name: Atom) {
        self.skip_newline = false;
        if self.body == NO_NODE {
            match name {
                a::HEAD => {
                    if self.head != NO_NODE {
                        while self.open.len() > 1 {
                            self.pop();
                        }
                    }
                    return;
                }
                a::BODY | a::HTML | a::BR => {
                    self.ensure_body();
                }
                _ => {
                    if self.open.len() > 2 && self.top_tag() == name {
                        self.pop();
                    }
                    return;
                }
            }
        }
        if self.foreign > 0 {
            // Close the nearest matching element, as long as it is inside the foreign subtree.
            for i in (1..self.open.len()).rev() {
                let t = self.doc.tag(self.open[i]);
                if t == name {
                    while self.open.len() > i {
                        self.pop();
                    }
                    return;
                }
                if t == a::SVG || t == a::MATH {
                    break;
                }
            }
            return;
        }
        match name {
            a::BODY | a::HTML | a::HEAD => {}
            a::P => {
                if !self.in_scope(a::P, Scope::Button) {
                    self.insert(a::P, Vec::new(), true);
                }
                self.close_p();
            }
            a::BR => {
                self.reconstruct_fmt();
                self.insert_void(a::BR, Vec::new(), true);
            }
            a::LI => {
                if self.in_scope(a::LI, Scope::ListItem) {
                    self.generate_implied(a::LI);
                    self.pop_until(a::LI);
                }
            }
            a::DD | a::DT => {
                if self.in_scope(name, Scope::Default) {
                    self.generate_implied(name);
                    self.pop_until(name);
                }
            }
            _ if is_heading(name) => {
                let any = [a::H1, a::H2, a::H3, a::H4, a::H5, a::H6].iter().any(|&h| self.in_scope(h, Scope::Default));
                if any {
                    self.generate_implied(Atom::NONE);
                    while self.open.len() > 1 {
                        let n = self.pop();
                        if is_heading(self.doc.tag(n)) {
                            break;
                        }
                    }
                }
            }
            a::TABLE => {
                if self.in_scope(a::TABLE, Scope::Table) {
                    self.pop_until(a::TABLE);
                }
            }
            _ if is_table_part(name) => {
                if self.in_scope(name, Scope::Table) {
                    self.pop_until(name);
                }
            }
            a::APPLET | a::MARQUEE | a::OBJECT => {
                if self.in_scope(name, Scope::Default) {
                    self.generate_implied(Atom::NONE);
                    self.pop_until(name);
                    self.clear_fmt_to_marker();
                }
            }
            _ if is_formatting(name) => self.end_formatting(name),
            _ if closes_p(name) || name == a::BUTTON => {
                if self.in_scope(name, Scope::Default) {
                    self.generate_implied(Atom::NONE);
                    self.pop_until(name);
                }
            }
            _ => self.end_other(name),
        }
    }

    fn run(mut self) -> Document {
        loop {
            match self.tok.next_token() {
                Token::Eof => break,
                Token::Doctype => {
                    if self.html == NO_NODE {
                        self.seen_doctype = true;
                        self.doc.quirks = false;
                        let d = self.doc.create_doctype();
                        self.doc.append_child(DOCUMENT_NODE, d);
                    }
                }
                Token::Text(t) => self.text(&t),
                Token::Start { name, attrs, self_closing } => self.start(name, attrs, self_closing),
                Token::End { name } => self.end(name),
            }
        }
        self.ensure_body();
        self.doc.shrink();
        self.doc
    }
}

/// Parse an HTML document. `scripting` controls whether `<noscript>` content is live markup
/// (scripting off) or inert text (scripting on).
pub fn parse(src: &str, scripting: bool) -> Document {
    let mut doc = Document::new();
    doc.quirks = true;
    Builder {
        doc,
        tok: Tokenizer::new(src, scripting),
        open: Vec::with_capacity(32),
        fmt: Vec::new(),
        html: NO_NODE,
        head: NO_NODE,
        body: NO_NODE,
        foreign: 0,
        skip_newline: false,
        seen_doctype: false,
    }
    .run()
}

/// Parse `src` as the content of an element with tag `context` (the `innerHTML` setter). The new
/// nodes are created in `target`, detached, and returned in order.
pub fn parse_fragment(target: &mut Document, context: Atom, src: &str, scripting: bool) -> Vec<NodeId> {
    if src.is_empty() {
        return Vec::new();
    }
    // raw-text and escapable-raw-text elements take the markup as text
    if matches!(context, a::SCRIPT | a::STYLE | a::TEXTAREA | a::TITLE | a::XMP | a::PLAINTEXT | a::NOEMBED | a::NOFRAMES | a::IFRAME)
        || (context == a::NOSCRIPT && scripting)
    {
        return vec![target.create_text(src.to_owned())];
    }
    // table parts only parse correctly inside their table scaffolding
    let (prefix, path): (&str, &[Atom]) = match context {
        a::TABLE => ("<table>", &[a::TABLE]),
        a::TBODY | a::THEAD | a::TFOOT => ("<table><tbody>", &[a::TABLE, a::TBODY]),
        a::TR => ("<table><tbody><tr>", &[a::TABLE, a::TBODY, a::TR]),
        a::COLGROUP => ("<table><colgroup>", &[a::TABLE, a::COLGROUP]),
        a::SELECT => ("<select>", &[a::SELECT]),
        _ => ("", &[]),
    };
    let mut wrapped = String::with_capacity(src.len() + 64);
    wrapped.push_str("<!doctype html><html><head></head><body>");
    wrapped.push_str(prefix);
    wrapped.push_str(src);
    let tmp = parse(&wrapped, scripting);
    let mut container = tmp.body();
    for &tag in path {
        container = tmp.find_child(container, tag);
    }
    if container == NO_NODE {
        return Vec::new();
    }
    let kids: Vec<NodeId> = tmp.children(container).collect();
    kids.into_iter().map(|k| target.import_subtree(&tmp, k)).collect()
}

/// Debug helper: serialise a subtree as an indented outline.
pub fn dump(doc: &Document, node: NodeId, depth: usize, out: &mut String) {
    use crate::dom::NodeData;
    let pad = "  ".repeat(depth);
    match &doc.node(node).data {
        NodeData::Document => {}
        NodeData::Doctype => out.push_str("<!doctype>\n"),
        NodeData::Comment(_) => {}
        NodeData::Text(t) => {
            out.push_str(&pad);
            out.push_str(&format!("{:?}\n", t));
        }
        NodeData::Element(e) => {
            out.push_str(&pad);
            out.push('<');
            out.push_str(e.tag.as_str());
            for at in &e.attrs {
                out.push_str(&format!(" {}={:?}", at.name, at.value));
            }
            out.push_str(">\n");
        }
    }
    let d = if matches!(doc.node(node).data, NodeData::Document) { depth } else { depth + 1 };
    for c in doc.children(node) {
        dump(doc, c, d, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline(src: &str) -> String {
        let doc = parse(src, false);
        let mut s = String::new();
        dump(&doc, DOCUMENT_NODE, 0, &mut s);
        s
    }

    #[test]
    fn implied_structure() {
        let s = outline("<title>Hi</title><p>One<p>Two");
        assert!(s.contains("<head>"));
        assert!(s.contains("<body>"));
        assert_eq!(s.matches("<p>").count(), 2);
        // second <p> is a sibling, not a child, of the first
        assert!(s.contains("    <p>\n      \"One\"\n    <p>\n      \"Two\""));
    }

    #[test]
    fn table_repair() {
        let s = outline("<table><tr><td>a<td>b<tr><td>c</table>x");
        assert!(s.contains("<tbody>"));
        assert_eq!(s.matches("<tr>").count(), 2);
        assert_eq!(s.matches("<td>").count(), 3);
    }

    #[test]
    fn formatting_reconstruct() {
        let s = outline("<p><b>bold<p>still bold");
        assert_eq!(s.matches("<b>").count(), 2);
    }

    #[test]
    fn lists_and_void() {
        let s = outline("<ul><li>a<li>b<br>c</ul><img src=x>");
        assert_eq!(s.matches("<li>").count(), 2);
        assert!(s.contains("<img src=\"x\">"));
    }

    #[test]
    fn svg_self_closing() {
        let s = outline("<svg viewBox='0 0 1 1'><path d='M0 0'/><path d='M1 1'/></svg><p>after");
        assert!(s.contains("    <svg viewbox=\"0 0 1 1\">\n      <path d=\"M0 0\">\n      <path d=\"M1 1\">\n    <p>"));
    }
}
