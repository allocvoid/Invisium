//! Module linking and evaluation: environments and live import bindings, evaluation in
//! dependency post-order (each module once), top-level `await` (a module waits for its async
//! dependencies, then runs from a promise job), dynamic `import()` and the embedding calls
//! `run_module`, `take_import_requests` and `finish_import`. Every graph walk is iterative
//! and charges the step budget per module and edge.

use crate::ast::{ExportEntry, ModuleInfo, Pos};
use crate::builtins::arg;
use crate::module::{LinkError, Resolved, Status, Target};
use crate::promise::slot;
use crate::runtime::*;
use crate::value::*;
use std::collections::HashSet;

/// What running a module body did.
pub(crate) enum Outcome {
    Done,
    Started,
    Threw(Value),
}

/// Where request `i` is written (for error positions).
fn request_pos(info: &ModuleInfo, i: usize) -> Pos {
    if let Some(im) = info.imports.iter().find(|im| im.request == i) {
        return im.pos;
    }
    for e in &info.exports {
        match e {
            ExportEntry::Indirect { request, pos, .. } | ExportEntry::Star { request, pos } if *request == i => return *pos,
            _ => {}
        }
    }
    Pos::default()
}

fn num_slot(slots: &[Value], i: usize) -> usize {
    match slot(slots, i) {
        Value::Number(n) if n >= 0.0 => n as usize,
        _ => usize::MAX,
    }
}

pub(crate) fn async_ok(rt: &mut Runtime, slots: &[Value], _: Value, _: &[Value]) -> JsResult<Value> {
    rt.module_complete(num_slot(slots, 0))?;
    Ok(Value::Undefined)
}

pub(crate) fn async_err(rt: &mut Runtime, slots: &[Value], _: Value, a: &[Value]) -> JsResult<Value> {
    rt.module_fail(num_slot(slots, 0), arg(a, 0));
    Ok(Value::Undefined)
}

