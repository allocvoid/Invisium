//! Property access: own-property lookup (incl. exotic arrays/strings/host objects), [[Get]],
//! [[Set]], [[DefineOwnProperty]], [[Delete]] and key enumeration.

use crate::runtime::*;
use crate::value::*;

impl Runtime {
    pub(crate) fn key_index(&self, k: PropKey) -> Option<u32> {
        match k {
            PropKey::Str(a) => self.atom_array_index(a),
            PropKey::Sym(_) => None,
        }
    }

    pub(crate) fn key_rust(&self, k: PropKey) -> String {
        match k {
            PropKey::Str(a) => String::from_utf16_lossy(self.atom_units(a)),
            PropKey::Sym(s) => {
                let d = self.syms.get(s.0 as usize).cloned().flatten();
                format!("Symbol({})", d.map(|d| String::from_utf16_lossy(&d)).unwrap_or_default())
            }
        }
    }

    pub(crate) fn get_own_prop(&self, o: ObjId, k: PropKey) -> Option<Prop> {
        let obj = self.obj(o);
        match &obj.kind {
            ObjKind::Array(items) => {
                if let Some(i) = self.key_index(k) {
                    return items.get(i as usize).copied().flatten().map(|v| {
                        let flags = match obj.integrity {
                            2 => ENUMERABLE,
                            1 => ENUMERABLE | WRITABLE,
                            _ => DEFAULT_FLAGS,
                        };
                        Prop::data(v, flags)
                    });
                }
                if k == PropKey::Str(A_LENGTH) {
                    let w = if obj.integrity == 2 { 0 } else { WRITABLE };
                    return Some(Prop::data(Value::Number(items.len() as f64), w));
                }
            }
            ObjKind::String(s) => {
                let units = self.str_units(*s);
                if let Some(i) = self.key_index(k) {
                    if (i as usize) < units.len() {
                        // value materialised by callers that need it (get_string_index)
                        return Some(Prop::data(Value::Undefined, ENUMERABLE));
                    }
                }
                if k == PropKey::Str(A_LENGTH) {
                    return Some(Prop::data(Value::Number(units.len() as f64), 0));
                }
            }
            ObjKind::TypedArray(_) => {
                // integer-indexed exotic object: an index key is an element or nothing at all
                if let Some(i) = self.key_index(k) {
                    return self.typed_get(o, i as usize).map(|n| Prop::data(Value::Number(n), DEFAULT_FLAGS));
                }
            }
            ObjKind::Arguments(Some(_)) => {
                // a mapped index reads the parameter's current value
                if let (Some((env, slot)), Some(p)) = (self.arg_mapping(o, k), obj.props.get(k)) {
                    let live = self.env(env).and_then(|e| e.slots.get(slot).copied().flatten());
                    return Some(Prop { value: live.unwrap_or(p.value), ..*p });
                }
            }
            _ => {}
        }
        obj.props.get(k).copied()
    }

    /// The parameter binding (environment, slot) a mapped arguments object aliases at `k`.
    fn arg_mapping(&self, o: ObjId, k: PropKey) -> Option<(EnvId, usize)> {
        let ObjKind::Arguments(Some(m)) = &self.obj(o).kind else { return None };
        let slot = *m.slots.get(self.key_index(k)? as usize)?;
        if slot == u32::MAX { None } else { Some((m.env, slot as usize)) }
    }

    fn arg_unmap(&mut self, o: ObjId, k: PropKey) {
        let Some(i) = self.key_index(k) else { return };
        if let ObjKind::Arguments(Some(m)) = &mut self.obj_mut(o).kind {
            if let Some(s) = m.slots.get_mut(i as usize) {
                *s = u32::MAX;
            }
        }
    }

    /// Writes through a mapped arguments index to the parameter it aliases.
    fn arg_write(&mut self, o: ObjId, k: PropKey, v: Value) {
        if let Some((env, slot)) = self.arg_mapping(o, k) {
            if let Some(s) = self.env_mut(env).and_then(|e| e.slots.get_mut(slot)) {
                *s = Some(v);
            }
        }
    }

