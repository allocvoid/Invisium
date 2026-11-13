//! Embedding additions (Milestone 2a, step 0): host wrappers with a prototype, property
//! definition with explicit attributes, construction from Rust and the embedder data slot.

use crate::runtime::*;
use crate::value::*;
use crate::{ClassId, JsError};
use std::rc::Rc;

impl Runtime {
    /// Registers a host class without get/set hooks: property access on its wrappers is plain
    /// own-property / prototype-chain lookup (no callback, no key string built).
    pub fn register_plain_host_class(&mut self, name: &'static str) -> ClassId {
        self.host_classes.push((name, None));
        ClassId(self.host_classes.len() as u32 - 1)
    }

    /// The hooks of a host class, or None for plain (hook-less) and unknown classes.
    pub(crate) fn host_hooks(&self, class: u32) -> Option<crate::HostClass> {
        self.host_classes.get(class as usize).and_then(|c| c.1)
    }

    /// Like `host_object`, but a wrapper created by this call gets `proto` as its prototype.
    /// An existing wrapper for (class, id) is returned unchanged.
    pub fn host_object_with_proto(&mut self, class: ClassId, id: u64, proto: Value) -> Value {
        if let Some(o) = self.host_objs.get(&(class.0, id)) {
            return Value::Object(*o);
        }
        let p = match proto {
            Value::Object(p) => Some(p),
            Value::Null => None,
            _ => self.intr.object_proto,
        };
        let o = self.alloc(Object::new(p, ObjKind::Host(class.0, id)));
        self.host_objs.insert((class.0, id), o);
        Value::Object(o)
    }

    fn define_from_host(&mut self, obj: Value, key: &str, p: Prop) -> JsResult<()> {
        let Value::Object(o) = obj else {
            return Err(self.type_error("Cannot define a property on a non-object"));
        };
        let k = self.key_str(key);
        if !self.define_own(o, k, p) {
            return Err(self.type_error(&format!("Cannot redefine property: {}", key)));
        }
        Ok(())
    }

    /// Define or replace an accessor property (configurable). `None` = no getter / no setter.
    pub fn define_accessor(&mut self, obj: Value, key: &str, getter: Option<Value>, setter: Option<Value>, enumerable: bool) -> Result<(), JsError> {
        let mark = self.host_enter_with(&[obj, getter.unwrap_or(Value::Undefined), setter.unwrap_or(Value::Undefined)], &[]);
        let mut r = Ok(());
        for f in [getter, setter].into_iter().flatten() {
            if f != Value::Undefined && !self.is_callable_val(f) {
                r = Err(self.type_error("Getter/setter must be a function"));
            }
        }
        let flags = ACCESSOR | CONFIGURABLE | if enumerable { ENUMERABLE } else { 0 };
        let p = Prop { value: getter.unwrap_or(Value::Undefined), setter: setter.unwrap_or(Value::Undefined), flags };
        let r = r.and_then(|_| self.define_from_host(obj, key, p));
        self.finish_with(r, mark)
    }

    /// Define or replace a data property with explicit attributes.
    pub fn define_property(&mut self, obj: Value, key: &str, value: Value, writable: bool, enumerable: bool, configurable: bool) -> Result<(), JsError> {
        let mark = self.host_enter_with(&[obj, value], &[]);
        let flags = if writable { WRITABLE } else { 0 } | if enumerable { ENUMERABLE } else { 0 } | if configurable { CONFIGURABLE } else { 0 };
        let r = self.define_from_host(obj, key, Prop::data(value, flags));
        self.finish_with(r, mark)
    }

    /// `new ctor(...args)`.
    pub fn construct(&mut self, ctor: Value, args: &[Value]) -> Result<Value, JsError> {
        let mark = self.host_enter_with(&[ctor], args);
        let r = self.construct_value(ctor, args, ctor);
        let r = self.finish_with(r, mark);
        if let Ok(v) = r {
            self.keep_for_native(v);
        }
        r
    }

    /// Drains the promise job queue. An unhandled rejection is not an error here; a step-budget
    /// or out-of-memory abort is, and drops the jobs that were still queued.
    pub fn run_jobs(&mut self) -> Result<(), JsError> {
        self.host_enter();
        let r = self.run_jobs_internal();
        self.finish(r)
    }

    /// True if `run_jobs` has work to do.
    pub fn has_pending_jobs(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// A pending promise and its resolving functions: (promise, resolve, reject).
    pub fn new_promise(&mut self) -> (Value, Value, Value) {
        let p = self.new_promise_obj(self.intr.promise_proto);
        let (res, rej) = self.resolving_functions(p);
        (Value::Object(p), res, rej)
    }

    /// Messages of the most recent rejections nobody handled (recorded by `run_jobs`).
    pub fn take_unhandled_rejections(&mut self) -> Vec<String> {
        std::mem::take(&mut self.unhandled)
    }

    /// One opaque slot for the embedder; natives and host-class hooks read it back.
    pub fn set_host_data(&mut self, data: Rc<dyn std::any::Any>) {
        self.host_data = Some(data);
    }

    pub fn host_data(&self) -> Option<Rc<dyn std::any::Any>> {
        self.host_data.clone()
    }

    /// How much native stack script may use below the point where the embedder entered the
    /// runtime, before the next native call raises `RangeError` (stack overflow).
    /// The default is 640 KB. The embedder guarantees that much stack really exists.
    pub fn set_native_stack_budget(&mut self, bytes: usize) {
        self.native_stack_budget = bytes;
    }
}
