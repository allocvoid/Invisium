//! Shared test helpers: build a program, validate WGSL with naga, render with wgpu.
#![allow(dead_code)]

use invisium_glsl::{compile, link, Base, Place, Program, Stage};
use std::sync::OnceLock;

pub fn build(vs: &str, fs: &str, webgl2: bool) -> Program {
    try_build(vs, fs, webgl2, &[]).unwrap_or_else(|e| panic!("{e}\n--- vs ---\n{vs}\n--- fs ---\n{fs}"))
}

pub fn try_build(vs: &str, fs: &str, webgl2: bool, binds: &[(String, u32)]) -> Result<Program, String> {
    let v = compile(Stage::Vertex, vs, webgl2).map_err(|e| format!("vertex: {e}"))?;
    let f = compile(Stage::Fragment, fs, webgl2).map_err(|e| format!("fragment: {e}"))?;
    link(&v, &f, binds)
}

/// Parse and validate WGSL with every validation flag; panics with the source on failure.
pub fn validate(wgsl: &str) {
    let module = match naga::front::wgsl::parse_str(wgsl) {
        Ok(m) => m,
        Err(e) => panic!("WGSL parse error: {}\n{wgsl}", e.emit_to_string(wgsl)),
    };
    let mut v = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default());
    if let Err(e) = v.validate(&module) {
        panic!("WGSL validation error: {}\n{wgsl}", e.emit_to_string(wgsl));
    }
}

pub fn validate_program(p: &Program) {
    validate(&p.vertex_wgsl);
    validate(&p.fragment_wgsl);
}

/// Write `vals` into the default uniform block at the place reflection reports for `name`
/// (array element `index`).
pub fn set(p: &Program, buf: &mut [u8], name: &str, index: u32, vals: &[f32]) {
    let u = p.uniforms.iter().find(|u| u.name == name).unwrap_or_else(|| panic!("no uniform {name}: {:?}", p.uniforms));
    let Place::Block { offset, stride, base, rows, cols, col_stride } = u.place else { panic!("{name} is not in the block") };
    let start = offset + index * stride;
    for c in 0..cols as u32 {
        for r in 0..rows as u32 {
            let v = vals[(c * rows as u32 + r) as usize];
            let bytes = match base {
                Base::F32 => v.to_le_bytes(),
                Base::I32 => (v as i32).to_le_bytes(),
                Base::U32 => (v as u32).to_le_bytes(),
                Base::Bool => ((v != 0.0) as u32).to_le_bytes(),
            };
            let at = (start + c * col_stride + r * 4) as usize;
            buf[at..at + 4].copy_from_slice(&bytes);
        }
    }
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// The shared device, or `None` (after printing a skip notice) when there is no adapter.
pub fn gpu() -> Option<&'static Gpu> {
    static G: OnceLock<Option<Gpu>> = OnceLock::new();
    let g = G.get_or_init(|| {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).ok()?;
        Some(Gpu { device, queue })
    });
    if g.is_none() {
        println!("SKIP: no GPU adapter available; GPU checks not run");
    }
    g.as_ref()
}

pub enum Data {
    F(Vec<f32>),
    I(Vec<i32>),
}

pub struct Draw<'a> {
    pub prog: &'a Program,
    pub width: u32,
    pub height: u32,
    /// Per-vertex data by attribute name.
    pub attribs: Vec<(&'a str, Data)>,
    pub vertices: u32,
    pub uniforms: Vec<u8>,
    /// RGBA8 textures (width, height, texels) for the texture slots, in order.
    pub textures: Vec<(u32, u32, Vec<u8>)>,
}

