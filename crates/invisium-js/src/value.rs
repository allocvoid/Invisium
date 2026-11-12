//! Value representation and heap object layout.

use crate::bytecode::FuncCode;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// Handle to a string. Strings with the high bit set are interned atoms (never collected).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StrId(pub(crate) u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SymId(pub(crate) u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ObjId(pub(crate) u32);
/// Handle to a BigInt (an immutable arbitrary-precision integer owned by the runtime).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BigId(pub(crate) u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Atom(pub(crate) u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EnvId(pub(crate) u32);

pub const ATOM_BIT: u32 = 0x8000_0000;

impl StrId {
    pub(crate) fn from_atom(a: Atom) -> StrId {
        StrId(a.0 | ATOM_BIT)
    }
    pub(crate) fn as_atom(self) -> Option<Atom> {
        if self.0 & ATOM_BIT != 0 { Some(Atom(self.0 & !ATOM_BIT)) } else { None }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(StrId),
    Symbol(SymId),
    Object(ObjId),
    BigInt(BigId),
}

impl Value {
    pub fn is_nullish(self) -> bool {
        matches!(self, Value::Undefined | Value::Null)
    }
    pub fn as_obj(self) -> Option<ObjId> {
        if let Value::Object(o) = self { Some(o) } else { None }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PropKey {
    Str(Atom),
    Sym(SymId),
}

pub const WRITABLE: u8 = 1;
pub const ENUMERABLE: u8 = 2;
pub const CONFIGURABLE: u8 = 4;
pub const ACCESSOR: u8 = 8;
pub const DEFAULT_FLAGS: u8 = WRITABLE | ENUMERABLE | CONFIGURABLE;
pub const HIDDEN: u8 = WRITABLE | CONFIGURABLE;

/// A property slot. For accessors `value` holds the getter and `setter` the setter.
#[derive(Clone, Copy, Debug)]
pub struct Prop {
    pub value: Value,
    pub setter: Value,
    pub flags: u8,
}

impl Prop {
    pub fn data(value: Value, flags: u8) -> Prop {
        Prop { value, setter: Value::Undefined, flags }
    }
    pub fn is_accessor(&self) -> bool {
        self.flags & ACCESSOR != 0
    }
}

/// Insertion-ordered property map with a hash index once it grows.
#[derive(Default)]
pub struct PropMap {
    entries: Vec<(PropKey, Prop)>,
    index: Option<HashMap<PropKey, u32>>,
}

const INDEX_THRESHOLD: usize = 12;

impl PropMap {
    fn pos(&self, k: PropKey) -> Option<usize> {
        match &self.index {
            Some(ix) => ix.get(&k).map(|&i| i as usize),
            None => self.entries.iter().position(|(key, _)| *key == k),
        }
    }
    pub fn get(&self, k: PropKey) -> Option<&Prop> {
        self.pos(k).map(|i| &self.entries[i].1)
    }
    pub fn get_mut(&mut self, k: PropKey) -> Option<&mut Prop> {
        match self.pos(k) {
            Some(i) => Some(&mut self.entries[i].1),
            None => None,
        }
    }
    /// Inserts or replaces; returns true when a new entry was added.
    pub fn insert(&mut self, k: PropKey, p: Prop) -> bool {
        if let Some(i) = self.pos(k) {
            self.entries[i].1 = p;
            return false;
        }
        self.entries.push((k, p));
        if let Some(ix) = &mut self.index {
            ix.insert(k, self.entries.len() as u32 - 1);
        } else if self.entries.len() > INDEX_THRESHOLD {
            self.rebuild();
        }
        true
    }
    fn rebuild(&mut self) {
        let mut ix = HashMap::with_capacity(self.entries.len());
        for (i, (k, _)) in self.entries.iter().enumerate() {
            ix.insert(*k, i as u32);
        }
        self.index = Some(ix);
    }
    pub fn remove(&mut self, k: PropKey) -> bool {
        match self.pos(k) {
            Some(i) => {
                self.entries.remove(i);
                if self.index.is_some() {
                    self.rebuild();
                }
                true
            }
            None => false,
        }
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn iter(&self) -> impl Iterator<Item = &(PropKey, Prop)> {
        self.entries.iter()
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut (PropKey, Prop)> {
        self.entries.iter_mut()
    }
}

pub type NativeFn = Rc<dyn Fn(&mut crate::Runtime, Value, &[Value]) -> Result<Value, crate::Thrown>>;

pub struct Closure {
    pub code: Arc<FuncCode>,
    pub atoms: Rc<[Atom]>,
    pub env: Option<EnvId>,
    /// Arrow functions: captured `this` / `new.target`.
    pub this_val: Value,
    pub new_target: Value,
    pub home: Option<ObjId>,
    /// Arrow functions inside derived constructors: the constructor whose parent `super()` calls.
    pub ctor_callee: Option<ObjId>,
    /// Class constructors: the instance-field initialiser.
    pub fields: Option<ObjId>,
}

/// Engine-internal native: receives its captured values (`slots`), which the collector traces.
pub type InternFn = fn(&mut crate::Runtime, &[Value], Value, &[Value]) -> Result<Value, crate::runtime::Exc>;

pub enum FuncData {
    Closure(Closure),
    Native { f: NativeFn, ctor: bool },
    Bound { target: ObjId, this: Value, args: Vec<Value> },
    Intern { f: InternFn, slots: Vec<Value> },
    /// Generator / async-generator `next` (0, 3), `throw` (1, 4), `return` (2, 5): resumes a
    /// suspended frame on the VM's own frame stack instead of recursing natively.
    GenMethod(u8),
}

/// What happens when a promise settles.
#[derive(Clone, Copy)]
pub enum Reaction {
    /// `then` handlers; `derived` is the promise returned by `then` (None for internal uses).
    Then { on_ok: Value, on_err: Value, derived: Option<ObjId> },
    /// `then` handlers whose derived promise came from a foreign constructor: `cap` is the
    /// internal `[resolve, reject]` record of its PromiseCapability.
    ThenCap { on_ok: Value, on_err: Value, cap: ObjId },
    /// Resume the coroutine (async function / async generator) suspended at an `await`.
    Await { coro: ObjId },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GenStatus { Start, Yielded, Awaiting, Running, Done }

/// A frame taken off the VM stacks while its generator / async function is suspended.
pub struct SavedFrame {
    pub frame: crate::vm::Frame,
    pub locals: Vec<Option<Value>>,
    /// Arguments followed by the operand stack.
    pub stack: Vec<Value>,
    /// Handlers with `sp` relative to the frame's `args_base`.
    pub handlers: Vec<crate::vm::Handler>,
}

/// A queued `next` / `throw` / `return` call on an async generator.
pub struct GenRequest {
    pub mode: u8,
    pub value: Value,
    pub promise: ObjId,
}

pub struct GenState {
    pub status: GenStatus,
    /// 0 = generator, 1 = async function, 2 = async generator.
    pub kind: u8,
    pub frame: Option<Box<SavedFrame>>,
    /// Suspended inside `yield*`: a resume delivers (value, mode) instead of acting on the mode.
    pub delegating: bool,
    pub pending_return: Value,
    /// Async functions: the promise the call returned.
    pub promise: Option<ObjId>,
    pub queue: std::collections::VecDeque<GenRequest>,
}

impl GenState {
    fn trace(&self, f: &mut dyn FnMut(Value), fe: &mut dyn FnMut(EnvId)) {
        f(self.pending_return);
        if let Some(p) = self.promise {
            f(Value::Object(p));
        }
        for r in &self.queue {
            f(r.value);
            f(Value::Object(r.promise));
        }
        if let Some(s) = &self.frame {
            s.frame.trace(f, fe);
            s.locals.iter().flatten().for_each(|v| f(*v));
            s.stack.iter().for_each(|v| f(*v));
            s.handlers.iter().filter_map(|h| h.env).for_each(fe);
        }
    }
}

pub struct PromiseData {
    /// 0 = pending, 1 = fulfilled, 2 = rejected.
    pub state: u8,
    pub value: Value,
    pub reactions: Vec<Reaction>,
    pub handled: bool,
}

/// An entry of the promise job (microtask) queue.
#[derive(Clone, Copy)]
pub enum Job {
    Reaction { reaction: Reaction, rejected: bool, value: Value },
    Thenable { promise: ObjId, thenable: Value, then: Value },
    Call(Value),
    /// A FinalizationRegistry callback for a collected target: `callback(held)`.
    Cleanup { callback: Value, held: Value },
}

/// One `FinalizationRegistry.prototype.register` entry; `target` and `token` are weak.
pub struct FinCell {
    pub target: ObjId,
    pub held: Value,
    pub token: Option<ObjId>,
}

pub struct FinRegData {
    pub callback: Value,
    pub cells: Vec<FinCell>,
}

impl Reaction {
    pub fn trace(&self, f: &mut dyn FnMut(Value)) {
        match self {
            Reaction::Then { on_ok, on_err, derived } => {
                f(*on_ok);
                f(*on_err);
                if let Some(d) = derived {
                    f(Value::Object(*d));
                }
            }
            Reaction::ThenCap { on_ok, on_err, cap } => {
                f(*on_ok);
                f(*on_err);
                f(Value::Object(*cap));
            }
            Reaction::Await { coro } => f(Value::Object(*coro)),
        }
    }
}

impl Job {
    pub fn trace(&self, f: &mut dyn FnMut(Value)) {
        match self {
            Job::Reaction { reaction, value, .. } => {
                reaction.trace(f);
                f(*value);
            }
            Job::Thenable { promise, thenable, then } => {
                f(Value::Object(*promise));
                f(*thenable);
                f(*then);
            }
            Job::Call(v) => f(*v),
            Job::Cleanup { callback, held } => {
                f(*callback);
                f(*held);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum IterKind { Keys, Values, Entries }

pub enum ObjKind {
    Ordinary,
    Array(Vec<Option<Value>>),
    Function(Box<FuncData>),
    Error,
    Boolean(bool),
    Number(f64),
    String(StrId),
    Symbol(SymId),
    /// `Some`: a sloppy function's arguments object whose indices alias its parameters.
    Arguments(Option<Box<ArgMap>>),
    Host(u32, u64),
    ArrayIter { target: Value, index: u32, kind: IterKind, done: bool },
    StringIter { s: StrId, pos: u32 },
    ForIn { keys: Vec<StrId>, index: usize, obj: Value },
    /// Time value in milliseconds since the epoch (NaN = invalid date).
    Date(f64),
    /// Raw bytes; typed arrays and DataViews point at the buffer object by handle.
    ArrayBuffer(Vec<u8>),
    /// `length` counts elements.
    TypedArray(ViewData),
    /// `length` counts bytes; `kind` is unused.
    DataView(ViewData),
    /// A compiled pattern, shared with the runtime's regex cache.
    RegExp(Rc<RegexProg>),
    /// The iterator returned by `RegExp.prototype[Symbol.matchAll]`.
    RegExpStringIter { rx: Value, s: StrId, global: bool, unicode: bool, done: bool },
    Promise(Box<PromiseData>),
    Generator(Box<GenState>),
    /// Async iterator wrapping a sync iterator (`for await` over a sync iterable).
    AsyncFromSync(Value),
    Map(Box<MapData>),
    /// A Set stores each element as both key and value of its `MapData`.
    Set(Box<MapData>),
    MapIter { map: ObjId, pos: usize, kind: IterKind },
    /// Weak collections, keyed by object handle. The collector treats entries as ephemerons
    /// and drops those whose key died (see `collect_garbage`).
    WeakMap(Box<HashMap<u32, Value>>),
    WeakSet(Box<HashMap<u32, Value>>),
    Proxy(ProxyData),
    /// A BigInt wrapper object (`Object(1n)`).
    BigInt(BigId),
    /// The target is not traced; the collector clears it when the target dies.
    WeakRef(Option<ObjId>),
    FinReg(Box<FinRegData>),
    /// An `Intl` service object (DateTimeFormat, NumberFormat, ...).
    Intl(Box<crate::builtins::intl::IntlObj>),
    /// A module import binding: the exporting module's environment and binding slot.
    ImportCell(EnvId, u32),
}

/// The parameter bindings a mapped arguments object aliases: index -> slot of `env`
/// (`u32::MAX` once the index is unmapped).
pub struct ArgMap {
    pub env: EnvId,
    pub slots: Vec<u32>,
}

/// Element type of a typed array.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypedKind { Int8, Uint8, Uint8Clamped, Int16, Uint16, Int32, Uint32, Float32, Float64, BigInt64, BigUint64 }

impl TypedKind {
    pub const ALL: [TypedKind; 11] = [
        TypedKind::Int8, TypedKind::Uint8, TypedKind::Uint8Clamped, TypedKind::Int16, TypedKind::Uint16,
        TypedKind::Int32, TypedKind::Uint32, TypedKind::Float32, TypedKind::Float64, TypedKind::BigInt64, TypedKind::BigUint64,
    ];
    pub fn size(self) -> usize {
        match self {
            TypedKind::Int8 | TypedKind::Uint8 | TypedKind::Uint8Clamped => 1,
            TypedKind::Int16 | TypedKind::Uint16 => 2,
            TypedKind::Int32 | TypedKind::Uint32 | TypedKind::Float32 => 4,
            TypedKind::Float64 | TypedKind::BigInt64 | TypedKind::BigUint64 => 8,
        }
    }
    pub fn name(self) -> &'static str {
        [
            "Int8Array", "Uint8Array", "Uint8ClampedArray", "Int16Array", "Uint16Array", "Int32Array", "Uint32Array", "Float32Array",
            "Float64Array", "BigInt64Array", "BigUint64Array",
        ][self as usize]
    }
    /// Elements are BigInts (64-bit integers) rather than Numbers.
    pub fn is_big(self) -> bool {
        matches!(self, TypedKind::BigInt64 | TypedKind::BigUint64)
    }
}

/// A window onto an ArrayBuffer: a typed array or a DataView.
#[derive(Clone, Copy)]
pub struct ViewData {
    pub buffer: ObjId,
    /// Byte offset into the buffer.
    pub offset: usize,
    pub length: usize,
    pub kind: TypedKind,
}

/// A compiled regular expression with the pattern text it came from.
pub struct RegexProg {
    pub regex: invisium_regex::Regex,
    pub source: Rc<[u16]>,
    /// Heap charge: compiled program plus pattern text.
    pub bytes: usize,
}

/// A Proxy exotic object; `target`/`handler` are None once revoked.
#[derive(Clone, Copy)]
pub struct ProxyData {
    pub target: Option<ObjId>,
    pub handler: Option<ObjId>,
    pub callable: bool,
    pub ctor: bool,
}

/// Hashable identity of a value under SameValueZero.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum MapKey {
    Undefined,
    Null,
    Bool(bool),
    Num(u64),
    Str(Rc<[u16]>),
    Sym(u32),
    Obj(u32),
    /// A BigInt by value: sign and magnitude.
    Big(bool, Rc<[u32]>),
}

#[derive(Default)]
pub struct MapData {
    /// Insertion-ordered entries. A deleted entry leaves a tombstone so live iterators keep
    /// their position; tombstones are squeezed out by the collector when no iterator is alive.
    pub entries: Vec<Option<(Value, Value)>>,
    pub index: HashMap<MapKey, usize>,
    pub live: usize,
}

impl MapData {
    /// Removes the tombstones (only valid while no iterator holds a position).
    pub fn compact(&mut self) {
        let mut remap = Vec::with_capacity(self.entries.len());
        let mut n = 0;
        for e in &self.entries {
            remap.push(n);
            if e.is_some() {
                n += 1;
            }
        }
        self.entries.retain(|e| e.is_some());
        self.entries.shrink_to_fit();
        for v in self.index.values_mut() {
            *v = remap.get(*v).copied().unwrap_or(0);
        }
    }
}

impl ObjKind {
    /// Reports the values held by the Milestone 2 object kinds to the collector.
    pub fn trace_extra(&self, f: &mut dyn FnMut(Value), fe: &mut dyn FnMut(EnvId)) {
        match self {
            ObjKind::Promise(p) => {
                f(p.value);
                for r in &p.reactions {
                    r.trace(f);
                }
            }
            ObjKind::Generator(g) => g.trace(f, fe),
            ObjKind::AsyncFromSync(it) => f(*it),
            ObjKind::Intl(d) => d.trace(f),
            ObjKind::ImportCell(e, _) => fe(*e),
            ObjKind::Arguments(Some(m)) => fe(m.env),
            ObjKind::TypedArray(v) | ObjKind::DataView(v) => f(Value::Object(v.buffer)),
            ObjKind::RegExpStringIter { rx, s, .. } => {
                f(*rx);
                f(Value::String(*s));
            }
            ObjKind::Proxy(p) => {
                for o in [p.target, p.handler].into_iter().flatten() {
                    f(Value::Object(o));
                }
            }
            ObjKind::Map(d) | ObjKind::Set(d) => {
                for (k, v) in d.entries.iter().flatten() {
                    f(*k);
                    f(*v);
                }
            }
            _ => {}
        }
    }
}

pub struct Object {
    pub proto: Option<ObjId>,
    pub kind: ObjKind,
    pub props: PropMap,
    pub extensible: bool,
    /// Arrays: 1 = sealed, 2 = frozen (element attributes are not stored per element).
    pub integrity: u8,
}

impl Object {
    pub fn new(proto: Option<ObjId>, kind: ObjKind) -> Object {
        Object { proto, kind, props: PropMap::default(), extensible: true, integrity: 0 }
    }
    pub fn size_estimate(&self) -> usize {
        let base = 96 + self.props.len() * 48;
        base + match &self.kind {
            ObjKind::Array(v) => v.capacity() * 16,
            ObjKind::Function(f) => match &**f {
                FuncData::Bound { args, .. } => 64 + args.len() * 16,
                FuncData::Intern { slots, .. } => 48 + slots.len() * 16,
                _ => 128,
            },
            ObjKind::ForIn { keys, .. } => keys.len() * 4,
            ObjKind::Promise(p) => 64 + p.reactions.capacity() * 56,
            ObjKind::Map(d) | ObjKind::Set(d) => 96 + d.entries.capacity() * 40 + d.index.len() * 56,
            ObjKind::WeakMap(m) | ObjKind::WeakSet(m) => 64 + m.len() * 40,
            ObjKind::FinReg(d) => 64 + d.cells.len() * 40,
            ObjKind::ArrayBuffer(b) => b.capacity(),
            // a program is shared by the cache and every RegExp made from it: charge a share
            ObjKind::RegExp(p) => p.bytes / Rc::strong_count(p).max(1),
            ObjKind::Generator(g) => 160 + g.queue.len() * 40 + g.frame.as_ref().map(|s| 200 + (s.locals.len() + s.stack.len()) * 16 + s.handlers.len() * 40).unwrap_or(0),
            _ => 0,
        }
    }
}

pub struct Env {
    pub parent: Option<EnvId>,
    pub slots: Vec<Option<Value>>,
}

#[derive(Clone, Copy)]
pub struct GlobalLex {
    pub value: Option<Value>,
    pub mutable: bool,
}
