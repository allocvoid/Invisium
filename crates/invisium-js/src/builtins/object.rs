//! Object constructor and Object.prototype.

use super::arg;
use crate::runtime::*;
use crate::value::*;

fn object_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let v = arg(a, 0);
    if v.is_nullish() {
        let nt = rt.native_new_target;
        let p = rt.intr.object_proto;
        return Ok(Value::Object(rt.create_from_ctor(nt, p)?));
    }
    Ok(Value::Object(rt.to_object(v)?))
}

fn keys_impl(rt: &mut Runtime, v: Value, mode: u8) -> JsResult<Value> {
    let o = rt.to_object(v)?;
    let keys = rt.own_enum_keys_ex(o)?;
    let mut out = Vec::with_capacity(keys.len());
    for k in keys {
        let kv = rt.key_to_value(k);
        match mode {
            0 => out.push(kv),
            1 => out.push(rt.get_prop(Value::Object(o), k)?),
            _ => {
                let val = rt.get_prop(Value::Object(o), k)?;
                let pair = rt.new_array_values(&[kv, val]);
                out.push(pair);
            }
        }
    }
    Ok(rt.new_array_values(&out))
}

fn keys(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    keys_impl(rt, arg(a, 0), 0)
}
fn values(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    keys_impl(rt, arg(a, 0), 1)
}
fn entries(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    keys_impl(rt, arg(a, 0), 2)
}

fn assign(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let t = rt.to_object(arg(a, 0))?;
    for src in a.iter().skip(1) {
        if src.is_nullish() {
            continue;
        }
        let from = rt.to_object(*src)?;
        for k in rt.own_keys_ex(from)? {
            let en = rt.get_own_prop_ex(from, k)?.map(|p| p.flags & ENUMERABLE != 0).unwrap_or(false);
            if en {
                let v = rt.get_prop(Value::Object(from), k)?;
                if !rt.set_prop(Value::Object(t), k, v, Value::Object(t))? {
                    return Err(rt.type_error("Cannot assign to read only property"));
                }
            }
        }
    }
    Ok(Value::Object(t))
}

fn create(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let proto = match arg(a, 0) {
        Value::Object(o) => Some(o),
        Value::Null => None,
        _ => return Err(rt.type_error("Object prototype may only be an Object or null")),
    };
    let o = rt.new_object(proto);
    if let Value::Object(props) = arg(a, 1) {
        define_props(rt, o, props)?;
    }
    Ok(Value::Object(o))
}

/// Applies a property descriptor object to `o[k]`; a refused definition is a TypeError.
pub(crate) fn define_from_descriptor(rt: &mut Runtime, o: ObjId, k: PropKey, d: Value) -> JsResult<()> {
    if !define_desc(rt, o, k, d)? {
        let n = rt.key_rust(k);
        return Err(rt.type_error(&format!("Cannot redefine property: {}", n)));
    }
    Ok(())
}

