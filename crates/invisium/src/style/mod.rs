//! The cascade: matches rules to elements and produces computed styles.
//!
//! Memory design: elements whose cascade inputs are identical (same parent style, same matched
//! declaration blocks, same inline style and presentational attributes) share one `ComputedStyle`.
//! On real pages this collapses tens of thousands of elements into a few hundred styles.

pub mod computed;
mod ua;

pub use computed::*;

use crate::atom::{a, Atom};
use crate::css::parser::{parse_stylesheet, MediaEnv, MediaQueryList, Origin, Rule, Stylesheet};
use crate::css::properties::{self, longhands, Declaration, Kw, PropId, UrlResolver, Value};
use crate::css::selector::{matches, MatchCtx, PseudoElement, Selector};
use crate::css::tokenizer::tokenize_value;
use crate::css::values::{named_color, parse_hex_color, Color, CssColor, Len, LenCtx, Size};
use crate::dom::{Document, Element, NodeId, NO_NODE};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

pub type StyleId = u32;
pub const NO_STYLE: StyleId = u32::MAX;

/// Computed styles for one document.
pub struct StyleStore {
    pub styles: Vec<Rc<ComputedStyle>>,
    /// Style of each node, indexed by `NodeId` (`NO_STYLE` for text nodes and unstyled subtrees).
    pub of_node: Vec<StyleId>,
    pub before: HashMap<NodeId, StyleId>,
    pub after: HashMap<NodeId, StyleId>,
    pub root_font_size: f32,
    pub vw: f32,
    pub vh: f32,
}

impl StyleStore {
    pub fn empty() -> StyleStore {
        StyleStore {
            styles: Vec::new(),
            of_node: Vec::new(),
            before: HashMap::new(),
            after: HashMap::new(),
            root_font_size: 16.0,
            vw: 0.0,
            vh: 0.0,
        }
    }

    #[inline]
    pub fn id_of(&self, node: NodeId) -> StyleId {
        self.of_node.get(node as usize).copied().unwrap_or(NO_STYLE)
    }

    #[inline]
    pub fn get(&self, node: NodeId) -> Option<&ComputedStyle> {
        let id = self.id_of(node);
        if id == NO_STYLE {
            None
        } else {
            Some(&self.styles[id as usize])
        }
    }

    #[inline]
    pub fn style(&self, id: StyleId) -> &ComputedStyle {
        &self.styles[id as usize]
    }

    /// A shared handle to a style (cheap: one reference-count increment).
    #[inline]
    pub fn rc(&self, id: StyleId) -> Rc<ComputedStyle> {
        self.styles[id as usize].clone()
    }

    /// Add a style that is not attached to a DOM node (anonymous boxes) and return its id.
    pub fn push(&mut self, s: ComputedStyle) -> StyleId {
        self.styles.push(Rc::new(s));
        (self.styles.len() - 1) as StyleId
    }

    pub fn len_ctx(&self, font_size: f32) -> LenCtx {
        LenCtx { font_size, root_font_size: self.root_font_size, vw: self.vw, vh: self.vh }
    }

    pub fn memory_bytes(&self) -> usize {
        self.styles.capacity() * std::mem::size_of::<ComputedStyle>() + self.of_node.capacity() * 4
    }
}

/// 1-based index of each element among its element siblings, and element-child counts per node.
pub fn sibling_indices(doc: &Document) -> (Vec<u32>, Vec<u32>) {
    let n = doc.nodes.len();
    let mut index = vec![0u32; n];
    let mut count = vec![0u32; n];
    for (i, node) in doc.nodes.iter().enumerate() {
        if matches!(node.data, crate::dom::NodeData::Element(_)) && node.parent != NO_NODE {
            let p = node.parent as usize;
            count[p] += 1;
            index[i] = 0; // filled below
        }
    }
    // second pass in sibling order
    for p in 0..n {
        if count[p] == 0 {
            continue;
        }
        let mut k = 0;
        let mut c = doc.nodes[p].first_child;
        while c != NO_NODE {
            if doc.is_element(c) {
                k += 1;
                index[c as usize] = k;
            }
            c = doc.nodes[c as usize].next;
        }
    }
    (index, count)
}

// ───────────────────────────── rule index ─────────────────────────────

struct IRule {
    selectors: Rc<Vec<Selector>>,
    sel: u16,
    decls: Rc<Vec<Declaration>>,
    block: u32,
    spec: u32,
    order: u32,
    ua: bool,
}

#[derive(Default)]
struct RuleIndex {
    rules: Vec<IRule>,
    by_id: HashMap<Atom, Vec<u32>>,
    by_class: HashMap<Atom, Vec<u32>>,
    by_tag: HashMap<Atom, Vec<u32>>,
    universal: Vec<u32>,
    dynamic: bool,
    /// `@property` registrations; a later rule for the same name replaces an earlier one
    registered: HashMap<Atom, Registered>,
}

/// A custom property registered with `@property`.
pub struct Registered {
    pub inherits: bool,
    pub initial: Option<Rc<str>>,
}

impl RuleIndex {
    fn add_rules(&mut self, rules: &[Rule], origin: Origin, env: &MediaEnv) {
        for r in rules {
            match r {
                Rule::Style(sr) => {
                    for (si, sel) in sr.selectors.iter().enumerate() {
                        if sel.pseudo_element == PseudoElement::Other {
                            continue;
                        }
                        let idx = self.rules.len() as u32;
                        self.rules.push(IRule {
                            selectors: sr.selectors.clone(),
                            sel: si as u16,
                            decls: sr.decls.clone(),
                            block: sr.id,
                            spec: sel.specificity,
                            order: idx,
                            ua: origin == Origin::UserAgent,
                        });
                        self.dynamic |= sel.dynamic;
                        let subject = &sel.compounds[0];
                        if let Some(id) = subject.id {
                            self.by_id.entry(id).or_default().push(idx);
                        } else if let Some(&cl) = subject.classes.first() {
                            self.by_class.entry(cl).or_default().push(idx);
                        } else if let Some(tag) = subject.tag {
                            self.by_tag.entry(tag).or_default().push(idx);
                        } else {
                            self.universal.push(idx);
                        }
                    }
                }
                Rule::Media { query, rules } => {
                    if query.evaluate(env) {
                        self.add_rules(rules, origin, env);
                    }
                }
                Rule::Property { name, inherits, initial } => {
                    self.registered.insert(*name, Registered { inherits: *inherits, initial: initial.clone() });
                }
                Rule::Import { .. } | Rule::FontFace(_) => {}
            }
        }
    }
}

/// What `var()` can see while one element is cascaded.
pub struct VarScope<'a> {
    /// Inherited custom properties, the element's own included.
    pub chain: Option<&'a VarMap>,
    /// The element's own values of registered properties that do not inherit.
    pub local: &'a [(Atom, Rc<str>)],
    pub registered: Option<&'a HashMap<Atom, Registered>>,
}

impl VarScope<'_> {
    /// A scope with no registrations (plain inherited custom properties only).
    pub fn plain(chain: Option<&VarMap>) -> VarScope<'_> {
        VarScope { chain, local: &[], registered: None }
    }

    fn get(&self, name: Atom) -> Option<Rc<str>> {
        let reg = self.registered.and_then(|r| r.get(&name));
        let found = match reg {
            Some(r) if !r.inherits => self.local.iter().rev().find(|(n, _)| *n == name).map(|(_, v)| v.clone()),
            _ => self.chain.and_then(|m| m.get(name)).cloned(),
        };
        // `--x: initial` means "as if never set"
        let found = found.filter(|v| !v.trim().eq_ignore_ascii_case("initial"));
        found.or_else(|| reg.and_then(|r| r.initial.clone()))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Match {
    author: bool,
    spec: u32,
    order: u32,
    rule: u32,
}

// ───────────────────────────── stylist ─────────────────────────────

pub struct SheetEntry {
    pub sheet: Rc<Stylesheet>,
    pub media: MediaQueryList,
}

thread_local! {
    static UA_SHEET: Rc<Stylesheet> = Rc::new(parse_stylesheet(ua::UA_CSS, Origin::UserAgent, &|_| None));
    static UA_SCRIPTING: Rc<Stylesheet> = Rc::new(parse_stylesheet(ua::UA_SCRIPTING_CSS, Origin::UserAgent, &|_| None));
}

pub struct StyleInput<'a> {
    pub doc: &'a Document,
    pub env: MediaEnv,
    pub hover: &'a [NodeId],
    pub active: &'a [NodeId],
    pub focus: NodeId,
    pub target: NodeId,
    pub resolve: UrlResolver<'a>,
    pub scripting: bool,
}

#[derive(Default)]
pub struct Stylist {
    pub sheets: Vec<SheetEntry>,
    index: Option<(MediaEnv, bool, RuleIndex)>,
}

struct Cx {
    root_fs: f32,
    vw: f32,
    vh: f32,
}

impl Stylist {
    pub fn new() -> Stylist {
        Stylist::default()
    }

    pub fn invalidate(&mut self) {
        self.index = None;
    }

    pub fn add_sheet(&mut self, sheet: Rc<Stylesheet>, media: MediaQueryList) {
        self.sheets.push(SheetEntry { sheet, media });
        self.index = None;
    }

