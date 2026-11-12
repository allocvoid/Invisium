//! Suspendable frames: generators, async functions and async generators.
//!
//! A suspended frame is taken off the VM stacks and stored in its `Generator` object; resuming
//! puts it back on top of the frame stack, so neither `next()` nor `await` recurses natively.

use crate::bytecode::Op;
use crate::runtime::*;
use crate::value::*;

pub(crate) enum Resume {
    /// Completed without running the frame.
    Done(Value),
    /// The frame is on top of the frame stack, ready to run.
    Pushed,
    /// The frame is on top and this value must be thrown into it.
    PushedThrow(Value),
}

impl Runtime {
    pub(crate) fn gen_state(&self, o: ObjId) -> Option<&GenState> {
        match &self.obj(o).kind {
            ObjKind::Generator(g) => Some(g),
            _ => None,
        }
    }

    pub(crate) fn gen_state_mut(&mut self, o: ObjId) -> Option<&mut GenState> {
        match &mut self.obj_mut(o).kind {
            ObjKind::Generator(g) => Some(g),
            _ => None,
        }
    }

    /// Marks a coroutine as finished and drops its saved frame.
    pub(crate) fn finish_coro(&mut self, c: ObjId) {
        if let Some(g) = self.gen_state_mut(c) {
            g.status = GenStatus::Done;
            g.frame = None;
        }
    }

    /// The sentinel thrown to run a generator's `finally` blocks on `return()`.
    pub(crate) fn gen_marker(&self) -> Value {
        self.intr.gen_return_marker.map(Value::Object).unwrap_or(Value::Undefined)
    }

    pub(crate) fn is_marker(&self, v: Value) -> bool {
        matches!((v, self.intr.gen_return_marker), (Value::Object(o), Some(m)) if o == m)
    }

    /// Takes the top frame off the VM stacks together with its locals, operands and handlers.
    fn detach_top_frame(&mut self) -> Option<Box<SavedFrame>> {
        let frame = self.frames.pop()?;
        let fi = self.frames.len();
        let mut handlers = Vec::new();
        while self.handlers.last().map(|h| h.frame >= fi).unwrap_or(false) {
            if let Some(mut h) = self.handlers.pop() {
                h.sp = h.sp.saturating_sub(frame.args_base);
                handlers.push(h);
            }
        }
        handlers.reverse();
        let stack = self.stack.split_off(frame.args_base.min(self.stack.len()));
        let locals = self.locals.split_off(frame.locals_base.min(self.locals.len()));
        self.stack.truncate(frame.ret_sp);
        Some(Box::new(SavedFrame { frame, locals, stack, handlers }))
    }

    fn attach_frame(&mut self, saved: Box<SavedFrame>, ret_sp: usize) {
        let SavedFrame { mut frame, mut locals, mut stack, handlers } = *saved;
        let fi = self.frames.len();
        let base = self.stack.len();
        frame.args_base = base;
        frame.ret_sp = ret_sp;
        frame.locals_base = self.locals.len();
        self.stack.append(&mut stack);
        self.locals.append(&mut locals);
        for mut h in handlers {
            h.frame = fi;
            h.sp += base;
            self.handlers.push(h);
        }
        self.frames.push(frame);
    }

    fn suspend_top(&mut self, c: ObjId, status: GenStatus, delegating: bool) {
        let saved = self.detach_top_frame();
        if let Some(g) = self.gen_state_mut(c) {
            g.status = status;
            g.delegating = delegating;
            g.frame = saved;
        }
    }

    /// After a frame left the stack: hand `v` to the caller frame, or out of the run.
    pub(crate) fn after_pop(&mut self, v: Value, entry: usize) -> Option<Value> {
        if self.frames.len() <= entry {
            return Some(v);
        }
        self.stack.push(v);
        None
    }

    /// Turns the just-pushed frame of a generator function into a suspended generator object.
    pub(crate) fn make_generator_from_top(&mut self) -> JsResult<Value> {
        let (callee, is_async) = match self.frames.last() {
            Some(f) => (f.callee, f.code.is_async),
            None => return Ok(Value::Undefined),
        };
        // the prototype lookup may run script: do it while the frame is still attached (rooted)
        let proto = match callee {
            Some(c) => self.get_prop(Value::Object(c), PropKey::Str(A_PROTOTYPE)),
            None => Ok(Value::Undefined),
        };
        let saved = self.detach_top_frame();
        let proto = match proto? {
            Value::Object(p) => Some(p),
            _ if is_async => self.intr.async_generator_proto,
            _ => self.intr.generator_proto,
        };
        let bytes = saved.as_ref().map(|s| (s.locals.len() + s.stack.len()) * 16 + 200).unwrap_or(0);
        let g = GenState {
            status: GenStatus::Start,
            kind: if is_async { 2 } else { 0 },
            frame: saved,
            delegating: false,
            pending_return: Value::Undefined,
            promise: None,
            queue: Default::default(),
        };
        let o = self.alloc(Object::new(proto, ObjKind::Generator(Box::new(g))));
        self.grow(bytes);
        if let Some(s) = self.gen_state_mut(o).and_then(|g| g.frame.as_mut()) {
            s.frame.coro = Some(o);
        }
        Ok(Value::Object(o))
    }

