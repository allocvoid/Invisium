//! Selector parsing and matching.

use super::tokenizer::{split_commas, Tok};
use crate::atom::{a, Atom};
use crate::dom::{Document, NodeId, NO_NODE};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Combinator {
    Descendant,
    Child,
    NextSibling,
    SubsequentSibling,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AttrOp {
    Exists,
    Eq,
    Includes,
    DashMatch,
    Prefix,
    Suffix,
    Substring,
}

#[derive(Clone, Debug)]
pub struct AttrSel {
    pub name: Atom,
    pub op: AttrOp,
    pub value: Box<str>,
    pub ci: bool,
}

#[derive(Clone, Debug)]
pub enum Pseudo {
    FirstChild,
    LastChild,
    OnlyChild,
    FirstOfType,
    LastOfType,
    OnlyOfType,
    NthChild(i32, i32),
    NthLastChild(i32, i32),
    NthOfType(i32, i32),
    NthLastOfType(i32, i32),
    Root,
    Empty,
    Link,
    Hover,
    Active,
    Focus,
    FocusWithin,
    Checked,
    Disabled,
    Enabled,
    Target,
    Not(Vec<Selector>),
    Is(Vec<Selector>),
    /// `:where()`: like `:is()` but contributes no specificity.
    Where(Vec<Selector>),
    /// `:has()`: each entry is a relative selector with its leading combinator.
    Has(Vec<(Combinator, Selector)>),
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PseudoElement {
    None,
    Before,
    After,
    /// A pseudo-element we do not render (`::selection`, `::placeholder`, scrollbar parts, ...).
    Other,
}

#[derive(Clone, Debug, Default)]
pub struct Compound {
    pub tag: Option<Atom>,
    pub id: Option<Atom>,
    pub classes: Vec<Atom>,
    pub attrs: Vec<AttrSel>,
    pub pseudos: Vec<Pseudo>,
}

#[derive(Clone, Debug)]
pub struct Selector {
    /// Compounds from right (the subject) to left.
    pub compounds: Vec<Compound>,
    /// `combs[i]` joins `compounds[i]` with `compounds[i + 1]`.
    pub combs: Vec<Combinator>,
    pub specificity: u32,
    pub pseudo_element: PseudoElement,
    /// True if matching depends on hover/active/focus state.
    pub dynamic: bool,
}

fn parse_nth(toks: &[Tok]) -> Option<(i32, i32)> {
    let mut s = String::new();
    for t in toks {
        let piece = match t {
            Tok::Whitespace => continue,
            Tok::Ident(i) => {
                if i.eq_ignore_ascii_case("of") {
                    break;
                }
                i.to_ascii_lowercase()
            }
            Tok::Number(v) => format!("{}", *v as i32),
            Tok::Dimension(v, u) => format!("{}{}", *v as i32, u),
            Tok::Delim(c) => c.to_string(),
            _ => return None,
        };
        let needs_plus = matches!(t, Tok::Number(_) | Tok::Dimension(..))
            && !piece.starts_with('-')
            && !s.is_empty()
            && !s.ends_with('+')
            && !s.ends_with('-');
        if needs_plus {
            s.push('+');
        }
        s.push_str(&piece);
    }
    match s.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    if let Some(pos) = s.find('n') {
        let (a_str, b_str) = (&s[..pos], &s[pos + 1..]);
        let a = match a_str {
            "" | "+" => 1,
            "-" => -1,
            x => x.parse::<i32>().ok()?,
        };
        let b = if b_str.is_empty() { 0 } else { b_str.trim_start_matches('+').parse::<i32>().ok()? };
        Some((a, b))
    } else {
        Some((0, s.trim_start_matches('+').parse::<i32>().ok()?))
    }
}

struct P<'a> {
    t: &'a [Tok],
    i: usize,
    parent: Option<&'a [Selector]>,
    used_amp: bool,
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<&'a Tok> {
        self.t.get(self.i)
    }

    fn skip_ws(&mut self) -> bool {
        let mut any = false;
        while matches!(self.peek(), Some(Tok::Whitespace)) {
            self.i += 1;
            any = true;
        }
        any
    }

    /// Tokens inside a function's parentheses; advances past the closing `)`.
    fn args(&mut self) -> &'a [Tok] {
        let start = self.i;
        let mut depth = 1;
        while self.i < self.t.len() {
            match &self.t[self.i] {
                Tok::Function(_) | Tok::LParen => depth += 1,
                Tok::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            self.i += 1;
        }
        let inner = &self.t[start..self.i.min(self.t.len())];
        if self.i < self.t.len() {
            self.i += 1;
        }
        inner
    }

    fn attr(&mut self) -> Option<AttrSel> {
        // self.i is just past '['
        let start = self.i;
        while self.i < self.t.len() && !matches!(self.t[self.i], Tok::RBracket) {
            self.i += 1;
        }
        let inner = &self.t[start..self.i];
        if self.i < self.t.len() {
            self.i += 1;
        }
        let mut it = inner.iter().filter(|t| !matches!(t, Tok::Whitespace)).peekable();
        let name = match it.next()? {
            Tok::Ident(n) => Atom::lower(n),
            _ => return None,
        };
        let op = match it.next() {
            None => return Some(AttrSel { name, op: AttrOp::Exists, value: "".into(), ci: false }),
            Some(Tok::Delim('=')) => AttrOp::Eq,
            Some(Tok::Delim(c)) => {
                let op = match c {
                    '~' => AttrOp::Includes,
                    '|' => AttrOp::DashMatch,
                    '^' => AttrOp::Prefix,
                    '$' => AttrOp::Suffix,
                    '*' => AttrOp::Substring,
                    _ => return None,
                };
                if !matches!(it.next(), Some(Tok::Delim('='))) {
                    return None;
                }
                op
            }
            _ => return None,
        };
        let value: String = match it.next()? {
            Tok::Ident(v) | Tok::Str(v) => v.clone(),
            Tok::Number(n) => format!("{}", n),
            Tok::Dimension(n, u) => format!("{}{}", n, u),
            _ => return None,
        };
        let ci = matches!(it.next(), Some(Tok::Ident(f)) if f.eq_ignore_ascii_case("i"));
        // values of these attributes are compared case-insensitively in HTML
        let ci = ci || matches!(name, a::TYPE | a::ALIGN | a::METHOD | a::REL | a::LANG | a::DIR);
        Some(AttrSel { name, op, value: value.into_boxed_str(), ci })
    }

    fn pseudo(&mut self, comp: &mut Compound, pe: &mut PseudoElement) -> Option<()> {
        // self.i is just past the first ':'
        let mut double = false;
        if matches!(self.peek(), Some(Tok::Colon)) {
            self.i += 1;
            double = true;
        }
        match self.peek()? {
            Tok::Ident(name) => {
                self.i += 1;
                let l = name.to_ascii_lowercase();
                if double || matches!(l.as_str(), "before" | "after" | "first-line" | "first-letter") {
                    *pe = match l.as_str() {
                        "before" => PseudoElement::Before,
                        "after" => PseudoElement::After,
                        _ => PseudoElement::Other,
                    };
                    return Some(());
                }
                let p = match l.as_str() {
                    "first-child" => Pseudo::FirstChild,
                    "last-child" => Pseudo::LastChild,
                    "only-child" => Pseudo::OnlyChild,
                    "first-of-type" => Pseudo::FirstOfType,
                    "last-of-type" => Pseudo::LastOfType,
                    "only-of-type" => Pseudo::OnlyOfType,
                    "root" | "scope" => Pseudo::Root,
                    "empty" => Pseudo::Empty,
                    "link" | "any-link" => Pseudo::Link,
                    "hover" => Pseudo::Hover,
                    "active" => Pseudo::Active,
                    "focus" | "focus-visible" => Pseudo::Focus,
                    "focus-within" => Pseudo::FocusWithin,
                    "checked" => Pseudo::Checked,
                    "disabled" => Pseudo::Disabled,
                    "enabled" => Pseudo::Enabled,
                    "target" => Pseudo::Target,
                    "defined" | "read-write" | "valid" | "optional" => Pseudo::Always,
                    // :visited never matches: history must not be observable from styles
                    _ => Pseudo::Never,
                };
                comp.pseudos.push(p);
                Some(())
            }
            Tok::Function(name) => {
                self.i += 1;
                let l = name.to_ascii_lowercase();
                let inner = self.args();
                if double {
                    *pe = PseudoElement::Other;
                    return Some(());
                }
                let p = match l.as_str() {
                    "nth-child" => parse_nth(inner).map(|(a, b)| Pseudo::NthChild(a, b))?,
                    "nth-last-child" => parse_nth(inner).map(|(a, b)| Pseudo::NthLastChild(a, b))?,
                    "nth-of-type" => parse_nth(inner).map(|(a, b)| Pseudo::NthOfType(a, b))?,
                    "nth-last-of-type" => parse_nth(inner).map(|(a, b)| Pseudo::NthLastOfType(a, b))?,
                    "not" => Pseudo::Not(parse_selector_list(inner, self.parent)),
                    "is" | "matches" | "-webkit-any" | "-moz-any" => Pseudo::Is(parse_selector_list(inner, self.parent)),
                    "where" => Pseudo::Where(parse_selector_list(inner, self.parent)),
                    "has" => {
                        let mut rel = Vec::new();
                        for part in split_commas(inner) {
                            let mut k = 0;
                            while k < part.len() && matches!(part[k], Tok::Whitespace) {
                                k += 1;
                            }
                            let comb = match part.get(k) {
                                Some(Tok::Delim('>')) => {
                                    k += 1;
                                    Combinator::Child
                                }
                                Some(Tok::Delim('+')) => {
                                    k += 1;
                                    Combinator::NextSibling
                                }
                                Some(Tok::Delim('~')) => {
                                    k += 1;
                                    Combinator::SubsequentSibling
                                }
                                _ => Combinator::Descendant,
                            };
                            if let Some(s) = parse_complex(&part[k..], None) {
                                rel.push((comb, s));
                            }
                        }
                        Pseudo::Has(rel)
                    }
                    "lang" | "dir" => Pseudo::Always,
                    _ => Pseudo::Never,
                };
                comp.pseudos.push(p);
                Some(())
            }
            _ => None,
        }
    }

    fn compound(&mut self, pe: &mut PseudoElement) -> Option<Compound> {
        let mut c = Compound::default();
        let mut any = false;
        loop {
            match self.peek() {
                Some(Tok::Ident(name)) if !any => {
                    self.i += 1;
                    // namespace prefix `ns|tag`
                    if matches!(self.peek(), Some(Tok::Delim('|'))) {
                        self.i += 1;
                        match self.peek() {
                            Some(Tok::Ident(n2)) => {
                                self.i += 1;
                                c.tag = Some(Atom::lower(n2));
                            }
                            Some(Tok::Delim('*')) => self.i += 1,
                            _ => return None,
                        }
                    } else {
                        c.tag = Some(Atom::lower(name));
                    }
                }
                Some(Tok::Delim('*')) if !any => {
                    self.i += 1;
                    if matches!(self.peek(), Some(Tok::Delim('|'))) {
                        self.i += 1;
                        if let Some(Tok::Ident(n2)) = self.peek() {
                            self.i += 1;
                            c.tag = Some(Atom::lower(n2));
                        } else if matches!(self.peek(), Some(Tok::Delim('*'))) {
                            self.i += 1;
                        }
                    }
                }
                Some(Tok::Delim('&')) => {
                    self.i += 1;
                    self.used_amp = true;
                    match self.parent {
                        Some(p) => c.pseudos.push(Pseudo::Is(p.to_vec())),
                        None => c.pseudos.push(Pseudo::Root),
                    }
                }
                Some(Tok::Hash(h)) => {
                    self.i += 1;
                    if c.id.is_some() {
                        // two ids on one compound can never match the same element unless equal
                        c.attrs.push(AttrSel { name: a::ID, op: AttrOp::Eq, value: h.as_str().into(), ci: false });
                    } else {
                        c.id = Some(Atom::new(h));
                    }
                }
                Some(Tok::Delim('.')) => {
                    self.i += 1;
                    match self.peek() {
                        Some(Tok::Ident(cl)) => {
                            self.i += 1;
                            c.classes.push(Atom::new(cl));
                        }
                        _ => return None,
                    }
                }
                Some(Tok::LBracket) => {
                    self.i += 1;
                    c.attrs.push(self.attr()?);
                }
                Some(Tok::Colon) => {
                    self.i += 1;
                    self.pseudo(&mut c, pe)?;
                }
                _ => break,
            }
            any = true;
        }
        if any {
            Some(c)
        } else {
            None
        }
    }
}

