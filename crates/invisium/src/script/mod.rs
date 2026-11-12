//! Scripting: runs page scripts on the JavaScript engine (`invisium-js`) and gives them a DOM.
//!
//! Three layers: native primitives over the document (`natives*.rs`), the DOM classes scripts
//! see, written in JavaScript on top of those primitives (`js/*.js`, compiled once per page),
//! and this file, which owns the runtime and is the only thing the page talks to.

mod host;
mod natives;
mod natives_env;
mod serialize;
#[cfg(test)]
mod tests;

pub use host::{ConsoleLevel, ConsoleMessage};
pub(crate) use host::{Effects, HostState, Shared};

use crate::dom::{NodeId, NO_NODE};
use crate::page::UrlJoin;
use invisium_js::{ErrorKind, JsError, Limits, RootId, Runtime, Script, Value};
use natives::{wrap, Cx};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

/// Prefix of the made-up addresses under which inline `<script type="module">` blocks are
/// registered (they have no address of their own; their imports resolve against the page).
pub(crate) const INLINE_MODULE: &str = "inline-module:";

const BOOTSTRAP: &str = concat!(
    include_str!("js/00_events.js"),
    include_str!("js/10_nodes.js"),
    include_str!("js/20_element.js"),
    include_str!("js/30_html.js"),
    include_str!("js/40_document.js"),
    include_str!("js/45_fetch.js"),
    include_str!("js/46_streams.js"),
    include_str!("js/47_misc.js"),
    include_str!("js/50_window.js"),
);

/// Fields of an input event the engine raises on behalf of the user.
#[derive(Clone, Copy)]
pub(crate) struct EventInit<'a> {
    pub x: f32,
    pub y: f32,
    pub button: i32,
    pub key: Option<&'a str>,
    pub data: Option<&'a str>,
    pub submitter: NodeId,
}

impl Default for EventInit<'_> {
    fn default() -> Self {
        EventInit { x: 0.0, y: 0.0, button: 0, key: None, data: None, submitter: NO_NODE }
    }
}

/// Entry points of the bootstrap script, kept alive for the page's lifetime.
#[derive(Clone, Copy)]
struct Exports {
    event: Value,
    timer: Value,
    fetched: Value,
    focus: Value,
    ready: Value,
    loaded: Value,
    uncaught: Value,
}

pub(crate) struct ScriptEngine {
    rt: Runtime,
    cx: Rc<Cx>,
    natives: Value,
    exports: Option<Exports>,
    /// The bootstrap script failed; scripts cannot run on this page.
    broken: bool,
    /// Roots for values this file holds across runtime calls (see `keep`).
    tmp: Vec<RootId>,
}

impl ScriptEngine {
    /// `native_stack` is how much native stack scripts may use (0 = the engine's default).
    pub fn new(join: UrlJoin, native_stack: usize) -> ScriptEngine {
        // Real applications nest calls far deeper than the engine's conservative default, and
        // a large one (YouTube) keeps well over 100 MB of script heap. JS-to-JS calls do not
        // use the native stack, so the frame limit is a matter of memory only.
        let mut rt = Runtime::new(Limits { max_heap_bytes: 512 << 20, max_call_depth: 4000, ..Limits::default() });
        if native_stack > 0 {
            rt.set_native_stack_budget(native_stack);
        }
        // all behaviour lives on prototypes, so the wrapper class needs no property hooks
        let class = rt.register_plain_host_class("Node");
        let st: Shared = Rc::new(RefCell::new(HostState::new(join, true)));
        // Module specifiers are addresses, resolved like any other reference on the page.
        // Bare names ("lodash") would need an import map, which does not exist yet.
        let for_resolver = st.clone();
        rt.set_module_resolver(Box::new(move |spec: &str, referrer: &str| {
            let relative = spec.starts_with('/') || spec.starts_with("./") || spec.starts_with("../");
            let absolute = spec.contains("://") || spec.starts_with("data:");
            if !relative && !absolute {
                return None;
            }
            let st = for_resolver.borrow();
            if referrer.is_empty() || referrer.starts_with(INLINE_MODULE) {
                st.page.resolve(spec)
            } else {
                (st.page.join)(referrer, spec)
            }
        }));
        let cx = Rc::new(Cx { st, class });
        let n = rt.object();
        rt.root(n);
        natives::install(&mut rt, n, &cx);
        natives_env::install(&mut rt, n, &cx);
        ScriptEngine { rt, cx, natives: n, exports: None, broken: false, tmp: Vec::new() }
    }

    /// Root a freshly created value. Outside a native call the runtime may collect garbage on
    /// entry to any call, so every value made here must be kept until `release`.
    fn keep(&mut self, v: Value) -> Value {
        self.tmp.push(self.rt.root(v));
        v
    }

    fn release(&mut self) {
        for id in self.tmp.drain(..) {
            self.rt.unroot(id);
        }
    }

    pub fn state(&self) -> &Shared {
        &self.cx.st
    }

