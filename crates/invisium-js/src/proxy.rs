//! Proxy exotic objects: trap dispatch for the internal methods, and the proxy-aware (`_ex`)
//! variants of the own-property operations used by `Object.*`, `Reflect.*` and the VM.
//! Trap results are trusted: the spec's invariant checks against the target are not enforced.

use crate::runtime::*;
use crate::value::*;

impl Runtime {
    pub(crate) fn proxy_data(&self, o: ObjId) -> Option<ProxyData> {
        match self.obj(o).kind {
            ObjKind::Proxy(p) => Some(p),
            _ => None,
        }
    }

    /// (target, handler) of a live proxy, TypeError once revoked. Every trap dispatch passes
    /// through here, which also bounds native recursion through chains of proxies.
    fn proxy_parts(&mut self, p: ProxyData, what: &str) -> JsResult<(ObjId, ObjId)> {
        self.check_native_stack()?;
        match (p.target, p.handler) {
            (Some(t), Some(h)) => Ok((t, h)),
            _ => Err(self.type_error(&format!("Cannot perform '{}' on a proxy that has been revoked", what))),
        }
    }

    fn proxy_trap(&mut self, handler: ObjId, name: Atom) -> JsResult<Option<Value>> {
        let t = self.get_from(handler, PropKey::Str(name), Value::Object(handler))?;
        if t.is_nullish() {
            return Ok(None);
        }
        if !self.is_callable_val(t) {
            return Err(self.type_error("Proxy trap is not a function"));
        }
        Ok(Some(t))
    }

    pub(crate) fn proxy_get(&mut self, p: ProxyData, k: PropKey, receiver: Value) -> JsResult<Value> {
        let (t, h) = self.proxy_parts(p, "get")?;
        match self.proxy_trap(h, A_GET)? {
            None => self.get_from(t, k, receiver),
            Some(f) => {
                let kv = self.key_to_value(k);
                self.call_value(f, Value::Object(h), &[Value::Object(t), kv, receiver])
            }
        }
    }

    pub(crate) fn proxy_set(&mut self, p: ProxyData, k: PropKey, v: Value, receiver: Value) -> JsResult<bool> {
        let (t, h) = self.proxy_parts(p, "set")?;
        match self.proxy_trap(h, A_SET)? {
            None => self.set_prop(Value::Object(t), k, v, receiver),
            Some(f) => {
                let kv = self.key_to_value(k);
                let r = self.call_value(f, Value::Object(h), &[Value::Object(t), kv, v, receiver])?;
                Ok(self.truthy(r))
            }
        }
    }

    pub(crate) fn proxy_has(&mut self, p: ProxyData, k: PropKey) -> JsResult<bool> {
        let (t, h) = self.proxy_parts(p, "has")?;
        match self.proxy_trap(h, A_HAS)? {
            None => self.has_property(t, k),
            Some(f) => {
                let kv = self.key_to_value(k);
                let r = self.call_value(f, Value::Object(h), &[Value::Object(t), kv])?;
                Ok(self.truthy(r))
            }
        }
    }

    /// [[Delete]].
    pub(crate) fn delete_prop_ex(&mut self, o: ObjId, k: PropKey) -> JsResult<bool> {
        let Some(p) = self.proxy_data(o) else { return Ok(self.delete_prop(o, k)) };
        let (t, h) = self.proxy_parts(p, "deleteProperty")?;
        match self.proxy_trap(h, A_DELETE_PROPERTY)? {
            None => self.delete_prop_ex(t, k),
            Some(f) => {
                let kv = self.key_to_value(k);
                let r = self.call_value(f, Value::Object(h), &[Value::Object(t), kv])?;
                Ok(self.truthy(r))
            }
        }
    }

    /// Listing a typed array's keys interns one name per element: refuse a list the heap cap
    /// could not hold instead of building it.
    fn keys_guard(&mut self, o: ObjId) -> JsResult<()> {
        match self.typed_view(Value::Object(o)) {
            Some(v) if v.length > 0 => self.check_alloc(v.length.saturating_mul(96)).map_err(|t| self.thrown_to_exc(t)),
            _ => Ok(()),
        }
    }

