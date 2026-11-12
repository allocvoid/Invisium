//! Map, Set and their iterators.

use super::arg;
use crate::runtime::*;
use crate::value::*;

impl Runtime {
    pub(crate) fn map_key(&self, v: Value) -> MapKey {
        match v {
            Value::Undefined => MapKey::Undefined,
            Value::Null => MapKey::Null,
            Value::Bool(b) => MapKey::Bool(b),
            // SameValueZero: +0 and -0 are one key, every NaN is one key
            Value::Number(n) => MapKey::Num(if n == 0.0 { 0 } else if n.is_nan() { f64::NAN.to_bits() } else { n.to_bits() }),
            Value::String(s) => MapKey::Str(self.str_rc(s)),
            Value::Symbol(s) => MapKey::Sym(s.0),
            Value::Object(o) => MapKey::Obj(o.0),
            Value::BigInt(b) => {
                let big = self.big(b);
                self.big_note(big.mag.len());
                MapKey::Big(big.neg, big.mag.as_slice().into())
            }
        }
    }

    fn map_data(&self, o: ObjId) -> Option<&MapData> {
        match &self.obj(o).kind {
            ObjKind::Map(d) | ObjKind::Set(d) => Some(d),
            _ => None,
        }
    }

    fn map_data_mut(&mut self, o: ObjId) -> Option<&mut MapData> {
        match &mut self.obj_mut(o).kind {
            ObjKind::Map(d) | ObjKind::Set(d) => Some(d),
            _ => None,
        }
    }

    pub(crate) fn map_insert(&mut self, o: ObjId, k: Value, v: Value) {
        let key = self.map_key(k);
        let k = if k == Value::Number(0.0) { Value::Number(0.0) } else { k };
        let Some(d) = self.map_data_mut(o) else { return };
        match d.index.get(&key).copied() {
            Some(i) => {
                if let Some(Some(e)) = d.entries.get_mut(i) {
                    e.1 = v;
                }
            }
            None => {
                d.entries.push(Some((k, v)));
                d.index.insert(key, d.entries.len() - 1);
                d.live += 1;
                self.grow(96);
            }
        }
    }

    fn map_lookup(&self, o: ObjId, k: Value) -> Option<Value> {
        let d = self.map_data(o)?;
        let i = *d.index.get(&self.map_key(k))?;
        d.entries.get(i).copied().flatten().map(|e| e.1)
    }

    fn map_remove(&mut self, o: ObjId, k: Value) -> bool {
        let key = self.map_key(k);
        let Some(d) = self.map_data_mut(o) else { return false };
        match d.index.remove(&key) {
            Some(i) => {
                if let Some(e) = d.entries.get_mut(i) {
                    *e = None;
                }
                d.live = d.live.saturating_sub(1);
                true
            }
            None => false,
        }
    }

    fn new_map_iter(&mut self, map: ObjId, kind: IterKind) -> ObjId {
        let proto = if matches!(self.obj(map).kind, ObjKind::Set(_)) { self.intr.set_iter_proto } else { self.intr.map_iter_proto };
        self.alloc(Object::new(proto, ObjKind::MapIter { map, pos: 0, kind }))
    }

    /// Advances a Map/Set iterator to the next live entry.
    fn map_iter_pair(&mut self, it: ObjId) -> Option<(Value, Value)> {
        loop {
            let ObjKind::MapIter { map, pos, .. } = self.obj(it).kind else { return None };
            let entry = self.map_data(map).and_then(|d| d.entries.get(pos).copied());
            let ObjKind::MapIter { pos: p, .. } = &mut self.obj_mut(it).kind else { return None };
            match entry {
                None => {
                    *p = usize::MAX; // exhausted for good
                    return None;
                }
                Some(e) => {
                    *p = pos + 1;
                    if e.is_some() {
                        return e;
                    }
                }
            }
        }
    }

    pub(crate) fn map_iter_step(&mut self, it: ObjId) -> Option<Value> {
        let ObjKind::MapIter { kind, .. } = self.obj(it).kind else { return None };
        let (k, v) = self.map_iter_pair(it)?;
        Some(match kind {
            IterKind::Keys => k,
            IterKind::Values => v,
            IterKind::Entries => self.new_array_values(&[k, v]),
        })
    }
}

