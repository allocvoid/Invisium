//! Stylesheet parser: rules, at-rules, media queries, `@supports`, and native CSS nesting.

use super::properties::{self, Declaration, PropId, UrlResolver};
use super::selector::{parse_selector_list, Selector};
use super::tokenizer::{split_commas, tokenize, Cur, Tok};
use super::values::{parse_len, LenCtx};
use crate::atom::Atom;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    UserAgent,
    Author,
}

pub struct StyleRule {
    pub selectors: Rc<Vec<Selector>>,
    pub decls: Rc<Vec<Declaration>>,
    /// Process-unique id of the declaration block (used as a style-sharing key).
    pub id: u32,
}

#[derive(Clone, Debug)]
pub struct FontFaceRule {
    pub family: String,
    pub sources: Vec<(String, Option<String>)>,
    pub weight: (u16, u16),
    pub italic: bool,
    /// `unicode-range`: inclusive code point ranges; empty = every character.
    pub ranges: Vec<(u32, u32)>,
}

/// Parse the raw text of a `unicode-range` value (`U+0-7F, U+4??, U+20AC`). Items that do not
/// parse are skipped; the list is capped at 512 ranges.
fn parse_unicode_range(text: &str) -> Vec<(u32, u32)> {
    fn hex(s: &str) -> Option<u32> {
        if s.is_empty() || s.len() > 6 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(s, 16).ok()
    }
    let mut out = Vec::new();
    for item in text.split(',') {
        if out.len() >= 512 {
            break;
        }
        let t = item.trim();
        let Some(rest) = t.get(..2).filter(|p| p.eq_ignore_ascii_case("u+")).map(|_| &t[2..]) else { continue };
        let range = if let Some((a, b)) = rest.split_once('-') {
            hex(a).zip(hex(b))
        } else if rest.contains('?') {
            let digits = rest.trim_end_matches('?');
            let q = rest.len() - digits.len();
            if rest.len() > 6 || (!digits.is_empty() && hex(digits).is_none()) {
                None
            } else {
                let lo = format!("{}{}", digits, "0".repeat(q));
                let hi = format!("{}{}", digits, "F".repeat(q));
                hex(&lo).zip(hex(&hi))
            }
        } else {
            hex(rest).map(|v| (v, v))
        };
        if let Some((a, b)) = range {
            let b = b.min(0x10FFFF);
            if a <= b {
                out.push((a, b));
            }
        }
    }
    out
}

pub enum Rule {
    Style(StyleRule),
    Media { query: MediaQueryList, rules: Vec<Rule> },
    Import { url: String, media: MediaQueryList },
    FontFace(FontFaceRule),
    /// `@property --name { inherits: …; initial-value: … }`: a registered custom property.
    /// The `syntax` descriptor is not checked; the initial value is kept as text.
    Property { name: Atom, inherits: bool, initial: Option<Rc<str>> },
    /// `@keyframes name { … }`.
    Keyframes { name: Atom, frames: Rc<Vec<KeyframeRule>> },
}

/// One block of a `@keyframes` rule: the offsets it applies to (0..=1) and its declarations.
pub struct KeyframeRule {
    pub offsets: Vec<f32>,
    pub decls: Vec<Declaration>,
}

pub struct Stylesheet {
    pub rules: Vec<Rule>,
    pub origin: Origin,
}

static NEXT_BLOCK_ID: AtomicU32 = AtomicU32::new(1);

pub fn next_block_id() -> u32 {
    NEXT_BLOCK_ID.fetch_add(1, Ordering::Relaxed)
}

// ───────────────────────────── media queries ─────────────────────────────

#[derive(Clone, Debug)]
pub enum MediaFeature {
    MinWidth(f32),
    MaxWidth(f32),
    MinHeight(f32),
    MaxHeight(f32),
    Landscape(bool),
    Dark(bool),
    ReducedMotion(bool),
    Const(bool),
}

#[derive(Clone, Debug)]
pub struct MediaQuery {
    pub negated: bool,
    /// False for `print`, `speech` and unknown media types.
    pub type_ok: bool,
    pub features: Vec<MediaFeature>,
}

#[derive(Clone, Debug, Default)]
pub struct MediaQueryList(pub Vec<MediaQuery>);

/// The environment media queries are evaluated against. The privacy layer decides what goes in
/// here (e.g. a letterboxed viewport size and a fixed colour scheme).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MediaEnv {
    pub width: f32,
    pub height: f32,
    pub dark: bool,
}

impl MediaQueryList {
    pub fn is_always(&self) -> bool {
        self.0.is_empty()
    }

