//! Invisium GPU renderer. Owned by the GFX track — see `docs/agents/GFX.md` for the contract.
//!
//! One pipeline draws every item kind as an instanced quad; the fragment shader evaluates the
//! shape analytically (signed distance to a rounded box). Draw calls are only split when two
//! different images are needed in the same frame.

pub use invisium_draw as draw;
pub use wgpu;

pub mod images;
pub mod svg;

use draw::{GlyphSource, ImageId, Item, Layer, Rect, Rgba};
use std::collections::HashMap;

const SHADER: &str = include_str!("shader.wgsl");
const INITIAL_ATLAS: u32 = 1024;
const MAX_STOPS_PER_GRADIENT: usize = 256;
const MIN_INSTANCE_CAP: usize = 256;
const MIN_STOP_CAP: usize = 64;
const MAX_GLYPH_PX: f32 = 2048.0;

const KIND_RECT: u32 = 0;
const KIND_BORDER: u32 = 1;
const KIND_SHADOW_OUTER: u32 = 2;
const KIND_SHADOW_INSET: u32 = 3;
const KIND_LINEAR: u32 = 4;
const KIND_RADIAL: u32 = 5;
const KIND_GLYPH: u32 = 6;
const KIND_IMAGE: u32 = 7;

const FLAG_REPEAT: u32 = 1;
const FLAG_ROUNDED_CLIP: u32 = 2;

#[derive(Clone, Debug)]
pub struct GpuOptions {
    pub low_power: bool,
    pub vsync: bool,
    pub max_atlas_size: u32,
}

impl Default for GpuOptions {
    fn default() -> Self {
        GpuOptions { low_power: true, vsync: true, max_atlas_size: 4096 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GpuStats {
    pub atlas_bytes: usize,
    pub atlas_glyphs: usize,
    pub image_bytes: usize,
    pub images: usize,
    pub instance_buffer_bytes: usize,
    pub last_frame_instances: u32,
    pub last_frame_draw_calls: u32,
    pub adapter: String,
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    quad: [f32; 4],
    shape: [f32; 4],
    radii: [f32; 4],
    clip: [f32; 4],
    clip_radii: [f32; 4],
    colors: [u32; 4],
    p0: [f32; 4],
    p1: [f32; 4],
    /// kind, kind-specific (stop start / sigma bits), stop count, flags
    info: [u32; 4],
}

const INSTANCE_SIZE: usize = std::mem::size_of::<Instance>();
const STOP_SIZE: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    face: u16,
    id: u16,
    size_q: u32,
    sub: u8,
}

#[derive(Clone, Copy)]
struct GlyphEntry {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    left: i32,
    top: i32,
}

struct Shelf {
    y: u32,
    h: u32,
    x: u32,
}

struct Atlas {
    texture: wgpu::Texture,
    size: u32,
    shelves: Vec<Shelf>,
    next_y: u32,
}

impl Atlas {
    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > self.size || h > self.size {
            return None;
        }
        let size = self.size;
        let best = self
            .shelves
            .iter_mut()
            .filter(|s| h <= s.h && s.x + w <= size)
            .min_by_key(|s| s.h);
        if let Some(s) = best {
            let pos = (s.x, s.y);
            s.x += w;
            return Some(pos);
        }
        if self.next_y + h <= self.size {
            let y = self.next_y;
            self.shelves.push(Shelf { y, h, x: w });
            self.next_y += h;
            return Some((0, y));
        }
        None
    }

    fn reset(&mut self) {
        self.shelves.clear();
        self.next_y = 0;
    }
}

struct ImageEntry {
    bind_group: wgpu::BindGroup,
    bytes: usize,
    _texture: wgpu::Texture,
}

#[derive(Clone, Copy)]
struct Segment {
    image: Option<ImageId>,
    start: u32,
    end: u32,
}

#[derive(Clone, Copy)]
struct Clip {
    rect: Rect,
    rounded: Option<(Rect, [f32; 4])>,
}

#[derive(Clone, Copy)]
struct Xf {
    ox: f32,
    oy: f32,
    sx: f32,
    sy: f32,
    s: f32,
}

impl Xf {
    fn x(&self, x: f32) -> f32 {
        self.ox + (x - self.sx) * self.s
    }
    fn y(&self, y: f32) -> f32 {
        self.oy + (y - self.sy) * self.s
    }
    fn rect(&self, r: &Rect) -> Rect {
        Rect::new(self.x(r.x), self.y(r.y), r.w * self.s, r.h * self.s)
    }
    fn radii(&self, r: &[f32; 4]) -> [f32; 4] {
        r.map(|v| if v.is_finite() { (v * self.s).max(0.0) } else { 0.0 })
    }
}

enum Target {
    Headless { texture: Option<wgpu::Texture> },
    Surface { surface: wgpu::Surface<'static>, config: wgpu::SurfaceConfiguration },
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: Target,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    adapter_name: String,
    max_atlas: u32,
    max_texture: u32,

    pipeline: wgpu::RenderPipeline,
    bgl0: wgpu::BindGroupLayout,
    bgl1: wgpu::BindGroupLayout,
    bind0: Option<wgpu::BindGroup>,
    uniforms: wgpu::Buffer,
    sampler_clamp: wgpu::Sampler,
    sampler_repeat: wgpu::Sampler,
    dummy_atlas: wgpu::TextureView,
    dummy_image: wgpu::BindGroup,

    atlas: Option<Atlas>,
    glyph_cache: HashMap<GlyphKey, Option<GlyphEntry>>,
    atlas_glyphs: usize,

    images: HashMap<ImageId, ImageEntry>,
    next_image: ImageId,

    instances: Vec<Instance>,
    segments: Vec<Segment>,
    stops: Vec<[u32; 2]>,
    clip_stack: Vec<Clip>,
    fixed_stack: Vec<usize>,
    instance_buf: Option<wgpu::Buffer>,
    instance_cap: usize,
    stop_buf: wgpu::Buffer,
    stop_cap: usize,

    pending_clear: Option<wgpu::Color>,
    frame_instances: u32,
    frame_draws: u32,
    last_instances: u32,
    last_draws: u32,
}

async fn pick_adapter(
    instance: &wgpu::Instance,
    opts: &GpuOptions,
    surface: Option<&wgpu::Surface<'_>>,
) -> Option<wgpu::Adapter> {
    let power_preference =
        if opts.low_power { wgpu::PowerPreference::LowPower } else { wgpu::PowerPreference::HighPerformance };
    for force_fallback_adapter in [false, true] {
        let options =
            wgpu::RequestAdapterOptions { power_preference, force_fallback_adapter, compatible_surface: surface };
        if let Some(a) = instance.request_adapter(&options).await {
            return Some(a);
        }
    }
    None
}

async fn open_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), String> {
    let limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
    adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                label: Some("invisium-gfx"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::MemoryUsage,
            },
            None,
        )
        .await
        .map_err(|e| format!("request_device failed: {e}"))
}

