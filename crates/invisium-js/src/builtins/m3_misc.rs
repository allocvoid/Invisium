//! Newer and legacy library functions pages call: Array.prototype.copyWithin / toSpliced,
//! Object.groupBy, Map.groupBy, Promise.withResolvers, Array.fromAsync and
//! RegExp.prototype.compile.

use super::arg;
use super::array::{delete_index, has_index, set_index, this_len, tick};
use crate::runtime::*;
use crate::value::*;
use std::collections::HashMap;

fn copy_within(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, len) = this_len(rt, this)?;
    let lf = len as f64;
    let to = rt.rel_index(arg(a, 0), lf, 0.0)? as u64;
    let from = rt.rel_index(arg(a, 1), lf, 0.0)? as u64;
    let fin = rt.rel_index(arg(a, 2), lf, lf)? as u64;
    let count = fin.saturating_sub(from).min(len - to);
    // overlapping ranges are copied from the end so nothing is read after being overwritten
    let backwards = from < to && to < from + count;
    for n in 0..count {
        tick(rt, n)?;
        let i = if backwards { count - 1 - n } else { n };
        if has_index(rt, o, from + i)? {
            let v = rt.get_idx(o, from + i)?;
            set_index(rt, o, to + i, v)?;
        } else {
            delete_index(rt, o, to + i)?;
        }
    }
    Ok(o)
}

fn to_spliced(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, len) = this_len(rt, this)?;
    let start = rt.rel_index(arg(a, 0), len as f64, 0.0)? as u64;
    let skip = match a.len() {
        0 => 0,
        1 => len - start,
        _ => crate::ops::to_integer(rt.to_num(a[1])?).max(0.0).min((len - start) as f64) as u64,
    };
    let items = a.get(2..).unwrap_or(&[]);
    let new_len = len - skip + items.len() as u64;
    if new_len > u32::MAX as u64 {
        return Err(rt.range_error("Invalid array length"));
    }
    rt.check_alloc(new_len as usize * 16).map_err(|t| rt.thrown_to_exc(t))?;
    let mut out = Vec::with_capacity(new_len as usize);
    for i in (0..start).chain(start + skip..len) {
        tick(rt, i)?;
        if i == start + skip {
            out.extend_from_slice(items);
        }
        let v = rt.get_idx(o, i)?;
        rt.scratch.push(v);
        out.push(v);
    }
    if start + skip >= len {
        out.extend_from_slice(items);
    }
    Ok(rt.new_array_values(&out))
}

/// Runs `callback(value, index)` over an iterable and collects the values per returned key.
fn group<K: std::hash::Hash + Eq + Clone>(rt: &mut Runtime, a: &[Value], mut key_of: impl FnMut(&mut Runtime, Value) -> JsResult<(K, Value)>) -> JsResult<Vec<(Value, Vec<Value>)>> {
    let cb = rt.callable_arg(arg(a, 1), "groupBy callback")?;
    let items = arg(a, 0);
    if items.is_nullish() {
        return Err(rt.type_error("groupBy requires an iterable"));
    }
    let it = rt.get_iterator(items)?;
    rt.scratch.push(it);
    let mut index: HashMap<K, usize> = HashMap::new();
    let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
    let mut i = 0u64;
    while let Some(v) = rt.iter_step(it)? {
        tick(rt, i)?;
        rt.scratch.push(v);
        let k = match rt.call_value(cb, Value::Undefined, &[v, Value::Number(i as f64)]).and_then(|k| key_of(rt, k)) {
            Ok(k) => k,
            Err(e) => {
                let _ = rt.iter_close(it);
                return Err(e);
            }
        };
        rt.scratch.push(k.1);
        match index.get(&k.0) {
            Some(&g) => groups[g].1.push(v),
            None => {
                index.insert(k.0, groups.len());
                groups.push((k.1, vec![v]));
            }
        }
        i += 1;
    }
    Ok(groups)
}

fn object_group_by(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let groups = group(rt, a, |rt, k| {
        let key = rt.to_property_key(k)?;
        Ok((key, rt.key_to_value(key)))
    })?;
    let out = rt.new_object(None);
    for (k, items) in groups {
        let key = rt.to_property_key(k)?;
        let arr = rt.new_array_values(&items);
        rt.define_value(out, key, arr, DEFAULT_FLAGS);
    }
    Ok(Value::Object(out))
}

fn map_group_by(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let groups = group(rt, a, |rt, k| Ok((rt.map_key(k), k)))?;
    let proto = rt.intr.map_proto;
    let out = rt.alloc(Object::new(proto, ObjKind::Map(Box::default())));
    for (k, items) in groups {
        let arr = rt.new_array_values(&items);
        rt.map_insert(out, k, arr);
        rt.drain_pending_steps()?;
    }
    Ok(Value::Object(out))
}

