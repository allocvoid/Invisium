//! The bytecode dispatch loop (hot operations). Less frequent ops live in `interp_slow.rs`.

use crate::bytecode::Op;
use crate::ops::number_op;
use crate::runtime::*;
use crate::value::*;

impl Runtime {
    #[inline]
    pub(crate) fn pop(&mut self) -> Value {
        self.stack.pop().unwrap_or(Value::Undefined)
    }

    #[inline]
    pub(crate) fn peek(&self, n: usize) -> Value {
        let l = self.stack.len();
        if n < l { self.stack[l - 1 - n] } else { Value::Undefined }
    }

    pub(crate) fn env_at(&self, mut env: Option<EnvId>, hops: u32) -> Option<EnvId> {
        for _ in 0..hops {
            env = env.and_then(|e| self.env(e)).and_then(|e| e.parent);
        }
        env
    }

    pub(crate) fn tdz_error(&mut self) -> Exc {
        self.reference_error("Cannot access variable before initialization")
    }


    #[inline]
    fn replace2(&mut self, v: Value) {
        let l = self.stack.len();
        self.stack.truncate(l.saturating_sub(2));
        self.stack.push(v);
    }

    /// Executes the top frame until it returns or a call switches frames.
    /// Ok(Some(v)) = the entry frame returned `v`; Ok(None) = continue with the new top frame.
    pub(crate) fn exec(&mut self, entry: usize) -> JsResult<Option<Value>> {
        let fi = self.frames.len() - 1;
        let code = self.frames[fi].code.clone();
        let atoms = self.frames[fi].atoms.clone();
        let lb = self.frames[fi].locals_base;
        let strict = code.strict;
        let atom = |n: u32| atoms.get(n as usize).copied().unwrap_or(A_EMPTY);
        loop {
            let pc = self.frames[fi].pc;
            let Some(&op) = code.ops.get(pc) else {
                return self.coro_return(Value::Undefined, entry);
            };
            self.frames[fi].pc = pc + 1;
            match op {
                Op::Debugger => {}
                Op::Undef => self.stack.push(Value::Undefined),
                Op::Null => self.stack.push(Value::Null),
                Op::True => self.stack.push(Value::Bool(true)),
                Op::False => self.stack.push(Value::Bool(false)),
                Op::Num(n) => self.stack.push(Value::Number(n)),
                Op::Str(n) => self.stack.push(Value::String(StrId::from_atom(atom(n)))),
                Op::Pop => {
                    self.stack.pop();
                }
                Op::Dup => {
                    let v = self.peek(0);
                    self.stack.push(v);
                }
                Op::Dup2 => {
                    let (a, b) = (self.peek(1), self.peek(0));
                    self.stack.push(a);
                    self.stack.push(b);
                }
                Op::Swap => {
                    let l = self.stack.len();
                    if l >= 2 {
                        self.stack.swap(l - 1, l - 2);
                    }
                }
                Op::Rot(n) => {
                    let l = self.stack.len();
                    let n = n as usize;
                    if n >= 1 && l >= n {
                        let v = self.pop();
                        self.stack.insert(l - n, v);
                    }
                }
                Op::GetLocal(i) => match self.locals.get(lb + i as usize).copied().flatten() {
                    Some(v) => self.stack.push(v),
                    None => return Err(self.tdz_error()),
                },
                Op::PutLocal(i) => {
                    let v = self.pop();
                    match self.locals.get_mut(lb + i as usize) {
                        Some(slot @ Some(_)) => *slot = Some(v),
                        _ => return Err(self.tdz_error()),
                    }
                }
                Op::InitLocal(i) => {
                    let v = self.pop();
                    if let Some(slot) = self.locals.get_mut(lb + i as usize) {
                        *slot = Some(v);
                    }
                }
                Op::ClearLocal(i) => {
                    if let Some(slot) = self.locals.get_mut(lb + i as usize) {
                        *slot = None;
                    }
                }
                Op::GetEnv(h, s) => {
                    let e = self.env_at(self.frames[fi].env, h);
                    match e.and_then(|e| self.env(e)).and_then(|e| e.slots.get(s as usize).copied().flatten()) {
                        Some(v) => self.stack.push(v),
                        None => return Err(self.tdz_error()),
                    }
                }
                Op::PutEnv(h, s) | Op::InitEnv(h, s) => {
                    let v = self.pop();
                    let init = matches!(op, Op::InitEnv(..));
                    let e = self.env_at(self.frames[fi].env, h);
                    let mut ok = false;
                    if let Some(env) = e.and_then(|e| self.env_mut(e)) {
                        if let Some(slot) = env.slots.get_mut(s as usize) {
                            if init || slot.is_some() {
                                *slot = Some(v);
                                ok = true;
                            }
                        }
                    }
                    if !ok {
                        return Err(self.tdz_error());
                    }
                }
                Op::PushEnv(n) => {
                    let parent = self.frames[fi].env;
                    let e = self.alloc_env(parent, n);
                    self.frames[fi].env = Some(e);
                }
                Op::PopEnv => {
                    let cur = self.frames[fi].env;
                    self.frames[fi].env = cur.and_then(|e| self.env(e)).and_then(|e| e.parent);
                }
                Op::CopyEnv => {
                    if let Some(cur) = self.frames[fi].env {
                        let (parent, slots) = match self.env(cur) {
                            Some(e) => (e.parent, e.slots.clone()),
                            None => (None, Vec::new()),
                        };
                        let e = self.alloc_env(parent, 0);
                        if let Some(env) = self.env_mut(e) {
                            env.slots = slots;
                        }
                        self.frames[fi].env = Some(e);
                    }
                }
                Op::GetProp(n) => {
                    let o = self.peek(0);
                    let k = PropKey::Str(atom(n));
                    let v = match o {
                        Value::Object(oid) => match self.obj(oid).props.get(k) {
                            Some(p) if p.flags & ACCESSOR == 0 && !matches!(self.obj(oid).kind, ObjKind::Host(..)) => p.value,
                            _ => self.get_prop(o, k)?,
                        },
                        _ => self.get_prop(o, k)?,
                    };
                    if let Some(top) = self.stack.last_mut() {
                        *top = v;
                    }
                }
                Op::GetPropKeep(n) => {
                    let o = self.peek(0);
                    let v = self.get_prop(o, PropKey::Str(atom(n)))?;
                    self.stack.push(v);
                }
                Op::PutProp(n) => {
                    let (o, v) = (self.peek(1), self.peek(0));
                    let k = PropKey::Str(atom(n));
                    let ok = self.set_prop(o, k, v, o)?;
                    if !ok && strict {
                        let name = self.key_rust(k);
                        return Err(self.type_error(&format!("Cannot assign to read only property '{}' of object", name)));
                    }
                    self.replace2(v);
                }
                Op::GetElem => {
                    let v = self.get_elem(self.peek(1), self.peek(0))?;
                    self.replace2(v);
                }
                Op::GetElemKeep => {
                    let v = self.get_elem(self.peek(1), self.peek(0))?;
                    if let Some(top) = self.stack.last_mut() {
                        *top = v;
                    }
                }
                Op::PutElem => {
                    let (o, k, v) = (self.peek(2), self.peek(1), self.peek(0));
                    let ok = self.put_elem(o, k, v)?;
                    if !ok && strict {
                        return Err(self.type_error("Cannot assign to read only property of object"));
                    }
                    let l = self.stack.len();
                    self.stack.truncate(l.saturating_sub(3));
                    self.stack.push(v);
                }
                Op::Add => {
                    let v = match (self.peek(1), self.peek(0)) {
                        (Value::Number(x), Value::Number(y)) => Value::Number(x + y),
                        (a, b) => self.add(a, b)?,
                    };
                    self.replace2(v);
                }
                Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::Exp | Op::Shl | Op::Shr | Op::UShr | Op::BitAnd | Op::BitOr | Op::BitXor => {
                    let v = match (self.peek(1), self.peek(0)) {
                        (Value::Number(x), Value::Number(y)) => Value::Number(number_op(op, x, y)),
                        // conversions, and BigInt arithmetic
                        (a, b) => self.numeric_binary(op, a, b)?,
                    };
                    self.replace2(v);
                }
                Op::Lt | Op::Gt | Op::Le | Op::Ge => {
                    let (a, b) = (self.peek(1), self.peek(0));
                    let r = if let (Value::Number(x), Value::Number(y)) = (a, b) {
                        match op {
                            Op::Lt => x < y,
                            Op::Gt => x > y,
                            Op::Le => x <= y,
                            _ => x >= y,
                        }
                    } else {
                        match op {
                            Op::Lt => self.less_than(a, b, true)? == Some(true),
                            Op::Gt => self.less_than(b, a, false)? == Some(true),
                            Op::Le => self.less_than(b, a, false)? == Some(false),
                            _ => self.less_than(a, b, true)? == Some(false),
                        }
                    };
                    self.replace2(Value::Bool(r));
                }
                Op::StrictEq | Op::StrictNe => {
                    let r = self.strict_equals(self.peek(1), self.peek(0));
                    self.replace2(Value::Bool(r == (op == Op::StrictEq)));
                }
                Op::Eq | Op::Ne => {
                    let r = self.loose_equals(self.peek(1), self.peek(0))?;
                    self.replace2(Value::Bool(r == (op == Op::Eq)));
                }
                Op::Not => {
                    let v = self.pop();
                    let t = self.truthy(v);
                    self.stack.push(Value::Bool(!t));
                }
                Op::Inc | Op::Dec => {
                    let d = if op == Op::Inc { 1.0 } else { -1.0 };
                    let v = match self.peek(0) {
                        Value::Number(n) => Value::Number(n + d),
                        other => self.numeric_unary(op, other)?,
                    };
                    if let Some(top) = self.stack.last_mut() {
                        *top = v;
                    }
                }
                Op::ToNumeric => {
                    if !matches!(self.peek(0), Value::Number(_)) {
                        let n = self.to_numeric(self.peek(0))?;
                        if let Some(top) = self.stack.last_mut() {
                            *top = n;
                        }
                    }
                }
                Op::Jump(t) => {
                    if (t as usize) <= pc {
                        self.safepoint()?;
                    }
                    self.frames[fi].pc = t as usize;
                }
                Op::JumpIfFalse(t) | Op::JumpIfTrue(t) => {
                    let v = self.pop();
                    if self.truthy(v) == matches!(op, Op::JumpIfTrue(_)) {
                        if (t as usize) <= pc {
                            self.safepoint()?;
                        }
                        self.frames[fi].pc = t as usize;
                    }
                }
                Op::AndJump(t) | Op::OrJump(t) => {
                    let jump = self.truthy(self.peek(0)) == matches!(op, Op::OrJump(_));
                    if jump {
                        self.frames[fi].pc = t as usize;
                    } else {
                        self.stack.pop();
                    }
                }
                Op::NullishJump(t) => {
                    if self.peek(0).is_nullish() {
                        self.stack.pop();
                    } else {
                        self.frames[fi].pc = t as usize;
                    }
                }
                Op::OptJump(t, n) => {
                    if self.peek(0).is_nullish() {
                        let l = self.stack.len();
                        self.stack.truncate(l.saturating_sub(n as usize));
                        self.frames[fi].pc = t as usize;
                    }
                }
                Op::Call(n) | Op::CallMethod(n) => {
                    let method = matches!(op, Op::CallMethod(_));
                    let len = self.stack.len();
                    let fpos = len.saturating_sub(n as usize + 1);
                    let f = self.stack.get(fpos).copied().unwrap_or(Value::Undefined);
                    let (this, ret_sp) = if method { (self.stack.get(fpos.saturating_sub(1)).copied().unwrap_or(Value::Undefined), fpos.saturating_sub(1)) } else { (Value::Undefined, fpos) };
                    if self.invoke(f, this, fpos + 1, ret_sp)? {
                        return Ok(None);
                    }
                }
                Op::New(n) => {
                    let fpos = self.stack.len().saturating_sub(n as usize + 1);
                    if self.op_new(fpos)? {
                        return Ok(None);
                    }
                }
                Op::Return => {
                    let v = self.pop();
                    if self.frames[fi].coro.is_some() {
                        return self.coro_return(v, entry);
                    }
                    return Ok(self.pop_frame(v, entry));
                }
                Op::GenStart => {
                    let g = self.make_generator_from_top()?;
                    return Ok(self.after_pop(g, entry));
                }
                Op::Yield | Op::YieldDelegate | Op::Await => {
                    if let Some(r) = self.op_suspend(op, fi, entry)? {
                        return Ok(r);
                    }
                }
                Op::Closure(i) => {
                    let Some(child) = code.children.get(i as usize).cloned() else {
                        return Err(self.type_error("invalid closure"));
                    };
                    let f = self.make_closure(child, atoms.clone());
                    self.stack.push(Value::Object(f));
                }
                Op::GetArg(i) => {
                    let f = &self.frames[fi];
                    let v = if i < f.argc { self.stack.get(f.args_base + i as usize).copied().unwrap_or(Value::Undefined) } else { Value::Undefined };
                    self.stack.push(v);
                }
                Op::GetGlobal(n) => {
                    let a = atom(n);
                    let v = self.get_global(a, false)?;
                    self.stack.push(v);
                }
                other => {
                    if self.exec_slow(other, fi, &atoms, strict)? {
                        return Ok(None);
                    }
                }
            }
        }
    }