fn compound_specificity(c: &Compound) -> u32 {
    let mut s = 0u32;
    if c.id.is_some() {
        s += 1 << 16;
    }
    s += (c.classes.len() as u32 + c.attrs.len() as u32) << 8;
    if c.tag.is_some() {
        s += 1;
    }
    for p in &c.pseudos {
        match p {
            Pseudo::Not(l) | Pseudo::Is(l) => s += l.iter().map(|x| x.specificity).max().unwrap_or(0),
            Pseudo::Has(l) => s += l.iter().map(|x| x.1.specificity).max().unwrap_or(0),
            Pseudo::Where(_) => {}
            _ => s += 1 << 8,
        }
    }
    s
}

fn compound_dynamic(c: &Compound) -> bool {
    c.pseudos.iter().any(|p| match p {
        Pseudo::Hover | Pseudo::Active | Pseudo::Focus | Pseudo::FocusWithin => true,
        Pseudo::Not(l) | Pseudo::Is(l) | Pseudo::Where(l) => l.iter().any(|s| s.dynamic),
        Pseudo::Has(l) => l.iter().any(|s| s.1.dynamic),
        _ => false,
    })
}

/// Parse one complex selector (no top-level commas).
pub fn parse_complex(toks: &[Tok], parent: Option<&[Selector]>) -> Option<Selector> {
    let mut p = P { t: toks, i: 0, parent, used_amp: false };
    let mut compounds: Vec<Compound> = Vec::new(); // left to right
    let mut combs: Vec<Combinator> = Vec::new();
    let mut pe = PseudoElement::None;
    p.skip_ws();
    // a nested selector may start with a combinator: `> .child`
    let mut leading: Option<Combinator> = None;
    if parent.is_some() {
        leading = match p.peek() {
            Some(Tok::Delim('>')) => Some(Combinator::Child),
            Some(Tok::Delim('+')) => Some(Combinator::NextSibling),
            Some(Tok::Delim('~')) => Some(Combinator::SubsequentSibling),
            _ => None,
        };
        if leading.is_some() {
            p.i += 1;
            p.skip_ws();
        }
    }
    loop {
        if pe != PseudoElement::None {
            // nothing may follow a pseudo-element (except state pseudo-classes, which we drop)
            p.skip_ws();
            if p.peek().is_some() {
                while matches!(p.peek(), Some(Tok::Colon)) {
                    p.i += 1;
                    let mut dummy = Compound::default();
                    let mut pe2 = PseudoElement::None;
                    p.pseudo(&mut dummy, &mut pe2)?;
                    pe = PseudoElement::Other;
                }
                p.skip_ws();
                if p.peek().is_some() {
                    return None;
                }
            }
            break;
        }
        let c = p.compound(&mut pe)?;
        compounds.push(c);
        let ws = p.skip_ws();
        match p.peek() {
            None => break,
            Some(Tok::Delim('>')) => {
                p.i += 1;
                combs.push(Combinator::Child);
            }
            Some(Tok::Delim('+')) => {
                p.i += 1;
                combs.push(Combinator::NextSibling);
            }
            Some(Tok::Delim('~')) => {
                p.i += 1;
                combs.push(Combinator::SubsequentSibling);
            }
            Some(_) if ws => combs.push(Combinator::Descendant),
            Some(_) => return None,
        }
        p.skip_ws();
    }
    if compounds.is_empty() {
        return None;
    }
    // Nested rule without `&`: implicitly descends from the parent rule's subject.
    if let Some(parent) = parent {
        if !p.used_amp || leading.is_some() {
            let mut pc = Compound::default();
            pc.pseudos.push(Pseudo::Is(parent.to_vec()));
            compounds.insert(0, pc);
            combs.insert(0, leading.unwrap_or(Combinator::Descendant));
        }
    }
    let specificity = compounds.iter().map(compound_specificity).sum::<u32>()
        + if matches!(pe, PseudoElement::Before | PseudoElement::After | PseudoElement::Other) { 1 } else { 0 };
    let dynamic = compounds.iter().any(compound_dynamic);
    compounds.reverse();
    combs.reverse();
    Some(Selector { compounds, combs, specificity, pseudo_element: pe, dynamic })
}

