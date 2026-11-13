//! Shader translator: GLSL ES 1.00 and 3.00 in, WGSL out, plus the reflection data WebGL
//! needs (active attributes and uniforms, where each uniform lives, texture slots, outputs).
//!
//! Pure CPU code: no GPU, no I/O. The public items in this file are the contract with the GL
//! state machine (`invisium-gl`); see `docs/agents/GLSL.md`.

pub use invisium_draw::gl::{BlockSlot, Scalar, TexSlot};

mod builtins;
mod check;
mod gbuiltin;
mod gen;
mod gentry;
mod gexpr;
mod gops;
mod ir;
mod layout;
mod lex;
mod link;
mod parse;
mod pdecl;
mod pexpr;
mod pp;
mod ppexpr;
mod pstmt;

/// Largest shader source accepted.
const MAX_SOURCE: usize = 1 << 20;

/// A compile error at a line of the original source.
#[derive(Debug)]
pub(crate) struct Error {
    pub line: u32,
    pub msg: String,
}

impl Error {
    pub fn new(line: u32, msg: impl Into<String>) -> Error {
        Error { line, msg: msg.into() }
    }
    pub fn log(&self) -> String {
        format!("ERROR: 0:{}: {}\n", self.line, self.msg)
    }
}

pub(crate) fn err<T>(line: u32, msg: impl Into<String>) -> Result<T, Error> {
    Err(Error::new(line, msg))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Vertex,
    Fragment,
}

/// A shader that compiled: parsed, type-checked and ready to link.
pub struct Shader {
    stage: Stage,
    version: u32,
    module: ir::Module,
}

impl Shader {
    pub fn stage(&self) -> Stage {
        self.stage
    }
    /// 100 or 300.
    pub fn version(&self) -> u32 {
        self.version
    }
}

/// Run `f` on a thread with a large stack (the parser and generator recurse to bounded but
/// sizeable depths); on the caller's thread if no thread can be started.
fn big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> Option<T> {
    let cell = std::sync::Mutex::new(Some(f));
    let take = || cell.lock().ok().and_then(|mut g| g.take());
    let r = std::thread::scope(|s| {
        match std::thread::Builder::new().stack_size(64 << 20).spawn_scoped(s, || take().map(|f| f())) {
            Ok(h) => Some(h.join().ok().flatten()),
            Err(_) => None,
        }
    });
    match r {
        Some(v) => v,
        // No thread: run here, still turning a panic into `None` (an internal error).
        None => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| take().map(|f| f()))).ok().flatten(),
    }
}

/// Compile one shader. `webgl2` says what the context accepts: a WebGL 2 context takes
/// `#version 300 es` and version 100 shaders, a WebGL 1 context only version 100.
/// `Err` is the info log (`ERROR: 0:<line>: <message>` lines).
pub fn compile(stage: Stage, source: &str, webgl2: bool) -> Result<Shader, String> {
    if source.len() > MAX_SOURCE {
        return Err(Error::new(1, "shader source is too large (limit 1 MiB)").log());
    }
    let r = big_stack(|| -> Result<Shader, Error> {
        let pp = pp::preprocess(source, stage, webgl2)?;
        let mut p = parse::Parser::new(&pp.toks, stage, pp.version, pp.exts);
        p.translation_unit()?;
        Ok(Shader { stage, version: pp.version, module: p.m })
    });
    match r {
        Some(r) => r.map_err(|e| e.log()),
        None => Err("ERROR: 0:1: internal compiler error\n".into()),
    }
}

/// Link a vertex and a fragment shader. `attrib_bindings` are the `bindAttribLocation`
/// requests (name, location). `Err` is the info log.
pub fn link(vertex: &Shader, fragment: &Shader, attrib_bindings: &[(String, u32)]) -> Result<Program, String> {
    if vertex.stage != Stage::Vertex || fragment.stage != Stage::Fragment {
        return Err("ERROR: link needs a vertex and a fragment shader\n".into());
    }
    if vertex.version != fragment.version {
        return Err("ERROR: the vertex and fragment shaders have different versions\n".into());
    }
    big_stack(|| link::link(&vertex.module, &fragment.module, attrib_bindings))
        .unwrap_or_else(|| Err("ERROR: 0:1: internal compiler error\n".into()))
}

/// A linked program.
#[derive(Clone, Debug, Default)]
pub struct Program {
    /// WGSL module with the entry point `vs_main`.
    pub vertex_wgsl: String,
    /// WGSL module with the entry point `fs_main`.
    pub fragment_wgsl: String,
    /// Size of the default uniform block (group 0, binding 0); 0 if there is none.
    pub uniform_bytes: u32,
    pub attributes: Vec<Attribute>,
    pub uniforms: Vec<Uniform>,
    /// Texture + sampler pairs (group 1), in slot order.
    pub textures: Vec<TexSlot>,
    pub blocks: Vec<Block>,
    pub outputs: Vec<Output>,
    pub writes_depth: bool,
}

/// An active vertex attribute.
#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub name: String,
    /// GL type enum (`FLOAT_VEC3` = 0x8B51, ...).
    pub gl_type: u32,
    /// First `@location`. A matrix takes `columns` consecutive locations.
    pub location: u32,
    pub columns: u32,
    pub base: Scalar,
}

/// A fragment shader output.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub name: String,
    pub location: u32,
    pub base: Scalar,
}

/// How values of a uniform are stored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Base {
    F32,
    I32,
    U32,
    /// Stored as a `u32`: 0 or 1.
    Bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    /// In the default uniform block. Element `i` of an array starts at `offset + i * stride`;
    /// column `c` of a matrix starts `c * col_stride` bytes into its element; a column (or a
    /// vector, or a scalar) is `rows` values of 4 bytes each.
    Block { offset: u32, stride: u32, base: Base, rows: u8, cols: u8, col_stride: u32 },
    /// A sampler. Array element `i` uses `Program::textures[slot + i]`.
    Texture { slot: u32 },
    /// A member of the named block `Program::blocks[block]`, `offset` bytes into it. It is set
    /// through the buffer bound to that block, not with `uniform*`.
    Named { block: u32, offset: u32 },
}

/// An active uniform, named the way `getActiveUniform` reports it: `uTime`, `light.color`,
/// `weights[0]` (an array of a basic type is one entry with `size` elements),
/// `lights[2].position` (arrays of structures are one entry per element and member).
#[derive(Clone, Debug, PartialEq)]
pub struct Uniform {
    pub name: String,
    /// GL type enum (`FLOAT_MAT4` = 0x8B5C, `SAMPLER_2D` = 0x8B5E, ...).
    pub gl_type: u32,
    /// Array length; 1 if it is not an array.
    pub size: u32,
    pub place: Place,
}

/// A named uniform block (`uniform Name { ... };`, GLSL ES 3.00).
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub name: String,
    pub slot: BlockSlot,
    pub in_vertex: bool,
    pub in_fragment: bool,
}