fn new_instance() -> wgpu::Instance {
    wgpu::Instance::new(&wgpu::InstanceDescriptor { backends: wgpu::Backends::PRIMARY, ..Default::default() })
}

fn pack(c: Rgba) -> u32 {
    c.r as u32 | (c.g as u32) << 8 | (c.b as u32) << 16 | (c.a as u32) << 24
}

fn finite_rect(r: &Rect) -> bool {
    r.x.is_finite() && r.y.is_finite() && r.w.is_finite() && r.h.is_finite()
}

fn r4(r: &Rect) -> [f32; 4] {
    [r.x, r.y, r.w, r.h]
}

/// Scale radii down as CSS does when adjacent radii exceed the side length.
fn fix_radii(r: &Rect, radii: [f32; 4]) -> [f32; 4] {
    let [tl, tr, br, bl] = radii;
    let mut f: f32 = 1.0;
    for (len, sum) in [(r.w, tl + tr), (r.w, bl + br), (r.h, tl + bl), (r.h, tr + br)] {
        if sum > 0.0 && len.max(0.0) < sum * f {
            f = len.max(0.0) / sum;
        }
    }
    radii.map(|v| v * f)
}

fn max4(v: &[f32; 4]) -> f32 {
    v[0].max(v[1]).max(v[2]).max(v[3])
}

