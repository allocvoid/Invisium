//! Objects of Milestone 2: shaders, programs, uniform locations, vertex arrays, samplers and
//! the inert query / sync / transform-feedback stand-ins.

use crate::cmd::ConstValue;
use crate::consts1::*;
use crate::objects::TexParams;
use std::rc::Rc;

pub struct Shader {
    /// `VERTEX_SHADER` or `FRAGMENT_SHADER`.
    pub ty: u32,
    pub source: String,
    pub compiled: Option<invisium_glsl::Shader>,
    pub compile_status: bool,
    pub log: String,
    pub delete_pending: bool,
    /// Programs this shader is attached to.
    pub attached: u32,
}

/// A successful link: the translator's program and the uniform values.
pub struct Linked {
    pub prog: invisium_glsl::Program,
    pub data: Vec<u8>,
    /// What the last draw handed out; `None` after a change.
    pub shared: Option<Rc<[u8]>>,
    /// Texture unit of every texture slot.
    pub units: Vec<u32>,
    /// `uniformBlockBinding` of every named block.
    pub block_bindings: Vec<u32>,
}

impl Linked {
    pub fn new(prog: invisium_glsl::Program) -> Linked {
        let data = vec![0; prog.uniform_bytes as usize];
        let units = vec![0; prog.textures.len()];
        let block_bindings = vec![0; prog.blocks.len()];
        Linked { prog, data, shared: None, units, block_bindings }
    }
    pub fn uniforms(&mut self) -> Rc<[u8]> {
        self.shared.get_or_insert_with(|| Rc::from(self.data.as_slice())).clone()
    }
}

#[derive(Default)]
pub struct Program {
    /// Attached shaders (at most one per stage).
    pub shaders: Vec<u32>,
    pub bindings: Vec<(String, u32)>,
    pub link_status: bool,
    pub validate_status: bool,
    pub log: String,
    /// The executable used by draws and uniform calls.
    pub exe: Option<Linked>,
    pub delete_pending: bool,
    pub on_dev: bool,
    pub tf_varyings: u32,
    pub tf_mode: u32,
}

/// What a `WebGLUniformLocation` names.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Loc {
    pub program: u32,
    pub uniform: usize,
    pub elem: u32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AttribState {
    pub enabled: bool,
    pub buffer: u32,
    pub size: u8,
    pub ty: u32,
    pub normalized: bool,
    pub integer: bool,
    /// As given (0 = tightly packed).
    pub stride: u32,
    pub offset: u64,
    pub divisor: u32,
}

impl Default for AttribState {
    fn default() -> AttribState {
        AttribState { enabled: false, buffer: 0, size: 4, ty: FLOAT, normalized: false, integer: false, stride: 0, offset: 0, divisor: 0 }
    }
}

pub struct Vao {
    pub attribs: Vec<AttribState>,
    /// The element-array binding while this vertex array is not bound.
    pub element: u32,
    pub bound_once: bool,
}

impl Vao {
    pub fn new() -> Vao {
        Vao { attribs: vec![AttribState::default(); crate::limits::MAX_VERTEX_ATTRIBS as usize], element: 0, bound_once: false }
    }
}

pub struct Sampler {
    pub params: TexParams,
}

pub struct Query {
    pub target: u32,
}

#[derive(Default)]
pub struct Tf {
    pub bound_once: bool,
}

pub fn default_attrib_value() -> ConstValue {
    ConstValue::F([0.0, 0.0, 0.0, 1.0])
}