    pub fn evaluate(&self, env: &MediaEnv) -> bool {
        if self.0.is_empty() {
            return true;
        }
        self.0.iter().any(|q| {
            let ok = q.type_ok
                && q.features.iter().all(|f| match f {
                    MediaFeature::MinWidth(v) => env.width >= *v,
                    MediaFeature::MaxWidth(v) => env.width <= *v,
                    MediaFeature::MinHeight(v) => env.height >= *v,
                    MediaFeature::MaxHeight(v) => env.height <= *v,
                    MediaFeature::Landscape(l) => (env.width >= env.height) == *l,
                    MediaFeature::Dark(d) => env.dark == *d,
                    // a fixed answer for everyone: no preference
                    MediaFeature::ReducedMotion(r) => !*r,
                    MediaFeature::Const(b) => *b,
                });
            ok != q.negated
        })
    }
}

const MQ_CTX: LenCtx = LenCtx { font_size: 16.0, root_font_size: 16.0, vw: 0.0, vh: 0.0 };

fn mq_len(c: &mut Cur) -> Option<f32> {
    parse_len(c, true).and_then(|l| l.to_px(&MQ_CTX))
}

fn parse_media_feature(inner: &[Tok]) -> MediaFeature {
    let mut c = Cur::new(inner);
    c.skip_ws();
    // Range syntax: (width >= 600px), (400px <= width <= 800px)
    let has_cmp = inner.iter().any(|t| matches!(t, Tok::Delim('<') | Tok::Delim('>')));
    if has_cmp {
        return parse_range_feature(inner);
    }
    let Some(name) = c.ident() else { return MediaFeature::Const(false) };
    let name = name.to_ascii_lowercase();
    c.skip_ws();
    let has_value = matches!(c.peek(), Some(Tok::Colon));
    if has_value {
        c.next();
        c.skip_ws();
    }
    match name.as_str() {
        "min-width" | "min-device-width" => mq_len(&mut c).map_or(MediaFeature::Const(false), MediaFeature::MinWidth),
        "max-width" | "max-device-width" => mq_len(&mut c).map_or(MediaFeature::Const(false), MediaFeature::MaxWidth),
        "min-height" | "min-device-height" => mq_len(&mut c).map_or(MediaFeature::Const(false), MediaFeature::MinHeight),
        "max-height" | "max-device-height" => mq_len(&mut c).map_or(MediaFeature::Const(false), MediaFeature::MaxHeight),
        "orientation" => match c.ident() {
            Some(v) => MediaFeature::Landscape(v.eq_ignore_ascii_case("landscape")),
            None => MediaFeature::Const(true),
        },
        "prefers-color-scheme" => match c.ident() {
            Some(v) => MediaFeature::Dark(v.eq_ignore_ascii_case("dark")),
            None => MediaFeature::Const(true),
        },
        "prefers-reduced-motion" => match c.ident() {
            Some(v) => MediaFeature::ReducedMotion(v.eq_ignore_ascii_case("reduce")),
            None => MediaFeature::Const(false),
        },
        "hover" | "any-hover" => match c.ident() {
            Some(v) => MediaFeature::Const(v.eq_ignore_ascii_case("hover")),
            None => MediaFeature::Const(true),
        },
        "pointer" | "any-pointer" => match c.ident() {
            Some(v) => MediaFeature::Const(v.eq_ignore_ascii_case("fine")),
            None => MediaFeature::Const(true),
        },
        "color" | "grid" if !has_value => MediaFeature::Const(name == "color"),
        "min-color" | "color-gamut" | "scripting" => match c.ident() {
            Some(v) => MediaFeature::Const(v.eq_ignore_ascii_case("srgb") || v.eq_ignore_ascii_case("enabled")),
            None => MediaFeature::Const(true),
        },
        "prefers-contrast" | "forced-colors" | "prefers-reduced-transparency" | "prefers-reduced-data"
        | "inverted-colors" => match c.ident() {
            Some(v) => MediaFeature::Const(v.eq_ignore_ascii_case("no-preference") || v.eq_ignore_ascii_case("none")),
            None => MediaFeature::Const(false),
        },
        "display-mode" => match c.ident() {
            Some(v) => MediaFeature::Const(v.eq_ignore_ascii_case("browser")),
            None => MediaFeature::Const(true),
        },
        // Device pixel ratio is always reported as 1 (a fingerprinting surface otherwise).
        "min-resolution" | "-webkit-min-device-pixel-ratio" | "min-device-pixel-ratio" => {
            let v = match c.next() {
                Some(Tok::Number(n)) => *n,
                Some(Tok::Dimension(n, u)) => match u.as_str() {
                    "dppx" | "x" => *n,
                    "dpi" => *n / 96.0,
                    "dpcm" => *n * 2.54 / 96.0,
                    _ => 99.0,
                },
                _ => 99.0,
            };
            MediaFeature::Const(v <= 1.0)
        }
        "max-resolution" | "-webkit-max-device-pixel-ratio" | "max-device-pixel-ratio" => {
            let v = match c.next() {
                Some(Tok::Number(n)) => *n,
                Some(Tok::Dimension(n, u)) => match u.as_str() {
                    "dppx" | "x" => *n,
                    "dpi" => *n / 96.0,
                    "dpcm" => *n * 2.54 / 96.0,
                    _ => 0.0,
                },
                _ => 0.0,
            };
            MediaFeature::Const(v >= 1.0)
        }
        "width" | "height" if has_value => {
            // exact match is practically never true; treat as min==max
            MediaFeature::Const(false)
        }
        "width" | "height" | "aspect-ratio" | "resolution" | "update" | "overflow-block" | "overflow-inline"
            if !has_value =>
        {
            MediaFeature::Const(true)
        }
        _ => MediaFeature::Const(false),
    }
}

