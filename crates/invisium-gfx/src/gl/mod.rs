//! The WebGL device: runs `draw::gl::Cmd` lists on the renderer's wgpu device.

mod format;
mod ops;
mod pass;
mod program;
mod res;
mod util;
mod vertex;

#[cfg(test)]
mod tests;

use crate::draw::gl::*;
use crate::draw::ImageId;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use crate::ErrorSink;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Largest single buffer or texture a page may allocate.
const MAX_ALLOC: u64 = 1 << 30;
/// Most GPU memory one context may hold in total (the state machine's own cap is lower).
const CTX_CAP: u64 = 1 << 30;

/// Device-wide state shared by every context.
pub(crate) struct Gpu {
    pub dev: wgpu::Device,
    pub queue: wgpu::Queue,
    pub features: wgpu::Features,
    pub limits: wgpu::Limits,
    /// The renderer's image bind group layout (presented images are drawn as renderer images).
    pub image_layout: wgpu::BindGroupLayout,
    pub helpers: util::Helpers,
    pub samplers: HashMap<res::SamplerKey, wgpu::Sampler>,
    pub dummies: HashMap<(TexKind, format::Class), (wgpu::Texture, wgpu::TextureView)>,
    /// Render passes encoded so far (tests check batching with it).
    pub passes: u64,
    pub serial: u64,
    pub debug: bool,
    /// Per-context GPU memory cap (`CTX_CAP`; tests lower it).
    pub ctx_cap: u64,
    pub errors: Arc<ErrorSink>,
}

