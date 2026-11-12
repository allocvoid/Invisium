//! Global functions: isNaN, isFinite, URI encoding/decoding.

use super::arg;
use crate::runtime::*;
use crate::value::*;

fn g_is_nan(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(rt.to_num(arg(a, 0))?.is_nan()))
}

fn g_is_finite(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(rt.to_num(arg(a, 0))?.is_finite()))
}

const URI_UNRESERVED: &str = "-_.!~*'()";
const URI_RESERVED: &str = ";/?:@&=+$,#";

fn uri_error(rt: &mut Runtime) -> Exc {
    Exc::Throw(rt.make_error("URIError", "URI malformed"))
}

fn encode(rt: &mut Runtime, v: Value, keep: &str) -> JsResult<Value> {
    let s = rt.to_units(v)?;
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c < 128 {
            let ch = c as u8 as char;
            if ch.is_ascii_alphanumeric() || URI_UNRESERVED.contains(ch) || keep.contains(ch) {
                out.push(ch);
                i += 1;
                continue;
            }
        }
        let cp = if (0xDC00..0xE000).contains(&c) {
            return Err(uri_error(rt));
        } else if (0xD800..0xDC00).contains(&c) {
            match s.get(i + 1) {
                Some(&lo) if (0xDC00..0xE000).contains(&lo) => {
                    i += 1;
                    0x10000 + ((c as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00)
                }
                _ => return Err(uri_error(rt)),
            }
        } else {
            c as u32
        };
        let ch = char::from_u32(cp).unwrap_or('\u{fffd}');
        let mut buf = [0u8; 4];
        for b in ch.encode_utf8(&mut buf).bytes() {
            out.push_str(&format!("%{:02X}", b));
        }
        i += 1;
    }
    Ok(rt.new_str(&out))
}

fn hexval(u: u16) -> Option<u8> {
    char::from_u32(u as u32).and_then(|c| c.to_digit(16)).map(|d| d as u8)
}

fn decode(rt: &mut Runtime, v: Value, reserved: &str) -> JsResult<Value> {
    let s = rt.to_units(v)?;
    let mut out: Vec<u16> = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] != b'%' as u16 {
            out.push(s[i]);
            i += 1;
            continue;
        }
        let byte_at = |j: usize| -> Option<u8> {
            if j + 2 < s.len() + 0 && s[j] == b'%' as u16 {
                Some(hexval(s[j + 1])? * 16 + hexval(s[j + 2])?)
            } else {
                None
            }
        };
        let Some(b0) = byte_at(i) else { return Err(uri_error(rt)) };
        if b0 < 0x80 {
            let ch = b0 as char;
            if reserved.contains(ch) {
                out.extend_from_slice(&s[i..i + 3]);
            } else {
                out.push(b0 as u16);
            }
            i += 3;
            continue;
        }
        let n = if b0 & 0xE0 == 0xC0 { 2 } else if b0 & 0xF0 == 0xE0 { 3 } else if b0 & 0xF8 == 0xF0 { 4 } else { return Err(uri_error(rt)) };
        let mut bytes = vec![b0];
        for k in 1..n {
            match byte_at(i + 3 * k) {
                Some(b) if b & 0xC0 == 0x80 => bytes.push(b),
                _ => return Err(uri_error(rt)),
            }
        }
        match std::str::from_utf8(&bytes) {
            Ok(st) => {
                for ch in st.chars() {
                    crate::lexer::push_utf16(&mut out, ch);
                }
            }
            Err(_) => return Err(uri_error(rt)),
        }
        i += 3 * n;
    }
    Ok(rt.new_str16(out))
}

fn encode_uri_component(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    encode(rt, arg(a, 0), "")
}
fn encode_uri(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    encode(rt, arg(a, 0), URI_RESERVED)
}
fn decode_uri_component(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    decode(rt, arg(a, 0), "")
}
fn decode_uri(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    decode(rt, arg(a, 0), URI_RESERVED)
}

/// The global `eval`. Reached through a call written `eval(...)` the interpreter runs the
/// code in the caller's scope (see `op_direct_eval`); any other call lands here and runs it
/// as a global script.
fn g_eval(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let Value::String(s) = arg(a, 0) else { return Ok(arg(a, 0)) };
    let src = rt.rust_string(s);
    rt.eval_internal(&src, "<eval>")
}

pub(super) fn setup(rt: &mut Runtime) {
    let g = rt.global;
    let ev = rt.method(g, "eval", 1, g_eval);
    rt.intr.eval_fn = Some(ev);
    rt.method(g, "isNaN", 1, g_is_nan);
    rt.method(g, "isFinite", 1, g_is_finite);
    rt.method(g, "encodeURIComponent", 1, encode_uri_component);
    rt.method(g, "encodeURI", 1, encode_uri);
    rt.method(g, "decodeURIComponent", 1, decode_uri_component);
    rt.method(g, "decodeURI", 1, decode_uri);
}