pub(crate) fn import_fulfil(rt: &mut Runtime, slots: &[Value], _: Value, _: &[Value]) -> JsResult<Value> {
    if let Value::Object(p) = slot(slots, 0) {
        let ns = rt.module_namespace(num_slot(slots, 1))?;
        rt.resolve_promise(p, Value::Object(ns))?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn import_reject(rt: &mut Runtime, slots: &[Value], _: Value, a: &[Value]) -> JsResult<Value> {
    if let Value::Object(p) = slot(slots, 0) {
        rt.reject_promise(p, arg(a, 0));
    }
    Ok(Value::Undefined)
}

/// The hidden global `*import*` behind `import(specifier)`: a promise, settled by the embedder
/// through `finish_import` (or rejected now when the resolver refuses the specifier).
fn dynamic_import(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let p = rt.new_promise_obj(rt.intr.promise_proto);
    rt.scratch.push(Value::Object(p));
    let referrer = rt.frames.last().map(|f| f.code.script.to_string()).unwrap_or_default();
    let spec = match rt.to_rust_string(arg(a, 0)) {
        Ok(s) => s,
        Err(Exc::Throw(e)) => {
            rt.reject_promise(p, e);
            return Ok(Value::Object(p));
        }
        Err(abort) => return Err(abort),
    };
    match rt.resolve_specifier(&spec, &referrer) {
        None => {
            let e = rt.make_error("TypeError", &format!("Failed to resolve module specifier '{}'", spec));
            rt.reject_promise(p, e);
        }
        Some(url) => {
            let id = rt.modules.next_import;
            rt.modules.next_import = id.wrapping_add(1);
            rt.grow(64 + url.len());
            rt.modules.imports.insert(id, (p, url.clone()));
            rt.modules.requests.push((id, url));
        }
    }
    Ok(Value::Object(p))
}

pub(crate) fn setup(rt: &mut Runtime) {
    let f = rt.native_fn("import", 1, dynamic_import);
    let a = rt.intern("*import*");
    rt.global_lex.insert(a, GlobalLex { value: Some(Value::Object(f)), mutable: false });
}

impl Runtime {
    fn status(&self, m: usize) -> Status {
        self.modules.list.get(m).map(|r| r.status).unwrap_or(Status::Errored)
    }

    /// Links the not-yet-linked part of the graph rooted at `root`.
    pub(crate) fn module_link(&mut self, root: usize) -> JsResult<Result<(), LinkError>> {
        let mut graph = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![root];
        while let Some(m) = stack.pop() {
            if !seen.insert(m) || self.status(m) != Status::New {
                continue;
            }
            graph.push(m);
            let rec = &self.modules.list[m];
            let mut deps = Vec::with_capacity(rec.requested.len());
            for (i, r) in rec.requested.iter().enumerate() {
                let found = r.as_ref().and_then(|u| self.modules.by_url.get(u).copied());
                let message = match (r, found) {
                    (_, Some(d)) => {
                        deps.push(d);
                        continue;
                    }
                    (None, _) => format!("Failed to resolve module specifier '{}'", rec.code.info.requests.get(i).map(|s| s.as_str()).unwrap_or("")),
                    (Some(u), None) => format!("Cannot find module '{}' imported from '{}'", u, rec.url),
                };
                return Ok(Err(LinkError { name: "TypeError", message, url: rec.url.clone(), pos: request_pos(&rec.code.info, i) }));
            }
            self.charge(1 + deps.len() as u64)?;
            stack.extend(deps.iter().copied());
            self.modules.list[m].deps = deps;
        }
        for &m in &graph {
            let n = self.modules.list[m].code.nslots;
            let e = self.alloc_env(None, n);
            self.modules.list[m].env = Some(e);
        }
        if let Err(e) = self.bind_imports(&graph)? {
            for &m in &graph {
                self.modules.list[m].env = None;
            }
            return Ok(Err(e));
        }
        for &m in &graph {
            let f = self.module_closure(m, false);
            self.rooted(|rt| {
                rt.scratch.push(f);
                rt.call_value(f, Value::Undefined, &[])
            })?;
            self.modules.list[m].status = Status::Linked;
        }
        Ok(Ok(()))
    }

    fn set_module_slot(&mut self, m: usize, slot: u32, v: Value) {
        if let Some(e) = self.modules.list[m].env {
            if let Some(s) = self.env_mut(e).and_then(|e| e.slots.get_mut(slot as usize)) {
                *s = Some(v);
            }
        }
    }

    /// Fills every import binding (a cell naming the exporter's binding, or a namespace) and
    /// `import.meta`.
    fn bind_imports(&mut self, graph: &[usize]) -> JsResult<Result<(), LinkError>> {
        for &m in graph {
            let code = self.modules.list[m].code.clone();
            let url = self.modules.list[m].url.clone();
            self.charge(1 + code.info.imports.len() as u64)?;
            for im in &code.info.imports {
                let dep = self.modules.list[m].deps.get(im.request).copied().unwrap_or(usize::MAX);
                let slot = code.slots.get(&im.local).copied().unwrap_or(0);
                let value = match &im.import_name {
                    None => Value::Object(self.module_namespace(dep)?),
                    Some(name) => {
                        let mut work = 0;
                        let r = self.modules.resolve_export(dep, name, &mut work);
                        self.charge(work)?;
                        let spec = code.info.requests.get(im.request).cloned().unwrap_or_default();
                        let message = match r {
                            Resolved::Found(Target::Binding(t, s)) => match self.modules.list.get(t).and_then(|r| r.env) {
                                Some(e) => {
                                    let cell = self.alloc(Object::new(None, ObjKind::ImportCell(e, s)));
                                    self.set_module_slot(m, slot, Value::Object(cell));
                                    continue;
                                }
                                None => format!("The requested module '{}' is not linked", spec),
                            },
                            Resolved::Found(Target::Namespace(t)) => {
                                let ns = self.module_namespace(t)?;
                                self.set_module_slot(m, slot, Value::Object(ns));
                                continue;
                            }
                            Resolved::NotFound => format!("The requested module '{}' does not provide an export named '{}'", spec, name),
                            Resolved::Ambiguous => format!("The requested module '{}' contains conflicting star exports for name '{}'", spec, name),
                            Resolved::TooDeep => format!("Resolving export '{}' of module '{}' is too deeply nested", name, spec),
                        };
                        return Ok(Err(LinkError { name: "SyntaxError", message, url: url.clone(), pos: im.pos }));
                    }
                };
                self.set_module_slot(m, slot, value);
            }
            let meta = self.new_object(None);
            self.set_module_slot(m, code.meta_slot, Value::Object(meta));
            let u = self.new_str(&url);
            let k = self.key_str("url");
            self.define_value(meta, k, u, DEFAULT_FLAGS);
        }
        Ok(Ok(()))
    }

    /// The promise that settles with module `m`'s evaluation.
    pub(crate) fn module_done(&mut self, m: usize) -> ObjId {
        if let Some(p) = self.modules.list[m].done {
            return p;
        }
        let p = self.new_promise_obj(self.intr.promise_proto);
        self.modules.list[m].done = Some(p);
        match self.modules.list[m].status {
            Status::Evaluated => self.fulfill_promise(p, Value::Undefined),
            Status::Errored => {
                let e = self.modules.list[m].error;
                self.reject_promise(p, e);
            }
            _ => {}
        }
        p
    }
}
