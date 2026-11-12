//! Symbol and the built-in iterator prototypes.

use super::arg;
use crate::runtime::*;
use crate::value::*;
use std::rc::Rc;

impl Runtime {
    pub(crate) fn iter_result(&mut self, value: Value, done: bool) -> Value {
        let o = self.plain_object();
        self.define_value(o, PropKey::Str(A_VALUE), value, DEFAULT_FLAGS);
        self.define_value(o, PropKey::Str(A_DONE), Value::Bool(done), DEFAULT_FLAGS);
        Value::Object(o)
    }

    pub(crate) fn new_symbol(&mut self, desc: Option<Rc<[u16]>>) -> SymId {
        self.permanent_bytes += 32 + desc.as_ref().map(|d| d.len() * 2).unwrap_or(0);
        self.syms.push(desc);
        SymId(self.syms.len() as u32 - 1)
    }
}

fn symbol_fn(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    if rt.native_new_target != Value::Undefined {
        return Err(rt.type_error("Symbol is not a constructor"));
    }
    let d = match arg(a, 0) {
        Value::Undefined => None,
        v => Some(rt.to_units(v)?),
    };
    Ok(Value::Symbol(rt.new_symbol(d)))
}

fn symbol_for(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let k = rt.to_units(arg(a, 0))?;
    if let Some(s) = rt.sym_registry.get(&k) {
        return Ok(Value::Symbol(*s));
    }
    let s = rt.new_symbol(Some(k.clone()));
    rt.sym_registry.insert(k, s);
    Ok(Value::Symbol(s))
}

fn symbol_key_for(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let Value::Symbol(s) = arg(a, 0) else { return Err(rt.type_error("not a symbol")) };
    let found = rt.sym_registry.iter().find(|(_, v)| **v == s).map(|(k, _)| k.clone());
    Ok(match found {
        Some(k) => rt.new_str16(k.to_vec()),
        None => Value::Undefined,
    })
}

fn this_symbol(rt: &mut Runtime, this: Value) -> JsResult<SymId> {
    match this {
        Value::Symbol(s) => Ok(s),
        Value::Object(o) => match rt.obj(o).kind {
            ObjKind::Symbol(s) => Ok(s),
            _ => Err(rt.type_error("not a symbol")),
        },
        _ => Err(rt.type_error("not a symbol")),
    }
}

fn symbol_to_string(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let s = this_symbol(rt, this)?;
    let d = rt.syms.get(s.0 as usize).cloned().flatten().map(|d| String::from_utf16_lossy(&d)).unwrap_or_default();
    Ok(rt.new_str(&format!("Symbol({})", d)))
}

fn symbol_value_of(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Symbol(this_symbol(rt, this)?))
}

fn symbol_description(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let s = this_symbol(rt, this)?;
    Ok(match rt.syms.get(s.0 as usize).cloned().flatten() {
        Some(d) => rt.new_str16(d.to_vec()),
        None => Value::Undefined,
    })
}

fn iterator_self(_: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(this)
}

fn builtin_iter_next(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let ok = matches!(this, Value::Object(o) if matches!(rt.obj(o).kind, ObjKind::ArrayIter { .. } | ObjKind::StringIter { .. }));
    if !ok {
        return Err(rt.type_error("next method called on incompatible receiver"));
    }
    match rt.iter_step(this)? {
        Some(v) => Ok(rt.iter_result(v, false)),
        None => Ok(rt.iter_result(Value::Undefined, true)),
    }
}

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto;
    let sp = rt.new_object(op);
    rt.intr.symbol_proto = Some(sp);
    let c = rt.constructor("Symbol", 0, symbol_fn, sp);
    rt.method(c, "for", 1, symbol_for);
    rt.method(c, "keyFor", 1, symbol_key_for);
    for (i, name) in WELL_KNOWN_SYMBOLS.iter().enumerate() {
        let k = rt.key_str(name);
        rt.define_value(c, k, Value::Symbol(SymId(i as u32)), 0);
    }
    rt.method(sp, "toString", 0, symbol_to_string);
    rt.method(sp, "valueOf", 0, symbol_value_of);
    rt.getter(sp, "description", symbol_description);
    let tag = rt.new_str("Symbol");
    rt.define_value(sp, PropKey::Sym(SYM_TO_STRING_TAG), tag, CONFIGURABLE);

    let ip = rt.new_object(op);
    rt.intr.iterator_proto = Some(ip);
    rt.method_sym(ip, SYM_ITERATOR, "[Symbol.iterator]", 0, iterator_self);
    let aip = rt.new_object(Some(ip));
    rt.intr.array_iter_proto = Some(aip);
    rt.method(aip, "next", 0, builtin_iter_next);
    let t = rt.new_str("Array Iterator");
    rt.define_value(aip, PropKey::Sym(SYM_TO_STRING_TAG), t, CONFIGURABLE);
    let sip = rt.new_object(Some(ip));
    rt.intr.string_iter_proto = Some(sip);
    rt.method(sip, "next", 0, builtin_iter_next);
    let t = rt.new_str("String Iterator");
    rt.define_value(sip, PropKey::Sym(SYM_TO_STRING_TAG), t, CONFIGURABLE);
}