    pub(crate) fn get_elem(&mut self, o: Value, k: Value) -> JsResult<Value> {
        if let (Value::Object(oid), Value::Number(n)) = (o, k) {
            if let ObjKind::Array(items) = &self.obj(oid).kind {
                if n >= 0.0 && n.fract() == 0.0 {
                    if let Some(Some(v)) = items.get(n as usize) {
                        return Ok(*v);
                    }
                }
            } else if let ObjKind::TypedArray(vd) = self.obj(oid).kind {
                // every numeric key is an element or nothing: no property key is built
                let index = n >= 0.0 && n.fract() == 0.0 && n < 9007199254740992.0;
                if vd.kind.is_big() {
                    let hit = if index { self.typed_value(oid, n as usize) } else { None };
                    return Ok(hit.unwrap_or(Value::Undefined));
                }
                let hit = if index { self.typed_get(oid, n as usize) } else { None };
                return Ok(hit.map(Value::Number).unwrap_or(Value::Undefined));
            }
        }
        if let (Value::String(s), Value::Number(n)) = (o, k) {
            if n >= 0.0 && n.fract() == 0.0 {
                return Ok(match self.str_units(s).get(n as usize).copied() {
                    Some(u) => self.new_str16(vec![u]),
                    None => Value::Undefined,
                });
            }
        }
        if o.is_nullish() {
            let ks = match k {
                Value::Symbol(_) => "symbol".to_string(),
                _ => self.to_rust_string(k).unwrap_or_default(),
            };
            let what = if o == Value::Undefined { "undefined" } else { "null" };
            return Err(self.type_error(&format!("Cannot read properties of {} (reading '{}')", what, ks)));
        }
        let key = self.to_property_key(k)?;
        self.get_prop(o, key)
    }