/// [[DefineOwnProperty]] with a descriptor object; Ok(false) when the object refuses.
pub(crate) fn define_desc(rt: &mut Runtime, o: ObjId, k: PropKey, d: Value) -> JsResult<bool> {
    let Value::Object(_) = d else { return Err(rt.type_error("Property description must be an object")) };
    if let Some(p) = rt.proxy_data(o) {
        return rt.proxy_define(p, k, d);
    }
    let field = |rt: &mut Runtime, a: Atom| -> JsResult<Option<Value>> {
        let Value::Object(dob) = d else { return Ok(None) };
        if rt.has_property(dob, PropKey::Str(a))? { Ok(Some(rt.get_prop(d, PropKey::Str(a))?)) } else { Ok(None) }
    };
    let value = field(rt, A_VALUE)?;
    let writable = field(rt, A_WRITABLE)?;
    let enumerable = field(rt, A_ENUMERABLE)?;
    let configurable = field(rt, A_CONFIGURABLE)?;
    let get = field(rt, A_GET)?;
    let set = field(rt, A_SET)?;
    for f in [get, set].into_iter().flatten() {
        if f != Value::Undefined && !rt.is_callable_val(f) {
            return Err(rt.type_error("Getter/setter must be a function"));
        }
    }
    let accessor = get.is_some() || set.is_some();
    if accessor && (value.is_some() || writable.is_some()) {
        return Err(rt.type_error("Invalid property descriptor. Cannot both specify accessors and a value or writable attribute"));
    }
    let existing = rt.get_own_prop(o, k);
    let bit = |rt: &Runtime, v: Option<Value>, flag: u8, old: u8| -> u8 {
        match v {
            Some(v) => if rt.truthy(v) { flag } else { 0 },
            None => old & flag,
        }
    };
    let old_flags = existing.map(|p| p.flags).unwrap_or(0);
    let mut flags = bit(rt, enumerable, ENUMERABLE, old_flags) | bit(rt, configurable, CONFIGURABLE, old_flags);
    let p = if accessor {
        flags |= ACCESSOR;
        let (og, os) = match existing {
            Some(e) if e.is_accessor() => (e.value, e.setter),
            _ => (Value::Undefined, Value::Undefined),
        };
        Prop { value: get.unwrap_or(og), setter: set.unwrap_or(os), flags }
    } else {
        let keep_acc = existing.map(|e| e.is_accessor()).unwrap_or(false) && value.is_none() && writable.is_none();
        if keep_acc {
            let e = existing.unwrap_or(Prop::data(Value::Undefined, 0));
            Prop { value: e.value, setter: e.setter, flags: flags | ACCESSOR }
        } else {
            let old_val = existing.filter(|e| !e.is_accessor()).map(|e| e.value).unwrap_or(Value::Undefined);
            let old_w = if existing.map(|e| e.is_accessor()).unwrap_or(false) { 0 } else { old_flags };
            flags |= bit(rt, writable, WRITABLE, old_w);
            Prop::data(value.unwrap_or(old_val), flags)
        }
    };
    // (array element attributes are not tracked per element: only the value is stored)
    Ok(rt.define_own(o, k, p))
}

fn define_props(rt: &mut Runtime, o: ObjId, props: ObjId) -> JsResult<()> {
    for k in rt.own_enum_keys_ex(props)? {
        let d = rt.get_prop(Value::Object(props), k)?;
        define_from_descriptor(rt, o, k, d)?;
    }
    for k in rt.own_keys_ex(props)? {
        if let PropKey::Sym(_) = k {
            let d = rt.get_prop(Value::Object(props), k)?;
            define_from_descriptor(rt, o, k, d)?;
        }
    }
    Ok(())
}

fn define_property(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let Value::Object(o) = arg(a, 0) else { return Err(rt.type_error("Object.defineProperty called on non-object")) };
    let k = rt.to_property_key(arg(a, 1))?;
    define_from_descriptor(rt, o, k, arg(a, 2))?;
    Ok(Value::Object(o))
}

fn define_properties(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let Value::Object(o) = arg(a, 0) else { return Err(rt.type_error("Object.defineProperties called on non-object")) };
    let p = rt.to_object(arg(a, 1))?;
    define_props(rt, o, p)?;
    Ok(Value::Object(o))
}

pub(crate) fn descriptor_object(rt: &mut Runtime, p: Prop) -> Value {
    let d = rt.plain_object();
    if p.is_accessor() {
        rt.define_value(d, PropKey::Str(A_GET), p.value, DEFAULT_FLAGS);
        rt.define_value(d, PropKey::Str(A_SET), p.setter, DEFAULT_FLAGS);
    } else {
        rt.define_value(d, PropKey::Str(A_VALUE), p.value, DEFAULT_FLAGS);
        rt.define_value(d, PropKey::Str(A_WRITABLE), Value::Bool(p.flags & WRITABLE != 0), DEFAULT_FLAGS);
    }
    rt.define_value(d, PropKey::Str(A_ENUMERABLE), Value::Bool(p.flags & ENUMERABLE != 0), DEFAULT_FLAGS);
    rt.define_value(d, PropKey::Str(A_CONFIGURABLE), Value::Bool(p.flags & CONFIGURABLE != 0), DEFAULT_FLAGS);
    Value::Object(d)
}

