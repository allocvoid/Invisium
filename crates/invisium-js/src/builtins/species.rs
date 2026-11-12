//! Symbol.species: the accessor on the built-in constructors, and the lookups used by the
//! Array, typed-array and Promise methods that create new instances.

use crate::runtime::*;
use crate::value::*;

fn species_getter(_: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(this)
}

impl Runtime {
    /// True while `c` still has the built-in `Symbol.species` accessor (the common case, which
    /// then needs no property lookups or calls).
    fn species_pristine(&self, c: ObjId) -> bool {
        let getter = self.intr.species_getter;
        getter.is_some() && matches!(self.obj(c).props.get(PropKey::Sym(SYM_SPECIES)), Some(p) if p.is_accessor() && p.value.as_obj() == getter)
    }

    /// SpeciesConstructor(o, default). `None` means: create the default kind of object.
    pub(crate) fn species_ctor(&mut self, o: Value, default: Option<ObjId>) -> JsResult<Option<Value>> {
        let c = self.get_prop(o, PropKey::Str(A_CONSTRUCTOR))?;
        if c == Value::Undefined {
            return Ok(None);
        }
        let Value::Object(co) = c else {
            return Err(self.type_error("object.constructor is not an object"));
        };
        if Some(co) == default && self.species_pristine(co) {
            return Ok(None);
        }
        let s = self.get_prop(c, PropKey::Sym(SYM_SPECIES))?;
        if s.is_nullish() || (s.as_obj().is_some() && s.as_obj() == default) {
            return Ok(None);
        }
        if !self.is_constructor(s) {
            return Err(self.type_error("object.constructor[Symbol.species] is not a constructor"));
        }
        Ok(Some(s))
    }

    /// ArraySpeciesCreate(o, len): `Some(new object)` when `o` is an array whose species is not
    /// the built-in Array; `None` when the caller should create a plain array.
    pub(crate) fn array_species(&mut self, o: Value, len: u64) -> JsResult<Option<Value>> {
        let Value::Object(oid) = o else { return Ok(None) };
        if !self.is_array_obj(oid) {
            return Ok(None);
        }
        match self.species_ctor(o, self.intr.array_ctor)? {
            None => Ok(None),
            Some(c) => Ok(Some(self.construct_value(c, &[Value::Number(len as f64)], c)?)),
        }
    }

    /// TypedArraySpeciesCreate: `Some(typed array)` built by a non-default species constructor
    /// from `args`; `min_len` is the element count the caller is going to write.
    pub(crate) fn typed_species(&mut self, this: Value, kind: TypedKind, args: &[Value], min_len: usize) -> JsResult<Option<ObjId>> {
        let default = self.intr.typed_ctors[kind as usize];
        let Some(c) = self.species_ctor(this, default)? else { return Ok(None) };
        let r = self.construct_value(c, args, c)?;
        match (r, self.typed_view(r)) {
            (Value::Object(o), Some(v)) if v.length >= min_len => Ok(Some(o)),
            (_, Some(_)) => Err(self.type_error("The typed array created by the species constructor is too short")),
            _ => Err(self.type_error("The species constructor did not create a typed array")),
        }
    }

    /// The capability `then` uses: a plain promise, or NewPromiseCapability of the receiver's
    /// species constructor (any constructor: a subclass or a userland promise class).
    pub(crate) fn promise_species(&mut self, this: Value) -> JsResult<Capability> {
        match self.species_ctor(this, self.intr.promise_ctor)? {
            None => Ok(Capability::Native(self.new_promise_obj(self.intr.promise_proto))),
            Some(c) => self.new_promise_capability(c),
        }
    }

    /// True when `c` should be treated as the intrinsic `Promise` (it is, or it cannot construct).
    pub(crate) fn is_intrinsic_promise_ctor(&self, c: Value) -> bool {
        c.as_obj().is_none() || c.as_obj() == self.intr.promise_ctor || !self.is_constructor(c)
    }

    /// NewPromiseCapability(C): `new C(executor)`; the executor must have been called with a
    /// callable resolve and reject. The object C returns is used as is.
    pub(crate) fn new_promise_capability(&mut self, c: Value) -> JsResult<Capability> {
        if !self.is_constructor(c) {
            return Err(self.type_error("Promise capability constructor is not a constructor"));
        }
        if c.as_obj() == self.intr.promise_ctor {
            return Ok(Capability::Native(self.new_promise_obj(self.intr.promise_proto)));
        }
        let cap = self.new_array(vec![Some(Value::Undefined), Some(Value::Undefined)]);
        let exec = self.new_intern("", 2, capability_executor, vec![Value::Object(cap)]);
        let promise = self.construct_value(c, &[Value::Object(exec)], c)?;
        if !self.is_callable_val(self.cap_get(cap, 0)) {
            return Err(self.type_error("Promise resolve function is not callable"));
        }
        if !self.is_callable_val(self.cap_get(cap, 1)) {
            return Err(self.type_error("Promise reject function is not callable"));
        }
        Ok(Capability::Foreign { promise, cap })
    }

