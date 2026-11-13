//! Frames, calls, construction, exception unwinding and the execution entry points.

use crate::bytecode::FuncCode;
use crate::runtime::*;
use crate::value::*;
use std::rc::Rc;
use std::sync::Arc;

pub(crate) struct Frame {
    pub code: Arc<FuncCode>,
    pub atoms: Rc<[Atom]>,
    pub pc: usize,
    pub locals_base: usize,
    pub args_base: usize,
    pub argc: u32,
    /// Operand stack height to restore on return.
    pub ret_sp: usize,
    pub env: Option<EnvId>,
    pub this: Value,
    pub new_target: Value,
    pub callee: Option<ObjId>,
    pub home: Option<ObjId>,
    pub ctor_callee: Option<ObjId>,
    pub construct: bool,
    /// Generator / async state object this frame runs for.
    pub coro: Option<ObjId>,
    /// Coroutine frames: what the caller receives when the frame suspends at an `await` or
    /// completes (an async function's promise until it was handed over, then undefined).
    pub ret_val: Value,
}

impl Frame {
    pub fn trace(&self, f: &mut dyn FnMut(Value), fe: &mut dyn FnMut(EnvId)) {
        f(self.this);
        f(self.new_target);
        f(self.ret_val);
        for o in [self.callee, self.home, self.ctor_callee, self.coro].into_iter().flatten() {
            f(Value::Object(o));
        }
        if let Some(e) = self.env {
            fe(e);
        }
    }
}

pub(crate) struct Handler {
    pub frame: usize,
    pub pc: usize,
    pub sp: usize,
    pub env: Option<EnvId>,
}

#[inline(never)]
pub(crate) fn stack_addr() -> usize {
    let x = 0u8;
    std::hint::black_box(&x) as *const u8 as usize
}

impl Runtime {
    pub(crate) fn current_pos(&self) -> (u32, u32) {
        match self.frames.last() {
            Some(f) => {
                let p = f.code.pos_at(f.pc.saturating_sub(1));
                (p.line, p.col)
            }
            None => (0, 0),
        }
    }

    /// Checks the native stack budget (nested runs / native recursion).
    pub(crate) fn check_native_stack(&mut self) -> JsResult<()> {
        let here = stack_addr();
        if self.stack_base.abs_diff(here) > self.native_stack_budget {
            return Err(self.stack_overflow());
        }
        Ok(())
    }

    pub(crate) fn stack_overflow(&mut self) -> Exc {
        let e = self.make_error("RangeError", "Maximum call stack size exceeded");
        if let Value::Object(o) = e {
            self.stack_overflow_error = Some(o);
        }
        Exc::Throw(e)
    }

    /// Step accounting + GC safepoint. Called on calls and backward jumps.
    #[inline]
    pub(crate) fn safepoint(&mut self) -> JsResult<()> {
        self.steps += 1;
        let owed = self.pending_steps.get();
        if owed != 0 {
            self.pending_steps.set(0);
            self.steps = self.steps.saturating_add(owed);
        }
        if self.steps > self.limits.step_budget {
            return Err(Exc::Abort(ErrorKind::StepBudget));
        }
        if self.prof.is_some() && self.steps & 1023 == 0 {
            self.profile_sample();
        }
        if self.gc_due() {
            let started = self.prof.is_some().then(std::time::Instant::now);
            self.collect_garbage();
            if let Some(t) = started {
                self.profile_gc(t);
            }
            if self.total_heap() > self.limits.max_heap_bytes {
                return Err(Exc::Abort(ErrorKind::OutOfMemory));
            }
        }
        Ok(())
    }

