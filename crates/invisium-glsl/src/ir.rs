//! Types and the typed intermediate representation the parser builds and the generator reads.

use invisium_draw::gl::{SampleKind, TexKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum K {
    Bool,
    Int,
    Uint,
    Float,
}

impl K {
    pub fn wgsl(self) -> &'static str {
        match self {
            K::Bool => "bool",
            K::Int => "i32",
            K::Uint => "u32",
            K::Float => "f32",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Samp {
    S2D,
    S3D,
    Cube,
    S2DArray,
    S2DShadow,
    CubeShadow,
    S2DArrayShadow,
    I2D,
    I3D,
    ICube,
    I2DArray,
    U2D,
    U3D,
    UCube,
    U2DArray,
}

pub const SAMPLERS: [(&str, Samp, u32); 15] = [
    ("sampler2D", Samp::S2D, 0x8B5E),
    ("sampler3D", Samp::S3D, 0x8B5F),
    ("samplerCube", Samp::Cube, 0x8B60),
    ("sampler2DArray", Samp::S2DArray, 0x8DC1),
    ("sampler2DShadow", Samp::S2DShadow, 0x8B62),
    ("samplerCubeShadow", Samp::CubeShadow, 0x8DC5),
    ("sampler2DArrayShadow", Samp::S2DArrayShadow, 0x8DC4),
    ("isampler2D", Samp::I2D, 0x8DCA),
    ("isampler3D", Samp::I3D, 0x8DCB),
    ("isamplerCube", Samp::ICube, 0x8DCC),
    ("isampler2DArray", Samp::I2DArray, 0x8DCF),
    ("usampler2D", Samp::U2D, 0x8DD2),
    ("usampler3D", Samp::U3D, 0x8DD3),
    ("usamplerCube", Samp::UCube, 0x8DD4),
    ("usampler2DArray", Samp::U2DArray, 0x8DD7),
];

impl Samp {
    pub fn info(self) -> (&'static str, u32) {
        SAMPLERS.iter().find(|s| s.1 == self).map_or(("sampler2D", 0x8B5E), |s| (s.0, s.2))
    }
    pub fn kind(self) -> TexKind {
        use Samp::*;
        match self {
            S2D | S2DShadow | I2D | U2D => TexKind::D2,
            S3D | I3D | U3D => TexKind::D3,
            Cube | CubeShadow | ICube | UCube => TexKind::Cube,
            S2DArray | S2DArrayShadow | I2DArray | U2DArray => TexKind::Array,
        }
    }
    pub fn sample(self) -> SampleKind {
        use Samp::*;
        match self {
            S2D | S3D | Cube | S2DArray => SampleKind::Float,
            S2DShadow | CubeShadow | S2DArrayShadow => SampleKind::Shadow,
            I2D | I3D | ICube | I2DArray => SampleKind::Int,
            U2D | U3D | UCube | U2DArray => SampleKind::Uint,
        }
    }
    /// WGSL texture and sampler types.
    pub fn wgsl(self) -> (String, &'static str) {
        let dim = match self.kind() {
            TexKind::D2 => "2d",
            TexKind::D3 => "3d",
            TexKind::Cube => "cube",
            TexKind::Array => "2d_array",
        };
        match self.sample() {
            SampleKind::Shadow => (format!("texture_depth_{dim}"), "sampler_comparison"),
            SampleKind::Int => (format!("texture_{dim}<i32>"), "sampler"),
            SampleKind::Uint => (format!("texture_{dim}<u32>"), "sampler"),
            SampleKind::Float => (format!("texture_{dim}<f32>"), "sampler"),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Ty {
    Void,
    S(K),
    V(K, u8),
    /// Columns, rows.
    M(u8, u8),
    Struct(u32),
    Array(Box<Ty>, u32),
    Sampler(Samp),
}

pub const FLOAT: Ty = Ty::S(K::Float);
pub const INT: Ty = Ty::S(K::Int);
pub const UINT: Ty = Ty::S(K::Uint);
pub const BOOL: Ty = Ty::S(K::Bool);

impl Ty {
    pub fn vec(k: K, n: u8) -> Ty {
        if n == 1 {
            Ty::S(k)
        } else {
            Ty::V(k, n)
        }
    }
    /// Component kind of a scalar, vector or matrix.
    pub fn kind(&self) -> Option<K> {
        match self {
            Ty::S(k) | Ty::V(k, _) => Some(*k),
            Ty::M(..) => Some(K::Float),
            _ => None,
        }
    }
    /// Components of a scalar, vector or matrix (0 for anything else).
    pub fn comps(&self) -> u32 {
        match self {
            Ty::S(_) => 1,
            Ty::V(_, n) => *n as u32,
            Ty::M(c, r) => (*c as u32) * (*r as u32),
            _ => 0,
        }
    }
    /// Vector size (1 for scalars).
    pub fn size(&self) -> u8 {
        match self {
            Ty::V(_, n) => *n,
            _ => 1,
        }
    }
    pub fn is_scalar(&self) -> bool {
        matches!(self, Ty::S(_))
    }
    pub fn is_vector(&self) -> bool {
        matches!(self, Ty::V(..))
    }
    pub fn is_sv(&self) -> bool {
        matches!(self, Ty::S(_) | Ty::V(..))
    }
    pub fn is_mat(&self) -> bool {
        matches!(self, Ty::M(..))
    }
    pub fn is_numeric(&self) -> bool {
        matches!(self.kind(), Some(K::Int | K::Uint | K::Float))
    }
    pub fn is_int(&self) -> bool {
        matches!(self, Ty::S(K::Int | K::Uint) | Ty::V(K::Int | K::Uint, _))
    }
    pub fn is_float(&self) -> bool {
        matches!(self, Ty::S(K::Float) | Ty::V(K::Float, _))
    }
    pub fn is_bool(&self) -> bool {
        matches!(self, Ty::S(K::Bool) | Ty::V(K::Bool, _))
    }
    pub fn has_sampler(&self, m: &Module) -> bool {
        match self {
            Ty::Sampler(_) => true,
            Ty::Array(e, _) => e.has_sampler(m),
            Ty::Struct(s) => m.structs[*s as usize].sampler,
            _ => false,
        }
    }
    /// Nesting depth of structure types inside this type (0 for none).
    pub fn struct_depth(&self, m: &Module) -> u32 {
        match self {
            Ty::Array(e, _) => e.struct_depth(m),
            Ty::Struct(s) => m.structs[*s as usize].depth,
            _ => 0,
        }
    }
    /// Rough size in 4-byte words, for the size limits. Constant time: structures store
    /// their size when they are defined.
    pub fn words(&self, m: &Module) -> u64 {
        match self {
            Ty::Array(e, n) => e.words(m).saturating_mul(*n as u64),
            Ty::Struct(s) => m.structs[*s as usize].words,
            Ty::M(c, _) => *c as u64 * 4,
            _ => 4,
        }
    }
    pub fn name(&self, m: &Module) -> String {
        match self {
            Ty::Void => "void".into(),
            Ty::S(K::Bool) => "bool".into(),
            Ty::S(K::Int) => "int".into(),
            Ty::S(K::Uint) => "uint".into(),
            Ty::S(K::Float) => "float".into(),
            Ty::V(k, n) => format!("{}vec{n}", ["b", "i", "u", ""][*k as usize]),
            Ty::M(c, r) if c == r => format!("mat{c}"),
            Ty::M(c, r) => format!("mat{c}x{r}"),
            Ty::Struct(s) => m.structs[*s as usize].name.clone(),
            Ty::Array(e, n) => format!("{}[{n}]", e.name(m)),
            Ty::Sampler(s) => s.info().0.into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CVal {
    B(bool),
    I(i32),
    U(u32),
    F(f32),
}

impl CVal {
    pub fn ty(self) -> Ty {
        match self {
            CVal::B(_) => BOOL,
            CVal::I(_) => INT,
            CVal::U(_) => UINT,
            CVal::F(_) => FLOAT,
        }
    }
    pub fn as_i64(self) -> Option<i64> {
        match self {
            CVal::I(v) => Some(v as i64),
            CVal::U(v) => Some(v as i64),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UOp {
    Neg,
    Not,
    BitNot,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
    And,
    Or,
    Xor,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    LogXor,
}

pub type VarId = u32;
pub type FuncId = u32;

#[derive(Clone, Debug)]
pub struct Expr {
    pub ty: Ty,
    pub k: EK,
    /// Has side effects (assignments, `++`, calls of user functions).
    pub fx: bool,
    pub depth: u32,
}

#[derive(Clone, Debug)]
pub enum EK {
    Lit(CVal),
    Var(VarId),
    Un(UOp, Box<Expr>),
    Bin(BOp, Box<Expr>, Box<Expr>),
    LogAnd(Box<Expr>, Box<Expr>),
    LogOr(Box<Expr>, Box<Expr>),
    Assign(Option<BOp>, Box<Expr>, Box<Expr>),
    Inc { pre: bool, dec: bool, e: Box<Expr> },
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Comma(Box<Expr>, Box<Expr>),
    Ctor(Vec<Expr>),
    Swz(Box<Expr>, Vec<u8>),
    Index(Box<Expr>, Box<Expr>),
    Field(Box<Expr>, u32),
    Call(FuncId, Vec<Expr>),
    Bi(&'static str, Vec<Expr>),
}

impl Expr {
    pub fn new(ty: Ty, k: EK) -> Expr {
        let mut fx = matches!(k, EK::Assign(..) | EK::Inc { .. } | EK::Call(..) | EK::Bi("modf", _));
        let mut depth = 0;
        for c in k.children() {
            fx |= c.fx;
            depth = depth.max(c.depth);
        }
        Expr { ty, k, fx, depth: depth + 1 }
    }
    pub fn lit(c: CVal) -> Expr {
        Expr::new(c.ty(), EK::Lit(c))
    }
}

impl EK {
    pub fn children(&self) -> Vec<&Expr> {
        match self {
            EK::Lit(_) | EK::Var(_) => vec![],
            EK::Un(_, a) | EK::Swz(a, _) | EK::Field(a, _) | EK::Inc { e: a, .. } => vec![a],
            EK::Bin(_, a, b) | EK::LogAnd(a, b) | EK::LogOr(a, b) | EK::Assign(_, a, b) | EK::Comma(a, b) | EK::Index(a, b) => {
                vec![a, b]
            }
            EK::Cond(a, b, c) => vec![a, b, c],
            EK::Ctor(v) | EK::Call(_, v) | EK::Bi(_, v) => v.iter().collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Decl(VarId, Option<Expr>),
    Expr(Expr),
    Block(Vec<Stmt>),
    /// `if (c0) s0 else if (c1) s1 ... else e`, kept flat so long chains never recurse.
    If(Vec<(Expr, Stmt)>, Option<Box<Stmt>>),
    For { init: Vec<Stmt>, cond: Option<Expr>, step: Option<Expr>, body: Box<Stmt> },
    While(Expr, Box<Stmt>),
    DoWhile(Box<Stmt>, Expr),
    Switch(Expr, Vec<Case>),
    Break,
    Continue,
    Return(Option<Expr>),
    Discard,
}

#[derive(Clone, Debug)]
pub struct Case {
    /// `None` is `default`.
    pub labels: Vec<Option<CVal>>,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PQ {
    In,
    Out,
    InOut,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BV {
    Position,
    PointSize,
    VertexID,
    InstanceID,
    FragCoord,
    FrontFacing,
    PointCoord,
    FragColor,
    FragData,
    FragDepth,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Storage {
    Local,
    Param(PQ),
    Global,
    Const,
    Uniform,
    Attribute,
    VaryingOut,
    VaryingIn,
    FragOut,
    Builtin(BV),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Interp {
    pub flat: bool,
    pub centroid: bool,
}

#[derive(Clone, Debug)]
pub struct Var {
    pub name: String,
    pub ty: Ty,
    pub storage: Storage,
    pub line: u32,
    pub location: Option<u32>,
    pub interp: Interp,
    pub cval: Option<CVal>,
    pub global: bool,
}

#[derive(Clone, Debug)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
    /// Computed once at the definition, so no type walk recurses through structures.
    pub words: u64,
    pub sampler: bool,
    pub depth: u32,
    /// Uniform-buffer layout: field offsets and the 16-byte rounded size.
    pub offs: Vec<u32>,
    pub size: u32,
}

#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub ret: Ty,
    pub params: Vec<VarId>,
    pub body: Option<Vec<Stmt>>,
    pub line: u32,
    /// Calls made by the body: callee and line.
    pub calls: Vec<(FuncId, u32)>,
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub structs: Vec<StructDef>,
    pub vars: Vec<Var>,
    pub funcs: Vec<Func>,
    /// Global variables (any storage) in declaration order.
    pub globals: Vec<VarId>,
    /// Initialisers of private globals and global constants, in order.
    pub inits: Vec<(VarId, Expr)>,
    pub main: FuncId,
}
