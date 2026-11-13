//! Native primitives for everything around the tree: layout queries, the viewport, focus and
//! form values, timers, the address, the title, cookies and media queries.

use super::host::{HostState, NavRequest, Timer};
use super::natives::{arg, def, node, opt_node, wrap, Cx, NRes, Native};
use crate::atom::a;
use crate::css::parser::{parse_media_query_list, MediaEnv};
use crate::css::tokenizer::tokenize_value;
use crate::css::values::Color;
use crate::dom::{NodeId, DOCUMENT_NODE, NO_NODE};
use crate::layout::{BoxKind, F_POSITIONED, NO_BOX};
use invisium_js::{Runtime, Value};
use std::rc::Rc;

fn nums(rt: &mut Runtime, v: &[f32]) -> Value {
    let vals: Vec<Value> = v.iter().map(|&x| Value::Number(x as f64)).collect();
    rt.array(&vals)
}

/// A script is about to read geometry or computed style: make sure both reflect what the
/// script has done to the document so far.
fn ensure_layout(st: &mut HostState) {
    if st.page.force_layout(&mut st.fx) {
        st.box_index = None;
    }
}

/// Border box of a node in document coordinates.
fn doc_rect(st: &mut HostState, n: NodeId) -> Option<(f32, f32, f32, f32)> {
    ensure_layout(st);
    if !st.page.laid_out {
        return None;
    }
    let b = st.box_of(n);
    if b == NO_BOX {
        return None;
    }
    let bx = &st.page.tree.boxes[b as usize];
    // an inline box has no geometry of its own: it is where its line fragments are
    if matches!(bx.kind, BoxKind::Inline | BoxKind::Text | BoxKind::Break) {
        if let Some(r) = st.page.tree.inline_rect(b) {
            return Some(r);
        }
    }
    let (x, y) = st.page.tree.abs_pos(b);
    Some((x, y, bx.w, bx.h))
}

