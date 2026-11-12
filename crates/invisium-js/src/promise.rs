//! Promise state machine and the job (microtask) queue.

use crate::runtime::*;
use crate::value::*;

const MAX_UNHANDLED: usize = 16;
const JOB_BYTES: usize = 64;

pub(crate) fn slot(slots: &[Value], i: usize) -> Value {
    slots.get(i).copied().unwrap_or(Value::Undefined)
}

fn resolve_fn(rt: &mut Runtime, slots: &[Value], _: Value, args: &[Value]) -> JsResult<Value> {
    if let (Value::Object(p), Value::Object(cell)) = (slot(slots, 0), slot(slots, 1)) {
        if rt.cell_get(cell) != Value::Bool(true) {
            rt.cell_set(cell, Value::Bool(true));
            rt.resolve_promise(p, slot(args, 0))?;
        }
    }
    Ok(Value::Undefined)
}

fn reject_fn(rt: &mut Runtime, slots: &[Value], _: Value, args: &[Value]) -> JsResult<Value> {
    if let (Value::Object(p), Value::Object(cell)) = (slot(slots, 0), slot(slots, 1)) {
        if rt.cell_get(cell) != Value::Bool(true) {
            rt.cell_set(cell, Value::Bool(true));
            rt.reject_promise(p, slot(args, 0));
        }
    }
    Ok(Value::Undefined)
}

impl Runtime {
    /// A prototype-less object holding one mutable value (shared state of internal closures).
    pub(crate) fn new_cell(&mut self, v: Value) -> ObjId {
        let o = self.alloc(Object::new(None, ObjKind::Ordinary));
        self.obj_mut(o).props.insert(PropKey::Str(A_VALUE), Prop::data(v, WRITABLE));
        self.grow(48);
        o
    }

    pub(crate) fn cell_get(&self, o: ObjId) -> Value {
        self.obj(o).props.get(PropKey::Str(A_VALUE)).map(|p| p.value).unwrap_or(Value::Undefined)
    }

    pub(crate) fn cell_set(&mut self, o: ObjId, v: Value) {
        if let Some(p) = self.obj_mut(o).props.get_mut(PropKey::Str(A_VALUE)) {
            p.value = v;
        }
    }

    pub(crate) fn promise_data(&self, o: ObjId) -> Option<&PromiseData> {
        match &self.obj(o).kind {
            ObjKind::Promise(p) => Some(p),
            _ => None,
        }
    }

    fn promise_data_mut(&mut self, o: ObjId) -> Option<&mut PromiseData> {
        match &mut self.obj_mut(o).kind {
            ObjKind::Promise(p) => Some(p),
            _ => None,
        }
    }

    pub(crate) fn as_promise(&self, v: Value) -> Option<ObjId> {
        match v {
            Value::Object(o) if matches!(self.obj(o).kind, ObjKind::Promise(_)) => Some(o),
            _ => None,
        }
    }

    pub(crate) fn new_promise_obj(&mut self, proto: Option<ObjId>) -> ObjId {
        let d = PromiseData { state: 0, value: Value::Undefined, reactions: Vec::new(), handled: false };
        self.alloc(Object::new(proto, ObjKind::Promise(Box::new(d))))
    }

    /// CreateResolvingFunctions: (resolve, reject) sharing one "already resolved" flag.
    pub(crate) fn resolving_functions(&mut self, p: ObjId) -> (Value, Value) {
        let cell = self.new_cell(Value::Bool(false));
        let slots = vec![Value::Object(p), Value::Object(cell)];
        let res = self.new_intern("", 1, resolve_fn, slots.clone());
        let rej = self.new_intern("", 1, reject_fn, slots);
        (Value::Object(res), Value::Object(rej))
    }

    pub(crate) fn enqueue_job(&mut self, job: Job) {
        self.jobs.push_back(job);
        self.grow(JOB_BYTES);
    }

    /// The promise resolve function's steps (thenables are assimilated by a later job).
    pub(crate) fn resolve_promise(&mut self, p: ObjId, resolution: Value) -> JsResult<()> {
        if resolution == Value::Object(p) {
            let e = self.make_error("TypeError", "Chaining cycle detected for promise");
            self.reject_promise(p, e);
            return Ok(());
        }
        if !matches!(resolution, Value::Object(_)) {
            self.settle_promise(p, 1, resolution);
            return Ok(());
        }
        let then = match self.get_prop(resolution, PropKey::Str(A_THEN)) {
            Ok(t) => t,
            Err(Exc::Throw(e)) => {
                self.reject_promise(p, e);
                return Ok(());
            }
            Err(abort) => return Err(abort),
        };
        if !self.is_callable_val(then) {
            self.settle_promise(p, 1, resolution);
            return Ok(());
        }
        self.enqueue_job(Job::Thenable { promise: p, thenable: resolution, then });
        Ok(())
    }

    pub(crate) fn reject_promise(&mut self, p: ObjId, reason: Value) {
        self.settle_promise(p, 2, reason);
    }

    /// Fulfils with a value known not to be a thenable.
    pub(crate) fn fulfill_promise(&mut self, p: ObjId, value: Value) {
        self.settle_promise(p, 1, value);
    }

