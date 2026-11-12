//! ES module records: registration (`add_module`), linking (environments, live import
//! bindings, hoisting), export resolution and namespace objects. Evaluation, top-level
//! `await` and dynamic `import()` are in `module_eval.rs`. The engine never fetches: the
//! embedder supplies sources, and its resolver turns specifiers into addresses.

use crate::ast::{ExportEntry, Pos};
use crate::compile_module::ModuleCode;
use crate::runtime::*;
use crate::value::*;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Status {
    New,
    Linked,
    Evaluating,
    /// Waiting for a top-level `await` (its own or a dependency's).
    Async,
    Evaluated,
    Errored,
}

pub(crate) struct ModuleRec {
    pub url: String,
    pub code: Arc<ModuleCode>,
    pub atoms: Rc<[Atom]>,
    /// Resolved address of each request (`None`: the resolver refused it).
    pub requested: Vec<Option<String>>,
    pub deps: Vec<usize>,
    pub env: Option<EnvId>,
    pub status: Status,
    pub error: Value,
    pub namespace: Option<ObjId>,
    /// Settles when this module's evaluation completes (made on demand).
    pub done: Option<ObjId>,
    /// Async dependencies still running, and the modules waiting for this one.
    pub pending: u32,
    pub async_parents: Vec<usize>,
    /// Post-order position in the evaluation that reached it (async completion order).
    pub order: u32,
}

/// `(specifier, referrer address) -> resolved address`.
pub(crate) type Resolver = Box<dyn FnMut(&str, &str) -> Option<String>>;

#[derive(Default)]
pub(crate) struct Modules {
    pub list: Vec<ModuleRec>,
    pub by_url: HashMap<String, usize>,
    pub resolver: Option<Resolver>,
    /// Dynamic imports not yet handed to the embedder, and those awaiting `finish_import`.
    pub requests: Vec<(u32, String)>,
    pub imports: HashMap<u32, (ObjId, String)>,
    pub next_import: u32,
    pub next_order: u32,
}

impl Modules {
    pub fn trace(&self, f: &mut dyn FnMut(Value), fe: &mut dyn FnMut(EnvId)) {
        for m in &self.list {
            if let Some(e) = m.env {
                fe(e);
            }
            f(m.error);
            for o in [m.namespace, m.done].into_iter().flatten() {
                f(Value::Object(o));
            }
        }
        for (p, _) in self.imports.values() {
            f(Value::Object(*p));
        }
    }
}

/// A link failure, reported with the module address and the position of the declaration.
pub(crate) struct LinkError {
    pub name: &'static str,
    pub message: String,
    pub url: String,
    pub pos: Pos,
}

impl LinkError {
    pub fn to_js(&self) -> crate::JsError {
        let kind = if self.name == "SyntaxError" { ErrorKind::Syntax } else { ErrorKind::Thrown };
        let stack = format!("{}: {}\n    at {}:{}:{}", self.name, self.message, self.url, self.pos.line, self.pos.col);
        crate::JsError { kind, name: self.name.into(), message: self.message.clone(), line: self.pos.line, column: self.pos.col, stack }
    }
}

/// Where an export leads: a binding slot of a module, or a module's namespace object.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Target {
    Binding(usize, u32),
    Namespace(usize),
}

pub(crate) enum Resolved {
    Found(Target),
    NotFound,
    Ambiguous,
    /// More resolution work than any real graph needs.
    TooDeep,
}

/// What the module itself says about an export name.
enum Step {
    Done(Resolved),
    Follow(usize, String),
    Stars,
}

const MAX_RESOLVE_WORK: u64 = 1_000_000;

impl Modules {
    /// The part of ResolveExport that looks only at module `m` itself.
    fn resolve_local(&self, m: usize, name: &str, seen: &mut HashSet<(usize, String)>) -> Step {
        if !seen.insert((m, name.to_string())) {
            return Step::Done(Resolved::NotFound);
        }
        let Some(rec) = self.list.get(m) else { return Step::Done(Resolved::NotFound) };
        let dep = |r: usize| rec.deps.get(r).copied().unwrap_or(usize::MAX);
        for e in &rec.code.info.exports {
            match e {
                ExportEntry::Local { name: n, local, .. } if n == name => {
                    // re-export of an import binding: follow it
                    if let Some(im) = rec.code.info.imports.iter().find(|i| i.local == *local && i.import_name.is_some()) {
                        return Step::Follow(dep(im.request), im.import_name.clone().unwrap_or_default());
                    }
                    return Step::Done(match rec.code.slots.get(local) {
                        Some(s) => Resolved::Found(Target::Binding(m, *s)),
                        None => Resolved::NotFound,
                    });
                }
                ExportEntry::Indirect { name: n, request, import_name, .. } if n == name => {
                    return match import_name {
                        None => Step::Done(Resolved::Found(Target::Namespace(dep(*request)))),
                        Some(i) => Step::Follow(dep(*request), i.clone()),
                    };
                }
                _ => {}
            }
        }
        let has_star = rec.code.info.exports.iter().any(|e| matches!(e, ExportEntry::Star { .. }));
        if name == "default" || !has_star {
            return Step::Done(Resolved::NotFound);
        }
        Step::Stars
    }

