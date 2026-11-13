//! Scripting: runs page scripts on the JavaScript engine (`invisium-js`) and gives them a DOM.
//!
//! Three layers: native primitives over the document (`natives*.rs`), the DOM classes scripts
//! see, written in JavaScript on top of those primitives (`js/*.js`, compiled once per page),
//! and this file, which owns the runtime and is the only thing the page talks to.

mod host;
mod natives;
mod natives_anim;
mod natives_canvas;
mod natives_frames;
mod natives_sockets;
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
    include_str!("js/48_animation.js"),
    include_str!("js/49_canvas.js"),
    include_str!("js/49_frames.js"),
    include_str!("js/49_sockets.js"),
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
    /// Modifier keys that are down: `MOD_CTRL`, `MOD_SHIFT`, `MOD_ALT`.
    pub mods: u8,
}

/// With profiling on (`INVIS_PROFILE`), say which piece of script work held the main thread
/// for long: `what` ran from `start` to `called`, and its promise jobs from then until now.
fn long_task(what: &str, start: Option<std::time::Instant>, called: Option<std::time::Instant>) {
    let (Some(start), Some(called)) = (start, called) else { return };
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    let (run, jobs) = (ms(called - start), ms(called.elapsed()));
    if run + jobs >= 30.0 {
        let tail: String = what.chars().rev().take(70).collect::<Vec<_>>().into_iter().rev().collect();
        eprintln!("long task: {run:.0} ms + {jobs:.0} ms of promise jobs in {tail}");
    }
}

/// The time now, if profiling is on (the clock is not read otherwise).
fn stopwatch() -> Option<std::time::Instant> {
    crate::draw::prof::enabled().then(std::time::Instant::now)
}

pub(crate) const MOD_CTRL: u8 = 1;
pub(crate) const MOD_SHIFT: u8 = 2;
pub(crate) const MOD_ALT: u8 = 4;

impl Default for EventInit<'_> {
    fn default() -> Self {
        EventInit { x: 0.0, y: 0.0, button: 0, key: None, data: None, submitter: NO_NODE, mods: 0 }
    }
}