    /// Puts the saved frame of `g` back on top of the frame stack (status becomes Running).
    fn reattach(&mut self, g: ObjId, ret_sp: usize) -> JsResult<()> {
        self.safepoint()?;
        if self.frames.len() as u32 >= self.limits.max_call_depth {
            return Err(self.stack_overflow());
        }
        let saved = self.gen_state_mut(g).and_then(|s| {
            s.status = GenStatus::Running;
            s.frame.take()
        });
        match saved {
            Some(saved) => {
                self.attach_frame(saved, ret_sp);
                Ok(())
            }
            None => {
                self.finish_coro(g);
                Err(self.type_error("Generator has no suspended frame"))
            }
        }
    }

    /// Hands a resume request to the freshly attached frame.
    fn deliver(&mut self, g: ObjId, fresh: bool, delegating: bool, mode: u8, arg: Value) -> Resume {
        if fresh {
            return Resume::Pushed;
        }
        if delegating {
            self.stack.push(arg);
            self.stack.push(Value::Number(mode as f64));
            return Resume::Pushed;
        }
        match mode {
            0 => {
                self.stack.push(arg);
                Resume::Pushed
            }
            1 => Resume::PushedThrow(arg),
            _ => {
                if let Some(s) = self.gen_state_mut(g) {
                    s.pending_return = arg;
                }
                Resume::PushedThrow(self.gen_marker())
            }
        }
    }

    /// Generator.prototype.next (mode 0) / throw (1) / return (2).
    pub(crate) fn gen_resume(&mut self, this: Value, mode: u8, arg: Value, ret_sp: usize) -> JsResult<Resume> {
        let g = match this {
            Value::Object(o) if self.gen_state(o).map(|s| s.kind == 0).unwrap_or(false) => o,
            _ => return Err(self.type_error("Generator method called on incompatible receiver")),
        };
        let (mut status, delegating) = self.gen_state(g).map(|s| (s.status, s.delegating)).unwrap_or((GenStatus::Done, false));
        if status == GenStatus::Start && mode != 0 {
            self.finish_coro(g);
            status = GenStatus::Done;
        }
        match status {
            GenStatus::Running | GenStatus::Awaiting => Err(self.type_error("Generator is already running")),
            GenStatus::Done if mode == 1 => Err(Exc::Throw(arg)),
            GenStatus::Done => {
                let v = if mode == 2 { arg } else { Value::Undefined };
                Ok(Resume::Done(self.iter_result(v, true)))
            }
            GenStatus::Start | GenStatus::Yielded => {
                self.reattach(g, ret_sp)?;
                Ok(self.deliver(g, status == GenStatus::Start, delegating, mode, arg))
            }
        }
    }

    /// AsyncGenerator.prototype.next / throw / return: queues the request and starts serving
    /// it when the generator is idle. The result is always the request's promise.
    pub(crate) fn asyncgen_request(&mut self, this: Value, mode: u8, arg: Value, ret_sp: usize) -> JsResult<Resume> {
        let p = self.new_promise_obj(self.intr.promise_proto);
        let g = match this {
            Value::Object(o) if self.gen_state(o).map(|s| s.kind == 2).unwrap_or(false) => o,
            _ => {
                let e = self.make_error("TypeError", "AsyncGenerator method called on incompatible receiver");
                self.reject_promise(p, e);
                return Ok(Resume::Done(Value::Object(p)));
            }
        };
        let (mut status, delegating) = match self.gen_state_mut(g) {
            Some(s) => {
                s.queue.push_back(GenRequest { mode, value: arg, promise: p });
                (s.status, s.delegating)
            }
            None => (GenStatus::Done, false),
        };
        if status == GenStatus::Start && mode != 0 {
            self.finish_coro(g);
            status = GenStatus::Done;
        }
        match status {
            GenStatus::Running | GenStatus::Awaiting => {}
            GenStatus::Done => self.asyncgen_drain(g),
            GenStatus::Start | GenStatus::Yielded => {
                if let Err(e) = self.reattach(g, ret_sp) {
                    if let Some(s) = self.gen_state_mut(g) {
                        s.queue.pop_back();
                    }
                    return Err(e);
                }
                if let Some(f) = self.frames.last_mut() {
                    f.ret_val = Value::Object(p);
                }
                return Ok(self.deliver(g, status == GenStatus::Start, delegating, mode, arg));
            }
        }
        Ok(Resume::Done(Value::Object(p)))
    }

    /// Settles every request queued on a finished async generator.
    fn asyncgen_drain(&mut self, g: ObjId) {
        while let Some(req) = self.gen_state_mut(g).and_then(|s| s.queue.pop_front()) {
            if req.mode == 1 {
                self.reject_promise(req.promise, req.value);
            } else {
                let v = if req.mode == 2 { req.value } else { Value::Undefined };
                let r = self.iter_result(v, true);
                self.fulfill_promise(req.promise, r);
            }
        }
    }