fn parse_range_feature(inner: &[Tok]) -> MediaFeature {
    // Supports: name OP value | value OP name | value OP name OP value (first comparison only + second
    // folded in via a nested feature list is overkill; the two-sided form is split by the caller).
    #[derive(Clone, Copy, PartialEq)]
    enum Op {
        Lt,
        Le,
        Gt,
        Ge,
    }
    let mut c = Cur::new(inner);
    c.skip_ws();
    let read_op = |c: &mut Cur| -> Option<Op> {
        c.skip_ws();
        let lt = match c.next()? {
            Tok::Delim('<') => true,
            Tok::Delim('>') => false,
            _ => return None,
        };
        let eq = matches!(c.peek(), Some(Tok::Delim('=')));
        if eq {
            c.next();
        }
        Some(match (lt, eq) {
            (true, true) => Op::Le,
            (true, false) => Op::Lt,
            (false, true) => Op::Ge,
            (false, false) => Op::Gt,
        })
    };
    let make = |name: &str, op: Op, v: f32| -> MediaFeature {
        // name OP v
        let width = name == "width";
        if name != "width" && name != "height" {
            return MediaFeature::Const(false);
        }
        match op {
            Op::Ge => {
                if width {
                    MediaFeature::MinWidth(v)
                } else {
                    MediaFeature::MinHeight(v)
                }
            }
            Op::Gt => {
                if width {
                    MediaFeature::MinWidth(v + 0.01)
                } else {
                    MediaFeature::MinHeight(v + 0.01)
                }
            }
            Op::Le => {
                if width {
                    MediaFeature::MaxWidth(v)
                } else {
                    MediaFeature::MaxHeight(v)
                }
            }
            Op::Lt => {
                if width {
                    MediaFeature::MaxWidth(v - 0.01)
                } else {
                    MediaFeature::MaxHeight(v - 0.01)
                }
            }
        }
    };
    let flip = |op: Op| match op {
        Op::Lt => Op::Gt,
        Op::Le => Op::Ge,
        Op::Gt => Op::Lt,
        Op::Ge => Op::Le,
    };
    if let Some(name) = c.clone().ident() {
        let name = name.to_ascii_lowercase();
        c.ident();
        let Some(op) = read_op(&mut c) else { return MediaFeature::Const(false) };
        let Some(v) = mq_len(&mut c) else { return MediaFeature::Const(false) };
        return make(&name, op, v);
    }
    let Some(v) = mq_len(&mut c) else { return MediaFeature::Const(false) };
    let Some(op) = read_op(&mut c) else { return MediaFeature::Const(false) };
    let Some(name) = c.ident() else { return MediaFeature::Const(false) };
    make(&name.to_ascii_lowercase(), flip(op), v)
}

/// Split `400px <= width <= 800px` into two single comparisons.
fn range_features(inner: &[Tok]) -> Vec<MediaFeature> {
    let cmp_positions: Vec<usize> = inner
        .iter()
        .enumerate()
        .filter(|(_, t)| matches!(t, Tok::Delim('<') | Tok::Delim('>')))
        .map(|(i, _)| i)
        .collect();
    if cmp_positions.len() == 2 {
        // value OP name OP value
        let second = cmp_positions[1];
        let name_start = {
            let mut i = second;
            while i > 0 && !matches!(inner[i - 1], Tok::Ident(_)) {
                i -= 1;
            }
            i.saturating_sub(1)
        };
        let first: Vec<Tok> = inner[..second].to_vec();
        let second_part: Vec<Tok> = inner[name_start..].to_vec();
        vec![parse_range_feature(&first), parse_range_feature(&second_part)]
    } else {
        vec![parse_range_feature(inner)]
    }
}

