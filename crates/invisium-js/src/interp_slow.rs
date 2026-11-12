//! Less frequent bytecode operations.

use crate::bytecode::Op;
use crate::runtime::*;
use crate::value::*;
use crate::vm::Handler;

impl Runtime {
    /// Returns true when a new frame was pushed.
    pub(crate) fn exec_slow(&mut self, op: Op, fi: usize, atoms: &[Atom], strict: bool) -> JsResult<bool> {
        let atom = |n: u32| atoms.get(n as usize).copied().unwrap_or(A_EMPTY);
        match op {
            Op::Regex(p, f) => {
                let (p, f) = (atom(p), atom(f));
                let r = self.regexp_literal(p, f)?;
                self.stack.push(r);
            }
            Op::GetGlobalTypeof(n) => {
                let v = self.get_global(atom(n), true)?;
                self.stack.push(v);
            }
            Op::PutGlobal(n) => {
                let v = self.pop();
                self.put_global(atom(n), v, strict)?;
            }
            Op::DeleteGlobal(n) => {
                let k = PropKey::Str(atom(n));
                let g = self.global;
                let r = self.delete_prop(g, k);
                self.stack.push(Value::Bool(r));
            }
            Op::DeclareVar(n) => {
                let k = PropKey::Str(atom(n));
                let g = self.global;
                if self.get_own_prop(g, k).is_none() {
                    self.define_value(g, k, Value::Undefined, WRITABLE | ENUMERABLE);
                }
            }
            Op::DeclareFunc(n) => {
                let v = self.pop();
                let k = PropKey::Str(atom(n));
                let g = self.global;
                match self.get_own_prop(g, k) {
                    Some(p) if p.flags & CONFIGURABLE == 0 && !p.is_accessor() => {
                        self.set_prop(Value::Object(g), k, v, Value::Object(g))?;
                    }
                    _ => {
                        self.define_value(g, k, v, WRITABLE | ENUMERABLE);
                    }
                }
            }
            Op::DeclareLex(n, is_const) => {
                let a = atom(n);
                if self.global_lex.contains_key(&a) {
                    let name = String::from_utf16_lossy(self.atom_units(a));
                    let e = self.make_error("SyntaxError", &format!("Identifier '{}' has already been declared", name));
                    return Err(Exc::Throw(e));
                }
                self.global_lex.insert(a, GlobalLex { value: None, mutable: !is_const });
            }
            Op::InitGlobalLex(n) => {
                let v = self.pop();
                let a = atom(n);
                let mutable = self.global_lex.get(&a).map(|g| g.mutable).unwrap_or(true);
                self.global_lex.insert(a, GlobalLex { value: Some(v), mutable });
            }
            Op::ThrowConstAssign(_) => return Err(self.type_error("Assignment to constant variable.")),
            Op::DeleteProp(n) => {
                let o = self.pop();
                let r = self.delete_value(o, PropKey::Str(atom(n)), strict)?;
                self.stack.push(Value::Bool(r));
            }
            Op::DeleteElem => {
                let k = self.pop();
                let o = self.pop();
                let key = self.to_property_key(k)?;
                let r = self.delete_value(o, key, strict)?;
                self.stack.push(Value::Bool(r));
            }
            Op::SuperGetElem => {
                let (this, k) = (self.peek(1), self.peek(0));
                let key = self.to_property_key(k)?;
                let home = self.frames[fi].home;
                let proto = home.and_then(|h| self.obj(h).proto);
                let v = match proto {
                    Some(p) => self.get_from(p, key, this)?,
                    None => Value::Undefined,
                };
                let l = self.stack.len();
                self.stack.truncate(l.saturating_sub(2));
                self.stack.push(v);
            }
            Op::SuperPutElem => {
                let (this, k, v) = (self.peek(2), self.peek(1), self.peek(0));
                let key = self.to_property_key(k)?;
                let home = self.frames[fi].home;
                match home.and_then(|h| self.obj(h).proto) {
                    Some(p) => {
                        self.set_prop(Value::Object(p), key, v, this)?;
                    }
                    None => {
                        self.set_prop(this, key, v, this)?;
                    }
                }
                let l = self.stack.len();
                self.stack.truncate(l.saturating_sub(3));
                self.stack.push(v);
            }
            Op::ToPropertyKey => {
                let k = self.peek(0);
                if !matches!(k, Value::String(_) | Value::Symbol(_) | Value::Number(_)) {
                    let key = self.to_property_key(k)?;
                    let v = self.key_to_value(key);
                    if let Some(t) = self.stack.last_mut() {
                        *t = v;
                    }
                }
            }
            Op::In => {
                let (k, o) = (self.peek(1), self.peek(0));
                let Value::Object(oid) = o else {
                    return Err(self.type_error("Cannot use 'in' operator to search for a key in a non-object"));
                };
                let key = self.to_property_key(k)?;
                let r = self.has_property(oid, key)?;
                self.pop2_push(Value::Bool(r));
            }
            Op::InstanceOf => {
                let (v, t) = (self.peek(1), self.peek(0));
                let r = self.instance_of(v, t)?;
                self.pop2_push(Value::Bool(r));
            }
            Op::Neg | Op::Plus | Op::BitNot => {
                let v = self.peek(0);
                let r = match (op, v) {
                    (Op::Neg, Value::Number(n)) => Value::Number(-n),
                    // unary plus is ToNumber: it rejects BigInts
                    (Op::Plus, _) => Value::Number(self.to_num(v)?),
                    _ => self.numeric_unary(op, v)?,
                };
                if let Some(t) = self.stack.last_mut() {
                    *t = r;
                }
            }
            Op::TypeOf => {
                let v = self.pop();
                let s = self.typeof_str(v);
                let a = self.intern(s);
                self.stack.push(self.atom_value(a));
            }
            Op::ToStr => {
                let s = self.to_str(self.peek(0))?;
                if let Some(t) = self.stack.last_mut() {
                    *t = Value::String(s);
                }
            }
            Op::JumpIfObject(t) => {
                if matches!(self.peek(0), Value::Object(_)) {
                    self.frames[fi].pc = t as usize;
                }
            }
            Op::CheckObjectCoercible => {
                if self.peek(0).is_nullish() {
                    let what = if self.peek(0) == Value::Undefined { "undefined" } else { "null" };
                    return Err(self.type_error(&format!("Cannot destructure '{}' as it is {}.", what, what)));
                }
            }
            Op::CallArr | Op::CallMethodArr => {
                let arr = self.pop();
                let items = self.array_items(arr);
                let fpos = self.stack.len().saturating_sub(1);
                let f = self.peek(0);
                let (this, ret_sp) = if op == Op::CallMethodArr { (self.peek(1), fpos.saturating_sub(1)) } else { (Value::Undefined, fpos) };
                self.stack.extend(items);
                return self.invoke(f, this, fpos + 1, ret_sp);
            }
            Op::NewArr => {
                let arr = self.pop();
                let items = self.array_items(arr);
                let fpos = self.stack.len().saturating_sub(1);
                self.stack.extend(items);
                return self.op_new(fpos);
            }
            Op::SuperCall(_) | Op::SuperCallArr => {
                let args: Vec<Value> = if let Op::SuperCall(n) = op {
                    let l = self.stack.len();
                    let a = self.stack[l.saturating_sub(n as usize)..].to_vec();
                    a
                } else {
                    let arr = self.peek(0);
                    self.array_items(arr)
                };
                let base = match op {
                    Op::SuperCall(n) => self.stack.len().saturating_sub(n as usize),
                    _ => self.stack.len().saturating_sub(1),
                };
                let f = &self.frames[fi];
                let (ctor, nt) = (f.ctor_callee, f.new_target);
                let parent = ctor.and_then(|c| self.obj(c).proto);
                let Some(parent) = parent else { return Err(self.type_error("Super constructor is not a constructor")) };
                if !self.is_constructor(Value::Object(parent)) {
                    return Err(self.type_error("Super constructor is not a constructor"));
                }
                let this = self.construct_value(Value::Object(parent), &args, nt)?;
                if let Some(c) = ctor {
                    self.run_field_init(c, this)?;
                }
                self.stack.truncate(base);
                self.stack.push(this);
            }
            Op::RunFieldInit => {
                let f = &self.frames[fi];
                if let Some(c) = f.ctor_callee {
                    let this = f.this;
                    self.run_field_init(c, this)?;
                }
            }
            Op::LoadThis => {
                let t = self.frames[fi].this;
                self.stack.push(t);
            }
            Op::LoadNewTarget => {
                let t = self.frames[fi].new_target;
                self.stack.push(t);
            }
            Op::LoadCallee => {
                let c = self.frames[fi].callee.map(Value::Object).unwrap_or(Value::Undefined);
                self.stack.push(c);
            }
            Op::CreateArguments => {
                let f = &self.frames[fi];
                let args: Vec<Value> = self.stack.get(f.args_base..f.args_base + f.argc as usize).map(|s| s.to_vec()).unwrap_or_default();
                let callee = f.callee;
                let o = self.make_arguments(&args, callee);
                self.stack.push(Value::Object(o));
            }
            Op::MapArgument(i, slot) => {
                let (env, argc) = (self.frames[fi].env, self.frames[fi].argc);
                if let (Some(env), Value::Object(o), true) = (env, self.peek(0), i < argc) {
                    if let ObjKind::Arguments(m) = &mut self.obj_mut(o).kind {
                        let m = m.get_or_insert_with(|| Box::new(ArgMap { env, slots: Vec::new() }));
                        if m.slots.len() <= i as usize {
                            m.slots.resize(i as usize + 1, u32::MAX);
                        }
                        m.slots[i as usize] = slot;
                    }
                }
            }
            Op::RestArgs(i) => {
                let f = &self.frames[fi];
                let start = f.args_base + i as usize;
                let end = f.args_base + f.argc as usize;
                let items: Vec<Value> = if start < end { self.stack[start..end].to_vec() } else { Vec::new() };
                let a = self.new_array_values(&items);
                self.stack.push(a);
            }
            Op::NewObject => {
                let o = self.plain_object();
                self.stack.push(Value::Object(o));
            }
            Op::NewArray => {
                let a = self.new_array(Vec::new());
                self.stack.push(Value::Object(a));
            }
            Op::ArrayPush | Op::ArrayHole => {
                let v = if op == Op::ArrayPush { Some(self.pop()) } else { None };
                if let Value::Object(a) = self.peek(0) {
                    if let ObjKind::Array(items) = &mut self.obj_mut(a).kind {
                        items.push(v);
                    }
                    self.grow(16);
                }
            }
            Op::ArraySpread => {
                let src = self.peek(0);
                let items = self.iterate_to_vec_ex(src, true)?;
                self.pop();
                if let Value::Object(a) = self.peek(0) {
                    self.grow(items.len() * 16);
                    if let ObjKind::Array(v) = &mut self.obj_mut(a).kind {
                        v.extend(items.into_iter().map(Some));
                    }
                }
            }
            Op::DefineProp(n) => {
                let v = self.pop();
                if let Value::Object(o) = self.peek(0) {
                    self.define_value(o, PropKey::Str(atom(n)), v, DEFAULT_FLAGS);
                }
            }
            Op::DefineElem => {
                let v = self.pop();
                let k = self.pop();
                let key = self.to_property_key(k)?;
                if let Value::Object(o) = self.peek(0) {
                    self.define_value(o, key, v, DEFAULT_FLAGS);
                }
            }
            Op::DefineMethod(enumerable) => {
                let f = self.pop();
                let k = self.pop();
                let key = self.to_property_key(k)?;
                if let Value::Object(o) = self.peek(0) {
                    if let Value::Object(fo) = f {
                        if let Some(c) = self.closure_mut(fo) {
                            c.home = Some(o);
                        }
                    }
                    let flags = if enumerable { DEFAULT_FLAGS } else { HIDDEN };
                    self.define_value(o, key, f, flags);
                }
            }
            Op::DefineAccessor(is_get, enumerable) => {
                let f = self.pop();
                let k = self.pop();
                let key = self.to_property_key(k)?;
                if let Value::Object(o) = self.peek(0) {
                    if let Value::Object(fo) = f {
                        if let Some(c) = self.closure_mut(fo) {
                            c.home = Some(o);
                        }
                    }
                    let mut p = match self.obj(o).props.get(key) {
                        Some(p) if p.is_accessor() => *p,
                        _ => Prop { value: Value::Undefined, setter: Value::Undefined, flags: ACCESSOR | CONFIGURABLE },
                    };
                    if is_get {
                        p.value = f;
                    } else {
                        p.setter = f;
                    }
                    p.flags = ACCESSOR | CONFIGURABLE | if enumerable { ENUMERABLE } else { 0 };
                    self.define_own(o, key, p);
                }
            }
            Op::SetProtoLiteral => {
                let v = self.pop();
                if let Value::Object(o) = self.peek(0) {
                    match v {
                        Value::Object(p) => self.obj_mut(o).proto = Some(p),
                        Value::Null => self.obj_mut(o).proto = None,
                        _ => {}
                    }
                }
            }
            Op::CopyDataProps => {
                let src = self.peek(0);
                let target = self.peek(1);
                if let Value::Object(t) = target {
                    self.copy_data_props(t, src, &[])?;
                }
                self.pop();
            }
            Op::ObjRest(n) => {
                let l = self.stack.len();
                let keys_v: Vec<Value> = self.stack[l.saturating_sub(n as usize)..].to_vec();
                let src = self.stack.get(l.saturating_sub(n as usize + 1)).copied().unwrap_or(Value::Undefined);
                let mut keys = Vec::new();
                for k in keys_v {
                    keys.push(self.to_property_key(k)?);
                }
                let t = self.plain_object();
                self.stack.push(Value::Object(t));
                self.copy_data_props(t, src, &keys)?;
                let l = self.stack.len();
                self.stack.truncate(l.saturating_sub(n as usize + 2));
                self.stack.push(Value::Object(t));
            }
            Op::TemplateObject(i) => {
                let v = self.template_object(fi, i);
                self.stack.push(v);
            }
            Op::MakeClass(idx, has_parent) => self.make_class(fi, idx, has_parent, atoms)?,
            Op::SetFieldInit => {
                let f = self.pop();
                let c = self.pop();
                if let (Value::Object(c), Value::Object(f)) = (c, f) {
                    if let Some(cl) = self.closure_mut(c) {
                        cl.fields = Some(f);
                    }
                }
            }
            Op::SetHome => {
                let h = self.pop();
                if let (Value::Object(f), Value::Object(h)) = (self.peek(0), h) {
                    if let Some(c) = self.closure_mut(f) {
                        c.home = Some(h);
                    }
                }
            }
            Op::Throw => {
                let v = self.pop();
                self.throw_pos = self.current_pos();
                return Err(Exc::Throw(v));
            }
            Op::PushTry(t) => {
                let env = self.frames[fi].env;
                let sp = self.stack.len();
                self.handlers.push(Handler { frame: fi, pc: t as usize, sp, env });
            }
            Op::PopTry => {
                if self.handlers.last().map(|h| h.frame == fi).unwrap_or(false) {
                    self.handlers.pop();
                }
            }
            Op::GetIter => {
                let v = self.peek(0);
                let it = self.get_iterator_ex(v, true)?;
                if let Some(t) = self.stack.last_mut() {
                    *t = it;
                }
            }
            Op::IterNext(t) => {
                let it = self.peek(0);
                match self.iter_step(it)? {
                    Some(v) => self.stack.push(v),
                    None => self.frames[fi].pc = t as usize,
                }
            }
            Op::IterNextOrUndef => {
                let it = self.peek(0);
                let v = self.iter_step(it)?.unwrap_or(Value::Undefined);
                self.stack.push(v);
            }
            Op::IterRest => {
                let it = self.peek(0);
                let mut items = Vec::new();
                while let Some(v) = self.iter_step(it)? {
                    items.push(v);
                    self.safepoint()?;
                }
                let a = self.new_array_values(&items);
                self.stack.push(a);
            }
            Op::IterClose => {
                let it = self.peek(0);
                self.iter_close(it)?;
                self.pop();
            }
            Op::IterCloseThrow => {
                let (it, exc) = (self.peek(1), self.peek(0));
                // the body's exception wins over anything `return()` throws
                if let Err(Exc::Abort(k)) = self.iter_close(it) {
                    return Err(Exc::Abort(k));
                }
                return Err(Exc::Throw(exc));
            }
            Op::ForInStart => {
                let v = self.peek(0);
                let it = self.for_in_iterator(v)?;
                if let Some(t) = self.stack.last_mut() {
                    *t = it;
                }
            }
            Op::ForInNext(t) => {
                let it = self.peek(0);
                match self.for_in_next(it)? {
                    Some(k) => self.stack.push(Value::String(k)),
                    None => self.frames[fi].pc = t as usize,
                }
            }
            Op::GenReturn => {
                let v = self.pop();
                if let Some(s) = self.frames[fi].coro.and_then(|c| self.gen_state_mut(c)) {
                    s.pending_return = v;
                }
                return Err(Exc::Throw(self.gen_marker()));
            }
            Op::RethrowMarker => {
                if self.is_marker(self.peek(0)) {
                    return Err(Exc::Throw(self.pop()));
                }
            }
            Op::GetAsyncIter => {
                let v = self.peek(0);
                let it = self.get_async_iterator(v)?;
                if let Some(t) = self.stack.last_mut() {
                    *t = it;
                }
            }
            Op::CheckIterResult => {
                if !matches!(self.peek(0), Value::Object(_)) {
                    return Err(self.type_error("Iterator result is not an object"));
                }
            }
            Op::ThrowTypeError(n) => {
                let m = String::from_utf16_lossy(self.atom_units(atom(n)));
                return Err(self.type_error(&m));
            }
            other => return self.exec_scope_op(other, fi, atoms, strict),
        }
        Ok(false)
    }

