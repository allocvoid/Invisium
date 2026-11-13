//! Invisium JavaScript engine. Owned by the JS track — see `docs/agents/JS.md` for the contract.
//!
//! Architecture: source is parsed and compiled to bytecode on a short-lived helper thread with a
//! large stack (so deeply nested input cannot exhaust the caller's stack); bytecode runs on a
//! register/stack VM whose JS-to-JS calls do not recurse natively. Objects, strings and
//! environments live in index-addressed arenas collected by mark-and-sweep.
#![forbid(unsafe_code)]

mod analyze;
mod ast;
mod bigint;
mod bigint_bits;
mod builtins;
mod bytecode;
mod class_private;
mod compile_eval;
mod compile_expr;
mod compile_module;
mod compile_pattern;
mod compile_stmt;
mod compiler;
mod coroutine;
mod embed;
mod heap;
mod ident_table;
mod interp;
mod interp_slow;
mod iter;
mod lexer;
mod module;
mod module_eval;
mod module_run;
mod numfmt;
mod ops;
mod parser;
mod parser_expr;
mod parser_func;
mod parser_module;
mod parser_primary;
mod parser_stmt;
mod profile;
mod promise;
mod props;
mod proxy;
mod runtime;
mod scope;
mod value;
mod vm;

pub use profile::ProfileRow;
pub use runtime::{ClassId, ErrorKind, HostClass, JsError, Limits, RootId, Runtime, Thrown};
pub use value::{BigId, NativeFn, ObjId, StrId, SymId, Value};

use runtime::{Exc, A_MESSAGE, A_NAME, A_STACK};
use std::rc::Rc;
use std::sync::Arc;
use value::{Atom, ObjKind, Object, PropKey};

const COMPILE_STACK: usize = 256 << 20;

/// An immutable compiled program: compile it once with [`Runtime::compile`] and run it in any
/// number of runtimes with [`Runtime::run`]. Cloning shares the program.
#[derive(Clone)]
pub struct Script(Arc<bytecode::CompiledScript>);

const _: fn() = || {
    fn shared<T: Send + Sync>() {}
    shared::<Script>();
};

pub(crate) fn syntax_error(e: parser::ParseError, filename: &str) -> JsError {
    JsError {
        kind: ErrorKind::Syntax,
        name: "SyntaxError".into(),
        stack: format!("SyntaxError: {}\n    at <anonymous> ({}:{}:{})", e.message, filename, e.line, e.column),
        message: e.message,
        line: e.line,
        column: e.column,
    }
}

type Compiled = Result<bytecode::CompiledScript, parser::ParseError>;

/// `dynamic`: the source comes from a running script (indirect `eval`, `Function`).
fn compile_source(src: &str, filename: &str, dynamic: bool) -> Compiled {
    let chars: Arc<[char]> = src.chars().collect::<Vec<char>>().into();
    let filename = filename.to_string();
    on_compile_thread(move || {
        let prog = parser::parse_program(&chars)?;
        compiler::compile_program(&prog, chars.clone(), &filename, dynamic)
    })
}

/// Compiles direct-eval code against the scope chain of its call site.
pub(crate) fn compile_eval_source(src: &str, scope: Arc<bytecode::EvalScope>) -> Compiled {
    let chars: Arc<[char]> = src.chars().collect::<Vec<char>>().into();
    on_compile_thread(move || {
        let opts = parser::ParseOpts { strict: scope.strict, new_target: scope.new_target, super_prop: scope.super_prop, super_call: scope.super_call };
        let prog = parser::parse_eval(&chars, opts)?;
        compile_eval::compile_eval(&prog, chars.clone(), "<eval>", &scope)
    })
}

/// Parses and compiles module code.
pub(crate) fn compile_module_source(src: &str, url: &str) -> Result<compile_module::ModuleCode, parser::ParseError> {
    let chars: Arc<[char]> = src.chars().collect::<Vec<char>>().into();
    let url = url.to_string();
    on_compile_thread(move || {
        let (prog, info) = parser::parse_module(&chars)?;
        compile_module::compile_module(&prog, info, chars.clone(), &url)
    })
}

fn on_compile_thread<T: Send + 'static>(job: impl FnOnce() -> Result<T, parser::ParseError> + Send + 'static) -> Result<T, parser::ParseError> {
    let internal = || parser::ParseError { message: "internal compiler error".into(), line: 0, column: 0 };
    match std::thread::Builder::new().name("invisium-js-compile".into()).stack_size(COMPILE_STACK).spawn(job) {
        Ok(h) => h.join().unwrap_or_else(|_| Err(internal())),
        Err(_) => Err(internal()),
    }
}

