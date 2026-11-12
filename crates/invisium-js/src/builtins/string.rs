//! String constructor and String.prototype.

use super::arg;
use crate::numfmt::is_js_whitespace;
use crate::runtime::*;
use crate::value::*;
use std::rc::Rc;

fn this_units(rt: &mut Runtime, this: Value) -> JsResult<Rc<[u16]>> {
    if this.is_nullish() {
        return Err(rt.type_error("String.prototype method called on null or undefined"));
    }
    rt.to_units(this)
}

fn string_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let s = match a.first() {
        None => StrId::from_atom(A_EMPTY),
        Some(Value::Symbol(s)) if rt.native_new_target == Value::Undefined => {
            let d = rt.syms.get(s.0 as usize).cloned().flatten().map(|d| String::from_utf16_lossy(&d)).unwrap_or_default();
            match rt.new_str(&format!("Symbol({})", d)) {
                Value::String(s) => s,
                _ => StrId::from_atom(A_EMPTY),
            }
        }
        Some(v) => rt.to_str(*v)?,
    };
    let nt = rt.native_new_target;
    if nt == Value::Undefined {
        return Ok(Value::String(s));
    }
    let p = rt.intr.string_proto;
    let proto = rt.proto_from_ctor(nt, p)?;
    Ok(Value::Object(rt.alloc(Object::new(proto, ObjKind::String(s)))))
}

fn from_char_code(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let mut out = Vec::with_capacity(a.len());
    for v in a {
        out.push(crate::ops::to_uint32(rt.to_num(*v)?) as u16);
    }
    Ok(rt.new_str16(out))
}

fn this_string_value(rt: &mut Runtime, this: Value) -> JsResult<Value> {
    match this {
        Value::String(_) => Ok(this),
        Value::Object(o) => match rt.obj(o).kind {
            ObjKind::String(s) => Ok(Value::String(s)),
            _ => Err(rt.type_error("String.prototype.valueOf requires that 'this' be a String")),
        },
        _ => Err(rt.type_error("String.prototype.valueOf requires that 'this' be a String")),
    }
}

fn to_string(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    this_string_value(rt, this)
}

fn pos_arg(rt: &mut Runtime, v: Value) -> JsResult<f64> {
    Ok(crate::ops::to_integer(rt.to_num(v)?))
}

fn char_at(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let p = pos_arg(rt, arg(a, 0))?;
    if p < 0.0 || p >= s.len() as f64 {
        return Ok(rt.atom_value(A_EMPTY));
    }
    Ok(rt.new_str16(vec![s[p as usize]]))
}

fn char_code_at(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let p = pos_arg(rt, arg(a, 0))?;
    if p < 0.0 || p >= s.len() as f64 {
        return Ok(Value::Number(f64::NAN));
    }
    Ok(Value::Number(s[p as usize] as f64))
}

fn code_point_at(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let p = pos_arg(rt, arg(a, 0))?;
    if p < 0.0 || p >= s.len() as f64 {
        return Ok(Value::Undefined);
    }
    let i = p as usize;
    let hi = s[i];
    if (0xD800..0xDC00).contains(&hi) && i + 1 < s.len() && (0xDC00..0xE000).contains(&s[i + 1]) {
        let cp = 0x10000 + ((hi as u32 - 0xD800) << 10) + (s[i + 1] as u32 - 0xDC00);
        return Ok(Value::Number(cp as f64));
    }
    Ok(Value::Number(hi as f64))
}

fn at(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let p = pos_arg(rt, arg(a, 0))?;
    let i = if p < 0.0 { s.len() as f64 + p } else { p };
    if i < 0.0 || i >= s.len() as f64 {
        return Ok(Value::Undefined);
    }
    Ok(rt.new_str16(vec![s[i as usize]]))
}

pub(crate) fn find_units(h: &[u16], n: &[u16], from: usize) -> Option<usize> {
    if n.is_empty() {
        return if from <= h.len() { Some(from) } else { None };
    }
    if n.len() > h.len() {
        return None;
    }
    (from..=h.len() - n.len()).find(|&i| &h[i..i + n.len()] == n)
}

