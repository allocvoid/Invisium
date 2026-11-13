//! Iteration protocol, for-in enumeration and miscellaneous object helpers used by the VM.

use crate::runtime::*;
use crate::value::*;
use std::collections::HashSet;

impl Runtime {
    fn array_iter_pristine(&self, o: ObjId) -> bool {
        let ap = self.intr.array_proto;
        if self.obj(o).proto != ap || self.obj(o).props.get(PropKey::Sym(SYM_ITERATOR)).is_some() {
            return false;
        }
        match (ap, self.intr.array_values) {
            (Some(ap), Some(av)) => matches!(self.obj(ap).props.get(PropKey::Sym(SYM_ITERATOR)), Some(p) if p.value == Value::Object(av)),
            _ => false,
        }
    }

    pub(crate) fn new_array_iter(&mut self, target: Value, kind: IterKind) -> Value {
        let p = self.intr.array_iter_proto;
        Value::Object(self.alloc(Object::new(p, ObjKind::ArrayIter { target, index: 0, kind, done: false })))
    }

    pub(crate) fn get_iterator(&mut self, v: Value) -> JsResult<Value> {
        self.get_iterator_ex(v, false)
    }

    /// `at_site`: called by the bytecode op itself, so the error can name the source expression.
    pub(crate) fn get_iterator_ex(&mut self, v: Value, at_site: bool) -> JsResult<Value> {
        if let Value::Object(o) = v {
            if matches!(self.obj(o).kind, ObjKind::Array(_)) && self.array_iter_pristine(o) {
                return Ok(self.new_array_iter(v, IterKind::Values));
            }
        }
        if let Value::String(s) = v {
            let p = self.intr.string_iter_proto;
            return Ok(Value::Object(self.alloc(Object::new(p, ObjKind::StringIter { s, pos: 0 }))));
        }
        if v.is_nullish() {
            let d = if at_site { self.site_or_describe(v) } else { self.describe(v) };
            return Err(self.type_error(&format!("{} is not iterable", d)));
        }
        let site = if at_site { self.site_desc() } else { None };
        let m = self.get_prop(v, PropKey::Sym(SYM_ITERATOR))?;
        if !self.is_callable_val(m) {
            let d = match site {
                Some(s) => s,
                None => self.describe(v),
            };
            return Err(self.type_error(&format!("{} is not iterable", d)));
        }
        let it = self.call_value(m, v, &[])?;
        if !matches!(it, Value::Object(_)) {
            return Err(self.type_error("Result of the Symbol.iterator method is not an object"));
        }
        Ok(it)
    }

    /// Advances an iterator: Some(value) or None when done.
    pub(crate) fn iter_step(&mut self, it: Value) -> JsResult<Option<Value>> {
        let Value::Object(o) = it else { return Err(self.type_error("iterator is not an object")) };
        match self.obj(o).kind {
            ObjKind::ArrayIter { target, index, kind, done } => {
                if done {
                    return Ok(None);
                }
                let len = self.length_of(target)?;
                if index as u64 >= len {
                    if let ObjKind::ArrayIter { done, .. } = &mut self.obj_mut(o).kind {
                        *done = true;
                    }
                    return Ok(None);
                }
                if let ObjKind::ArrayIter { index, .. } = &mut self.obj_mut(o).kind {
                    *index += 1;
                }
                let out = match kind {
                    IterKind::Keys => Value::Number(index as f64),
                    IterKind::Values => self.get_idx(target, index as u64)?,
                    IterKind::Entries => {
                        let v = self.get_idx(target, index as u64)?;
                        self.new_array_values(&[Value::Number(index as f64), v])
                    }
                };
                return Ok(Some(out));
            }
            ObjKind::StringIter { s, pos } => {
                let units = self.str_units(s);
                let p = pos as usize;
                if p >= units.len() {
                    return Ok(None);
                }
                let mut n = 1;
                if (0xD800..0xDC00).contains(&units[p]) && p + 1 < units.len() && (0xDC00..0xE000).contains(&units[p + 1]) {
                    n = 2;
                }
                let chunk = units[p..p + n].to_vec();
                if let ObjKind::StringIter { pos, .. } = &mut self.obj_mut(o).kind {
                    *pos += n as u32;
                }
                return Ok(Some(self.new_str16(chunk)));
            }
            ObjKind::MapIter { .. } => return Ok(self.map_iter_step(o)),
            _ => {}
        }
        let next = self.get_prop(it, PropKey::Str(A_NEXT))?;
        let r = self.call_value(next, it, &[])?;
        if !matches!(r, Value::Object(_)) {
            return Err(self.type_error("Iterator result is not an object"));
        }
        let done = self.get_prop(r, PropKey::Str(A_DONE))?;
        if self.truthy(done) {
            return Ok(None);
        }
        Ok(Some(self.get_prop(r, PropKey::Str(A_VALUE))?))
    }

