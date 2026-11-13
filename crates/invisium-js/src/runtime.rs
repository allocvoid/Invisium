//! The `Runtime` struct, public embedding types and construction.

use crate::bytecode::FuncCode;
use crate::value::*;
use crate::vm::{Frame, Handler};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_heap_bytes: usize,
    pub max_call_depth: u32,
    pub step_budget: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_heap_bytes: 64 << 20, max_call_depth: 400, step_budget: 50_000_000 }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorKind {
    Syntax,
    Thrown,
    StepBudget,
    OutOfMemory,
    StackOverflow,
}

#[derive(Clone, Debug)]
pub struct JsError {
    pub kind: ErrorKind,
    pub name: String,
    pub message: String,
    pub line: u32,
    pub column: u32,
    pub stack: String,
}

impl std::fmt::Display for JsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {} ({}:{})", self.name, self.message, self.line, self.column)
    }
}

impl std::error::Error for JsError {}

/// What native functions return on failure: the JS value to throw.
#[derive(Clone, Copy, Debug)]
pub struct Thrown(pub Value);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ClassId(pub(crate) u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RootId(pub(crate) u32);

#[derive(Clone, Copy)]
pub struct HostClass {
    pub name: &'static str,
    /// Property read. `None` = fall through to the prototype chain / undefined.
    pub get: fn(&mut Runtime, id: u64, key: &str) -> Result<Option<Value>, Thrown>,
    /// Property write. `Ok(false)` = not handled, store it as an ordinary own property.
    pub set: fn(&mut Runtime, id: u64, key: &str, value: Value) -> Result<bool, Thrown>,
}

/// Internal exception channel: a JS throw, or an uncatchable abort.
#[derive(Clone, Copy, Debug)]
pub enum Exc {
    Throw(Value),
    Abort(ErrorKind),
}

pub type JsResult<T> = Result<T, Exc>;

macro_rules! predef_atoms {
    ($($name:ident = $s:expr),* $(,)?) => {
        pub const PREDEF: &[&str] = &[$($s),*];
        #[allow(non_camel_case_types, clippy::upper_case_acronyms, dead_code)]
        #[repr(u32)]
        enum PredefIdx { $($name),* }
        $(pub const $name: Atom = Atom(PredefIdx::$name as u32);)*
    };
}

predef_atoms! {
    A_EMPTY = "", A_LENGTH = "length", A_PROTOTYPE = "prototype", A_CONSTRUCTOR = "constructor",
    A_NAME = "name", A_MESSAGE = "message", A_STACK = "stack", A_NEXT = "next", A_DONE = "done",
    A_VALUE = "value", A_TO_STRING = "toString", A_VALUE_OF = "valueOf", A_GET = "get", A_SET = "set",
    A_WRITABLE = "writable", A_ENUMERABLE = "enumerable", A_CONFIGURABLE = "configurable",
    A_RETURN = "return", A_CALLEE = "callee", A_RAW = "raw", A_CAUSE = "cause",
    A_TO_JSON = "toJSON", A_UNDEFINED = "undefined", A_NULL = "null", A_TRUE = "true", A_FALSE = "false",
    A_SOURCE = "source", A_FLAGS = "flags", A_LAST_INDEX = "lastIndex", A_THEN = "then",
    A_HAS = "has", A_DELETE_PROPERTY = "deleteProperty", A_OWN_KEYS = "ownKeys",
    A_GET_OWN_PROPERTY_DESCRIPTOR = "getOwnPropertyDescriptor", A_DEFINE_PROPERTY = "defineProperty",
    A_APPLY = "apply", A_CONSTRUCT = "construct",
    A_INDEX = "index", A_INPUT = "input", A_GROUPS = "groups", A_INDICES = "indices", A_EXEC = "exec",
    A_STACK_TRACE_LIMIT = "stackTraceLimit",
}

/// Most frames a `stack` string lists.
pub const MAX_STACK_FRAMES: usize = 10;

pub const SYM_ITERATOR: SymId = SymId(0);
pub const SYM_HAS_INSTANCE: SymId = SymId(1);
pub const SYM_TO_PRIMITIVE: SymId = SymId(2);
pub const SYM_TO_STRING_TAG: SymId = SymId(3);
pub const SYM_ASYNC_ITERATOR: SymId = SymId(4);
pub const SYM_UNSCOPABLES: SymId = SymId(5);
pub const SYM_MATCH: SymId = SymId(6);
pub const SYM_MATCH_ALL: SymId = SymId(7);
pub const SYM_REPLACE: SymId = SymId(8);
pub const SYM_SEARCH: SymId = SymId(9);
pub const SYM_SPLIT: SymId = SymId(10);
pub const SYM_SPECIES: SymId = SymId(11);
/// Descriptions of the well-known symbols, indexed by `SymId`.
pub const WELL_KNOWN_SYMBOLS: [&str; 12] = [
    "iterator", "hasInstance", "toPrimitive", "toStringTag", "asyncIterator", "unscopables", "match", "matchAll", "replace", "search", "split",
    "species",
];

#[derive(Default, Clone, Copy)]
pub struct Intrinsics {
    pub object_proto: Option<ObjId>,
    pub function_proto: Option<ObjId>,
    pub array_proto: Option<ObjId>,
    pub string_proto: Option<ObjId>,
    pub number_proto: Option<ObjId>,
    pub boolean_proto: Option<ObjId>,
    pub symbol_proto: Option<ObjId>,
    pub error_proto: Option<ObjId>,
    pub type_error_proto: Option<ObjId>,
    pub range_error_proto: Option<ObjId>,
    pub syntax_error_proto: Option<ObjId>,
    pub reference_error_proto: Option<ObjId>,
    pub eval_error_proto: Option<ObjId>,
    pub uri_error_proto: Option<ObjId>,
    pub iterator_proto: Option<ObjId>,
    pub array_iter_proto: Option<ObjId>,
    pub string_iter_proto: Option<ObjId>,
    pub regexp_proto: Option<ObjId>,
    pub array_values: Option<ObjId>,
    pub promise_proto: Option<ObjId>,
    pub aggregate_error_proto: Option<ObjId>,
    pub generator_proto: Option<ObjId>,
    pub async_generator_proto: Option<ObjId>,
    pub async_from_sync_proto: Option<ObjId>,
    /// Sentinel thrown through a generator's `finally` blocks by `return()`.
    pub gen_return_marker: Option<ObjId>,
    pub map_proto: Option<ObjId>,
    pub set_proto: Option<ObjId>,
    pub map_iter_proto: Option<ObjId>,
    pub set_iter_proto: Option<ObjId>,
    pub weakmap_proto: Option<ObjId>,
    pub weakset_proto: Option<ObjId>,
    pub regexp_ctor: Option<ObjId>,
    /// The built-in `RegExp.prototype.exec`, recognised so unmodified regexes take fast paths.
    pub regexp_exec: Option<ObjId>,
    pub regexp_iter_proto: Option<ObjId>,
    pub date_proto: Option<ObjId>,
    pub array_buffer_proto: Option<ObjId>,
    pub data_view_proto: Option<ObjId>,
    /// One prototype per typed-array element kind, in `TypedKind` order.
    pub typed_protos: [Option<ObjId>; 11],
    /// The `Error` constructor (holds `stackTraceLimit`).
    pub error_ctor: Option<ObjId>,
    /// The built-in `eval`: a call written `eval(...)` that reaches it is a direct eval.
    pub eval_fn: Option<ObjId>,
    /// The shared `get [Symbol.species]` accessor and the constructors it is compared against.
    pub species_getter: Option<ObjId>,
    pub array_ctor: Option<ObjId>,
    pub promise_ctor: Option<ObjId>,
    pub typed_ctors: [Option<ObjId>; 11],
    pub bigint_proto: Option<ObjId>,
    /// The script helper behind `Array.fromAsync`, compiled on first use.
    pub from_async: Option<ObjId>,
    pub weakref_proto: Option<ObjId>,
    pub finreg_proto: Option<ObjId>,
    /// Prototypes of the `Intl` constructors (see `builtins::intl::P_*`).
    pub intl_protos: [Option<ObjId>; 10],
}

pub struct Runtime {
    pub(crate) limits: Limits,
    pub(crate) objs: Vec<Option<Object>>,
    pub(crate) free_objs: Vec<u32>,
    pub(crate) strs: Vec<Option<Rc<[u16]>>>,
    pub(crate) free_strs: Vec<u32>,
    pub(crate) envs: Vec<Option<Env>>,
    pub(crate) free_envs: Vec<u32>,
    pub(crate) bigs: Vec<Option<Rc<crate::bigint::BigInt>>>,
    pub(crate) free_bigs: Vec<u32>,
    /// The value of each BigInt literal evaluated so far, by its source text.
    pub(crate) big_literals: HashMap<Atom, BigId>,
    pub(crate) atoms: Vec<Rc<[u16]>>,
    pub(crate) atom_map: HashMap<Rc<[u16]>, u32>,
    /// Array-index value of each atom, or u32::MAX.
    pub(crate) atom_index: Vec<u32>,
    pub(crate) syms: Vec<Option<Rc<[u16]>>>,
    pub(crate) sym_registry: HashMap<Rc<[u16]>, SymId>,
    pub(crate) heap_bytes: usize,
    pub(crate) permanent_bytes: usize,
    pub(crate) gc_threshold: usize,
    pub(crate) gc_requested: bool,
    /// GC stress: collect at every `gc_stress`-th collection point (0 = off).
    pub(crate) gc_stress: u32,
    pub(crate) gc_stress_tick: u32,
    pub(crate) gc_count: u64,

    pub(crate) roots: HashMap<u32, Value>,
    pub(crate) next_root: u32,
    pub(crate) scratch: Vec<Value>,
    pub(crate) native_active: bool,
    /// Registered host classes; `None` = plain class (no get/set hooks).
    pub(crate) host_classes: Vec<(&'static str, Option<HostClass>)>,
    pub(crate) host_objs: HashMap<(u32, u64), ObjId>,
    pub(crate) template_cache: HashMap<(usize, u32), (Arc<FuncCode>, ObjId)>,
    pub(crate) global_lex: HashMap<Atom, GlobalLex>,

    pub(crate) stack: Vec<Value>,
    pub(crate) locals: Vec<Option<Value>>,
    pub(crate) frames: Vec<Frame>,
    pub(crate) handlers: Vec<Handler>,
    pub(crate) steps: u64,
    pub(crate) entry_depth: u32,
    pub(crate) native_depth: u32,
    pub(crate) stack_base: usize,
    /// Native stack script may use below `stack_base` (see `set_native_stack_budget`).
    pub(crate) native_stack_budget: usize,
    pub(crate) pending_abort: Option<ErrorKind>,
    /// `new.target` visible to the currently running native constructor.
    pub(crate) native_new_target: Value,
    pub(crate) stack_overflow_error: Option<ObjId>,
    pub(crate) throw_pos: (u32, u32),
    /// Frames that were active when an uncatchable abort (step budget, heap cap) began.
    pub(crate) abort_stack: String,
    /// Private names minted so far (see `class_private.rs`).
    pub(crate) next_private: u32,
    /// Steps owed for BigInt work done where the step counter cannot be reached (`&self`
    /// comparisons and hashing); the next safepoint adds them to `steps`.
    pub(crate) pending_steps: std::cell::Cell<u64>,
    pub(crate) fn_has_instance: Option<ObjId>,
    pub(crate) join_stack: Vec<ObjId>,

    pub(crate) global: ObjId,
    pub(crate) intr: Intrinsics,
    pub(crate) clock: Box<dyn FnMut() -> f64>,
    pub(crate) random: Box<dyn FnMut() -> f64>,
    pub(crate) host_data: Option<Rc<dyn std::any::Any>>,

    /// Promise job (microtask) queue, drained by `run_jobs`.
    pub(crate) jobs: std::collections::VecDeque<Job>,
    /// Promises rejected while they had no handler; checked when the job queue drains.
    pub(crate) pending_rejections: Vec<ObjId>,
    pub(crate) unhandled: Vec<String>,
    /// Compiled patterns keyed by (source, flag bits): a literal in a loop compiles once.
    pub(crate) regex_cache: HashMap<(Rc<[u16]>, u8), Rc<RegexProg>>,
    /// ES module records, the resolver and pending dynamic imports.
    pub(crate) modules: crate::module::Modules,
}

pub const NATIVE_STACK_BUDGET: usize = 640 * 1024;

fn default_clock() -> Box<dyn FnMut() -> f64> {
    Box::new(|| {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0);
        (ms / 100.0).floor() * 100.0
    })
}

fn default_random() -> Box<dyn FnMut() -> f64> {
    // xorshift64*, seeded from the clock and a heap address
    let seed_src = Box::new(0u8);
    let addr = &*seed_src as *const u8 as usize as u64;
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
    let mut state = (t ^ addr.rotate_left(17)) | 1;
    Box::new(move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let r = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (r >> 11) as f64 / (1u64 << 53) as f64
    })
}