/// Parse a comma-separated selector list, dropping selectors we cannot parse.
pub fn parse_selector_list(toks: &[Tok], parent: Option<&[Selector]>) -> Vec<Selector> {
    split_commas(toks).into_iter().filter_map(|part| parse_complex(part, parent)).collect()
}

// ───────────────────────────── matching ─────────────────────────────

pub struct MatchCtx<'a> {
    pub doc: &'a Document,
    /// 1-based index of each element among its element siblings (0 for non-elements).
    pub elem_index: &'a [u32],
    /// Number of element children per node.
    pub elem_count: &'a [u32],
    pub hover: &'a [NodeId],
    pub active: &'a [NodeId],
    pub focus: NodeId,
    pub target: NodeId,
}

fn nth_matches(a: i32, b: i32, index: i32) -> bool {
    if a == 0 {
        return index == b;
    }
    let d = index - b;
    d % a == 0 && d / a >= 0
}

fn attr_matches(sel: &AttrSel, v: &str) -> bool {
    let (v_l, s_l);
    let (v, s): (&str, &str) = if sel.ci {
        v_l = v.to_ascii_lowercase();
        s_l = sel.value.to_ascii_lowercase();
        (&v_l, &s_l)
    } else {
        (v, &sel.value)
    };
    match sel.op {
        AttrOp::Exists => true,
        AttrOp::Eq => v == s,
        AttrOp::Includes => !s.is_empty() && v.split_ascii_whitespace().any(|w| w == s),
        AttrOp::DashMatch => v == s || (v.starts_with(s) && v.as_bytes().get(s.len()) == Some(&b'-')),
        AttrOp::Prefix => !s.is_empty() && v.starts_with(s),
        AttrOp::Suffix => !s.is_empty() && v.ends_with(s),
        AttrOp::Substring => !s.is_empty() && v.contains(s),
    }
}

