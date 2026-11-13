//! The WebGL command contract.
//!
//! Three parties meet here and nowhere else:
//!   * the shader translator (`invisium-glsl`) turns GLSL ES into WGSL and describes what the
//!     program needs ([`TexSlot`], [`AttrSlot`], [`OutSlot`], [`BlockSlot`]);
//!   * the GL state machine (`invisium-gl`) implements WebGL's rules on the CPU and emits
//!     [`Cmd`]s;
//!   * the GPU side (`invisium-gfx`) implements [`GlDevice`] and runs the commands.
//!
//! Like the rest of this crate it is pure data. Changing it is done by the project lead only.
//!
//! # Conventions (everything below is in *device terms*)
//!
//! * **Rows.** A texture or render target stores row 0 first, and row 0 is the row GL calls the
//!   bottom one (`t = 0`, window `y = 0`). Vertex shaders made by the translator negate
//!   clip-space `y`, so GL's bottom row is rendered into row 0, and they map clip-space depth
//!   from GL's `-w..w` to `0..w`. Hence: viewport, scissor, blit and read rectangles are
//!   `[x, y, width, height]` in texels with `y` counted from row 0, exactly as GL passes them;
//!   the presented image of a surface must be shown with row 0 at the *bottom*; and
//!   [`DrawState::front_ccw`] is already swapped by the state machine to make up for the
//!   mirrored winding (hand it to the GPU as it is).
//! * **Ids** are chosen by the state machine, unique per context and kind, never 0; id 0 means
//!   "none". Creating an id that exists replaces the old object.
//! * **Hostile input.** A page controls every command. The device must never panic or abort on
//!   one; a command it cannot run draws nothing.

use std::rc::Rc;

pub type BufferId = u32;
pub type TextureId = u32;
pub type ProgramId = u32;

/// `[x, y, width, height]` in texels, `y` counted from row 0.
pub type Rect4 = [i32; 4];

/// Storage format of a texture (renderbuffers are textures too).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TexFormat {
    R8,
    Rg8,
    Rgba8,
    /// sRGB-encoded colour with linear alpha; sampling and blending convert.
    Srgb8A8,
    R8Snorm,
    Rg8Snorm,
    Rgba8Snorm,
    R16F,
    Rg16F,
    Rgba16F,
    R32F,
    Rg32F,
    Rgba32F,
    R11G11B10F,
    Rgb10A2,
    R8I,
    R8U,
    R16I,
    R16U,
    R32I,
    R32U,
    Rg8I,
    Rg8U,
    Rg16I,
    Rg16U,
    Rg32I,
    Rg32U,
    Rgba8I,
    Rgba8U,
    Rgba16I,
    Rgba16U,
    Rgba32I,
    Rgba32U,
    Depth16,
    Depth24,
    Depth32F,
    Depth24Stencil8,
    Depth32FStencil8,
    Stencil8,
}

