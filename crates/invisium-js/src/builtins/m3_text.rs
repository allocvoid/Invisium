//! Text built-ins real pages still use: String.fromCodePoint, the Annex B HTML methods,
//! `escape` / `unescape` and String.prototype.normalize.

use super::arg;
use super::normalize::{normalize, Form};
use crate::runtime::*;
use crate::value::*;
use std::rc::Rc;

fn this_units(rt: &mut Runtime, this: Value, what: &str) -> JsResult<Rc<[u16]>> {
    if this.is_nullish() {
        return Err(rt.type_error(&format!("String.prototype.{} called on null or undefined", what)));
    }
    rt.to_units(this)
}

fn from_code_point(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let mut out: Vec<u16> = Vec::with_capacity(a.len());
    for v in a {
        let n = rt.to_num(*v)?;
        if !(0.0..=1_114_111.0).contains(&n) || n.fract() != 0.0 {
            let shown = rt.to_rust_string(*v).unwrap_or_default();
            return Err(rt.range_error(&format!("Invalid code point {}", shown)));
        }
        let cp = n as u32;
        if cp >= 0x10000 {
            let c = cp - 0x10000;
            out.push(0xD800 + (c >> 10) as u16);
            out.push(0xDC00 + (c & 0x3FF) as u16);
        } else {
            out.push(cp as u16);
        }
    }
    Ok(rt.new_str16(out))
}

/// Annex B CreateHTML: `<tag attr="value">string</tag>`.
fn html(rt: &mut Runtime, this: Value, a: &[Value], name: &str, tag: &str, attr: Option<&str>) -> JsResult<Value> {
    let s = this_units(rt, this, name)?;
    let mut out: Vec<u16> = format!("<{}", tag).encode_utf16().collect();
    if let Some(at) = attr {
        let v = rt.to_units(arg(a, 0))?;
        out.extend(format!(" {}=\"", at).encode_utf16());
        for &u in v.iter() {
            if u == b'"' as u16 {
                out.extend("&quot;".encode_utf16());
            } else {
                out.push(u);
            }
        }
        out.push(b'"' as u16);
    }
    out.push(b'>' as u16);
    out.extend_from_slice(&s);
    out.extend(format!("</{}>", tag).encode_utf16());
    Ok(rt.new_str16(out))
}

macro_rules! html_fn {
    ($name:expr, $tag:expr, $attr:expr) => {
        ($name, if $attr.is_some() { 1 } else { 0 }, (|rt, this, a| html(rt, this, a, $name, $tag, $attr)) as super::NFn)
    };
}

const NO_ATTR: Option<&str> = None;

fn is_escape_safe(u: u16) -> bool {
    u < 128 && ((u as u8).is_ascii_alphanumeric() || b"@*_+-./".contains(&(u as u8)))
}

fn escape(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let s = rt.to_units(arg(a, 0))?;
    let mut out = String::with_capacity(s.len());
    for &u in s.iter() {
        if is_escape_safe(u) {
            out.push(u as u8 as char);
        } else if u < 256 {
            out.push_str(&format!("%{:02X}", u));
        } else {
            out.push_str(&format!("%u{:04X}", u));
        }
    }
    Ok(rt.new_str(&out))
}

fn hex_units(s: &[u16]) -> Option<u16> {
    let mut v: u32 = 0;
    for &u in s {
        v = v * 16 + char::from_u32(u as u32)?.to_digit(16)?;
    }
    Some(v as u16)
}

fn unescape(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let s = rt.to_units(arg(a, 0))?;
    let mut out: Vec<u16> = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' as u16 {
            if s.get(i + 1) == Some(&(b'u' as u16)) {
                if let Some(v) = s.get(i + 2..i + 6).and_then(hex_units) {
                    out.push(v);
                    i += 6;
                    continue;
                }
            } else if let Some(v) = s.get(i + 1..i + 3).and_then(hex_units) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    Ok(rt.new_str16(out))
}

fn normalize_method(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this, "normalize")?;
    let form = match arg(a, 0) {
        Value::Undefined => Form::Nfc,
        v => match rt.to_rust_string(v)?.as_str() {
            "NFC" => Form::Nfc,
            "NFD" => Form::Nfd,
            "NFKC" => Form::Nfkc,
            "NFKD" => Form::Nfkd,
            other => {
                let shown: String = other.chars().take(40).collect();
                return Err(rt.range_error(&format!("The normalization form should be one of NFC, NFD, NFKC, NFKD. (got {})", shown)));
            }
        },
    };
    if s.iter().all(|u| *u < 0x80) {
        return Ok(if matches!(this, Value::String(_)) { this } else { rt.new_str16(s.to_vec()) });
    }
    // decomposition grows a string by a small constant factor at most
    rt.check_alloc(s.len() * 8).map_err(|t| rt.thrown_to_exc(t))?;
    rt.steps = rt.steps.saturating_add(s.len() as u64 / 8);
    rt.safepoint_counted()?;
    let out = normalize(&s, form);
    Ok(rt.new_str16(out))
}

pub(super) fn setup(rt: &mut Runtime) {
    let g = rt.global;
    rt.method(g, "escape", 1, escape);
    rt.method(g, "unescape", 1, unescape);
    let sk = rt.key_str("String");
    if let Some(string) = rt.get_own_prop(g, sk).and_then(|p| p.value.as_obj()) {
        rt.method(string, "fromCodePoint", 1, from_code_point);
    }
    let Some(sp) = rt.intr.string_proto else { return };
    rt.method(sp, "normalize", 0, normalize_method);
    let methods: [(&str, u32, super::NFn); 13] = [
        html_fn!("anchor", "a", Some("name")),
        html_fn!("big", "big", NO_ATTR),
        html_fn!("blink", "blink", NO_ATTR),
        html_fn!("bold", "b", NO_ATTR),
        html_fn!("fixed", "tt", NO_ATTR),
        html_fn!("fontcolor", "font", Some("color")),
        html_fn!("fontsize", "font", Some("size")),
        html_fn!("italics", "i", NO_ATTR),
        html_fn!("link", "a", Some("href")),
        html_fn!("small", "small", NO_ATTR),
        html_fn!("strike", "strike", NO_ATTR),
        html_fn!("sub", "sub", NO_ATTR),
        html_fn!("sup", "sup", NO_ATTR),
    ];
    for (n, ar, f) in methods {
        rt.method(sp, n, ar, f);
    }
}