impl Runtime {
    pub(crate) fn host_enter(&mut self) {
        if self.entry_depth == 0 {
            self.steps = 0;
            self.pending_steps.set(0);
            self.abort_stack.clear();
            self.stack_base = vm::stack_addr();
            let over = self.total_heap() > self.limits.max_heap_bytes;
            if self.gc_due() || over {
                // e.g. after an out-of-memory abort: reclaim before running anything new
                self.collect_garbage();
            }
        }
        if self.entry_depth == 0 {
            self.profile_enter();
        }
        self.entry_depth += 1;
    }

    /// `host_enter` for an API call whose inputs are `vals`: they are rooted before the
    /// collection `host_enter` may run and stay rooted for the whole call (an embedder's
    /// arguments often exist only in its Rust locals). Returns the mark for `finish_with`.
    pub(crate) fn host_enter_with(&mut self, vals: &[Value], more: &[Value]) -> usize {
        let mark = self.scratch.len();
        self.scratch.extend_from_slice(vals);
        self.scratch.extend_from_slice(more);
        self.host_enter();
        mark
    }

    /// `finish` for an entry made with `host_enter_with`: also unroots the inputs.
    pub(crate) fn finish_with<T>(&mut self, r: Result<T, Exc>, mark: usize) -> Result<T, JsError> {
        let r = self.finish(r);
        self.scratch.truncate(mark);
        r
    }

    /// Inside a native, a value handed back to it from script stays rooted until the native
    /// returns: natives keep such values in Rust locals while they call back into script.
    #[inline]
    pub(crate) fn keep_for_native(&mut self, v: Value) {
        if self.native_active && matches!(v, Value::Object(_) | Value::String(_) | Value::BigInt(_)) {
            self.scratch.push(v);
        }
    }

    fn host_exit(&mut self) {
        self.entry_depth = self.entry_depth.saturating_sub(1);
        if self.entry_depth == 0 {
            self.scratch.clear();
            self.pending_abort = None;
        }
    }

    fn to_js_error(&mut self, e: Exc) -> JsError {
        let (line, column) = self.throw_pos;
        match e {
            Exc::Abort(kind) => {
                let message = match kind {
                    ErrorKind::StepBudget => "script step budget exhausted",
                    ErrorKind::OutOfMemory => "out of memory: heap limit exceeded",
                    _ => "aborted",
                };
                let stack = format!("InternalError: {}{}", message, std::mem::take(&mut self.abort_stack));
                JsError { kind, name: "InternalError".into(), message: message.into(), line, column, stack }
            }
            Exc::Throw(v) => {
                let kind = match (v, self.stack_overflow_error) {
                    (Value::Object(o), Some(so)) if o == so => ErrorKind::StackOverflow,
                    _ => ErrorKind::Thrown,
                };
                let (name, message, stack) = match v {
                    Value::Object(_) => {
                        let field = |rt: &mut Runtime, a: Atom| -> String {
                            match rt.get_prop(v, PropKey::Str(a)) {
                                Ok(Value::String(s)) => rt.rust_string(s),
                                Ok(Value::Undefined) | Err(_) => String::new(),
                                Ok(other) => rt.to_string_lossy(other),
                            }
                        };
                        (field(self, A_NAME), field(self, A_MESSAGE), field(self, A_STACK))
                    }
                    other => (String::new(), self.to_string_lossy(other), String::new()),
                };
                JsError { kind, name, message, line, column, stack }
            }
        }
    }

    pub(crate) fn finish<T>(&mut self, r: Result<T, Exc>) -> Result<T, JsError> {
        let r = r.map_err(|e| self.to_js_error(e));
        self.host_exit();
        if self.entry_depth == 0 && matches!(&r, Err(e) if e.kind == ErrorKind::OutOfMemory) {
            // the aborted script's garbage is unreachable now; reclaim it for the next entry
            // (its queued jobs go with it: they may be what filled the heap)
            self.jobs.clear();
            self.pending_rejections.clear();
            self.collect_garbage();
        }
        r
    }

    /// Compiles and runs a script in the global scope; returns its completion value.
    pub fn eval(&mut self, source: &str, filename: &str) -> Result<Value, JsError> {
        let script = compile_source(source, filename, false).map_err(|e| syntax_error(e, filename))?;
        self.host_enter();
        let r = self.run_script(&script);
        self.finish(r)
    }

    /// Compiles a script without running it. The result is independent of any runtime.
    pub fn compile(source: &str, filename: &str) -> Result<Script, JsError> {
        compile_source(source, filename, false).map(|s| Script(Arc::new(s))).map_err(|e| syntax_error(e, filename))
    }

    /// Runs a compiled script in this runtime's global scope; returns its completion value.
    pub fn run(&mut self, script: &Script) -> Result<Value, JsError> {
        self.host_enter();
        let r = self.run_script(&script.0);
        self.finish(r)
    }

