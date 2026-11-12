//! Promise, AggregateError and queueMicrotask.

use super::arg;
use super::species::Capability;
use crate::promise::slot;
use crate::runtime::*;
use crate::value::*;

fn this_promise(rt: &mut Runtime, this: Value) -> JsResult<ObjId> {
    match rt.as_promise(this) {
        Some(p) => Ok(p),
        None => Err(rt.type_error("Method Promise.prototype.then called on incompatible receiver")),
    }
}

fn promise_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let nt = rt.native_new_target;
    if nt == Value::Undefined {
        return Err(rt.type_error("Promise constructor cannot be invoked without 'new'"));
    }
    let exec = arg(a, 0);
    if !rt.is_callable_val(exec) {
        return Err(rt.type_error("Promise resolver is not a function"));
    }
    let proto = rt.proto_from_ctor(nt, rt.intr.promise_proto)?;
    let p = rt.new_promise_obj(proto);
    let (res, rej) = rt.resolving_functions(p);
    match rt.call_value(exec, Value::Undefined, &[res, rej]) {
        Ok(_) => {}
        Err(Exc::Throw(e)) => {
            rt.call_value(rej, Value::Undefined, &[e])?;
        }
        Err(abort) => return Err(abort),
    }
    Ok(Value::Object(p))
}

fn then(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let p = this_promise(rt, this)?;
    let cap = rt.promise_species(this)?;
    rt.promise_then(p, cap.reaction(arg(a, 0), arg(a, 1)));
    Ok(cap.promise())
}

fn catch(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let then = rt.get_prop(this, PropKey::Str(A_THEN))?;
    rt.call_value(then, this, &[Value::Undefined, arg(a, 0)])
}

fn value_thunk(_: &mut Runtime, slots: &[Value], _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(slot(slots, 0))
}

fn thrower(_: &mut Runtime, slots: &[Value], _: Value, _: &[Value]) -> JsResult<Value> {
    Err(Exc::Throw(slot(slots, 0)))
}

/// `finally` handlers: slots = [onFinally, rethrow?, species constructor]; run onFinally, then
/// pass the outcome on.
fn finally_step(rt: &mut Runtime, slots: &[Value], _: Value, args: &[Value]) -> JsResult<Value> {
    let r = rt.call_value(slot(slots, 0), Value::Undefined, &[])?;
    let c = slot(slots, 2);
    let rethrow = slot(slots, 1) == Value::Bool(true);
    let pass = rt.new_intern("", 0, if rethrow { thrower } else { value_thunk }, vec![slot(args, 0)]);
    if rt.is_intrinsic_promise_ctor(c) {
        let p = rt.promise_resolve_value(r)?;
        let derived = rt.new_promise_obj(rt.intr.promise_proto);
        rt.promise_then(p, Reaction::Then { on_ok: Value::Object(pass), on_err: Value::Undefined, derived: Some(derived) });
        return Ok(Value::Object(derived));
    }
    let p = rt.promise_resolve_with(c, r)?;
    let then = rt.get_prop(p, PropKey::Str(A_THEN))?;
    rt.call_value(then, p, &[Value::Object(pass)])
}

fn finally(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    if !matches!(this, Value::Object(_)) {
        return Err(rt.type_error("Promise.prototype.finally called on a non-object"));
    }
    let c = rt.species_ctor(this, rt.intr.promise_ctor)?.unwrap_or(Value::Undefined);
    let f = arg(a, 0);
    let then = rt.get_prop(this, PropKey::Str(A_THEN))?;
    if !rt.is_callable_val(f) {
        return rt.call_value(then, this, &[f, f]);
    }
    let ok = rt.new_intern("", 1, finally_step, vec![f, Value::Bool(false), c]);
    let err = rt.new_intern("", 1, finally_step, vec![f, Value::Bool(true), c]);
    rt.call_value(then, this, &[Value::Object(ok), Value::Object(err)])
}

impl Runtime {
    /// PromiseResolve(C, x) for any constructor C.
    pub(crate) fn promise_resolve_with(&mut self, c: Value, x: Value) -> JsResult<Value> {
        if self.is_intrinsic_promise_ctor(c) {
            return Ok(Value::Object(self.promise_resolve_value(x)?));
        }
        if self.as_promise(x).is_some() && self.get_prop(x, PropKey::Str(A_CONSTRUCTOR))? == c {
            return Ok(x);
        }
        let cap = self.new_promise_capability(c)?;
        self.cap_settle(&cap, false, x)?;
        Ok(cap.promise())
    }
}

