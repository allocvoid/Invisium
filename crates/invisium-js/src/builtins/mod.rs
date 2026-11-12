//! Standard built-in objects.

mod array;
mod array_fn;
mod bigint;
mod buffer;
mod clone;
mod collections;
mod compat;
mod data_view;
mod date;
mod date_time;
mod error;
mod extras;
mod function;
mod generator;
mod globals;
pub(crate) mod intl;
mod json;
mod json_stringify;
mod m3_misc;
mod m3_text;
mod math;
mod normalize;
mod normalize_data;
mod number_math;
mod object;
mod promise;
mod reflect;
mod regexp;
mod regexp_replace;
mod regexp_sym;
mod species;
mod string;
mod string_split;
mod symbol_iter;
mod typed_array;
mod weak;
mod weakref;

use crate::runtime::*;
use crate::value::*;
use std::rc::Rc;

pub(crate) use object::define_desc;

pub(crate) type NFn = fn(&mut Runtime, Value, &[Value]) -> JsResult<Value>;

pub(crate) fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).copied().unwrap_or(Value::Undefined)
}

impl Runtime {
    pub(crate) fn exc_to_thrown(&mut self, e: Exc) -> crate::Thrown {
        match e {
            Exc::Throw(v) => crate::Thrown(v),
            Exc::Abort(k) => {
                self.pending_abort = Some(k);
                crate::Thrown(Value::Undefined)
            }
        }
    }

    pub(crate) fn wrap(f: NFn) -> NativeFn {
        Rc::new(move |rt: &mut Runtime, this: Value, args: &[Value]| f(rt, this, args).map_err(|e| rt.exc_to_thrown(e)))
    }

    pub(crate) fn native_fn(&mut self, name: &str, arity: u32, f: NFn) -> ObjId {
        self.new_native(name, arity, Self::wrap(f), false)
    }

    /// Defines a non-enumerable method on `o`.
    pub(crate) fn method(&mut self, o: ObjId, name: &str, arity: u32, f: NFn) -> ObjId {
        let func = self.native_fn(name, arity, f);
        let k = self.key_str(name);
        self.define_value(o, k, Value::Object(func), HIDDEN);
        func
    }

    pub(crate) fn method_sym(&mut self, o: ObjId, sym: SymId, name: &str, arity: u32, f: NFn) -> ObjId {
        let func = self.native_fn(name, arity, f);
        self.define_value(o, PropKey::Sym(sym), Value::Object(func), HIDDEN);
        func
    }

    pub(crate) fn getter(&mut self, o: ObjId, name: &str, f: NFn) {
        let func = self.native_fn(&format!("get {}", name), 0, f);
        let k = self.key_str(name);
        self.define_own(o, k, Prop { value: Value::Object(func), setter: Value::Undefined, flags: ACCESSOR | CONFIGURABLE });
    }

    /// Creates a constructor bound to the global object, linked with `proto`.
    pub(crate) fn constructor(&mut self, name: &str, arity: u32, f: NFn, proto: ObjId) -> ObjId {
        let c = self.new_native(name, arity, Self::wrap(f), true);
        self.define_value(c, PropKey::Str(A_PROTOTYPE), Value::Object(proto), 0);
        self.define_value(proto, PropKey::Str(A_CONSTRUCTOR), Value::Object(c), HIDDEN);
        let g = self.global;
        let k = self.key_str(name);
        self.define_value(g, k, Value::Object(c), HIDDEN);
        c
    }

    pub(crate) fn set_global(&mut self, name: &str, v: Value) {
        let g = self.global;
        let k = self.key_str(name);
        self.define_value(g, k, v, HIDDEN);
    }

    pub(crate) fn callable_arg(&mut self, v: Value, what: &str) -> JsResult<Value> {
        if !self.is_callable_val(v) {
            let d = self.describe(v);
            return Err(self.type_error(&format!("{} is not a function ({})", d, what)));
        }
        Ok(v)
    }

    /// Relative index helper used by slice/splice/at etc.
    pub(crate) fn rel_index(&mut self, v: Value, len: f64, default: f64) -> JsResult<f64> {
        if v == Value::Undefined {
            return Ok(default);
        }
        let n = crate::ops::to_integer(self.to_num(v)?);
        Ok(if n < 0.0 { (len + n).max(0.0) } else { n.min(len) })
    }
}

pub(crate) fn setup(rt: &mut Runtime) {
    let object_proto = rt.new_object(None);
    rt.intr.object_proto = Some(object_proto);
    let fp = rt.alloc(Object::new(
        Some(object_proto),
        ObjKind::Function(Box::new(FuncData::Native { f: Runtime::wrap(|_, _, _| Ok(Value::Undefined)), ctor: false })),
    ));
    rt.intr.function_proto = Some(fp);
    rt.global = rt.new_object(Some(object_proto));
    let g = rt.global;
    rt.set_global("globalThis", Value::Object(g));
    rt.define_value(g, PropKey::Str(A_UNDEFINED), Value::Undefined, 0);
    let nan = rt.key_str("NaN");
    rt.define_value(g, nan, Value::Number(f64::NAN), 0);
    let inf = rt.key_str("Infinity");
    rt.define_value(g, inf, Value::Number(f64::INFINITY), 0);

    object::setup(rt);
    function::setup(rt);
    symbol_iter::setup(rt);
    error::setup(rt);
    array::setup(rt);
    string::setup(rt);
    number_math::setup(rt);
    json::setup(rt);
    globals::setup(rt);
    promise::setup(rt);
    generator::setup(rt);
    collections::setup(rt);
    weak::setup(rt);
    reflect::setup(rt);
    extras::setup(rt);
    regexp::setup(rt);
    date::setup(rt);
    buffer::setup(rt);
    typed_array::setup(rt);
    data_view::setup(rt);
    clone::setup(rt);
    bigint::setup(rt);
    m3_text::setup(rt);
    m3_misc::setup(rt);
    weakref::setup(rt);
    species::setup(rt);
    compat::setup(rt);
    intl::setup(rt);
    crate::module_eval::setup(rt);
}