pub fn parse_media_query_list(toks: &[Tok]) -> MediaQueryList {
    let mut out = Vec::new();
    for part in split_commas(toks) {
        let mut q = MediaQuery { negated: false, type_ok: true, features: Vec::new() };
        let mut c = Cur::new(part);
        let mut any = false;
        loop {
            c.skip_ws();
            match c.next() {
                None => break,
                Some(Tok::Ident(i)) => {
                    any = true;
                    match i.to_ascii_lowercase().as_str() {
                        "not" => q.negated = !q.negated,
                        "only" | "and" => {}
                        "or" => {
                            // `a or b` inside one query: approximate as satisfied by either
                            out.push(std::mem::replace(
                                &mut q,
                                MediaQuery { negated: false, type_ok: true, features: Vec::new() },
                            ));
                        }
                        "all" | "screen" => {}
                        _ => q.type_ok = false,
                    }
                }
                Some(Tok::LParen) => {
                    any = true;
                    let inner = c.block();
                    let has_cmp = inner.t.iter().any(|t| matches!(t, Tok::Delim('<') | Tok::Delim('>')));
                    if has_cmp {
                        q.features.extend(range_features(inner.t));
                    } else if matches!(inner.t.iter().find(|t| !matches!(t, Tok::Whitespace)), Some(Tok::LParen))
                        || inner.t.iter().any(|t| t.is_ident("not"))
                    {
                        // nested condition: evaluate as a sub-list that must hold
                        let sub = parse_media_query_list(inner.t);
                        // fold: cannot evaluate lazily here, so keep simple cases only
                        if sub.0.len() == 1 && !sub.0[0].negated {
                            q.features.extend(sub.0[0].features.iter().cloned());
                        } else {
                            q.features.push(MediaFeature::Const(false));
                        }
                    } else {
                        q.features.push(parse_media_feature(inner.t));
                    }
                }
                Some(_) => {
                    any = true;
                    q.type_ok = false;
                }
            }
        }
        if any {
            out.push(q);
        }
    }
    MediaQueryList(out)
}

// ───────────────────────────── @supports ─────────────────────────────

fn supports_condition(toks: &[Tok]) -> bool {
    let mut c = Cur::new(toks);
    let mut result: Option<bool> = None;
    let mut negate_next = false;
    let mut op_or = false;
    loop {
        c.skip_ws();
        let Some(t) = c.next() else { break };
        let val = match t {
            Tok::Ident(i) => {
                match i.to_ascii_lowercase().as_str() {
                    "not" => negate_next = true,
                    "and" => op_or = false,
                    "or" => op_or = true,
                    _ => {}
                }
                continue;
            }
            Tok::LParen => {
                let inner = c.block();
                let it = inner.t;
                let first = it.iter().find(|t| !matches!(t, Tok::Whitespace));
                if matches!(first, Some(Tok::LParen)) || matches!(first, Some(Tok::Ident(i)) if i.eq_ignore_ascii_case("not")) {
                    supports_condition(it)
                } else {
                    // (property: value)
                    match it.iter().position(|t| matches!(t, Tok::Colon)) {
                        Some(p) => {
                            let name: String = it[..p]
                                .iter()
                                .filter_map(|t| if let Tok::Ident(i) = t { Some(i.to_ascii_lowercase()) } else { None })
                                .collect();
                            if name.starts_with("--") {
                                true
                            } else {
                                match properties::prop_from_name(&name) {
                                    Some(prop) => {
                                        let mut out = Vec::new();
                                        properties::parse_value(prop, &it[p + 1..], false, &|_| None, &mut out);
                                        !out.is_empty()
                                    }
                                    None => false,
                                }
                            }
                        }
                        None => false,
                    }
                }
            }
            Tok::Function(f) => {
                let inner = c.block();
                if f.eq_ignore_ascii_case("selector") {
                    !parse_selector_list(inner.t, None).is_empty()
                } else {
                    false
                }
            }
            _ => continue,
        };
        let val = if negate_next { !val } else { val };
        negate_next = false;
        result = Some(match result {
            None => val,
            Some(r) => {
                if op_or {
                    r || val
                } else {
                    r && val
                }
            }
        });
    }
    result.unwrap_or(false)
}

// ───────────────────────────── rule parser ─────────────────────────────

struct Parser<'a> {
    toks: &'a [Tok],
    spans: &'a [(u32, u32)],
    src: &'a str,
    resolve: UrlResolver<'a>,
}

