//! WeakMap and WeakSet. Entries are keyed by object handle; the collector keeps a value alive
//! only while its key is, and removes the entries of dead keys (`collect_garbage`).

use super::arg;
use crate::runtime::*;
use crate::value::*;
use std::collections::HashMap;

fn weak_data(rt: &mut Runtime, this: Value, set: bool) -> JsResult<&mut HashMap<u32, Value>> {
    let o = match this {
        Value::Object(o) if matches!((&rt.obj(o).kind, set), (ObjKind::WeakMap(_), false) | (ObjKind::WeakSet(_), true)) => o,
        _ => return Err(rt.type_error(if set { "Method WeakSet.prototype method called on incompatible receiver" } else { "Method WeakMap.prototype method called on incompatible receiver" })),
    };
    match &mut rt.obj_mut(o).kind {
        ObjKind::WeakMap(m) | ObjKind::WeakSet(m) => Ok(m),
        _ => Err(Exc::Throw(Value::Undefined)),
    }
}

fn weak_insert(rt: &mut Runtime, this: Value, key: Value, value: Value, set: bool) -> JsResult<Value> {
    let Value::Object(k) = key else {
        return Err(rt.type_error(if set { "Invalid value used in weak set" } else { "Invalid value used as weak map key" }));
    };
    if weak_data(rt, this, set)?.insert(k.0, value).is_none() {
        rt.grow(40);
    }
    Ok(this)
}

fn construct_weak(rt: &mut Runtime, a: &[Value], set: bool) -> JsResult<Value> {
    let nt = rt.native_new_target;
    if nt == Value::Undefined {
        return Err(rt.type_error(if set { "Constructor WeakSet requires 'new'" } else { "Constructor WeakMap requires 'new'" }));
    }
    let (default, kind) = if set { (rt.intr.weakset_proto, ObjKind::WeakSet(Box::default())) } else { (rt.intr.weakmap_proto, ObjKind::WeakMap(Box::default())) };
    let proto = rt.proto_from_ctor(nt, default)?;
    let o = Value::Object(rt.alloc(Object::new(proto, kind)));
    let src = arg(a, 0);
    if src.is_nullish() {
        return Ok(o);
    }
    let it = rt.get_iterator(src)?;
    rt.scratch.push(it);
    let mark = rt.scratch.len();
    while let Some(item) = rt.iter_step(it)? {
        rt.scratch.push(item);
        if set {
            weak_insert(rt, o, item, Value::Bool(true), true)?;
        } else {
            if !matches!(item, Value::Object(_)) {
                return Err(rt.type_error("Iterator value is not an entry object"));
            }
            let k = rt.get_idx(item, 0)?;
            rt.scratch.push(k);
            let v = rt.get_idx(item, 1)?;
            weak_insert(rt, o, k, v, false)?;
        }
        rt.scratch.truncate(mark);
    }
    Ok(o)
}

fn weakmap_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    construct_weak(rt, a, false)
}
fn weakset_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    construct_weak(rt, a, true)
}

fn weakmap_get(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let m = weak_data(rt, this, false)?;
    Ok(match arg(a, 0) {
        Value::Object(k) => m.get(&k.0).copied().unwrap_or(Value::Undefined),
        _ => Value::Undefined,
    })
}
fn weakmap_set(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    weak_insert(rt, this, arg(a, 0), arg(a, 1), false)
}
fn weakset_add(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    weak_insert(rt, this, arg(a, 0), Value::Bool(true), true)
}

fn has(rt: &mut Runtime, this: Value, a: &[Value], set: bool) -> JsResult<Value> {
    let m = weak_data(rt, this, set)?;
    Ok(Value::Bool(matches!(arg(a, 0), Value::Object(k) if m.contains_key(&k.0))))
}
fn delete(rt: &mut Runtime, this: Value, a: &[Value], set: bool) -> JsResult<Value> {
    let m = weak_data(rt, this, set)?;
    Ok(Value::Bool(matches!(arg(a, 0), Value::Object(k) if m.remove(&k.0).is_some())))
}

fn weakmap_has(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { has(rt, t, a, false) }
fn weakset_has(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { has(rt, t, a, true) }
fn weakmap_delete(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { delete(rt, t, a, false) }
fn weakset_delete(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { delete(rt, t, a, true) }

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto;
    let wm = rt.new_object(op);
    rt.intr.weakmap_proto = Some(wm);
    rt.constructor("WeakMap", 0, weakmap_ctor, wm);
    let methods: [(&str, u32, super::NFn); 4] = [("get", 1, weakmap_get), ("set", 2, weakmap_set), ("has", 1, weakmap_has), ("delete", 1, weakmap_delete)];
    for (n, ar, f) in methods {
        rt.method(wm, n, ar, f);
    }
    let t = rt.new_str("WeakMap");
    rt.define_value(wm, PropKey::Sym(SYM_TO_STRING_TAG), t, CONFIGURABLE);

    let ws = rt.new_object(op);
    rt.intr.weakset_proto = Some(ws);
    rt.constructor("WeakSet", 0, weakset_ctor, ws);
    let methods: [(&str, u32, super::NFn); 3] = [("add", 1, weakset_add), ("has", 1, weakset_has), ("delete", 1, weakset_delete)];
    for (n, ar, f) in methods {
        rt.method(ws, n, ar, f);
    }
    let t = rt.new_str("WeakSet");
    rt.define_value(ws, PropKey::Sym(SYM_TO_STRING_TAG), t, CONFIGURABLE);
}
