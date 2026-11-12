//! Allocation, handle access, string/atom tables and the mark-and-sweep collector.

use crate::runtime::*;
use crate::value::*;
use std::rc::Rc;

fn array_index_of(units: &[u16]) -> u32 {
    if units.is_empty() || units.len() > 10 {
        return u32::MAX;
    }
    if units.len() > 1 && units[0] == b'0' as u16 {
        return u32::MAX;
    }
    let mut v: u64 = 0;
    for &c in units {
        if !(b'0' as u16..=b'9' as u16).contains(&c) {
            return u32::MAX;
        }
        v = v * 10 + (c - b'0' as u16) as u64;
    }
    if v >= u32::MAX as u64 { u32::MAX } else { v as u32 }
}

impl Runtime {
    // ---- atoms ----
    pub(crate) fn intern16(&mut self, units: &[u16]) -> Atom {
        if let Some(&i) = self.atom_map.get(units) {
            return Atom(i);
        }
        let rc: Rc<[u16]> = units.into();
        let i = self.atoms.len() as u32;
        self.atoms.push(rc.clone());
        self.atom_map.insert(rc, i);
        self.atom_index.push(array_index_of(units));
        self.permanent_bytes += 48 + units.len() * 4;
        Atom(i)
    }

    pub(crate) fn intern(&mut self, s: &str) -> Atom {
        let u: Vec<u16> = s.encode_utf16().collect();
        self.intern16(&u)
    }

    pub(crate) fn atom_units(&self, a: Atom) -> &[u16] {
        self.atoms.get(a.0 as usize).map(|r| &r[..]).unwrap_or(&[])
    }

    pub(crate) fn atom_array_index(&self, a: Atom) -> Option<u32> {
        match self.atom_index.get(a.0 as usize) {
            Some(&i) if i != u32::MAX => Some(i),
            _ => None,
        }
    }

    pub(crate) fn key_str(&mut self, s: &str) -> PropKey {
        PropKey::Str(self.intern(s))
    }

    // ---- strings ----
    pub(crate) fn str_units(&self, s: StrId) -> &[u16] {
        match s.as_atom() {
            Some(a) => self.atom_units(a),
            None => match self.strs.get(s.0 as usize) {
                Some(Some(r)) => r,
                _ => &[],
            },
        }
    }

    pub(crate) fn str_rc(&self, s: StrId) -> Rc<[u16]> {
        match s.as_atom() {
            Some(a) => self.atoms.get(a.0 as usize).cloned().unwrap_or_else(|| Rc::from(Vec::new())),
            None => match self.strs.get(s.0 as usize) {
                Some(Some(r)) => r.clone(),
                _ => Rc::from(Vec::new()),
            },
        }
    }

    pub(crate) fn alloc_str_rc(&mut self, units: Rc<[u16]>) -> StrId {
        self.heap_bytes += 40 + units.len() * 2;
        let id = match self.free_strs.pop() {
            Some(i) => {
                self.strs[i as usize] = Some(units);
                StrId(i)
            }
            None => {
                self.strs.push(Some(units));
                StrId(self.strs.len() as u32 - 1)
            }
        };
        if self.native_active {
            self.scratch.push(Value::String(id));
        }
        self.note_alloc();
        id
    }

    /// Creates a string value. Short strings are interned to avoid churn.
    pub(crate) fn new_str16(&mut self, units: Vec<u16>) -> Value {
        if units.len() <= 1 {
            return Value::String(StrId::from_atom(self.intern16(&units)));
        }
        Value::String(self.alloc_str_rc(units.into()))
    }

    pub(crate) fn new_str(&mut self, s: &str) -> Value {
        self.new_str16(s.encode_utf16().collect())
    }

    pub(crate) fn atom_value(&self, a: Atom) -> Value {
        Value::String(StrId::from_atom(a))
    }

    pub(crate) fn rust_string(&self, s: StrId) -> String {
        String::from_utf16_lossy(self.str_units(s))
    }

    // ---- objects ----
    pub(crate) fn obj(&self, id: ObjId) -> &Object {
        match self.objs.get(id.0 as usize) {
            Some(Some(o)) => o,
            _ => self.objs[0].as_ref().expect("dummy object"),
        }
    }

    pub(crate) fn obj_mut(&mut self, id: ObjId) -> &mut Object {
        let idx = match self.objs.get(id.0 as usize) {
            Some(Some(_)) => id.0 as usize,
            _ => 0,
        };
        self.objs[idx].as_mut().expect("dummy object")
    }