impl<'a> Parser<'a> {
    /// Index of the matching `}` for the `{` at `open`.
    fn block_end(&self, open: usize, limit: usize) -> usize {
        let mut depth = 0;
        let mut i = open;
        while i < limit {
            match &self.toks[i] {
                Tok::LBrace => depth += 1,
                Tok::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        return i;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        limit
    }

    /// Scan from `i` to the first top-level `{` or `;` (whichever comes first).
    fn scan_prelude(&self, mut i: usize, end: usize) -> (usize, bool) {
        let mut depth = 0;
        while i < end {
            match &self.toks[i] {
                Tok::Function(_) | Tok::LParen | Tok::LBracket => depth += 1,
                Tok::RParen | Tok::RBracket => depth -= 1,
                Tok::LBrace if depth <= 0 => return (i, true),
                Tok::Semicolon if depth <= 0 => return (i, false),
                Tok::RBrace if depth <= 0 => return (i, false),
                _ => {}
            }
            i += 1;
        }
        (end, false)
    }

    fn rules(&mut self, start: usize, end: usize, parent: Option<&[Selector]>, out: &mut Vec<Rule>) {
        let mut i = start;
        while i < end {
            match &self.toks[i] {
                Tok::Whitespace | Tok::Semicolon | Tok::RBrace => {
                    i += 1;
                }
                Tok::AtKeyword(name) => {
                    let name = name.to_ascii_lowercase();
                    i = self.at_rule(&name, i + 1, end, parent, out);
                }
                _ => {
                    let (pos, is_block) = self.scan_prelude(i, end);
                    if !is_block {
                        i = pos + 1;
                        continue;
                    }
                    let close = self.block_end(pos, end);
                    let selectors = parse_selector_list(&self.toks[i..pos], parent);
                    if !selectors.is_empty() {
                        self.style_block(selectors, pos + 1, close, out);
                    }
                    i = close + 1;
                }
            }
        }
    }

    /// Parse the inside of a style rule: declarations plus nested rules.
    fn style_block(&mut self, selectors: Vec<Selector>, start: usize, end: usize, out: &mut Vec<Rule>) {
        let mut decls: Vec<Declaration> = Vec::new();
        let mut nested: Vec<Rule> = Vec::new();
        let mut i = start;
        while i < end {
            while i < end && matches!(self.toks[i], Tok::Whitespace | Tok::Semicolon) {
                i += 1;
            }
            if i >= end {
                break;
            }
            if let Tok::AtKeyword(name) = &self.toks[i] {
                let name = name.to_ascii_lowercase();
                i = self.at_rule(&name, i + 1, end, Some(&selectors), &mut nested);
                continue;
            }
            let (pos, is_block) = self.scan_prelude(i, end);
            let is_custom = matches!(&self.toks[i], Tok::Ident(n) if n.starts_with("--"));
            if is_block && !is_custom {
                // nested style rule
                let close = self.block_end(pos, end);
                let sels = parse_selector_list(&self.toks[i..pos], Some(&selectors));
                if !sels.is_empty() {
                    self.style_block(sels, pos + 1, close, &mut nested);
                }
                i = close + 1;
            } else {
                let decl_end = if is_block { self.block_end(pos, end) + 1 } else { pos };
                properties::parse_declarations(self.toks, self.spans, self.src, i, decl_end.min(end), self.resolve, &mut decls);
                i = decl_end + 1;
            }
        }
        if !decls.is_empty() {
            decls.shrink_to_fit();
            out.push(Rule::Style(StyleRule { selectors: Rc::new(selectors), decls: Rc::new(decls), id: next_block_id() }));
        }
        out.append(&mut nested);
    }

    /// Handle an at-rule whose keyword has been consumed; returns the index after it.
    fn at_rule(&mut self, name: &str, start: usize, end: usize, parent: Option<&[Selector]>, out: &mut Vec<Rule>) -> usize {
        let (pos, is_block) = self.scan_prelude(start, end);
        let prelude = &self.toks[start..pos.min(end)];
        if !is_block {
            if name == "import" {
                let mut c = Cur::new(prelude);
                c.skip_ws();
                let url = match c.next() {
                    Some(Tok::Str(s)) | Some(Tok::Url(s)) => Some(s.clone()),
                    Some(Tok::Function(f)) if f.eq_ignore_ascii_case("url") => {
                        let mut inner = c.block();
                        inner.skip_ws();
                        match inner.next() {
                            Some(Tok::Str(s)) => Some(s.clone()),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if let Some(u) = url {
                    // skip layer()/supports() modifiers, keep the media list
                    let mut rest_start = c.i;
                    loop {
                        let mut cc = Cur::new(&prelude[rest_start..]);
                        cc.skip_ws();
                        match cc.peek() {
                            Some(Tok::Function(f)) if f.eq_ignore_ascii_case("layer") || f.eq_ignore_ascii_case("supports") => {
                                cc.next();
                                cc.block();
                                rest_start += cc.i;
                            }
                            Some(Tok::Ident(i)) if i.eq_ignore_ascii_case("layer") => {
                                cc.next();
                                rest_start += cc.i;
                            }
                            _ => break,
                        }
                    }
                    if let Some(abs) = (self.resolve)(&u) {
                        out.push(Rule::Import { url: abs, media: parse_media_query_list(&prelude[rest_start..]) });
                    }
                }
            }
            return pos + 1;
        }
        let close = self.block_end(pos, end);
        match name {
            "media" => {
                let query = parse_media_query_list(prelude);
                let mut rules = Vec::new();
                self.conditional_body(pos + 1, close, parent, &mut rules);
                if !rules.is_empty() {
                    out.push(Rule::Media { query, rules });
                }
            }
            "supports" => {
                if supports_condition(prelude) {
                    self.conditional_body(pos + 1, close, parent, out);
                }
            }
            // cascade layers are flattened (layer ordering is not modelled)
            "layer" => {
                self.conditional_body(pos + 1, close, parent, out);
            }
            "font-face" => {
                if let Some(ff) = self.font_face(pos + 1, close) {
                    out.push(Rule::FontFace(ff));
                }
            }
            "property" => {
                if let Some(rule) = self.property_rule(prelude, pos + 1, close) {
                    out.push(rule);
                }
            }
            "keyframes" | "-webkit-keyframes" | "-moz-keyframes" => {
                let name = prelude.iter().find_map(|t| match t {
                    Tok::Ident(n) | Tok::Str(n) => Some(Atom::new(n)),
                    _ => None,
                });
                if let Some(name) = name {
                    out.push(Rule::Keyframes { name, frames: Rc::new(self.keyframes(pos + 1, close)) });
                }
            }
            // @page, @container, @counter-style, ...: skipped
            _ => {}
        }
        close + 1
    }

    /// The blocks of a `@keyframes` rule (`from`, `to`, percentages, comma lists of them).
    fn keyframes(&mut self, start: usize, end: usize) -> Vec<KeyframeRule> {
        let mut out = Vec::new();
        let mut i = start;
        while i < end {
            let (pos, is_block) = self.scan_prelude(i, end);
            if !is_block {
                i = pos + 1;
                continue;
            }
            let close = self.block_end(pos, end);
            let mut offsets = Vec::new();
            let mut ok = true;
            for t in &self.toks[i..pos] {
                match t {
                    Tok::Whitespace | Tok::Comma => {}
                    Tok::Ident(w) if w.eq_ignore_ascii_case("from") => offsets.push(0.0),
                    Tok::Ident(w) if w.eq_ignore_ascii_case("to") => offsets.push(1.0),
                    Tok::Percentage(p) if (0.0..=100.0).contains(p) => offsets.push(*p / 100.0),
                    _ => ok = false,
                }
            }
            if ok && !offsets.is_empty() {
                let mut decls = Vec::new();
                properties::parse_declarations(self.toks, self.spans, self.src, pos + 1, close.min(end), self.resolve, &mut decls);
                out.push(KeyframeRule { offsets, decls });
            }
            i = close + 1;
            if out.len() > 2000 {
                break;
            }
        }
        out
    }

    /// `@property --name { syntax: …; inherits: …; initial-value: … }`. `inherits` is required,
    /// as the specification says; a rule without it is dropped.
    fn property_rule(&self, prelude: &[Tok], start: usize, end: usize) -> Option<Rule> {
        let name = prelude.iter().find_map(|t| match t {
            Tok::Ident(n) if n.starts_with("--") && n.len() > 2 => Some(Atom::new(n)),
            _ => None,
        })?;
        let mut inherits = None;
        let mut initial = None;
        let mut i = start;
        while i < end {
            let (pos, _) = self.scan_prelude(i, end);
            let decl_end = pos.min(end);
            let from = i;
            i = pos + 1;
            let Some(colon) = (from..decl_end).find(|&k| matches!(self.toks[k], Tok::Colon)) else { continue };
            let key: String = self.toks[from..colon]
                .iter()
                .filter_map(|t| if let Tok::Ident(n) = t { Some(n.to_ascii_lowercase()) } else { None })
                .collect();
            let (mut a, mut b) = (colon + 1, decl_end);
            while a < b && matches!(self.toks[a], Tok::Whitespace) {
                a += 1;
            }
            while b > a && matches!(self.toks[b - 1], Tok::Whitespace) {
                b -= 1;
            }
            match key.as_str() {
                "inherits" if a < b => {
                    inherits = match &self.toks[a] {
                        Tok::Ident(v) if v.eq_ignore_ascii_case("true") => Some(true),
                        Tok::Ident(v) if v.eq_ignore_ascii_case("false") => Some(false),
                        _ => None,
                    }
                }
                "initial-value" if a < b => {
                    let text = &self.src[self.spans[a].0 as usize..self.spans[b - 1].1 as usize];
                    initial = Some(Rc::from(text.trim()));
                }
                _ => {}
            }
        }
        Some(Rule::Property { name, inherits: inherits?, initial })
    }

    /// Body of a conditional group rule. Inside a style rule it may hold bare declarations.
    fn conditional_body(&mut self, start: usize, end: usize, parent: Option<&[Selector]>, out: &mut Vec<Rule>) {
        match parent {
            Some(p) => self.style_block(p.to_vec(), start, end, out),
            None => self.rules(start, end, None, out),
        }
    }

    fn font_face(&mut self, start: usize, end: usize) -> Option<FontFaceRule> {
        let mut family = None;
        let mut sources = Vec::new();
        let mut weight = (400u16, 400u16);
        let mut italic = false;
        let mut ranges = Vec::new();
        // split on top-level ';'
        let mut i = start;
        while i < end {
            let (pos, _) = self.scan_prelude(i, end);
            let from = i;
            let decl_end = pos.min(end);
            let decl = &self.toks[i..decl_end];
            i = pos + 1;
            let Some(colon) = decl.iter().position(|t| matches!(t, Tok::Colon)) else { continue };
            let name: String = decl[..colon]
                .iter()
                .filter_map(|t| if let Tok::Ident(n) = t { Some(n.to_ascii_lowercase()) } else { None })
                .collect();
            let value = &decl[colon + 1..];
            match name.as_str() {
                "font-family" => {
                    let s: Vec<String> = value
                        .iter()
                        .filter_map(|t| match t {
                            Tok::Str(s) | Tok::Ident(s) => Some(s.to_ascii_lowercase()),
                            _ => None,
                        })
                        .collect();
                    family = Some(s.join(" "));
                }
                "src" => {
                    for part in split_commas(value) {
                        let mut c = Cur::new(part);
                        let mut url = None;
                        let mut format = None;
                        loop {
                            c.skip_ws();
                            match c.next() {
                                None => break,
                                Some(Tok::Url(u)) => url = (self.resolve)(u),
                                Some(Tok::Function(f)) => {
                                    let mut inner = c.block();
                                    inner.skip_ws();
                                    let arg = match inner.next() {
                                        Some(Tok::Str(s)) | Some(Tok::Ident(s)) => Some(s.clone()),
                                        _ => None,
                                    };
                                    if f.eq_ignore_ascii_case("url") {
                                        url = arg.and_then(|u| (self.resolve)(&u));
                                    } else if f.eq_ignore_ascii_case("format") {
                                        format = arg.map(|s| s.to_ascii_lowercase());
                                    }
                                }
                                _ => {}
                            }
                        }
                        if let Some(u) = url {
                            sources.push((u, format));
                        }
                    }
                }
                "font-weight" => {
                    let nums: Vec<u16> = value
                        .iter()
                        .filter_map(|t| match t {
                            Tok::Number(n) => Some(*n as u16),
                            Tok::Ident(i) if i.eq_ignore_ascii_case("bold") => Some(700),
                            Tok::Ident(i) if i.eq_ignore_ascii_case("normal") => Some(400),
                            _ => None,
                        })
                        .collect();
                    if let Some(&a) = nums.first() {
                        weight = (a, *nums.get(1).unwrap_or(&a));
                    }
                }
                "font-style" => italic = value.iter().any(|t| t.is_ident("italic") || t.is_ident("oblique")),
                "unicode-range" => {
                    let (a, b) = (from + colon + 1, decl_end);
                    let text = match (self.spans.get(a), b.checked_sub(1).and_then(|k| self.spans.get(k))) {
                        (Some(s), Some(e)) if a < b => self.src.get(s.0 as usize..e.1 as usize),
                        _ => None,
                    };
                    ranges = text.map(parse_unicode_range).unwrap_or_default();
                }
                _ => {}
            }
        }
        Some(FontFaceRule { family: family?, sources, weight, italic, ranges })
    }
}

pub fn parse_stylesheet(src: &str, origin: Origin, resolve: UrlResolver) -> Stylesheet {
    let tokens = tokenize(src);
    let mut p = Parser { toks: &tokens.toks, spans: &tokens.spans, src, resolve };
    let mut rules = Vec::new();
    p.rules(0, tokens.toks.len(), None, &mut rules);
    rules.shrink_to_fit();
    Stylesheet { rules, origin }
}

impl Stylesheet {
    pub fn rule_count(&self) -> usize {
        fn count(rules: &[Rule]) -> usize {
            rules
                .iter()
                .map(|r| match r {
                    Rule::Media { rules, .. } => count(rules),
                    _ => 1,
                })
                .sum()
        }
        count(&self.rules)
    }

    /// True if any rule in the sheet depends on the viewport or colour scheme.
    pub fn has_media_rules(&self) -> bool {
        self.rules.iter().any(|r| matches!(r, Rule::Media { .. } | Rule::Import { .. }))
    }
}

/// True if `prop` is one of the properties resolved in the first cascade phase.
pub fn is_early_prop(prop: PropId) -> bool {
    matches!(prop, PropId::FontSize | PropId::Color | PropId::Font)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(s: &str) -> Stylesheet {
        parse_stylesheet(s, Origin::Author, &|u| Some(u.to_string()))
    }

    #[test]
    fn basic_rules() {
        let s = sheet("/* c */ a, .b > c { color: red; margin: 0 } @media (max-width: 768px) { h1 { font-size: 32px } } @import url(x.css) screen;");
        assert_eq!(s.rules.len(), 3);
        match &s.rules[0] {
            Rule::Style(r) => {
                assert_eq!(r.selectors.len(), 2);
                assert_eq!(r.decls.len(), 5);
            }
            _ => panic!(),
        }
        match &s.rules[1] {
            Rule::Media { query, rules } => {
                assert!(query.evaluate(&MediaEnv { width: 700.0, height: 500.0, dark: false }));
                assert!(!query.evaluate(&MediaEnv { width: 900.0, height: 500.0, dark: false }));
                assert_eq!(rules.len(), 1);
            }
            _ => panic!(),
        }
        assert!(matches!(&s.rules[2], Rule::Import { url, .. } if url == "x.css"));
    }

    #[test]
    fn nesting_and_supports() {
        let s = sheet(".card { color: red; &:hover { color: blue } .title { font-weight: 700 } @media (min-width: 100px) { padding: 4px } } @supports (display: grid) { .g { display: grid } } @supports (frobnicate: 1) { .x { color: red } }");
        // .card, .card:hover, .card .title, @media{.card}, .g
        assert_eq!(s.rules.len(), 5);
        assert!(matches!(&s.rules[3], Rule::Media { .. }));
    }

    #[test]
    fn range_queries() {
        let q = parse_media_query_list(&super::super::tokenizer::tokenize_value("(400px <= width <= 800px)"));
        assert!(q.evaluate(&MediaEnv { width: 600.0, height: 1.0, dark: false }));
        assert!(!q.evaluate(&MediaEnv { width: 900.0, height: 1.0, dark: false }));
        let q = parse_media_query_list(&super::super::tokenizer::tokenize_value("screen and (width >= 600px), print"));
        assert!(q.evaluate(&MediaEnv { width: 600.0, height: 1.0, dark: false }));
        assert!(!q.evaluate(&MediaEnv { width: 599.0, height: 1.0, dark: false }));
    }

    fn face_ranges(css: &str) -> Vec<(u32, u32)> {
        match &sheet(css).rules[0] {
            Rule::FontFace(f) => f.ranges.clone(),
            _ => panic!("not a font-face"),
        }
    }

    #[test]
    fn font_face_unicode_range() {
        let r = |v: &str| face_ranges(&format!("@font-face {{ font-family: X; src: url(a.woff2); unicode-range: {v}; }}"));
        assert_eq!(r("U+0041"), vec![(0x41, 0x41)]);
        assert_eq!(r("u+0000-00FF"), vec![(0, 0xFF)]);
        assert_eq!(r("U+4??"), vec![(0x400, 0x4FF)]);
        assert_eq!(r("U+0-7F, U+4??, u+20ac, U+1F600-1F64F"), vec![(0, 0x7F), (0x400, 0x4FF), (0x20AC, 0x20AC), (0x1F600, 0x1F64F)]);
        assert_eq!(r("U+41, garbage, U+zz, U+4?1, U+50-40, U+42"), vec![(0x41, 0x41), (0x42, 0x42)]);
        assert_eq!(r("U+10FFF0-1FFFFF"), vec![(0x10FFF0, 0x10FFFF)]);
        assert!(face_ranges("@font-face { font-family: X; src: url(a.woff2) }").is_empty());
        let many: Vec<String> = (0..600).map(|i| format!("U+{i:X}")).collect();
        assert_eq!(r(&many.join(",")).len(), 512);
    }
}