    fn report(&mut self, context: &str, e: &JsError) {
        let what = match e.kind {
            ErrorKind::StepBudget => "script ran too long and was stopped".to_string(),
            ErrorKind::OutOfMemory => "script used too much memory and was stopped".to_string(),
            ErrorKind::StackOverflow => "RangeError: maximum call stack size exceeded".to_string(),
            _ if e.name.is_empty() => e.message.clone(),
            _ => format!("{}: {}", e.name, e.message),
        };
        let text = format!("Uncaught {what} ({context}:{}:{})", e.line, e.column);
        self.cx.st.borrow_mut().log(ConsoleLevel::Error, text);
    }

    /// Build the DOM classes. Must run while the page's document is lent to the host state.
    pub fn boot(&mut self) {
        if self.exports.is_some() || self.broken {
            return;
        }
        let global = self.rt.global();
        // the DOM classes are the same for every page: compile them once per process
        static COMPILED: OnceLock<Result<Script, JsError>> = OnceLock::new();
        let compiled = COMPILED.get_or_init(|| Runtime::compile(BOOTSTRAP, "invisium:dom"));
        let result = match compiled {
            Ok(script) => match self.rt.run(script) {
                Ok(f) => {
                    self.keep(f);
                    self.rt.call(f, Value::Undefined, &[self.natives, global])
                }
                Err(e) => Err(e),
            },
            Err(e) => Err(e.clone()),
        };
        self.release();
        match result {
            Ok(exports) => {
                self.rt.root(exports);
                let mut grab = |name: &str| {
                    let v = self.rt.get(exports, name).unwrap_or(Value::Undefined);
                    self.rt.root(v);
                    v
                };
                self.exports = Some(Exports {
                    event: grab("event"),
                    timer: grab("timer"),
                    fetched: grab("fetched"),
                    focus: grab("focus"),
                    ready: grab("ready"),
                    loaded: grab("loaded"),
                    uncaught: grab("uncaught"),
                });
            }
            Err(e) => {
                self.broken = true;
                self.report("invisium:dom", &e);
            }
        }
    }

    pub fn usable(&self) -> bool {
        self.exports.is_some()
    }

    fn drain_jobs(&mut self) {
        if let Err(e) = self.rt.run_jobs() {
            self.report("promise job", &e);
        }
        for what in self.rt.take_unhandled_rejections() {
            self.cx.st.borrow_mut().log(ConsoleLevel::Error, format!("Uncaught (in promise) {what}"));
        }
    }

    /// Register a module's source. Returns the addresses of the modules it imports.
    pub fn add_module(&mut self, url: &str, source: &str) -> Vec<String> {
        match self.rt.add_module(url, source) {
            Ok(deps) => deps,
            Err(e) => {
                self.report(url, &e);
                Vec::new()
            }
        }
    }

    pub fn has_module(&self, url: &str) -> bool {
        self.rt.has_module(url)
    }

    /// Link and run the module graph rooted at `url` (a `<script type="module">`).
    pub fn run_module(&mut self, url: &str) {
        if self.exports.is_none() {
            return;
        }
        if let Err(e) = self.rt.run_module(url) {
            self.report(url, &e);
        }
        self.drain_jobs();
    }

    /// `import()` calls made by script since the last call: (request id, module address).
    pub fn take_import_requests(&mut self) -> Vec<(u32, String)> {
        self.rt.take_import_requests()
    }

    pub fn finish_import(&mut self, id: u32, result: Result<(), String>) {
        self.rt.finish_import(id, result);
        self.drain_jobs();
    }