fn type_index(doc: &Document, node: NodeId, from_end: bool) -> i32 {
    let tag = doc.tag(node);
    let mut n = 1;
    let mut s = if from_end { doc.next_element(node) } else { doc.prev_element(node) };
    while s != NO_NODE {
        if doc.tag(s) == tag {
            n += 1;
        }
        s = if from_end { doc.next_element(s) } else { doc.prev_element(s) };
    }
    n
}

fn is_form_control(tag: Atom) -> bool {
    matches!(tag, a::INPUT | a::BUTTON | a::SELECT | a::TEXTAREA | a::OPTION | a::OPTGROUP | a::FIELDSET)
}

fn match_pseudo(p: &Pseudo, node: NodeId, ctx: &MatchCtx) -> bool {
    let doc = ctx.doc;
    let idx = || ctx.elem_index.get(node as usize).copied().unwrap_or(1) as i32;
    let count = || {
        let parent = doc.parent(node);
        if parent == NO_NODE {
            1
        } else {
            ctx.elem_count.get(parent as usize).copied().unwrap_or(1) as i32
        }
    };
    match p {
        Pseudo::FirstChild => idx() == 1,
        Pseudo::LastChild => idx() == count(),
        Pseudo::OnlyChild => count() == 1,
        Pseudo::FirstOfType => type_index(doc, node, false) == 1,
        Pseudo::LastOfType => type_index(doc, node, true) == 1,
        Pseudo::OnlyOfType => type_index(doc, node, false) == 1 && type_index(doc, node, true) == 1,
        Pseudo::NthChild(a, b) => nth_matches(*a, *b, idx()),
        Pseudo::NthLastChild(a, b) => nth_matches(*a, *b, count() - idx() + 1),
        Pseudo::NthOfType(a, b) => nth_matches(*a, *b, type_index(doc, node, false)),
        Pseudo::NthLastOfType(a, b) => nth_matches(*a, *b, type_index(doc, node, true)),
        Pseudo::Root => doc.tag(node) == a::HTML && doc.parent(node) == crate::dom::DOCUMENT_NODE,
        Pseudo::Empty => doc.children(node).all(|c| !doc.is_element(c) && doc.text(c).map_or(true, |t| t.is_empty())),
        Pseudo::Link => {
            let t = doc.tag(node);
            (t == a::A || t == a::AREA) && doc.attr(node, a::HREF).is_some()
        }
        Pseudo::Hover => ctx.hover.contains(&node),
        Pseudo::Active => ctx.active.contains(&node),
        Pseudo::Focus => ctx.focus == node,
        Pseudo::FocusWithin => {
            let mut n = ctx.focus;
            while n != NO_NODE {
                if n == node {
                    return true;
                }
                n = doc.parent(n);
            }
            false
        }
        Pseudo::Checked => {
            let t = doc.tag(node);
            (t == a::INPUT && doc.attr(node, a::CHECKED).is_some())
                || (t == a::OPTION && doc.attr(node, a::SELECTED).is_some())
        }
        Pseudo::Disabled => is_form_control(doc.tag(node)) && doc.attr(node, a::DISABLED).is_some(),
        Pseudo::Enabled => is_form_control(doc.tag(node)) && doc.attr(node, a::DISABLED).is_none(),
        Pseudo::Target => ctx.target == node,
        Pseudo::Not(list) => !list.iter().any(|s| matches(s, node, ctx)),
        Pseudo::Is(list) | Pseudo::Where(list) => list.iter().any(|s| matches(s, node, ctx)),
        Pseudo::Has(list) => list.iter().any(|(comb, s)| match comb {
            Combinator::Descendant => doc.descendants(node).any(|d| doc.is_element(d) && matches_scoped(s, d, node, ctx)),
            Combinator::Child => doc.children(node).any(|d| doc.is_element(d) && matches_scoped(s, d, node, ctx)),
            Combinator::NextSibling => {
                let n = doc.next_element(node);
                n != NO_NODE && matches(s, n, ctx)
            }
            Combinator::SubsequentSibling => {
                let mut n = doc.next_element(node);
                while n != NO_NODE {
                    if matches(s, n, ctx) {
                        return true;
                    }
                    n = doc.next_element(n);
                }
                false
            }
        }),
        Pseudo::Always => true,
        Pseudo::Never => false,
    }
}