impl TexFormat {
    /// Bytes per texel in [`Cmd::TextureWrite`] data and in [`GlDevice::read_pixels`] results
    /// (little-endian components in the order of the name). Depth and stencil formats are never
    /// written from the CPU; reading them is not supported.
    pub fn bytes(self) -> u32 {
        use TexFormat::*;
        match self {
            R8 | R8Snorm | R8I | R8U | Stencil8 => 1,
            Rg8 | Rg8Snorm | Rg8I | Rg8U | R16F | R16I | R16U | Depth16 => 2,
            Rgba8 | Srgb8A8 | Rgba8Snorm | Rgba8I | Rgba8U | Rg16F | Rg16I | Rg16U | R32F | R32I | R32U
            | R11G11B10F | Rgb10A2 | Depth24 | Depth32F | Depth24Stencil8 => 4,
            Rgba16F | Rgba16I | Rgba16U | Rg32F | Rg32I | Rg32U | Depth32FStencil8 => 8,
            Rgba32F | Rgba32I | Rgba32U => 16,
        }
    }
    pub fn has_depth(self) -> bool {
        use TexFormat::*;
        matches!(self, Depth16 | Depth24 | Depth32F | Depth24Stencil8 | Depth32FStencil8)
    }
    pub fn has_stencil(self) -> bool {
        matches!(self, TexFormat::Depth24Stencil8 | TexFormat::Depth32FStencil8 | TexFormat::Stencil8)
    }
    pub fn is_color(self) -> bool {
        !self.has_depth() && !self.has_stencil()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TexKind {
    D2,
    /// Six layers: +X, -X, +Y, -Y, +Z, -Z.
    Cube,
    D3,
    Array,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TexDesc {
    pub kind: TexKind,
    pub format: TexFormat,
    pub width: u32,
    pub height: u32,
    /// Depth of a `D3` texture, layers of an `Array`; 1 otherwise (a cube always has 6 faces).
    pub depth: u32,
    pub mips: u32,
    /// 1, or the sample count of a multisampled render target (`D2`, one mip, never sampled).
    pub samples: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Filter {
    Nearest,
    Linear,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Wrap {
    Repeat,
    Clamp,
    Mirror,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Compare {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

/// How one texture is sampled in one draw.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sampling {
    pub mag: Filter,
    pub min: Filter,
    /// `None`: only the base level is used.
    pub mip: Option<Filter>,
    /// s, t, r.
    pub wrap: [Wrap; 3],
    /// Set for shadow samplers (`TEXTURE_COMPARE_MODE`).
    pub compare: Option<Compare>,
    pub lod_min: f32,
    pub lod_max: f32,
    /// First and last mip level visible to the shader (`TEXTURE_BASE_LEVEL` / `MAX_LEVEL`,
    /// already clamped to the texture's levels).
    pub base_level: u32,
    pub max_level: u32,
}

/// Component class of a vertex input, a fragment output or a sampled texture.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Scalar {
    F32,
    I32,
    U32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SampleKind {
    /// `texture_2d<f32>` and friends. The bound texture may turn out to be unfilterable (32-bit
    /// float) or a depth texture; the device must still draw (nearest sampling is acceptable).
    Float,
    /// `texture_depth_*` with a comparison sampler.
    Shadow,
    Int,
    Uint,
}

/// One texture + sampler pair of a program: bind group 1, bindings `texture` and `sampler`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TexSlot {
    pub texture: u32,
    pub sampler: u32,
    pub kind: TexKind,
    pub sample: SampleKind,
    pub in_vertex: bool,
    pub in_fragment: bool,
}

/// A vertex shader input: `@location(location)` of `vs_main`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct AttrSlot {
    pub location: u32,
    pub base: Scalar,
}

/// A fragment shader output: `@location(location)` of `fs_main`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct OutSlot {
    pub location: u32,
    pub base: Scalar,
}

/// A named uniform block: bind group 0, binding `binding` (1 or higher), `bytes` long.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BlockSlot {
    pub binding: u32,
    pub bytes: u32,
}

/// A linked program as the GPU sees it. Entry points are `vs_main` and `fs_main`.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ProgramDesc {
    pub vertex_wgsl: String,
    pub fragment_wgsl: String,
    /// Size of the default uniform block (bind group 0, binding 0, visible to both stages);
    /// 0 if the program has none. Always a multiple of 16.
    pub uniform_bytes: u32,
    pub blocks: Vec<BlockSlot>,
    pub textures: Vec<TexSlot>,
    pub attributes: Vec<AttrSlot>,
    pub outputs: Vec<OutSlot>,
    /// The fragment shader writes `frag_depth`.
    pub writes_depth: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AttrType {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F16,
    /// `INT_2_10_10_10_REV` / `UNSIGNED_INT_2_10_10_10_REV`.
    I1010102,
    U1010102,
}

/// A vertex attribute read from a buffer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct VertexAttr {
    pub location: u32,
    pub buffer: BufferId,
    /// Byte offset of the first element.
    pub offset: u32,
    /// Bytes between elements; never 0.
    pub stride: u32,
    pub ty: AttrType,
    /// Components per element, 1..=4. Missing components read as (0, 0, 0, 1).
    pub size: u8,
    /// Integer data mapped to 0..1 / -1..1 (otherwise converted to float as it is).
    pub normalized: bool,
    /// The shader input is an integer (`vertexAttribIPointer`); `normalized` is then false.
    pub integer: bool,
    /// 0 = per vertex; n = advances once every n instances.
    pub divisor: u32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ConstValue {
    F([f32; 4]),
    I([i32; 4]),
    U([u32; 4]),
}

/// A vertex attribute with one value for every vertex (its array is disabled).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ConstAttr {
    pub location: u32,
    pub value: ConstValue,
}

/// `LINE_LOOP` and `TRIANGLE_FAN` never reach the device: the state machine rewrites them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Topology {
    Points,
    Lines,
    LineStrip,
    Triangles,
    TriangleStrip,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BlendFactor {
    Zero,
    One,
    SrcColor,
    OneMinusSrcColor,
    DstColor,
    OneMinusDstColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    ConstantColor,
    OneMinusConstantColor,
    ConstantAlpha,
    OneMinusConstantAlpha,
    SrcAlphaSaturate,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BlendOp {
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Blend {
    pub color_op: BlendOp,
    pub alpha_op: BlendOp,
    pub src_color: BlendFactor,
    pub dst_color: BlendFactor,
    pub src_alpha: BlendFactor,
    pub dst_alpha: BlendFactor,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StencilOp {
    Keep,
    Zero,
    Replace,
    Incr,
    Decr,
    Invert,
    IncrWrap,
    DecrWrap,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StencilFace {
    pub func: Compare,
    pub fail: StencilOp,
    pub depth_fail: StencilOp,
    pub pass: StencilOp,
}

/// WebGL requires the reference and the masks to be the same for both faces.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Stencil {
    pub front: StencilFace,
    pub back: StencilFace,
    pub reference: u32,
    pub read_mask: u32,
    pub write_mask: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Face {
    Front,
    Back,
}

/// Everything about a draw that is not geometry or bindings.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DrawState {
    pub topology: Topology,
    pub cull: Option<Face>,
    /// Device terms (see the module comment).
    pub front_ccw: bool,
    /// Applies to every colour target that can blend; integer targets never blend.
    pub blend: Option<Blend>,
    pub blend_color: [f32; 4],
    pub color_mask: [bool; 4],
    pub depth_test: Option<Compare>,
    pub depth_write: bool,
    /// `polygonOffset(factor, units)` while enabled.
    pub depth_bias: Option<(f32, f32)>,
    pub stencil: Option<Stencil>,
    pub alpha_to_coverage: bool,
    /// May reach outside the target; the device clips it (drawing inside the target stays
    /// where GL would put it as far as the GPU API allows).
    pub viewport: Rect4,
    /// `depthRange(near, far)`, both in 0..=1.
    pub depth_range: [f32; 2],
    /// Already intersected with nothing: clamp it to the target.
    pub scissor: Option<Rect4>,
}

/// One image of a texture used as a render target.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Attachment {
    pub texture: TextureId,
    pub level: u32,
    /// Cube face, array layer or 3D slice; 0 otherwise.
    pub layer: u32,
}

/// Where drawing goes (and where reads and blits come from).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Target {
    /// The context's drawing buffer (see [`Cmd::Configure`]).
    Surface,
    /// A complete framebuffer object. `color[i]` is draw buffer `i` (`None` = not drawn to).
    /// All attachments have the same size and sample count.
    Fbo { color: Vec<Option<Attachment>>, depth_stencil: Option<Attachment> },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TexBind {
    /// 0 = nothing usable is bound: samples as transparent black.
    pub texture: TextureId,
    pub sampling: Sampling,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BlockBind {
    pub binding: u32,
    pub buffer: BufferId,
    pub offset: u32,
    pub size: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct IndexSource {
    pub buffer: BufferId,
    /// Byte offset of index 0 in the buffer. `Draw::first` counts indices from there (both
    /// apply).
    pub offset: u32,
    /// 32-bit indices; otherwise 16-bit. (8-bit index data is widened by the state machine.)
    pub wide: bool,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Draw {
    pub target: Target,
    pub state: DrawState,
    pub program: ProgramId,
    /// Enabled attribute arrays the program uses. Reads are inside their buffers for every
    /// vertex and instance of this draw (the state machine has checked).
    pub attrs: Vec<VertexAttr>,
    /// Program inputs whose array is disabled.
    pub consts: Vec<ConstAttr>,
    pub index: Option<IndexSource>,
    /// First vertex (non-indexed) or first index.
    pub first: u32,
    pub count: u32,
    pub instances: u32,
    /// Contents of the default uniform block, exactly `ProgramDesc::uniform_bytes` long. Draws
    /// whose uniforms did not change share one allocation (compare with `Rc::ptr_eq`).
    pub uniforms: Rc<[u8]>,
    pub blocks: Vec<BlockBind>,
    /// Parallel to `ProgramDesc::textures`.
    pub textures: Vec<TexBind>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ClearColor {
    F([f32; 4]),
    I([i32; 4]),
    U([u32; 4]),
}

#[derive(Clone, PartialEq, Debug)]
pub enum Cmd {
    /// (Re)create the drawing buffer; its contents start as transparent black (alpha 1 when
    /// `alpha` is false), depth 1, stencil 0. Sent once before anything else and again when the
    /// canvas is resized. Without `alpha` the surface's alpha always reads as 1; the state
    /// machine never lets a draw or clear write it. `samples` is 1 or 4.
    Configure {
        width: u32,
        height: u32,
        alpha: bool,
        depth: bool,
        stencil: bool,
        samples: u32,
        /// Colour values are already multiplied by alpha (WebGL's default).
        premultiplied: bool,
        /// Keep the drawing buffer after `Present` instead of clearing it.
        preserve: bool,
    },
    /// The drawing buffer (resolved if multisampled) becomes the image shown for this context.
    /// Unless `preserve` was set, the drawing buffer is then cleared as in `Configure`.
    Present,

    /// Create a buffer holding `bytes`. `index` buffers are only ever used as index sources.
    BufferCreate { id: BufferId, index: bool, bytes: Vec<u8> },
    BufferWrite { id: BufferId, offset: u32, bytes: Vec<u8> },
    /// `copyBufferSubData`.
    BufferCopy { src: BufferId, src_offset: u32, dst: BufferId, dst_offset: u32, size: u32 },
    BufferDelete { id: BufferId },

    /// Create a texture; every texel starts as zero.
    TextureCreate { id: TextureId, desc: TexDesc },
    /// Give the texture `mips` levels (more than it has); existing levels keep their contents.
    TextureGrow { id: TextureId, mips: u32 },
    /// Tightly packed rows in the texture's format, row 0 first. `layer` is the cube face, array
    /// layer or first 3D slice; `depth` slices follow each other (1 unless `D3`/`Array`).
    TextureWrite { id: TextureId, level: u32, layer: u32, x: u32, y: u32, width: u32, height: u32, depth: u32, bytes: Vec<u8> },
    /// Fill levels 1.. from level 0 (box or linear filter).
    GenerateMips { id: TextureId },
    /// `copyTexSubImage2D`: colour buffer `src_color` of `src` into one image of a texture.
    /// The rectangle is inside both.
    CopyToTexture { src: Target, src_color: u32, src_x: u32, src_y: u32, dst: Attachment, dst_x: u32, dst_y: u32, width: u32, height: u32 },
    TextureDelete { id: TextureId },

    ProgramCreate { id: ProgramId, desc: Box<ProgramDesc> },
    ProgramDelete { id: ProgramId },

    /// Clear parts of `target`. `colors` lists (draw buffer, value) pairs; `color_mask` and
    /// `stencil_mask` limit what is written; `scissor` limits where (clamp it to the target).
    Clear {
        target: Target,
        colors: Vec<(u32, ClearColor)>,
        depth: Option<f32>,
        stencil: Option<u32>,
        color_mask: [bool; 4],
        stencil_mask: u32,
        scissor: Option<Rect4>,
    },
    Draw(Box<Draw>),
    /// `blitFramebuffer`. Rectangles may be flipped (negative width or height) and may reach
    /// outside their target (clip, keeping the scale). Multisampled sources are resolved.
    Blit {
        src: Target,
        /// Read buffer index of `src`.
        src_color: u32,
        src_rect: Rect4,
        dst: Target,
        dst_rect: Rect4,
        color: bool,
        depth: bool,
        stencil: bool,
        linear: bool,
    },
}

#[derive(Clone, Debug, Default)]
pub struct GlStats {
    pub contexts: u32,
    /// GPU bytes of buffers plus any CPU copies the device keeps of them.
    pub buffer_bytes: usize,
    pub texture_bytes: usize,
    pub programs: u32,
    pub pipelines: u32,
}

/// The GPU side of WebGL. One device serves every context of every page.
pub trait GlDevice {
    /// A new context, or `None` if this machine's GPU cannot provide what the contract needs.
    fn create(&mut self) -> Option<u32>;
    /// Free everything the context owns. Its image draws nothing afterwards.
    fn destroy(&mut self, ctx: u32);
    /// The image under which the renderer draws what the context last presented (stable for
    /// the context's life; draws nothing before the first `Present`). Used in `Item::Image`.
    fn image(&self, ctx: u32) -> super::ImageId;
    /// Run the commands in order and leave `cmds` empty.
    fn run(&mut self, ctx: u32, cmds: &mut Vec<Cmd>);
    /// Read `rect` (inside the target) of colour buffer `color` of `target`: its format and
    /// tightly packed rows, row 0 first. Multisampled sources are resolved. `None` if it cannot
    /// be read.
    fn read_pixels(&mut self, ctx: u32, target: &Target, color: u32, rect: Rect4) -> Option<(TexFormat, Vec<u8>)>;
    /// Read `len` bytes of a buffer starting at byte `offset` (the range is inside the buffer),
    /// as they are after every command run so far. `None` if it cannot be read.
    fn read_buffer(&mut self, ctx: u32, buffer: BufferId, offset: u32, len: u32) -> Option<Vec<u8>>;
    fn stats(&self) -> GlStats;
}