    /// ResolveExport, iteratively (re-export chains and `export *` fan-out use an explicit
    /// stack); `work` counts the modules visited and is charged to the step budget by the caller.
    pub fn resolve_export(&self, m0: usize, name0: &str, work: &mut u64) -> Resolved {
        struct Frame {
            m: usize,
            name: String,
            next: usize,
            star: Option<Target>,
            waiting: bool,
        }
        let mut seen = HashSet::new();
        let mut stack: Vec<Frame> = Vec::new();
        let mut call = Some((m0, name0.to_string()));
        let mut ret = Resolved::NotFound;
        loop {
            if let Some((m, name)) = call.take() {
                *work += 1;
                if *work > MAX_RESOLVE_WORK {
                    return Resolved::TooDeep;
                }
                match self.resolve_local(m, &name, &mut seen) {
                    Step::Done(r) => ret = r,
                    Step::Follow(m2, n2) => {
                        call = Some((m2, n2));
                        continue;
                    }
                    Step::Stars => stack.push(Frame { m, name, next: 0, star: None, waiting: false }),
                }
            }
            // hand the latest result to the frame that asked for it
            let Some(top) = stack.last_mut() else { return ret };
            if top.waiting {
                top.waiting = false;
                match std::mem::replace(&mut ret, Resolved::NotFound) {
                    Resolved::NotFound => {}
                    Resolved::Found(t) if top.star.is_none() || top.star == Some(t) => top.star = Some(t),
                    Resolved::Found(_) => {
                        stack.pop();
                        ret = Resolved::Ambiguous;
                        continue;
                    }
                    other => {
                        stack.pop();
                        ret = other;
                        continue;
                    }
                }
            }
            let exports = self.list.get(top.m).map(|r| &r.code.info.exports[..]).unwrap_or(&[]);
            let next = exports.iter().enumerate().skip(top.next).find_map(|(i, e)| match e {
                ExportEntry::Star { request, .. } => Some((i, *request)),
                _ => None,
            });
            match next {
                Some((i, request)) => {
                    top.next = i + 1;
                    top.waiting = true;
                    let dep = self.list.get(top.m).and_then(|r| r.deps.get(request).copied()).unwrap_or(usize::MAX);
                    call = Some((dep, top.name.clone()));
                }
                None => {
                    ret = top.star.map(Resolved::Found).unwrap_or(Resolved::NotFound);
                    stack.pop();
                }
            }
        }
    }

    /// GetExportedNames, iteratively over `export *` edges.
    pub fn exported_names(&self, root: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![(root, true)];
        while let Some((m, is_root)) = stack.pop() {
            if !seen.insert(m) {
                continue;
            }
            let Some(rec) = self.list.get(m) else { continue };
            for e in &rec.code.info.exports {
                match e {
                    ExportEntry::Local { name, .. } | ExportEntry::Indirect { name, .. } => {
                        if (is_root || name != "default") && !out.contains(name) {
                            out.push(name.clone());
                        }
                    }
                    ExportEntry::Star { request, .. } => {
                        if let Some(d) = rec.deps.get(*request) {
                            stack.push((*d, false));
                        }
                    }
                }
            }
        }
        out
    }
}

/// The getter behind a namespace property: slots (module index, binding slot or -1 for the
/// module's own namespace).
fn ns_getter(rt: &mut Runtime, slots: &[Value], _: Value, _: &[Value]) -> JsResult<Value> {
    let num = |i: usize| match slots.get(i) {
        Some(Value::Number(n)) => *n,
        _ => -1.0,
    };
    let (m, slot) = (num(0) as usize, num(1));
    if slot < 0.0 {
        return Ok(Value::Object(rt.module_namespace(m)?));
    }
    let env = rt.modules.list.get(m).and_then(|r| r.env);
    match env.and_then(|e| rt.env(e)).and_then(|e| e.slots.get(slot as usize).copied().flatten()) {
        Some(v) => Ok(v),
        None => Err(rt.tdz_error()),
    }
}