fn resolve(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    rt.promise_resolve_with(this, arg(a, 0))
}

fn reject(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    if !rt.is_intrinsic_promise_ctor(this) {
        let cap = rt.new_promise_capability(this)?;
        rt.cap_settle(&cap, true, arg(a, 0))?;
        return Ok(cap.promise());
    }
    let p = rt.new_promise_obj(rt.intr.promise_proto);
    rt.reject_promise(p, arg(a, 0));
    Ok(Value::Object(p))
}

impl Runtime {
    pub(crate) fn make_aggregate_error(&mut self, errors: Value, msg: &str) -> Value {
        let proto = self.intr.aggregate_error_proto;
        let o = self.alloc(Object::new(proto, ObjKind::Error));
        let m = self.new_str(msg);
        self.define_value(o, PropKey::Str(A_MESSAGE), m, HIDDEN);
        let s = self.new_str(&format!("AggregateError: {}", msg));
        self.define_value(o, PropKey::Str(A_STACK), s, HIDDEN);
        let k = self.key_str("errors");
        self.define_value(o, k, errors, HIDDEN);
        Value::Object(o)
    }
}

fn aggregate_error_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let items = rt.iterate_to_vec(arg(a, 0))?;
    let errors = rt.new_array_values(&items);
    let msg = match arg(a, 1) {
        Value::Undefined => String::new(),
        m => rt.to_rust_string(m)?,
    };
    let e = rt.make_aggregate_error(errors, &msg);
    let nt = rt.native_new_target;
    if let (Value::Object(o), Some(p)) = (e, rt.proto_from_ctor(nt, rt.intr.aggregate_error_proto)?) {
        rt.obj_mut(o).proto = Some(p);
    }
    Ok(e)
}

/// Per-element handler of all / allSettled / any.
/// slots = [values, index, remaining, settle function, kind].
fn combinator_element(rt: &mut Runtime, slots: &[Value], _: Value, args: &[Value]) -> JsResult<Value> {
    let (Value::Object(values), Value::Number(index), Value::Object(remaining), Value::Number(kind)) = (slot(slots, 0), slot(slots, 1), slot(slots, 2), slot(slots, 4)) else {
        return Ok(Value::Undefined);
    };
    let v = slot(args, 0);
    let stored = match kind as u8 {
        1 | 2 => {
            let o = rt.plain_object();
            let (status, field) = if kind as u8 == 1 { ("fulfilled", "value") } else { ("rejected", "reason") };
            let sv = rt.new_str(status);
            let sk = rt.key_str("status");
            rt.define_value(o, sk, sv, DEFAULT_FLAGS);
            let fk = rt.key_str(field);
            rt.define_value(o, fk, v, DEFAULT_FLAGS);
            Value::Object(o)
        }
        _ => v,
    };
    rt.array_set_index(values, index as u32, stored)?;
    let left = match rt.cell_get(remaining) {
        Value::Number(n) => n - 1.0,
        _ => 0.0,
    };
    rt.cell_set(remaining, Value::Number(left));
    if left == 0.0 {
        let out = if kind as u8 == 3 { rt.make_aggregate_error(Value::Object(values), "All promises were rejected") } else { Value::Object(values) };
        rt.call_value(slot(slots, 3), Value::Undefined, &[out])?;
    }
    Ok(Value::Undefined)
}