    /// Own property including the materialised value for string-wrapper indices.
    pub(crate) fn get_own_prop_full(&mut self, o: ObjId, k: PropKey) -> Option<Prop> {
        if let ObjKind::TypedArray(vd) = self.obj(o).kind {
            if vd.kind.is_big() {
                if let Some(i) = self.key_index(k) {
                    // 64-bit integer elements are BigInts, made on demand
                    return self.typed_value(o, i as usize).map(|v| Prop::data(v, DEFAULT_FLAGS));
                }
            }
        }
        if let ObjKind::String(s) = self.obj(o).kind {
            if let Some(i) = self.key_index(k) {
                let u = self.str_units(s).get(i as usize).copied()?;
                let v = self.new_str16(vec![u]);
                return Some(Prop::data(v, ENUMERABLE));
            }
        }
        self.get_own_prop(o, k)
    }

    pub(crate) fn get_prop(&mut self, v: Value, k: PropKey) -> JsResult<Value> {
        match v {
            Value::Object(o) => self.get_from(o, k, v),
            Value::String(s) => {
                if k == PropKey::Str(A_LENGTH) {
                    return Ok(Value::Number(self.str_units(s).len() as f64));
                }
                if let Some(i) = self.key_index(k) {
                    if let Some(&u) = self.str_units(s).get(i as usize) {
                        return Ok(self.new_str16(vec![u]));
                    }
                    return Ok(Value::Undefined);
                }
                match self.intr.string_proto {
                    Some(p) => self.get_from(p, k, v),
                    None => Ok(Value::Undefined),
                }
            }
            Value::Number(_) => self.get_from_opt(self.intr.number_proto, k, v),
            Value::Bool(_) => self.get_from_opt(self.intr.boolean_proto, k, v),
            Value::Symbol(s) => {
                if k == PropKey::Str(A_NAME) || k == self.key_desc() {
                    let d = self.syms.get(s.0 as usize).cloned().flatten();
                    return Ok(match d {
                        Some(d) => self.new_str16(d.to_vec()),
                        None => Value::Undefined,
                    });
                }
                self.get_from_opt(self.intr.symbol_proto, k, v)
            }
            Value::BigInt(_) => self.get_from_opt(self.intr.bigint_proto, k, v),
            Value::Undefined | Value::Null => {
                let what = if v == Value::Undefined { "undefined" } else { "null" };
                let key = self.key_rust(k);
                Err(self.type_error(&format!("Cannot read properties of {} (reading '{}')", what, key)))
            }
        }
    }

    fn key_desc(&self) -> PropKey {
        PropKey::Str(self.atom_map.get(&"description".encode_utf16().collect::<Vec<u16>>()[..]).map(|&i| Atom(i)).unwrap_or(A_EMPTY))
    }

    fn get_from_opt(&mut self, p: Option<ObjId>, k: PropKey, receiver: Value) -> JsResult<Value> {
        match p {
            Some(p) => self.get_from(p, k, receiver),
            None => Ok(Value::Undefined),
        }
    }

    pub(crate) fn get_from(&mut self, mut o: ObjId, k: PropKey, receiver: Value) -> JsResult<Value> {
        for hop in 0..100_000u32 {
            if hop & 15 == 15 {
                // a very long prototype chain is paid for like a loop
                self.steps += 1;
                if self.steps > self.limits.step_budget {
                    return Err(Exc::Abort(ErrorKind::StepBudget));
                }
            }
            match self.obj(o).kind {
                ObjKind::Host(class, id) => {
                    if let (PropKey::Str(a), Some(c)) = (k, self.host_hooks(class)) {
                        let key = String::from_utf16_lossy(self.atom_units(a));
                        if let Some(v) = self.call_host(|rt| (c.get)(rt, id, &key))? {
                            return Ok(v);
                        }
                    }
                }
                ObjKind::Proxy(p) => return self.proxy_get(p, k, receiver),
                _ => {}
            }
            match self.get_own_prop_full(o, k) {
                Some(p) => {
                    if p.is_accessor() {
                        if p.value == Value::Undefined {
                            return Ok(Value::Undefined);
                        }
                        return self.call_value(p.value, receiver, &[]);
                    }
                    return Ok(p.value);
                }
                None => match self.obj(o).proto {
                    Some(p) => o = p,
                    None => return Ok(Value::Undefined),
                },
            }
        }
        Ok(Value::Undefined)
    }