fn with_resolvers(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    if !matches!(this, Value::Object(_)) {
        return Err(rt.type_error("Promise.withResolvers called on non-object"));
    }
    let (p, res, rej) = if rt.is_intrinsic_promise_ctor(this) {
        let proto = rt.proto_from_ctor(this, rt.intr.promise_proto)?;
        let p = rt.new_promise_obj(proto);
        let (res, rej) = rt.resolving_functions(p);
        (Value::Object(p), res, rej)
    } else {
        let cap = rt.new_promise_capability(this)?;
        let (res, rej) = rt.cap_functions(&cap);
        (cap.promise(), res, rej)
    };
    let out = rt.plain_object();
    for (name, v) in [("promise", p), ("resolve", res), ("reject", rej)] {
        let k = rt.key_str(name);
        rt.define_value(out, k, v, DEFAULT_FLAGS);
    }
    Ok(Value::Object(out))
}

/// Array.fromAsync is an async loop; it is written in script and compiled on first use.
const FROM_ASYNC: &str = "(async function fromAsync(C, items, mapfn, thisArg) {
  if (mapfn !== undefined && typeof mapfn !== 'function') throw new TypeError('Array.fromAsync: the mapper is not a function');
  if (items == null) throw new TypeError('Array.fromAsync requires an array-like or iterable object');
  var out, i = 0, v;
  if (items[Symbol.asyncIterator] != null || items[Symbol.iterator] != null) {
    out = typeof C === 'function' && C !== Array ? new C() : [];
    for await (v of items) { out[i] = mapfn ? await mapfn.call(thisArg, v, i) : v; i++; }
  } else {
    var like = Object(items), len = Math.min(Math.max(Math.trunc(Number(like.length)) || 0, 0), 9007199254740991);
    out = typeof C === 'function' && C !== Array ? new C(len) : [];
    for (; i < len; i++) { v = await like[i]; out[i] = mapfn ? await mapfn.call(thisArg, v, i) : v; }
  }
  out.length = i;
  return out;
})";

fn from_async(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let helper = match rt.intr.from_async {
        Some(h) => Value::Object(h),
        None => {
            let h = rt.eval_internal(FROM_ASYNC, "<native>")?;
            rt.intr.from_async = h.as_obj();
            h
        }
    };
    rt.call_value(helper, Value::Undefined, &[this, arg(a, 0), arg(a, 1), arg(a, 2)])
}

/// Annex B RegExp.prototype.compile: re-initialises the receiver in place.
fn regexp_compile(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let Value::Object(o) = this else { return Err(rt.type_error("RegExp.prototype.compile requires that 'this' be a RegExp object")) };
    if rt.regex_prog(this).is_none() {
        return Err(rt.type_error("RegExp.prototype.compile requires that 'this' be a RegExp object"));
    }
    let prog = match rt.regex_prog(arg(a, 0)) {
        Some(p) => {
            if arg(a, 1) != Value::Undefined {
                return Err(rt.type_error("Invalid flags supplied to RegExp.prototype.compile"));
            }
            p
        }
        None => {
            let source = match arg(a, 0) {
                Value::Undefined => rt.str_rc(StrId::from_atom(A_EMPTY)),
                v => rt.to_units(v)?,
            };
            let flags = match arg(a, 1) {
                Value::Undefined => rt.str_rc(StrId::from_atom(A_EMPTY)),
                v => rt.to_units(v)?,
            };
            let f = rt.parse_flags(&flags)?;
            rt.regex_compile(source, f)?
        }
    };
    rt.obj_mut(o).kind = ObjKind::RegExp(prog);
    rt.set_last_index(this, 0.0)?;
    Ok(this)
}

pub(super) fn setup(rt: &mut Runtime) {
    let global = Value::Object(rt.global);
    let ctor = |rt: &mut Runtime, name: &str| -> Option<ObjId> {
        let k = rt.key_str(name);
        rt.get_prop(global, k).ok().and_then(|v| v.as_obj())
    };
    if let Some(ap) = rt.intr.array_proto {
        rt.method(ap, "copyWithin", 2, copy_within);
        rt.method(ap, "toSpliced", 2, to_spliced);
        // method names a `with (array)` block must not capture
        let unscopables = rt.new_object(None);
        for name in ["at", "copyWithin", "entries", "fill", "find", "findIndex", "findLast", "findLastIndex", "flat", "flatMap", "includes", "keys", "toReversed", "toSorted", "toSpliced", "values"] {
            let k = rt.key_str(name);
            rt.define_value(unscopables, k, Value::Bool(true), DEFAULT_FLAGS);
        }
        rt.define_value(ap, PropKey::Sym(SYM_UNSCOPABLES), Value::Object(unscopables), CONFIGURABLE);
    }
    if let Some(array) = ctor(rt, "Array") {
        rt.method(array, "fromAsync", 1, from_async);
    }
    if let Some(object) = ctor(rt, "Object") {
        rt.method(object, "groupBy", 2, object_group_by);
    }
    if let Some(map) = ctor(rt, "Map") {
        rt.method(map, "groupBy", 2, map_group_by);
    }
    if let Some(promise) = ctor(rt, "Promise") {
        rt.method(promise, "withResolvers", 0, with_resolvers);
    }
    if let Some(rp) = rt.intr.regexp_proto {
        rt.method(rp, "compile", 2, regexp_compile);
    }
}