    pub(crate) fn alloc(&mut self, o: Object) -> ObjId {
        self.heap_bytes += o.size_estimate();
        let id = match self.free_objs.pop() {
            Some(i) => {
                self.objs[i as usize] = Some(o);
                ObjId(i)
            }
            None => {
                self.objs.push(Some(o));
                ObjId(self.objs.len() as u32 - 1)
            }
        };
        if self.native_active {
            self.scratch.push(Value::Object(id));
        }
        self.note_alloc();
        id
    }

    pub(crate) fn new_object(&mut self, proto: Option<ObjId>) -> ObjId {
        self.alloc(Object::new(proto, ObjKind::Ordinary))
    }

    pub(crate) fn plain_object(&mut self) -> ObjId {
        let p = self.intr.object_proto;
        self.new_object(p)
    }

    pub(crate) fn new_array(&mut self, items: Vec<Option<Value>>) -> ObjId {
        let p = self.intr.array_proto;
        self.alloc(Object::new(p, ObjKind::Array(items)))
    }

    pub(crate) fn new_array_values(&mut self, items: &[Value]) -> Value {
        Value::Object(self.new_array(items.iter().map(|v| Some(*v)).collect()))
    }

    // ---- BigInts ----
    pub(crate) fn alloc_big(&mut self, b: crate::bigint::BigInt) -> Value {
        self.heap_bytes += 48 + b.mag.len() * 4;
        let id = match self.free_bigs.pop() {
            Some(i) => {
                self.bigs[i as usize] = Some(Rc::new(b));
                BigId(i)
            }
            None => {
                self.bigs.push(Some(Rc::new(b)));
                BigId(self.bigs.len() as u32 - 1)
            }
        };
        if self.native_active {
            self.scratch.push(Value::BigInt(id));
        }
        self.note_alloc();
        Value::BigInt(id)
    }

    pub(crate) fn big_ref(&self, id: BigId) -> Option<&crate::bigint::BigInt> {
        self.bigs.get(id.0 as usize).and_then(|b| b.as_deref())
    }

    /// Charges work linear in a BigInt of `limbs` limbs from a `&self` context: recorded
    /// now, added to the step count at the next safepoint.
    pub(crate) fn big_note(&self, limbs: usize) {
        self.pending_steps.set(self.pending_steps.get().saturating_add(limbs as u64 / 8));
    }

    /// The integer behind a handle (zero for a stale handle).
    pub(crate) fn big(&self, id: BigId) -> Rc<crate::bigint::BigInt> {
        self.bigs.get(id.0 as usize).cloned().flatten().unwrap_or_default()
    }

    pub(crate) fn alloc_env(&mut self, parent: Option<EnvId>, n: u32) -> EnvId {
        self.heap_bytes += 32 + n as usize * 16;
        let e = Env { parent, slots: vec![None; n as usize] };
        let id = match self.free_envs.pop() {
            Some(i) => {
                self.envs[i as usize] = Some(e);
                EnvId(i)
            }
            None => {
                self.envs.push(Some(e));
                EnvId(self.envs.len() as u32 - 1)
            }
        };
        self.note_alloc();
        id
    }

    pub(crate) fn env(&self, id: EnvId) -> Option<&Env> {
        self.envs.get(id.0 as usize).and_then(|e| e.as_ref())
    }

    pub(crate) fn env_mut(&mut self, id: EnvId) -> Option<&mut Env> {
        self.envs.get_mut(id.0 as usize).and_then(|e| e.as_mut())
    }

    pub(crate) fn note_alloc(&mut self) {
        if self.heap_bytes + self.permanent_bytes > self.gc_threshold {
            self.gc_requested = true;
        }
    }

    pub(crate) fn grow(&mut self, bytes: usize) {
        self.heap_bytes += bytes;
        self.note_alloc();
    }

    /// Fails (by arming an uncatchable abort) if allocating `bytes` more would exceed the cap.
    pub(crate) fn check_alloc(&mut self, bytes: usize) -> Result<(), crate::Thrown> {
        if self.heap_bytes.saturating_add(self.permanent_bytes).saturating_add(bytes) > self.limits.max_heap_bytes {
            self.pending_abort = Some(ErrorKind::OutOfMemory);
            return Err(crate::Thrown(Value::Undefined));
        }
        Ok(())
    }

    pub(crate) fn total_heap(&self) -> usize {
        self.heap_bytes + self.permanent_bytes
    }