fn get_own_property_descriptor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = rt.to_object(arg(a, 0))?;
    let k = rt.to_property_key(arg(a, 1))?;
    Ok(match rt.get_own_prop_ex(o, k)? {
        Some(p) => descriptor_object(rt, p),
        None => Value::Undefined,
    })
}

fn get_own_property_descriptors(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = rt.to_object(arg(a, 0))?;
    let out = rt.plain_object();
    for k in rt.own_keys_ex(o)? {
        if let Some(p) = rt.get_own_prop_ex(o, k)? {
            let d = descriptor_object(rt, p);
            rt.define_value(out, k, d, DEFAULT_FLAGS);
        }
    }
    Ok(Value::Object(out))
}

fn get_own_property_names(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = rt.to_object(arg(a, 0))?;
    let ks: Vec<Value> = rt.own_keys_ex(o)?.into_iter().filter(|k| matches!(k, PropKey::Str(_))).map(|k| rt.key_to_value(k)).collect();
    Ok(rt.new_array_values(&ks))
}

fn get_own_property_symbols(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = rt.to_object(arg(a, 0))?;
    let ks: Vec<Value> = rt.own_keys_ex(o)?.into_iter().filter(|k| matches!(k, PropKey::Sym(_))).map(|k| rt.key_to_value(k)).collect();
    Ok(rt.new_array_values(&ks))
}

fn set_integrity(rt: &mut Runtime, v: Value, level: u8) -> Value {
    if let Value::Object(o) = v {
        let o = rt.unwrap_proxy(o);
        let ob = rt.obj_mut(o);
        ob.extensible = false;
        if level > 0 {
            ob.integrity = ob.integrity.max(level);
            for (k, p) in ob.props.iter_mut() {
                if k.is_private() {
                    continue; // private members are not properties: freezing leaves them alone
                }
                p.flags &= !CONFIGURABLE;
                if level == 2 && !p.is_accessor() {
                    p.flags &= !WRITABLE;
                }
            }
        }
    }
    v
}

fn test_integrity(rt: &mut Runtime, v: Value, level: u8) -> bool {
    let Value::Object(o) = v else { return true };
    let ob = rt.obj(rt.unwrap_proxy(o));
    if ob.extensible {
        return false;
    }
    if level == 0 {
        return true;
    }
    if let ObjKind::Array(items) = &ob.kind {
        if !items.is_empty() && ob.integrity < level {
            return false;
        }
    }
    ob.props.iter().all(|(k, p)| k.is_private() || (p.flags & CONFIGURABLE == 0 && (level < 2 || p.is_accessor() || p.flags & WRITABLE == 0)))
}

fn freeze(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(set_integrity(rt, arg(a, 0), 2))
}
fn seal(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(set_integrity(rt, arg(a, 0), 1))
}
fn prevent_extensions(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(set_integrity(rt, arg(a, 0), 0))
}
fn is_frozen(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(test_integrity(rt, arg(a, 0), 2)))
}
fn is_sealed(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(test_integrity(rt, arg(a, 0), 1)))
}
fn is_extensible(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(match arg(a, 0) {
        Value::Object(o) => rt.obj(rt.unwrap_proxy(o)).extensible,
        _ => false,
    }))
}

fn get_prototype_of(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = rt.to_object(arg(a, 0))?;
    Ok(rt.proto_of(o).map(Value::Object).unwrap_or(Value::Null))
}

fn set_prototype_of(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let v = arg(a, 0);
    let proto = match arg(a, 1) {
        Value::Object(p) => Some(p),
        Value::Null => None,
        _ => return Err(rt.type_error("Object prototype may only be an Object or null")),
    };
    if let Value::Object(o) = v {
        if !rt.set_proto_of(o, proto) {
            return Err(rt.type_error("Cannot set prototype: cyclic __proto__ value or object is not extensible"));
        }
    } else if v.is_nullish() {
        return Err(rt.type_error("Object.setPrototypeOf called on null or undefined"));
    }
    Ok(v)
}

fn has_own(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let o = rt.to_object(arg(a, 0))?;
    let k = rt.to_property_key(arg(a, 1))?;
    Ok(Value::Bool(rt.get_own_prop_ex(o, k)?.is_some()))
}