    /// [[OwnPropertyKeys]].
    pub(crate) fn own_keys_ex(&mut self, o: ObjId) -> JsResult<Vec<PropKey>> {
        let Some(p) = self.proxy_data(o) else {
            self.keys_guard(o)?;
            return Ok(self.own_keys(o));
        };
        let (t, h) = self.proxy_parts(p, "ownKeys")?;
        let Some(f) = self.proxy_trap(h, A_OWN_KEYS)? else { return self.own_keys_ex(t) };
        let r = self.call_value(f, Value::Object(h), &[Value::Object(t)])?;
        if !matches!(r, Value::Object(_)) {
            return Err(self.type_error("ownKeys trap result is not an object"));
        }
        let mark = self.scratch.len();
        self.scratch.push(r);
        let out = self.keys_from_list(r);
        self.scratch.truncate(mark);
        out
    }

    fn keys_from_list(&mut self, list: Value) -> JsResult<Vec<PropKey>> {
        let n = self.length_of(list)?;
        if n > 10_000_000 {
            return Err(self.range_error("ownKeys trap result is too long"));
        }
        let mut out = Vec::new();
        for i in 0..n {
            match self.get_idx(list, i)? {
                v @ (Value::String(_) | Value::Symbol(_)) => out.push(self.to_property_key(v)?),
                _ => return Err(self.type_error("ownKeys trap result must contain only strings and symbols")),
            }
        }
        Ok(out)
    }

    /// Own enumerable string keys, in order (EnumerableOwnProperties).
    pub(crate) fn own_enum_keys_ex(&mut self, o: ObjId) -> JsResult<Vec<PropKey>> {
        if self.proxy_data(o).is_none() {
            self.keys_guard(o)?;
            return Ok(self.own_enum_keys(o));
        }
        let mut out = Vec::new();
        for k in self.own_keys_ex(o)? {
            if matches!(k, PropKey::Str(_)) && self.get_own_prop_ex(o, k)?.map(|p| p.flags & ENUMERABLE != 0).unwrap_or(false) {
                out.push(k);
            }
        }
        Ok(out)
    }

    /// [[GetOwnProperty]].
    pub(crate) fn get_own_prop_ex(&mut self, o: ObjId, k: PropKey) -> JsResult<Option<Prop>> {
        let Some(p) = self.proxy_data(o) else { return Ok(self.get_own_prop_full(o, k)) };
        let (t, h) = self.proxy_parts(p, "getOwnPropertyDescriptor")?;
        let Some(f) = self.proxy_trap(h, A_GET_OWN_PROPERTY_DESCRIPTOR)? else { return self.get_own_prop_ex(t, k) };
        let kv = self.key_to_value(k);
        let d = self.call_value(f, Value::Object(h), &[Value::Object(t), kv])?;
        match d {
            Value::Undefined => Ok(None),
            Value::Object(_) => {
                let mark = self.scratch.len();
                self.scratch.push(d);
                let r = self.prop_from_descriptor(d);
                self.scratch.truncate(mark);
                r.map(Some)
            }
            _ => Err(self.type_error("getOwnPropertyDescriptor trap returned neither object nor undefined")),
        }
    }

    /// ToPropertyDescriptor with absent fields defaulted.
    fn prop_from_descriptor(&mut self, d: Value) -> JsResult<Prop> {
        let mut flags = 0;
        for (a, bit) in [(A_ENUMERABLE, ENUMERABLE), (A_CONFIGURABLE, CONFIGURABLE), (A_WRITABLE, WRITABLE)] {
            let v = self.get_prop(d, PropKey::Str(a))?;
            if self.truthy(v) {
                flags |= bit;
            }
        }
        let get = self.get_prop(d, PropKey::Str(A_GET))?;
        let set = self.get_prop(d, PropKey::Str(A_SET))?;
        if get != Value::Undefined || set != Value::Undefined {
            return Ok(Prop { value: get, setter: set, flags: (flags & !WRITABLE) | ACCESSOR });
        }
        let value = self.get_prop(d, PropKey::Str(A_VALUE))?;
        Ok(Prop::data(value, flags))
    }

    /// [[DefineOwnProperty]] on a proxy, with the descriptor as an object.
    pub(crate) fn proxy_define(&mut self, p: ProxyData, k: PropKey, desc: Value) -> JsResult<bool> {
        let (t, h) = self.proxy_parts(p, "defineProperty")?;
        match self.proxy_trap(h, A_DEFINE_PROPERTY)? {
            None => crate::builtins::define_desc(self, t, k, desc),
            Some(f) => {
                let kv = self.key_to_value(k);
                let r = self.call_value(f, Value::Object(h), &[Value::Object(t), kv, desc])?;
                Ok(self.truthy(r))
            }
        }
    }