/// What the pointer did (see `ScriptEngine::pointer`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PointerInput {
    Move = 0,
    Down = 1,
    Up = 2,
    /// It is no longer over the page.
    Leave = 3,
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
    files: Value,
    frame: Value,
    anim: Value,
    anim_done: Value,
    message: Value,
    pointer: Value,
    socket: Value,
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
        // Debugging aid: collect garbage at every n-th point where the engine may collect, so
        // a value this glue forgot to root fails at once instead of once in a thousand loads.
        if crate::draw::prof::enabled() {
            rt.set_profile(true);
        }
        if let Some(n) = std::env::var("INVIS_GC_STRESS").ok().and_then(|v| v.parse::<u32>().ok()) {
            rt.set_gc_stress(n);
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
        let (gl_class, c2d_class) = natives_canvas::classes(&mut rt);
        let cx = Rc::new(Cx { st, class, gl_class, c2d_class });
        let n = rt.object();
        rt.root(n);
        natives::install(&mut rt, n, &cx);
        natives_env::install(&mut rt, n, &cx);
        natives_anim::install(&mut rt, n, &cx);
        natives_canvas::install(&mut rt, n, &cx);
        natives_frames::install(&mut rt, n, &cx);
        natives_sockets::install(&mut rt, n, &cx);
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
                    files: grab("files"),
                    frame: grab("frame"),
                    anim: grab("anim"),
                    anim_done: grab("animDone"),
                    message: grab("message"),
                    pointer: grab("pointer"),
                    socket: grab("socket"),
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
        let t0 = stopwatch();
        let r = self.rt.call(f, Value::Undefined, args);
        let out = match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.report(context, &e);
                None
            }
        };
        let t1 = stopwatch();
        self.drain_jobs();
        long_task(context, t0, t1);
        out
    }

    /// Run a classic script. `node` is its `<script>` element (`document.currentScript`).
    pub fn run_script(&mut self, source: &str, url: &str, node: NodeId) {
        let Some(ex) = self.exports else { return };
        self.cx.st.borrow_mut().current_script = node;
        let t0 = stopwatch();
        let r = self.rt.eval(source, url);
        if let Err(e) = r {
            self.report(url, &e);
            let msg = if e.name.is_empty() { e.message.clone() } else { format!("{}: {}", e.name, e.message) };
            let m = self.rt.string(&msg);
            self.keep(m);
            let _ = self.rt.call(ex.uncaught, Value::Undefined, &[m]);
            self.release();
        }
        let t1 = stopwatch();
        // The microtask checkpoint is part of running the script, so promise reactions queued
        // by it still see `document.currentScript` (bundler runtimes read it after an `await`).
        self.drain_jobs();
        long_task(url, t0, t1);
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
        for (bit, name) in [(MOD_CTRL, "ctrlKey"), (MOD_SHIFT, "shiftKey"), (MOD_ALT, "altKey")] {
            if init.mods & bit != 0 {
                let _ = self.rt.set(o, name, Value::Bool(true));
            }
        }
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
        // Things a page may only do in answer to the user (opening the file dialog) are
        // allowed while one of these events is being handled.
        let gesture = matches!(kind, "click" | "mousedown" | "mouseup" | "keydown" | "keyup" | "pointerdown" | "pointerup");
        self.cx.st.borrow_mut().user_gesture = gesture;
        let r = self.call_export(ex.event, &[t, ty, o], "event listener");
        self.cx.st.borrow_mut().user_gesture = false;
        self.release();
        match r {
            Some(v) => self.rt.to_boolean(v),
            None => true,
        }
    }

    /// The pointer moved, a button went down or came up, or it left the page. `target` is the
    /// element under it, `button` the one that changed, `buttons` those that are down
    /// afterwards (the bits of `MouseEvent.buttons`). Script hears it as pointer and mouse
    /// events, with the events for crossing element borders in front. Returns false if a
    /// listener cancelled the mouse event of a press or a release.
    pub fn pointer(&mut self, kind: PointerInput, target: NodeId, pos: (f32, f32), button: i32, buttons: u8, mods: u8) -> bool {
        let Some(ex) = self.exports else { return true };
        let t = wrap(&mut self.rt, &self.cx, target);
        let args = [
            Value::Number(kind as u8 as f64),
            t,
            Value::Number(pos.0 as f64),
            Value::Number(pos.1 as f64),
            Value::Number(button as f64),
            Value::Number(buttons as f64),
            Value::Number(mods as f64),
        ];
        // a press or a release is the user asking for something (see `event`)
        let gesture = matches!(kind, PointerInput::Down | PointerInput::Up);
        self.cx.st.borrow_mut().user_gesture = gesture;
        let r = self.call_export(ex.pointer, &args, "event listener");
        self.cx.st.borrow_mut().user_gesture = false;
        self.release();
        match r {
            Some(v) => self.rt.to_boolean(v),
            None => true,
        }
    }

    /// The user chose files for a file control: (file id, name, type, last modified, size) each.
    pub fn files_changed(&mut self, control: NodeId, files: &[(u32, String, String, f64, f64)]) {
        let Some(ex) = self.exports else { return };
        let node = wrap(&mut self.rt, &self.cx, control);
        let mut flat = Vec::with_capacity(files.len() * 5);
        for (id, name, mime, modified, size) in files {
            flat.push(Value::Number(*id as f64));
            let n = self.rt.string(name);
            flat.push(self.keep(n));
            let m = self.rt.string(mime);
            flat.push(self.keep(m));
            flat.push(Value::Number(*modified));
            flat.push(Value::Number(*size));
        }
        let list = self.rt.array(&flat);
        self.keep(list);
        self.call_export(ex.files, &[node, list], "file selection");
        self.release();
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

    /// True if a script has work for the next frame.
    pub fn frame_wanted(&self) -> bool {
        let st = self.cx.st.borrow();
        st.frame_wanted || st.observers_due
    }

    /// True if scripts watch element sizes or visibility (they are told after layout changes).
    pub fn observing(&self) -> bool {
        self.cx.st.borrow().observing
    }

    /// A frame is about to be drawn: run what scripts queued for it (animation-frame
    /// callbacks, then the size and visibility observers).
    pub fn run_frame(&mut self, now: f64) {
        let Some(ex) = self.exports else { return };
        {
            let mut st = self.cx.st.borrow_mut();
            st.set_now(now);
            st.frame_wanted = false;
            st.observers_due = false;
        }
        self.call_export(ex.frame, &[Value::Number(now.floor())], "animation frame");
    }

    /// A CSS transition or animation started or ended on `node`. `name` is the property or
    /// the `@keyframes` name; `elapsed` is in seconds.
    pub fn animation_event(&mut self, node: NodeId, kind: &str, name: &str, elapsed: f64) {
        let Some(ex) = self.exports else { return };
        if !self.cx.st.borrow().page.document().is_valid(node) {
            return;
        }
        let t = wrap(&mut self.rt, &self.cx, node);
        let k = self.rt.string(kind);
        self.keep(k);
        let n = self.rt.string(name);
        self.keep(n);
        self.call_export(ex.anim, &[t, k, n, Value::Number(elapsed)], "animation event");
        self.release();
    }

    /// A message from another document arrived: its data as JSON text, the sender's origin
    /// and the frame element it came through (`NO_NODE` = from the document this one is
    /// framed in).
    pub fn frame_message(&mut self, json: &str, origin: &str, through: NodeId) {
        let Some(ex) = self.exports else { return };
        let j = self.rt.string(json);
        self.keep(j);
        let o = self.rt.string(origin);
        self.keep(o);
        let n = wrap(&mut self.rt, &self.cx, through);
        self.call_export(ex.message, &[j, o, n], "message listener");
        self.release();
    }

    /// Something happened on one of the page's WebSockets.
    pub fn socket_event(&mut self, id: u32, event: &crate::page::SocketEvent) {
        use crate::page::SocketEvent as E;
        let Some(ex) = self.exports else { return };
        let mut args = vec![Value::Number(id as f64)];
        match event {
            E::Open { protocol } => {
                let p = self.rt.string(protocol);
                args.extend([Value::Number(0.0), self.keep(p)]);
            }
            E::Text(text) => {
                let t = self.rt.string(text);
                args.extend([Value::Number(1.0), self.keep(t)]);
            }
            E::Binary(data) => {
                let b = self.rt.array_buffer(data);
                args.extend([Value::Number(2.0), self.keep(b)]);
            }
            E::Closed { code, reason, clean } => {
                let r = self.rt.string(reason);
                args.extend([Value::Number(3.0), self.keep(r), Value::Number(*code as f64), Value::Bool(*clean)]);
            }
        }
        self.call_export(ex.socket, &args, "socket listener");
        self.release();
    }

    /// Script animations (`Element.animate`) that reached their end.
    pub fn animations_finished(&mut self, ids: &[u32]) {
        let Some(ex) = self.exports else { return };
        let vals: Vec<Value> = ids.iter().map(|&i| Value::Number(i as f64)).collect();
        let list = self.rt.array(&vals);
        self.keep(list);
        self.call_export(ex.anim_done, &[list], "animation finish");
        self.release();
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
            st.set_now(now);
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

    /// Where script time went since the last call: (function, milliseconds), most expensive
    /// first. Empty unless profiling is on (`INVIS_PROFILE`).
    pub fn take_profile(&mut self) -> Vec<(String, f64)> {
        self.rt.take_profile().into_iter().map(|r| (r.what, r.ms)).collect()
    }
}
