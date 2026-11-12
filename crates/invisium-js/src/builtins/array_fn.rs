//! Array.prototype: callback methods, searching, join, sort, flat.

use super::arg;
use super::array::{has_index, set_index, set_len, this_len, tick};
use crate::runtime::*;
use crate::value::*;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq)]
enum Cb { ForEach, Map, Filter, Some, Every, Find, FindIndex }

fn cb_loop(rt: &mut Runtime, this: Value, a: &[Value], kind: Cb) -> JsResult<Value> {
    let (o, n) = this_len(rt, this)?;
    let f = rt.callable_arg(arg(a, 0), "callback")?;
    let t = arg(a, 1);
    let species = match kind {
        Cb::Map => rt.array_species(o, n)?,
        Cb::Filter => rt.array_species(o, 0)?,
        _ => None,
    };
    let out = match (kind, species) {
        (_, Some(v)) => v,
        (Cb::Map, None) => {
            rt.check_alloc(n as usize * 16).map_err(|e| rt.thrown_to_exc(e))?;
            Value::Object(rt.new_array(vec![None; n as usize]))
        }
        (Cb::Filter, None) => Value::Object(rt.new_array(Vec::new())),
        _ => Value::Undefined,
    };
    let mut kept = 0u64;
    let visit_holes = matches!(kind, Cb::Find | Cb::FindIndex);
    for i in 0..n {
        tick(rt, i)?;
        if !visit_holes && !has_index(rt, o, i)? {
            continue;
        }
        let v = rt.get_idx(o, i)?;
        let r = rt.call_value(f, t, &[v, Value::Number(i as f64), o])?;
        match kind {
            Cb::ForEach => {}
            Cb::Map => set_index(rt, out, i, r)?,
            Cb::Filter => {
                if rt.truthy(r) {
                    set_index(rt, out, kept, v)?;
                    kept += 1;
                }
            }
            Cb::Some => {
                if rt.truthy(r) {
                    return Ok(Value::Bool(true));
                }
            }
            Cb::Every => {
                if !rt.truthy(r) {
                    return Ok(Value::Bool(false));
                }
            }
            Cb::Find => {
                if rt.truthy(r) {
                    return Ok(v);
                }
            }
            Cb::FindIndex => {
                if rt.truthy(r) {
                    return Ok(Value::Number(i as f64));
                }
            }
        }
    }
    Ok(match kind {
        Cb::ForEach | Cb::Find => Value::Undefined,
        Cb::Map | Cb::Filter => out,
        Cb::Some => Value::Bool(false),
        Cb::Every => Value::Bool(true),
        Cb::FindIndex => Value::Number(-1.0),
    })
}