    /// The tail of [[Set]] when the receiver is a proxy: update or create the property through
    /// the receiver's own [[GetOwnProperty]] / [[DefineOwnProperty]].
    pub(crate) fn proxy_receiver_set(&mut self, r: ObjId, p: ProxyData, k: PropKey, v: Value) -> JsResult<bool> {
        let existing = self.get_own_prop_ex(r, k)?;
        if matches!(existing, Some(e) if e.is_accessor() || e.flags & WRITABLE == 0) {
            return Ok(false);
        }
        let mark = self.scratch.len();
        let d = self.plain_object();
        self.scratch.push(Value::Object(d));
        self.define_value(d, PropKey::Str(A_VALUE), v, DEFAULT_FLAGS);
        if existing.is_none() {
            for a in [A_WRITABLE, A_ENUMERABLE, A_CONFIGURABLE] {
                self.define_value(d, PropKey::Str(a), Value::Bool(true), DEFAULT_FLAGS);
            }
        }
        let out = self.proxy_define(p, k, Value::Object(d));
        self.scratch.truncate(mark);
        out
    }

    /// [[Call]] on a callable proxy.
    pub(crate) fn proxy_call(&mut self, p: ProxyData, this: Value, args: &[Value]) -> JsResult<Value> {
        let (t, h) = self.proxy_parts(p, "apply")?;
        match self.proxy_trap(h, A_APPLY)? {
            None => self.call_value(Value::Object(t), this, args),
            Some(f) => {
                let list = self.new_array_values(args);
                self.call_value(f, Value::Object(h), &[Value::Object(t), this, list])
            }
        }
    }

    /// [[Construct]] on a proxy `me` whose target is a constructor.
    pub(crate) fn proxy_construct(&mut self, me: ObjId, args: &[Value], new_target: Value) -> JsResult<Value> {
        let Some(p) = self.proxy_data(me) else { return Err(self.type_error("value is not a constructor")) };
        let (t, h) = self.proxy_parts(p, "construct")?;
        match self.proxy_trap(h, A_CONSTRUCT)? {
            None => {
                let nt = if new_target == Value::Object(me) { Value::Object(t) } else { new_target };
                self.construct_value(Value::Object(t), args, nt)
            }
            Some(f) => {
                let list = self.new_array_values(args);
                let r = self.call_value(f, Value::Object(h), &[Value::Object(t), list, new_target])?;
                if !matches!(r, Value::Object(_)) {
                    return Err(self.type_error("proxy [[Construct]] must return an object"));
                }
                Ok(r)
            }
        }
    }

    /// The object behind a chain of (live) proxies. The walk is bounded so that a hostile,
    /// very long chain cannot make one call expensive; past the bound the answer is the proxy
    /// reached, whose prototype slot still mirrors the chain's base object.
    pub(crate) fn unwrap_proxy(&self, mut o: ObjId) -> ObjId {
        for _ in 0..256 {
            match self.proxy_data(o).and_then(|p| p.target) {
                Some(t) => o = t,
                None => break,
            }
        }
        o
    }

    /// [[GetPrototypeOf]] (proxies answer for their target).
    pub(crate) fn proto_of(&self, o: ObjId) -> Option<ObjId> {
        self.obj(self.unwrap_proxy(o)).proto
    }

    /// IsArray.
    pub(crate) fn is_array_obj(&self, o: ObjId) -> bool {
        matches!(self.obj(self.unwrap_proxy(o)).kind, ObjKind::Array(_))
    }

    /// [[SetPrototypeOf]]; false when the object refuses.
    pub(crate) fn set_proto_of(&mut self, o: ObjId, proto: Option<ObjId>) -> bool {
        let target = self.unwrap_proxy(o);
        if self.obj(target).proto == proto {
            return true;
        }
        if !self.obj(target).extensible {
            return false;
        }
        let mut cur = proto;
        let mut guard = 0;
        while let Some(c) = cur {
            if c == target || c == o || guard > 100_000 {
                return false;
            }
            cur = self.obj(c).proto;
            guard += 1;
        }
        // the cycle check walked the whole chain: charge it (the next safepoint enforces it)
        self.steps += guard as u64 / 16;
        self.obj_mut(target).proto = proto;
        if target != o {
            self.obj_mut(o).proto = proto;
        }
        true
    }
}