    /// Runs a host callback as a native call (temporaries rooted, abort propagated).
    pub(crate) fn call_host<T>(&mut self, f: impl FnOnce(&mut Runtime) -> Result<T, crate::Thrown>) -> JsResult<T> {
        let saved = self.native_active;
        let mark = self.scratch.len();
        self.native_active = true;
        let r = f(self);
        self.native_active = saved;
        self.scratch.truncate(mark);
        r.map_err(|t| self.thrown_to_exc(t))
    }

    pub(crate) fn thrown_to_exc(&mut self, t: crate::Thrown) -> Exc {
        match self.pending_abort.take() {
            Some(k) => Exc::Abort(k),
            None => Exc::Throw(t.0),
        }
    }

    /// [[Set]]; returns false when the assignment was rejected.
    pub(crate) fn set_prop(&mut self, target: Value, k: PropKey, v: Value, receiver: Value) -> JsResult<bool> {
        let mut cur = match target {
            Value::Object(o) => Some(o),
            Value::String(_) if k == PropKey::Str(A_LENGTH) || self.key_index(k).is_some() => return Ok(false),
            Value::String(_) => self.intr.string_proto,
            Value::Number(_) => self.intr.number_proto,
            Value::Bool(_) => self.intr.boolean_proto,
            Value::Symbol(_) => self.intr.symbol_proto,
            Value::BigInt(_) => self.intr.bigint_proto,
            Value::Undefined | Value::Null => {
                let what = if target == Value::Undefined { "undefined" } else { "null" };
                let key = self.key_rust(k);
                return Err(self.type_error(&format!("Cannot set properties of {} (setting '{}')", what, key)));
            }
        };
        let mut steps = 0;
        while let Some(o) = cur {
            steps += 1;
            if steps > 100_000 {
                break;
            }
            match self.obj(o).kind {
                ObjKind::Host(class, id) => {
                    if let (PropKey::Str(a), true, Some(c)) = (k, Value::Object(o) == receiver, self.host_hooks(class)) {
                        let key = String::from_utf16_lossy(self.atom_units(a));
                        if self.call_host(|rt| (c.set)(rt, id, &key, v))? {
                            return Ok(true);
                        }
                    }
                }
                ObjKind::Proxy(p) => return self.proxy_set(p, k, v, receiver),
                ObjKind::TypedArray(_) if Value::Object(o) == receiver => {
                    if let Some(i) = self.key_index(k) {
                        // an out-of-range index is silently dropped, never an own property
                        self.typed_store(o, i as usize, v)?;
                        return Ok(true);
                    }
                }
                _ => {}
            }
            match self.get_own_prop(o, k) {
                Some(p) if p.is_accessor() => {
                    if p.setter == Value::Undefined {
                        return Ok(false);
                    }
                    self.call_value(p.setter, receiver, &[v])?;
                    return Ok(true);
                }
                Some(p) => {
                    if p.flags & WRITABLE == 0 {
                        return Ok(false);
                    }
                    if Value::Object(o) == receiver {
                        return self.write_own_data(o, k, v);
                    }
                    break;
                }
                None => cur = self.obj(o).proto,
            }
        }
        let Value::Object(r) = receiver else { return Ok(false) };
        if let Some(p) = self.proxy_data(r) {
            return self.proxy_receiver_set(r, p, k, v);
        }
        if let Some(p) = self.get_own_prop(r, k) {
            if p.is_accessor() || p.flags & WRITABLE == 0 {
                return Ok(false);
            }
            return self.write_own_data(r, k, v);
        }
        if !self.obj(r).extensible {
            return Ok(false);
        }
        self.write_own_data(r, k, v)
    }