/// rect(node) -> [x, y, width, height] in viewport coordinates
fn n_rect(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let r = {
        let mut st = cx.st.borrow_mut();
        let scroll = st.page.scroll;
        doc_rect(&mut st, n).map(|(x, y, w, h)| [x - scroll.0, y - scroll.1, w, h]).unwrap_or([0.0; 4])
    };
    Ok(nums(rt, &r))
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Forgiving base64 decoding, as `atob` wants it: ASCII whitespace is skipped, padding is
/// optional, anything else that is not in the alphabet is an error.
pub(crate) fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut vals: Vec<u8> = Vec::with_capacity(text.len());
    for b in text.bytes() {
        match b {
            b' ' | 0x09 | 0x0A | 0x0C | 0x0D => {}
            b'A'..=b'Z' => vals.push(b - b'A'),
            b'a'..=b'z' => vals.push(b - b'a' + 26),
            b'0'..=b'9' => vals.push(b - b'0' + 52),
            b'+' => vals.push(62),
            b'/' => vals.push(63),
            b'=' => vals.push(64),
            _ => return None,
        }
    }
    // one or two `=` may end a text whose length is a multiple of four; nowhere else
    if vals.len() % 4 == 0 {
        for _ in 0..2 {
            if vals.last() == Some(&64) {
                vals.pop();
            }
        }
    }
    if vals.len() % 4 == 1 || vals.contains(&64) {
        return None;
    }
    let mut out = Vec::with_capacity(vals.len() / 4 * 3 + 2);
    for chunk in vals.chunks(4) {
        let mut acc = 0u32;
        for (i, &v) in chunk.iter().enumerate() {
            acc |= (v as u32) << (18 - 6 * i);
        }
        out.push((acc >> 16) as u8);
        if chunk.len() > 2 {
            out.push((acc >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(acc as u8);
        }
    }
    Some(out)
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let acc = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(BASE64[(acc >> 18) as usize & 63] as char);
        out.push(BASE64[(acc >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { BASE64[(acc >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { BASE64[acc as usize & 63] as char } else { '=' });
    }
    out
}

/// atob(text) -> the decoded bytes as a string of code units 0..255, or null if the text is
/// not base64. (In script this took a second for a few megabytes; pages embed whole models.)
fn n_atob(rt: &mut Runtime, _cx: &Cx, args: &[Value]) -> NRes {
    let text = rt.to_string_lossy(arg(args, 0));
    Ok(match base64_decode(&text) {
        Some(bytes) => {
            let s: String = bytes.iter().map(|&b| b as char).collect();
            rt.string(&s)
        }
        None => Value::Null,
    })
}

/// btoa(text) -> base64 of a string of code units 0..255, or null if a unit is larger.
fn n_btoa(rt: &mut Runtime, _cx: &Cx, args: &[Value]) -> NRes {
    let text = rt.to_string_lossy(arg(args, 0));
    let mut bytes = Vec::with_capacity(text.len());
    for c in text.chars() {
        match u8::try_from(c as u32) {
            Ok(b) => bytes.push(b),
            Err(_) => return Ok(Value::Null),
        }
    }
    Ok(rt.string(&base64_encode(&bytes)))
}

/// elementAt(x, y) -> the element drawn at a point of the viewport, or null
fn n_element_at(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let x = rt.to_number(arg(args, 0)) as f32;
    let y = rt.to_number(arg(args, 1)) as f32;
    let n = {
        let mut st = cx.st.borrow_mut();
        ensure_layout(&mut st);
        let (vw, vh) = st.page.viewport;
        if x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 && x < vw && y < vh {
            // inside the viewport there is always something: the root element at least
            match st.page.element_at(x, y) {
                NO_NODE | DOCUMENT_NODE => st.page.doc.document_element(),
                e => e,
            }
        } else {
            NO_NODE
        }
    };
    Ok(wrap(rt, cx, n))
}

/// The node whose box is the offset parent of `n` (nearest positioned ancestor, else `<body>`).
fn offset_parent(st: &mut HostState, n: NodeId) -> NodeId {
    let mut p = st.page.doc.parent(n);
    while p != NO_NODE && p != DOCUMENT_NODE {
        let b = st.box_of(p);
        if b != NO_BOX && st.page.tree.boxes[b as usize].flags & F_POSITIONED != 0 {
            return p;
        }
        if st.page.doc.tag(p) == a::BODY {
            return p;
        }
        p = st.page.doc.parent(p);
    }
    NO_NODE
}

fn n_offset_parent(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let p = {
        let mut st = cx.st.borrow_mut();
        if doc_rect(&mut st, n).is_some() {
            offset_parent(&mut st, n)
        } else {
            NO_NODE
        }
    };
    Ok(wrap(rt, cx, p))
}

/// metrics(node) -> [offsetLeft, offsetTop, offsetWidth, offsetHeight, clientLeft, clientTop,
///                   clientWidth, clientHeight, scrollWidth, scrollHeight]
fn n_metrics(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let m = {
        let mut st = cx.st.borrow_mut();
        let st = &mut *st;
        let is_root = n == st.page.doc.document_element();
        match doc_rect(st, n) {
            None => [0.0; 10],
            Some((x, y, w, h)) => {
                let op = offset_parent(st, n);
                let (ox, oy) = if op != NO_NODE { doc_rect(st, op).map_or((0.0, 0.0), |r| (r.0, r.1)) } else { (0.0, 0.0) };
                let b = st.box_of(n);
                let bw = st.page.styles.style(st.page.tree.boxes[b as usize].style).border_width;
                let (cw, ch) = if is_root { st.page.viewport } else { ((w - bw[1] - bw[3]).max(0.0), (h - bw[0] - bw[2]).max(0.0)) };
                let (sw, sh) = if is_root { (st.page.tree.width.max(cw), st.page.tree.height.max(ch)) } else { (cw, ch) };
                [x - ox, y - oy, w, h, bw[3], bw[0], cw, ch, sw, sh]
            }
        }
    };
    Ok(nums(rt, &m))
}

/// viewport() -> [width, height, scrollX, scrollY, documentWidth, documentHeight]
fn n_viewport(rt: &mut Runtime, cx: &Cx, _args: &[Value]) -> NRes {
    let v = {
        let mut st = cx.st.borrow_mut();
        ensure_layout(&mut st);
        [st.page.viewport.0, st.page.viewport.1, st.page.scroll.0, st.page.scroll.1, st.page.tree.width, st.page.tree.height]
    };
    Ok(nums(rt, &v))
}

/// viewNow() -> [width, height, scrollX, scrollY] as they are, without bringing layout up to
/// date first: the size of the viewport never depends on layout, and an input event carries
/// the scroll position of the moment it happened.
fn n_view_now(rt: &mut Runtime, cx: &Cx, _args: &[Value]) -> NRes {
    let v = {
        let st = cx.st.borrow();
        [st.page.viewport.0, st.page.viewport.1, st.page.scroll.0, st.page.scroll.1]
    };
    Ok(nums(rt, &v))
}

fn n_scroll_to(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let x = rt.to_number(arg(args, 0));
    let y = rt.to_number(arg(args, 1));
    let mut st = cx.st.borrow_mut();
    let clean = |v: f64, cur: f32| if v.is_finite() { v.max(0.0) as f32 } else { cur };
    let target = (clean(x, st.page.scroll.0), clean(y, st.page.scroll.1));
    let max = ((st.page.tree.width - st.page.viewport.0).max(0.0), (st.page.tree.height - st.page.viewport.1).max(0.0));
    st.page.scroll = (target.0.min(max.0), target.1.min(max.1));
    st.fx.scroll_to = Some(st.page.scroll);
    Ok(Value::Undefined)
}

fn kebab(debug_name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in debug_name.chars().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 {
            out.push('-');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

fn css_color(c: Color) -> String {
    if c.a == 255 {
        format!("rgb({}, {}, {})", c.r, c.g, c.b)
    } else {
        format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, (c.a as f32 / 255.0 * 1000.0).round() / 1000.0)
    }
}

fn px(v: f32) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    format!("{r}px")
}

/// computed(node, property): the resolved value of the properties scripts usually ask for.
fn n_computed(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let prop = rt.to_string_lossy(arg(args, 1)).to_ascii_lowercase();
    let out = {
        let mut st = cx.st.borrow_mut();
        let st = &mut *st;
        ensure_layout(st);
        let b = if st.page.laid_out { st.box_of(n) } else { NO_BOX };
        let bx = if b != NO_BOX { Some(st.page.tree.boxes[b as usize].clone()) } else { None };
        match st.page.styles.get(n) {
            None => String::new(),
            Some(s) => match prop.as_str() {
                "display" => kebab(&format!("{:?}", s.display)),
                "position" => kebab(&format!("{:?}", s.position)),
                "visibility" => kebab(&format!("{:?}", s.visibility)),
                "float" => kebab(&format!("{:?}", s.float)),
                "overflow" | "overflow-y" => kebab(&format!("{:?}", s.overflow_y)),
                "overflow-x" => kebab(&format!("{:?}", s.overflow_x)),
                "box-sizing" => kebab(&format!("{:?}", s.box_sizing)),
                "text-align" => match s.text_align {
                    crate::style::TextAlign::BlockCenter => "center".into(),
                    other => kebab(&format!("{other:?}")),
                },
                "white-space" => kebab(&format!("{:?}", s.white_space)),
                "flex-direction" => kebab(&format!("{:?}", s.flex_direction)),
                "pointer-events" => if s.pointer_events { "auto".into() } else { "none".into() },
                "opacity" => format!("{}", s.opacity),
                "z-index" => s.z_index.map_or("auto".into(), |z| z.to_string()),
                "color" => css_color(s.color),
                "background-color" => css_color(s.background_color),
                "font-size" => px(s.font_size),
                "font-weight" => s.font_weight.to_string(),
                "font-style" => if s.italic { "italic".into() } else { "normal".into() },
                "font-family" => s.font_family.iter().map(|f| f.as_str()).collect::<Vec<_>>().join(", "),
                // pages copy these into a canvas to redraw their own text
                "letter-spacing" => if s.letter_spacing == 0.0 { "normal".into() } else { px(s.letter_spacing) },
                "word-spacing" => px(s.word_spacing),
                "text-transform" => kebab(&format!("{:?}", s.text_transform)),
                "line-height" => match s.line_height {
                    crate::style::LineHeight::Normal => "normal".into(),
                    crate::style::LineHeight::Number(n) => px(n * s.font_size),
                    crate::style::LineHeight::Px(v) => px(v),
                },
                "cursor" => kebab(&format!("{:?}", s.cursor)),
                "width" => bx.as_ref().map_or("auto".into(), |b| px((b.w - b.pad[1] - b.pad[3] - s.border_width[1] - s.border_width[3]).max(0.0))),
                "height" => bx.as_ref().map_or("auto".into(), |b| px((b.h - b.pad[0] - b.pad[2] - s.border_width[0] - s.border_width[2]).max(0.0))),
                "margin-top" => px(bx.as_ref().map_or(0.0, |b| b.margin[0])),
                "margin-right" => px(bx.as_ref().map_or(0.0, |b| b.margin[1])),
                "margin-bottom" => px(bx.as_ref().map_or(0.0, |b| b.margin[2])),
                "margin-left" => px(bx.as_ref().map_or(0.0, |b| b.margin[3])),
                "padding-top" => px(bx.as_ref().map_or(0.0, |b| b.pad[0])),
                "padding-right" => px(bx.as_ref().map_or(0.0, |b| b.pad[1])),
                "padding-bottom" => px(bx.as_ref().map_or(0.0, |b| b.pad[2])),
                "padding-left" => px(bx.as_ref().map_or(0.0, |b| b.pad[3])),
                "border-top-width" => px(s.border_width[0]),
                "border-right-width" => px(s.border_width[1]),
                "border-bottom-width" => px(s.border_width[2]),
                "border-left-width" => px(s.border_width[3]),
                _ => {
                    // custom properties: `--name`
                    let mut found = String::new();
                    if prop.starts_with("--") {
                        let key = crate::atom::Atom::new(&prop);
                        let mut vars = s.vars.clone();
                        'outer: while let Some(v) = vars {
                            for (k, val) in &v.own {
                                if *k == key {
                                    found = val.to_string();
                                    break 'outer;
                                }
                            }
                            vars = v.parent.clone();
                        }
                    }
                    found
                }
            },
        }
    };
    Ok(rt.string(&out))
}

/// focus(node|null)
fn n_focus(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = opt_node(rt, cx, arg(args, 0))?;
    let mut st = cx.st.borrow_mut();
    if st.page.focus != n {
        st.page.focus = n;
        st.fx.focus = Some(n);
    }
    Ok(Value::Undefined)
}

fn n_active(rt: &mut Runtime, cx: &Cx, _args: &[Value]) -> NRes {
    let n = cx.st.borrow().page.focus;
    Ok(wrap(rt, cx, n))
}

fn current_value(st: &HostState, n: NodeId) -> String {
    if let Some(v) = st.page.form_values.get(&n) {
        return v.clone();
    }
    match st.page.doc.tag(n) {
        a::TEXTAREA => st.page.doc.text_content(n),
        _ => st.page.doc.attr(n, a::VALUE).unwrap_or("").to_string(),
    }
}

fn n_value(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let v = current_value(&cx.st.borrow(), n);
    Ok(rt.string(&v))
}

fn n_set_value(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let v = rt.to_string_lossy(arg(args, 1));
    let mut st = cx.st.borrow_mut();
    st.page.form_values.insert(n, v);
    st.fx.dirty_paint = true;
    Ok(Value::Undefined)
}

/// timer(id, delayMs, repeat)
fn n_timer(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let id = rt.to_number(arg(args, 0)) as u32;
    let delay = rt.to_number(arg(args, 1));
    let repeat = rt.to_boolean(arg(args, 2));
    let delay = if delay.is_finite() { delay.clamp(0.0, 2_147_483_647.0) } else { 0.0 };
    let mut st = cx.st.borrow_mut();
    if st.timers.len() >= 10_000 {
        return Ok(Value::Undefined); // timer bombs
    }
    // from the time script was entered (the page clock), so timers stay in step with the page
    let due = st.now_ms + delay;
    // repeating timers never fire faster than every 4 ms, as in other browsers
    st.timers.push(Timer { id, due, interval: if repeat { Some(delay.max(4.0)) } else { None } });
    Ok(Value::Undefined)
}

fn n_clear_timer(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let id = rt.to_number(arg(args, 0)) as u32;
    cx.st.borrow_mut().timers.retain(|t| t.id != id);
    Ok(Value::Undefined)
}

fn n_now(_rt: &mut Runtime, cx: &Cx, _args: &[Value]) -> NRes {
    // whole milliseconds: fine timers are a fingerprinting and side-channel surface
    Ok(Value::Number(cx.st.borrow().live_now().floor()))
}

fn n_url(rt: &mut Runtime, cx: &Cx, _args: &[Value]) -> NRes {
    let u = cx.st.borrow().page.url.clone();
    Ok(rt.string(&u))
}

/// resolve(reference[, base]) -> absolute URL or null
fn n_resolve(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let reference = rt.to_string_lossy(arg(args, 0));
    let base = match arg(args, 1) {
        Value::Undefined | Value::Null => None,
        v => Some(rt.to_string_lossy(v)),
    };
    let abs = {
        let st = cx.st.borrow();
        match base {
            Some(b) => (st.page.join)(&b, reference.trim()),
            None => st.resolve(&reference),
        }
    };
    Ok(match abs {
        Some(u) => rt.string(&u),
        None => Value::Null,
    })
}

/// navigate(url, replace)
fn n_navigate(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let reference = rt.to_string_lossy(arg(args, 0));
    let replace = rt.to_boolean(arg(args, 1));
    let mut st = cx.st.borrow_mut();
    if let Some(url) = st.resolve(&reference) {
        if !url.to_ascii_lowercase().starts_with("javascript:") {
            st.fx.navigate = Some(NavRequest { url, replace });
        }
    }
    Ok(Value::Undefined)
}

fn n_reload(_rt: &mut Runtime, cx: &Cx, _args: &[Value]) -> NRes {
    cx.st.borrow_mut().fx.reload = true;
    Ok(Value::Undefined)
}

/// setUrl(url): `history.pushState` / `replaceState`. Same-origin only; returns success.
fn n_set_url(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let reference = rt.to_string_lossy(arg(args, 0));
    let mut st = cx.st.borrow_mut();
    let origin = |u: &str| u.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
    match st.resolve(&reference) {
        Some(url) if origin(&url) == origin(&st.page.url) => {
            st.page.url = url.clone();
            st.fx.url_changed = Some(url);
            Ok(Value::Bool(true))
        }
        _ => Ok(Value::Bool(false)),
    }
}

fn title_node(st: &HostState) -> NodeId {
    st.page.doc.descendants(DOCUMENT_NODE).find(|&n| st.page.doc.tag(n) == a::TITLE && st.page.doc.closest(n, a::SVG) == NO_NODE).unwrap_or(NO_NODE)
}

fn n_title(rt: &mut Runtime, cx: &Cx, _args: &[Value]) -> NRes {
    let t = {
        let st = cx.st.borrow();
        let n = title_node(&st);
        if n == NO_NODE {
            String::new()
        } else {
            st.page.doc.text_content(n).split_whitespace().collect::<Vec<_>>().join(" ")
        }
    };
    Ok(rt.string(&t))
}

fn n_set_title(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let text = rt.to_string_lossy(arg(args, 0));
    let mut st = cx.st.borrow_mut();
    let st = &mut *st;
    let mut n = title_node(st);
    if n == NO_NODE {
        let head = st.page.doc.head();
        if head == NO_NODE {
            return Ok(Value::Undefined);
        }
        n = st.page.doc.create_element(a::TITLE, Vec::new());
        st.page.doc.append_child(head, n);
    }
    st.page.doc.remove_children(n);
    let t = st.page.doc.create_text(text);
    st.page.doc.append_child(n, t);
    st.fx.text_changed.push(n);
    st.sib = None;
    Ok(Value::Undefined)
}

/// cookie() -> "a=1; b=2". Script-visible cookies live in the page only (nothing on disk).
fn n_cookie(rt: &mut Runtime, cx: &Cx, _args: &[Value]) -> NRes {
    let hook = {
        let st = cx.st.borrow();
        st.cookie_access.as_ref().map(|(get, _)| (get.clone(), st.page.url.clone()))
    };
    if let Some((get, url)) = hook {
        let s = get(&url);
        return Ok(rt.string(&s));
    }
    let s = cx.st.borrow().cookies.iter().map(|(k, v)| if k.is_empty() { v.clone() } else { format!("{k}={v}") }).collect::<Vec<_>>().join("; ");
    Ok(rt.string(&s))
}

fn n_set_cookie(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let text = rt.to_string_lossy(arg(args, 0));
    let hook = {
        let st = cx.st.borrow();
        st.cookie_access.as_ref().map(|(_, set)| (set.clone(), st.page.url.clone()))
    };
    if let Some((set, url)) = hook {
        set(&url, &text);
        return Ok(Value::Undefined);
    }
    let mut parts = text.split(';');
    let pair = parts.next().unwrap_or("").trim();
    let (name, value) = match pair.split_once('=') {
        Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
        None => (String::new(), pair.to_string()),
    };
    if name.is_empty() && value.is_empty() {
        return Ok(Value::Undefined);
    }
    // an expiry in the past or max-age <= 0 deletes
    let mut delete = false;
    for attr in parts {
        let (k, v) = attr.split_once('=').map_or((attr.trim(), ""), |(k, v)| (k.trim(), v.trim()));
        if k.eq_ignore_ascii_case("max-age") && v.parse::<i64>().map_or(false, |n| n <= 0) {
            delete = true;
        }
        if k.eq_ignore_ascii_case("expires") && (v.contains("1970") || v.contains(" 19") || v.contains(" 200")) {
            delete = true;
        }
    }
    let mut st = cx.st.borrow_mut();
    st.cookies.retain(|(k, _)| *k != name);
    if !delete && st.cookies.len() < 200 && name.len() + value.len() <= 4096 {
        st.cookies.push((name, value));
    }
    Ok(Value::Undefined)
}

/// media(query) -> bool
fn n_media(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let q = rt.to_string_lossy(arg(args, 0));
    let st = cx.st.borrow();
    let list = parse_media_query_list(&tokenize_value(&q));
    Ok(Value::Bool(list.evaluate(&MediaEnv { width: st.page.viewport.0, height: st.page.viewport.1, dark: st.page.cfg.dark })))
}

fn n_document(rt: &mut Runtime, cx: &Cx, _args: &[Value]) -> NRes {
    Ok(wrap(rt, cx, DOCUMENT_NODE))
}

fn n_current_script(rt: &mut Runtime, cx: &Cx, _args: &[Value]) -> NRes {
    let n = cx.st.borrow().current_script;
    Ok(wrap(rt, cx, n))
}

/// special(which) -> documentElement (0), head (1), body (2)
fn n_special(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let which = rt.to_number(arg(args, 0)) as i32;
    let n = {
        let st = cx.st.borrow();
        match which {
            0 => st.page.doc.document_element(),
            1 => st.page.doc.head(),
            _ => st.page.doc.body(),
        }
    };
    Ok(wrap(rt, cx, n))
}

/// fetch(method, url, [name, value, ...], body|null, sameOriginOnly, credentials) -> request id.
/// credentials: 0 = omit, 1 = same-origin, 2 = include.
fn n_fetch(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let method = rt.to_string_lossy(arg(args, 0)).to_ascii_uppercase();
    let url = rt.to_string_lossy(arg(args, 1));
    let list = arg(args, 2);
    let len = match rt.get(list, "length") {
        Ok(v) => rt.to_number(v),
        Err(_) => 0.0,
    };
    let len = if len.is_finite() { (len.max(0.0) as u32).min(512) } else { 0 };
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i + 1 < len {
        let k = rt.get_index(list, i).unwrap_or(Value::Undefined);
        let k = rt.to_string_lossy(k);
        let v = rt.get_index(list, i + 1).unwrap_or(Value::Undefined);
        let v = rt.to_string_lossy(v);
        headers.push((k, v));
        i += 2;
    }
    // A string goes out as UTF-8, an ArrayBuffer as it is. With the seventh argument set, the
    // body is a list of parts (a multipart form): strings, buffers, and three consecutive
    // numbers `file id, start, end` for a range of a picked file, which is copied straight from
    // the page's copy of the file.
    let in_parts = rt.to_boolean(arg(args, 6));
    let body = match arg(args, 3) {
        Value::Null | Value::Undefined => None,
        v if in_parts => {
            let n = rt.get(v, "length").map(|l| rt.to_number(l)).unwrap_or(0.0).clamp(0.0, 100_000.0) as u32;
            let mut out: Vec<u8> = Vec::new();
            let mut i = 0;
            while i < n {
                let part = rt.get_index(v, i).unwrap_or(Value::Undefined);
                if rt.type_of(part) == "number" {
                    let mut num = |k: u32| {
                        let x = rt.get_index(v, k).unwrap_or(Value::Undefined);
                        rt.to_number(x)
                    };
                    let (id, start, end) = (num(i), num(i + 1), num(i + 2));
                    i += 3;
                    let st = cx.st.borrow();
                    if let Some(f) = st.page.picked.get(id as usize).filter(|_| id >= 0.0) {
                        let len = f.bytes.len();
                        let a = (start.max(0.0) as usize).min(len);
                        let b = (end.max(0.0) as usize).clamp(a, len);
                        out.extend_from_slice(&f.bytes[a..b]);
                    }
                    continue;
                }
                if let Some(bytes) = rt.buffer_bytes(part) {
                    out.extend_from_slice(&bytes);
                } else {
                    out.extend_from_slice(rt.to_string_lossy(part).as_bytes());
                }
                i += 1;
            }
            Some(out)
        }
        v => Some(match rt.buffer_bytes(v) {
            Some(bytes) => bytes,
            None => rt.to_string_lossy(v).into_bytes(),
        }),
    };
    let same_origin = rt.to_boolean(arg(args, 4));
    let credentials = (rt.to_number(arg(args, 5)) as i32).clamp(0, 2) as u8;
    let id = {
        let mut st = cx.st.borrow_mut();
        let abs = st.resolve(&url).filter(|u| {
            let low = u.to_ascii_lowercase();
            low.starts_with("http://") || low.starts_with("https://") || low.starts_with("data:")
        });
        match abs {
            Some(abs) if st.live_fetches < 256 => {
                st.next_fetch += 1;
                st.live_fetches += 1;
                let id = st.next_fetch;
                st.fx.fetches.push(crate::page::ScriptFetch { id, url: abs, method, headers, body, same_origin, credentials });
                Some(id)
            }
            _ => None,
        }
    };
    match id {
        Some(id) => Ok(Value::Number(id as f64)),
        None => Err(rt.throw_error("TypeError", "Failed to fetch")),
    }
}

/// Unpredictable bytes for `crypto.getRandomValues`. The standard library exposes operating
/// system randomness only through `RandomState` (SipHash keys seeded by the OS), so each block
/// is the keyed hash of a counter under a fresh key. To be replaced by a platform RNG behind
/// the engine's platform interface.
fn os_random(len: usize) -> Vec<u8> {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = Vec::with_capacity(len + 8);
    let mut counter = 0u64;
    while out.len() < len {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(counter);
        counter = counter.wrapping_add(1);
        out.extend_from_slice(&h.finish().to_le_bytes());
    }
    out.truncate(len);
    out
}

/// random(byteCount) -> ArrayBuffer
fn n_random(rt: &mut Runtime, _cx: &Cx, args: &[Value]) -> NRes {
    let len = rt.to_number(arg(args, 0));
    let len = if len.is_finite() { (len.max(0.0) as usize).min(65536) } else { 0 };
    Ok(rt.array_buffer(&os_random(len)))
}

/// encode(text): the UTF-8 bytes of a string, as a new ArrayBuffer.
fn n_encode(rt: &mut Runtime, _cx: &Cx, args: &[Value]) -> NRes {
    let text = rt.to_string_lossy(arg(args, 0));
    Ok(rt.array_buffer(text.as_bytes()))
}

/// decode(buffer, charset): the text of an ArrayBuffer (or of the window a view covers). UTF-8
/// unless the label names a Latin-1 family encoding; line endings are left alone.
fn n_decode(rt: &mut Runtime, _cx: &Cx, args: &[Value]) -> NRes {
    let Some(bytes) = rt.buffer_bytes(arg(args, 0)) else {
        return Err(rt.throw_error("TypeError", "argument is not a buffer"));
    };
    let label = match arg(args, 1) {
        Value::Null | Value::Undefined => String::new(),
        v => rt.to_string_lossy(v).trim().to_ascii_lowercase(),
    };
    let text = crate::page::decode_bytes(&bytes, &label, false);
    Ok(rt.string(&text))
}

/// pickFiles(input): ask for the file dialog of a file control. Only honoured while a
/// user-input event is being handled, so a page cannot open dialogs on its own.
fn n_pick_files(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let control = node(rt, cx, arg(args, 0))?;
    let mut st = cx.st.borrow_mut();
    if st.user_gesture {
        st.fx.pick_files = Some(control);
    }
    Ok(Value::Bool(st.user_gesture))
}

/// clearFiles(input): forget the selection of a file control.
fn n_clear_files(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let control = node(rt, cx, arg(args, 0))?;
    let mut st = cx.st.borrow_mut();
    if st.page.file_values.remove(&control).is_some() {
        st.page.form_values.remove(&control);
        st.fx.dirty_paint = true;
    }
    Ok(Value::Undefined)
}

/// fileBytes(id, start, end): a copy of a byte range of a picked file, as an ArrayBuffer.
fn n_file_bytes(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let id = rt.to_number(arg(args, 0));
    let start = rt.to_number(arg(args, 1));
    let end = rt.to_number(arg(args, 2));
    let bytes: Option<Vec<u8>> = {
        let st = cx.st.borrow();
        st.page.picked.get(id as usize).filter(|_| id >= 0.0).map(|f| {
            let len = f.bytes.len();
            let a = (start.max(0.0) as usize).min(len);
            let b = (end.max(0.0) as usize).clamp(a, len);
            f.bytes[a..b].to_vec()
        })
    };
    match bytes {
        Some(b) => Ok(rt.array_buffer(&b)),
        None => Err(rt.throw_error("TypeError", "no such file")),
    }
}

fn n_abort_fetch(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let id = rt.to_number(arg(args, 0)) as u32;
    cx.st.borrow_mut().fx.fetch_aborts.push(id);
    Ok(Value::Undefined)
}

/// submit(form, submitter|null): ask the page to submit a form (no `submit` event is fired).
fn n_submit(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let form = node(rt, cx, arg(args, 0))?;
    let submitter = opt_node(rt, cx, arg(args, 1))?;
    cx.st.borrow_mut().fx.submit = Some((form, submitter));
    Ok(Value::Undefined)
}

pub(crate) fn install(rt: &mut Runtime, n: Value, cx: &Rc<Cx>) {
    let table: &[(&str, u32, Native)] = &[
        ("submit", 2, n_submit),
        ("fetch", 7, n_fetch),
        ("abortFetch", 1, n_abort_fetch),
        ("pickFiles", 1, n_pick_files),
        ("clearFiles", 1, n_clear_files),
        ("fileBytes", 3, n_file_bytes),
        ("encode", 1, n_encode),
        ("decode", 2, n_decode),
        ("random", 1, n_random),
        ("rect", 1, n_rect),
        ("elementAt", 2, n_element_at),
        ("atob", 1, n_atob),
        ("btoa", 1, n_btoa),
        ("offsetParent", 1, n_offset_parent),
        ("metrics", 1, n_metrics),
        ("viewport", 0, n_viewport),
        ("viewNow", 0, n_view_now),
        ("scrollTo", 2, n_scroll_to),
        ("computed", 2, n_computed),
        ("focus", 1, n_focus),
        ("active", 0, n_active),
        ("value", 1, n_value),
        ("setValue", 2, n_set_value),
        ("timer", 3, n_timer),
        ("clearTimer", 1, n_clear_timer),
        ("now", 0, n_now),
        ("url", 0, n_url),
        ("resolve", 2, n_resolve),
        ("navigate", 2, n_navigate),
        ("reload", 0, n_reload),
        ("setUrl", 1, n_set_url),
        ("title", 0, n_title),
        ("setTitle", 1, n_set_title),
        ("cookie", 0, n_cookie),
        ("setCookie", 1, n_set_cookie),
        ("media", 1, n_media),
        ("document", 0, n_document),
        ("currentScript", 0, n_current_script),
        ("special", 1, n_special),
    ];
    for &(name, arity, f) in table {
        def(rt, n, cx, name, arity, f);
    }
}
