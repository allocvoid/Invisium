//! Milestone 4 acceptance tests: command lists and WGSL written by hand, headless renderer.

mod followup;
mod geometry;
mod robust;
mod state;
mod surface;
mod targets;
mod textures;

pub(super) use crate::draw::gl::*;
pub(super) use std::rc::Rc;
use crate::{GpuOptions, Renderer};
use std::cell::RefCell;
use std::sync::{Mutex, MutexGuard};

static GPU: Mutex<()> = Mutex::new(());

pub(super) struct H {
    pub r: Renderer,
    pub dev: Rc<RefCell<dyn GlDevice>>,
    pub ctx: u32,
    _lock: MutexGuard<'static, ()>,
}

/// A headless renderer of `w` x `h` and one context, or `None` (skip) without a usable GPU.
pub(super) fn h(w: u32, h: u32) -> Option<H> {
    let lock = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let r = match pollster::block_on(Renderer::new_headless(w, h, GpuOptions::default())) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("SKIP: {e}");
            return None;
        }
    };
    let dev = r.gl_device();
    let Some(ctx) = dev.borrow_mut().create() else {
        eprintln!("SKIP: the adapter cannot run WebGL contexts");
        return None;
    };
    Some(H { r, dev, ctx, _lock: lock })
}

impl H {
    pub fn run(&self, mut cmds: Vec<Cmd>) {
        self.dev.borrow_mut().run(self.ctx, &mut cmds);
        assert!(cmds.is_empty());
    }

    pub fn read(&self, t: &Target, color: u32, rect: Rect4) -> Option<(TexFormat, Vec<u8>)> {
        self.dev.borrow_mut().read_pixels(self.ctx, t, color, rect)
    }

    /// One RGBA8 pixel of colour buffer 0 of `t`.
    pub fn px(&self, t: &Target, x: i32, y: i32) -> [u8; 4] {
        let (_, v) = self.read(t, 0, [x, y, 1, 1]).expect("read_pixels");
        [v[0], v[1], v[2], v[3]]
    }

    pub fn surf(&self, x: i32, y: i32) -> [u8; 4] {
        self.px(&Target::Surface, x, y)
    }

    pub fn passes(&self) -> u64 {
        self.r.gl_inner().borrow().gpu.passes
    }

    pub fn stats(&self) -> GlStats {
        self.dev.borrow().stats()
    }
}

pub(super) fn near(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tol)
}

#[track_caller]
pub(super) fn assert_near(a: [u8; 4], b: [u8; 4], tol: u8) {
    assert!(near(a, b, tol), "got {a:?}, want {b:?} (±{tol})");
}

pub(super) fn cfg(w: u32, h: u32) -> Cmd {
    Cmd::Configure { width: w, height: h, alpha: true, depth: true, stencil: true, samples: 1, premultiplied: true, preserve: false }
}

pub(super) fn st(w: u32, h: u32) -> DrawState {
    DrawState {
        topology: Topology::Triangles,
        cull: None,
        front_ccw: true,
        blend: None,
        blend_color: [0.0; 4],
        color_mask: [true; 4],
        depth_test: None,
        depth_write: false,
        depth_bias: None,
        stencil: None,
        alpha_to_coverage: false,
        viewport: [0, 0, w as i32, h as i32],
        depth_range: [0.0, 1.0],
        scissor: None,
    }
}

/// Position (location 0, GL clip space) with y negated as the translator does.
pub(super) const VS: &str = "
@vertex fn vs_main(@location(0) p: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(p.x, -p.y, 0.0, 1.0);
}";

/// Position with a z coordinate (location 0, vec3).
pub(super) const VS3: &str = "
@vertex fn vs_main(@location(0) p: vec3<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(p.x, -p.y, p.z, 1.0);
}";

/// Colour from the default uniform block.
pub(super) const FS_UNI: &str = "
struct U { c: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;
@fragment fn fs_main() -> @location(0) vec4<f32> { return u.c; }";

pub(super) fn desc(vs: &str, fs: &str, uniform_bytes: u32, attrs: &[(u32, Scalar)]) -> ProgramDesc {
    ProgramDesc {
        vertex_wgsl: vs.into(),
        fragment_wgsl: fs.into(),
        uniform_bytes,
        attributes: attrs.iter().map(|&(location, base)| AttrSlot { location, base }).collect(),
        outputs: vec![OutSlot { location: 0, base: Scalar::F32 }],
        ..Default::default()
    }
}

pub(super) fn program(id: u32, d: ProgramDesc) -> Cmd {
    Cmd::ProgramCreate { id, desc: Box::new(d) }
}

/// The standard program 1: `VS` + `FS_UNI`.
pub(super) fn basic_program() -> Cmd {
    program(1, desc(VS, FS_UNI, 16, &[(0, Scalar::F32)]))
}

pub(super) fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub(super) fn buffer(id: u32, bytes: Vec<u8>) -> Cmd {
    Cmd::BufferCreate { id, index: false, bytes }
}

/// Two triangles covering clip space.
pub(super) fn quad() -> Vec<u8> {
    f32s(&[-1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, 1.0])
}

pub(super) fn attr(location: u32, buffer: u32, ty: AttrType, size: u8, stride: u32, offset: u32) -> VertexAttr {
    VertexAttr { location, buffer, offset, stride, ty, size, normalized: false, integer: false, divisor: 0 }
}

pub(super) fn uni(c: [f32; 4]) -> Rc<[u8]> {
    f32s(&c).into()
}

pub(super) fn draw(target: Target, program: u32, state: DrawState, attrs: Vec<VertexAttr>, count: u32, uniforms: Rc<[u8]>) -> Draw {
    Draw {
        target,
        state,
        program,
        attrs,
        consts: vec![],
        index: None,
        first: 0,
        count,
        instances: 1,
        uniforms,
        blocks: vec![],
        textures: vec![],
    }
}

/// Program 1 drawing the full-screen quad from buffer 1 in colour `c`.
pub(super) fn quad_draw(target: Target, state: DrawState, c: [f32; 4]) -> Cmd {
    Cmd::Draw(Box::new(draw(target, 1, state, vec![attr(0, 1, AttrType::F32, 2, 8, 0)], 6, uni(c))))
}

pub(super) fn clear(target: Target, c: [f32; 4]) -> Cmd {
    Cmd::Clear {
        target,
        colors: vec![(0, ClearColor::F(c))],
        depth: Some(1.0),
        stencil: Some(0),
        color_mask: [true; 4],
        stencil_mask: 0xff,
        scissor: None,
    }
}

pub(super) fn tex2d(format: TexFormat, w: u32, h: u32, mips: u32) -> TexDesc {
    TexDesc { kind: TexKind::D2, format, width: w, height: h, depth: 1, mips, samples: 1 }
}

pub(super) fn fbo(tex: u32) -> Target {
    Target::Fbo { color: vec![Some(Attachment { texture: tex, level: 0, layer: 0 })], depth_stencil: None }
}

pub(super) fn sampling(f: Filter, wrap: Wrap) -> Sampling {
    Sampling { mag: f, min: f, mip: None, wrap: [wrap; 3], compare: None, lod_min: 0.0, lod_max: 1000.0, base_level: 0, max_level: 1000 }
}