impl Runtime {
    pub fn set_module_resolver(&mut self, resolve: Box<dyn FnMut(&str, &str) -> Option<String>>) {
        self.modules.resolver = Some(resolve);
    }

    /// The resolver's answer; without a resolver a specifier is its own address.
    pub(crate) fn resolve_specifier(&mut self, spec: &str, referrer: &str) -> Option<String> {
        match self.modules.resolver.as_mut() {
            Some(r) => r(spec, referrer),
            None => Some(spec.to_string()),
        }
    }

    pub fn add_module(&mut self, url: &str, source: &str) -> Result<Vec<String>, crate::JsError> {
        if let Some(&i) = self.modules.by_url.get(url) {
            return Ok(Self::unique(&self.modules.list[i].requested));
        }
        let code = crate::compile_module_source(source, url).map_err(|e| crate::syntax_error(e, url))?;
        let requested: Vec<Option<String>> = code.info.requests.iter().map(|s| self.resolve_specifier(s, url)).collect();
        let atoms: Rc<[Atom]> = code.names.iter().map(|n| self.intern16(n)).collect::<Vec<Atom>>().into();
        // the retained source text and code count toward the heap
        self.grow(source.len() * 4 + code.nslots as usize * 16);
        let out = Self::unique(&requested);
        let rec = ModuleRec {
            url: url.to_string(),
            code: Arc::new(code),
            atoms,
            requested,
            deps: Vec::new(),
            env: None,
            status: Status::New,
            error: Value::Undefined,
            namespace: None,
            done: None,
            pending: 0,
            async_parents: Vec::new(),
            order: 0,
        };
        self.modules.list.push(rec);
        self.modules.by_url.insert(url.to_string(), self.modules.list.len() - 1);
        Ok(out)
    }

    fn unique(requested: &[Option<String>]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in requested.iter().flatten() {
            if !out.contains(r) {
                out.push(r.clone());
            }
        }
        out
    }

    pub fn has_module(&self, url: &str) -> bool {
        self.modules.by_url.contains_key(url)
    }

    /// A function running a module's `init` or `body` code in its environment.
    pub(crate) fn module_closure(&mut self, m: usize, body: bool) -> Value {
        let rec = &self.modules.list[m];
        let code = if body { rec.code.body.clone() } else { rec.code.init.clone() };
        let c = Closure { code, atoms: rec.atoms.clone(), env: rec.env, this_val: Value::Undefined, new_target: Value::Undefined, home: None, ctor_callee: None, fields: None };
        let fp = self.intr.function_proto;
        Value::Object(self.alloc(Object::new(fp, ObjKind::Function(Box::new(FuncData::Closure(c))))))
    }

    /// The namespace object of module `m` (made once): a null-prototype, non-extensible object
    /// with an enumerable getter per export name, in code-unit order.
    pub(crate) fn module_namespace(&mut self, m: usize) -> JsResult<ObjId> {
        if let Some(ns) = self.modules.list.get(m).and_then(|r| r.namespace) {
            return Ok(ns);
        }
        let o = self.new_object(None);
        if let Some(r) = self.modules.list.get_mut(m) {
            r.namespace = Some(o);
        }
        let mut names = self.modules.exported_names(m);
        names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        self.charge(1 + names.len() as u64)?;
        for name in names {
            let mut work = 0;
            let r = self.modules.resolve_export(m, &name, &mut work);
            self.charge(work)?;
            let slots = match r {
                Resolved::Found(Target::Binding(t, s)) => vec![Value::Number(t as f64), Value::Number(s as f64)],
                Resolved::Found(Target::Namespace(t)) => vec![Value::Number(t as f64), Value::Number(-1.0)],
                _ => continue,
            };
            let g = self.new_intern(&name, 0, ns_getter, slots);
            let k = self.key_str(&name);
            self.define_own(o, k, Prop { value: Value::Object(g), setter: Value::Undefined, flags: ACCESSOR | ENUMERABLE });
        }
        let tag = self.new_str("Module");
        self.define_value(o, PropKey::Sym(SYM_TO_STRING_TAG), tag, 0);
        self.obj_mut(o).extensible = false;
        Ok(o)
    }
}