fn index_of(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let n = rt.to_units(arg(a, 0))?;
    let from = pos_arg(rt, arg(a, 1))?.max(0.0).min(s.len() as f64) as usize;
    Ok(Value::Number(find_units(&s, &n, from).map(|i| i as f64).unwrap_or(-1.0)))
}

fn last_index_of(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let n = rt.to_units(arg(a, 0))?;
    let from = match rt.to_num(arg(a, 1))? {
        x if x.is_nan() => f64::INFINITY,
        x => x.trunc(),
    };
    if n.len() > s.len() {
        return Ok(Value::Number(-1.0));
    }
    let start = from.max(0.0).min((s.len() - n.len()) as f64) as usize;
    let r = (0..=start).rev().find(|&i| &s[i..i + n.len()] == &n[..]);
    Ok(Value::Number(r.map(|i| i as f64).unwrap_or(-1.0)))
}

fn includes(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    if rt.is_regexp(arg(a, 0))? {
        return Err(rt.type_error("First argument must not be a regular expression"));
    }
    let n = rt.to_units(arg(a, 0))?;
    let from = pos_arg(rt, arg(a, 1))?.max(0.0).min(s.len() as f64) as usize;
    Ok(Value::Bool(find_units(&s, &n, from).is_some()))
}

fn starts_with(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let n = rt.to_units(arg(a, 0))?;
    let p = pos_arg(rt, arg(a, 1))?.max(0.0).min(s.len() as f64) as usize;
    Ok(Value::Bool(s[p..].starts_with(&n)))
}

fn ends_with(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let n = rt.to_units(arg(a, 0))?;
    let end = match arg(a, 1) {
        Value::Undefined => s.len(),
        v => pos_arg(rt, v)?.max(0.0).min(s.len() as f64) as usize,
    };
    Ok(Value::Bool(s[..end].ends_with(&n)))
}

fn slice(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let len = s.len() as f64;
    let from = rt.rel_index(arg(a, 0), len, 0.0)? as usize;
    let to = rt.rel_index(arg(a, 1), len, len)? as usize;
    Ok(if from < to { rt.new_str16(s[from..to].to_vec()) } else { rt.atom_value(A_EMPTY) })
}

fn substring(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let len = s.len() as f64;
    let st = pos_arg(rt, arg(a, 0))?.max(0.0).min(len) as usize;
    let en = match arg(a, 1) {
        Value::Undefined => s.len(),
        v => pos_arg(rt, v)?.max(0.0).min(len) as usize,
    };
    let (x, y) = if st < en { (st, en) } else { (en, st) };
    Ok(rt.new_str16(s[x..y].to_vec()))
}

fn substr(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let len = s.len() as f64;
    let st = rt.rel_index(arg(a, 0), len, 0.0)?;
    let cnt = match arg(a, 1) {
        Value::Undefined => len,
        v => pos_arg(rt, v)?,
    };
    let end = (st + cnt.max(0.0)).min(len);
    Ok(if st < end { rt.new_str16(s[st as usize..end as usize].to_vec()) } else { rt.atom_value(A_EMPTY) })
}

fn is_ws16(u: u16) -> bool {
    char::from_u32(u as u32).map(is_js_whitespace).unwrap_or(false)
}

