//! Abstract syntax tree. Plain owned data (Send) so parsing can run on a helper thread.

pub type Str16 = Vec<u16>;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod, Exp, Shl, Shr, UShr, BitAnd, BitOr, BitXor,
    Eq, Ne, StrictEq, StrictNe, Lt, Gt, Le, Ge, In, InstanceOf,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LogicOp { And, Or, Nullish }

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnOp { Neg, Plus, Not, BitNot, TypeOf, Void, Delete }

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AssignOp { Assign, Bin(BinOp), Logic(LogicOp) }

#[derive(Clone, Debug)]
pub enum PropName {
    Ident(String),
    Str(Str16),
    Num(f64),
    Computed(Box<Expr>),
    /// `#name` (class members only).
    Private(String),
}

#[derive(Clone, Debug)]
pub enum ArrayItem {
    Hole,
    Expr(Expr),
    Spread(Expr),
}

#[derive(Clone, Debug)]
pub enum ObjProp {
    Init(PropName, Expr),
    Shorthand(String, Pos),
    /// `{a = 1}` — only valid once converted to a pattern.
    CoverInit(String, Expr),
    Method(PropName, Box<Function>),
    Get(PropName, Box<Function>),
    Set(PropName, Box<Function>),
    Spread(Expr),
}

#[derive(Clone, Debug)]
pub struct TemplateParts {
    pub cooked: Vec<Option<Str16>>,
    pub raw: Vec<Str16>,
    pub exprs: Vec<Expr>,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Num(f64),
    /// A BigInt literal (its digits, with any radix prefix).
    BigInt(String),
    Str(Str16),
    Bool(bool),
    Null,
    Template(Box<TemplateParts>),
    Tagged(Box<Expr>, Box<TemplateParts>, Pos),
    Regex(String, String),
    Ident(String, Pos),
    This,
    NewTarget,
    Array(Vec<ArrayItem>),
    Object(Vec<ObjProp>),
    Function(Box<Function>),
    Class(Box<Class>),
    Unary(UnOp, Box<Expr>, Pos),
    Update { inc: bool, prefix: bool, target: Box<Expr>, pos: Pos },
    Binary(BinOp, Box<Expr>, Box<Expr>, Pos),
    Logical(LogicOp, Box<Expr>, Box<Expr>),
    Assign(AssignOp, Box<Pattern>, Box<Expr>, Pos),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Call { callee: Box<Expr>, args: Vec<ArrayItem>, optional: bool, pos: Pos },
    New { callee: Box<Expr>, args: Vec<ArrayItem>, pos: Pos },
    Member { obj: Box<Expr>, name: String, optional: bool, pos: Pos },
    Index { obj: Box<Expr>, index: Box<Expr>, optional: bool, pos: Pos },
    SuperMember(String, Pos),
    SuperIndex(Box<Expr>, Pos),
    SuperCall(Vec<ArrayItem>, Pos),
    /// Delimits the short-circuit range of an optional chain.
    OptChain(Box<Expr>),
    Seq(Vec<Expr>),
    /// Parenthesised expression (kept so `(a) = 1` / cover grammar can be checked).
    Paren(Box<Expr>),
    /// `obj.#name`
    PrivateMember { obj: Box<Expr>, name: String, optional: bool, pos: Pos },
    /// `#name in obj`
    PrivateIn(String, Box<Expr>, Pos),
    /// Compiler-made: defines private member `#name` on `this`; kind 0 field, 1 method,
    /// 2 getter, 3 setter.
    PrivateDefine { name: String, value: Box<Expr>, kind: u8 },
    /// `yield expr` / `yield* expr` (the flag marks delegation).
    Yield(Option<Box<Expr>>, bool),
    Await(Box<Expr>),
}