    /// Writes/creates an own data property honouring array exotics (attributes preserved).
    fn write_own_data(&mut self, o: ObjId, k: PropKey, v: Value) -> JsResult<bool> {
        if let ObjKind::Array(_) = self.obj(o).kind {
            if let Some(i) = self.key_index(k) {
                return self.array_set_index(o, i, v);
            }
            if k == PropKey::Str(A_LENGTH) {
                return self.array_set_length(o, v);
            }
        }
        if matches!(self.obj(o).kind, ObjKind::Arguments(Some(_))) {
            self.arg_write(o, k, v);
        }
        let obj = self.obj_mut(o);
        match obj.props.get_mut(k) {
            Some(p) => {
                p.value = v;
                Ok(true)
            }
            None => {
                obj.props.insert(k, Prop::data(v, DEFAULT_FLAGS));
                self.grow(48);
                Ok(true)
            }
        }
    }

    pub(crate) fn array_set_index(&mut self, o: ObjId, i: u32, v: Value) -> JsResult<bool> {
        let (len, ext, integ) = match &self.obj(o).kind {
            ObjKind::Array(items) => (items.len(), self.obj(o).extensible, self.obj(o).integrity),
            _ => return Ok(false),
        };
        let i = i as usize;
        if integ == 2 || (i >= len && !ext) {
            return Ok(false);
        }
        if i >= len {
            let extra = (i + 1 - len) * 16;
            if extra > 4096 {
                self.check_alloc(extra).map_err(|t| self.thrown_to_exc(t))?;
            }
            self.grow(extra);
        }
        if let ObjKind::Array(items) = &mut self.obj_mut(o).kind {
            if i >= items.len() {
                items.resize(i + 1, None);
            }
            items[i] = Some(v);
        }
        Ok(true)
    }

    pub(crate) fn array_set_length(&mut self, o: ObjId, v: Value) -> JsResult<bool> {
        let n = self.to_num(v)?;
        if n < 0.0 || n.fract() != 0.0 || n > 4294967295.0 {
            return Err(self.range_error("Invalid array length"));
        }
        let n = n as usize;
        let (len, integ) = match &self.obj(o).kind {
            ObjKind::Array(items) => (items.len(), self.obj(o).integrity),
            _ => return Ok(false),
        };
        if integ == 2 && n != len {
            return Ok(false);
        }
        if n > len {
            self.check_alloc((n - len) * 16).map_err(|t| self.thrown_to_exc(t))?;
            self.grow((n - len) * 16);
        }
        if let ObjKind::Array(items) = &mut self.obj_mut(o).kind {
            items.resize(n, None);
        }
        Ok(true)
    }