    /// True if any indexed rule depends on `:hover`, `:active` or `:focus`.
    pub fn has_dynamic_rules(&self) -> bool {
        self.index.as_ref().map_or(false, |i| i.2.dynamic)
    }

    /// True if styles may change when the viewport is resized.
    pub fn depends_on_viewport(&self) -> bool {
        self.sheets.iter().any(|s| !s.media.is_always() || s.sheet.has_media_rules())
    }

    pub fn rule_count(&self) -> usize {
        self.index.as_ref().map_or(0, |i| i.2.rules.len())
    }

    fn ensure_index(&mut self, env: MediaEnv, scripting: bool) {
        if let Some((e, s, _)) = &self.index {
            if *e == env && *s == scripting {
                return;
            }
        }
        let mut idx = RuleIndex::default();
        UA_SHEET.with(|ua| idx.add_rules(&ua.rules, Origin::UserAgent, &env));
        if scripting {
            UA_SCRIPTING.with(|ua| idx.add_rules(&ua.rules, Origin::UserAgent, &env));
        }
        for entry in &self.sheets {
            if entry.media.evaluate(&env) {
                idx.add_rules(&entry.sheet.rules, entry.sheet.origin, &env);
            }
        }
        self.index = Some((env, scripting, idx));
    }

    /// Compute styles for the whole document.
    pub fn compute(&mut self, input: &StyleInput) -> StyleStore {
        self.ensure_index(input.env, input.scripting);
        let index = &self.index.as_ref().unwrap().2;
        let doc = input.doc;
        let (elem_index, elem_count) = sibling_indices(doc);
        let mctx = MatchCtx {
            doc,
            elem_index: &elem_index,
            elem_count: &elem_count,
            hover: input.hover,
            active: input.active,
            focus: input.focus,
            target: input.target,
        };
        let mut store = StyleStore {
            styles: Vec::with_capacity(256),
            of_node: vec![NO_STYLE; doc.nodes.len()],
            before: HashMap::new(),
            after: HashMap::new(),
            root_font_size: 16.0,
            vw: input.env.width,
            vh: input.env.height,
        };
        let root = doc.document_element();
        if root == NO_NODE {
            return store;
        }
        let mut cache: HashMap<u64, StyleId> = HashMap::with_capacity(512);
        let mut stack: Vec<(NodeId, StyleId)> = vec![(root, NO_STYLE)];
        let mut matched: Vec<Match> = Vec::with_capacity(64);
        let mut before: Vec<Match> = Vec::new();
        let mut after: Vec<Match> = Vec::new();
        let initial = ComputedStyle::initial();

        while let Some((node, parent_sid)) = stack.pop() {
            let Some(el) = doc.element(node) else { continue };
            matched.clear();
            before.clear();
            after.clear();
            collect(index, el, node, &mctx, &mut matched, &mut before, &mut after);
            matched.sort_unstable();

            let hints = presentational_hints(doc, node, el);
            let inline = el.attr(a::STYLE).unwrap_or("");
            let is_root = node == root;

            let mut h = DefaultHasher::new();
            parent_sid.hash(&mut h);
            el.tag.0.hash(&mut h);
            is_root.hash(&mut h);
            for m in &matched {
                index.rules[m.rule as usize].block.hash(&mut h);
            }
            hints.hash(&mut h);
            inline.hash(&mut h);
            let key = h.finish();

            let sid = match cache.get(&key) {
                Some(&sid) => sid,
                None => {
                    let mut cx = Cx { root_fs: store.root_font_size, vw: input.env.width, vh: input.env.height };
                    let parent: &ComputedStyle =
                        if parent_sid == NO_STYLE { &initial } else { &store.styles[parent_sid as usize] };
                    let st = cascade(index, parent, &matched, &hints, inline, is_root, false, &mut cx, input);
                    let sid = store.push(st);
                    cache.insert(key, sid);
                    sid
                }
            };
            store.of_node[node as usize] = sid;
            if is_root {
                store.root_font_size = store.styles[sid as usize].font_size;
            }
            if store.styles[sid as usize].display == Display::None {
                continue;
            }

            for (list, is_before) in [(&mut before, true), (&mut after, false)] {
                if list.is_empty() {
                    continue;
                }
                list.sort_unstable();
                let mut h = DefaultHasher::new();
                (0xBEF0u32, is_before, sid).hash(&mut h);
                for m in list.iter() {
                    index.rules[m.rule as usize].block.hash(&mut h);
                }
                let key = h.finish();
                let psid = match cache.get(&key) {
                    Some(&p) => p,
                    None => {
                        let mut cx = Cx { root_fs: store.root_font_size, vw: input.env.width, vh: input.env.height };
                        let st = cascade(index, &store.styles[sid as usize], &list[..], "", "", false, true, &mut cx, input);
                        let p = store.push(st);
                        cache.insert(key, p);
                        p
                    }
                };
                let ps = &store.styles[psid as usize];
                if ps.content.is_some() && ps.display != Display::None {
                    if is_before {
                        store.before.insert(node, psid);
                    } else {
                        store.after.insert(node, psid);
                    }
                }
            }

            let mut c = doc.node(node).last_child;
            while c != NO_NODE {
                if doc.is_element(c) {
                    stack.push((c, sid));
                }
                c = doc.node(c).prev;
            }
        }
        // Custom-property maps are only needed while cascading.
        for s in &mut store.styles {
            if let Some(m) = Rc::get_mut(s) {
                m.vars = None;
            }
        }
        store.styles.shrink_to_fit();
        store
    }
}

fn collect(
    index: &RuleIndex,
    el: &Element,
    node: NodeId,
    ctx: &MatchCtx,
    matched: &mut Vec<Match>,
    before: &mut Vec<Match>,
    after: &mut Vec<Match>,
) {
    let mut test = |list: &Vec<u32>| {
        for &ri in list {
            let r = &index.rules[ri as usize];
            let sel = &r.selectors[r.sel as usize];
            if matches(sel, node, ctx) {
                let m = Match { author: !r.ua, spec: r.spec, order: r.order, rule: ri };
                match sel.pseudo_element {
                    PseudoElement::None => matched.push(m),
                    PseudoElement::Before => before.push(m),
                    PseudoElement::After => after.push(m),
                    PseudoElement::Other => {}
                }
            }
        }
    };
    if !el.id.is_none() {
        if let Some(l) = index.by_id.get(&el.id) {
            test(l);
        }
    }
    for c in &el.classes {
        if let Some(l) = index.by_class.get(c) {
            test(l);
        }
    }
    if let Some(l) = index.by_tag.get(&el.tag) {
        test(l);
    }
    test(&index.universal);
}

// ───────────────────────────── var() substitution ─────────────────────────────

fn starts_with_ci(hay: &[u8], needle: &[u8]) -> bool {
    hay.len() >= needle.len() && hay[..needle.len()].eq_ignore_ascii_case(needle)
}

/// Replace `var()` / `env()` references. Returns `None` if a reference has no value and no fallback.
pub fn substitute_vars(text: &str, vars: &VarScope, depth: u32) -> Option<String> {
    if depth > 24 {
        return None;
    }
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 16);
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        let is_var = starts_with_ci(&b[i..], b"var(");
        let is_env = !is_var && starts_with_ci(&b[i..], b"env(");
        let boundary = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'-' || b[i - 1] == b'_');
        if (is_var || is_env) && boundary {
            let args_start = i + 4;
            let mut depth_p = 1;
            let mut j = args_start;
            let mut comma: Option<usize> = None;
            while j < b.len() {
                match b[j] {
                    b'(' => depth_p += 1,
                    b')' => {
                        depth_p -= 1;
                        if depth_p == 0 {
                            break;
                        }
                    }
                    b',' if depth_p == 1 && comma.is_none() => comma = Some(j),
                    q @ (b'"' | b'\'') => {
                        j += 1;
                        while j < b.len() && b[j] != q {
                            if b[j] == b'\\' {
                                j += 1;
                            }
                            j += 1;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            let j = j.min(b.len());
            let name_end = comma.unwrap_or(j);
            let name = text[args_start..name_end].trim();
            let fallback = comma.map(|c| &text[c + 1..j]);
            out.push_str(&text[last..i]);
            let val: Option<Rc<str>> = if is_var { vars.get(Atom::new(name)) } else { None };
            match val {
                Some(v) => {
                    if v.contains("var(") {
                        out.push_str(&substitute_vars(&v, vars, depth + 1)?);
                    } else {
                        out.push_str(&v);
                    }
                }
                None => match fallback {
                    Some(f) => out.push_str(&substitute_vars(f, vars, depth + 1)?),
                    None => {
                        if is_env {
                            out.push_str("0px");
                        } else {
                            return None;
                        }
                    }
                },
            }
            i = (j + 1).min(b.len());
            last = i;
            continue;
        }
        i += 1;
    }
    out.push_str(&text[last..]);
    Some(out)
}

// ───────────────────────────── presentational hints ─────────────────────────────

fn html_dimension(v: &str) -> Option<String> {
    let v = v.trim();
    let end = v.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(v.len());
    if end == 0 {
        return None;
    }
    let n: f32 = v[..end].parse().ok()?;
    if v[end..].trim_start().starts_with('%') {
        Some(format!("{}%", n))
    } else {
        Some(format!("{}px", n))
    }
}

fn legacy_color(v: &str) -> Option<String> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    if let Some(hex) = v.strip_prefix('#') {
        if parse_hex_color(hex).is_some() {
            return Some(v.to_string());
        }
        return None;
    }
    if named_color(v).is_some() {
        return Some(v.to_ascii_lowercase());
    }
    if (v.len() == 6 || v.len() == 3) && v.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(format!("#{}", v));
    }
    None
}