    fn run_script(&mut self, script: &bytecode::CompiledScript) -> Result<Value, Exc> {
        let atoms: Rc<[Atom]> = script.names.iter().map(|n| self.intern16(n)).collect::<Vec<Atom>>().into();
        let fp = self.intr.function_proto;
        let c = value::Closure {
            code: script.top.clone(),
            atoms,
            env: None,
            this_val: Value::Undefined,
            new_target: Value::Undefined,
            home: None,
            ctor_callee: None,
            fields: None,
        };
        let f = self.alloc(Object::new(fp, ObjKind::Function(Box::new(value::FuncData::Closure(c)))));
        self.scratch.push(Value::Object(f));
        let g = Value::Object(self.global);
        let sp = self.stack.len();
        self.push_frame(f, g, Value::Undefined, sp, sp, false)?;
        let entry = self.frames.len() - 1;
        self.run_frames(entry)
    }

    /// Runs `source` as a global script from inside the engine: the `Function` constructor
    /// and indirect `eval`.
    pub(crate) fn eval_internal(&mut self, source: &str, filename: &str) -> Result<Value, Exc> {
        self.charge_compile(source.len())?;
        match compile_source(source, filename, true) {
            Ok(s) => {
                // the retained source text is heap the script is responsible for
                self.grow(s.top.source.len() * 4);
                self.run_script(&s)
            }
            Err(e) => Err(Exc::Throw(self.make_error("SyntaxError", &e.message))),
        }
    }

    /// Compiling from script (`eval`, `Function`) costs far more than a bytecode step: charge
    /// it to the step budget so a loop of compilations ends in bounded time.
    pub(crate) fn charge_compile(&mut self, source_len: usize) -> Result<(), Exc> {
        self.steps = self.steps.saturating_add(2000 + source_len as u64 / 4);
        if self.steps > self.limits.step_budget {
            return Err(Exc::Abort(ErrorKind::StepBudget));
        }
        Ok(())
    }

    pub fn call(&mut self, func: Value, this: Value, args: &[Value]) -> Result<Value, JsError> {
        let mark = self.host_enter_with(&[func, this], args);
        let r = self.call_value(func, this, args);
        let r = self.finish_with(r, mark);
        if let Ok(v) = r {
            self.keep_for_native(v);
        }
        r
    }

    pub fn global(&self) -> Value {
        Value::Object(self.global)
    }

    pub fn string(&mut self, s: &str) -> Value {
        self.new_str(s)
    }

    pub fn object(&mut self) -> Value {
        Value::Object(self.plain_object())
    }

    pub fn array(&mut self, items: &[Value]) -> Value {
        self.new_array_values(items)
    }

    pub fn function(&mut self, name: &str, arity: u32, f: NativeFn) -> Value {
        Value::Object(self.new_native(name, arity, f, false))
    }

    pub fn get(&mut self, obj: Value, key: &str) -> Result<Value, JsError> {
        let mark = self.host_enter_with(&[obj], &[]);
        let k = self.key_str(key);
        let r = self.get_prop(obj, k);
        let r = self.finish_with(r, mark);
        if let Ok(v) = r {
            self.keep_for_native(v);
        }
        r
    }

    pub fn set(&mut self, obj: Value, key: &str, value: Value) -> Result<(), JsError> {
        let mark = self.host_enter_with(&[obj, value], &[]);
        let k = self.key_str(key);
        let r = self.set_prop(obj, k, value, obj).map(|_| ());
        self.finish_with(r, mark)
    }

    pub fn get_index(&mut self, obj: Value, index: u32) -> Result<Value, JsError> {
        let mark = self.host_enter_with(&[obj], &[]);
        let r = props_get_index(self, obj, index);
        let r = self.finish_with(r, mark);
        if let Ok(v) = r {
            self.keep_for_native(v);
        }
        r
    }

    /// ToString; "" if it throws.
    pub fn to_string_lossy(&mut self, v: Value) -> String {
        let mark = self.host_enter_with(&[v], &[]);
        let r = self.to_str(v).map(|s| self.rust_string(s));
        self.host_exit();
        self.scratch.truncate(mark);
        r.unwrap_or_default()
    }

    pub fn to_number(&mut self, v: Value) -> f64 {
        let mark = self.host_enter_with(&[v], &[]);
        let r = ops_to_number(self, v);
        self.host_exit();
        self.scratch.truncate(mark);
        r.unwrap_or(f64::NAN)
    }

    pub fn to_boolean(&self, v: Value) -> bool {
        self.truthy(v)
    }

