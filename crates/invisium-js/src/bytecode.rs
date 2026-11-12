//! Bytecode produced by the compiler. Plain owned data (Send + Sync) so compilation can run on a
//! helper thread; name operands index the script's name table.

use crate::ast::{FuncKind, Pos, Str16};
use std::sync::Arc;

pub type Name = u32;
pub type Label = u32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Undef,
    Null,
    True,
    False,
    Num(f64),
    /// A BigInt literal; the operand names its digits.
    BigInt(Name),
    Str(Name),
    Regex(Name, Name),
    Pop,
    Dup,
    Dup2,
    Swap,
    /// Moves the top value below the next `n - 1` values.
    Rot(u32),

    GetLocal(u32),
    PutLocal(u32),
    InitLocal(u32),
    ClearLocal(u32),
    GetEnv(u32, u32),
    PutEnv(u32, u32),
    InitEnv(u32, u32),
    PushEnv(u32),
    PopEnv,
    CopyEnv,
    GetGlobal(Name),
    GetGlobalTypeof(Name),
    PutGlobal(Name),
    DeleteGlobal(Name),
    DeclareVar(Name),
    DeclareFunc(Name),
    DeclareLex(Name, bool),
    InitGlobalLex(Name),
    ThrowConstAssign(Name),

    GetProp(Name),
    GetPropKeep(Name),
    PutProp(Name),
    GetElem,
    GetElemKeep,
    PutElem,
    DeleteProp(Name),
    DeleteElem,
    SuperGetElem,
    SuperPutElem,
    ToPropertyKey,

    Add, Sub, Mul, Div, Mod, Exp, Shl, Shr, UShr, BitAnd, BitOr, BitXor,
    Eq, Ne, StrictEq, StrictNe, Lt, Gt, Le, Ge, In, InstanceOf,
    Neg, Plus, Not, BitNot, TypeOf, Inc, Dec, ToNumeric, ToStr,

    Jump(Label),
    JumpIfFalse(Label),
    JumpIfTrue(Label),
    /// If top is falsy jump (keeping it), else pop.
    AndJump(Label),
    OrJump(Label),
    /// If top is neither null nor undefined jump (keeping it), else pop.
    NullishJump(Label),
    /// If top is null/undefined pop `n` values and jump.
    OptJump(Label, u32),
    /// Peek: jump if the top value is an object.
    JumpIfObject(Label),
    CheckObjectCoercible,

    Call(u32),
    CallMethod(u32),
    New(u32),
    CallArr,
    CallMethodArr,
    NewArr,
    SuperCall(u32),
    SuperCallArr,
    RunFieldInit,
    Return,

    Closure(u32),
    LoadThis,
    LoadNewTarget,
    CreateArguments,
    /// args -> args ; argument `index` (when it was passed) aliases env slot `slot`
    MapArgument(u32, u32),
    LoadCallee,
    GetArg(u32),
    RestArgs(u32),

    NewObject,
    NewArray,
    ArrayPush,
    ArrayHole,
    ArraySpread,
    DefineProp(Name),
    DefineElem,
    /// obj key func -> obj ; flag = enumerable
    DefineMethod(bool),
    /// obj key func -> obj ; (is_getter, enumerable)
    DefineAccessor(bool, bool),
    SetProtoLiteral,
    CopyDataProps,
    ObjRest(u32),
    TemplateObject(u32),
    /// parent? -> ctor proto ; (ctor code index, has_parent)
    MakeClass(u32, bool),
    SetFieldInit,
    SetHome,

    Throw,
    PushTry(Label),
    PopTry,

    GetIter,
    /// iter -> iter value, or jump with iter when done
    IterNext(Label),
    /// iter -> iter value (undefined when exhausted)
    IterNextOrUndef,
    IterRest,
    IterClose,
    /// iter exception -> ; closes the iterator (errors from `return()` are dropped), rethrows
    IterCloseThrow,
    /// Ends a generator function's prologue (parameters are bound): the frame becomes the
    /// suspended generator object the call returns.
    GenStart,
    ForInStart,
    ForInNext(Label),
    Debugger,

    /// f args.. -> result ; a call written `eval(...)`: direct eval when `f` is the built-in
    /// `eval`, else an ordinary call. (argc, index into `FuncCode::eval_scopes`)
    DirectEval(u32, u32),
    /// Name lookups that may hit a `with` object or an eval-declared variable first; the
    /// operand indexes `FuncCode::dyn_refs`.
    DynGet(u32),
    DynTypeof(u32),
    /// value ->
    DynPut(u32),
    /// -> bool
    DynDelete(u32),
    /// -> this func
    DynCallee(u32),
    /// Declares `name` in the variable object kept at env (hops, slot). (name, hops, slot)
    DeclareEvalVar(Name, u32, u32),
    /// value -> object ; TypeError for null / undefined
    ToObject,
    /// -> key ; a fresh private name
    NewPrivateName,
    /// obj key -> value ; the operand names the member in error messages
    PrivGet(Name),
    /// obj key -> obj value
    PrivGetKeep(Name),
    /// obj key value -> value
    PrivPut(Name),
    /// key obj -> bool
    PrivIn,
    /// obj key value -> obj ; (kind: 0 field, 1 method, 2 getter, 3 setter; name)
    PrivDefine(u32, Name),

    /// value -> (frame suspends) -> received value
    Yield,
    /// value -> (frame suspends) -> received mode ; `yield*` step: mode 0 next, 1 throw, 2 return
    YieldDelegate,
    /// value -> (frame suspends) -> fulfilment value, or throws the rejection reason
    Await,
    /// value -> ; completes the generator with `value` through the enclosing `finally` blocks
    GenReturn,
    /// Rethrows the generator-return marker when it reaches a `catch` clause.
    RethrowMarker,
    /// iterable -> async iterator
    GetAsyncIter,
    /// Throws a TypeError unless the top value is an object.
    CheckIterResult,
    ThrowTypeError(Name),
    /// -> value ; reads a module import binding: env (hops, slot) holds the cell that names
    /// the exporter's binding (ReferenceError while that is uninitialised)
    GetImport(u32, u32),
}