    pub(crate) fn put_elem(&mut self, o: Value, k: Value, v: Value) -> JsResult<bool> {
        if let (Value::Object(oid), Value::Number(n)) = (o, k) {
            if let ObjKind::TypedArray(_) = self.obj(oid).kind {
                // an index outside the array (or not an integer) drops the write
                let i = if n >= 0.0 && n.fract() == 0.0 && n < 9007199254740992.0 { n as usize } else { usize::MAX };
                self.typed_store(oid, i, v)?;
                return Ok(true);
            }
        }
        if let (Value::Object(oid), Value::Number(n)) = (o, k) {
            let obj = self.obj_mut(oid);
            if obj.integrity == 0 {
                if let ObjKind::Array(items) = &mut obj.kind {
                    if n >= 0.0 && n.fract() == 0.0 && (n as usize) < items.len() {
                        if let Some(slot @ Some(_)) = items.get_mut(n as usize) {
                            *slot = Some(v);
                            return Ok(true);
                        }
                    }
                }
            }
        }
        let key = self.to_property_key(k)?;
        self.set_prop(o, key, v, o)
    }

    /// Calls `f` with args at `stack[args_base..]`; returns true when a frame was pushed.
    pub(crate) fn invoke(&mut self, f: Value, this: Value, args_base: usize, ret_sp: usize) -> JsResult<bool> {
        let Value::Object(fo) = f else {
            let s = self.site_or_describe(f);
            return Err(self.type_error(&format!("{} is not a function", s)));
        };
        enum K { Closure(bool), Native(NativeFn), Bound(ObjId, Value, Vec<Value>), Intern(InternFn, Vec<Value>), Gen(u8), Proxy(ProxyData), None }
        let k = match &self.obj(fo).kind {
            ObjKind::Function(d) => match &**d {
                FuncData::Closure(c) => K::Closure(matches!(c.code.kind, crate::ast::FuncKind::BaseCtor | crate::ast::FuncKind::DerivedCtor)),
                FuncData::Native { f, .. } => K::Native(f.clone()),
                FuncData::Bound { target, this, args } => K::Bound(*target, *this, args.clone()),
                FuncData::Intern { f, slots } => K::Intern(*f, slots.clone()),
                FuncData::GenMethod(m) => K::Gen(*m),
            },
            ObjKind::Proxy(p) if p.callable => K::Proxy(*p),
            _ => K::None,
        };
        match k {
            K::Proxy(p) => {
                self.safepoint()?;
                let args: Vec<Value> = self.stack.get(args_base..).map(|s| s.to_vec()).unwrap_or_default();
                let r = self.proxy_call(p, this, &args)?;
                self.stack.truncate(ret_sp);
                self.stack.push(r);
                Ok(false)
            }
            K::Closure(false) => {
                // (a generator function's frame runs its prologue, then suspends at `GenStart`)
                self.push_frame(fo, this, Value::Undefined, args_base, ret_sp, false)?;
                Ok(true)
            }
            K::Gen(mode) => {
                // resume the generator's frame on the VM frame stack: no native recursion
                let a0 = self.stack.get(args_base).copied().unwrap_or(Value::Undefined);
                let r = if mode < 3 { self.gen_resume(this, mode, a0, ret_sp)? } else { self.asyncgen_request(this, mode - 3, a0, ret_sp)? };
                match r {
                    crate::coroutine::Resume::Done(v) => {
                        self.stack.truncate(ret_sp);
                        self.stack.push(v);
                        Ok(false)
                    }
                    crate::coroutine::Resume::Pushed => Ok(true),
                    crate::coroutine::Resume::PushedThrow(e) => Err(Exc::Throw(e)),
                }
            }
            K::Intern(nf, slots) => {
                self.safepoint()?;
                let args: Vec<Value> = self.stack.get(args_base..).map(|s| s.to_vec()).unwrap_or_default();
                let r = self.call_intern(nf, &slots, this, &args)?;
                self.stack.truncate(ret_sp);
                self.stack.push(r);
                Ok(false)
            }
            K::Closure(true) => Err(self.type_error("Class constructor cannot be invoked without 'new'")),
            K::Native(nf) => {
                self.safepoint()?;
                let args: Vec<Value> = self.stack.get(args_base..).map(|s| s.to_vec()).unwrap_or_default();
                let r = self.call_native(&nf, this, &args, Value::Undefined)?;
                self.stack.truncate(ret_sp);
                self.stack.push(r);
                Ok(false)
            }
            K::Bound(t, bt, bargs) => {
                let rest: Vec<Value> = self.stack.get(args_base..).map(|s| s.to_vec()).unwrap_or_default();
                self.stack.truncate(args_base);
                self.stack.extend(bargs);
                self.stack.extend(rest);
                self.invoke(Value::Object(t), bt, args_base, ret_sp)
            }
            K::None => {
                let s = self.site_or_describe(f);
                Err(self.type_error(&format!("{} is not a function", s)))
            }
        }
    }

