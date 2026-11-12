//! JSON.stringify.

use super::arg;
use crate::runtime::*;
use crate::value::*;

struct Ctx {
    out: Vec<u16>,
    stack: Vec<ObjId>,
    indent: Vec<u16>,
    gap: Vec<u16>,
    replacer: Value,
    allow: Option<Vec<PropKey>>,
}

fn push_str(out: &mut Vec<u16>, s: &str) {
    out.extend(s.encode_utf16());
}

pub(crate) fn quote(out: &mut Vec<u16>, s: &[u16]) {
    out.push(b'"' as u16);
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        match c {
            0x22 => push_str(out, "\\\""),
            0x5C => push_str(out, "\\\\"),
            0x08 => push_str(out, "\\b"),
            0x0C => push_str(out, "\\f"),
            0x0A => push_str(out, "\\n"),
            0x0D => push_str(out, "\\r"),
            0x09 => push_str(out, "\\t"),
            c if c < 0x20 => push_str(out, &format!("\\u{:04x}", c)),
            c if (0xD800..0xDC00).contains(&c) => {
                if i + 1 < s.len() && (0xDC00..0xE000).contains(&s[i + 1]) {
                    out.push(c);
                    out.push(s[i + 1]);
                    i += 1;
                } else {
                    push_str(out, &format!("\\u{:04x}", c));
                }
            }
            c if (0xDC00..0xE000).contains(&c) => push_str(out, &format!("\\u{:04x}", c)),
            c => out.push(c),
        }
        i += 1;
    }
    out.push(b'"' as u16);
}

/// Serialises `holder[key]`; returns false when the value is not serialisable (undefined).
fn ser(rt: &mut Runtime, cx: &mut Ctx, holder: Value, key: PropKey, mut v: Value) -> JsResult<bool> {
    rt.check_native_stack()?;
    if let Value::Object(_) = v {
        let tj = rt.get_prop(v, PropKey::Str(A_TO_JSON))?;
        if rt.is_callable_val(tj) {
            let kv = rt.key_to_value(key);
            v = rt.call_value(tj, v, &[kv])?;
        }
    }
    if cx.replacer != Value::Undefined {
        let kv = rt.key_to_value(key);
        v = rt.call_value(cx.replacer, holder, &[kv, v])?;
    }
    if let Value::Object(o) = v {
        v = match rt.obj(o).kind {
            ObjKind::Number(_) => Value::Number(rt.to_num(v)?),
            ObjKind::String(_) => Value::String(rt.to_str(v)?),
            ObjKind::Boolean(b) => Value::Bool(b),
            ObjKind::BigInt(b) => Value::BigInt(b),
            _ => v,
        };
    }
    match v {
        Value::Null => push_str(&mut cx.out, "null"),
        Value::Bool(b) => push_str(&mut cx.out, if b { "true" } else { "false" }),
        Value::Number(n) => {
            if n.is_finite() {
                push_str(&mut cx.out, &crate::numfmt::number_to_string(n));
            } else {
                push_str(&mut cx.out, "null");
            }
        }
        Value::String(s) => {
            let u = rt.str_rc(s);
            quote(&mut cx.out, &u);
        }
        Value::Undefined | Value::Symbol(_) => return Ok(false),
        Value::BigInt(_) => return Err(rt.type_error("Do not know how to serialize a BigInt")),
        Value::Object(o) => {
            if rt.is_callable_obj(o) {
                return Ok(false);
            }
            if cx.stack.contains(&o) {
                return Err(rt.type_error("Converting circular structure to JSON"));
            }
            cx.stack.push(o);
            let saved_indent = cx.indent.clone();
            cx.indent.extend_from_slice(&cx.gap.clone());
            let is_array = rt.is_array_obj(o);
            let mut first = true;
            cx.out.push(if is_array { b'[' as u16 } else { b'{' as u16 });
            if is_array {
                let n = rt.length_of(v)?;
                for i in 0..n {
                    super::array::tick(rt, i)?;
                    sep(cx, &mut first);
                    let item = rt.get_idx(v, i)?;
                    let k = rt.index_key(i);
                    if !ser(rt, cx, v, k, item)? {
                        push_str(&mut cx.out, "null");
                    }
                }
            } else {
                let keys = match &cx.allow {
                    Some(list) => list.clone(),
                    None => rt.own_enum_keys_ex(o)?,
                };
                for k in keys {
                    let item = rt.get_prop(v, k)?;
                    let mark = cx.out.len();
                    let was_first = first;
                    sep(cx, &mut first);
                    let PropKey::Str(a) = k else { continue };
                    let ku = rt.atom_units(a).to_vec();
                    quote(&mut cx.out, &ku);
                    cx.out.push(b':' as u16);
                    if !cx.gap.is_empty() {
                        cx.out.push(b' ' as u16);
                    }
                    if !ser(rt, cx, v, k, item)? {
                        cx.out.truncate(mark);
                        first = was_first;
                    }
                }
            }
            cx.indent = saved_indent;
            if !first && !cx.gap.is_empty() {
                cx.out.push(b'\n' as u16);
                cx.out.extend_from_slice(&cx.indent.clone());
            }
            cx.out.push(if is_array { b']' as u16 } else { b'}' as u16 });
            cx.stack.pop();
            if cx.out.len() > (1 << 28) {
                return Err(rt.range_error("Invalid string length"));
            }
        }
    }
    Ok(true)
}