fn trim_impl(rt: &mut Runtime, this: Value, start: bool, end: bool) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let mut a = 0;
    let mut b = s.len();
    if start {
        while a < b && is_ws16(s[a]) {
            a += 1;
        }
    }
    if end {
        while b > a && is_ws16(s[b - 1]) {
            b -= 1;
        }
    }
    Ok(rt.new_str16(s[a..b].to_vec()))
}
fn trim(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { trim_impl(rt, t, true, true) }
fn trim_start(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { trim_impl(rt, t, true, false) }
fn trim_end(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { trim_impl(rt, t, false, true) }

fn pad_impl(rt: &mut Runtime, this: Value, a: &[Value], at_start: bool) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let max = rt.to_num(arg(a, 0))?;
    let fill: Rc<[u16]> = match arg(a, 1) {
        Value::Undefined => Rc::from(vec![b' ' as u16]),
        v => rt.to_units(v)?,
    };
    if !(max > s.len() as f64) || fill.is_empty() {
        return Ok(rt.new_str16(s.to_vec()));
    }
    if max > (1u64 << 28) as f64 {
        return Err(rt.range_error("Invalid string length"));
    }
    rt.check_alloc(max as usize * 2).map_err(|t| rt.thrown_to_exc(t))?;
    let need = max as usize - s.len();
    let pad: Vec<u16> = fill.iter().copied().cycle().take(need).collect();
    let out = if at_start { [pad, s.to_vec()].concat() } else { [s.to_vec(), pad].concat() };
    Ok(rt.new_str16(out))
}
fn pad_start(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { pad_impl(rt, t, a, true) }
fn pad_end(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { pad_impl(rt, t, a, false) }

fn repeat(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let n = rt.to_num(arg(a, 0))?;
    if n < 0.0 || n.is_infinite() {
        return Err(rt.range_error("Invalid count value"));
    }
    let n = crate::ops::to_integer(n) as usize;
    let total = s.len().saturating_mul(n);
    if total > (1 << 28) {
        return Err(rt.range_error("Invalid string length"));
    }
    rt.check_alloc(total * 2).map_err(|t| rt.thrown_to_exc(t))?;
    Ok(rt.new_str16(s.repeat(n)))
}

fn case_impl(rt: &mut Runtime, this: Value, upper: bool) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let st = String::from_utf16_lossy(&s);
    let r = if upper { st.to_uppercase() } else { st.to_lowercase() };
    Ok(rt.new_str(&r))
}
fn to_upper(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { case_impl(rt, t, true) }
fn to_lower(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { case_impl(rt, t, false) }

fn concat(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let mut out = this_units(rt, this)?.to_vec();
    for v in a {
        let u = rt.to_units(*v)?;
        out.extend_from_slice(&u);
        if out.len() > (1 << 28) {
            return Err(rt.range_error("Invalid string length"));
        }
    }
    Ok(rt.new_str16(out))
}

fn locale_compare(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let t = rt.to_units(arg(a, 0))?;
    super::intl::locale_compare(rt, &s, &t, arg(a, 1), arg(a, 2))
}

fn iterator(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let v = rt.new_str16(s.to_vec());
    rt.get_iterator(v)
}

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto;
    let sp = rt.alloc(Object::new(op, ObjKind::String(StrId::from_atom(A_EMPTY))));
    rt.intr.string_proto = Some(sp);
    let c = rt.constructor("String", 1, string_ctor, sp);
    rt.method(c, "fromCharCode", 1, from_char_code);
    let methods: [(&str, u32, super::NFn); 31] = [
        ("toString", 0, to_string),
        ("valueOf", 0, to_string),
        ("charAt", 1, char_at),
        ("charCodeAt", 1, char_code_at),
        ("codePointAt", 1, code_point_at),
        ("at", 1, at),
        ("indexOf", 1, index_of),
        ("lastIndexOf", 1, last_index_of),
        ("includes", 1, includes),
        ("startsWith", 1, starts_with),
        ("endsWith", 1, ends_with),
        ("slice", 2, slice),
        ("substring", 2, substring),
        ("substr", 2, substr),
        ("trim", 0, trim),
        ("trimStart", 0, trim_start),
        ("trimEnd", 0, trim_end),
        ("trimLeft", 0, trim_start),
        ("trimRight", 0, trim_end),
        ("padStart", 1, pad_start),
        ("padEnd", 1, pad_end),
        ("repeat", 1, repeat),
        ("toUpperCase", 0, to_upper),
        ("toLowerCase", 0, to_lower),
        ("toLocaleUpperCase", 0, to_upper),
        ("toLocaleLowerCase", 0, to_lower),
        ("concat", 1, concat),
        ("localeCompare", 1, locale_compare),
        ("split", 2, super::string_split::split),
        ("replace", 2, super::string_split::replace),
        ("replaceAll", 2, super::string_split::replace_all),
    ];
    for (n, ar, f) in methods {
        rt.method(sp, n, ar, f);
    }
    rt.method_sym(sp, SYM_ITERATOR, "[Symbol.iterator]", 0, iterator);
}