impl Renderer {
    /// Render to a window. `target` is anything wgpu accepts as a 'static surface target
    /// (e.g. `Arc<winit::window::Window>`); this crate must not depend on winit.
    pub async fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
        opts: GpuOptions,
    ) -> Result<Renderer, String> {
        let instance = new_instance();
        let surface = instance.create_surface(target).map_err(|e| format!("create_surface failed: {e}"))?;
        let adapter =
            pick_adapter(&instance, &opts, Some(&surface)).await.ok_or_else(|| "no GPU adapter available".to_string())?;
        let (device, queue) = open_device(&adapter).await?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .ok_or_else(|| "surface offers no non-sRGB format".to_string())?;
        let present_mode = if opts.vsync { wgpu::PresentMode::Fifo } else { wgpu::PresentMode::AutoNoVsync };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
        };
        if width > 0 && height > 0 {
            surface.configure(&device, &config);
        }
        let info = adapter.get_info();
        Ok(Self::build(device, queue, Target::Surface { surface, config }, format, width, height, opts, &info))
    }

    /// Render to an offscreen texture (tests, screenshots). No window is created.
    pub async fn new_headless(width: u32, height: u32, opts: GpuOptions) -> Result<Renderer, String> {
        let instance = new_instance();
        let adapter = pick_adapter(&instance, &opts, None).await.ok_or_else(|| "no GPU adapter available".to_string())?;
        let (device, queue) = open_device(&adapter).await?;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let info = adapter.get_info();
        let mut r = Self::build(device, queue, Target::Headless { texture: None }, format, width, height, opts, &info);
        r.resize(width, height);
        Ok(r)
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target: Target,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        opts: GpuOptions,
        info: &wgpu::AdapterInfo,
    ) -> Renderer {
        let max_texture = device.limits().max_texture_dimension_2d;
        let max_atlas = opts.max_atlas_size.clamp(64, max_texture.max(64));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("invisium-gfx shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let frag = wgpu::ShaderStages::FRAGMENT;
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: frag,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: frag,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let bgl0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("invisium-gfx globals"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | frag,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: frag,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                sampler_entry(3),
                sampler_entry(4),
            ],
        });
        let bgl1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("invisium-gfx image"),
            entries: &[tex_entry(0)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("invisium-gfx layout"),
            bind_group_layouts: &[&bgl0, &bgl1],
            push_constant_ranges: &[],
        });
        let attrs = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
            5 => Uint32x4, 6 => Float32x4, 7 => Float32x4, 8 => Uint32x4
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("invisium-gfx pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attrs,
                }],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("invisium-gfx uniforms"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mk_sampler = |mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("invisium-gfx sampler"),
                address_mode_u: mode,
                address_mode_v: mode,
                address_mode_w: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            })
        };
        let sampler_clamp = mk_sampler(wgpu::AddressMode::ClampToEdge);
        let sampler_repeat = mk_sampler(wgpu::AddressMode::Repeat);
        let tiny = |fmt| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("invisium-gfx dummy"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: fmt,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let dummy_atlas = tiny(wgpu::TextureFormat::R8Unorm).create_view(&Default::default());
        let dummy_image_view = tiny(wgpu::TextureFormat::Rgba8Unorm).create_view(&Default::default());
        let dummy_image = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("invisium-gfx dummy image"),
            layout: &bgl1,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&dummy_image_view) }],
        });
        let stop_buf = Self::make_stop_buf(&device, MIN_STOP_CAP);
        Renderer {
            device,
            queue,
            target,
            format,
            width,
            height,
            adapter_name: format!("{} ({:?})", info.name, info.backend),
            max_atlas,
            max_texture,
            pipeline,
            bgl0,
            bgl1,
            bind0: None,
            uniforms,
            sampler_clamp,
            sampler_repeat,
            dummy_atlas,
            dummy_image,
            atlas: None,
            glyph_cache: HashMap::new(),
            atlas_glyphs: 0,
            images: HashMap::new(),
            next_image: 1,
            instances: Vec::new(),
            segments: Vec::new(),
            stops: Vec::new(),
            clip_stack: Vec::new(),
            fixed_stack: Vec::new(),
            instance_buf: None,
            instance_cap: 0,
            stop_buf,
            stop_cap: MIN_STOP_CAP,
            pending_clear: None,
            frame_instances: 0,
            frame_draws: 0,
            last_instances: 0,
            last_draws: 0,
        }
    }

    fn make_stop_buf(device: &wgpu::Device, cap: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("invisium-gfx stops"),
            size: (cap * STOP_SIZE) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Physical pixels.
    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.min(self.max_texture);
        let height = height.min(self.max_texture);
        self.width = width;
        self.height = height;
        match &mut self.target {
            Target::Headless { texture } => {
                *texture = (width > 0 && height > 0).then(|| {
                    self.device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("invisium-gfx headless target"),
                        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: self.format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    })
                });
            }
            Target::Surface { surface, config } => {
                config.width = width;
                config.height = height;
                if width > 0 && height > 0 {
                    surface.configure(&self.device, config);
                }
            }
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Upload straight-alpha RGBA8 pixels (`rgba.len() == w*h*4`). The caller may drop its copy.
    /// Invalid input yields an id that draws nothing.
    pub fn create_image(&mut self, width: u32, height: u32, rgba: &[u8]) -> ImageId {
        let id = self.next_image;
        self.next_image = self.next_image.wrapping_add(1).max(1);
        let len = width as u64 * height as u64 * 4;
        if width == 0 || height == 0 || width > self.max_texture || height > self.max_texture || rgba.len() as u64 != len {
            return id;
        }
        // Premultiply so bilinear filtering does not bleed colour from transparent texels.
        let mut pm = rgba.to_vec();
        for px in pm.chunks_exact_mut(4) {
            let a = px[3] as u32;
            if a != 255 {
                for c in &mut px[..3] {
                    *c = ((*c as u32 * a + 127) / 255) as u8;
                }
            }
        }
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("invisium-gfx image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pm,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            size,
        );
        let view = texture.create_view(&Default::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("invisium-gfx image"),
            layout: &self.bgl1,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }],
        });
        self.images.insert(id, ImageEntry { bind_group, bytes: len as usize, _texture: texture });
        id
    }

    pub fn drop_image(&mut self, id: ImageId) {
        self.images.remove(&id);
    }

    /// Draw one frame: clear to `clear`, then the layers in order.
    pub fn render(&mut self, layers: &[Layer], glyphs: &dyn GlyphSource, clear: Rgba) -> Result<(), String> {
        self.frame_instances = 0;
        self.frame_draws = 0;
        self.last_instances = 0;
        self.last_draws = 0;
        if self.width == 0 || self.height == 0 {
            return Ok(());
        }
        let mut frame = None;
        let view = match &mut self.target {
            Target::Headless { texture } => match texture {
                Some(t) => t.create_view(&Default::default()),
                None => return Ok(()),
            },
            Target::Surface { surface, config } => {
                let tex = match surface.get_current_texture() {
                    Ok(t) => t,
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        surface.configure(&self.device, config);
                        match surface.get_current_texture() {
                            Ok(t) => t,
                            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
                            Err(e) => return Err(format!("surface unavailable: {e}")),
                        }
                    }
                    Err(wgpu::SurfaceError::Timeout) => return Ok(()),
                    Err(e) => return Err(format!("surface unavailable: {e}")),
                };
                let v = tex.texture.create_view(&Default::default());
                frame = Some(tex);
                v
            }
        };
        let a = clear.a as f64 / 255.0;
        self.pending_clear = Some(wgpu::Color {
            r: clear.r as f64 / 255.0 * a,
            g: clear.g as f64 / 255.0 * a,
            b: clear.b as f64 / 255.0 * a,
            a,
        });
        self.instances.clear();
        self.segments.clear();
        self.stops.clear();
        for layer in layers {
            self.build_layer(layer, glyphs, &view);
        }
        self.flush(&view);
        if let Some(f) = frame {
            let suboptimal = f.suboptimal;
            f.present();
            if suboptimal {
                if let Target::Surface { surface, config } = &self.target {
                    surface.configure(&self.device, config);
                }
            }
        }
        self.last_instances = self.frame_instances;
        self.last_draws = self.frame_draws;
        Ok(())
    }

    fn build_layer(&mut self, layer: &Layer, src: &dyn GlyphSource, view: &wgpu::TextureView) {
        let screen = Rect::new(0.0, 0.0, self.width as f32, self.height as f32);
        if !finite_rect(&layer.viewport) {
            return;
        }
        let vp = layer.viewport.intersect(&screen);
        if vp.is_empty() {
            return;
        }
        let s = if layer.scale.is_finite() && layer.scale > 0.0 { layer.scale } else { 1.0 };
        let fin = |v: f32| if v.is_finite() { v } else { 0.0 };
        let (scroll_x, scroll_y) = (fin(layer.scroll.0), fin(layer.scroll.1));
        let list = layer.list;
        let base = Clip { rect: vp, rounded: None };
        let mut clips = std::mem::take(&mut self.clip_stack);
        let mut fixed = std::mem::take(&mut self.fixed_stack);
        clips.clear();
        fixed.clear();
        clips.push(base);

        for item in &list.items {
            let is_fixed = !fixed.is_empty();
            let xf = Xf {
                ox: layer.viewport.x,
                oy: layer.viewport.y,
                sx: if is_fixed { 0.0 } else { scroll_x },
                sy: if is_fixed { 0.0 } else { scroll_y },
                s,
            };
            let clip = *clips.last().unwrap_or(&base);
            match item {
                Item::PushClip { rect, radii } => {
                    let r = xf.rect(rect);
                    let r = if finite_rect(&r) { r } else { Rect::ZERO };
                    let rad = fix_radii(&r, xf.radii(radii));
                    let rounded = if max4(&rad) > 0.0 { Some((r, rad)) } else { clip.rounded };
                    clips.push(Clip { rect: clip.rect.intersect(&r), rounded });
                }
                Item::PopClip => {
                    let floor = fixed.last().map(|n| n + 1).unwrap_or(1);
                    if clips.len() > floor {
                        clips.pop();
                    }
                }
                Item::PushFixed => {
                    fixed.push(clips.len());
                    clips.push(base);
                }
                Item::PopFixed => {
                    if let Some(n) = fixed.pop() {
                        clips.truncate(n.max(1));
                    }
                }
                Item::Rect { rect, color, radii } => {
                    if color.a == 0 {
                        continue;
                    }
                    let r = xf.rect(rect);
                    if !finite_rect(&r) || r.is_empty() {
                        continue;
                    }
                    let rad = fix_radii(&r, xf.radii(radii));
                    // Clamp huge shapes so the SDF stays numerically sane; corners stay out of view.
                    let shape = r.intersect(&clip.rect.inflate(max4(&rad) + 2.0));
                    let inst = Instance {
                        shape: r4(&shape),
                        radii: rad,
                        colors: [pack(*color), 0, 0, 0],
                        info: [KIND_RECT, 0, 0, 0],
                        ..Default::default()
                    };
                    self.push(inst, r.inflate(1.0), &clip, None);
                }
                Item::Border { rect, widths, colors, radii } => {
                    let r = xf.rect(rect);
                    if !finite_rect(&r) || r.is_empty() || colors.iter().all(|c| c.a == 0) {
                        continue;
                    }
                    let w = xf.radii(widths); // same sanitising: finite, >= 0, scaled
                    if max4(&w) <= 0.0 {
                        continue;
                    }
                    let rad = fix_radii(&r, xf.radii(radii));
                    let [wt, wr, wb, wl] = w;
                    let inner = [
                        (rad[0] - wt.max(wl)).max(0.0),
                        (rad[1] - wt.max(wr)).max(0.0),
                        (rad[2] - wb.max(wr)).max(0.0),
                        (rad[3] - wb.max(wl)).max(0.0),
                    ];
                    let inst = Instance {
                        shape: r4(&r),
                        radii: rad,
                        colors: colors.map(pack),
                        p0: w,
                        p1: inner,
                        info: [KIND_BORDER, 0, 0, 0],
                        ..Default::default()
                    };
                    self.push(inst, r.inflate(1.0), &clip, None);
                }
                Item::Shadow { rect, radii, dx, dy, blur, spread, color, inset } => {
                    if color.a == 0 {
                        continue;
                    }
                    let b = xf.rect(rect);
                    let (dx, dy, spread) = (fin(*dx) * s, fin(*dy) * s, fin(*spread) * s);
                    let sigma = fin(*blur).max(0.0) * 0.5 * s;
                    if !finite_rect(&b) || b.is_empty() {
                        continue;
                    }
                    let brad = fix_radii(&b, xf.radii(radii));
                    let (shadow, srad, bounds, kind) = if *inset {
                        let sh = b.translate(dx, dy).inflate(-spread);
                        let sr = brad.map(|v| (v - spread).max(0.0));
                        (sh, sr, b, KIND_SHADOW_INSET)
                    } else {
                        let sh = b.translate(dx, dy).inflate(spread);
                        let sr = brad.map(|v| if v > 0.0 { (v + spread).max(0.0) } else { 0.0 });
                        (sh, sr, sh.inflate(3.0 * sigma + 1.0), KIND_SHADOW_OUTER)
                    };
                    if !finite_rect(&shadow) || !sigma.is_finite() {
                        continue;
                    }
                    if !*inset && shadow.is_empty() {
                        continue;
                    }
                    let srad = fix_radii(&shadow, srad);
                    let inst = Instance {
                        shape: r4(&shadow),
                        radii: srad,
                        colors: [pack(*color), 0, 0, 0],
                        p0: r4(&b),
                        p1: brad,
                        info: [kind, sigma.to_bits(), 0, 0],
                        ..Default::default()
                    };
                    self.push(inst, bounds, &clip, None);
                }
                Item::LinearGradient { rect, radii, angle, stops, repeating } => {
                    let r = xf.rect(rect);
                    if !finite_rect(&r) || r.is_empty() {
                        continue;
                    }
                    let Some((start, count)) = self.add_stops(&list.stops, *stops) else { continue };
                    let th = fin(*angle).to_radians();
                    let (dir_x, dir_y) = (th.sin(), -th.cos());
                    let len = (r.w * dir_x).abs() + (r.h * dir_y).abs();
                    let (cx, cy) = (r.x + r.w * 0.5, r.y + r.h * 0.5);
                    let p0 = if len > 0.0 && len.is_finite() {
                        [cx - dir_x * len * 0.5, cy - dir_y * len * 0.5, dir_x / len, dir_y / len]
                    } else {
                        [cx, cy, 0.0, 0.0]
                    };
                    let inst = Instance {
                        shape: r4(&r),
                        radii: fix_radii(&r, xf.radii(radii)),
                        p0,
                        info: [KIND_LINEAR, start, count, if *repeating { FLAG_REPEAT } else { 0 }],
                        ..Default::default()
                    };
                    self.push(inst, r.inflate(1.0), &clip, None);
                }
                Item::RadialGradient { rect, radii, center, radius, stops, repeating } => {
                    let r = xf.rect(rect);
                    if !finite_rect(&r) || r.is_empty() {
                        continue;
                    }
                    let Some((start, count)) = self.add_stops(&list.stops, *stops) else { continue };
                    let inv = |v: f32| {
                        let v = fin(v) * s;
                        if v > 1e-6 { 1.0 / v } else { 1e6 }
                    };
                    let p0 = [r.x + fin(center.0) * s, r.y + fin(center.1) * s, inv(radius.0), inv(radius.1)];
                    let inst = Instance {
                        shape: r4(&r),
                        radii: fix_radii(&r, xf.radii(radii)),
                        p0,
                        info: [KIND_RADIAL, start, count, if *repeating { FLAG_REPEAT } else { 0 }],
                        ..Default::default()
                    };
                    self.push(inst, r.inflate(1.0), &clip, None);
                }
                Item::Image { rect, image, radii, opacity, uv, repeat } => {
                    let op = if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { 0.0 };
                    if op <= 0.0 || !self.images.contains_key(image) {
                        continue;
                    }
                    let r = xf.rect(rect);
                    if !finite_rect(&r) || r.is_empty() {
                        continue;
                    }
                    let uv = if uv.iter().all(|v| v.is_finite()) { *uv } else { [0.0, 0.0, 1.0, 1.0] };
                    let inst = Instance {
                        shape: r4(&r),
                        radii: fix_radii(&r, xf.radii(radii)),
                        p0: uv,
                        p1: [op, 0.0, 0.0, 0.0],
                        info: [KIND_IMAGE, 0, 0, if *repeat { FLAG_REPEAT } else { 0 }],
                        ..Default::default()
                    };
                    self.push(inst, r.inflate(1.0), &clip, Some(*image));
                }
                Item::Glyphs { x, baseline, w, size, color, glyphs } => {
                    if color.a == 0 {
                        continue;
                    }
                    let px_size = size * s;
                    if !(px_size.is_finite() && px_size > 0.0 && px_size <= MAX_GLYPH_PX) {
                        continue;
                    }
                    let size_q = (px_size * 4.0).round() as u32;
                    if size_q == 0 {
                        continue;
                    }
                    let base_y = xf.y(*baseline).round();
                    if !base_y.is_finite() || !x.is_finite() {
                        continue;
                    }
                    if w.is_finite() && *w >= 0.0 {
                        let run = Rect::new(xf.x(*x) - px_size, base_y - px_size * 1.5, w * s + 2.0 * px_size, px_size * 2.5);
                        if !run.intersects(&clip.rect) {
                            continue;
                        }
                    }
                    let lo = (glyphs.0 as usize).min(layer.glyphs.len());
                    let hi = (glyphs.1 as usize).min(layer.glyphs.len());
                    let packed = pack(*color);
                    let mut pen = *x;
                    for g in layer.glyphs.get(lo..hi).unwrap_or(&[]) {
                        if g.id != 0 {
                            let q = (xf.x(pen) * 4.0).round();
                            let ix = (q / 4.0).floor();
                            let sub = ((q - ix * 4.0) as u8).min(3);
                            let key = GlyphKey { face: g.face, id: g.id, size_q, sub };
                            if let Some(e) = self.glyph_entry(key, src, view) {
                                let quad = Rect::new(ix + e.left as f32, base_y + e.top as f32, e.w as f32, e.h as f32);
                                let inst = Instance {
                                    shape: r4(&quad),
                                    colors: [packed, 0, 0, 0],
                                    p1: [e.x as f32, e.y as f32, 0.0, 0.0],
                                    info: [KIND_GLYPH, 0, 0, 0],
                                    ..Default::default()
                                };
                                self.push(inst, quad, &clip, None);
                            }
                        }
                        if g.adv.is_finite() {
                            pen += g.adv;
                        }
                    }
                }
            }
        }
        self.clip_stack = clips;
        self.fixed_stack = fixed;
    }

    fn add_stops(&mut self, all: &[draw::Stop], span: draw::Span) -> Option<(u32, u32)> {
        let lo = (span.0 as usize).min(all.len());
        let hi = (span.1 as usize).min(all.len()).min(lo + MAX_STOPS_PER_GRADIENT);
        if hi <= lo {
            return None;
        }
        let start = self.stops.len() as u32;
        let mut prev = f32::MIN;
        for st in &all[lo..hi] {
            let mut o = if st.offset.is_finite() { st.offset } else { 0.0 };
            if o < prev {
                o = prev;
            }
            prev = o;
            self.stops.push([o.to_bits(), pack(st.color)]);
        }
        Some((start, (hi - lo) as u32))
    }

    fn push(&mut self, mut inst: Instance, bounds: Rect, clip: &Clip, image: Option<ImageId>) {
        let q = bounds.intersect(&clip.rect);
        if !(finite_rect(&q) && q.w > 0.0 && q.h > 0.0) {
            return;
        }
        inst.quad = r4(&q);
        if let Some((r, rad)) = clip.rounded {
            inst.clip = r4(&r);
            inst.clip_radii = rad;
            inst.info[3] |= FLAG_ROUNDED_CLIP;
        }
        let n = self.instances.len() as u32;
        match self.segments.last_mut() {
            Some(seg) if image.is_none() || seg.image.is_none() || seg.image == image => {
                if seg.image.is_none() {
                    seg.image = image;
                }
                seg.end = n + 1;
            }
            _ => self.segments.push(Segment { image, start: n, end: n + 1 }),
        }
        self.instances.push(inst);
    }

    fn glyph_entry(&mut self, key: GlyphKey, src: &dyn GlyphSource, view: &wgpu::TextureView) -> Option<GlyphEntry> {
        if let Some(e) = self.glyph_cache.get(&key) {
            return *e;
        }
        let bm = src.rasterize(key.face, key.id, key.size_q as f32 / 4.0, key.sub);
        let entry = match bm {
            Some(b)
                if b.width > 0
                    && b.height > 0
                    && b.width <= self.max_atlas
                    && b.height <= self.max_atlas
                    && b.data.len() as u64 >= b.width as u64 * b.height as u64 =>
            {
                self.place_glyph(&b, view)
            }
            _ => None,
        };
        if entry.is_some() {
            self.atlas_glyphs += 1;
        }
        self.glyph_cache.insert(key, entry);
        entry
    }

    fn new_atlas_texture(&self, size: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("invisium-gfx glyph atlas"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    fn place_glyph(&mut self, b: &draw::GlyphBitmap, view: &wgpu::TextureView) -> Option<GlyphEntry> {
        if self.atlas.is_none() {
            let size = INITIAL_ATLAS.min(self.max_atlas);
            let texture = self.new_atlas_texture(size);
            self.atlas = Some(Atlas { texture, size, shelves: Vec::new(), next_y: 0 });
            self.bind0 = None;
        }
        for _ in 0..8 {
            let atlas = self.atlas.as_mut()?;
            if let Some((x, y)) = atlas.alloc(b.width, b.height) {
                let size = wgpu::Extent3d { width: b.width, height: b.height, depth_or_array_layers: 1 };
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &atlas.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x, y, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &b.data[..(b.width * b.height) as usize],
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(b.width), rows_per_image: Some(b.height) },
                    size,
                );
                return Some(GlyphEntry { x, y, w: b.width, h: b.height, left: b.left, top: b.top });
            }
            if atlas.size < self.max_atlas {
                // Grow: copy the old contents on the GPU; existing entries keep their coordinates.
                let old_size = atlas.size;
                let new_size = (old_size * 2).min(self.max_atlas);
                let texture = self.new_atlas_texture(new_size);
                let mut enc = self.device.create_command_encoder(&Default::default());
                let atlas = self.atlas.as_mut()?;
                enc.copy_texture_to_texture(
                    atlas.texture.as_image_copy(),
                    texture.as_image_copy(),
                    wgpu::Extent3d { width: old_size, height: old_size, depth_or_array_layers: 1 },
                );
                self.queue.submit([enc.finish()]);
                atlas.texture = texture;
                atlas.size = new_size;
                self.bind0 = None;
            } else {
                // Full at maximum size: draw what references the atlas so far, then evict all.
                if !self.instances.is_empty() {
                    self.flush(view);
                }
                if let Some(a) = self.atlas.as_mut() {
                    a.reset();
                }
                self.glyph_cache.clear();
                self.atlas_glyphs = 0;
            }
        }
        None
    }

    fn ensure_bind0(&mut self) {
        if self.bind0.is_some() {
            return;
        }
        let atlas_view = self.atlas.as_ref().map(|a| a.texture.create_view(&Default::default()));
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("invisium-gfx globals"),
            layout: &self.bgl0,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(atlas_view.as_ref().unwrap_or(&self.dummy_atlas)),
                },
                wgpu::BindGroupEntry { binding: 2, resource: self.stop_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler_clamp) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler_repeat) },
            ],
        });
        self.bind0 = Some(bg);
    }

    /// Encode and submit everything collected so far.
    fn flush(&mut self, view: &wgpu::TextureView) {
        let n = self.instances.len();
        if n > self.instance_cap || self.instance_buf.is_none() {
            let cap = n.next_power_of_two().max(MIN_INSTANCE_CAP);
            self.instance_buf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("invisium-gfx instances"),
                size: (cap * INSTANCE_SIZE) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.instance_cap = cap;
        }
        if self.stops.len() > self.stop_cap {
            let cap = self.stops.len().next_power_of_two();
            self.stop_buf = Self::make_stop_buf(&self.device, cap);
            self.stop_cap = cap;
            self.bind0 = None;
        }
        self.ensure_bind0();
        let globals = [self.width as f32, self.height as f32, 0.0, 0.0];
        self.queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&globals));
        if !self.stops.is_empty() {
            self.queue.write_buffer(&self.stop_buf, 0, bytemuck::cast_slice(&self.stops));
        }
        let (Some(ibuf), Some(bind0)) = (self.instance_buf.as_ref(), self.bind0.as_ref()) else { return };
        if n > 0 {
            self.queue.write_buffer(ibuf, 0, bytemuck::cast_slice(&self.instances));
        }
        let load = match self.pending_clear.take() {
            Some(c) => wgpu::LoadOp::Clear(c),
            None => wgpu::LoadOp::Load,
        };
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("invisium-gfx frame") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("invisium-gfx pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if n > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, bind0, &[]);
                pass.set_vertex_buffer(0, ibuf.slice(..(n * INSTANCE_SIZE) as u64));
                for seg in &self.segments {
                    let bg = seg.image.and_then(|id| self.images.get(&id)).map(|e| &e.bind_group);
                    pass.set_bind_group(1, bg.unwrap_or(&self.dummy_image), &[]);
                    pass.draw(0..4, seg.start..seg.end);
                    self.frame_draws += 1;
                }
            }
        }
        self.queue.submit([enc.finish()]);
        self.frame_instances += n as u32;
        self.instances.clear();
        self.segments.clear();
        self.stops.clear();
    }

    /// Headless only: the last rendered frame as tightly packed straight-alpha RGBA8 rows
    /// (top row first).
    pub fn read_rgba(&mut self) -> Result<Vec<u8>, String> {
        let texture = match &self.target {
            Target::Headless { texture: Some(t) } => t,
            Target::Headless { texture: None } => return Ok(Vec::new()),
            Target::Surface { .. } => return Err("read_rgba is only available on headless renderers".into()),
        };
        let (w, h) = (self.width, self.height);
        let row = w * 4;
        let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("invisium-gfx readback"),
            size: padded as u64 * h as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::Maintain::Wait);
        rx.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity((row * h) as usize);
        {
            let data = slice.get_mapped_range();
            for y in 0..h as usize {
                let start = y * padded as usize;
                out.extend_from_slice(&data[start..start + row as usize]);
            }
        }
        buf.unmap();
        for px in out.chunks_exact_mut(4) {
            let a = px[3] as u32;
            if a != 0 && a != 255 {
                for c in &mut px[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        Ok(out)
    }

    /// Headless only: write the last rendered frame as a PNG.
    pub fn save_png(&mut self, path: &std::path::Path) -> Result<(), String> {
        let px = self.read_rgba()?;
        let img = image::RgbaImage::from_raw(self.width, self.height, px).ok_or_else(|| "empty frame".to_string())?;
        img.save_with_format(path, image::ImageFormat::Png).map_err(|e| e.to_string())
    }

    /// Free what can be rebuilt (glyph atlas contents, spare buffer capacity).
    pub fn trim(&mut self) {
        self.atlas = None;
        self.glyph_cache = HashMap::new();
        self.atlas_glyphs = 0;
        self.instances = Vec::new();
        self.segments = Vec::new();
        self.stops = Vec::new();
        self.clip_stack = Vec::new();
        self.fixed_stack = Vec::new();
        self.instance_buf = None;
        self.instance_cap = 0;
        if self.stop_cap > MIN_STOP_CAP {
            self.stop_buf = Self::make_stop_buf(&self.device, MIN_STOP_CAP);
            self.stop_cap = MIN_STOP_CAP;
        }
        self.bind0 = None;
    }

    pub fn stats(&self) -> GpuStats {
        GpuStats {
            atlas_bytes: self.atlas.as_ref().map(|a| a.size as usize * a.size as usize).unwrap_or(0),
            atlas_glyphs: self.atlas_glyphs,
            image_bytes: self.images.values().map(|e| e.bytes).sum(),
            images: self.images.len(),
            instance_buffer_bytes: self.instance_cap * INSTANCE_SIZE + self.stop_cap * STOP_SIZE,
            last_frame_instances: self.last_instances,
            last_frame_draw_calls: self.last_draws,
            adapter: self.adapter_name.clone(),
        }
    }
}