fn match_compound(c: &Compound, node: NodeId, ctx: &MatchCtx) -> bool {
    let Some(el) = ctx.doc.element(node) else { return false };
    if let Some(t) = c.tag {
        if el.tag != t {
            return false;
        }
    }
    if let Some(id) = c.id {
        if el.id != id {
            return false;
        }
    }
    for cl in &c.classes {
        if !el.classes.contains(cl) {
            return false;
        }
    }
    for at in &c.attrs {
        match el.attr(at.name) {
            Some(v) => {
                if !attr_matches(at, v) {
                    return false;
                }
            }
            None => return false,
        }
    }
    for p in &c.pseudos {
        if !match_pseudo(p, node, ctx) {
            return false;
        }
    }
    true
}

fn match_from(sel: &Selector, i: usize, node: NodeId, scope: NodeId, ctx: &MatchCtx) -> bool {
    if !match_compound(&sel.compounds[i], node, ctx) {
        return false;
    }
    if i + 1 == sel.compounds.len() {
        return true;
    }
    let doc = ctx.doc;
    match sel.combs[i] {
        Combinator::Child => {
            let p = doc.parent(node);
            p != NO_NODE && p != scope && doc.is_element(p) && match_from(sel, i + 1, p, scope, ctx)
        }
        Combinator::Descendant => {
            let mut p = doc.parent(node);
            while p != NO_NODE && p != scope && doc.is_element(p) {
                if match_from(sel, i + 1, p, scope, ctx) {
                    return true;
                }
                p = doc.parent(p);
            }
            false
        }
        Combinator::NextSibling => {
            let s = doc.prev_element(node);
            s != NO_NODE && match_from(sel, i + 1, s, scope, ctx)
        }
        Combinator::SubsequentSibling => {
            let mut s = doc.prev_element(node);
            while s != NO_NODE {
                if match_from(sel, i + 1, s, scope, ctx) {
                    return true;
                }
                s = doc.prev_element(s);
            }
            false
        }
    }
}