fn css_url(v: &str) -> String {
    format!("url(\"{}\")", v.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Map legacy presentational attributes (`bgcolor`, `width`, `align`, ...) to CSS text.
fn presentational_hints(doc: &Document, node: NodeId, el: &Element) -> String {
    if el.attrs.is_empty() && !matches!(el.tag, a::TD | a::TH) {
        return String::new();
    }
    let mut s = String::new();
    let mut put = |k: &str, v: &str| {
        s.push_str(k);
        s.push(':');
        s.push_str(v);
        s.push(';');
    };
    let dim_attr = |name: Atom| el.attr(name).and_then(html_dimension);
    let color_attr = |name: Atom| el.attr(name).and_then(legacy_color);
    let align_text = |v: &str| -> Option<&'static str> {
        match v.to_ascii_lowercase().as_str() {
            "center" | "middle" => Some("-webkit-center"),
            "left" => Some("left"),
            "right" => Some("right"),
            "justify" => Some("justify"),
            _ => None,
        }
    };
    let valign = |v: &str| -> Option<&'static str> {
        match v.to_ascii_lowercase().as_str() {
            "top" => Some("top"),
            "middle" | "center" => Some("middle"),
            "bottom" => Some("bottom"),
            "baseline" => Some("baseline"),
            _ => None,
        }
    };
    match el.tag {
        a::BODY => {
            if let Some(c) = color_attr(a::BGCOLOR) {
                put("background-color", &c);
            }
            if let Some(c) = color_attr(a::TEXT) {
                put("color", &c);
            }
            if let Some(b) = el.attr(a::BACKGROUND) {
                put("background-image", &css_url(b));
            }
        }
        a::TABLE => {
            if let Some(w) = dim_attr(a::WIDTH) {
                put("width", &w);
            }
            if let Some(h) = dim_attr(a::HEIGHT) {
                put("height", &h);
            }
            if let Some(c) = color_attr(a::BGCOLOR) {
                put("background-color", &c);
            }
            if let Some(b) = el.attr(a::BORDER) {
                let n: f32 = b.trim().parse().unwrap_or(1.0);
                if n > 0.0 {
                    put("border", &format!("{}px solid #808080", n));
                }
            }
            if let Some(cs) = el.attr(a::CELLSPACING).and_then(|v| v.trim().parse::<f32>().ok()) {
                put("border-spacing", &format!("{}px", cs));
            }
            match el.attr(a::ALIGN).map(|v| v.to_ascii_lowercase()).as_deref() {
                Some("center") => {
                    put("margin-left", "auto");
                    put("margin-right", "auto");
                }
                Some("left") => put("float", "left"),
                Some("right") => put("float", "right"),
                _ => {}
            }
            if let Some(b) = el.attr(a::BACKGROUND) {
                put("background-image", &css_url(b));
            }
        }
        a::TD | a::TH => {
            if let Some(w) = dim_attr(a::WIDTH) {
                put("width", &w);
            }
            if let Some(h) = dim_attr(a::HEIGHT) {
                put("height", &h);
            }
            if let Some(c) = color_attr(a::BGCOLOR) {
                put("background-color", &c);
            }
            if let Some(v) = el.attr(a::ALIGN).and_then(align_text) {
                put("text-align", v);
            }
            if let Some(v) = el.attr(a::VALIGN).and_then(valign) {
                put("vertical-align", v);
            }
            if el.has_attr(a::NOWRAP) {
                put("white-space", "nowrap");
            }
            if let Some(b) = el.attr(a::BACKGROUND) {
                put("background-image", &css_url(b));
            }
            let table = doc.closest(node, a::TABLE);
            if table != NO_NODE {
                if let Some(cp) = doc.attr(table, a::CELLPADDING).and_then(|v| v.trim().parse::<f32>().ok()) {
                    put("padding", &format!("{}px", cp));
                }
                if let Some(b) = doc.attr(table, a::BORDER) {
                    if b.trim().parse::<f32>().unwrap_or(1.0) > 0.0 {
                        put("border", "1px solid #808080");
                    }
                }
            }
        }
        a::TR | a::THEAD | a::TBODY | a::TFOOT => {
            if let Some(c) = color_attr(a::BGCOLOR) {
                put("background-color", &c);
            }
            if let Some(v) = el.attr(a::ALIGN).and_then(align_text) {
                put("text-align", v);
            }
            if let Some(v) = el.attr(a::VALIGN).and_then(valign) {
                put("vertical-align", v);
            }
            if let Some(h) = dim_attr(a::HEIGHT) {
                put("height", &h);
            }
        }
        a::COL | a::COLGROUP => {
            if let Some(w) = dim_attr(a::WIDTH) {
                put("width", &w);
            }
        }
        a::IMG | a::IFRAME | a::VIDEO | a::CANVAS | a::EMBED | a::OBJECT | a::SVG | a::INPUT => {
            if el.tag != a::INPUT || el.attr(a::TYPE).map_or(false, |t| t.eq_ignore_ascii_case("image")) {
                if let Some(w) = dim_attr(a::WIDTH) {
                    put("width", &w);
                }
                if let Some(h) = dim_attr(a::HEIGHT) {
                    put("height", &h);
                }
            }
            if el.tag == a::IMG {
                if let Some(b) = el.attr(a::BORDER).and_then(|v| v.trim().parse::<f32>().ok()) {
                    put("border", &format!("{}px solid", b));
                }
                if let Some(v) = el.attr(a::HSPACE).and_then(|v| v.trim().parse::<f32>().ok()) {
                    put("margin-left", &format!("{}px", v));
                    put("margin-right", &format!("{}px", v));
                }
                if let Some(v) = el.attr(a::VSPACE).and_then(|v| v.trim().parse::<f32>().ok()) {
                    put("margin-top", &format!("{}px", v));
                    put("margin-bottom", &format!("{}px", v));
                }
                match el.attr(a::ALIGN).map(|v| v.to_ascii_lowercase()).as_deref() {
                    Some("left") => put("float", "left"),
                    Some("right") => put("float", "right"),
                    Some("top") => put("vertical-align", "top"),
                    Some("middle") | Some("absmiddle") => put("vertical-align", "middle"),
                    Some("bottom") => put("vertical-align", "baseline"),
                    _ => {}
                }
            }
            if el.tag == a::INPUT {
                if let Some(n) = el.attr(a::SIZE).and_then(|v| v.trim().parse::<f32>().ok()) {
                    if n > 0.0 {
                        put("width", &format!("{}em", n * 0.55 + 0.8));
                    }
                }
            }
        }
        a::FONT => {
            if let Some(c) = color_attr(a::COLOR) {
                put("color", &c);
            }
            if let Some(f) = el.attr(a::FACE) {
                let clean: String = f.chars().filter(|c| !matches!(c, ';' | '{' | '}' | ':')).collect();
                if !clean.trim().is_empty() {
                    put("font-family", &clean);
                }
            }
            if let Some(sz) = el.attr(a::SIZE) {
                let t = sz.trim();
                let n = t.trim_start_matches('+').parse::<i32>().ok();
                if let Some(n) = n {
                    let abs = if t.starts_with('+') || t.starts_with('-') { 3 + n } else { n };
                    let kw = match abs.clamp(1, 7) {
                        1 => "x-small",
                        2 => "small",
                        3 => "medium",
                        4 => "large",
                        5 => "x-large",
                        6 => "xx-large",
                        _ => "xxx-large",
                    };
                    put("font-size", kw);
                }
            }
        }
        a::P | a::DIV | a::H1 | a::H2 | a::H3 | a::H4 | a::H5 | a::H6 | a::CAPTION => {
            if let Some(v) = el.attr(a::ALIGN).and_then(align_text) {
                put("text-align", v);
            }
        }
        a::HR => {
            if let Some(w) = dim_attr(a::WIDTH) {
                put("width", &w);
            }
            if let Some(h) = el.attr(a::SIZE).and_then(|v| v.trim().parse::<f32>().ok()) {
                put("height", &format!("{}px", (h - 2.0).max(0.0)));
            }
            if let Some(c) = color_attr(a::COLOR) {
                put("color", &c);
                put("background-color", &c);
                put("border-color", &c);
            }
            match el.attr(a::ALIGN).map(|v| v.to_ascii_lowercase()).as_deref() {
                Some("left") => put("margin-left", "0"),
                Some("right") => put("margin-right", "0"),
                _ => {}
            }
        }
        a::BR => match el.attr(Atom::new("clear")).map(|v| v.to_ascii_lowercase()).as_deref() {
            Some("left") => put("clear", "left"),
            Some("right") => put("clear", "right"),
            Some("all") | Some("both") => put("clear", "both"),
            _ => {}
        },
        a::OL | a::UL | a::LI => {
            if let Some(t) = el.attr(a::TYPE) {
                let v = match t {
                    "1" => Some("decimal"),
                    "a" => Some("lower-alpha"),
                    "A" => Some("upper-alpha"),
                    "i" => Some("lower-roman"),
                    "I" => Some("upper-roman"),
                    _ => match t.to_ascii_lowercase().as_str() {
                        "disc" => Some("disc"),
                        "circle" => Some("circle"),
                        "square" => Some("square"),
                        "none" => Some("none"),
                        _ => None,
                    },
                };
                if let Some(v) = v {
                    put("list-style-type", v);
                }
            }
        }
        a::TEXTAREA => {
            let cols = el.attr(a::COLS).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(20.0);
            let rows = el.attr(a::ROWS).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(2.0);
            put("width", &format!("{}em", cols * 0.55 + 1.0));
            put("height", &format!("{}em", rows * 1.25 + 0.5));
        }
        _ => {}
    }
    s
}