    /// Job: the promise a coroutine awaited has settled.
    pub(crate) fn resume_await(&mut self, c: ObjId, rejected: bool, value: Value) -> JsResult<()> {
        if self.gen_state(c).map(|s| s.status) != Some(GenStatus::Awaiting) {
            return Ok(());
        }
        let ret_sp = self.stack.len();
        self.reattach(c, ret_sp)?;
        let entry = self.frames.len() - 1;
        let pending = if rejected {
            Some(Exc::Throw(value))
        } else {
            self.stack.push(value);
            None
        };
        self.run_with(entry, pending).map(|_| ())
    }

    /// `yield` / `yield*` step / `await` in the top frame. Ok(None) = keep executing this frame;
    /// Ok(Some(r)) = the frame left the stack and `r` is what `exec` returns.
    pub(crate) fn op_suspend(&mut self, op: Op, fi: usize, entry: usize) -> JsResult<Option<Option<Value>>> {
        let Some(c) = self.frames[fi].coro else {
            return Err(self.type_error("Cannot suspend outside a generator or async function"));
        };
        if op == Op::Await {
            let v = self.peek(0);
            let p = self.promise_resolve_value(v)?;
            self.promise_then(p, Reaction::Await { coro: c });
            self.pop();
            let ret = std::mem::replace(&mut self.frames[fi].ret_val, Value::Undefined);
            self.suspend_top(c, GenStatus::Awaiting, false);
            return Ok(Some(self.after_pop(ret, entry)));
        }
        let delegating = op == Op::YieldDelegate;
        let v = self.pop();
        let result = self.iter_result(v, false);
        if self.gen_state(c).map(|s| s.kind) != Some(2) {
            self.suspend_top(c, GenStatus::Yielded, delegating);
            return Ok(Some(self.after_pop(result, entry)));
        }
        // async generator: settle the request being served, then take the next one if queued
        let (head, next) = match self.gen_state_mut(c) {
            Some(s) => (s.queue.pop_front(), s.queue.front().map(|r| (r.mode, r.value))),
            None => (None, None),
        };
        if let Some(req) = head {
            self.fulfill_promise(req.promise, result);
        }
        match next {
            None => {
                let ret = std::mem::replace(&mut self.frames[fi].ret_val, Value::Undefined);
                self.suspend_top(c, GenStatus::Yielded, delegating);
                Ok(Some(self.after_pop(ret, entry)))
            }
            Some((mode, value)) => match self.deliver(c, false, delegating, mode, value) {
                Resume::PushedThrow(e) => Err(Exc::Throw(e)),
                _ => Ok(None),
            },
        }
    }

    /// `return` in the top frame, which may belong to a coroutine.
    pub(crate) fn coro_return(&mut self, v: Value, entry: usize) -> JsResult<Option<Value>> {
        let Some(top) = self.frames.len().checked_sub(1) else { return Ok(Some(v)) };
        let Some(c) = self.frames[top].coro else { return Ok(self.pop_frame(v, entry)) };
        let (kind, promise) = self.gen_state(c).map(|s| (s.kind, s.promise)).unwrap_or((0, None));
        let out = match kind {
            0 => self.iter_result(v, true),
            1 => {
                if let Some(p) = promise {
                    // keep the value rooted: looking up `then` may run script
                    self.stack.push(v);
                    let r = self.resolve_promise(p, v);
                    self.pop();
                    r?;
                }
                self.frames[top].ret_val
            }
            _ => {
                let r = self.iter_result(v, true);
                if let Some(req) = self.gen_state_mut(c).and_then(|s| s.queue.pop_front()) {
                    self.fulfill_promise(req.promise, r);
                }
                self.finish_coro(c);
                self.asyncgen_drain(c);
                self.frames[top].ret_val
            }
        };
        self.finish_coro(c);
        Ok(self.pop_frame(out, entry))
    }

    /// An exception is leaving the top frame, which belongs to coroutine `c`.
    /// Ok(None) = it keeps unwinding; Ok(Some(r)) = absorbed, the frame was popped with result `r`.
    pub(crate) fn coro_throw(&mut self, c: ObjId, v: Value, entry: usize) -> JsResult<Option<Option<Value>>> {
        let top = self.frames.len().saturating_sub(1);
        let (kind, promise, pending) = self.gen_state(c).map(|s| (s.kind, s.promise, s.pending_return)).unwrap_or((0, None, Value::Undefined));
        let is_return = self.is_marker(v);
        self.finish_coro(c);
        let out = match kind {
            0 if !is_return => return Ok(None),
            0 => self.iter_result(pending, true),
            1 => {
                if let Some(p) = promise {
                    self.reject_promise(p, v);
                }
                self.frames[top].ret_val
            }
            _ => {
                if let Some(req) = self.gen_state_mut(c).and_then(|s| s.queue.pop_front()) {
                    if is_return {
                        let r = self.iter_result(pending, true);
                        self.fulfill_promise(req.promise, r);
                    } else {
                        self.reject_promise(req.promise, v);
                    }
                }
                self.asyncgen_drain(c);
                self.frames[top].ret_val
            }
        };
        Ok(Some(self.pop_frame(out, entry)))
    }
}