    pub(crate) fn measure_heap(&self) -> usize {
        let mut n = self.objs.len() * 8 + self.strs.len() * 8 + self.envs.len() * 8 + self.jobs.len() * 64;
        // source text kept alive by closures of script-compiled code, counted once per text
        let mut sources: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for o in self.objs.iter().flatten() {
            n += o.size_estimate();
            if let ObjKind::Function(f) = &o.kind {
                if let FuncData::Closure(c) = &**f {
                    if c.code.dynamic && sources.insert(c.code.source.as_ptr() as usize) {
                        n += c.code.source.len() * 4;
                    }
                }
            }
        }
        for s in self.strs.iter().flatten() {
            n += 40 + s.len() * 2;
        }
        for e in self.envs.iter().flatten() {
            n += 32 + e.slots.len() * 16;
        }
        for p in self.regex_cache.values() {
            n += 64 + p.bytes / Rc::strong_count(p).max(1);
        }
        n += self.bigs.len() * 8;
        for b in self.bigs.iter().flatten() {
            n += 48 + b.mag.len() * 4;
        }
        n
    }

    // ---- garbage collection ----
    pub(crate) fn collect_garbage(&mut self) {
        let mut mo = vec![false; self.objs.len()];
        let mut me = vec![false; self.envs.len()];
        let mut ms = vec![false; self.strs.len()];
        let mut mb = vec![false; self.bigs.len()];
        let mut work_o: Vec<u32> = Vec::new();
        let mut work_e: Vec<u32> = Vec::new();
        let mut weak_objs: Vec<u32> = Vec::new();
        let mut weak_refs: Vec<u32> = Vec::new();
        let mut busy_maps: std::collections::HashSet<u32> = std::collections::HashSet::new();
        mo[0] = true;

        let mut roots: Vec<Value> = Vec::new();
        let mut env_roots: Vec<EnvId> = Vec::new();
        roots.push(Value::Object(self.global));
        let i = self.intr;
        for p in [
            i.object_proto, i.function_proto, i.array_proto, i.string_proto, i.number_proto, i.boolean_proto,
            i.symbol_proto, i.error_proto, i.type_error_proto, i.range_error_proto, i.syntax_error_proto,
            i.reference_error_proto, i.eval_error_proto, i.uri_error_proto, i.iterator_proto, i.array_iter_proto,
            i.string_iter_proto, i.regexp_proto, i.array_values, self.stack_overflow_error,
            i.promise_proto, i.aggregate_error_proto, i.generator_proto, i.async_generator_proto,
            i.async_from_sync_proto, i.gen_return_marker, i.map_proto, i.set_proto, i.map_iter_proto,
            i.set_iter_proto, i.weakmap_proto, i.weakset_proto, i.regexp_ctor, i.regexp_exec, i.regexp_iter_proto,
            i.date_proto, i.array_buffer_proto, i.data_view_proto, i.error_ctor, i.eval_fn,
            i.species_getter, i.array_ctor, i.promise_ctor, i.from_async, i.weakref_proto, i.finreg_proto,
        ]
        .into_iter()
        .chain(i.typed_protos)
        .chain(i.typed_ctors)
        .flatten()
        {
            roots.push(Value::Object(p));
        }
        for j in &self.jobs {
            j.trace(&mut |v| roots.push(v));
        }
        roots.extend(self.pending_rejections.iter().map(|o| Value::Object(*o)));
        roots.extend(self.stack.iter().copied());
        roots.extend(self.locals.iter().flatten().copied());
        roots.extend(self.roots.values().copied());
        roots.extend(self.scratch.iter().copied());
        roots.push(self.native_new_target);
        roots.extend(self.host_objs.values().map(|o| Value::Object(*o)));
        roots.extend(self.template_cache.values().map(|(_, o)| Value::Object(*o)));
        roots.extend(self.global_lex.values().filter_map(|g| g.value));
        roots.extend(self.big_literals.values().map(|b| Value::BigInt(*b)));
        self.modules.trace(&mut |v| roots.push(v), &mut |e| env_roots.push(e));
        for f in &self.frames {
            f.trace(&mut |v| roots.push(v), &mut |e| env_roots.push(e));
        }
        for h in &self.handlers {
            if let Some(e) = h.env {
                env_roots.push(e);
            }
        }

        fn mark_val(v: Value, mo: &mut [bool], ms: &mut [bool], mb: &mut [bool], wo: &mut Vec<u32>) {
            match v {
                Value::BigInt(b) => {
                    if let Some(m) = mb.get_mut(b.0 as usize) {
                        *m = true;
                    }
                }
                Value::Object(o) => {
                    let i = o.0 as usize;
                    if i < mo.len() && !mo[i] {
                        mo[i] = true;
                        wo.push(o.0);
                    }
                }
                Value::String(s) => {
                    if s.as_atom().is_none() {
                        if let Some(m) = ms.get_mut(s.0 as usize) {
                            *m = true;
                        }
                    }
                }
                _ => {}
            }
        }
        fn mark_env(e: EnvId, me: &mut [bool], we: &mut Vec<u32>) {
            let i = e.0 as usize;
            if i < me.len() && !me[i] {
                me[i] = true;
                we.push(e.0);
            }
        }

        for v in roots {
            mark_val(v, &mut mo, &mut ms, &mut mb, &mut work_o);
        }
        for e in env_roots {
            mark_env(e, &mut me, &mut work_e);
        }
        loop {
            if let Some(oi) = work_o.pop() {
                let Some(Some(o)) = self.objs.get(oi as usize) else { continue };
                if let Some(p) = o.proto {
                    mark_val(Value::Object(p), &mut mo, &mut ms, &mut mb, &mut work_o);
                }
                for (k, p) in o.props.iter() {
                    let _ = k;
                    mark_val(p.value, &mut mo, &mut ms, &mut mb, &mut work_o);
                    mark_val(p.setter, &mut mo, &mut ms, &mut mb, &mut work_o);
                }
                match &o.kind {
                    ObjKind::Array(items) => {
                        for v in items.iter().flatten() {
                            mark_val(*v, &mut mo, &mut ms, &mut mb, &mut work_o);
                        }
                    }
                    ObjKind::Function(f) => match &**f {
                        FuncData::Closure(c) => {
                            if let Some(e) = c.env {
                                mark_env(e, &mut me, &mut work_e);
                            }
                            mark_val(c.this_val, &mut mo, &mut ms, &mut mb, &mut work_o);
                            mark_val(c.new_target, &mut mo, &mut ms, &mut mb, &mut work_o);
                            for x in [c.home, c.ctor_callee, c.fields].into_iter().flatten() {
                                mark_val(Value::Object(x), &mut mo, &mut ms, &mut mb, &mut work_o);
                            }
                        }
                        FuncData::Native { .. } | FuncData::GenMethod(_) => {}
                        FuncData::Bound { target, this, args } => {
                            mark_val(Value::Object(*target), &mut mo, &mut ms, &mut mb, &mut work_o);
                            mark_val(*this, &mut mo, &mut ms, &mut mb, &mut work_o);
                            for a in args {
                                mark_val(*a, &mut mo, &mut ms, &mut mb, &mut work_o);
                            }
                        }
                        FuncData::Intern { slots, .. } => {
                            for a in slots {
                                mark_val(*a, &mut mo, &mut ms, &mut mb, &mut work_o);
                            }
                        }
                    },
                    ObjKind::String(s) => mark_val(Value::String(*s), &mut mo, &mut ms, &mut mb, &mut work_o),
                    ObjKind::ArrayIter { target, .. } => mark_val(*target, &mut mo, &mut ms, &mut mb, &mut work_o),
                    ObjKind::StringIter { s, .. } => mark_val(Value::String(*s), &mut mo, &mut ms, &mut mb, &mut work_o),
                    ObjKind::ForIn { keys, obj, .. } => {
                        for k in keys {
                            mark_val(Value::String(*k), &mut mo, &mut ms, &mut mb, &mut work_o);
                        }
                        mark_val(*obj, &mut mo, &mut ms, &mut mb, &mut work_o);
                    }
                    ObjKind::MapIter { map, .. } => {
                        // a live iterator pins its collection's entry positions
                        busy_maps.insert(map.0);
                        mark_val(Value::Object(*map), &mut mo, &mut ms, &mut mb, &mut work_o);
                    }
                    // weak entries are not traced here: see the ephemeron pass below
                    ObjKind::WeakMap(_) | ObjKind::WeakSet(_) => weak_objs.push(oi),
                    ObjKind::WeakRef(_) => weak_refs.push(oi),
                    ObjKind::BigInt(b) => mark_val(Value::BigInt(*b), &mut mo, &mut ms, &mut mb, &mut work_o),
                    ObjKind::FinReg(d) => {
                        // the callback and the held values are strong; targets and tokens are not
                        mark_val(d.callback, &mut mo, &mut ms, &mut mb, &mut work_o);
                        for c in &d.cells {
                            mark_val(c.held, &mut mo, &mut ms, &mut mb, &mut work_o);
                        }
                        weak_refs.push(oi);
                    }
                    other => other.trace_extra(&mut |v| mark_val(v, &mut mo, &mut ms, &mut mb, &mut work_o), &mut |e| mark_env(e, &mut me, &mut work_e)),
                }
            } else if let Some(ei) = work_e.pop() {
                let Some(Some(e)) = self.envs.get(ei as usize) else { continue };
                if let Some(p) = e.parent {
                    mark_env(p, &mut me, &mut work_e);
                }
                for v in e.slots.iter().flatten() {
                    mark_val(*v, &mut mo, &mut ms, &mut mb, &mut work_o);
                }
            } else {
                // ephemerons: a weak entry's value is alive only while its key is
                for &wi in &weak_objs {
                    let Some(Some(o)) = self.objs.get(wi as usize) else { continue };
                    if let ObjKind::WeakMap(m) | ObjKind::WeakSet(m) = &o.kind {
                        for (k, v) in m.iter() {
                            if mo.get(*k as usize).copied().unwrap_or(false) {
                                mark_val(*v, &mut mo, &mut ms, &mut mb, &mut work_o);
                            }
                        }
                    }
                }
                if work_o.is_empty() {
                    break;
                }
            }
        }
        for wi in weak_objs {
            if let Some(Some(o)) = self.objs.get_mut(wi as usize) {
                if let ObjKind::WeakMap(m) | ObjKind::WeakSet(m) = &mut o.kind {
                    m.retain(|k, _| mo.get(*k as usize).copied().unwrap_or(false));
                }
            }
        }
        // weak references to dead targets are cleared; registries get a cleanup job per dead target
        let alive = |o: ObjId| mo.get(o.0 as usize).copied().unwrap_or(false);
        let mut cleanups: Vec<Job> = Vec::new();
        for wi in weak_refs {
            match self.objs.get_mut(wi as usize).and_then(|o| o.as_mut()).map(|o| &mut o.kind) {
                Some(ObjKind::WeakRef(t)) => {
                    if matches!(*t, Some(x) if !alive(x)) {
                        *t = None;
                    }
                }
                Some(ObjKind::FinReg(d)) => {
                    let callback = d.callback;
                    d.cells.retain_mut(|c| {
                        if matches!(c.token, Some(t) if !alive(t)) {
                            c.token = None;
                        }
                        if alive(c.target) {
                            return true;
                        }
                        cleanups.push(Job::Cleanup { callback, held: c.held });
                        false
                    });
                }
                _ => {}
            }
        }
        self.jobs.extend(cleanups);
        for (i, m) in mo.iter().enumerate() {
            match &mut self.objs[i] {
                Some(_) if !m => {
                    self.objs[i] = None;
                    self.free_objs.push(i as u32);
                }
                Some(o) => {
                    if let ObjKind::Map(d) | ObjKind::Set(d) = &mut o.kind {
                        if d.entries.len() > d.live && !busy_maps.contains(&(i as u32)) {
                            d.compact();
                        }
                    }
                }
                None => {}
            }
        }
        for (i, m) in me.iter().enumerate() {
            if !m && self.envs[i].is_some() {
                self.envs[i] = None;
                self.free_envs.push(i as u32);
            }
        }
        for (i, m) in ms.iter().enumerate() {
            if !m && self.strs[i].is_some() {
                self.strs[i] = None;
                self.free_strs.push(i as u32);
            }
        }
        for (i, m) in mb.iter().enumerate() {
            if !m && self.bigs[i].is_some() {
                self.bigs[i] = None;
                self.free_bigs.push(i as u32);
            }
        }
        // compiled patterns no live RegExp uses stay cached only while they are small in total
        let idle: usize = self.regex_cache.values().filter(|p| Rc::strong_count(p) == 1).map(|p| p.bytes).sum();
        if idle > (256 << 10) || self.regex_cache.len() > 512 {
            self.regex_cache.retain(|_, p| Rc::strong_count(p) > 1);
        }
        self.heap_bytes = self.measure_heap();
        let live = self.total_heap();
        let max = self.limits.max_heap_bytes;
        self.gc_threshold = (live * 2).max(4 << 20).min(max.max(live + (1 << 20)));
        self.gc_requested = false;
    }
}
