//! WebGL state machine: everything WebGL 1 and 2 define that can be decided on the CPU
//! (objects, state, validation, errors, queries, pixel conversion), emitting the GPU commands
//! of `invisium_draw::gl` for the rest.
//!
//! The public items in this file are the contract with the engine's script bindings; see
//! `docs/agents/GL.md`.

pub use invisium_draw::gl as cmd;
use invisium_draw::gl::GlDevice;
use std::collections::HashMap;

mod args;
mod buffers;
mod clear;
#[allow(dead_code)]
mod consts1;
#[allow(dead_code, non_upper_case_globals)]
mod consts2;
mod dispatch;
mod draw;
mod drawstate;
mod fbo;
mod fbquery;
mod formats;
mod inert;
mod introspect;
#[allow(dead_code)]
mod limits;
mod objects;
mod params;
mod pixels;
mod progobj;
mod programs;
mod readback;
mod renderbuffers;
mod setters;
mod state;
mod tables;
mod teximage;
mod textures;
mod uniforms;
mod upload;
mod vertex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Version {
    WebGl1,
    WebGl2,
}

/// `WebGLContextAttributes`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Attributes {
    pub alpha: bool,
    pub depth: bool,
    pub stencil: bool,
    pub antialias: bool,
    pub premultiplied_alpha: bool,
    pub preserve_drawing_buffer: bool,
}

impl Default for Attributes {
    fn default() -> Attributes {
        Attributes { alpha: true, depth: true, stencil: false, antialias: true, premultiplied_alpha: true, preserve_drawing_buffer: false }
    }
}

/// Element type of a typed array handed to a call.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Elem {
    I8,
    U8,
    U8Clamped,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
    /// An `ArrayBuffer` or a `DataView`: bytes with no element type.
    Raw,
}

/// The kinds of WebGL objects scripts hold.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Buffer,
    Texture,
    Renderbuffer,
    Framebuffer,
    Shader,
    Program,
    UniformLocation,
    VertexArray,
    Sampler,
    Query,
    Sync,
    TransformFeedback,
}

/// One argument of a WebGL call, already taken out of the script engine.
#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    /// `undefined`, or an argument that was not passed.
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(&'a str),
    /// A typed array, `DataView` or `ArrayBuffer`: the bytes its view covers.
    Bytes(Elem, &'a [u8]),
    /// A plain array of numbers (`uniform3fv(loc, [1, 2, 3])`).
    Nums(&'a [f64]),
    /// A WebGL object by handle.
    Object(Kind, u32),
    /// Pixels of an image, canvas, video frame or `ImageData`: straight-alpha RGBA8, top row
    /// first.
    Pixels { width: u32, height: u32, rgba: &'a [u8] },
}

/// The result of a WebGL call.
#[derive(Clone, Debug, PartialEq)]
pub enum Ret {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    /// A WebGL object. The same handle must map to the same script object every time.
    Object(Kind, u32),
    /// A `Float32Array`.
    F32s(Vec<f32>),
    /// An `Int32Array`.
    I32s(Vec<i32>),
    /// A `Uint32Array`.
    U32s(Vec<u32>),
    /// A plain array of booleans.
    Bools(Vec<bool>),
    /// A plain array of strings.
    Strs(Vec<String>),
    /// A plain array of WebGL objects.
    Objects(Kind, Vec<u32>),
    /// A `WebGLActiveInfo`.
    ActiveInfo { name: String, size: i32, gl_type: u32 },
    /// A `WebGLShaderPrecisionFormat`.
    Precision { range_min: i32, range_max: i32, precision: i32 },
    /// A `WebGLContextAttributes` dictionary.
    Attributes(Attributes),
    /// `getExtension` succeeded: the binding builds the extension object from
    /// [`extension`]`(name)`.
    Extension(&'static str),
    /// Copy `bytes` into the view passed as argument number `arg`, starting `offset` bytes into
    /// that view (`readPixels`, `getBufferSubData`). The call itself returns `undefined`.
    Fill { arg: usize, offset: usize, bytes: Vec<u8> },
}

/// What an extension object carries.
#[derive(Clone, Copy, Debug)]
pub struct ExtensionInfo {
    pub constants: &'static [(&'static str, u32)],
    /// Method name on the extension object and the op to call on the context.
    pub methods: &'static [(&'static str, u16)],
}

/// One WebGL context.
pub struct Context {
    id: u32,
    version: Version,
    size: (u32, u32),
    attrs: Attributes,
    st: state::State,
    objs: objects::Objects,
    cmds: Vec<cmd::Cmd>,
    pending: usize,
    error: u32,
    lost: bool,
    lost_pending: bool,
    drawn: bool,
    exts: Vec<&'static str>,
    next_handle: u32,
    dev_bytes: u64,
    dev_sizes: HashMap<u32, u64>,
    /// Device index buffer for rewritten topologies and widened 8-bit indices (0 = none yet).
    scratch: u32,
}

