//! Module evaluation (post-order, each module once, top-level `await` through promise jobs)
//! and the embedding calls that drive it.

use crate::module::Status;
use crate::module_eval::{async_err, async_ok, import_fulfil, import_reject, Outcome};
use crate::runtime::*;
use crate::value::*;
use crate::JsError;
use std::collections::HashSet;

impl Runtime {
    /// Evaluates the linked graph rooted at `root`; returns the promise of its completion.
    pub(crate) fn module_evaluate(&mut self, root: usize) -> JsResult<ObjId> {
        let done = self.module_done(root);
        if self.modules.list[root].status != Status::Linked {
            return Ok(done);
        }
        // the not-yet-evaluated part of the graph in post-order (iterative depth-first walk)
        let mut order = Vec::new();
        let mut entered = HashSet::new();
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        entered.insert(root);
        self.modules.list[root].status = Status::Evaluating;
        while let Some(top) = stack.last_mut() {
            let (m, i) = *top;
            let dep = self.modules.list[m].deps.get(i).copied();
            self.charge(1)?;
            match dep {
                Some(d) => {
                    top.1 += 1;
                    if self.modules.list.get(d).map(|r| r.status) == Some(Status::Linked) && entered.insert(d) {
                        self.modules.list[d].status = Status::Evaluating;
                        stack.push((d, 0));
                    }
                }
                None => {
                    stack.pop();
                    order.push(m);
                }
            }
        }
        for (pos, &m) in order.iter().enumerate() {
            self.modules.next_order += 1;
            self.modules.list[m].order = self.modules.next_order;
            let deps = self.modules.list[m].deps.clone();
            // a dependency that failed fails this module too
            if let Some(e) = deps.iter().find(|d| self.modules.list[**d].status == Status::Errored).map(|d| self.modules.list[*d].error) {
                self.module_fail(m, e);
                continue;
            }
            let mut waiting: Vec<usize> = deps.iter().copied().filter(|&d| d != m && self.modules.list[d].status == Status::Async).collect();
            waiting.sort_unstable();
            waiting.dedup();
            if !waiting.is_empty() {
                self.modules.list[m].status = Status::Async;
                self.modules.list[m].pending = waiting.len() as u32;
                for d in waiting {
                    self.modules.list[d].async_parents.push(m);
                }
                continue;
            }
            match self.module_run_body(m)? {
                Outcome::Done => self.module_complete(m)?,
                Outcome::Started => {}
                Outcome::Threw(e) => {
                    self.module_fail(m, e);
                    // evaluation stops: what depends on the failure fails, the rest stays linked
                    for &rest in &order[pos + 1..] {
                        let failed = self.modules.list[rest].deps.iter().find(|d| self.modules.list[**d].status == Status::Errored).copied();
                        match failed {
                            Some(d) => {
                                let e = self.modules.list[d].error;
                                self.module_fail(rest, e);
                            }
                            None => self.modules.list[rest].status = Status::Linked,
                        }
                    }
                    break;
                }
            }
        }
        Ok(done)
    }

    /// Runs module `m`'s statements: done, started (top-level `await`), or threw.
    fn module_run_body(&mut self, m: usize) -> JsResult<Outcome> {
        self.modules.list[m].status = Status::Evaluating;
        let has_tla = self.modules.list[m].code.info.has_tla;
        let f = self.module_closure(m, true);
        let r = self.rooted(|rt| {
            rt.scratch.push(f);
            rt.call_value(f, Value::Undefined, &[])
        });
        match r {
            Ok(v) if has_tla => {
                self.modules.list[m].status = Status::Async;
                let Some(p) = self.as_promise(v) else { return Ok(Outcome::Done) };
                let on_ok = Value::Object(self.new_intern("", 1, async_ok, vec![Value::Number(m as f64)]));
                self.scratch.push(on_ok);
                let on_err = Value::Object(self.new_intern("", 1, async_err, vec![Value::Number(m as f64)]));
                self.promise_then(p, Reaction::Then { on_ok, on_err, derived: None });
                Ok(Outcome::Started)
            }
            Ok(_) => Ok(Outcome::Done),
            Err(Exc::Throw(e)) => Ok(Outcome::Threw(e)),
            Err(abort) => Err(abort),
        }
    }