    pub fn is_callable(&self, v: Value) -> bool {
        self.is_callable_val(v)
    }

    pub fn type_of(&self, v: Value) -> &'static str {
        self.typeof_str(v)
    }

    pub fn register_host_class(&mut self, class: HostClass) -> ClassId {
        self.host_classes.push((class.name, Some(class)));
        ClassId(self.host_classes.len() as u32 - 1)
    }

    /// The same (class, id) always yields the same object.
    pub fn host_object(&mut self, class: ClassId, id: u64) -> Value {
        if let Some(o) = self.host_objs.get(&(class.0, id)) {
            return Value::Object(*o);
        }
        let p = self.intr.object_proto;
        let o = self.alloc(Object::new(p, ObjKind::Host(class.0, id)));
        self.host_objs.insert((class.0, id), o);
        Value::Object(o)
    }

    pub fn host_id(&self, v: Value) -> Option<(ClassId, u64)> {
        match v {
            Value::Object(o) => match self.obj(o).kind {
                ObjKind::Host(c, id) => Some((ClassId(c), id)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Keeps `v` alive across collections until `unroot`.
    ///
    /// The embedder's rule. The runtime collects only inside calls that can run script:
    /// `eval`, `run`, `call`, `construct`, `get`, `set`, `get_index`, `define_property`,
    /// `define_accessor`, `to_string_lossy`, `to_number`, `run_jobs`, `run_module`,
    /// `finish_import` and `gc`. Calls that only make or inspect values (`string`, `object`,
    /// `array`, `function`, `new_promise`, `host_object`, `host_id`, `type_of`, `to_boolean`,
    /// `is_callable`, `root`) never collect. So:
    /// - A value made with `string` / `object` / `array` / `function` (or returned by any call)
    ///   may sit unrooted in a Rust local until the next collecting call. Values passed *as
    ///   arguments* to that call (the function, `this`, the arguments, the object and value of
    ///   `set`, ...) are rooted by the call itself for its whole duration, so making a value
    ///   and passing it straight in is safe.
    /// - A value kept *across* a collecting call it is not an argument of (across another
    ///   `call`, `set`, `run_jobs`, an `eval`, ...) must be rooted first; otherwise it may be
    ///   freed and its handle may later name an unrelated object.
    /// - Inside a native function (and a host-class hook called from one): `this`, the
    ///   arguments, everything the native allocates, and every value it gets back from
    ///   `call` / `construct` / `get` / `get_index` stay alive until the native returns. A
    ///   value the native returns is rooted by the engine from then on.
    /// - Values that must outlive the current entry (a timer callback, an event listener, a
    ///   promise to settle later) must be rooted, and unrooted when no longer needed. Host
    ///   objects made with `host_object` are always kept alive by the runtime.
    pub fn root(&mut self, v: Value) -> RootId {
        let id = self.next_root;
        self.next_root = self.next_root.wrapping_add(1);
        self.roots.insert(id, v);
        RootId(id)
    }

    pub fn unroot(&mut self, id: RootId) {
        self.roots.remove(&id.0);
    }

    pub fn gc(&mut self) {
        self.collect_garbage();
    }

    /// Collect garbage at every `every`-th point at which the engine may collect at all
    /// (0 = normal behaviour). For tests and for embedders hunting rooting mistakes.
    pub fn set_gc_stress(&mut self, every: u32) {
        self.gc_stress = every;
        self.gc_stress_tick = 0;
    }

    /// Number of collections so far.
    pub fn gc_count(&self) -> u64 {
        self.gc_count
    }

    pub fn heap_bytes(&self) -> usize {
        self.total_heap()
    }

    pub fn set_clock(&mut self, now_ms: Box<dyn FnMut() -> f64>) {
        self.clock = now_ms;
    }

    pub fn set_random(&mut self, next: Box<dyn FnMut() -> f64>) {
        self.random = next;
    }
}

fn props_get_index(rt: &mut Runtime, obj: Value, index: u32) -> Result<Value, Exc> {
    rt.get_idx(obj, index as u64)
}

fn ops_to_number(rt: &mut Runtime, v: Value) -> Result<f64, Exc> {
    // `Runtime::to_number` (public, lossy) shadows the internal conversion of the same name.
    match v {
        Value::Number(n) => Ok(n),
        _ => {
            let p = rt.to_primitive(v, ops::Hint::Number)?;
            match p {
                Value::Number(n) => Ok(n),
                Value::String(s) => Ok(rt.str_to_num(s)),
                Value::Bool(b) => Ok(b as u8 as f64),
                Value::Null => Ok(0.0),
                Value::Undefined => Ok(f64::NAN),
                _ => Err(rt.type_error("Cannot convert a Symbol value to a number")),
            }
        }
    }
}