fn this_coll(rt: &mut Runtime, this: Value, set: bool) -> JsResult<ObjId> {
    if let Value::Object(o) = this {
        match (&rt.obj(o).kind, set) {
            (ObjKind::Map(_), false) | (ObjKind::Set(_), true) => return Ok(o),
            _ => {}
        }
    }
    Err(rt.type_error(if set { "Method Set.prototype method called on incompatible receiver" } else { "Method Map.prototype method called on incompatible receiver" }))
}

/// Shared constructor body: fills the new collection through its `set` / `add` method.
fn construct_coll(rt: &mut Runtime, a: &[Value], set: bool) -> JsResult<Value> {
    let nt = rt.native_new_target;
    if nt == Value::Undefined {
        return Err(rt.type_error(if set { "Constructor Set requires 'new'" } else { "Constructor Map requires 'new'" }));
    }
    let (default, kind) = if set { (rt.intr.set_proto, ObjKind::Set(Box::default())) } else { (rt.intr.map_proto, ObjKind::Map(Box::default())) };
    let proto = rt.proto_from_ctor(nt, default)?;
    let o = rt.alloc(Object::new(proto, kind));
    let src = arg(a, 0);
    if src.is_nullish() {
        return Ok(Value::Object(o));
    }
    let name = rt.key_str(if set { "add" } else { "set" });
    let adder = rt.get_prop(Value::Object(o), name)?;
    let adder = rt.callable_arg(adder, "collection adder")?;
    let it = rt.get_iterator(src)?;
    rt.scratch.push(it);
    let mark = rt.scratch.len();
    while let Some(item) = rt.iter_step(it)? {
        rt.scratch.push(item);
        if set {
            rt.call_value(adder, Value::Object(o), &[item])?;
        } else {
            if !matches!(item, Value::Object(_)) {
                rt.iter_close(it)?;
                return Err(rt.type_error("Iterator value is not an entry object"));
            }
            let k = rt.get_idx(item, 0)?;
            rt.scratch.push(k);
            let v = rt.get_idx(item, 1)?;
            rt.scratch.push(v);
            rt.call_value(adder, Value::Object(o), &[k, v])?;
        }
        rt.scratch.truncate(mark);
        // keying a BigInt is charged by size: settle it per element
        rt.drain_pending_steps()?;
    }
    Ok(Value::Object(o))
}

fn map_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    construct_coll(rt, a, false)
}
fn set_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    construct_coll(rt, a, true)
}

fn map_get(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let m = this_coll(rt, this, false)?;
    let v = rt.map_lookup(m, arg(a, 0)).unwrap_or(Value::Undefined);
    rt.drain_pending_steps()?;
    Ok(v)
}
fn map_set(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let m = this_coll(rt, this, false)?;
    rt.map_insert(m, arg(a, 0), arg(a, 1));
    rt.drain_pending_steps()?;
    Ok(this)
}
fn set_add(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let m = this_coll(rt, this, true)?;
    if rt.map_lookup(m, arg(a, 0)).is_none() {
        rt.map_insert(m, arg(a, 0), arg(a, 0));
    }
    rt.drain_pending_steps()?;
    Ok(this)
}

fn has(rt: &mut Runtime, this: Value, a: &[Value], set: bool) -> JsResult<Value> {
    let m = this_coll(rt, this, set)?;
    let found = rt.map_lookup(m, arg(a, 0)).is_some();
    rt.drain_pending_steps()?;
    Ok(Value::Bool(found))
}
fn delete(rt: &mut Runtime, this: Value, a: &[Value], set: bool) -> JsResult<Value> {
    let m = this_coll(rt, this, set)?;
    let removed = rt.map_remove(m, arg(a, 0));
    rt.drain_pending_steps()?;
    Ok(Value::Bool(removed))
}
fn clear(rt: &mut Runtime, this: Value, set: bool) -> JsResult<Value> {
    let m = this_coll(rt, this, set)?;
    if let Some(d) = rt.map_data_mut(m) {
        // keep the slots as tombstones: iterators must not revisit or skip entries
        d.entries.iter_mut().for_each(|e| *e = None);
        d.index.clear();
        d.live = 0;
    }
    Ok(Value::Undefined)
}
fn size(rt: &mut Runtime, this: Value, set: bool) -> JsResult<Value> {
    let m = this_coll(rt, this, set)?;
    Ok(Value::Number(rt.map_data(m).map(|d| d.live).unwrap_or(0) as f64))
}
fn for_each(rt: &mut Runtime, this: Value, a: &[Value], set: bool) -> JsResult<Value> {
    let m = this_coll(rt, this, set)?;
    let cb = rt.callable_arg(arg(a, 0), "forEach callback")?;
    // an iterator object keeps the walk valid if the callback mutates the collection
    let it = rt.new_map_iter(m, IterKind::Entries);
    while let Some((k, v)) = rt.map_iter_pair(it) {
        rt.call_value(cb, arg(a, 1), &[v, k, this])?;
    }
    Ok(Value::Undefined)
}
fn iter(rt: &mut Runtime, this: Value, set: bool, kind: IterKind) -> JsResult<Value> {
    let m = this_coll(rt, this, set)?;
    Ok(Value::Object(rt.new_map_iter(m, kind)))
}