impl Runtime {
    pub fn new(limits: Limits) -> Runtime {
        let mut rt = Runtime {
            limits,
            objs: Vec::new(),
            free_objs: Vec::new(),
            strs: Vec::new(),
            free_strs: Vec::new(),
            envs: Vec::new(),
            free_envs: Vec::new(),
            bigs: Vec::new(),
            free_bigs: Vec::new(),
            big_literals: HashMap::new(),
            atoms: Vec::new(),
            atom_map: HashMap::new(),
            atom_index: Vec::new(),
            syms: Vec::new(),
            sym_registry: HashMap::new(),
            heap_bytes: 0,
            permanent_bytes: 0,
            gc_threshold: 4 << 20,
            gc_requested: false,
            gc_stress: 0,
            gc_stress_tick: 0,
            gc_count: 0,
            roots: HashMap::new(),
            next_root: 1,
            scratch: Vec::new(),
            native_active: false,
            host_classes: Vec::new(),
            host_objs: HashMap::new(),
            template_cache: HashMap::new(),
            global_lex: HashMap::new(),
            stack: Vec::new(),
            locals: Vec::new(),
            frames: Vec::new(),
            handlers: Vec::new(),
            steps: 0,
            entry_depth: 0,
            native_depth: 0,
            stack_base: 0,
            native_stack_budget: NATIVE_STACK_BUDGET,
            pending_abort: None,
            native_new_target: Value::Undefined,
            stack_overflow_error: None,
            throw_pos: (0, 0),
            abort_stack: String::new(),
            next_private: 0,
            pending_steps: std::cell::Cell::new(0),
            fn_has_instance: None,
            join_stack: Vec::new(),
            global: ObjId(0),
            intr: Intrinsics::default(),
            clock: default_clock(),
            random: default_random(),
            host_data: None,
            jobs: std::collections::VecDeque::new(),
            pending_rejections: Vec::new(),
            unhandled: Vec::new(),
            regex_cache: HashMap::new(),
            modules: crate::module::Modules::default(),
        };
        // slot 0 is a permanent dummy object returned for stale handles
        rt.objs.push(Some(Object::new(None, ObjKind::Ordinary)));
        for s in PREDEF {
            rt.intern(s);
        }
        for d in WELL_KNOWN_SYMBOLS {
            let u: Rc<[u16]> = format!("Symbol.{}", d).encode_utf16().collect::<Vec<u16>>().into();
            rt.syms.push(Some(u));
        }
        crate::builtins::setup(&mut rt);
        rt.heap_bytes = rt.measure_heap();
        rt.gc_threshold = (rt.heap_bytes * 2).max(4 << 20);
        rt
    }
}
