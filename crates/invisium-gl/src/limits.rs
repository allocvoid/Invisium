//! Fixed implementation limits: the same on every machine (privacy, see the charter).

pub const MAX_TEXTURE_SIZE: u32 = 8192;
pub const MAX_CUBE_MAP_TEXTURE_SIZE: u32 = 8192;
pub const MAX_RENDERBUFFER_SIZE: u32 = 8192;
pub const MAX_VIEWPORT_DIM: i32 = 8192;
pub const MAX_3D_TEXTURE_SIZE: u32 = 2048;
pub const MAX_ARRAY_TEXTURE_LAYERS: u32 = 256;
pub const MAX_VERTEX_ATTRIBS: u32 = 16;
pub const MAX_TEXTURE_IMAGE_UNITS: u32 = 16;
pub const MAX_VERTEX_TEXTURE_IMAGE_UNITS: u32 = 16;
pub const MAX_COMBINED_TEXTURE_IMAGE_UNITS: u32 = 32;
pub const MAX_VERTEX_UNIFORM_VECTORS: u32 = 1024;
pub const MAX_FRAGMENT_UNIFORM_VECTORS: u32 = 1024;
pub const MAX_VARYING_VECTORS: u32 = 30;
pub const MAX_DRAW_BUFFERS: u32 = 8;
pub const MAX_COLOR_ATTACHMENTS: u32 = 8;
pub const MAX_SAMPLES: u32 = 4;
pub const MAX_UNIFORM_BUFFER_BINDINGS: u32 = 24;
pub const MAX_UNIFORM_BLOCK_SIZE: u32 = 65536;
pub const UNIFORM_BUFFER_OFFSET_ALIGNMENT: u32 = 256;
pub const MAX_TEXTURE_LOD_BIAS: f32 = 2.0;
pub const SUBPIXEL_BITS: u32 = 4;
pub const MAX_VERTEX_UNIFORM_BLOCKS: u32 = 12;
pub const MAX_FRAGMENT_UNIFORM_BLOCKS: u32 = 12;
pub const MAX_COMBINED_UNIFORM_BLOCKS: u32 = 24;
pub const MAX_TRANSFORM_FEEDBACK_SEPARATE_ATTRIBS: u32 = 4;

/// The memory budget of one context's device objects.
pub const MEMORY_BUDGET: u64 = 512 * 1024 * 1024;
/// Pending command data that triggers a flush.
pub const FLUSH_BYTES: usize = 4 * 1024 * 1024;