    pub(crate) fn iter_close(&mut self, it: Value) -> JsResult<()> {
        let Value::Object(o) = it else { return Ok(()) };
        if matches!(self.obj(o).kind, ObjKind::ArrayIter { .. } | ObjKind::StringIter { .. } | ObjKind::MapIter { .. }) {
            return Ok(());
        }
        let r = self.get_prop(it, PropKey::Str(A_RETURN))?;
        if self.is_callable_val(r) {
            self.call_value(r, it, &[])?;
        }
        Ok(())
    }

    pub(crate) fn iterate_to_vec(&mut self, v: Value) -> JsResult<Vec<Value>> {
        self.iterate_to_vec_ex(v, false)
    }

    /// Inside a native the items stay rooted until the native returns: they may be reachable
    /// from nothing else (a generator's values, an array the next callback empties) while the
    /// native goes on to call script.
    pub(crate) fn iterate_to_vec_ex(&mut self, v: Value, at_site: bool) -> JsResult<Vec<Value>> {
        if let Value::Object(o) = v {
            if self.array_iter_pristine(o) {
                if let ObjKind::Array(items) = &self.obj(o).kind {
                    let items: Vec<Value> = items.iter().map(|x| x.unwrap_or(Value::Undefined)).collect();
                    if self.native_active {
                        self.scratch.extend(items.iter().copied().filter(|x| matches!(x, Value::Object(_) | Value::String(_) | Value::BigInt(_))));
                    }
                    return Ok(items);
                }
            }
        }
        let it = self.get_iterator_ex(v, at_site)?;
        let mark = self.scratch.len();
        self.scratch.push(it);
        let r = self.collect_iter(it);
        if !self.native_active {
            self.scratch.truncate(mark);
        }
        r
    }

    fn collect_iter(&mut self, it: Value) -> JsResult<Vec<Value>> {
        let mut out = Vec::new();
        while let Some(x) = self.iter_step(it)? {
            out.push(x);
            self.scratch.push(x);
            if out.len() % 1024 == 0 {
                self.safepoint_counted()?;
                self.check_alloc(out.len() * 16).map_err(|t| self.thrown_to_exc(t))?;
            }
        }
        Ok(out)
    }

    pub(crate) fn safepoint_counted(&mut self) -> JsResult<()> {
        self.steps += 1;
        if self.steps > self.limits.step_budget {
            return Err(Exc::Abort(ErrorKind::StepBudget));
        }
        self.drain_pending_steps()
    }

    pub(crate) fn for_in_iterator(&mut self, v: Value) -> JsResult<Value> {
        let mut keys = Vec::new();
        if !v.is_nullish() {
            let o = self.to_object(v)?;
            let mut seen: HashSet<PropKey> = HashSet::new();
            let mut cur = Some(o);
            let mut guard = 0;
            while let Some(c) = cur {
                for k in self.own_keys_ex(c)? {
                    if let PropKey::Str(a) = k {
                        if seen.insert(k) {
                            let en = self.get_own_prop_ex(c, k)?.map(|p| p.flags & ENUMERABLE != 0).unwrap_or(false);
                            if en {
                                keys.push(StrId::from_atom(a));
                            }
                        }
                    }
                }
                cur = self.proto_of(c);
                guard += 1;
                if guard > 10_000 {
                    break;
                }
            }
        }
        let p = self.intr.object_proto;
        Ok(Value::Object(self.alloc(Object::new(p, ObjKind::ForIn { keys, index: 0, obj: v }))))
    }

    pub(crate) fn for_in_next(&mut self, it: Value) -> JsResult<Option<StrId>> {
        let Value::Object(o) = it else { return Ok(None) };
        loop {
            let (key, obj) = match &mut self.obj_mut(o).kind {
                ObjKind::ForIn { keys, index, obj } => {
                    if *index >= keys.len() {
                        return Ok(None);
                    }
                    *index += 1;
                    (keys[*index - 1], *obj)
                }
                _ => return Ok(None),
            };
            // skip keys deleted during enumeration
            if let (Value::Object(ob), Some(a)) = (obj, key.as_atom()) {
                if !self.has_property(ob, PropKey::Str(a))? {
                    continue;
                }
            }
            return Ok(Some(key));
        }
    }