    pub(crate) fn closure_data(&self, o: ObjId) -> Option<&Closure> {
        match &self.obj(o).kind {
            ObjKind::Function(f) => match &**f {
                FuncData::Closure(c) => Some(c),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn closure_mut(&mut self, o: ObjId) -> Option<&mut Closure> {
        match &mut self.obj_mut(o).kind {
            ObjKind::Function(f) => match &mut **f {
                FuncData::Closure(c) => Some(c),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn is_constructor(&self, v: Value) -> bool {
        let Value::Object(o) = v else { return false };
        match &self.obj(o).kind {
            ObjKind::Function(f) => match &**f {
                FuncData::Closure(c) => {
                    !c.code.is_async && !c.code.is_gen && matches!(c.code.kind, crate::ast::FuncKind::Normal | crate::ast::FuncKind::BaseCtor | crate::ast::FuncKind::DerivedCtor)
                }
                FuncData::Native { ctor, .. } => *ctor,
                FuncData::Bound { target, .. } => self.is_constructor(Value::Object(*target)),
                FuncData::Intern { .. } | FuncData::GenMethod(_) => false,
            },
            ObjKind::Proxy(p) => p.ctor,
            _ => false,
        }
    }

    /// Pushes a frame for a closure whose arguments are already at `stack[args_base..]`.
    pub(crate) fn push_frame(&mut self, f: ObjId, this: Value, new_target: Value, args_base: usize, ret_sp: usize, construct: bool) -> JsResult<()> {
        // the callee, `this` (for `new`, an object allocated just now) and `new.target` are in
        // no frame yet: keep them alive across the collection the safepoint may run
        let mark = self.scratch.len();
        self.scratch.extend_from_slice(&[Value::Object(f), this, new_target]);
        let sp = self.safepoint();
        self.scratch.truncate(mark);
        sp?;
        if self.frames.len() as u32 >= self.limits.max_call_depth {
            return Err(self.stack_overflow());
        }
        let Some(c) = self.closure_data(f) else { return Err(self.type_error("not a function")) };
        let code = c.code.clone();
        let atoms = c.atoms.clone();
        let env = c.env;
        let arrow = code.kind == crate::ast::FuncKind::Arrow;
        let (this, new_target, home, ctor_callee) = if arrow {
            (c.this_val, c.new_target, c.home, c.ctor_callee)
        } else {
            let home = c.home;
            let this = if code.strict || construct {
                this
            } else {
                match this {
                    Value::Undefined | Value::Null => Value::Object(self.global),
                    Value::Object(_) => this,
                    other => Value::Object(self.to_object(other)?),
                }
            };
            (this, new_target, home, Some(f))
        };
        let argc = (self.stack.len() - args_base) as u32;
        let locals_base = self.locals.len();
        self.locals.resize(locals_base + code.nlocals as usize, None);
        // an async function runs as a coroutine whose result is delivered through a promise
        let (coro, ret_val) = if code.is_async && !code.is_gen {
            let p = self.new_promise_obj(self.intr.promise_proto);
            let g = GenState { status: GenStatus::Running, kind: 1, frame: None, delegating: false, pending_return: Value::Undefined, promise: Some(p), queue: Default::default() };
            (Some(self.alloc(Object::new(None, ObjKind::Generator(Box::new(g))))), Value::Object(p))
        } else {
            (None, Value::Undefined)
        };
        self.frames.push(Frame {
            code,
            atoms,
            pc: 0,
            locals_base,
            args_base,
            argc,
            ret_sp,
            env,
            this,
            new_target,
            callee: Some(f),
            home,
            ctor_callee,
            construct,
            coro,
            ret_val,
        });
        Ok(())
    }

    /// Pops the top frame; returns Some(value) when it was the entry frame of this run.
    pub(crate) fn pop_frame(&mut self, mut v: Value, entry: usize) -> Option<Value> {
        let f = self.frames.pop()?;
        if f.construct && !matches!(v, Value::Object(_)) {
            v = f.this;
        }
        let depth = self.frames.len();
        while self.handlers.last().map(|h| h.frame >= depth).unwrap_or(false) {
            self.handlers.pop();
        }
        self.stack.truncate(f.ret_sp);
        self.locals.truncate(f.locals_base);
        if depth <= entry {
            return Some(v);
        }
        self.stack.push(v);
        None
    }

    /// Runs frames until the frame at index `entry` returns.
    pub(crate) fn run_frames(&mut self, entry: usize) -> JsResult<Value> {
        self.run_with(entry, None)
    }

    /// Like `run`; `pending` is an exception to raise in the top frame before executing.
    pub(crate) fn run_with(&mut self, entry: usize, mut pending: Option<Exc>) -> JsResult<Value> {
        let saved_native = self.native_active;
        self.native_active = false;
        let r = loop {
            let step = match pending.take() {
                Some(e) => Err(e),
                None => self.exec(entry),
            };
            match step {
                Ok(Some(v)) => break Ok(v),
                Ok(None) => {}
                Err(e) => match self.handle_exc(e, entry) {
                    Ok(None) => {}
                    Ok(Some(v)) => break Ok(v),
                    Err(e) => break Err(e),
                },
            }
        };
        self.native_active = saved_native;
        r
    }

    /// Dispatches an exception frame by frame. Ok(None) = handled, keep executing the top frame;
    /// Ok(Some(v)) = the entry frame completed with `v` (a coroutine absorbed the exception);
    /// Err = it leaves this run, whose frames are gone.
    fn handle_exc(&mut self, e: Exc, entry: usize) -> JsResult<Option<Value>> {
        let Exc::Throw(v) = e else {
            if self.abort_stack.is_empty() {
                self.abort_stack = self.stack_frames(None);
            }
            // uncatchable abort: the coroutines of the discarded frames can never resume
            for i in entry..self.frames.len() {
                if let Some(c) = self.frames[i].coro {
                    self.finish_coro(c);
                }
            }
            if let Some(base) = self.frames.get(entry).map(|f| (f.ret_sp, f.locals_base)) {
                self.unwind_to(entry);
                self.stack.truncate(base.0);
                self.locals.truncate(base.1);
            }
            return Err(e);
        };
        loop {
            if self.frames.len() <= entry {
                return Err(Exc::Throw(v));
            }
            let top = self.frames.len() - 1;
            if self.handlers.last().map(|h| h.frame == top).unwrap_or(false) {
                let h = self.handlers.pop().expect("handler");
                self.stack.truncate(h.sp);
                self.stack.push(v);
                let f = self.frames.last_mut().expect("frame");
                f.pc = h.pc;
                f.env = h.env;
                return Ok(None);
            }
            // no handler in the top frame: it completes abruptly
            if let Some(c) = self.frames[top].coro {
                if let Some(done) = self.coro_throw(c, v, entry)? {
                    return Ok(done);
                }
            }
            self.unwind_to(top);
        }
    }

    /// Drops frames above `depth` (exclusive) and their handlers.
    fn unwind_to(&mut self, depth: usize) {
        while self.frames.len() > depth {
            if let Some(f) = self.frames.pop() {
                self.locals.truncate(f.locals_base);
                self.stack.truncate(f.ret_sp);
            }
        }
        while self.handlers.last().map(|h| h.frame >= depth).unwrap_or(false) {
            self.handlers.pop();
        }
    }

    pub(crate) fn call_native(&mut self, f: &NativeFn, this: Value, args: &[Value], new_target: Value) -> JsResult<Value> {
        self.check_native_stack()?;
        let saved_nt = std::mem::replace(&mut self.native_new_target, new_target);
        let saved_active = self.native_active;
        let mark = self.scratch.len();
        self.native_active = true;
        self.native_depth += 1;
        let r = f(self, this, args);
        self.native_depth -= 1;
        self.native_active = saved_active;
        self.scratch.truncate(mark);
        self.native_new_target = saved_nt;
        r.map_err(|t| self.thrown_to_exc(t))
    }

    /// Calls an engine-internal native; its captured slots, `this` and arguments stay rooted.
    pub(crate) fn call_intern(&mut self, f: InternFn, slots: &[Value], this: Value, args: &[Value]) -> JsResult<Value> {
        self.check_native_stack()?;
        let saved_nt = std::mem::replace(&mut self.native_new_target, Value::Undefined);
        let saved_active = self.native_active;
        let mark = self.scratch.len();
        self.scratch.push(this);
        self.scratch.extend_from_slice(args);
        self.scratch.extend_from_slice(slots);
        self.native_active = true;
        self.native_depth += 1;
        let r = f(self, slots, this, args);
        self.native_depth -= 1;
        self.native_active = saved_active;
        self.scratch.truncate(mark);
        self.native_new_target = saved_nt;
        r
    }

    /// Creates an engine-internal native function object.
    pub(crate) fn new_intern(&mut self, name: &str, arity: u32, f: InternFn, slots: Vec<Value>) -> ObjId {
        let proto = self.intr.function_proto;
        let o = self.alloc(Object::new(proto, ObjKind::Function(Box::new(FuncData::Intern { f, slots }))));
        self.define_value(o, PropKey::Str(A_LENGTH), Value::Number(arity as f64), CONFIGURABLE);
        let n = self.new_str(name);
        self.define_value(o, PropKey::Str(A_NAME), n, CONFIGURABLE);
        o
    }

    /// Runs `f` with allocations rooted (as inside a native call); the result itself is not rooted.
    pub(crate) fn rooted<T>(&mut self, f: impl FnOnce(&mut Runtime) -> T) -> T {
        let saved = self.native_active;
        let mark = self.scratch.len();
        self.native_active = true;
        let r = f(self);
        self.native_active = saved;
        self.scratch.truncate(mark);
        r
    }

    /// Runs `f` with `v` rooted: for a value held in a Rust local while `f` may run script.
    pub(crate) fn holding<T>(&mut self, v: Value, f: impl FnOnce(&mut Runtime) -> T) -> T {
        let mark = self.scratch.len();
        self.scratch.push(v);
        let r = f(self);
        self.scratch.truncate(mark);
        r
    }

    /// Calls any callable value from Rust (nested run for closures). Inside a native, the
    /// result stays rooted until that native returns.
    pub(crate) fn call_value(&mut self, f: Value, this: Value, args: &[Value]) -> JsResult<Value> {
        let r = self.call_value_raw(f, this, args);
        if let Ok(v) = r {
            self.keep_for_native(v);
        }
        r
    }

    fn call_value_raw(&mut self, f: Value, this: Value, args: &[Value]) -> JsResult<Value> {
        let Value::Object(fo) = f else {
            return Err(self.type_error("value is not a function"));
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
            K::None => Err(self.type_error("value is not a function")),
            K::Proxy(p) => {
                self.safepoint_light()?;
                self.proxy_call(p, this, args)
            }
            K::Gen(mode) => {
                use crate::coroutine::Resume;
                self.check_native_stack()?;
                let ret_sp = self.stack.len();
                let a0 = args.first().copied().unwrap_or(Value::Undefined);
                let r = if mode < 3 { self.gen_resume(this, mode, a0, ret_sp)? } else { self.asyncgen_request(this, mode - 3, a0, ret_sp)? };
                match r {
                    Resume::Done(v) => Ok(v),
                    Resume::Pushed => self.run_frames(self.frames.len() - 1),
                    Resume::PushedThrow(e) => self.run_with(self.frames.len() - 1, Some(Exc::Throw(e))),
                }
            }
            K::Intern(nf, slots) => {
                self.safepoint_light()?;
                self.call_intern(nf, &slots, this, args)
            }
            K::Native(nf) => {
                self.safepoint_light()?;
                self.call_native(&nf, this, args, Value::Undefined)
            }
            K::Bound(t, bt, mut bargs) => {
                bargs.extend_from_slice(args);
                self.call_value(Value::Object(t), bt, &bargs)
            }
            K::Closure(true) => Err(self.type_error("Class constructor cannot be invoked without 'new'")),
            K::Closure(false) => {
                self.check_native_stack()?;
                let ret_sp = self.stack.len();
                self.stack.extend_from_slice(args);
                if let Err(e) = self.push_frame(fo, this, Value::Undefined, ret_sp, ret_sp, false) {
                    self.stack.truncate(ret_sp);
                    return Err(e);
                }
                let entry = self.frames.len() - 1;
                self.run_frames(entry)
            }
        }
    }

    fn safepoint_light(&mut self) -> JsResult<()> {
        self.steps += 1;
        if self.steps > self.limits.step_budget {
            return Err(Exc::Abort(ErrorKind::StepBudget));
        }
        self.drain_pending_steps()
    }

    /// Adds the steps owed for BigInt work done under `&self` (equality, map keys; see
    /// `big_note`) to the step count, and aborts when that exhausts the budget. Native
    /// loops that compare or key values call this after each element.
    #[inline]
    pub(crate) fn drain_pending_steps(&mut self) -> JsResult<()> {
        let owed = self.pending_steps.get();
        if owed != 0 {
            self.pending_steps.set(0);
            self.steps = self.steps.saturating_add(owed);
            if self.steps > self.limits.step_budget {
                return Err(Exc::Abort(ErrorKind::StepBudget));
            }
        }
        Ok(())
    }

    /// [[Construct]] from Rust. Inside a native, the result stays rooted until it returns.
    pub(crate) fn construct_value(&mut self, f: Value, args: &[Value], new_target: Value) -> JsResult<Value> {
        let r = self.construct_value_raw(f, args, new_target);
        if let Ok(v) = r {
            self.keep_for_native(v);
        }
        r
    }

    fn construct_value_raw(&mut self, f: Value, args: &[Value], new_target: Value) -> JsResult<Value> {
        if !self.is_constructor(f) {
            return Err(self.type_error("value is not a constructor"));
        }
        let Value::Object(fo) = f else { return Err(self.type_error("value is not a constructor")) };
        enum K { Closure(bool), Native(NativeFn), Bound(ObjId, Vec<Value>) }
        let k = match &self.obj(fo).kind {
            ObjKind::Function(d) => match &**d {
                FuncData::Closure(c) => K::Closure(c.code.kind == crate::ast::FuncKind::DerivedCtor),
                FuncData::Native { f, .. } => K::Native(f.clone()),
                FuncData::Bound { target, args, .. } => K::Bound(*target, args.clone()),
                FuncData::Intern { .. } | FuncData::GenMethod(_) => return Err(self.type_error("value is not a constructor")),
            },
            ObjKind::Proxy(_) => {
                self.safepoint_light()?;
                return self.proxy_construct(fo, args, new_target);
            }
            _ => return Err(self.type_error("value is not a constructor")),
        };
        match k {
            K::Native(nf) => {
                self.safepoint_light()?;
                self.call_native(&nf, Value::Undefined, args, new_target)
            }
            K::Bound(t, mut bargs) => {
                bargs.extend_from_slice(args);
                let nt = if new_target == f { Value::Object(t) } else { new_target };
                self.construct_value(Value::Object(t), &bargs, nt)
            }
            K::Closure(derived) => {
                self.check_native_stack()?;
                let this = if derived { Value::Undefined } else { Value::Object(self.create_from_ctor(new_target, self.intr.object_proto)?) };
                let ret_sp = self.stack.len();
                self.stack.extend_from_slice(args);
                if let Err(e) = self.push_frame(fo, this, new_target, ret_sp, ret_sp, true) {
                    self.stack.truncate(ret_sp);
                    return Err(e);
                }
                let entry = self.frames.len() - 1;
                self.run_frames(entry)
            }
        }
    }

    /// OrdinaryCreateFromConstructor: object whose prototype is newTarget.prototype.
    pub(crate) fn create_from_ctor(&mut self, new_target: Value, fallback: Option<ObjId>) -> JsResult<ObjId> {
        let proto = self.proto_from_ctor(new_target, fallback)?;
        Ok(self.new_object(proto))
    }

    pub(crate) fn proto_from_ctor(&mut self, new_target: Value, fallback: Option<ObjId>) -> JsResult<Option<ObjId>> {
        if let Value::Object(_) = new_target {
            let p = self.get_prop(new_target, PropKey::Str(A_PROTOTYPE))?;
            if let Value::Object(p) = p {
                return Ok(Some(p));
            }
        }
        Ok(fallback)
    }

    pub(crate) fn make_closure(&mut self, code: Arc<FuncCode>, atoms: Rc<[Atom]>) -> ObjId {
        use crate::ast::FuncKind;
        let (env, this_val, new_target, home, ctor_callee) = match self.frames.last() {
            Some(fr) if code.kind == FuncKind::Arrow => (fr.env, fr.this, fr.new_target, fr.home, fr.ctor_callee),
            Some(fr) => (fr.env, Value::Undefined, Value::Undefined, None, None),
            None => (None, Value::Undefined, Value::Undefined, None, None),
        };
        let kind = code.kind;
        let length = code.length;
        let name = code.name.clone();
        let (is_async, is_gen) = (code.is_async, code.is_gen);
        let c = Closure { code, atoms, env, this_val, new_target, home, ctor_callee, fields: None };
        let proto = self.intr.function_proto;
        let f = self.alloc(Object::new(proto, ObjKind::Function(Box::new(FuncData::Closure(c)))));
        self.define_value(f, PropKey::Str(A_LENGTH), Value::Number(length as f64), CONFIGURABLE);
        let nv = if name.is_empty() { self.atom_value(A_EMPTY) } else { Value::String(StrId::from_atom(self.intern16(&name))) };
        self.define_value(f, PropKey::Str(A_NAME), nv, CONFIGURABLE);
        if is_gen {
            // generator functions are not constructors; `prototype` is what their generators inherit from
            let gp = if is_async { self.intr.async_generator_proto } else { self.intr.generator_proto };
            let p = self.new_object(gp);
            self.define_value(f, PropKey::Str(A_PROTOTYPE), Value::Object(p), WRITABLE);
        } else if kind == FuncKind::Normal && !is_async {
            let p = self.plain_object();
            self.define_value(p, PropKey::Str(A_CONSTRUCTOR), Value::Object(f), HIDDEN);
            self.define_value(f, PropKey::Str(A_PROTOTYPE), Value::Object(p), WRITABLE);
        }
        f
    }

    pub(crate) fn new_native(&mut self, name: &str, arity: u32, f: NativeFn, ctor: bool) -> ObjId {
        let proto = self.intr.function_proto;
        let o = self.alloc(Object::new(proto, ObjKind::Function(Box::new(FuncData::Native { f, ctor }))));
        self.define_value(o, PropKey::Str(A_LENGTH), Value::Number(arity as f64), CONFIGURABLE);
        let n = self.new_str(name);
        self.define_value(o, PropKey::Str(A_NAME), n, CONFIGURABLE);
        o
    }
}