#[derive(Debug)]
pub struct Template {
    pub cooked: Vec<Option<Str16>>,
    pub raw: Vec<Str16>,
}

/// What a name falls back to when no dynamic scope object has it.
#[derive(Clone, Copy, Debug)]
pub enum DynBase {
    Local(u32),
    Env(u32, u32),
    Global,
}

/// A name reference compiled under a `with` statement or in reach of a direct `eval`.
#[derive(Debug)]
pub struct DynRef {
    pub name: Name,
    /// Scope objects to try first, innermost first: (env hops, slot, is a `with` object).
    pub objs: Vec<(u32, u32, bool)>,
    pub base: DynBase,
    pub is_const: bool,
}

/// One lexical scope as a direct `eval` call site sees it.
#[derive(Debug)]
pub struct ScopeInfo {
    /// (name, env slot, 0 var / 1 let / 2 const)
    pub bindings: Vec<(String, u32, u8)>,
    pub env: bool,
    pub obj_slot: Option<u32>,
    pub is_with: bool,
}

#[derive(Debug)]
pub struct FnInfo {
    pub kind: FuncKind,
    pub strict: bool,
    pub is_script: bool,
    pub is_eval: bool,
    pub scopes: Vec<ScopeInfo>,
}

/// The static scope chain at a direct `eval` call site: enclosing functions, outermost first.
#[derive(Debug)]
pub struct EvalScope {
    pub fns: Vec<FnInfo>,
    pub strict: bool,
    pub new_target: bool,
    pub super_prop: bool,
    pub super_call: bool,
}

#[derive(Debug)]
pub struct FuncCode {
    pub name: Str16,
    pub ops: Vec<Op>,
    pub children: Vec<Arc<FuncCode>>,
    pub templates: Vec<Template>,
    pub lines: Vec<(u32, Pos)>,
    pub nlocals: u32,
    pub length: u32,
    pub kind: FuncKind,
    pub is_async: bool,
    pub is_gen: bool,
    pub strict: bool,
    pub source: Arc<[char]>,
    pub source_range: (usize, usize),
    /// Name of the script this function came from (the `filename` given to `eval`).
    pub script: Arc<str>,
    /// Source text of the callee / iterable expression at a call, `new` or iteration site,
    /// sorted by pc; only read when such a site throws.
    pub descs: Vec<(u32, Box<str>)>,
    pub dyn_refs: Vec<DynRef>,
    pub eval_scopes: Vec<Arc<EvalScope>>,
    /// Compiled by script (`eval`, `Function`): its source text counts toward the heap cap
    /// while a closure of it is alive.
    pub dynamic: bool,
}

impl FuncCode {
    pub fn desc_at(&self, pc: usize) -> Option<&str> {
        let i = self.descs.binary_search_by(|(p, _)| (*p as usize).cmp(&pc)).ok()?;
        Some(&self.descs[i].1)
    }

    pub fn pos_at(&self, pc: usize) -> Pos {
        let i = match self.lines.binary_search_by(|(p, _)| (*p as usize).cmp(&pc)) {
            Ok(i) => i,
            Err(0) => return Pos { line: 0, col: 0 },
            Err(i) => i - 1,
        };
        self.lines[i].1
    }
}

pub struct CompiledScript {
    pub names: Vec<Str16>,
    pub top: Arc<FuncCode>,
}