fn sep(cx: &mut Ctx, first: &mut bool) {
    if !*first {
        cx.out.push(b',' as u16);
    }
    *first = false;
    if !cx.gap.is_empty() {
        cx.out.push(b'\n' as u16);
        let ind = cx.indent.clone();
        cx.out.extend_from_slice(&ind);
    }
}

pub(super) fn stringify(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let r = arg(a, 1);
    let mut replacer = Value::Undefined;
    let mut allow = None;
    if rt.is_callable_val(r) {
        replacer = r;
    } else if let Value::Object(ro) = r {
        if matches!(rt.obj(ro).kind, ObjKind::Array(_)) {
            let mut list: Vec<PropKey> = Vec::new();
            let n = rt.length_of(r)?;
            for i in 0..n {
                let item = rt.get_idx(r, i)?;
                let ok = match item {
                    Value::String(_) | Value::Number(_) => true,
                    Value::Object(io) => matches!(rt.obj(io).kind, ObjKind::String(_) | ObjKind::Number(_)),
                    _ => false,
                };
                if ok {
                    let s = rt.to_str(item)?;
                    let k = rt.to_property_key(Value::String(s))?;
                    if !list.contains(&k) {
                        list.push(k);
                    }
                }
            }
            allow = Some(list);
        }
    }
    let mut space = arg(a, 2);
    if let Value::Object(so) = space {
        space = match rt.obj(so).kind {
            ObjKind::Number(_) => Value::Number(rt.to_num(space)?),
            ObjKind::String(_) => Value::String(rt.to_str(space)?),
            _ => space,
        };
    }
    let gap: Vec<u16> = match space {
        Value::Number(n) => {
            let n = crate::ops::to_integer(n).clamp(0.0, 10.0) as usize;
            vec![b' ' as u16; n]
        }
        Value::String(s) => rt.str_units(s).iter().take(10).copied().collect(),
        _ => Vec::new(),
    };
    let mut cx = Ctx { out: Vec::new(), stack: Vec::new(), indent: Vec::new(), gap, replacer, allow };
    let wrapper = rt.plain_object();
    let v = arg(a, 0);
    rt.define_value(wrapper, PropKey::Str(A_EMPTY), v, DEFAULT_FLAGS);
    if ser(rt, &mut cx, Value::Object(wrapper), PropKey::Str(A_EMPTY), v)? {
        Ok(rt.new_str16(cx.out))
    } else {
        Ok(Value::Undefined)
    }
}