/// mode: 0 = all, 1 = allSettled, 2 = any, 3 = race.
/// `ctor` is the foreign `this` of the static (its `resolve` and the items' `then` are used
/// generically), or None for the intrinsic Promise.
fn combine(rt: &mut Runtime, iterable: Value, mode: u8, res: Value, rej: Value, ctor: Option<Value>) -> JsResult<()> {
    let ctor_resolve = match ctor {
        Some(c) => {
            let k = rt.key_str("resolve");
            let f = rt.get_prop(c, k)?;
            if !rt.is_callable_val(f) {
                return Err(rt.type_error("Promise resolve is not a function"));
            }
            Some((c, f))
        }
        None => None,
    };
    let items = rt.iterate_to_vec(iterable)?;
    // the items may be reachable only from this list while `then` lookups run script
    rt.scratch.extend_from_slice(&items);
    let values = rt.new_array(vec![Some(Value::Undefined); items.len()]);
    if items.is_empty() && mode != 3 {
        let (f, out) = if mode == 2 { (rej, rt.make_aggregate_error(Value::Object(values), "All promises were rejected")) } else { (res, Value::Object(values)) };
        rt.call_value(f, Value::Undefined, &[out])?;
        return Ok(());
    }
    let remaining = rt.new_cell(Value::Number(items.len() as f64));
    for (i, item) in items.into_iter().enumerate() {
        let element = |rt: &mut Runtime, settle: Value, kind: u8| {
            let slots = vec![Value::Object(values), Value::Number(i as f64), Value::Object(remaining), settle, Value::Number(kind as f64)];
            Value::Object(rt.new_intern("", 1, combinator_element, slots))
        };
        let (on_ok, on_err) = match mode {
            0 => (element(rt, res, 0), rej),
            1 => (element(rt, res, 1), element(rt, res, 2)),
            2 => (res, element(rt, rej, 3)),
            _ => (res, rej),
        };
        match ctor_resolve {
            None => {
                let p = rt.promise_resolve_value(item)?;
                rt.promise_then(p, Reaction::Then { on_ok, on_err, derived: None });
            }
            Some((c, resolve)) => {
                let next = rt.call_value(resolve, c, &[item])?;
                rt.scratch.push(next);
                let then = rt.get_prop(next, PropKey::Str(A_THEN))?;
                rt.call_value(then, next, &[on_ok, on_err])?;
            }
        }
    }
    Ok(())
}

fn combinator(rt: &mut Runtime, this: Value, a: &[Value], mode: u8) -> JsResult<Value> {
    let (cap, ctor) = if rt.is_intrinsic_promise_ctor(this) {
        (Capability::Native(rt.new_promise_obj(rt.intr.promise_proto)), None)
    } else {
        (rt.new_promise_capability(this)?, Some(this))
    };
    let (res, rej) = rt.cap_functions(&cap);
    match combine(rt, arg(a, 0), mode, res, rej, ctor) {
        Ok(()) => {}
        Err(Exc::Throw(e)) => {
            rt.call_value(rej, Value::Undefined, &[e])?;
        }
        Err(abort) => return Err(abort),
    }
    Ok(cap.promise())
}

fn all(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    combinator(rt, this, a, 0)
}
fn all_settled(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    combinator(rt, this, a, 1)
}
fn any(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    combinator(rt, this, a, 2)
}
fn race(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    combinator(rt, this, a, 3)
}

fn queue_microtask(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let f = rt.callable_arg(arg(a, 0), "queueMicrotask")?;
    rt.enqueue_job(Job::Call(f));
    Ok(Value::Undefined)
}

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto;
    let pp = rt.new_object(op);
    rt.intr.promise_proto = Some(pp);
    let c = rt.constructor("Promise", 1, promise_ctor, pp);
    rt.method(pp, "then", 2, then);
    rt.method(pp, "catch", 1, catch);
    rt.method(pp, "finally", 1, finally);
    let tag = rt.new_str("Promise");
    rt.define_value(pp, PropKey::Sym(SYM_TO_STRING_TAG), tag, CONFIGURABLE);
    let statics: [(&str, u32, super::NFn); 6] =
        [("resolve", 1, resolve), ("reject", 1, reject), ("all", 1, all), ("allSettled", 1, all_settled), ("any", 1, any), ("race", 1, race)];
    for (n, ar, f) in statics {
        rt.method(c, n, ar, f);
    }

    let ep = rt.intr.error_proto;
    let ap = rt.new_object(ep);
    rt.intr.aggregate_error_proto = Some(ap);
    let ac = rt.constructor("AggregateError", 2, aggregate_error_ctor, ap);
    let ek = rt.key_str("Error");
    let error_ctor = rt.get_own_prop(rt.global, ek).and_then(|p| p.value.as_obj());
    rt.obj_mut(ac).proto = error_ctor.or(rt.intr.function_proto);
    let nv = rt.new_str("AggregateError");
    rt.define_value(ap, PropKey::Str(A_NAME), nv, HIDDEN);
    let empty = rt.atom_value(A_EMPTY);
    rt.define_value(ap, PropKey::Str(A_MESSAGE), empty, HIDDEN);

    let qm = rt.native_fn("queueMicrotask", 1, queue_microtask);
    rt.set_global("queueMicrotask", Value::Object(qm));
}