fn map_has(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { has(rt, t, a, false) }
fn set_has(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { has(rt, t, a, true) }
fn map_delete(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { delete(rt, t, a, false) }
fn set_delete(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { delete(rt, t, a, true) }
fn map_clear(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { clear(rt, t, false) }
fn set_clear(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { clear(rt, t, true) }
fn map_size(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { size(rt, t, false) }
fn set_size(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { size(rt, t, true) }
fn map_for_each(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { for_each(rt, t, a, false) }
fn set_for_each(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> { for_each(rt, t, a, true) }
fn map_keys(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { iter(rt, t, false, IterKind::Keys) }
fn map_values(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { iter(rt, t, false, IterKind::Values) }
fn map_entries(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { iter(rt, t, false, IterKind::Entries) }
fn set_values(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { iter(rt, t, true, IterKind::Values) }
fn set_entries(rt: &mut Runtime, t: Value, _: &[Value]) -> JsResult<Value> { iter(rt, t, true, IterKind::Entries) }

fn iter_next(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let it = match this {
        Value::Object(o) if matches!(rt.obj(o).kind, ObjKind::MapIter { .. }) => o,
        _ => return Err(rt.type_error("next method called on incompatible receiver")),
    };
    Ok(match rt.map_iter_step(it) {
        Some(v) => rt.iter_result(v, false),
        None => rt.iter_result(Value::Undefined, true),
    })
}

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto;
    let ip = rt.intr.iterator_proto;
    let tag = |rt: &mut Runtime, o: ObjId, name: &str| {
        let t = rt.new_str(name);
        rt.define_value(o, PropKey::Sym(SYM_TO_STRING_TAG), t, CONFIGURABLE);
    };

    let mp = rt.new_object(op);
    rt.intr.map_proto = Some(mp);
    rt.constructor("Map", 0, map_ctor, mp);
    let methods: [(&str, u32, super::NFn); 8] =
        [("get", 1, map_get), ("set", 2, map_set), ("has", 1, map_has), ("delete", 1, map_delete), ("clear", 0, map_clear), ("forEach", 1, map_for_each), ("keys", 0, map_keys), ("values", 0, map_values)];
    for (n, ar, f) in methods {
        rt.method(mp, n, ar, f);
    }
    let entries = rt.method(mp, "entries", 0, map_entries);
    rt.define_value(mp, PropKey::Sym(SYM_ITERATOR), Value::Object(entries), HIDDEN);
    rt.getter(mp, "size", map_size);
    tag(rt, mp, "Map");

    let sp = rt.new_object(op);
    rt.intr.set_proto = Some(sp);
    rt.constructor("Set", 0, set_ctor, sp);
    let methods: [(&str, u32, super::NFn); 6] =
        [("add", 1, set_add), ("has", 1, set_has), ("delete", 1, set_delete), ("clear", 0, set_clear), ("forEach", 1, set_for_each), ("entries", 0, set_entries)];
    for (n, ar, f) in methods {
        rt.method(sp, n, ar, f);
    }
    let values = rt.method(sp, "values", 0, set_values);
    let keys = rt.key_str("keys");
    rt.define_value(sp, keys, Value::Object(values), HIDDEN);
    rt.define_value(sp, PropKey::Sym(SYM_ITERATOR), Value::Object(values), HIDDEN);
    rt.getter(sp, "size", set_size);
    tag(rt, sp, "Set");

    for (is_set, name) in [(false, "Map Iterator"), (true, "Set Iterator")] {
        let p = rt.new_object(ip);
        rt.method(p, "next", 0, iter_next);
        tag(rt, p, name);
        if is_set {
            rt.intr.set_iter_proto = Some(p);
        } else {
            rt.intr.map_iter_proto = Some(p);
        }
    }
}