/// Does `sel` match `node`? (The pseudo-element part is checked by the caller.)
pub fn matches(sel: &Selector, node: NodeId, ctx: &MatchCtx) -> bool {
    match_from(sel, 0, node, NO_NODE, ctx)
}

/// Like `matches`, but ancestor traversal stops below `scope` (used by `:has()`).
fn matches_scoped(sel: &Selector, node: NodeId, scope: NodeId, ctx: &MatchCtx) -> bool {
    match_from(sel, 0, node, scope, ctx)
}

#[cfg(test)]
mod tests {
    use super::super::tokenizer::tokenize_value;
    use super::*;
    use crate::html;

    fn sel(s: &str) -> Selector {
        parse_complex(&tokenize_value(s), None).expect(s)
    }

    fn check(doc: &Document, s: &str, id: &str) -> bool {
        let (idx, cnt) = crate::style::sibling_indices(doc);
        let ctx = MatchCtx { doc, elem_index: &idx, elem_count: &cnt, hover: &[], active: &[], focus: NO_NODE, target: NO_NODE };
        let node = doc.element_by_id(id);
        assert_ne!(node, NO_NODE, "no element #{id}");
        matches(&sel(s), node, &ctx)
    }

    #[test]
    fn matching() {
        let doc = html::parse(
            "<div class='a b' id=x><p id=p1>one</p><p id=p2 class=c>two <a id=l href='/x'>link</a></p><ul><li id=l1><li id=l2><li id=l3></ul></div>",
            false,
        );
        assert!(check(&doc, "div.a.b", "x"));
        assert!(!check(&doc, "div.a.z", "x"));
        assert!(check(&doc, "div > p", "p1"));
        assert!(check(&doc, "div p + p.c", "p2"));
        assert!(check(&doc, "#x a[href^='/']", "l"));
        assert!(check(&doc, "p:first-child", "p1"));
        assert!(!check(&doc, "p:first-child", "p2"));
        assert!(check(&doc, "li:nth-child(2n+1)", "l1"));
        assert!(!check(&doc, "li:nth-child(odd)", "l2"));
        assert!(check(&doc, "li:last-child", "l3"));
        assert!(check(&doc, "li:not(:first-child):not(:last-child)", "l2"));
        assert!(check(&doc, ":is(div, section) > p.c", "p2"));
        assert!(check(&doc, "p:has(a)", "p2"));
        assert!(!check(&doc, "p:has(a)", "p1"));
        assert!(check(&doc, "a:link", "l"));
        assert!(!check(&doc, "a:visited", "l"));
        assert_eq!(sel("#a .b c").specificity, (1 << 16) + (1 << 8) + 1);
        assert_eq!(sel("a::before").pseudo_element, PseudoElement::Before);
    }
}