    pub(crate) fn copy_data_props(&mut self, target: ObjId, src: Value, excluded: &[PropKey]) -> JsResult<()> {
        if src.is_nullish() {
            return Ok(());
        }
        let from = self.to_object(src)?;
        self.scratch.push(Value::Object(from));
        for k in self.own_keys_ex(from)? {
            if excluded.contains(&k) {
                continue;
            }
            let en = self.get_own_prop_ex(from, k)?.map(|p| p.flags & ENUMERABLE != 0).unwrap_or(false);
            if en {
                let v = self.get_prop(Value::Object(from), k)?;
                self.define_value(target, k, v, DEFAULT_FLAGS);
            }
        }
        self.scratch.pop();
        Ok(())
    }

    pub(crate) fn make_arguments(&mut self, args: &[Value], callee: Option<ObjId>) -> ObjId {
        let p = self.intr.object_proto;
        let o = self.alloc(Object::new(p, ObjKind::Arguments(None)));
        for (i, a) in args.iter().enumerate() {
            let k = self.index_key(i as u64);
            self.define_value(o, k, *a, DEFAULT_FLAGS);
        }
        self.define_value(o, PropKey::Str(A_LENGTH), Value::Number(args.len() as f64), HIDDEN);
        if let Some(av) = self.intr.array_values {
            self.define_value(o, PropKey::Sym(SYM_ITERATOR), Value::Object(av), HIDDEN);
        }
        if let Some(c) = callee {
            self.define_value(o, PropKey::Str(A_CALLEE), Value::Object(c), HIDDEN);
        }
        o
    }

    pub(crate) fn template_object(&mut self, fi: usize, i: u32) -> Value {
        let code = self.frames[fi].code.clone();
        let key = (std::sync::Arc::as_ptr(&code) as usize, i);
        if let Some((_, o)) = self.template_cache.get(&key) {
            return Value::Object(*o);
        }
        let Some(t) = code.templates.get(i as usize) else { return Value::Undefined };
        let cooked: Vec<Value> = t.cooked.clone().into_iter().map(|c| match c {
            Some(u) => self.new_str16(u),
            None => Value::Undefined,
        }).collect();
        let raw: Vec<Value> = t.raw.clone().into_iter().map(|u| self.new_str16(u)).collect();
        let ca = self.new_array_values(&cooked);
        let ra = self.new_array_values(&raw);
        if let (Value::Object(c), Value::Object(r)) = (ca, ra) {
            self.define_value(c, PropKey::Str(A_RAW), ra, 0);
            for o in [c, r] {
                let ob = self.obj_mut(o);
                ob.integrity = 2;
                ob.extensible = false;
            }
            self.template_cache.insert(key, (code.clone(), c));
        }
        ca
    }

    pub(crate) fn make_class(&mut self, fi: usize, idx: u32, has_parent: bool, _atoms: &[Atom]) -> JsResult<()> {
        let parent = if has_parent { Some(self.pop()) } else { None };
        let (proto_parent, ctor_parent) = match parent {
            None => (self.intr.object_proto, self.intr.function_proto),
            Some(Value::Null) => (None, self.intr.function_proto),
            Some(p) => {
                if !self.is_constructor(p) {
                    let d = self.describe(p);
                    return Err(self.type_error(&format!("Class extends value {} is not a constructor or null", d)));
                }
                let pp = self.get_prop(p, PropKey::Str(A_PROTOTYPE))?;
                let pp = match pp {
                    Value::Object(o) => Some(o),
                    Value::Null => None,
                    _ => return Err(self.type_error("Class extends value does not have valid prototype property")),
                };
                (pp, p.as_obj())
            }
        };
        let proto = self.new_object(proto_parent);
        self.stack.push(Value::Object(proto));
        let code = self.frames[fi].code.children.get(idx as usize).cloned();
        let Some(code) = code else { return Err(self.type_error("invalid class")) };
        let atoms = self.frames[fi].atoms.clone();
        let ctor = self.make_closure(code, atoms);
        self.obj_mut(ctor).proto = ctor_parent;
        if let Some(c) = self.closure_mut(ctor) {
            c.home = Some(proto);
        }
        self.define_value(ctor, PropKey::Str(A_PROTOTYPE), Value::Object(proto), 0);
        self.define_value(proto, PropKey::Str(A_CONSTRUCTOR), Value::Object(ctor), HIDDEN);
        self.pop();
        self.stack.push(Value::Object(ctor));
        self.stack.push(Value::Object(proto));
        Ok(())
    }
}