#[derive(Clone, Debug)]
pub enum Pattern {
    Ident(String, Pos),
    /// Assignment-only target: member/index expression.
    Expr(Expr),
    Object { props: Vec<(PropName, Pattern)>, rest: Option<Box<Pattern>> },
    Array { elems: Vec<Option<Pattern>>, rest: Option<Box<Pattern>> },
    Default(Box<Pattern>, Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FuncKind { Normal, Arrow, Method, Getter, Setter, BaseCtor, DerivedCtor, ClassInit }

#[derive(Clone, Debug)]
pub struct Function {
    pub name: Option<String>,
    /// Named function expression: the name is bound inside the body.
    pub self_binding: bool,
    pub params: Vec<Pattern>,
    pub rest: Option<Pattern>,
    pub body: Vec<Stmt>,
    pub kind: FuncKind,
    pub is_async: bool,
    pub is_gen: bool,
    pub strict: bool,
    pub simple_params: bool,
    pub pos: Pos,
    /// Source text (for Function.prototype.toString).
    pub source: (usize, usize),
}

#[derive(Clone, Debug)]
pub enum MemberKind {
    Method(Box<Function>),
    Get(Box<Function>),
    Set(Box<Function>),
    Field(Option<Expr>),
    StaticBlock(Vec<Stmt>),
}

#[derive(Clone, Debug)]
pub struct ClassMember {
    pub is_static: bool,
    pub key: PropName,
    pub kind: MemberKind,
}

#[derive(Clone, Debug)]
pub struct Class {
    pub name: Option<String>,
    pub extends: Option<Expr>,
    pub ctor: Option<Box<Function>>,
    pub members: Vec<ClassMember>,
    pub pos: Pos,
    pub source: (usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VarKind { Var, Let, Const }

#[derive(Clone, Debug)]
pub enum ForInit {
    Var(VarKind, Vec<(Pattern, Option<Expr>)>),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub enum ForHead {
    Var(VarKind, Pattern),
    Pattern(Pattern),
}

#[derive(Clone, Debug)]
pub struct Case {
    pub test: Option<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    Expr(Expr),
    Var(VarKind, Vec<(Pattern, Option<Expr>)>),
    Func(Box<Function>),
    Class(Box<Class>),
    Return(Option<Expr>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    Block(Vec<Stmt>),
    For { init: Option<ForInit>, test: Option<Expr>, update: Option<Expr>, body: Box<Stmt> },
    ForIn(ForHead, Expr, Box<Stmt>),
    ForOf(ForHead, Expr, Box<Stmt>),
    ForAwait(ForHead, Expr, Box<Stmt>),
    While(Expr, Box<Stmt>),
    DoWhile(Box<Stmt>, Expr),
    Break(Option<String>),
    Continue(Option<String>),
    Throw(Expr),
    Try { block: Vec<Stmt>, param: Option<Pattern>, handler: Option<Vec<Stmt>>, finalizer: Option<Vec<Stmt>> },
    Switch(Expr, Vec<Case>),
    Labeled(String, Box<Stmt>),
    With(Expr, Box<Stmt>),
    Empty,
    Debugger,
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub pos: Pos,
}

pub struct Program {
    pub body: Vec<Stmt>,
    pub strict: bool,
}

/// `import ... from 'request'`: binds `local` to export `import_name` of the requested
/// module (`None`: its namespace object).
#[derive(Clone, Debug)]
pub struct ImportEntry {
    pub request: usize,
    pub import_name: Option<String>,
    pub local: String,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
pub enum ExportEntry {
    /// `export { local as name }` and exported declarations.
    Local { name: String, local: String, pos: Pos },
    /// `export { x as name } from 'request'` and `export * as name from 'request'` (`None`).
    Indirect { name: String, request: usize, import_name: Option<String>, pos: Pos },
    /// `export * from 'request'`.
    Star { request: usize, pos: Pos },
}

/// The import / export declarations of a module; the declarations they carry stay in the body.
#[derive(Clone, Debug, Default)]
pub struct ModuleInfo {
    /// Module specifiers, in source order, without duplicates.
    pub requests: Vec<String>,
    pub imports: Vec<ImportEntry>,
    pub exports: Vec<ExportEntry>,
    /// The body awaits at top level.
    pub has_tla: bool,
}