// ───────────────────────────── cascade ─────────────────────────────

fn is_inherited(p: PropId) -> bool {
    use PropId::*;
    matches!(
        p,
        Color | FontFamily | FontSize | FontWeight | FontStyle | LineHeight | TextAlign | TextTransform
            | TextIndent | LetterSpacing | WordSpacing | WhiteSpace | Visibility | Cursor | ListStyleType
            | BorderCollapse | BorderSpacing | OverflowWrap | WordBreak | PointerEvents | TextFillColor
    )
}

fn copy_prop(d: &mut ComputedStyle, s: &ComputedStyle, p: PropId) {
    use PropId::*;
    match p {
        Display => d.display = s.display,
        Position => d.position = s.position,
        Float => d.float = s.float,
        Clear => d.clear = s.clear,
        BoxSizing => d.box_sizing = s.box_sizing,
        Visibility => d.visibility = s.visibility,
        Opacity => d.opacity = s.opacity,
        ZIndex => d.z_index = s.z_index,
        OverflowX => d.overflow_x = s.overflow_x,
        OverflowY => d.overflow_y = s.overflow_y,
        Width => d.width = s.width.clone(),
        Height => d.height = s.height.clone(),
        MinWidth => d.min_width = s.min_width.clone(),
        MinHeight => d.min_height = s.min_height.clone(),
        MaxWidth => d.max_width = s.max_width.clone(),
        MaxHeight => d.max_height = s.max_height.clone(),
        MarginTop => d.margin[0] = s.margin[0].clone(),
        MarginRight => d.margin[1] = s.margin[1].clone(),
        MarginBottom => d.margin[2] = s.margin[2].clone(),
        MarginLeft => d.margin[3] = s.margin[3].clone(),
        PaddingTop => d.padding[0] = s.padding[0].clone(),
        PaddingRight => d.padding[1] = s.padding[1].clone(),
        PaddingBottom => d.padding[2] = s.padding[2].clone(),
        PaddingLeft => d.padding[3] = s.padding[3].clone(),
        BorderTopWidth => d.border_width[0] = s.border_width[0],
        BorderRightWidth => d.border_width[1] = s.border_width[1],
        BorderBottomWidth => d.border_width[2] = s.border_width[2],
        BorderLeftWidth => d.border_width[3] = s.border_width[3],
        BorderTopStyle => d.border_style[0] = s.border_style[0],
        BorderRightStyle => d.border_style[1] = s.border_style[1],
        BorderBottomStyle => d.border_style[2] = s.border_style[2],
        BorderLeftStyle => d.border_style[3] = s.border_style[3],
        BorderTopColor | BorderRightColor | BorderBottomColor | BorderLeftColor => {
            let i = match p {
                BorderTopColor => 0,
                BorderRightColor => 1,
                BorderBottomColor => 2,
                _ => 3,
            };
            d.border_color[i] = s.border_color[i];
            d.border_color_set = (d.border_color_set & !(1 << i)) | (s.border_color_set & (1 << i));
        }
        BorderTopLeftRadius => d.border_radius[0] = s.border_radius[0].clone(),
        BorderTopRightRadius => d.border_radius[1] = s.border_radius[1].clone(),
        BorderBottomRightRadius => d.border_radius[2] = s.border_radius[2].clone(),
        BorderBottomLeftRadius => d.border_radius[3] = s.border_radius[3].clone(),
        Top => d.inset[0] = s.inset[0].clone(),
        Right => d.inset[1] = s.inset[1].clone(),
        Bottom => d.inset[2] = s.inset[2].clone(),
        Left => d.inset[3] = s.inset[3].clone(),
        Color => d.color = s.color,
        TextFillColor => d.text_fill = s.text_fill,
        BackgroundClip => d.bg_clip_text = s.bg_clip_text,
        Translate => d.translate = s.translate.clone(),
        BackgroundColor => d.background_color = s.background_color,
        BackgroundImage | BackgroundSize | BackgroundPosition | BackgroundRepeat => d.background = s.background.clone(),
        FontFamily => d.font_family = s.font_family.clone(),
        FontSize => d.font_size = s.font_size,
        FontWeight => d.font_weight = s.font_weight,
        FontStyle => d.italic = s.italic,
        LineHeight => d.line_height = s.line_height,
        TextAlign => d.text_align = s.text_align,
        TextDecorationLine => d.text_decoration_own = s.text_decoration_own,
        TextDecorationColor => {}
        TextTransform => d.text_transform = s.text_transform,
        TextIndent => d.text_indent = s.text_indent,
        LetterSpacing => d.letter_spacing = s.letter_spacing,
        WordSpacing => d.word_spacing = s.word_spacing,
        WhiteSpace => d.white_space = s.white_space,
        VerticalAlign => d.vertical_align = s.vertical_align,
        WordBreak => d.break_all = s.break_all,
        OverflowWrap => d.overflow_wrap = s.overflow_wrap,
        TextOverflow => d.text_overflow_ellipsis = s.text_overflow_ellipsis,
        ListStyleType => d.list_style = s.list_style,
        Cursor => d.cursor = s.cursor,
        PointerEvents => d.pointer_events = s.pointer_events,
        FlexDirection => d.flex_direction = s.flex_direction,
        FlexWrap => d.flex_wrap = s.flex_wrap,
        JustifyContent => d.justify_content = s.justify_content,
        AlignItems => d.align_items = s.align_items,
        AlignSelf => d.align_self = s.align_self,
        AlignContent => d.align_content = s.align_content,
        JustifyItems => d.justify_items = s.justify_items,
        JustifySelf => d.justify_self = s.justify_self,
        FlexGrow => d.flex_grow = s.flex_grow,
        FlexShrink => d.flex_shrink = s.flex_shrink,
        FlexBasis => d.flex_basis = s.flex_basis.clone(),
        Order => d.order = s.order,
        RowGap => d.row_gap = s.row_gap.clone(),
        ColumnGap => d.column_gap = s.column_gap.clone(),
        GridTemplateColumns => d.grid_template_columns = s.grid_template_columns.clone(),
        GridTemplateRows => d.grid_template_rows = s.grid_template_rows.clone(),
        GridAutoRows => d.grid_auto_rows = s.grid_auto_rows.clone(),
        GridAutoColumns => {}
        GridColumnStart => d.grid_column.0 = s.grid_column.0,
        GridColumnEnd => d.grid_column.1 = s.grid_column.1,
        GridRowStart => d.grid_row.0 = s.grid_row.0,
        GridRowEnd => d.grid_row.1 = s.grid_row.1,
        GridAutoFlow => d.grid_auto_flow_column = s.grid_auto_flow_column,
        GridTemplateAreas => d.grid_template_areas = s.grid_template_areas.clone(),
        GridAreaName => d.grid_area = s.grid_area,
        MaskImage => d.masked = s.masked,
        BoxShadow => d.box_shadow = s.box_shadow.clone(),
        BorderCollapse => d.border_collapse = s.border_collapse,
        BorderSpacing => d.border_spacing = s.border_spacing,
        TableLayout => d.table_layout_fixed = s.table_layout_fixed,
        Content => d.content = s.content.clone(),
        ObjectFit => d.object_fit = s.object_fit,
        AspectRatio => d.aspect_ratio = s.aspect_ratio,
        Transform => d.translate = s.translate.clone(),
        OutlineWidth => d.outline_width = s.outline_width,
        OutlineStyle => d.outline_style = s.outline_style,
        OutlineColor => {
            d.outline_color = s.outline_color;
            d.outline_color_set = s.outline_color_set;
        }
        OutlineOffset => d.outline_offset = s.outline_offset,
        _ => {}
    }
}

fn to_align(k: Kw) -> Align {
    match k {
        Kw::Auto => Align::Auto,
        Kw::Stretch => Align::Stretch,
        Kw::Center => Align::Center,
        Kw::Start | Kw::FlexStart | Kw::SelfStart | Kw::Left => Align::Start,
        Kw::End | Kw::FlexEnd | Kw::SelfEnd | Kw::Right => Align::End,
        Kw::Baseline => Align::Baseline,
        Kw::SpaceBetween => Align::SpaceBetween,
        Kw::SpaceAround => Align::SpaceAround,
        Kw::SpaceEvenly => Align::SpaceEvenly,
        _ => Align::Normal,
    }
}