    /// Module `first` finished: it and every waiting module whose last async dependency this
    /// was run on, in evaluation order (a work list, not recursion).
    pub(crate) fn module_complete(&mut self, first: usize) -> JsResult<()> {
        let mut work = vec![first];
        while let Some(m) = work.pop() {
            if m >= self.modules.list.len() || self.modules.list[m].status == Status::Errored {
                continue;
            }
            self.charge(1)?;
            self.modules.list[m].status = Status::Evaluated;
            if let Some(p) = self.modules.list[m].done {
                self.fulfill_promise(p, Value::Undefined);
            }
            let mut ready = Vec::new();
            for p in std::mem::take(&mut self.modules.list[m].async_parents) {
                let r = &mut self.modules.list[p];
                if r.status == Status::Async {
                    r.pending = r.pending.saturating_sub(1);
                    if r.pending == 0 {
                        ready.push(p);
                    }
                }
            }
            ready.sort_by_key(|p| std::cmp::Reverse(self.modules.list[*p].order));
            for p in ready.into_iter().rev() {
                match self.module_run_body(p)? {
                    Outcome::Done => work.push(p),
                    Outcome::Started => {}
                    Outcome::Threw(e) => self.module_fail(p, e),
                }
            }
        }
        Ok(())
    }

    /// Module `first` failed with `error`: so does every module waiting for it.
    pub(crate) fn module_fail(&mut self, first: usize, error: Value) {
        let mut work = vec![first];
        while let Some(m) = work.pop() {
            let Some(r) = self.modules.list.get_mut(m) else { continue };
            if r.status == Status::Errored {
                continue;
            }
            r.status = Status::Errored;
            r.error = error;
            work.extend(std::mem::take(&mut r.async_parents));
            if let Some(p) = r.done {
                self.reject_promise(p, error);
            }
        }
    }

    fn missing_module(url: &str) -> JsError {
        let message = format!("Module '{}' has not been added", url);
        JsError { kind: ErrorKind::Thrown, name: "TypeError".into(), stack: format!("TypeError: {}", message), message, line: 0, column: 0 }
    }

    /// Links and evaluates the graph rooted at `url`; returns its evaluation promise.
    pub fn run_module(&mut self, url: &str) -> Result<Value, JsError> {
        let Some(&m) = self.modules.by_url.get(url) else { return Err(Self::missing_module(url)) };
        self.host_enter();
        match self.module_link(m) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = self.finish(Ok(()));
                return Err(e.to_js());
            }
            Err(abort) => return self.finish(Err(abort)),
        }
        let r = self.module_evaluate(m).map(Value::Object);
        self.finish(r)
    }

    /// Dynamic `import()` calls made since the last call: (request id, resolved address).
    pub fn take_import_requests(&mut self) -> Vec<(u32, String)> {
        std::mem::take(&mut self.modules.requests)
    }

    /// Settles the `import()` promise of request `id`: `Ok` links and evaluates the module
    /// and resolves with its namespace; `Err(message)` rejects with a TypeError.
    pub fn finish_import(&mut self, id: u32, result: Result<(), String>) {
        let Some((p, url)) = self.modules.imports.remove(&id) else { return };
        // out of `imports`, the promise is reachable from nothing: root it before the
        // collection `host_enter` may run
        let mark = self.host_enter_with(&[Value::Object(p)], &[]);
        let r = self.finish_import_inner(p, &url, result);
        if r.is_err() {
            let e = self.make_error("Error", "import() aborted: the step budget or heap limit was exhausted");
            self.reject_promise(p, e);
        }
        let _ = self.finish_with(r, mark);
    }

    fn finish_import_inner(&mut self, p: ObjId, url: &str, result: Result<(), String>) -> JsResult<()> {
        if let Err(msg) = result {
            let e = self.make_error("TypeError", &msg);
            self.reject_promise(p, e);
            return Ok(());
        }
        let Some(&m) = self.modules.by_url.get(url) else {
            let e = self.make_error("TypeError", &Self::missing_module(url).message);
            self.reject_promise(p, e);
            return Ok(());
        };
        if let Err(le) = self.module_link(m)? {
            let e = self.make_error(le.name, &le.message);
            let stack = self.new_str(&le.to_js().stack);
            if let Value::Object(o) = e {
                self.define_value(o, PropKey::Str(A_STACK), stack, HIDDEN);
            }
            self.reject_promise(p, e);
            return Ok(());
        }
        let done = self.module_evaluate(m)?;
        let on_ok = Value::Object(self.new_intern("", 1, import_fulfil, vec![Value::Object(p), Value::Number(m as f64)]));
        self.scratch.push(on_ok);
        let on_err = Value::Object(self.new_intern("", 1, import_reject, vec![Value::Object(p)]));
        self.promise_then(done, Reaction::Then { on_ok, on_err, derived: None });
        Ok(())
    }
}
