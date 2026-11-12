//! Smaller Milestone 2a additions: Object.fromEntries, the copying / from-the-end Array
//! methods and String.raw.

use super::arg;
use super::array::{this_len, tick};
use crate::runtime::*;
use crate::value::*;

fn from_entries(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let out = rt.plain_object();
    let it = rt.get_iterator(arg(a, 0))?;
    rt.scratch.push(it);
    let mark = rt.scratch.len();
    while let Some(entry) = rt.iter_step(it)? {
        rt.scratch.push(entry);
        if !matches!(entry, Value::Object(_)) {
            rt.iter_close(it)?;
            return Err(rt.type_error("Iterator value is not an entry object"));
        }
        let k = rt.get_idx(entry, 0)?;
        rt.scratch.push(k);
        let v = rt.get_idx(entry, 1)?;
        rt.scratch.push(v);
        let key = rt.to_property_key(k)?;
        rt.define_value(out, key, v, DEFAULT_FLAGS);
        rt.scratch.truncate(mark);
    }
    Ok(Value::Object(out))
}

/// Resolves a relative index against `len`; None when out of range.
fn relative(rt: &mut Runtime, v: Value, len: u64) -> JsResult<Option<u64>> {
    let n = crate::ops::to_integer(rt.to_num(v)?);
    let k = if n < 0.0 { len as f64 + n } else { n };
    Ok(if k >= 0.0 && k < len as f64 { Some(k as u64) } else { None })
}

fn at(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, len) = this_len(rt, this)?;
    match relative(rt, arg(a, 0), len)? {
        Some(i) => rt.get_idx(o, i),
        None => Ok(Value::Undefined),
    }
}

fn find_last_impl(rt: &mut Runtime, this: Value, a: &[Value], want_index: bool) -> JsResult<Value> {
    let (o, len) = this_len(rt, this)?;
    let cb = rt.callable_arg(arg(a, 0), "predicate")?;
    for i in (0..len).rev() {
        tick(rt, i)?;
        let v = rt.get_idx(o, i)?;
        let r = rt.call_value(cb, arg(a, 1), &[v, Value::Number(i as f64), o])?;
        if rt.truthy(r) {
            return Ok(if want_index { Value::Number(i as f64) } else { v });
        }
    }
    Ok(if want_index { Value::Number(-1.0) } else { Value::Undefined })
}

fn find_last(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    find_last_impl(rt, this, a, false)
}
fn find_last_index(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    find_last_impl(rt, this, a, true)
}

/// The elements of an array-like as a dense list (holes read as undefined).
fn snapshot(rt: &mut Runtime, this: Value) -> JsResult<Vec<Value>> {
    let (o, len) = this_len(rt, this)?;
    if len > u32::MAX as u64 {
        return Err(rt.range_error("Invalid array length"));
    }
    rt.check_alloc(len as usize * 16).map_err(|t| rt.thrown_to_exc(t))?;
    let mut items = Vec::with_capacity(len as usize);
    for i in 0..len {
        tick(rt, i)?;
        let v = rt.get_idx(o, i)?;
        rt.scratch.push(v);
        items.push(v);
    }
    Ok(items)
}

fn to_reversed(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let mut items = snapshot(rt, this)?;
    items.reverse();
    Ok(rt.new_array_values(&items))
}

fn to_sorted(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let cmp = arg(a, 0);
    if cmp != Value::Undefined && !rt.is_callable_val(cmp) {
        return Err(rt.type_error("The comparison function must be either a function or undefined"));
    }
    let items = snapshot(rt, this)?;
    let copy = rt.new_array_values(&items);
    super::array_fn::sort(rt, copy, a)
}

fn with(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (_, len) = this_len(rt, this)?;
    let Some(i) = relative(rt, arg(a, 0), len)? else {
        return Err(rt.range_error("Invalid index"));
    };
    let mut items = snapshot(rt, this)?;
    if let Some(slot) = items.get_mut(i as usize) {
        *slot = arg(a, 1);
    }
    Ok(rt.new_array_values(&items))
}

fn string_raw(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let cooked = Value::Object(rt.to_object(arg(a, 0))?);
    let raw = rt.get_prop(cooked, PropKey::Str(A_RAW))?;
    let raw = Value::Object(rt.to_object(raw)?);
    let n = rt.length_of(raw)?;
    let mut out: Vec<u16> = Vec::new();
    for i in 0..n {
        tick(rt, i)?;
        let part = rt.get_idx(raw, i)?;
        out.extend_from_slice(&rt.to_units(part)?);
        if i + 1 < n {
            if let Some(sub) = a.get(i as usize + 1) {
                out.extend_from_slice(&rt.to_units(*sub)?);
            }
        }
        if out.len() > (1 << 28) {
            return Err(rt.range_error("Invalid string length"));
        }
    }
    Ok(rt.new_str16(out))
}

pub(super) fn setup(rt: &mut Runtime) {
    let global = Value::Object(rt.global);
    let ctor = |rt: &mut Runtime, name: &str| -> Option<ObjId> {
        let k = rt.key_str(name);
        rt.get_prop(global, k).ok().and_then(|v| v.as_obj())
    };
    if let Some(object) = ctor(rt, "Object") {
        rt.method(object, "fromEntries", 1, from_entries);
    }
    if let Some(string) = ctor(rt, "String") {
        rt.method(string, "raw", 1, string_raw);
    }
    if let Some(ap) = rt.intr.array_proto {
        let methods: [(&str, u32, super::NFn); 6] =
            [("at", 1, at), ("findLast", 1, find_last), ("findLastIndex", 1, find_last_index), ("toReversed", 0, to_reversed), ("toSorted", 1, to_sorted), ("with", 2, with)];
        for (n, ar, f) in methods {
            rt.method(ap, n, ar, f);
        }
    }
}
