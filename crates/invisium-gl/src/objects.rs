//! WebGL objects as the state machine keeps them.

use crate::cmd::{TexDesc, TexFormat, TexKind};
use crate::consts1::*;
use crate::consts2::*;
use std::collections::HashMap;

/// Mip levels a texture can have (8192 = 2^13).
pub const MAX_LEVELS: usize = 14;

pub struct Buffer {
    pub size: usize,
    pub usage: u32,
    /// WebGL's buffer type: `Some(true)` element array, `Some(false)` other data.
    pub elem: Option<bool>,
    /// CPU copy (element-array buffers, and every buffer of a WebGL 2 context).
    pub data: Option<Vec<u8>>,
    pub on_dev: bool,
    /// The last index scan of a draw: ((offset, count, type), highest index). Cleared
    /// whenever `data` changes.
    pub scan: Option<((usize, usize, u32), Option<u32>)>,
}

impl Buffer {
    /// The CPU copy for writing; forgets the cached index scan.
    pub fn data_mut(&mut self) -> Option<&mut Vec<u8>> {
        self.scan = None;
        self.data.as_mut()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub d: u32,
    /// The internal format as given (sized, or unsized in WebGL 1 style).
    pub internal: u32,
    pub format: u32,
    pub ty: u32,
    pub fmt: TexFormat,
    /// Its data lives in the device texture (it fits the mip chain of level 0).
    pub on_dev: bool,
}

#[derive(Clone, Copy)]
pub struct TexParams {
    pub min_filter: u32,
    pub mag_filter: u32,
    pub wrap: [u32; 3],
    pub base_level: i32,
    pub max_level: i32,
    pub min_lod: f32,
    pub max_lod: f32,
    pub compare_mode: u32,
    pub compare_func: u32,
}

impl Default for TexParams {
    fn default() -> TexParams {
        TexParams {
            min_filter: NEAREST_MIPMAP_LINEAR,
            mag_filter: LINEAR,
            wrap: [REPEAT; 3],
            base_level: 0,
            max_level: 1000,
            min_lod: -1000.0,
            max_lod: 1000.0,
            compare_mode: NONE,
            compare_func: LEQUAL,
        }
    }
}

pub struct Texture {
    pub kind: Option<TexKind>,
    pub immutable: bool,
    pub imm_levels: u32,
    /// `level * 6 + face`.
    pub images: Vec<Option<Image>>,
    pub dev: Option<TexDesc>,
    pub params: TexParams,
}

impl Texture {
    pub fn new() -> Texture {
        Texture { kind: None, immutable: false, imm_levels: 0, images: vec![None; MAX_LEVELS * 6], dev: None, params: TexParams::default() }
    }
    pub fn image(&self, level: usize, face: usize) -> Option<&Image> {
        self.images.get(level * 6 + face).and_then(|i| i.as_ref())
    }
    pub fn faces(&self) -> usize {
        if self.kind == Some(TexKind::Cube) {
            6
        } else {
            1
        }
    }
}

pub struct Renderbuffer {
    pub internal: u32,
    pub fmt: Option<TexFormat>,
    pub w: u32,
    pub h: u32,
    pub samples: u32,
    pub bound_once: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Att {
    /// `face` is the cube face index (0..6) or 0; `layer` the array layer / 3D slice.
    Tex { id: u32, level: u32, face: u32, layer: u32 },
    Rb(u32),
}

pub struct Framebuffer {
    pub color: [Option<Att>; 8],
    pub depth: Option<Att>,
    pub stencil: Option<Att>,
    /// WebGL 1 only: the separate `DEPTH_STENCIL_ATTACHMENT` point.
    pub depth_stencil: Option<Att>,
    pub draw_buffers: [u32; 8],
    pub read_buffer: u32,
    pub bound_once: bool,
}

impl Framebuffer {
    pub fn new() -> Framebuffer {
        let mut draw_buffers = [NONE; 8];
        draw_buffers[0] = COLOR_ATTACHMENT0;
        Framebuffer { color: [None; 8], depth: None, stencil: None, depth_stencil: None, draw_buffers, read_buffer: COLOR_ATTACHMENT0, bound_once: false }
    }
    pub fn all(&self) -> impl Iterator<Item = Att> + '_ {
        self.color.iter().chain([&self.depth, &self.stencil, &self.depth_stencil]).filter_map(|a| *a)
    }
    pub fn detach(&mut self, what: impl Fn(&Att) -> bool) {
        for slot in self.color.iter_mut().chain([&mut self.depth, &mut self.stencil, &mut self.depth_stencil]) {
            if slot.as_ref().map(&what).unwrap_or(false) {
                *slot = None;
            }
        }
    }
}

/// Every object of a context, keyed by handle.
#[derive(Default)]
pub struct Objects {
    pub buffers: HashMap<u32, Buffer>,
    pub textures: HashMap<u32, Texture>,
    pub renderbuffers: HashMap<u32, Renderbuffer>,
    pub framebuffers: HashMap<u32, Framebuffer>,
    pub shaders: HashMap<u32, crate::progobj::Shader>,
    pub programs: HashMap<u32, crate::progobj::Program>,
    pub locations: HashMap<u32, crate::progobj::Loc>,
    pub loc_cache: HashMap<crate::progobj::Loc, u32>,
    /// Key 0 is the default vertex array.
    pub vaos: HashMap<u32, crate::progobj::Vao>,
    pub samplers: HashMap<u32, crate::progobj::Sampler>,
    pub queries: HashMap<u32, crate::progobj::Query>,
    pub syncs: std::collections::HashSet<u32>,
    pub tfs: HashMap<u32, crate::progobj::Tf>,
}

/// Bytes of a device texture with all its levels, overflow-safe.
pub fn tex_bytes(desc: &TexDesc) -> Option<u64> {
    let layers: u64 = if desc.kind == TexKind::Cube { 6 } else { 1 };
    let mut total: u64 = 0;
    for l in 0..desc.mips.min(32) {
        let w = (desc.width >> l).max(1) as u64;
        let h = (desc.height >> l).max(1) as u64;
        let d = if desc.kind == TexKind::D3 { (desc.depth >> l).max(1) as u64 } else { desc.depth.max(1) as u64 };
        let level = w.checked_mul(h)?.checked_mul(d)?.checked_mul(desc.format.bytes() as u64)?.checked_mul(layers)?;
        total = total.checked_add(level)?;
    }
    total.checked_mul(desc.samples.max(1) as u64)
}

/// Number of levels of a full mip chain for these dimensions.
pub fn full_levels(w: u32, h: u32, d: u32) -> u32 {
    32 - w.max(h).max(d).max(1).leading_zeros()
}

/// Cube face index of a `TEXTURE_CUBE_MAP_*` target, 0 for other targets.
pub fn face_index(target: u32) -> u32 {
    if (TEXTURE_CUBE_MAP_POSITIVE_X..=TEXTURE_CUBE_MAP_NEGATIVE_Z).contains(&target) {
        target - TEXTURE_CUBE_MAP_POSITIVE_X
    } else {
        0
    }
}

/// The bind target of a texture image target.
pub fn bind_target(target: u32) -> u32 {
    if (TEXTURE_CUBE_MAP_POSITIVE_X..=TEXTURE_CUBE_MAP_NEGATIVE_Z).contains(&target) {
        TEXTURE_CUBE_MAP
    } else {
        target
    }
}

pub fn kind_of(target: u32) -> Option<TexKind> {
    match target {
        TEXTURE_2D => Some(TexKind::D2),
        TEXTURE_CUBE_MAP => Some(TexKind::Cube),
        TEXTURE_3D => Some(TexKind::D3),
        TEXTURE_2D_ARRAY => Some(TexKind::Array),
        _ => None,
    }
}