    fn pop2_push(&mut self, v: Value) {
        let l = self.stack.len();
        self.stack.truncate(l.saturating_sub(2));
        self.stack.push(v);
    }

    fn delete_value(&mut self, o: Value, k: PropKey, strict: bool) -> JsResult<bool> {
        let Value::Object(oid) = o else {
            if o.is_nullish() {
                return Err(self.type_error("Cannot convert undefined or null to object"));
            }
            return Ok(true);
        };
        let r = self.delete_prop_ex(oid, k)?;
        if !r && strict {
            let n = self.key_rust(k);
            return Err(self.type_error(&format!("Cannot delete property '{}' of object", n)));
        }
        Ok(r)
    }

    pub(crate) fn run_field_init(&mut self, ctor: ObjId, this: Value) -> JsResult<()> {
        let fields = self.closure_data(ctor).and_then(|c| c.fields);
        if let Some(f) = fields {
            self.call_value(Value::Object(f), this, &[])?;
        }
        Ok(())
    }

    /// Elements of an internal (compiler-built) array.
    pub(crate) fn array_items(&self, v: Value) -> Vec<Value> {
        match v {
            Value::Object(o) => match &self.obj(o).kind {
                ObjKind::Array(items) => items.iter().map(|x| x.unwrap_or(Value::Undefined)).collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    pub(crate) fn get_global(&mut self, a: Atom, typeof_mode: bool) -> JsResult<Value> {
        if let Some(g) = self.global_lex.get(&a) {
            return match g.value {
                Some(v) => Ok(v),
                None => Err(self.tdz_error()),
            };
        }
        let g = self.global;
        let k = PropKey::Str(a);
        if let Some(p) = self.obj(g).props.get(k) {
            if !p.is_accessor() {
                return Ok(p.value);
            }
        }
        if self.has_property(g, k)? {
            return self.get_from(g, k, Value::Object(g));
        }
        if typeof_mode {
            return Ok(Value::Undefined);
        }
        let name = String::from_utf16_lossy(self.atom_units(a));
        Err(self.reference_error(&format!("{} is not defined", name)))
    }

    pub(crate) fn put_global(&mut self, a: Atom, v: Value, strict: bool) -> JsResult<()> {
        if let Some(g) = self.global_lex.get_mut(&a) {
            if g.value.is_none() {
                return Err(self.tdz_error());
            }
            if !g.mutable {
                return Err(self.type_error("Assignment to constant variable."));
            }
            g.value = Some(v);
            return Ok(());
        }
        let g = self.global;
        let k = PropKey::Str(a);
        if strict && !self.has_property(g, k)? {
            let name = String::from_utf16_lossy(self.atom_units(a));
            return Err(self.reference_error(&format!("{} is not defined", name)));
        }
        let ok = self.set_prop(Value::Object(g), k, v, Value::Object(g))?;
        if !ok && strict {
            return Err(self.type_error("Cannot assign to read only global"));
        }
        Ok(())
    }
}