    pub(crate) fn cap_get(&self, cap: ObjId, i: usize) -> Value {
        match &self.obj(cap).kind {
            ObjKind::Array(items) => items.get(i).copied().flatten().unwrap_or(Value::Undefined),
            _ => Value::Undefined,
        }
    }

    fn cap_set(&mut self, cap: ObjId, i: usize, v: Value) {
        if let ObjKind::Array(items) = &mut self.obj_mut(cap).kind {
            if let Some(s) = items.get_mut(i) {
                *s = Some(v);
            }
        }
    }

    /// Settles a capability: `rejected` picks reject over resolve.
    pub(crate) fn cap_settle(&mut self, cap: &Capability, rejected: bool, v: Value) -> JsResult<()> {
        match *cap {
            Capability::Native(p) if rejected => {
                self.reject_promise(p, v);
                Ok(())
            }
            Capability::Native(p) => self.resolve_promise(p, v),
            Capability::Foreign { cap, .. } => {
                let f = self.cap_get(cap, if rejected { 1 } else { 0 });
                self.call_value(f, Value::Undefined, &[v]).map(|_| ())
            }
        }
    }

    /// The resolve and reject functions of a capability (created on demand for a native one).
    pub(crate) fn cap_functions(&mut self, cap: &Capability) -> (Value, Value) {
        match *cap {
            Capability::Native(p) => self.resolving_functions(p),
            Capability::Foreign { cap, .. } => (self.cap_get(cap, 0), self.cap_get(cap, 1)),
        }
    }
}

/// A PromiseCapability record.
#[derive(Clone, Copy)]
pub(crate) enum Capability {
    /// A promise of the intrinsic kind, settled directly.
    Native(ObjId),
    /// Made by another constructor; `cap` is the internal `[resolve, reject]` array.
    Foreign { promise: Value, cap: ObjId },
}

impl Capability {
    pub(crate) fn promise(&self) -> Value {
        match *self {
            Capability::Native(p) => Value::Object(p),
            Capability::Foreign { promise, .. } => promise,
        }
    }

    /// The reaction that settles this capability with a handler's outcome.
    pub(crate) fn reaction(&self, on_ok: Value, on_err: Value) -> Reaction {
        match *self {
            Capability::Native(p) => Reaction::Then { on_ok, on_err, derived: Some(p) },
            Capability::Foreign { cap, .. } => Reaction::ThenCap { on_ok, on_err, cap },
        }
    }
}

/// GetCapabilitiesExecutor: records resolve and reject once.
fn capability_executor(rt: &mut Runtime, slots: &[Value], _: Value, args: &[Value]) -> JsResult<Value> {
    let Value::Object(cap) = crate::promise::slot(slots, 0) else { return Ok(Value::Undefined) };
    if rt.cap_get(cap, 0) != Value::Undefined || rt.cap_get(cap, 1) != Value::Undefined {
        return Err(rt.type_error("Promise executor has already been invoked with non-undefined arguments"));
    }
    rt.cap_set(cap, 0, crate::promise::slot(args, 0));
    rt.cap_set(cap, 1, crate::promise::slot(args, 1));
    Ok(Value::Undefined)
}

pub(super) fn setup(rt: &mut Runtime) {
    let getter = rt.native_fn("get [Symbol.species]", 0, species_getter);
    rt.intr.species_getter = Some(getter);
    let g = rt.global;
    let find = |rt: &mut Runtime, name: &str| -> Option<ObjId> {
        let k = rt.key_str(name);
        rt.get_own_prop(g, k).and_then(|p| p.value.as_obj())
    };
    let accessor = Prop { value: Value::Object(getter), setter: Value::Undefined, flags: ACCESSOR | CONFIGURABLE };
    for name in ["Array", "Promise", "Map", "Set", "ArrayBuffer", "RegExp"] {
        if let Some(c) = find(rt, name) {
            rt.define_own(c, PropKey::Sym(SYM_SPECIES), accessor);
        }
    }
    rt.intr.array_ctor = find(rt, "Array");
    rt.intr.promise_ctor = find(rt, "Promise");
    for kind in TypedKind::ALL {
        let c = find(rt, kind.name());
        if let Some(c) = c {
            rt.define_own(c, PropKey::Sym(SYM_SPECIES), accessor);
        }
        rt.intr.typed_ctors[kind as usize] = c;
    }
}