fn is(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(rt.same_value(arg(a, 0), arg(a, 1))))
}

fn proto_has_own(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let k = rt.to_property_key(arg(a, 0))?;
    let o = rt.to_object(this)?;
    Ok(Value::Bool(rt.get_own_prop_ex(o, k)?.is_some()))
}

fn proto_is_prototype_of(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let Value::Object(mut v) = arg(a, 0) else { return Ok(Value::Bool(false)) };
    let o = rt.to_object(this)?;
    while let Some(p) = rt.obj(v).proto {
        if p == o {
            return Ok(Value::Bool(true));
        }
        v = p;
    }
    Ok(Value::Bool(false))
}

fn proto_property_is_enumerable(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let k = rt.to_property_key(arg(a, 0))?;
    let o = rt.to_object(this)?;
    Ok(Value::Bool(rt.get_own_prop_ex(o, k)?.map(|p| p.flags & ENUMERABLE != 0).unwrap_or(false)))
}

pub(crate) fn object_to_string(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let tag = match this {
        Value::Undefined => "Undefined".to_string(),
        Value::Null => "Null".to_string(),
        _ => {
            let o = rt.to_object(this)?;
            let builtin = match &rt.obj(o).kind {
                ObjKind::Array(_) => "Array",
                ObjKind::Function(_) => "Function",
                ObjKind::Error => "Error",
                ObjKind::Boolean(_) => "Boolean",
                ObjKind::Number(_) => "Number",
                ObjKind::String(_) => "String",
                ObjKind::Arguments(_) => "Arguments",
                ObjKind::RegExp(_) => "RegExp",
                ObjKind::Date(_) => "Date",
                ObjKind::Proxy(p) if p.callable => "Function",
                ObjKind::Proxy(_) if rt.is_array_obj(o) => "Array",
                _ => "Object",
            };
            let t = rt.get_prop(Value::Object(o), PropKey::Sym(SYM_TO_STRING_TAG))?;
            match t {
                Value::String(s) => rt.rust_string(s),
                _ => builtin.to_string(),
            }
        }
    };
    Ok(rt.new_str(&format!("[object {}]", tag)))
}

fn proto_to_locale_string(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let f = rt.get_prop(this, PropKey::Str(A_TO_STRING))?;
    rt.call_value(f, this, &[])
}

fn proto_value_of(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Object(rt.to_object(this)?))
}

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto.unwrap_or(ObjId(0));
    let c = rt.constructor("Object", 1, object_ctor, op);
    let statics: [(&str, u32, super::NFn); 21] = [
        ("keys", 1, keys),
        ("values", 1, values),
        ("entries", 1, entries),
        ("assign", 2, assign),
        ("create", 2, create),
        ("defineProperty", 3, define_property),
        ("defineProperties", 2, define_properties),
        ("getOwnPropertyDescriptor", 2, get_own_property_descriptor),
        ("getOwnPropertyDescriptors", 1, get_own_property_descriptors),
        ("getOwnPropertyNames", 1, get_own_property_names),
        ("getOwnPropertySymbols", 1, get_own_property_symbols),
        ("freeze", 1, freeze),
        ("seal", 1, seal),
        ("preventExtensions", 1, prevent_extensions),
        ("isFrozen", 1, is_frozen),
        ("isSealed", 1, is_sealed),
        ("isExtensible", 1, is_extensible),
        ("getPrototypeOf", 1, get_prototype_of),
        ("setPrototypeOf", 2, set_prototype_of),
        ("hasOwn", 2, has_own),
        ("is", 2, is),
    ];
    for (n, ar, f) in statics {
        rt.method(c, n, ar, f);
    }
    rt.method(op, "hasOwnProperty", 1, proto_has_own);
    rt.method(op, "isPrototypeOf", 1, proto_is_prototype_of);
    rt.method(op, "propertyIsEnumerable", 1, proto_property_is_enumerable);
    rt.method(op, "toString", 0, object_to_string);
    rt.method(op, "toLocaleString", 0, proto_to_locale_string);
    rt.method(op, "valueOf", 0, proto_value_of);
}