    fn settle_promise(&mut self, p: ObjId, state: u8, value: Value) {
        let Some(d) = self.promise_data_mut(p) else { return };
        if d.state != 0 {
            return;
        }
        d.state = state;
        d.value = value;
        let reactions = std::mem::take(&mut d.reactions);
        if state == 2 && !d.handled {
            self.pending_rejections.push(p);
        }
        for reaction in reactions {
            self.enqueue_job(Job::Reaction { reaction, rejected: state == 2, value });
        }
    }

    /// PerformPromiseThen.
    pub(crate) fn promise_then(&mut self, p: ObjId, reaction: Reaction) {
        let Some(d) = self.promise_data_mut(p) else { return };
        d.handled = true;
        match d.state {
            0 => {
                d.reactions.push(reaction);
                self.grow(56);
            }
            s => {
                let value = d.value;
                self.enqueue_job(Job::Reaction { reaction, rejected: s == 2, value });
            }
        }
    }

    /// PromiseResolve(%Promise%, v): `v` itself when it is a plain promise, else a promise for it.
    pub(crate) fn promise_resolve_value(&mut self, v: Value) -> JsResult<ObjId> {
        if let Some(o) = self.as_promise(v) {
            if self.obj(o).proto == self.intr.promise_proto {
                return Ok(o);
            }
        }
        self.rooted(|rt| {
            let p = rt.new_promise_obj(rt.intr.promise_proto);
            rt.resolve_promise(p, v)?;
            Ok(p)
        })
    }

    /// Drains the job queue. A JS exception never escapes; aborts drop the remaining jobs.
    pub(crate) fn run_jobs_internal(&mut self) -> JsResult<()> {
        loop {
            while !self.jobs.is_empty() {
                let r = match self.safepoint() {
                    Ok(()) => match self.jobs.pop_front() {
                        Some(job) => self.rooted(|rt| {
                            job.trace(&mut |v| rt.scratch.push(v));
                            rt.run_job(job)
                        }),
                        None => Ok(()),
                    },
                    Err(e) => Err(e),
                };
                match r {
                    Ok(()) => {}
                    Err(Exc::Throw(e)) => self.report_unhandled(e)?,
                    Err(abort) => {
                        self.jobs.clear();
                        return Err(abort);
                    }
                }
            }
            if self.pending_rejections.is_empty() {
                return Ok(());
            }
            for p in std::mem::take(&mut self.pending_rejections) {
                let reason = match self.promise_data(p) {
                    Some(d) if d.state == 2 && !d.handled => d.value,
                    _ => continue,
                };
                self.report_unhandled(reason)?;
            }
        }
    }

    fn report_unhandled(&mut self, reason: Value) -> JsResult<()> {
        let msg = match self.rooted(|rt| rt.to_rust_string(reason)) {
            Ok(s) => s,
            Err(Exc::Throw(_)) => "<unprintable rejection>".to_string(),
            Err(abort) => return Err(abort),
        };
        if self.unhandled.len() >= MAX_UNHANDLED {
            self.unhandled.remove(0);
        }
        self.unhandled.push(msg);
        Ok(())
    }

    fn run_job(&mut self, job: Job) -> JsResult<()> {
        match job {
            Job::Reaction { reaction: Reaction::Await { coro }, rejected, value } => self.resume_await(coro, rejected, value),
            Job::Reaction { reaction: Reaction::Then { on_ok, on_err, derived }, rejected, value } => {
                let handler = if rejected { on_err } else { on_ok };
                if !self.is_callable_val(handler) {
                    return match derived {
                        Some(d) if rejected => {
                            self.reject_promise(d, value);
                            Ok(())
                        }
                        Some(d) => self.resolve_promise(d, value),
                        None => Ok(()),
                    };
                }
                match (self.call_value(handler, Value::Undefined, &[value]), derived) {
                    (Ok(r), Some(d)) => self.resolve_promise(d, r),
                    (Ok(_), None) => Ok(()),
                    (Err(Exc::Throw(e)), Some(d)) => {
                        self.reject_promise(d, e);
                        Ok(())
                    }
                    (Err(e), _) => Err(e),
                }
            }
            Job::Reaction { reaction: Reaction::ThenCap { on_ok, on_err, cap }, rejected, value } => {
                let (res, rej) = (self.cap_get(cap, 0), self.cap_get(cap, 1));
                let handler = if rejected { on_err } else { on_ok };
                if !self.is_callable_val(handler) {
                    let f = if rejected { rej } else { res };
                    return self.call_value(f, Value::Undefined, &[value]).map(|_| ());
                }
                match self.call_value(handler, Value::Undefined, &[value]) {
                    Ok(r) => self.call_value(res, Value::Undefined, &[r]).map(|_| ()),
                    Err(Exc::Throw(e)) => self.call_value(rej, Value::Undefined, &[e]).map(|_| ()),
                    Err(abort) => Err(abort),
                }
            }
            Job::Thenable { promise, thenable, then } => {
                let (res, rej) = self.resolving_functions(promise);
                match self.call_value(then, thenable, &[res, rej]) {
                    Ok(_) => Ok(()),
                    Err(Exc::Throw(e)) => self.call_value(rej, Value::Undefined, &[e]).map(|_| ()),
                    Err(abort) => Err(abort),
                }
            }
            Job::Call(f) => self.call_value(f, Value::Undefined, &[]).map(|_| ()),
            Job::Cleanup { callback, held } => self.call_value(callback, Value::Undefined, &[held]).map(|_| ()),
        }
    }
}