impl Context {
    /// `id` is what `GlDevice::create` returned. Queues `Cmd::Configure`.
    pub fn new(id: u32, version: Version, attrs: Attributes, width: u32, height: u32) -> Context {
        let mut c = Context {
            id,
            version,
            size: (width, height),
            attrs,
            st: state::State::new(width, height),
            objs: objects::Objects::default(),
            cmds: Vec::new(),
            pending: 0,
            error: 0,
            lost: false,
            lost_pending: false,
            drawn: false,
            exts: Vec::new(),
            next_handle: 1,
            dev_bytes: 0,
            dev_sizes: HashMap::new(),
            scratch: 0,
        };
        c.objs.vaos.insert(0, progobj::Vao::new());
        c.configure();
        c
    }
    pub fn id(&self) -> u32 {
        self.id
    }
    pub fn version(&self) -> Version {
        self.version
    }
    /// The canvas's `width` / `height` changed: a new, cleared drawing buffer.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.size = (width, height);
        self.configure();
    }
    pub fn drawing_buffer_size(&self) -> (u32, u32) {
        self.size
    }
    /// Run one WebGL method; `op` comes from [`method_op`]. Never panics, whatever it is given:
    /// wrong arguments set a GL error (or return `Ret::Null`) as WebGL says.
    pub fn call(&mut self, dev: &mut dyn GlDevice, op: u16, args: &[Arg]) -> Ret {
        let ret = self.dispatch(dev, op, args);
        if self.pending > limits::FLUSH_BYTES {
            self.flush(dev);
        }
        ret
    }
    /// End of a frame. Sends every pending command to the device; if the drawing buffer was
    /// drawn to since the last present it is presented first and `true` is returned (the canvas
    /// needs repainting).
    pub fn present(&mut self, dev: &mut dyn GlDevice) -> bool {
        let drawn = self.drawn;
        if drawn {
            self.push(cmd::Cmd::Present);
            self.drawn = false;
        }
        self.flush(dev);
        drawn
    }
    /// Send pending commands now.
    pub fn flush(&mut self, dev: &mut dyn GlDevice) {
        if !self.cmds.is_empty() {
            dev.run(self.id, &mut self.cmds);
            self.cmds.clear();
        }
        self.pending = 0;
    }
    /// The canvas is gone: free the context's GPU objects.
    pub fn destroy(&mut self, dev: &mut dyn GlDevice) {
        self.cmds.clear();
        self.pending = 0;
        dev.destroy(self.id);
    }
    /// Bytes this context holds on the CPU side.
    pub fn memory_bytes(&self) -> usize {
        let buffers: usize = self.objs.buffers.values().filter_map(|b| b.data.as_ref()).map(|d| d.len()).sum();
        buffers + self.pending
    }

    fn configure(&mut self) {
        let a = self.attrs;
        self.push(cmd::Cmd::Configure {
            width: self.size.0,
            height: self.size.1,
            alpha: a.alpha,
            depth: a.depth,
            stencil: a.stencil,
            samples: if a.antialias { 4 } else { 1 },
            premultiplied: a.premultiplied_alpha,
            preserve: a.preserve_drawing_buffer,
        });
    }

    fn push(&mut self, c: cmd::Cmd) {
        self.pending += 64
            + match &c {
                cmd::Cmd::BufferCreate { bytes, .. } | cmd::Cmd::BufferWrite { bytes, .. } | cmd::Cmd::TextureWrite { bytes, .. } => bytes.len(),
                _ => 0,
            };
        self.cmds.push(c);
    }

    /// Record a GL error; the first one stays until `getError` reads it.
    fn set_error(&mut self, e: u32) {
        if self.error == 0 {
            self.error = e;
        }
    }

    fn new_handle(&mut self) -> u32 {
        let h = self.next_handle;
        self.next_handle = self.next_handle.saturating_add(1);
        h
    }

    fn is2(&self) -> bool {
        self.version == Version::WebGl2
    }

    fn ext(&self, name: &str) -> bool {
        self.exts.contains(&name)
    }

    /// Account for a device object of `bytes` under `id` (replacing what it had), or fail with
    /// `OUT_OF_MEMORY` and change nothing.
    fn reserve(&mut self, id: u32, bytes: Option<u64>) -> args::R<()> {
        let bytes = bytes.ok_or(consts1::OUT_OF_MEMORY)?;
        let old = self.dev_sizes.get(&id).copied().unwrap_or(0);
        let total = (self.dev_bytes - old).checked_add(bytes).ok_or(consts1::OUT_OF_MEMORY)?;
        if total > limits::MEMORY_BUDGET {
            return Err(consts1::OUT_OF_MEMORY);
        }
        self.dev_bytes = total;
        self.dev_sizes.insert(id, bytes);
        Ok(())
    }

    fn release(&mut self, id: u32) {
        if let Some(b) = self.dev_sizes.remove(&id) {
            self.dev_bytes -= b;
        }
    }
}

/// The op number of a method of `WebGLRenderingContext` / `WebGL2RenderingContext`.
pub fn method_op(version: Version, name: &str) -> Option<u16> {
    methods(version).iter().find(|m| m.0 == name).map(|m| m.1)
}

/// Every method of the context interface, with its op.
pub fn methods(version: Version) -> &'static [(&'static str, u16)] {
    tables::methods(version)
}

/// Every constant of the context interface (`DEPTH_TEST`, `TEXTURE_2D`, ...).
pub fn constants(version: Version) -> &'static [(&'static str, u32)] {
    tables::constants(version)
}

/// The extension called `name`, if this implementation offers it for `version`.
pub fn extension(version: Version, name: &str) -> Option<ExtensionInfo> {
    tables::extension(version, name)
}