fn to_border_style(k: Kw) -> BorderStyle {
    match k {
        Kw::Hidden => BorderStyle::Hidden,
        Kw::Solid | Kw::Auto => BorderStyle::Solid,
        Kw::Dashed => BorderStyle::Dashed,
        Kw::Dotted => BorderStyle::Dotted,
        Kw::Double => BorderStyle::Double,
        Kw::Groove => BorderStyle::Groove,
        Kw::Ridge => BorderStyle::Ridge,
        Kw::Inset => BorderStyle::Inset,
        Kw::Outset => BorderStyle::Outset,
        _ => BorderStyle::None,
    }
}

fn bg_layers<'s>(st: &'s mut ComputedStyle, n: usize) -> &'s mut Vec<BgLayer> {
    let rc = st.background.get_or_insert_with(|| Rc::new(Vec::new()));
    let layers = Rc::make_mut(rc);
    if layers.len() < n {
        layers.resize(n, BgLayer::default());
    }
    layers
}

fn apply(st: &mut ComputedStyle, parent: &ComputedStyle, prop: PropId, v: &Value, cx: &Cx) {
    use PropId::*;
    if let Value::Kw(k @ (Kw::Inherit | Kw::Initial | Kw::Unset | Kw::Revert)) = v {
        let inherit = match k {
            Kw::Inherit => true,
            Kw::Initial => false,
            _ => is_inherited(prop),
        };
        if inherit {
            copy_prop(st, parent, prop);
        } else {
            let init = ComputedStyle::initial();
            copy_prop(st, &init, prop);
        }
        return;
    }
    let lc = LenCtx { font_size: st.font_size, root_font_size: cx.root_fs, vw: cx.vw, vh: cx.vh };
    let size = |v: &Value| -> Size {
        match v {
            Value::Len(l) => l.to_size(&lc),
            Value::Kw(Kw::MinContent) => Size::MinContent,
            Value::Kw(Kw::MaxContent) => Size::MaxContent,
            Value::Kw(Kw::FitContent) => Size::FitContent,
            _ => Size::Auto,
        }
    };
    let px = |v: &Value| -> f32 {
        match v {
            Value::Len(l) => l.to_px(&lc).unwrap_or(0.0),
            _ => 0.0,
        }
    };
    let kw = match v {
        Value::Kw(k) => Some(*k),
        _ => None,
    };
    match prop {
        Display => {
            if let Some(k) = kw {
                st.display = match k {
                    Kw::None => computed::Display::None,
                    Kw::Block | Kw::FlowRoot => computed::Display::Block,
                    Kw::Inline => computed::Display::Inline,
                    Kw::InlineBlock => computed::Display::InlineBlock,
                    Kw::Flex => computed::Display::Flex,
                    Kw::InlineFlex => computed::Display::InlineFlex,
                    Kw::Grid => computed::Display::Grid,
                    Kw::InlineGrid => computed::Display::InlineGrid,
                    Kw::Table => computed::Display::Table,
                    Kw::InlineTable => computed::Display::InlineTable,
                    Kw::TableRow => computed::Display::TableRow,
                    Kw::TableCell => computed::Display::TableCell,
                    Kw::TableRowGroup => computed::Display::TableRowGroup,
                    Kw::TableHeaderGroup => computed::Display::TableHeaderGroup,
                    Kw::TableFooterGroup => computed::Display::TableFooterGroup,
                    Kw::TableColumn => computed::Display::TableColumn,
                    Kw::TableColumnGroup => computed::Display::TableColumnGroup,
                    Kw::TableCaption => computed::Display::TableCaption,
                    Kw::ListItem => computed::Display::ListItem,
                    Kw::Contents => computed::Display::Contents,
                    _ => st.display,
                };
            }
        }
        Position => {
            st.position = match kw {
                Some(Kw::Relative) => computed::Position::Relative,
                Some(Kw::Absolute) => computed::Position::Absolute,
                Some(Kw::Fixed) => computed::Position::Fixed,
                Some(Kw::Sticky) => computed::Position::Sticky,
                _ => computed::Position::Static,
            }
        }
        Float => {
            st.float = match kw {
                Some(Kw::Left | Kw::InlineStart) => computed::Float::Left,
                Some(Kw::Right | Kw::InlineEnd) => computed::Float::Right,
                _ => computed::Float::None,
            }
        }
        Clear => {
            st.clear = match kw {
                Some(Kw::Left | Kw::InlineStart) => computed::Clear::Left,
                Some(Kw::Right | Kw::InlineEnd) => computed::Clear::Right,
                Some(Kw::Both) => computed::Clear::Both,
                _ => computed::Clear::None,
            }
        }
        BoxSizing => {
            st.box_sizing = if kw == Some(Kw::BorderBox) { computed::BoxSizing::BorderBox } else { computed::BoxSizing::ContentBox }
        }
        Visibility => {
            st.visibility = match kw {
                Some(Kw::Hidden) => computed::Visibility::Hidden,
                Some(Kw::Collapse) => computed::Visibility::Collapse,
                _ => computed::Visibility::Visible,
            }
        }
        Opacity => {
            if let Value::Num(n) = v {
                st.opacity = n.clamp(0.0, 1.0);
            }
        }
        ZIndex => {
            st.z_index = match v {
                Value::Int(i) => Some(*i),
                _ => None,
            }
        }
        OverflowX | OverflowY => {
            let o = match kw {
                Some(Kw::Hidden) => computed::Overflow::Hidden,
                Some(Kw::Scroll) => computed::Overflow::Scroll,
                Some(Kw::Auto | Kw::Overlay) => computed::Overflow::Auto,
                Some(Kw::Clip) => computed::Overflow::Clip,
                _ => computed::Overflow::Visible,
            };
            if prop == OverflowX {
                st.overflow_x = o;
            } else {
                st.overflow_y = o;
            }
        }
        Width => st.width = size(v),
        Height => st.height = size(v),
        MinWidth => st.min_width = size(v),
        MinHeight => st.min_height = size(v),
        MaxWidth => st.max_width = size(v),
        MaxHeight => st.max_height = size(v),
        MarginTop => st.margin[0] = size(v),
        MarginRight => st.margin[1] = size(v),
        MarginBottom => st.margin[2] = size(v),
        MarginLeft => st.margin[3] = size(v),
        PaddingTop => st.padding[0] = size(v),
        PaddingRight => st.padding[1] = size(v),
        PaddingBottom => st.padding[2] = size(v),
        PaddingLeft => st.padding[3] = size(v),
        BorderTopWidth => st.border_width[0] = px(v).max(0.0),
        BorderRightWidth => st.border_width[1] = px(v).max(0.0),
        BorderBottomWidth => st.border_width[2] = px(v).max(0.0),
        BorderLeftWidth => st.border_width[3] = px(v).max(0.0),
        BorderTopStyle => st.border_style[0] = to_border_style(kw.unwrap_or(Kw::None)),
        BorderRightStyle => st.border_style[1] = to_border_style(kw.unwrap_or(Kw::None)),
        BorderBottomStyle => st.border_style[2] = to_border_style(kw.unwrap_or(Kw::None)),
        BorderLeftStyle => st.border_style[3] = to_border_style(kw.unwrap_or(Kw::None)),
        BorderTopColor | BorderRightColor | BorderBottomColor | BorderLeftColor => {
            let i = match prop {
                BorderTopColor => 0,
                BorderRightColor => 1,
                BorderBottomColor => 2,
                _ => 3,
            };
            match v {
                Value::Color(CssColor::Rgba(c)) => {
                    st.border_color[i] = *c;
                    st.border_color_set |= 1 << i;
                }
                _ => st.border_color_set &= !(1 << i),
            }
        }
        BorderTopLeftRadius => st.border_radius[0] = size(v),
        BorderTopRightRadius => st.border_radius[1] = size(v),
        BorderBottomRightRadius => st.border_radius[2] = size(v),
        BorderBottomLeftRadius => st.border_radius[3] = size(v),
        Top => st.inset[0] = size(v),
        Right => st.inset[1] = size(v),
        Bottom => st.inset[2] = size(v),
        Left => st.inset[3] = size(v),
        Color => {
            st.color = match v {
                Value::Color(CssColor::Rgba(c)) => *c,
                _ => parent.color,
            }
        }
        TextFillColor => {
            st.text_fill = match v {
                Value::Color(CssColor::Rgba(c)) => Some(*c),
                Value::Color(CssColor::Current) => None,
                _ => parent.text_fill,
            }
        }
        BackgroundClip => st.bg_clip_text = kw == Some(Kw::Text),
        BackgroundColor => {
            st.background_color = match v {
                Value::Color(CssColor::Rgba(c)) => *c,
                _ => st.color,
            }
        }
        BackgroundImage => {
            if let Value::Images(list) = v {
                let layers = bg_layers(st, list.len());
                layers.truncate(list.len().max(1));
                for (i, img) in list.iter().enumerate() {
                    layers[i].image = img.clone();
                }
            }
        }
        BackgroundSize => {
            if let Value::BgSizes(list) = v {
                if !list.is_empty() {
                    let layers = bg_layers(st, 1);
                    for (i, l) in layers.iter_mut().enumerate() {
                        l.size = list[i % list.len()].clone();
                    }
                }
            }
        }
        BackgroundPosition => {
            if let Value::Positions(list) = v {
                if !list.is_empty() {
                    let layers = bg_layers(st, 1);
                    for (i, l) in layers.iter_mut().enumerate() {
                        l.pos = list[i % list.len()].clone();
                    }
                }
            }
        }
        BackgroundRepeat => {
            if let Value::Repeats(list) = v {
                if !list.is_empty() {
                    let layers = bg_layers(st, 1);
                    for (i, l) in layers.iter_mut().enumerate() {
                        l.repeat = list[i % list.len()];
                    }
                }
            }
        }
        FontFamily => {
            if let Value::Families(f) = v {
                st.font_family = f.clone();
            }
        }
        FontSize => {
            let pfs = parent.font_size;
            st.font_size = match v {
                Value::Kw(k) => match k {
                    Kw::XxSmall => 9.0,
                    Kw::XSmall => 10.0,
                    Kw::Small => 13.0,
                    Kw::Medium => 16.0,
                    Kw::Large => 18.0,
                    Kw::XLarge => 24.0,
                    Kw::XxLarge => 32.0,
                    Kw::XxxLarge => 48.0,
                    Kw::Smaller => pfs / 1.2,
                    Kw::Larger => pfs * 1.2,
                    _ => pfs,
                },
                Value::Len(l) => {
                    let plc = LenCtx { font_size: pfs, root_font_size: cx.root_fs, vw: cx.vw, vh: cx.vh };
                    l.to_size(&plc).resolve(pfs).unwrap_or(pfs).max(0.0)
                }
                _ => pfs,
            };
        }
        FontWeight => {
            st.font_weight = match v {
                Value::Num(n) => *n as u16,
                Value::Kw(Kw::Bolder) => match parent.font_weight {
                    0..=399 => 400,
                    400..=599 => 700,
                    _ => 900,
                },
                Value::Kw(Kw::Lighter) => match parent.font_weight {
                    0..=599 => 100,
                    600..=799 => 400,
                    _ => 700,
                },
                _ => 400,
            }
        }
        FontStyle => st.italic = matches!(kw, Some(Kw::Italic | Kw::Oblique)),
        LineHeight => {
            st.line_height = match v {
                Value::Num(n) => computed::LineHeight::Number(*n),
                Value::Len(Len::Pct(p)) => computed::LineHeight::Px(st.font_size * p / 100.0),
                Value::Len(l) => computed::LineHeight::Px(l.to_px(&lc).unwrap_or(st.font_size * 1.2)),
                _ => computed::LineHeight::Normal,
            }
        }
        TextAlign => {
            st.text_align = match kw {
                Some(Kw::Right | Kw::End) => computed::TextAlign::Right,
                Some(Kw::Center) => computed::TextAlign::Center,
                Some(Kw::Justify) => computed::TextAlign::Justify,
                Some(Kw::WebkitCenter) => computed::TextAlign::BlockCenter,
                _ => computed::TextAlign::Left,
            }
        }
        TextDecorationLine => {
            if let Value::Int(bits) = v {
                st.text_decoration_own = *bits as u8;
            }
        }
        TextDecorationColor => {}
        TextTransform => {
            st.text_transform = match kw {
                Some(Kw::Uppercase) => computed::TextTransform::Uppercase,
                Some(Kw::Lowercase) => computed::TextTransform::Lowercase,
                Some(Kw::Capitalize) => computed::TextTransform::Capitalize,
                _ => computed::TextTransform::None,
            }
        }
        TextIndent => st.text_indent = px(v),
        LetterSpacing => st.letter_spacing = px(v),
        WordSpacing => st.word_spacing = px(v),
        WhiteSpace => {
            st.white_space = match kw {
                Some(Kw::Nowrap) => computed::WhiteSpace::Nowrap,
                Some(Kw::Pre) => computed::WhiteSpace::Pre,
                Some(Kw::PreWrap | Kw::BreakSpaces) => computed::WhiteSpace::PreWrap,
                Some(Kw::PreLine) => computed::WhiteSpace::PreLine,
                _ => computed::WhiteSpace::Normal,
            }
        }
        VerticalAlign => {
            st.vertical_align = match v {
                Value::Kw(k) => match k {
                    Kw::Sub => VAlign::Sub,
                    Kw::Super => VAlign::Super,
                    Kw::Top => VAlign::Top,
                    Kw::Middle => VAlign::Middle,
                    Kw::Bottom => VAlign::Bottom,
                    Kw::TextTop => VAlign::TextTop,
                    Kw::TextBottom => VAlign::TextBottom,
                    _ => VAlign::Baseline,
                },
                Value::Len(Len::Pct(p)) => VAlign::Length(st.font_size * 1.2 * p / 100.0),
                Value::Len(l) => VAlign::Length(l.to_px(&lc).unwrap_or(0.0)),
                _ => VAlign::Baseline,
            }
        }
        WordBreak => match kw {
            Some(Kw::BreakAll) => st.break_all = true,
            Some(Kw::BreakWord) => {
                st.break_all = false;
                st.overflow_wrap = computed::OverflowWrap::BreakWord;
            }
            _ => st.break_all = false,
        },
        OverflowWrap => {
            st.overflow_wrap = match kw {
                Some(Kw::BreakWord) => computed::OverflowWrap::BreakWord,
                Some(Kw::Anywhere) => computed::OverflowWrap::Anywhere,
                _ => computed::OverflowWrap::Normal,
            }
        }
        TextOverflow => st.text_overflow_ellipsis = kw == Some(Kw::Ellipsis),
        ListStyleType => {
            use computed::ListStyle as LS;
            st.list_style = match kw {
                Some(Kw::None) => LS::None,
                Some(Kw::Circle) => LS::Circle,
                Some(Kw::Square) => LS::Square,
                Some(Kw::Decimal) => LS::Decimal,
                Some(Kw::DecimalLeadingZero) => LS::DecimalLeadingZero,
                Some(Kw::LowerAlpha | Kw::LowerLatin) => LS::LowerAlpha,
                Some(Kw::UpperAlpha | Kw::UpperLatin) => LS::UpperAlpha,
                Some(Kw::LowerRoman) => LS::LowerRoman,
                Some(Kw::UpperRoman) => LS::UpperRoman,
                _ => LS::Disc,
            }
        }
        Cursor => {
            st.cursor = match kw {
                Some(Kw::Pointer) => computed::Cursor::Pointer,
                Some(Kw::Text) => computed::Cursor::Text,
                Some(Kw::Move) => computed::Cursor::Move,
                Some(Kw::NotAllowed) => computed::Cursor::NotAllowed,
                Some(Kw::Grab | Kw::Grabbing) => computed::Cursor::Grab,
                Some(Kw::Crosshair) => computed::Cursor::Crosshair,
                Some(Kw::Help) => computed::Cursor::Help,
                Some(Kw::Wait | Kw::Progress) => computed::Cursor::Wait,
                Some(Kw::ColResize | Kw::EwResize) => computed::Cursor::ResizeH,
                Some(Kw::RowResize | Kw::NsResize) => computed::Cursor::ResizeV,
                Some(Kw::Auto) => computed::Cursor::Auto,
                _ => computed::Cursor::Default,
            }
        }
        PointerEvents => st.pointer_events = kw != Some(Kw::None),
        FlexDirection => {
            st.flex_direction = match kw {
                Some(Kw::RowReverse) => computed::FlexDirection::RowReverse,
                Some(Kw::Column) => computed::FlexDirection::Column,
                Some(Kw::ColumnReverse) => computed::FlexDirection::ColumnReverse,
                _ => computed::FlexDirection::Row,
            }
        }
        FlexWrap => {
            st.flex_wrap = match kw {
                Some(Kw::Wrap) => computed::FlexWrap::Wrap,
                Some(Kw::WrapReverse) => computed::FlexWrap::WrapReverse,
                _ => computed::FlexWrap::Nowrap,
            }
        }
        JustifyContent => st.justify_content = to_align(kw.unwrap_or(Kw::Normal)),
        AlignItems => st.align_items = to_align(kw.unwrap_or(Kw::Normal)),
        AlignSelf => st.align_self = to_align(kw.unwrap_or(Kw::Auto)),
        AlignContent => st.align_content = to_align(kw.unwrap_or(Kw::Normal)),
        JustifyItems => st.justify_items = to_align(kw.unwrap_or(Kw::Normal)),
        JustifySelf => st.justify_self = to_align(kw.unwrap_or(Kw::Auto)),
        FlexGrow => {
            if let Value::Num(n) = v {
                st.flex_grow = *n;
            }
        }
        FlexShrink => {
            if let Value::Num(n) = v {
                st.flex_shrink = *n;
            }
        }
        FlexBasis => st.flex_basis = size(v),
        Order => {
            if let Value::Int(i) = v {
                st.order = *i;
            }
        }
        RowGap => st.row_gap = if kw.is_some() { Size::ZERO } else { size(v) },
        ColumnGap => st.column_gap = if kw.is_some() { Size::ZERO } else { size(v) },
        GridTemplateColumns => {
            st.grid_template_columns = match v {
                Value::Tracks(t) => Some(t.clone()),
                _ => None,
            }
        }
        GridTemplateRows => {
            st.grid_template_rows = match v {
                Value::Tracks(t) => Some(t.clone()),
                _ => None,
            }
        }
        GridAutoRows => {
            st.grid_auto_rows = match v {
                Value::Tracks(t) => Some(t.clone()),
                _ => None,
            }
        }
        GridAutoColumns => {}
        GridColumnStart => {
            if let Value::GridLine(g) = v {
                st.grid_column.0 = *g;
            }
        }
        GridColumnEnd => {
            if let Value::GridLine(g) = v {
                st.grid_column.1 = *g;
            }
        }
        GridRowStart => {
            if let Value::GridLine(g) = v {
                st.grid_row.0 = *g;
            }
        }
        GridRowEnd => {
            if let Value::GridLine(g) = v {
                st.grid_row.1 = *g;
            }
        }
        GridAutoFlow => st.grid_auto_flow_column = kw == Some(Kw::Column),
        GridTemplateAreas => {
            st.grid_template_areas = match v {
                Value::Areas(a) => Some(a.clone()),
                _ => None,
            }
        }
        GridAreaName => {
            st.grid_area = match v {
                Value::Name(n) => *n,
                _ => Atom::NONE,
            }
        }
        MaskImage => st.masked = matches!(v, Value::Int(_)),
        BoxShadow => {
            st.box_shadow = match v {
                Value::Shadows(list) => {
                    let col = st.color;
                    let out: Vec<Shadow> = list
                        .iter()
                        .map(|s| Shadow {
                            x: s.x.to_px(&lc).unwrap_or(0.0),
                            y: s.y.to_px(&lc).unwrap_or(0.0),
                            blur: s.blur.to_px(&lc).unwrap_or(0.0).max(0.0),
                            spread: s.spread.to_px(&lc).unwrap_or(0.0),
                            color: s.color.unwrap_or(col),
                            inset: s.inset,
                        })
                        .collect();
                    Some(out.into())
                }
                _ => None,
            }
        }
        BorderCollapse => st.border_collapse = kw == Some(Kw::Collapse),
        BorderSpacing => {
            if let Value::Pair(x, y) = v {
                st.border_spacing = (x.to_px(&lc).unwrap_or(0.0), y.to_px(&lc).unwrap_or(0.0));
            }
        }
        TableLayout => st.table_layout_fixed = kw == Some(Kw::Fixed),
        Content => {
            st.content = match v {
                Value::Content(items) => Some(items.clone()),
                _ => None,
            }
        }
        ObjectFit => {
            st.object_fit = match kw {
                Some(Kw::Contain) => computed::ObjectFit::Contain,
                Some(Kw::Cover) => computed::ObjectFit::Cover,
                Some(Kw::None) => computed::ObjectFit::None,
                Some(Kw::ScaleDown) => computed::ObjectFit::ScaleDown,
                _ => computed::ObjectFit::Fill,
            }
        }
        AspectRatio => {
            st.aspect_ratio = match v {
                Value::Num(n) => Some(*n),
                _ => None,
            }
        }
        Transform | Translate => {
            st.translate = match v {
                Value::Translate(x, y) => Some((x.to_size(&lc), y.to_size(&lc))),
                _ => None,
            }
        }
        OutlineWidth => st.outline_width = px(v).max(0.0),
        OutlineStyle => st.outline_style = to_border_style(kw.unwrap_or(Kw::None)),
        OutlineColor => match v {
            Value::Color(CssColor::Rgba(c)) => {
                st.outline_color = *c;
                st.outline_color_set = true;
            }
            _ => st.outline_color_set = false,
        },
        OutlineOffset => st.outline_offset = px(v),
        _ => {}
    }
}