    /// [[DefineOwnProperty]] with a fully specified property (simplified validation).
    pub(crate) fn define_own(&mut self, o: ObjId, k: PropKey, p: Prop) -> bool {
        if let (ObjKind::TypedArray(vd), Some(i)) = (&self.obj(o).kind, self.key_index(k)) {
            if vd.kind.is_big() {
                return !p.is_accessor() && self.typed_store(o, i as usize, p.value).is_ok();
            }
            let n = match p.value {
                _ if p.is_accessor() => return false,
                Value::Number(n) => n,
                other => match self.to_num(other) {
                    Ok(n) => n,
                    Err(_) => return false,
                },
            };
            return self.typed_put(o, i as usize, n);
        }
        if let ObjKind::Array(_) = self.obj(o).kind {
            if let Some(i) = self.key_index(k) {
                if !p.is_accessor() {
                    return self.array_set_index(o, i, p.value).unwrap_or(false);
                }
            }
            if k == PropKey::Str(A_LENGTH) {
                let ok = self.array_set_length(o, p.value).unwrap_or(false);
                if p.flags & WRITABLE == 0 {
                    self.obj_mut(o).integrity = 2;
                }
                return ok;
            }
        }
        if matches!(self.obj(o).kind, ObjKind::Arguments(Some(_))) {
            // defining a value writes through; an accessor or a read-only value ends the aliasing
            if !p.is_accessor() {
                self.arg_write(o, k, p.value);
            }
            if p.is_accessor() || p.flags & WRITABLE == 0 {
                self.arg_unmap(o, k);
            }
        }
        let existing = self.obj(o).props.get(k).copied();
        if let Some(e) = existing {
            if e.flags & CONFIGURABLE == 0 {
                // only a writable data value may change
                if e.is_accessor() || p.is_accessor() || e.flags & WRITABLE == 0 {
                    return self.same_value(e.value, p.value) && e.flags == p.flags;
                }
                if let Some(ep) = self.obj_mut(o).props.get_mut(k) {
                    ep.value = p.value;
                    ep.flags = p.flags & !CONFIGURABLE & !ENUMERABLE | (e.flags & ENUMERABLE);
                }
                return true;
            }
        } else if !self.obj(o).extensible {
            return false;
        }
        if self.obj_mut(o).props.insert(k, p) {
            self.grow(48);
        }
        true
    }

    pub(crate) fn define_value(&mut self, o: ObjId, k: PropKey, v: Value, flags: u8) {
        self.define_own(o, k, Prop::data(v, flags));
    }

    pub(crate) fn delete_prop(&mut self, o: ObjId, k: PropKey) -> bool {
        let integ = self.obj(o).integrity;
        if matches!(self.obj(o).kind, ObjKind::Array(_)) {
            if let Some(i) = self.key_index(k) {
                if integ > 0 {
                    return false;
                }
                if let ObjKind::Array(items) = &mut self.obj_mut(o).kind {
                    if let Some(slot) = items.get_mut(i as usize) {
                        *slot = None;
                    }
                }
                return true;
            }
            if k == PropKey::Str(A_LENGTH) {
                return false;
            }
        }
        if let ObjKind::String(s) = self.obj(o).kind {
            if k == PropKey::Str(A_LENGTH) || self.key_index(k).map(|i| (i as usize) < self.str_units(s).len()).unwrap_or(false) {
                return false;
            }
        }
        if let (ObjKind::TypedArray(_), Some(i)) = (&self.obj(o).kind, self.key_index(k)) {
            return self.typed_get(o, i as usize).is_none();
        }
        match self.obj(o).props.get(k) {
            Some(p) if p.flags & CONFIGURABLE == 0 => false,
            Some(_) => {
                self.arg_unmap(o, k);
                self.obj_mut(o).props.remove(k);
                true
            }
            None => true,
        }
    }

    pub(crate) fn has_property(&mut self, o: ObjId, k: PropKey) -> JsResult<bool> {
        let mut cur = Some(o);
        let mut n = 0;
        while let Some(c) = cur {
            match self.obj(c).kind {
                ObjKind::Host(class, id) => {
                    if let (PropKey::Str(a), Some(cl)) = (k, self.host_hooks(class)) {
                        let key = String::from_utf16_lossy(self.atom_units(a));
                        if self.call_host(|rt| (cl.get)(rt, id, &key))?.is_some() {
                            return Ok(true);
                        }
                    }
                }
                ObjKind::Proxy(p) => return self.proxy_has(p, k),
                _ => {}
            }
            if self.get_own_prop(c, k).is_some() {
                return Ok(true);
            }
            cur = self.obj(c).proto;
            n += 1;
            if n & 15 == 15 {
                self.steps += 1;
                if n > 100_000 || self.steps > self.limits.step_budget {
                    break;
                }
            }
        }
        Ok(false)
    }

