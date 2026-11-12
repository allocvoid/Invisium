//! Reflect and the Proxy constructor.

use super::arg;
use super::function::list_from_array_like;
use super::object::{define_desc, descriptor_object};
use crate::promise::slot;
use crate::runtime::*;
use crate::value::*;

fn target(rt: &mut Runtime, v: Value, what: &str) -> JsResult<ObjId> {
    match v {
        Value::Object(o) => Ok(o),
        _ => Err(rt.type_error(&format!("Reflect.{} called on non-object", what))),
    }
}

fn apply(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let f = rt.callable_arg(arg(a, 0), "Reflect.apply")?;
    let args = list_from_array_like(rt, arg(a, 2))?;
    rt.call_value(f, arg(a, 1), &args)
}

fn construct(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let f = arg(a, 0);
    let nt = if a.len() > 2 { arg(a, 2) } else { f };
    if !rt.is_constructor(f) || !rt.is_constructor(nt) {
        return Err(rt.type_error("Reflect.construct requires a constructor"));
    }
    let args = list_from_array_like(rt, arg(a, 1))?;
    rt.construct_value(f, &args, nt)
}

fn define_property(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = target(rt, arg(a, 0), "defineProperty")?;
    let k = rt.to_property_key(arg(a, 1))?;
    Ok(Value::Bool(define_desc(rt, o, k, arg(a, 2))?))
}

fn delete_property(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = target(rt, arg(a, 0), "deleteProperty")?;
    let k = rt.to_property_key(arg(a, 1))?;
    Ok(Value::Bool(rt.delete_prop_ex(o, k)?))
}

fn get(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = target(rt, arg(a, 0), "get")?;
    let k = rt.to_property_key(arg(a, 1))?;
    let receiver = if a.len() > 2 { arg(a, 2) } else { Value::Object(o) };
    rt.get_from(o, k, receiver)
}

fn get_own_property_descriptor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = target(rt, arg(a, 0), "getOwnPropertyDescriptor")?;
    let k = rt.to_property_key(arg(a, 1))?;
    Ok(match rt.get_own_prop_ex(o, k)? {
        Some(p) => descriptor_object(rt, p),
        None => Value::Undefined,
    })
}

fn get_prototype_of(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = target(rt, arg(a, 0), "getPrototypeOf")?;
    Ok(rt.proto_of(o).map(Value::Object).unwrap_or(Value::Null))
}

fn has(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = target(rt, arg(a, 0), "has")?;
    let k = rt.to_property_key(arg(a, 1))?;
    Ok(Value::Bool(rt.has_property(o, k)?))
}

fn own_keys(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = target(rt, arg(a, 0), "ownKeys")?;
    let keys: Vec<Value> = rt.own_keys_ex(o)?.into_iter().map(|k| rt.key_to_value(k)).collect();
    Ok(rt.new_array_values(&keys))
}

fn set(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = target(rt, arg(a, 0), "set")?;
    let k = rt.to_property_key(arg(a, 1))?;
    let receiver = if a.len() > 3 { arg(a, 3) } else { Value::Object(o) };
    Ok(Value::Bool(rt.set_prop(Value::Object(o), k, arg(a, 2), receiver)?))
}

fn set_prototype_of(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = target(rt, arg(a, 0), "setPrototypeOf")?;
    let proto = match arg(a, 1) {
        Value::Object(p) => Some(p),
        Value::Null => None,
        _ => return Err(rt.type_error("Object prototype may only be an Object or null")),
    };
    Ok(Value::Bool(rt.set_proto_of(o, proto)))
}

fn make_proxy(rt: &mut Runtime, a: &[Value]) -> JsResult<ObjId> {
    let (Value::Object(t), Value::Object(h)) = (arg(a, 0), arg(a, 1)) else {
        return Err(rt.type_error("Cannot create proxy with a non-object as target or handler"));
    };
    let data = ProxyData { target: Some(t), handler: Some(h), callable: rt.is_callable_obj(t), ctor: rt.is_constructor(Value::Object(t)) };
    // the prototype slot mirrors the target's, for code that walks prototype chains directly
    // (a proxy target already mirrors its own target, so this stays O(1) for chains)
    let proto = rt.obj(t).proto;
    Ok(rt.alloc(Object::new(proto, ObjKind::Proxy(data))))
}

fn proxy_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    if rt.native_new_target == Value::Undefined {
        return Err(rt.type_error("Constructor Proxy requires 'new'"));
    }
    Ok(Value::Object(make_proxy(rt, a)?))
}

/// slots = [proxy]
fn revoke(rt: &mut Runtime, slots: &[Value], _: Value, _: &[Value]) -> JsResult<Value> {
    if let Value::Object(p) = slot(slots, 0) {
        if let ObjKind::Proxy(d) = &mut rt.obj_mut(p).kind {
            d.target = None;
            d.handler = None;
        }
    }
    Ok(Value::Undefined)
}

fn revocable(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let p = make_proxy(rt, a)?;
    let f = rt.new_intern("", 0, revoke, vec![Value::Object(p)]);
    let out = rt.plain_object();
    let k = rt.key_str("proxy");
    rt.define_value(out, k, Value::Object(p), DEFAULT_FLAGS);
    let k = rt.key_str("revoke");
    rt.define_value(out, k, Value::Object(f), DEFAULT_FLAGS);
    Ok(Value::Object(out))
}

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto;
    let r = rt.new_object(op);
    let methods: [(&str, u32, super::NFn); 11] = [
        ("apply", 3, apply),
        ("construct", 2, construct),
        ("defineProperty", 3, define_property),
        ("deleteProperty", 2, delete_property),
        ("get", 2, get),
        ("getOwnPropertyDescriptor", 2, get_own_property_descriptor),
        ("getPrototypeOf", 1, get_prototype_of),
        ("has", 2, has),
        ("ownKeys", 1, own_keys),
        ("set", 3, set),
        ("setPrototypeOf", 2, set_prototype_of),
    ];
    for (n, ar, f) in methods {
        rt.method(r, n, ar, f);
    }
    let tag = rt.new_str("Reflect");
    rt.define_value(r, PropKey::Sym(SYM_TO_STRING_TAG), tag, CONFIGURABLE);
    rt.set_global("Reflect", Value::Object(r));

    let pc = rt.new_native("Proxy", 2, Runtime::wrap(proxy_ctor), true);
    rt.method(pc, "revocable", 2, revocable);
    rt.set_global("Proxy", Value::Object(pc));
}