fn blockify(d: Display) -> Display {
    match d {
        Display::Inline | Display::InlineBlock => Display::Block,
        Display::InlineFlex => Display::Flex,
        Display::InlineGrid => Display::Grid,
        Display::InlineTable => Display::Table,
        Display::TableRow | Display::TableCell | Display::TableRowGroup | Display::TableHeaderGroup
        | Display::TableFooterGroup | Display::TableColumn | Display::TableColumnGroup | Display::TableCaption => {
            Display::Block
        }
        other => other,
    }
}

#[allow(clippy::too_many_arguments)]
fn cascade(
    index: &RuleIndex,
    parent: &ComputedStyle,
    matched: &[Match],
    hints: &str,
    inline: &str,
    is_root: bool,
    _is_pseudo: bool,
    cx: &mut Cx,
    input: &StyleInput,
) -> ComputedStyle {
    let mut st = ComputedStyle::inherit_from(parent);
    let hint_decls = if hints.is_empty() { Vec::new() } else { properties::parse_declaration_list(hints, input.resolve) };
    let inline_decls =
        if inline.is_empty() { Vec::new() } else { properties::parse_declaration_list(inline, input.resolve) };

    // Declarations in ascending precedence.
    let mut seq: Vec<&Declaration> = Vec::with_capacity(64);
    let mut important: Vec<&Declaration> = Vec::new();
    let mut ua_important: Vec<&Declaration> = Vec::new();
    let split = matched.partition_point(|m| !m.author);
    for m in &matched[..split] {
        for d in index.rules[m.rule as usize].decls.iter() {
            if d.important {
                ua_important.push(d);
            } else {
                seq.push(d);
            }
        }
    }
    seq.extend(hint_decls.iter());
    for m in &matched[split..] {
        for d in index.rules[m.rule as usize].decls.iter() {
            if d.important {
                important.push(d);
            } else {
                seq.push(d);
            }
        }
    }
    for d in &inline_decls {
        if d.important {
            important.push(d);
        } else {
            seq.push(d);
        }
    }
    seq.append(&mut important);
    seq.append(&mut ua_important);

    // Custom properties first: they feed var() in everything else. Registered properties that
    // do not inherit stay with this element (`local`); the rest join the inherited chain.
    let registered = &index.registered;
    let mut own: Vec<(Atom, Rc<str>)> = Vec::new();
    let mut local: Vec<(Atom, Rc<str>)> = Vec::new();
    for d in &seq {
        if let Value::Custom(name, text) = &d.value {
            if registered.get(name).map_or(false, |r| !r.inherits) {
                local.push((*name, text.clone()));
            } else {
                own.push((*name, text.clone()));
            }
        }
    }
    if !own.is_empty() || !local.is_empty() {
        let needs_resolve = own.iter().chain(local.iter()).any(|(_, t)| t.contains("var("));
        let had_own = !own.is_empty();
        let raw = if had_own { Some(Rc::new(VarMap { parent: st.vars.clone(), own })) } else { None };
        if needs_resolve {
            let chain = match &raw {
                Some(r) => Some(&**r),
                None => st.vars.as_deref(),
            };
            let scope = VarScope { chain, local: &local, registered: Some(registered) };
            let resolve = |list: &[(Atom, Rc<str>)]| -> Vec<(Atom, Rc<str>)> {
                list.iter()
                    .filter_map(|(n, t)| {
                        if t.contains("var(") {
                            substitute_vars(t, &scope, 0).map(|s| (*n, Rc::from(s.as_str())))
                        } else {
                            Some((*n, t.clone()))
                        }
                    })
                    .collect()
            };
            let resolved_own = raw.as_ref().map(|r| resolve(&r.own));
            let resolved_local = resolve(&local);
            if let Some(resolved) = resolved_own {
                st.vars = Some(Rc::new(VarMap { parent: st.vars.take(), own: resolved }));
            }
            local = resolved_local;
        } else if let Some(raw) = raw {
            st.vars = Some(raw);
        }
    }

    // Resolve var()-dependent values into concrete longhands.
    let mut flat: Vec<(PropId, Value)> = Vec::with_capacity(seq.len() + 8);
    let mut tmp: Vec<Declaration> = Vec::new();
    for d in &seq {
        match &d.value {
            Value::Custom(..) => {}
            Value::Pending(text) => {
                let scope = VarScope { chain: st.vars.as_deref(), local: &local, registered: Some(registered) };
                match substitute_vars(text, &scope, 0) {
                    Some(s) => {
                        let toks = tokenize_value(&s);
                        tmp.clear();
                        properties::parse_value(d.prop, &toks, false, input.resolve, &mut tmp);
                        for t in tmp.drain(..) {
                            flat.push((t.prop, t.value));
                        }
                    }
                    None => {
                        // invalid at computed-value time: behaves as `unset`
                        if d.prop.is_shorthand() {
                            for &l in longhands(d.prop) {
                                flat.push((l, Value::Kw(Kw::Unset)));
                            }
                        } else {
                            flat.push((d.prop, Value::Kw(Kw::Unset)));
                        }
                    }
                }
            }
            v => flat.push((d.prop, v.clone())),
        }
    }

    // Phase 1: properties other values depend on (em units, currentColor).
    for (p, v) in &flat {
        if matches!(p, PropId::FontSize | PropId::Color) {
            apply(&mut st, parent, *p, v, cx);
        }
    }
    if is_root {
        cx.root_fs = st.font_size;
    }
    // Phase 2: everything else.
    for (p, v) in &flat {
        if !matches!(p, PropId::FontSize | PropId::Color) {
            apply(&mut st, parent, *p, v, cx);
        }
    }

    // ── fix-ups ──
    if st.is_out_of_flow() {
        st.float = Float::None;
    }
    let parent_is_flex_or_grid = parent.display.is_flex() || parent.display.is_grid();
    if st.display != Display::None
        && st.display != Display::Contents
        && (is_root || st.is_out_of_flow() || st.float != Float::None || parent_is_flex_or_grid)
    {
        st.display = blockify(st.display);
    }
    for i in 0..4 {
        if matches!(st.border_style[i], BorderStyle::None | BorderStyle::Hidden) {
            st.border_width[i] = 0.0;
        }
        if st.border_color_set & (1 << i) == 0 {
            st.border_color[i] = st.color;
        }
    }
    if st.outline_style == BorderStyle::None {
        st.outline_width = 0.0;
    }
    if !st.outline_color_set {
        st.outline_color = st.color;
    }
    let propagates = !(st.is_out_of_flow()
        || st.float != Float::None
        || matches!(st.display, Display::InlineBlock | Display::InlineFlex | Display::InlineGrid | Display::InlineTable));
    st.text_decoration = st.text_decoration_own | if propagates { parent.text_decoration } else { 0 };
    if let Some(bg) = &st.background {
        if bg.iter().all(|l| matches!(l.image, crate::css::values::Image::None)) {
            st.background = None;
        }
    }
    if st.bg_clip_text {
        st.text_bg = st.background.clone();
    }
    st
}