    fn call_export(&mut self, f: Value, args: &[Value], context: &str) -> Option<Value> {
        let r = self.rt.call(f, Value::Undefined, args);
        let out = match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.report(context, &e);
                None
            }
        };
        self.drain_jobs();
        out
    }

    /// Run a classic script. `node` is its `<script>` element (`document.currentScript`).
    pub fn run_script(&mut self, source: &str, url: &str, node: NodeId) {
        let Some(ex) = self.exports else { return };
        self.cx.st.borrow_mut().current_script = node;
        let r = self.rt.eval(source, url);
        if let Err(e) = r {
            self.report(url, &e);
            let msg = if e.name.is_empty() { e.message.clone() } else { format!("{}: {}", e.name, e.message) };
            let m = self.rt.string(&msg);
            self.keep(m);
            let _ = self.rt.call(ex.uncaught, Value::Undefined, &[m]);
            self.release();
        }
        // The microtask checkpoint is part of running the script, so promise reactions queued
        // by it still see `document.currentScript` (bundler runtimes read it after an `await`).
        self.drain_jobs();
        self.cx.st.borrow_mut().current_script = NO_NODE;
    }

    /// Dispatch a user-input event at `target` (`NO_NODE` = the window). Returns false if a
    /// listener called `preventDefault`.
    pub fn event(&mut self, target: NodeId, kind: &str, init: EventInit) -> bool {
        let Some(ex) = self.exports else { return true };
        let t = wrap(&mut self.rt, &self.cx, target);
        let ty = self.rt.string(kind);
        self.keep(ty);
        let o = self.rt.object();
        self.keep(o);
        let _ = self.rt.set(o, "clientX", Value::Number(init.x as f64));
        let _ = self.rt.set(o, "clientY", Value::Number(init.y as f64));
        let _ = self.rt.set(o, "button", Value::Number(init.button as f64));
        if let Some(k) = init.key {
            let v = self.rt.string(k);
            self.keep(v);
            let _ = self.rt.set(o, "key", v);
        }
        if let Some(d) = init.data {
            let v = self.rt.string(d);
            self.keep(v);
            let _ = self.rt.set(o, "data", v);
        }
        if init.submitter != NO_NODE {
            let v = wrap(&mut self.rt, &self.cx, init.submitter);
            let _ = self.rt.set(o, "submitter", v);
        }
        let r = self.call_export(ex.event, &[t, ty, o], "event listener");
        self.release();
        match r {
            Some(v) => self.rt.to_boolean(v),
            None => true,
        }
    }

    pub fn focus_changed(&mut self, prev: NodeId, next: NodeId) {
        let Some(ex) = self.exports else { return };
        let p = wrap(&mut self.rt, &self.cx, prev);
        let n = wrap(&mut self.rt, &self.cx, next);
        self.call_export(ex.focus, &[p, n], "focus listener");
    }

    /// `interactive` (fires `DOMContentLoaded`) or `complete` (fires `load`).
    pub fn ready(&mut self, state: &str) {
        let Some(ex) = self.exports else { return };
        let s = self.rt.string(state);
        self.keep(s);
        self.call_export(ex.ready, &[s], "lifecycle listener");
        self.release();
    }

    /// A resource element (`<script src>`, `<img>`, `<link>`) finished loading or failed.
    pub fn resource_done(&mut self, node: NodeId, ok: bool) {
        let Some(ex) = self.exports else { return };
        let n = wrap(&mut self.rt, &self.cx, node);
        self.call_export(ex.loaded, &[n, Value::Bool(ok)], "load listener");
    }

    /// A request made by script finished. `None` = it failed; script is never told why.
    pub fn fetch_done(&mut self, id: u32, result: Option<&crate::page::FetchResult>) {
        let Some(ex) = self.exports else { return };
        let idv = Value::Number(id as f64);
        match result {
            Some(r) => {
                let url = self.rt.string(&r.url);
                self.keep(url);
                let mut flat = Vec::with_capacity(r.headers.len() * 2);
                for (k, v) in &r.headers {
                    let kv = self.rt.string(k);
                    flat.push(self.keep(kv));
                    let vv = self.rt.string(v);
                    flat.push(self.keep(vv));
                }
                let headers = self.rt.array(&flat);
                self.keep(headers);
                let charset = r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).and_then(|(_, v)| {
                    let low = v.to_ascii_lowercase();
                    low.find("charset=").map(|p| low[p + 8..].trim_matches(|c: char| c == '"' || c == '\'' || c == ' ').split(';').next().unwrap_or("").to_string())
                });
                // the bytes go to script as they are; text is decoded there only if it is asked for
                let body = self.rt.array_buffer(&r.body);
                self.keep(body);
                let charset = match charset {
                    Some(c) => {
                        let v = self.rt.string(&c);
                        self.keep(v)
                    }
                    None => Value::Null,
                };
                let args = [idv, Value::Bool(true), Value::Number(r.status as f64), url, headers, body, charset];
                self.call_export(ex.fetched, &args, "request callback");
            }
            None => {
                self.call_export(ex.fetched, &[idv, Value::Bool(false)], "request callback");
            }
        }
        self.release();
    }

    /// Earliest deadline of a pending timer, in the page's clock.
    pub fn next_timer(&self) -> Option<f64> {
        self.cx.st.borrow().timers.iter().map(|t| t.due).fold(None, |m, d| Some(m.map_or(d, |m: f64| m.min(d))))
    }

    /// Run every timer that is due at `now`. Returns how many fired.
    pub fn run_timers(&mut self, now: f64) -> usize {
        let Some(ex) = self.exports else { return 0 };
        let mut due: Vec<(f64, u32)> = {
            let mut st = self.cx.st.borrow_mut();
            st.now_ms = now;
            st.timers.iter().filter(|t| t.due <= now).map(|t| (t.due, t.id)).collect()
        };
        due.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
        due.truncate(500);
        let mut fired = 0;
        for (_, id) in due {
            // a timer may have been cleared by an earlier callback in this batch
            let live = {
                let mut st = self.cx.st.borrow_mut();
                match st.timers.iter().position(|t| t.id == id) {
                    Some(i) => {
                        match st.timers[i].interval {
                            Some(every) => st.timers[i].due = now + every,
                            None => {
                                st.timers.swap_remove(i);
                            }
                        }
                        true
                    }
                    None => false,
                }
            };
            if live {
                self.call_export(ex.timer, &[Value::Number(id as f64)], "timer");
                fired += 1;
            }
        }
        fired
    }

    pub fn heap_bytes(&self) -> usize {
        self.rt.heap_bytes()
    }
}
