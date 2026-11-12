//! Web-compatibility surface of Object.prototype: the `__proto__` accessor and the legacy
//! `__defineGetter__` / `__defineSetter__` / `__lookupGetter__` / `__lookupSetter__`.

use super::arg;
use crate::runtime::*;
use crate::value::*;

fn proto_get(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = rt.to_object(this)?;
    Ok(rt.proto_of(o).map(Value::Object).unwrap_or(Value::Null))
}

fn proto_set(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    if this.is_nullish() {
        return Err(rt.type_error("Object.prototype.__proto__ called on null or undefined"));
    }
    let proto = match arg(a, 0) {
        Value::Object(p) => Some(p),
        Value::Null => None,
        _ => return Ok(Value::Undefined),
    };
    if let Value::Object(o) = this {
        if !rt.set_proto_of(o, proto) {
            return Err(rt.type_error("Cannot set prototype: cyclic __proto__ value or object is not extensible"));
        }
    }
    Ok(Value::Undefined)
}

/// Shared body of `__defineGetter__` (is_get) and `__defineSetter__`.
fn define_half(rt: &mut Runtime, this: Value, a: &[Value], is_get: bool) -> JsResult<Value> {
    let o = rt.to_object(this)?;
    let f = arg(a, 1);
    if !rt.is_callable_val(f) {
        return Err(rt.type_error(if is_get { "Object.prototype.__defineGetter__: Expecting function" } else { "Object.prototype.__defineSetter__: Expecting function" }));
    }
    let k = rt.to_property_key(arg(a, 0))?;
    let desc = rt.plain_object();
    let which = rt.key_str(if is_get { "get" } else { "set" });
    rt.define_value(desc, which, f, DEFAULT_FLAGS);
    for name in ["enumerable", "configurable"] {
        let key = rt.key_str(name);
        rt.define_value(desc, key, Value::Bool(true), DEFAULT_FLAGS);
    }
    if !super::define_desc(rt, o, k, Value::Object(desc))? {
        return Err(rt.type_error("Cannot redefine property"));
    }
    Ok(Value::Undefined)
}

fn define_getter(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    define_half(rt, this, a, true)
}
fn define_setter(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    define_half(rt, this, a, false)
}

/// Shared body of `__lookupGetter__` / `__lookupSetter__`: walks the prototype chain.
fn lookup_half(rt: &mut Runtime, this: Value, a: &[Value], is_get: bool) -> JsResult<Value> {
    let mut cur = Some(rt.to_object(this)?);
    let k = rt.to_property_key(arg(a, 0))?;
    let mut guard = 0;
    while let Some(o) = cur {
        if let Some(p) = rt.get_own_prop_ex(o, k)? {
            return Ok(match (p.is_accessor(), is_get) {
                (true, true) => p.value,
                (true, false) => p.setter,
                _ => Value::Undefined,
            });
        }
        cur = rt.proto_of(o);
        guard += 1;
        if guard > 100_000 {
            break;
        }
    }
    Ok(Value::Undefined)
}

fn lookup_getter(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    lookup_half(rt, this, a, true)
}
fn lookup_setter(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    lookup_half(rt, this, a, false)
}

pub(super) fn setup(rt: &mut Runtime) {
    let Some(op) = rt.intr.object_proto else { return };
    let get = rt.native_fn("get __proto__", 0, proto_get);
    let set = rt.native_fn("set __proto__", 1, proto_set);
    let k = rt.key_str("__proto__");
    rt.define_own(op, k, Prop { value: Value::Object(get), setter: Value::Object(set), flags: ACCESSOR | CONFIGURABLE });
    let methods: [(&str, u32, super::NFn); 4] =
        [("__defineGetter__", 2, define_getter), ("__defineSetter__", 2, define_setter), ("__lookupGetter__", 1, lookup_getter), ("__lookupSetter__", 1, lookup_setter)];
    for (n, ar, f) in methods {
        rt.method(op, n, ar, f);
    }
}