impl Gpu {
    pub fn next_serial(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    pub fn log(&self, msg: impl FnOnce() -> String) {
        if self.debug {
            let m = msg();
            eprintln!("invisium-gl: {}", m.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }

    /// Run `f` inside a validation error scope; `Err` if wgpu reported an error.
    pub fn scoped<T>(&self, f: impl FnOnce() -> T) -> Result<T, String> {
        self.dev.push_error_scope(wgpu::ErrorFilter::Validation);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        let err = pollster::block_on(self.dev.pop_error_scope());
        match (r, err) {
            (Ok(v), None) => Ok(v),
            (_, Some(e)) => Err(e.to_string()),
            (Err(_), None) => Err("panic inside wgpu".into()),
        }
    }
}

/// Texture bind group layout and pipeline layout of a program bound with given variants.
pub(crate) type Layouts = (wgpu::BindGroupLayout, wgpu::PipelineLayout);

/// Everything one context owns.
#[derive(Default)]
pub(crate) struct Ctx {
    pub image: ImageId,
    pub surface: Option<res::Surface>,
    pub presented: Option<res::Presented>,
    pub buffers: HashMap<BufferId, res::Buf>,
    pub textures: HashMap<TextureId, res::Tex>,
    pub programs: HashMap<ProgramId, Option<Rc<program::Prog>>>,
    pub pipelines: HashMap<program::PipeKey, Option<wgpu::RenderPipeline>>,
    pub layouts: HashMap<(u64, Vec<program::Variant>), Option<Layouts>>,
    pub bind_groups: HashMap<Vec<u64>, wgpu::BindGroup>,
    pub seg: pass::Seg,
}

impl Ctx {
    fn exec(&mut self, g: &mut Gpu, cmd: Cmd) {
        match cmd {
            Cmd::Configure { width, height, alpha, depth, stencil, samples, premultiplied, preserve } => {
                let cfg = res::SurfaceCfg { width, height, alpha, depth, stencil, samples, premultiplied, preserve };
                self.configure(g, cfg)
            }
            Cmd::Present => self.present(g),
            Cmd::BufferCreate { id, index, bytes } => self.buffer_create(g, id, index, bytes),
            Cmd::BufferWrite { id, offset, bytes } => self.buffer_write(g, id, offset, &bytes),
            Cmd::BufferCopy { src, src_offset, dst, dst_offset, size } => {
                self.buffer_copy(g, src, src_offset, dst, dst_offset, size)
            }
            Cmd::BufferDelete { id } => {
                self.buffers.remove(&id);
            }
            Cmd::TextureCreate { id, desc } => self.texture_create(g, id, desc),
            Cmd::TextureGrow { id, mips } => self.texture_grow(g, id, mips),
            Cmd::TextureWrite { id, level, layer, x, y, width, height, depth, bytes } => {
                self.texture_write(g, id, level, layer, [x, y, width, height, depth], &bytes)
            }
            Cmd::GenerateMips { id } => self.generate_mips(g, id),
            Cmd::CopyToTexture { src, src_color, src_x, src_y, dst, dst_x, dst_y, width, height } => {
                self.copy_to_texture(g, &src, src_color, [src_x, src_y], dst, [dst_x, dst_y], [width, height])
            }
            Cmd::TextureDelete { id } => {
                self.flush_pass(g);
                if self.textures.remove(&id).is_some() {
                    self.bind_groups.clear();
                }
            }
            Cmd::ProgramCreate { id, desc } => {
                let _t = invisium_draw::prof::span("gpu: shader modules");
                self.program_create(g, id, *desc)
            }
            Cmd::ProgramDelete { id } => {
                self.programs.remove(&id);
            }
            Cmd::Clear { target, colors, depth, stencil, color_mask, stencil_mask, scissor } => {
                let c = pass::ClearReq { colors, depth, stencil, color_mask, stencil_mask, scissor };
                self.clear(g, &target, c)
            }
            Cmd::Draw(d) => self.draw(g, &d),
            Cmd::Blit { src, src_color, src_rect, dst, dst_rect, color, depth, stencil, linear } => {
                self.blit(g, &src, src_color, src_rect, &dst, dst_rect, [color, depth, stencil], linear)
            }
        }
    }

    fn texture_bytes(&self) -> u64 {
        (self.textures.values().map(|t| t.bytes).sum::<usize>()
            + self.surface.as_ref().map_or(0, |s| s.bytes)
            + self.presented.as_ref().map_or(0, |p| p.bytes)) as u64
    }

    /// GPU memory this context holds.
    pub(crate) fn gpu_bytes(&self) -> u64 {
        self.buffers.values().map(|b| b.gpu.size()).sum::<u64>() + self.texture_bytes()
    }

    /// Whether `more` bytes (replacing `freed`) keep this context within the cap.
    pub(crate) fn fits(&self, g: &Gpu, more: u64, freed: u64) -> bool {
        let ok = self.gpu_bytes().saturating_sub(freed).saturating_add(more) <= g.ctx_cap;
        if !ok {
            g.log(|| format!("allocation of {more} bytes refused: context over its memory cap"));
        }
        ok
    }

    fn stats(&self) -> GlStats {
        GlStats {
            contexts: 1,
            // GPU bytes plus any CPU copies.
            buffer_bytes: self.buffers.values().map(|b| b.gpu.size() as usize + b.data.as_ref().map_or(0, Vec::len)).sum(),
            texture_bytes: self.texture_bytes() as usize,
            programs: self.programs.len() as u32,
            pipelines: self.pipelines.values().filter(|p| p.is_some()).count() as u32,
        }
    }
}

/// The `GlDevice` handed out by `Renderer::gl_device`.
pub(crate) struct Device {
    pub(crate) gpu: Gpu,
    capable: bool,
    image_ids: Rc<Cell<ImageId>>,
    ctxs: HashMap<u32, Ctx>,
    images: HashMap<ImageId, u32>,
    next_ctx: u32,
}

impl Device {
    pub(crate) fn new(
        dev: wgpu::Device,
        queue: wgpu::Queue,
        image_layout: wgpu::BindGroupLayout,
        image_ids: Rc<Cell<ImageId>>,
        errors: Arc<ErrorSink>,
    ) -> Device {
        let debug = std::env::var_os("INVIS_GL_DEBUG").is_some();
        let limits = dev.limits();
        let features = dev.features();
        let capable = limits.max_texture_dimension_2d >= 8192 && limits.max_color_attachments >= 8;
        let gpu = Gpu {
            helpers: util::Helpers::default(),
            samplers: HashMap::new(),
            dummies: HashMap::new(),
            passes: 0,
            serial: 0,
            debug,
            ctx_cap: CTX_CAP,
            errors,
            dev,
            queue,
            features,
            limits,
            image_layout,
        };
        Device { gpu, capable, image_ids, ctxs: HashMap::new(), images: HashMap::new(), next_ctx: 1 }
    }

    /// The bind group (renderer image layout) of a presented context image.
    pub(crate) fn image_bind_group(&self, id: ImageId) -> Option<&wgpu::BindGroup> {
        let ctx = self.ctxs.get(self.images.get(&id)?)?;
        ctx.presented.as_ref().map(|p| &p.bind_group)
    }
}

impl GlDevice for Device {
    fn create(&mut self) -> Option<u32> {
        if !self.capable {
            return None;
        }
        let id = self.next_ctx;
        self.next_ctx = self.next_ctx.checked_add(1)?;
        let image = self.image_ids.get();
        self.image_ids.set(image.wrapping_add(1).max(1));
        self.ctxs.insert(id, Ctx { image, ..Default::default() });
        self.images.insert(image, id);
        Some(id)
    }

    fn destroy(&mut self, ctx: u32) {
        if let Some(c) = self.ctxs.remove(&ctx) {
            self.images.remove(&c.image);
        }
    }

    fn image(&self, ctx: u32) -> ImageId {
        self.ctxs.get(&ctx).map_or(0, |c| c.image)
    }

    fn run(&mut self, ctx: u32, cmds: &mut Vec<Cmd>) {
        let list = std::mem::take(cmds);
        let Some(c) = self.ctxs.get_mut(&ctx) else { return };
        let _t = invisium_draw::prof::span("gpu: run commands");
        let g = &mut self.gpu;
        g.errors.gl_running.store(true, Ordering::Relaxed);
        g.dev.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        g.dev.push_error_scope(wgpu::ErrorFilter::Validation);
        for cmd in list {
            c.exec(g, cmd);
        }
        c.submit(g);
        for _ in 0..2 {
            if let Some(e) = pollster::block_on(g.dev.pop_error_scope()) {
                g.errors.gl_errors.fetch_add(1, Ordering::Relaxed);
                g.log(|| format!("wgpu error during run: {e}"));
            }
        }
        g.errors.gl_running.store(false, Ordering::Relaxed);
    }

    fn read_pixels(&mut self, ctx: u32, target: &Target, color: u32, rect: Rect4) -> Option<(TexFormat, Vec<u8>)> {
        let c = self.ctxs.get_mut(&ctx)?;
        let g = &mut self.gpu;
        g.errors.gl_running.store(true, Ordering::Relaxed);
        g.dev.push_error_scope(wgpu::ErrorFilter::Validation);
        let r = c.read_pixels(g, target, color, rect);
        let err = pollster::block_on(g.dev.pop_error_scope());
        g.errors.gl_running.store(false, Ordering::Relaxed);
        match err {
            None => r,
            Some(e) => {
                g.log(|| format!("read_pixels failed: {e}"));
                None
            }
        }
    }

    fn read_buffer(&mut self, ctx: u32, buffer: BufferId, offset: u32, len: u32) -> Option<Vec<u8>> {
        let c = self.ctxs.get_mut(&ctx)?;
        let g = &mut self.gpu;
        g.errors.gl_running.store(true, Ordering::Relaxed);
        g.dev.push_error_scope(wgpu::ErrorFilter::Validation);
        let r = c.read_buffer(g, buffer, offset, len);
        let err = pollster::block_on(g.dev.pop_error_scope());
        g.errors.gl_running.store(false, Ordering::Relaxed);
        match err {
            None => r,
            Some(e) => {
                g.log(|| format!("read_buffer failed: {e}"));
                None
            }
        }
    }

    fn stats(&self) -> GlStats {
        let mut s = GlStats::default();
        for c in self.ctxs.values() {
            let t = c.stats();
            s.contexts += 1;
            s.buffer_bytes += t.buffer_bytes;
            s.texture_bytes += t.texture_bytes;
            s.programs += t.programs;
            s.pipelines += t.pipelines;
        }
        s
    }
}