    /// Own property keys in spec order: integer indices, strings (insertion), symbols.
    pub(crate) fn own_keys(&mut self, o: ObjId) -> Vec<PropKey> {
        let mut idx: Vec<(u32, PropKey)> = Vec::new();
        let mut strs = Vec::new();
        let mut syms = Vec::new();
        let count = match &self.obj(o).kind {
            ObjKind::Array(items) => items.iter().enumerate().filter(|(_, v)| v.is_some()).map(|(i, _)| i as u32).collect::<Vec<u32>>(),
            ObjKind::String(s) => (0..self.str_units(*s).len() as u32).collect(),
            ObjKind::TypedArray(_) => (0..self.typed_view(Value::Object(o)).map(|v| v.length.min(u32::MAX as usize) as u32).unwrap_or(0)).collect(),
            _ => Vec::new(),
        };
        for i in count {
            let a = self.intern(&i.to_string());
            idx.push((i, PropKey::Str(a)));
        }
        let entries: Vec<PropKey> = self.obj(o).props.iter().map(|(k, _)| *k).collect();
        for k in entries {
            match k {
                PropKey::Str(a) => match self.atom_array_index(a) {
                    Some(i) => idx.push((i, k)),
                    None => strs.push(k),
                },
                PropKey::Sym(_) if k.is_private() => {}
                PropKey::Sym(_) => syms.push(k),
            }
        }
        if matches!(self.obj(o).kind, ObjKind::Array(_)) {
            strs.insert(0, PropKey::Str(A_LENGTH));
        } else if matches!(self.obj(o).kind, ObjKind::String(_)) {
            strs.insert(0, PropKey::Str(A_LENGTH));
        }
        idx.sort_by_key(|(i, _)| *i);
        let mut out: Vec<PropKey> = idx.into_iter().map(|(_, k)| k).collect();
        out.extend(strs);
        out.extend(syms);
        out
    }

    /// Own enumerable string-keyed properties, in order.
    pub(crate) fn own_enum_keys(&mut self, o: ObjId) -> Vec<PropKey> {
        let keys = self.own_keys(o);
        keys.into_iter()
            .filter(|k| matches!(k, PropKey::Str(_)))
            .filter(|k| self.get_own_prop(o, *k).map(|p| p.flags & ENUMERABLE != 0).unwrap_or(false))
            .collect()
    }

    pub(crate) fn array_len(&self, o: ObjId) -> Option<usize> {
        match &self.obj(o).kind {
            ObjKind::Array(items) => Some(items.len()),
            _ => None,
        }
    }

    /// LengthOfArrayLike.
    pub(crate) fn length_of(&mut self, v: Value) -> JsResult<u64> {
        if let Value::Object(o) = v {
            if let Some(n) = self.array_len(o) {
                return Ok(n as u64);
            }
            if let Some(view) = self.typed_view(v) {
                return Ok(view.length as u64);
            }
        }
        let l = self.get_prop(v, PropKey::Str(A_LENGTH))?;
        let n = crate::ops::to_integer(self.to_num(l)?);
        Ok(if n <= 0.0 { 0 } else { n.min(9007199254740991.0) as u64 })
    }

    pub(crate) fn get_idx(&mut self, v: Value, i: u64) -> JsResult<Value> {
        if let Value::Object(o) = v {
            if let ObjKind::Array(items) = &self.obj(o).kind {
                if let Some(Some(x)) = items.get(i as usize) {
                    return Ok(*x);
                }
            }
            if let ObjKind::TypedArray(_) = self.obj(o).kind {
                // no property key is built (and interned) for an element read
                return Ok(self.typed_value(o, i as usize).unwrap_or(Value::Undefined));
            }
        }
        let k = self.index_key(i);
        self.get_prop(v, k)
    }

    pub(crate) fn index_key(&mut self, i: u64) -> PropKey {
        PropKey::Str(self.intern(&i.to_string()))
    }
}