fn for_each(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { cb_loop(rt, t, a, Cb::ForEach) }
fn map(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { cb_loop(rt, t, a, Cb::Map) }
fn filter(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { cb_loop(rt, t, a, Cb::Filter) }
fn some(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { cb_loop(rt, t, a, Cb::Some) }
fn every(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { cb_loop(rt, t, a, Cb::Every) }
fn find(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { cb_loop(rt, t, a, Cb::Find) }
fn find_index(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { cb_loop(rt, t, a, Cb::FindIndex) }

fn reduce_impl(rt: &mut Runtime, this: Value, a: &[Value], right: bool) -> JsResult<Value> {
    let (o, n) = this_len(rt, this)?;
    let f = rt.callable_arg(arg(a, 0), "reducer")?;
    let idx = |k: u64| if right { n - 1 - k } else { k };
    let mut k = 0u64;
    let mut acc = if a.len() >= 2 {
        a[1]
    } else {
        let mut found = None;
        while k < n {
            let i = idx(k);
            k += 1;
            if has_index(rt, o, i)? {
                found = Some(rt.get_idx(o, i)?);
                break;
            }
        }
        match found {
            Some(v) => v,
            None => return Err(rt.type_error("Reduce of empty array with no initial value")),
        }
    };
    let mark = rt.scratch.len();
    while k < n {
        tick(rt, k)?;
        let i = idx(k);
        k += 1;
        if has_index(rt, o, i)? {
            let v = rt.get_idx(o, i)?;
            acc = rt.call_value(f, Value::Undefined, &[acc, v, Value::Number(i as f64), o])?;
            rt.scratch.truncate(mark);
            rt.scratch.push(acc);
        }
    }
    Ok(acc)
}

fn reduce(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { reduce_impl(rt, t, a, false) }
fn reduce_right(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { reduce_impl(rt, t, a, true) }

fn search(rt: &mut Runtime, this: Value, a: &[Value], mode: u8) -> JsResult<Value> {
    // mode 0 = indexOf, 1 = lastIndexOf, 2 = includes
    let (o, n) = this_len(rt, this)?;
    let target = arg(a, 0);
    if n == 0 {
        return Ok(if mode == 2 { Value::Bool(false) } else { Value::Number(-1.0) });
    }
    let nf = n as f64;
    if mode == 1 {
        let mut k = if a.len() > 1 {
            let f = crate::ops::to_integer(rt.to_num(a[1])?);
            if f < 0.0 { nf + f } else { f.min(nf - 1.0) }
        } else {
            nf - 1.0
        };
        while k >= 0.0 {
            let i = k as u64;
            if has_index(rt, o, i)? {
                let v = rt.get_idx(o, i)?;
                if rt.strict_equals(v, target) {
                    return Ok(Value::Number(k));
                }
                // comparing BigInts is charged by size: settle it per element
                rt.drain_pending_steps()?;
            }
            k -= 1.0;
        }
        return Ok(Value::Number(-1.0));
    }
    let from = crate::ops::to_integer(rt.to_num(arg(a, 1))?);
    let start = if from < 0.0 { (nf + from).max(0.0) } else { from } as u64;
    for i in start..n {
        tick(rt, i)?;
        if mode == 2 {
            let v = rt.get_idx(o, i)?;
            if rt.same_value_zero(v, target) {
                return Ok(Value::Bool(true));
            }
        } else if has_index(rt, o, i)? {
            let v = rt.get_idx(o, i)?;
            if rt.strict_equals(v, target) {
                return Ok(Value::Number(i as f64));
            }
        }
        rt.drain_pending_steps()?;
    }
    Ok(if mode == 2 { Value::Bool(false) } else { Value::Number(-1.0) })
}

fn index_of(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { search(rt, t, a, 0) }
fn last_index_of(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { search(rt, t, a, 1) }
fn includes(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { search(rt, t, a, 2) }

fn join(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, n) = this_len(rt, this)?;
    let sep: Rc<[u16]> = match arg(a, 0) {
        Value::Undefined => Rc::from(vec![b',' as u16]),
        v => rt.to_units(v)?,
    };
    let oid = o.as_obj().unwrap_or(ObjId(0));
    if rt.join_stack.contains(&oid) {
        return Ok(rt.atom_value(A_EMPTY));
    }
    rt.join_stack.push(oid);
    let r = join_inner(rt, o, n, &sep);
    rt.join_stack.pop();
    r
}

fn join_inner(rt: &mut Runtime, o: Value, n: u64, sep: &[u16]) -> JsResult<Value> {
    let mut out: Vec<u16> = Vec::new();
    for i in 0..n {
        tick(rt, i)?;
        if i > 0 {
            out.extend_from_slice(sep);
        }
        let v = rt.get_idx(o, i)?;
        if !v.is_nullish() {
            let s = rt.to_str(v)?;
            out.extend_from_slice(rt.str_units(s));
        }
        if out.len() > (1 << 28) {
            return Err(rt.range_error("Invalid string length"));
        }
        if out.len() & 0xFFFF == 0 {
            rt.check_alloc(out.len() * 2).map_err(|t| rt.thrown_to_exc(t))?;
        }
    }
    Ok(rt.new_str16(out))
}

/// Array.prototype.toLocaleString: each element's own `toLocaleString(locales, options)`,
/// joined with commas.
fn to_locale_string(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, n) = this_len(rt, this)?;
    let oid = o.as_obj().unwrap_or(ObjId(0));
    if rt.join_stack.contains(&oid) {
        return Ok(rt.atom_value(A_EMPTY));
    }
    rt.join_stack.push(oid);
    let r = locale_join(rt, o, n, a);
    rt.join_stack.pop();
    r
}

fn locale_join(rt: &mut Runtime, o: Value, n: u64, a: &[Value]) -> JsResult<Value> {
    let mut out: Vec<u16> = Vec::new();
    let key = rt.key_str("toLocaleString");
    for i in 0..n {
        tick(rt, i)?;
        if i > 0 {
            out.push(b',' as u16);
        }
        let v = rt.get_idx(o, i)?;
        if !v.is_nullish() {
            let f = rt.get_prop(v, key)?;
            if !rt.is_callable_val(f) {
                return Err(rt.type_error("toLocaleString is not a function"));
            }
            let r = rt.call_value(f, v, &[arg(a, 0), arg(a, 1)])?;
            let s = rt.to_str(r)?;
            out.extend_from_slice(rt.str_units(s));
        }
        if out.len() > (1 << 28) {
            return Err(rt.range_error("Invalid string length"));
        }
    }
    Ok(rt.new_str16(out))
}

fn to_string(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = rt.to_object(this)?;
    let k = rt.key_str("join");
    let j = rt.get_prop(Value::Object(o), k)?;
    if rt.is_callable_val(j) {
        return rt.call_value(j, Value::Object(o), &[]);
    }
    super::object::object_to_string(rt, Value::Object(o), &[])
}

pub(super) fn sort(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let cmp = arg(a, 0);
    if cmp != Value::Undefined && !rt.is_callable_val(cmp) {
        return Err(rt.type_error("The comparison function must be either a function or undefined"));
    }
    let (o, n) = this_len(rt, this)?;
    let mut items = Vec::new();
    let mut undefs = 0u64;
    for i in 0..n {
        tick(rt, i)?;
        if has_index(rt, o, i)? {
            let v = rt.get_idx(o, i)?;
            if v == Value::Undefined {
                undefs += 1;
            } else {
                items.push(v);
                rt.scratch.push(v);
            }
        }
    }
    let sorted = if cmp == Value::Undefined {
        let mut keyed: Vec<(Rc<[u16]>, Value)> = Vec::with_capacity(items.len());
        for v in items {
            let s = rt.to_units(v)?;
            keyed.push((s, v));
        }
        keyed.sort_by(|x, y| x.0.cmp(&y.0));
        keyed.into_iter().map(|(_, v)| v).collect()
    } else {
        merge_sort(rt, items, cmp)?
    };
    let count = sorted.len() as u64;
    for (i, v) in sorted.into_iter().enumerate() {
        set_index(rt, o, i as u64, v)?;
    }
    for i in count..count + undefs {
        set_index(rt, o, i, Value::Undefined)?;
    }
    for i in count + undefs..n {
        super::array::delete_index(rt, o, i)?;
    }
    Ok(o)
}

fn merge_sort(rt: &mut Runtime, mut src: Vec<Value>, cmp: Value) -> JsResult<Vec<Value>> {
    let n = src.len();
    let mut buf = vec![Value::Undefined; n];
    let mut width = 1;
    while width < n {
        let mut i = 0;
        while i < n {
            let mid = (i + width).min(n);
            let hi = (i + 2 * width).min(n);
            let (mut l, mut r, mut k) = (i, mid, i);
            while l < mid && r < hi {
                let res = rt.call_value(cmp, Value::Undefined, &[src[r], src[l]])?;
                let x = rt.to_num(res)?;
                if x < 0.0 {
                    buf[k] = src[r];
                    r += 1;
                } else {
                    buf[k] = src[l];
                    l += 1;
                }
                k += 1;
            }
            buf[k..k + (mid - l)].copy_from_slice(&src[l..mid]);
            k += mid - l;
            buf[k..k + (hi - r)].copy_from_slice(&src[r..hi]);
            i += 2 * width;
        }
        std::mem::swap(&mut src, &mut buf);
        width *= 2;
    }
    Ok(src)
}

fn is_array_val(rt: &Runtime, v: Value) -> bool {
    matches!(v, Value::Object(o) if matches!(rt.obj(o).kind, ObjKind::Array(_)))
}

fn flatten(rt: &mut Runtime, out: Value, start: u64, src: Value, depth: f64) -> JsResult<u64> {
    let mut n = start;
    let len = rt.length_of(src)?;
    let mut stack: Vec<(Value, u64, u64, f64)> = vec![(src, 0, len, depth)];
    let mut steps = 0u64;
    while let Some(top) = stack.last_mut() {
        if top.1 >= top.2 {
            stack.pop();
            continue;
        }
        let (s, i, d) = (top.0, top.1, top.3);
        top.1 += 1;
        steps += 1;
        tick(rt, steps)?;
        if !has_index(rt, s, i)? {
            continue;
        }
        let v = rt.get_idx(s, i)?;
        if d >= 1.0 && is_array_val(rt, v) {
            let l = rt.length_of(v)?;
            stack.push((v, 0, l, d - 1.0));
        } else {
            set_index(rt, out, n, v)?;
            n += 1;
        }
    }
    Ok(n)
}

fn flat(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let o = Value::Object(rt.to_object(this)?);
    let depth = match arg(a, 0) {
        Value::Undefined => 1.0,
        v => crate::ops::to_integer(rt.to_num(v)?),
    };
    let out = match rt.array_species(o, 0)? {
        Some(v) => v,
        None => Value::Object(rt.new_array(Vec::new())),
    };
    let n = flatten(rt, out, 0, o, depth)?;
    set_len(rt, out, n)?;
    Ok(out)
}

fn flat_map(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, n) = this_len(rt, this)?;
    let f = rt.callable_arg(arg(a, 0), "flatMap")?;
    let out = match rt.array_species(o, 0)? {
        Some(v) => v,
        None => Value::Object(rt.new_array(Vec::new())),
    };
    let mut k = 0u64;
    for i in 0..n {
        tick(rt, i)?;
        if !has_index(rt, o, i)? {
            continue;
        }
        let v = rt.get_idx(o, i)?;
        let r = rt.call_value(f, arg(a, 1), &[v, Value::Number(i as f64), o])?;
        if is_array_val(rt, r) {
            k = flatten(rt, out, k, r, 0.0)?;
        } else {
            set_index(rt, out, k, r)?;
            k += 1;
        }
    }
    Ok(out)
}

pub(super) fn setup(rt: &mut Runtime, ap: ObjId) {
    let methods: [(&str, u32, super::NFn); 18] = [
        ("forEach", 1, for_each),
        ("map", 1, map),
        ("filter", 1, filter),
        ("some", 1, some),
        ("every", 1, every),
        ("find", 1, find),
        ("findIndex", 1, find_index),
        ("reduce", 1, reduce),
        ("reduceRight", 1, reduce_right),
        ("indexOf", 1, index_of),
        ("lastIndexOf", 1, last_index_of),
        ("includes", 1, includes),
        ("join", 1, join),
        ("toString", 0, to_string),
        ("toLocaleString", 0, to_locale_string),
        ("sort", 1, sort),
        ("flat", 0, flat),
        ("flatMap", 1, flat_map),
    ];
    for (n, ar, f) in methods {
        rt.method(ap, n, ar, f);
    }
}
