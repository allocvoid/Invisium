//! WeakRef and FinalizationRegistry. Neither keeps its target alive: the collector clears a
//! WeakRef whose target died and queues a registry's callback as a job (`collect_garbage`).

use super::arg;
use crate::runtime::*;
use crate::value::*;

fn weakref_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let nt = rt.native_new_target;
    if nt == Value::Undefined {
        return Err(rt.type_error("Constructor WeakRef requires 'new'"));
    }
    let Value::Object(target) = arg(a, 0) else {
        return Err(rt.type_error("WeakRef: target must be an object"));
    };
    let proto = rt.proto_from_ctor(nt, rt.intr.weakref_proto)?;
    Ok(Value::Object(rt.alloc(Object::new(proto, ObjKind::WeakRef(Some(target))))))
}

fn weakref_deref(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    match this {
        Value::Object(o) => match rt.obj(o).kind {
            ObjKind::WeakRef(t) => Ok(t.map(Value::Object).unwrap_or(Value::Undefined)),
            _ => Err(rt.type_error("WeakRef.prototype.deref called on incompatible receiver")),
        },
        _ => Err(rt.type_error("WeakRef.prototype.deref called on incompatible receiver")),
    }
}

fn registry_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let nt = rt.native_new_target;
    if nt == Value::Undefined {
        return Err(rt.type_error("Constructor FinalizationRegistry requires 'new'"));
    }
    let callback = arg(a, 0);
    if !rt.is_callable_val(callback) {
        return Err(rt.type_error("FinalizationRegistry: cleanup must be callable"));
    }
    let proto = rt.proto_from_ctor(nt, rt.intr.finreg_proto)?;
    let data = FinRegData { callback, cells: Vec::new() };
    Ok(Value::Object(rt.alloc(Object::new(proto, ObjKind::FinReg(Box::new(data))))))
}

fn registry_data(rt: &mut Runtime, this: Value) -> JsResult<&mut FinRegData> {
    let o = match this {
        Value::Object(o) if matches!(rt.obj(o).kind, ObjKind::FinReg(_)) => o,
        _ => return Err(rt.type_error("FinalizationRegistry method called on incompatible receiver")),
    };
    match &mut rt.obj_mut(o).kind {
        ObjKind::FinReg(d) => Ok(d),
        _ => Err(Exc::Throw(Value::Undefined)),
    }
}

fn registry_register(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    registry_data(rt, this)?;
    let Value::Object(target) = arg(a, 0) else {
        return Err(rt.type_error("FinalizationRegistry.prototype.register: invalid target"));
    };
    let held = arg(a, 1);
    if held == arg(a, 0) {
        return Err(rt.type_error("FinalizationRegistry.prototype.register: target and holdings must not be same"));
    }
    let token = match arg(a, 2) {
        Value::Object(t) => Some(t),
        Value::Undefined => None,
        _ => return Err(rt.type_error("FinalizationRegistry.prototype.register: invalid unregister token")),
    };
    registry_data(rt, this)?.cells.push(FinCell { target, held, token });
    rt.grow(40);
    Ok(Value::Undefined)
}

fn registry_unregister(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let d = registry_data(rt, this)?;
    let before = d.cells.len();
    let Value::Object(token) = arg(a, 0) else {
        return Err(rt.type_error("FinalizationRegistry.prototype.unregister: invalid unregister token"));
    };
    let d = registry_data(rt, this)?;
    d.cells.retain(|c| c.token != Some(token));
    Ok(Value::Bool(d.cells.len() != before))
}

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto;
    let wp = rt.new_object(op);
    rt.intr.weakref_proto = Some(wp);
    rt.constructor("WeakRef", 1, weakref_ctor, wp);
    rt.method(wp, "deref", 0, weakref_deref);
    let t = rt.new_str("WeakRef");
    rt.define_value(wp, PropKey::Sym(SYM_TO_STRING_TAG), t, CONFIGURABLE);

    let fp = rt.new_object(op);
    rt.intr.finreg_proto = Some(fp);
    rt.constructor("FinalizationRegistry", 1, registry_ctor, fp);
    rt.method(fp, "register", 2, registry_register);
    rt.method(fp, "unregister", 1, registry_unregister);
    let t = rt.new_str("FinalizationRegistry");
    rt.define_value(fp, PropKey::Sym(SYM_TO_STRING_TAG), t, CONFIGURABLE);
}