/// A style for an anonymous box: inherits from `parent`, everything else initial, no borders.
pub fn anonymous_style(parent: &ComputedStyle, display: Display) -> ComputedStyle {
    let mut st = ComputedStyle::inherit_from(parent);
    st.display = display;
    st.border_width = [0.0; 4];
    st.outline_width = 0.0;
    st.text_decoration = parent.text_decoration;
    st.vars = None;
    st
}

/// Parse a legacy colour attribute value (used by the page for `<body link=...>` etc.).
pub fn parse_legacy_color(v: &str) -> Option<Color> {
    let s = legacy_color(v)?;
    match s.strip_prefix('#') {
        Some(h) => parse_hex_color(h),
        None => named_color(&s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html;

    fn styles(html_src: &str, css: &str) -> (Document, StyleStore) {
        let doc = html::parse(html_src, false);
        let mut stylist = Stylist::new();
        let resolve = |u: &str| Some(u.to_string());
        stylist.add_sheet(Rc::new(parse_stylesheet(css, Origin::Author, &resolve)), MediaQueryList::default());
        let input = StyleInput {
            doc: &doc,
            env: MediaEnv { width: 1000.0, height: 800.0, dark: false },
            hover: &[],
            active: &[],
            focus: NO_NODE,
            target: NO_NODE,
            resolve: &resolve,
            scripting: false,
        };
        let store = stylist.compute(&input);
        (doc, store)
    }

    #[test]
    fn cascade_basics() {
        let (doc, st) = styles(
            "<div id=a class=card><p id=p>hi <b id=b>x</b></p></div>",
            ":root { --pad: 24px; --accent: #3b82f6 } .card { padding: var(--pad); color: var(--accent); font-size: 20px; border: 1px solid } p { margin: 0 0 0.5em } #p { color: red !important } p { color: green }",
        );
        let a = st.get(doc.element_by_id("a")).unwrap();
        assert_eq!(a.padding[0], Size::Px(24.0));
        assert_eq!(a.color, Color::hex(0x3b82f6));
        assert_eq!(a.border_width[0], 1.0);
        assert_eq!(a.border_color[0], Color::hex(0x3b82f6)); // currentColor
        assert_eq!(a.display, Display::Block);
        let p = st.get(doc.element_by_id("p")).unwrap();
        assert_eq!(p.color, Color::rgb(255, 0, 0));
        assert_eq!(p.font_size, 20.0);
        assert_eq!(p.margin[2], Size::Px(10.0));
        let b = st.get(doc.element_by_id("b")).unwrap();
        assert_eq!(b.font_weight, 700);
        assert_eq!(b.color, Color::rgb(255, 0, 0));
        assert_eq!(b.display, Display::Inline);
    }

    #[test]
    fn registered_properties_have_initial_values_and_may_not_inherit() {
        // the shape of what utility-class frameworks emit
        let css = "
            @property --bs { syntax: \"*\"; inherits: false; initial-value: solid }
            @property --w { syntax: \"<length>\"; inherits: false; initial-value: 0px }
            @property --shared { syntax: \"*\"; inherits: true; initial-value: 7px }
            @property --broken { syntax: \"*\" }
            .border { border-style: var(--bs); border-width: 2px }
            .dashed { --bs: dashed }
            .pad { padding-left: var(--w) }
            .wide { --w: 30px }
            .m { margin-left: var(--shared) }
            .set { --shared: 11px }
            .reset { --w: initial }
            .x { margin-top: var(--broken, 3px) }";
        let (doc, st) = styles(
            "<div id=a class='border dashed wide pad set m'><div id=b class='border pad m'><span id=c class='pad wide reset x'>t</span></div></div>
             <div id=d class='border m'></div>",
            css,
        );
        let get = |id: &str| st.get(doc.element_by_id(id)).unwrap();
        // declared on the element itself
        assert_eq!(get("a").border_style[0], crate::style::computed::BorderStyle::Dashed);
        assert_eq!(get("a").padding[3], Size::Px(30.0));
        assert_eq!(get("a").margin[3], Size::Px(11.0));
        // not inherited: the child is back at the registered initial values...
        assert_eq!(get("b").border_style[0], crate::style::computed::BorderStyle::Solid);
        assert_eq!(get("b").border_width[0], 2.0);
        assert_eq!(get("b").padding[3], Size::Px(0.0));
        // ...while an inheriting registration does reach it
        assert_eq!(get("b").margin[3], Size::Px(11.0));
        // `initial` wins over an earlier value on the same element
        assert_eq!(get("c").padding[3], Size::Px(0.0));
        // a registration without `inherits` is invalid and ignored
        assert_eq!(get("c").margin[0], Size::Px(3.0));
        // never set anywhere: the initial value
        assert_eq!(get("d").border_style[0], crate::style::computed::BorderStyle::Solid);
        assert_eq!(get("d").margin[3], Size::Px(7.0));
    }

    #[test]
    fn framework_idioms_for_text() {
        let (doc, st) = styles(
            "<h3 id=a class=t>x</h3><p id=b class=g><b id=c>y</b></p><p id=d class=moved>z</p>",
            ":root { --lh: calc(2 / 1.5) } .t { font-size: 24px; line-height: var(--lh) }
             .g { background: linear-gradient(90deg, red, blue); -webkit-background-clip: text; background-clip: text; -webkit-text-fill-color: transparent }
             .moved { translate: 10px -50% }",
        );
        let get = |id: &str| st.get(doc.element_by_id(id)).unwrap();
        // a calc() of plain numbers is a multiplier, not 1.33 pixels
        assert_eq!(get("a").line_height, crate::style::computed::LineHeight::Number(2.0 / 1.5));
        // gradient text: the element keeps its background for its text, descendants inherit it
        assert!(get("b").bg_clip_text && get("b").text_bg.is_some());
        assert_eq!(get("b").text_fill, Some(Color::TRANSPARENT));
        assert!(!get("c").bg_clip_text && get("c").text_bg.is_some());
        assert_eq!(get("c").text_fill, Some(Color::TRANSPARENT));
        assert!(get("a").text_bg.is_none() && get("a").text_fill.is_none());
        // the standalone translate property
        assert_eq!(get("d").translate, Some((Size::Px(10.0), Size::Pct(-50.0))));
    }

    #[test]
    fn sharing_and_pseudo() {
        let (doc, st) = styles(
            "<ul><li class=i>a</li><li class=i>b</li><li class=i>c</li></ul><div id=t class=toggle></div>",
            ".i { color: blue } .toggle::after { content: ''; position: absolute; width: 16px }",
        );
        let lis: Vec<NodeId> = doc.descendants(0).filter(|&n| doc.tag(n) == a::LI).collect();
        assert_eq!(lis.len(), 3);
        assert_eq!(st.id_of(lis[0]), st.id_of(lis[1]));
        assert_eq!(st.id_of(lis[1]), st.id_of(lis[2]));
        let t = doc.element_by_id("t");
        let after = st.style(*st.after.get(&t).unwrap());
        assert_eq!(after.position, Position::Absolute);
        assert_eq!(after.display, Display::Block);
        assert_eq!(after.width, Size::Px(16.0));
    }

    #[test]
    fn hints_and_ua() {
        let (doc, st) = styles("<table id=t width=85% bgcolor=#f6f6ef cellpadding=4><tr><td id=c align=center>x</table><a id=l href=x>y</a>", "");
        let t = st.get(doc.element_by_id("t")).unwrap();
        assert_eq!(t.width, Size::Pct(85.0));
        assert_eq!(t.background_color, Color::hex(0xf6f6ef));
        assert_eq!(t.display, Display::Table);
        let c = st.get(doc.element_by_id("c")).unwrap();
        assert_eq!(c.padding[0], Size::Px(4.0));
        assert_eq!(c.text_align, TextAlign::BlockCenter);
        let l = st.get(doc.element_by_id("l")).unwrap();
        assert_eq!(l.text_decoration, DECO_UNDERLINE);
        assert_eq!(l.cursor, Cursor::Pointer);
    }
}