impl<'a> Draw<'a> {
    pub fn new(prog: &'a Program) -> Draw<'a> {
        Draw {
            prog,
            width: 4,
            height: 4,
            attribs: vec![("a_pos", Data::F(vec![-1.0, -1.0, 3.0, -1.0, -1.0, 3.0]))],
            vertices: 3,
            uniforms: vec![0; prog.uniform_bytes as usize],
            textures: Vec::new(),
        }
    }
}

impl Gpu {
    /// Draw and read back the `Rgba32Float` target, row 0 first.
    pub fn render(&self, d: &Draw) -> Vec<[f32; 4]> {
        let dev = &self.device;
        let p = d.prog;
        let vm = dev.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(p.vertex_wgsl.as_str().into()) });
        let fm = dev.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(p.fragment_wgsl.as_str().into()) });
        let mut bufs = Vec::new();
        let mut layouts: Vec<(u64, Vec<wgpu::VertexAttribute>)> = Vec::new();
        for a in &p.attributes {
            let data = d.attribs.iter().find(|x| x.0 == a.name).unwrap_or_else(|| panic!("no data for attribute {}", a.name));
            let (bytes, n): (Vec<u8>, usize) = match &data.1 {
                Data::F(v) => (bytemuck::cast_slice(v).to_vec(), v.len()),
                Data::I(v) => (bytemuck::cast_slice(v).to_vec(), v.len()),
            };
            let comps = (n / d.vertices as usize) as u32;
            let rows = comps / a.columns;
            let fmt = |k: u32| match (&data.1, k) {
                (Data::F(_), 1) => wgpu::VertexFormat::Float32,
                (Data::F(_), 2) => wgpu::VertexFormat::Float32x2,
                (Data::F(_), 3) => wgpu::VertexFormat::Float32x3,
                (Data::F(_), _) => wgpu::VertexFormat::Float32x4,
                (Data::I(_), 1) => wgpu::VertexFormat::Sint32,
                (Data::I(_), 2) => wgpu::VertexFormat::Sint32x2,
                (Data::I(_), 3) => wgpu::VertexFormat::Sint32x3,
                (Data::I(_), _) => wgpu::VertexFormat::Sint32x4,
            };
            let attrs = (0..a.columns)
                .map(|c| wgpu::VertexAttribute { format: fmt(rows), offset: (c * rows * 4) as u64, shader_location: a.location + c })
                .collect();
            let buf = wgpu::util::DeviceExt::create_buffer_init(dev, &wgpu::util::BufferInitDescriptor { label: None, contents: &bytes, usage: wgpu::BufferUsages::VERTEX });
            bufs.push(buf);
            layouts.push(((comps * 4) as u64, attrs));
        }
        let vb: Vec<wgpu::VertexBufferLayout> = layouts
            .iter()
            .map(|(s, a)| wgpu::VertexBufferLayout { array_stride: *s, step_mode: wgpu::VertexStepMode::Vertex, attributes: a })
            .collect();
        let format = wgpu::TextureFormat::Rgba32Float;
        let pipe = dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState { module: &vm, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &vb },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &fm,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview: None,
            cache: None,
        });
        let mut groups = Vec::new();
        if p.uniform_bytes > 0 || !p.textures.is_empty() {
            let ub = (p.uniform_bytes > 0).then(|| {
                wgpu::util::DeviceExt::create_buffer_init(dev, &wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: &d.uniforms,
                    usage: wgpu::BufferUsages::UNIFORM,
                })
            });
            let entries: Vec<wgpu::BindGroupEntry> =
                ub.iter().map(|b| wgpu::BindGroupEntry { binding: 0, resource: b.as_entire_binding() }).collect();
            groups.push(dev.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipe.get_bind_group_layout(0), entries: &entries }));
        }
        let sampler = dev.create_sampler(&wgpu::SamplerDescriptor::default());
        let mut views = Vec::new();
        for (k, _) in p.textures.iter().enumerate() {
            let (w, h, texels) = d.textures.get(k).cloned().unwrap_or((1, 1, vec![0, 0, 0, 0]));
            let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
            let tex = dev.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.queue.write_texture(
                tex.as_image_copy(),
                &texels,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
                size,
            );
            views.push(tex.create_view(&Default::default()));
        }
        if !p.textures.is_empty() {
            let mut entries = Vec::new();
            for (k, t) in p.textures.iter().enumerate() {
                entries.push(wgpu::BindGroupEntry { binding: t.texture, resource: wgpu::BindingResource::TextureView(&views[k]) });
                entries.push(wgpu::BindGroupEntry { binding: t.sampler, resource: wgpu::BindingResource::Sampler(&sampler) });
            }
            groups.push(dev.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipe.get_bind_group_layout(1), entries: &entries }));
        }
        let target = dev.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: d.width, height: d.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let row = (d.width * 16).div_ceil(256) * 256;
        let out = dev.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * d.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = dev.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: -1.0, g: -1.0, b: -1.0, a: -1.0 }), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&pipe);
            for (i, g) in groups.iter().enumerate() {
                pass.set_bind_group(i as u32, g, &[]);
            }
            for (i, b) in bufs.iter().enumerate() {
                pass.set_vertex_buffer(i as u32, b.slice(..));
            }
            pass.draw(0..d.vertices, 0..1);
        }
        enc.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &out, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(d.height) } },
            wgpu::Extent3d { width: d.width, height: d.height, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = out.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        dev.poll(wgpu::Maintain::Wait);
        let data = slice.get_mapped_range();
        let mut px = Vec::new();
        for y in 0..d.height {
            for x in 0..d.width {
                let o = (y * row + x * 16) as usize;
                let f: &[f32] = bytemuck::cast_slice(&data[o..o + 16]);
                px.push([f[0], f[1], f[2], f[3]]);
            }
        }
        px
    }
}