    pub(crate) fn op_new(&mut self, fpos: usize) -> JsResult<bool> {
        let f = self.stack.get(fpos).copied().unwrap_or(Value::Undefined);
        if !self.is_constructor(f) {
            let s = self.site_or_describe(f);
            return Err(self.type_error(&format!("{} is not a constructor", s)));
        }
        let Value::Object(fo) = f else { return Err(self.type_error("not a constructor")) };
        let derived = match self.closure_data(fo) {
            Some(c) => Some(c.code.kind == crate::ast::FuncKind::DerivedCtor),
            None => None,
        };
        match derived {
            Some(derived) => {
                let this = if derived { Value::Undefined } else { Value::Object(self.create_from_ctor(f, self.intr.object_proto)?) };
                self.push_frame(fo, this, f, fpos + 1, fpos, true)?;
                Ok(true)
            }
            None => {
                let args: Vec<Value> = self.stack.get(fpos + 1..).map(|s| s.to_vec()).unwrap_or_default();
                let r = self.construct_value(f, &args, f)?;
                self.stack.truncate(fpos);
                self.stack.push(r);
                Ok(false)
            }
        }
    }

    /// The source text recorded for the op the top frame is executing (a call, `new` or
    /// iteration site), if any.
    pub(crate) fn site_desc(&self) -> Option<String> {
        let f = self.frames.last()?;
        f.code.desc_at(f.pc.checked_sub(1)?).map(|s| s.to_string())
    }

    pub(crate) fn site_or_describe(&mut self, v: Value) -> String {
        match self.site_desc() {
            Some(s) => s,
            None => self.describe(v),
        }
    }

    /// Short description of a value for error messages.
    pub(crate) fn describe(&mut self, v: Value) -> String {
        match v {
            Value::String(s) => format!("\"{}\"", self.rust_string(s)),
            Value::Object(o) => {
                if self.is_callable_obj(o) {
                    "function".into()
                } else if matches!(self.obj(o).kind, ObjKind::Array(_)) {
                    "array".into()
                } else {
                    "object".into()
                }
            }
            Value::Symbol(_) => "Symbol()".into(),
            Value::BigInt(_) => format!("{}n", self.to_rust_string(v).unwrap_or_default()),
            other => self.to_rust_string(other).unwrap_or_default(),
        }
    }
}
