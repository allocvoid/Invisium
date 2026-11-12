//! Generator / AsyncGenerator prototypes and the async-from-sync iterator used by `for await`.

use super::arg;
use crate::promise::slot;
use crate::runtime::*;
use crate::value::*;

impl Runtime {
    /// GetIterator(v, async): `v[Symbol.asyncIterator]()`, else its sync iterator wrapped.
    pub(crate) fn get_async_iterator(&mut self, v: Value) -> JsResult<Value> {
        let m = if v.is_nullish() { Value::Undefined } else { self.get_prop(v, PropKey::Sym(SYM_ASYNC_ITERATOR))? };
        if m.is_nullish() {
            let it = self.get_iterator(v)?;
            let p = self.intr.async_from_sync_proto;
            return Ok(Value::Object(self.alloc(Object::new(p, ObjKind::AsyncFromSync(it)))));
        }
        if !self.is_callable_val(m) {
            return Err(self.type_error("Symbol.asyncIterator is not a function"));
        }
        let it = self.call_value(m, v, &[])?;
        if !matches!(it, Value::Object(_)) {
            return Err(self.type_error("Result of the Symbol.asyncIterator method is not an object"));
        }
        Ok(it)
    }

    fn gen_method(&mut self, o: ObjId, name: &str, mode: u8) {
        let proto = self.intr.function_proto;
        let f = self.alloc(Object::new(proto, ObjKind::Function(Box::new(FuncData::GenMethod(mode)))));
        self.define_value(f, PropKey::Str(A_LENGTH), Value::Number(1.0), CONFIGURABLE);
        let n = self.new_str(name);
        self.define_value(f, PropKey::Str(A_NAME), n, CONFIGURABLE);
        let k = self.key_str(name);
        self.define_value(o, k, Value::Object(f), HIDDEN);
    }
}

fn return_this(_: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(this)
}

/// slots = [done]: builds the iterator result once the step's value has been awaited.
fn wrap_step(rt: &mut Runtime, slots: &[Value], _: Value, args: &[Value]) -> JsResult<Value> {
    Ok(rt.iter_result(slot(args, 0), slot(slots, 0) == Value::Bool(true)))
}

fn sync_step(rt: &mut Runtime, this: Value, method: Atom, a: &[Value]) -> JsResult<ObjId> {
    let it = match this {
        Value::Object(o) => match rt.obj(o).kind {
            ObjKind::AsyncFromSync(it) => it,
            _ => return Err(rt.type_error("not an async-from-sync iterator")),
        },
        _ => return Err(rt.type_error("not an async-from-sync iterator")),
    };
    let f = rt.get_prop(it, PropKey::Str(method))?;
    if !rt.is_callable_val(f) {
        // the sync iterator has no `return`: finish with the given value
        let p = rt.new_promise_obj(rt.intr.promise_proto);
        let r = rt.iter_result(arg(a, 0), true);
        rt.fulfill_promise(p, r);
        return Ok(p);
    }
    let r = rt.call_value(f, it, a)?;
    if !matches!(r, Value::Object(_)) {
        return Err(rt.type_error("Iterator result is not an object"));
    }
    let done = rt.get_prop(r, PropKey::Str(A_DONE))?;
    let done = rt.truthy(done);
    let value = rt.get_prop(r, PropKey::Str(A_VALUE))?;
    let p = rt.promise_resolve_value(value)?;
    let wrap = rt.new_intern("", 1, wrap_step, vec![Value::Bool(done)]);
    let derived = rt.new_promise_obj(rt.intr.promise_proto);
    rt.promise_then(p, Reaction::Then { on_ok: Value::Object(wrap), on_err: Value::Undefined, derived: Some(derived) });
    Ok(derived)
}

/// Runs a step of the wrapped sync iterator; failures become a rejected promise.
fn async_from_sync(rt: &mut Runtime, this: Value, method: Atom, a: &[Value]) -> JsResult<Value> {
    match sync_step(rt, this, method, a) {
        Ok(p) => Ok(Value::Object(p)),
        Err(Exc::Throw(e)) => {
            let p = rt.new_promise_obj(rt.intr.promise_proto);
            rt.reject_promise(p, e);
            Ok(Value::Object(p))
        }
        Err(abort) => Err(abort),
    }
}

fn afs_next(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    async_from_sync(rt, this, A_NEXT, a)
}

fn afs_return(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    async_from_sync(rt, this, A_RETURN, a)
}

pub(super) fn setup(rt: &mut Runtime) {
    let ip = rt.intr.iterator_proto;
    let gp = rt.new_object(ip);
    rt.intr.generator_proto = Some(gp);
    for (name, mode) in [("next", 0), ("throw", 1), ("return", 2)] {
        rt.gen_method(gp, name, mode);
    }
    let tag = rt.new_str("Generator");
    rt.define_value(gp, PropKey::Sym(SYM_TO_STRING_TAG), tag, CONFIGURABLE);

    let op = rt.intr.object_proto;
    let agp = rt.new_object(op);
    rt.intr.async_generator_proto = Some(agp);
    for (name, mode) in [("next", 3), ("throw", 4), ("return", 5)] {
        rt.gen_method(agp, name, mode);
    }
    rt.method_sym(agp, SYM_ASYNC_ITERATOR, "[Symbol.asyncIterator]", 0, return_this);
    let tag = rt.new_str("AsyncGenerator");
    rt.define_value(agp, PropKey::Sym(SYM_TO_STRING_TAG), tag, CONFIGURABLE);

    let afs = rt.new_object(op);
    rt.intr.async_from_sync_proto = Some(afs);
    rt.method(afs, "next", 1, afs_next);
    rt.method(afs, "return", 1, afs_return);
    rt.method_sym(afs, SYM_ASYNC_ITERATOR, "[Symbol.asyncIterator]", 0, return_this);

    let marker = rt.new_object(None);
    rt.intr.gen_return_marker = Some(marker);
}
