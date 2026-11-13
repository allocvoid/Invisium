//! Programs, texture bindings and the pipeline cache.

use super::format::{self, Class};
use super::pass::Rt;
use super::res::{self, ViewKey};
use super::vertex::SlotKey;
use super::{Ctx, Gpu};
use crate::draw::gl::*;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

pub(crate) struct Prog {
    pub serial: u64,
    pub desc: ProgramDesc,
    pub vs: wgpu::ShaderModule,
    pub fs: wgpu::ShaderModule,
    pub bgl0: wgpu::BindGroupLayout,
}

/// How one texture slot is bound in a particular draw (decides the bind group layout).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Variant {
    Filterable,
    Unfilterable,
    Shadow,
    Sint,
    Uint,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct StateKey {
    topology: Topology,
    cull: Option<Face>,
    front_ccw: bool,
    blend: Option<Blend>,
    color_mask: [bool; 4],
    depth_test: Option<Compare>,
    depth_write: bool,
    bias: Option<(u32, u32)>,
    stencil: Option<(StencilFace, StencilFace, u32, u32)>,
    a2c: bool,
}

impl StateKey {
    pub fn new(s: &DrawState) -> StateKey {
        StateKey {
            topology: s.topology,
            cull: s.cull,
            front_ccw: s.front_ccw,
            blend: s.blend,
            color_mask: s.color_mask,
            depth_test: s.depth_test,
            depth_write: s.depth_write,
            bias: s.depth_bias.map(|(f, u)| (f.to_bits(), u.to_bits())),
            stencil: s.stencil.map(|st| (st.front, st.back, st.read_mask & 0xff, st.write_mask & 0xff)),
            a2c: s.alpha_to_coverage,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct PipeKey {
    pub prog: u64,
    pub variants: Vec<Variant>,
    pub slots: Vec<SlotKey>,
    pub state: StateKey,
    pub colors: Vec<Option<wgpu::TextureFormat>>,
    pub ds: Option<wgpu::TextureFormat>,
    pub samples: u32,
    pub strip: Option<wgpu::IndexFormat>,
}

fn stages(vertex: bool, fragment: bool) -> wgpu::ShaderStages {
    match (vertex, fragment) {
        (true, false) => wgpu::ShaderStages::VERTEX,
        (false, true) => wgpu::ShaderStages::FRAGMENT,
        _ => wgpu::ShaderStages::VERTEX_FRAGMENT,
    }
}

impl Ctx {
    pub(crate) fn program_create(&mut self, g: &mut Gpu, id: ProgramId, mut desc: ProgramDesc) {
        self.programs.remove(&id);
        if id == 0 {
            return;
        }
        let prog = Self::build_program(g, &mut desc).map(|(vs, fs, bgl0)| {
            Rc::new(Prog { serial: g.next_serial(), desc, vs, fs, bgl0 })
        });
        if let Err(e) = &prog {
            g.log(|| format!("program {id} rejected: {e}"));
        }
        self.programs.insert(id, prog.ok());
    }

    fn build_program(
        g: &Gpu,
        desc: &mut ProgramDesc,
    ) -> Result<(wgpu::ShaderModule, wgpu::ShaderModule, wgpu::BindGroupLayout), String> {
        let l = &g.limits;
        if desc.uniform_bytes > l.max_uniform_buffer_binding_size
            || desc.blocks.iter().any(|b| b.binding == 0 || b.bytes == 0 || b.bytes > l.max_uniform_buffer_binding_size)
            || desc.blocks.len() + 1 > l.max_uniform_buffers_per_shader_stage as usize
            || desc.textures.len() > l.max_sampled_textures_per_shader_stage as usize
        {
            return Err("program exceeds device limits".into());
        }
        let module = |src: &str| {
            g.scoped(|| {
                g.dev.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("invisium-gl program"),
                    source: wgpu::ShaderSource::Wgsl(src.into()),
                })
            })
        };
        let vs = module(&desc.vertex_wgsl)?;
        let fs = module(&desc.fragment_wgsl)?;
        desc.vertex_wgsl = String::new();
        desc.fragment_wgsl = String::new();
        let mut entries = Vec::new();
        if desc.uniform_bytes > 0 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(desc.uniform_bytes as u64),
                },
                count: None,
            });
        }
        for b in &desc.blocks {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: b.binding,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            });
        }
        let bgl0 = g.scoped(|| {
            g.dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("invisium-gl group 0"), entries: &entries })
        })?;
        Ok((vs, fs, bgl0))
    }

    /// Bind group layout 1 and pipeline layout for a program bound with `variants`.
    fn layout(&mut self, g: &Gpu, p: &Prog, variants: &[Variant]) -> Option<(wgpu::BindGroupLayout, wgpu::PipelineLayout)> {
        let key = (p.serial, variants.to_vec());
        if let Some(l) = self.layouts.get(&key) {
            return l.clone();
        }
        let mut entries = Vec::new();
        for (slot, v) in p.desc.textures.iter().zip(variants) {
            let visibility = stages(slot.in_vertex, slot.in_fragment);
            let sample_type = match v {
                Variant::Filterable => wgpu::TextureSampleType::Float { filterable: true },
                Variant::Unfilterable => wgpu::TextureSampleType::Float { filterable: false },
                Variant::Shadow => wgpu::TextureSampleType::Depth,
                Variant::Sint => wgpu::TextureSampleType::Sint,
                Variant::Uint => wgpu::TextureSampleType::Uint,
            };
            let sampler = match v {
                Variant::Filterable => wgpu::SamplerBindingType::Filtering,
                Variant::Shadow => wgpu::SamplerBindingType::Comparison,
                _ => wgpu::SamplerBindingType::NonFiltering,
            };
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: slot.texture,
                visibility,
                ty: wgpu::BindingType::Texture { sample_type, view_dimension: res::view_dim(slot.kind), multisampled: false },
                count: None,
            });
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: slot.sampler,
                visibility,
                ty: wgpu::BindingType::Sampler(sampler),
                count: None,
            });
        }
        let made = g
            .scoped(|| {
                let bgl1 = g.dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("invisium-gl group 1"),
                    entries: &entries,
                });
                let layout = g.dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("invisium-gl layout"),
                    bind_group_layouts: &[&p.bgl0, &bgl1],
                    push_constant_ranges: &[],
                });
                (bgl1, layout)
            })
            .map_err(|e| g.log(|| format!("texture layout rejected: {e}")))
            .ok();
        self.layouts.insert(key, made.clone());
        made
    }

    /// Group 1 (textures and samplers) of a draw, with the variants it was built for.
    pub(crate) fn bind_textures(&mut self, g: &mut Gpu, p: &Prog, d: &Draw) -> Option<(Vec<Variant>, wgpu::BindGroup)> {
        let mut variants = Vec::with_capacity(p.desc.textures.len());
        let mut views = Vec::with_capacity(p.desc.textures.len());
        let mut samplers = Vec::with_capacity(p.desc.textures.len());
        let mut key = vec![p.serial];
        let default_sampling = Sampling {
            mag: Filter::Nearest,
            min: Filter::Nearest,
            mip: None,
            wrap: [Wrap::Clamp; 3],
            compare: None,
            lod_min: 0.0,
            lod_max: 0.0,
            base_level: 0,
            max_level: 0,
        };
        for (i, slot) in p.desc.textures.iter().enumerate() {
            let tb = d.textures.get(i).copied().unwrap_or(TexBind { texture: 0, sampling: default_sampling });
            let s = tb.sampling;
            let (variant, view, tex_serial) = match self.textures.get_mut(&tb.texture).filter(|_| tb.texture != 0) {
                None => {
                    let (v, class) = match slot.sample {
                        SampleKind::Float => (Variant::Filterable, Class::Float),
                        SampleKind::Shadow => (Variant::Shadow, Class::Depth),
                        SampleKind::Int => (Variant::Sint, Class::Sint),
                        SampleKind::Uint => (Variant::Uint, Class::Uint),
                    };
                    (v, res::dummy_view(g, slot.kind, class), 0)
                }
                Some(t) => {
                    if t.desc.kind != slot.kind || t.desc.samples > 1 {
                        return None;
                    }
                    let class = format::class(t.fmt);
                    let v = match (slot.sample, class) {
                        (SampleKind::Float, Class::Float) if format::filterable(t.fmt, g.features) => Variant::Filterable,
                        (SampleKind::Float, Class::Float | Class::Depth) => Variant::Unfilterable,
                        (SampleKind::Shadow, Class::Depth) => Variant::Shadow,
                        (SampleKind::Int, Class::Sint) => Variant::Sint,
                        (SampleKind::Uint, Class::Uint) => Variant::Uint,
                        _ => return None,
                    };
                    let last = t.desc.mips - 1;
                    let base = s.base_level.min(last);
                    let mips = if s.mip.is_some() { s.max_level.clamp(base, last) - base + 1 } else { 1 };
                    let aspect =
                        if class == Class::Depth { wgpu::TextureAspect::DepthOnly } else { wgpu::TextureAspect::All };
                    let vk = ViewKey {
                        dim: res::view_dim(slot.kind),
                        base_mip: base,
                        mips,
                        base_layer: 0,
                        layers: t.layers(),
                        aspect,
                    };
                    key.extend([base as u64, mips as u64]);
                    (v, t.view(vk), t.serial)
                }
            };
            let nearest = matches!(variant, Variant::Unfilterable | Variant::Sint | Variant::Uint);
            samplers.push(res::sampler(g, &s, nearest, variant == Variant::Shadow));
            let mut h = DefaultHasher::new();
            (s.mag, s.min, s.mip, s.wrap, s.compare, s.lod_min.to_bits(), s.lod_max.to_bits()).hash(&mut h);
            key.extend([tex_serial, variant as u64, h.finish()]);
            variants.push(variant);
            views.push(view);
        }
        let (bgl1, _) = self.layout(g, p, &variants)?;
        if let Some(bg) = self.bind_groups.get(&key) {
            return Some((variants, bg.clone()));
        }
        let mut entries = Vec::new();
        for (i, slot) in p.desc.textures.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry { binding: slot.texture, resource: wgpu::BindingResource::TextureView(&views[i]) });
            entries.push(wgpu::BindGroupEntry { binding: slot.sampler, resource: wgpu::BindingResource::Sampler(&samplers[i]) });
        }
        let bg = g
            .scoped(|| {
                g.dev.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("invisium-gl textures"), layout: &bgl1, entries: &entries })
            })
            .ok()?;
        if self.bind_groups.len() > 1024 {
            self.bind_groups.clear();
        }
        self.bind_groups.insert(key, bg.clone());
        Some((variants, bg))
    }

    /// The cached pipeline for `key`, created on first use; `None` if wgpu rejects it.
    pub(crate) fn pipeline(&mut self, g: &Gpu, p: &Prog, key: PipeKey, s: &DrawState, rt: &Rt) -> Option<wgpu::RenderPipeline> {
        if let Some(pl) = self.pipelines.get(&key) {
            return pl.clone();
        }
        let (_, layout) = self.layout(g, p, &key.variants)?;
        let attrs: Vec<Vec<wgpu::VertexAttribute>> = key
            .slots
            .iter()
            .map(|sl| {
                sl.attrs
                    .iter()
                    .map(|&(format, offset, shader_location)| wgpu::VertexAttribute { format, offset, shader_location })
                    .collect()
            })
            .collect();
        let buffers: Vec<wgpu::VertexBufferLayout> = key
            .slots
            .iter()
            .zip(&attrs)
            .map(|(sl, a)| wgpu::VertexBufferLayout { array_stride: sl.stride, step_mode: sl.step, attributes: a })
            .collect();
        let triangles = matches!(s.topology, Topology::Triangles | Topology::TriangleStrip);
        let strip = matches!(s.topology, Topology::LineStrip | Topology::TriangleStrip);
        let primitive = wgpu::PrimitiveState {
            topology: format::topology(s.topology),
            strip_index_format: if strip { key.strip } else { None },
            front_face: if s.front_ccw { wgpu::FrontFace::Ccw } else { wgpu::FrontFace::Cw },
            cull_mode: s.cull.map(|f| match f {
                Face::Front => wgpu::Face::Front,
                Face::Back => wgpu::Face::Back,
            }),
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        };
        let depth_stencil = rt.ds.as_ref().map(|(_, fmt)| {
            let has_depth = fmt.has_depth_aspect();
            let stencil = match (fmt.has_stencil_aspect(), s.stencil) {
                (true, Some(st)) => wgpu::StencilState {
                    front: format::stencil_face(&st.front),
                    back: format::stencil_face(&st.back),
                    read_mask: st.read_mask & 0xff,
                    write_mask: st.write_mask & 0xff,
                },
                _ => wgpu::StencilState::default(),
            };
            let bias = match s.depth_bias {
                Some((f, u)) if triangles && has_depth && f.is_finite() && u.is_finite() => {
                    wgpu::DepthBiasState { constant: u.clamp(-1e9, 1e9) as i32, slope_scale: f, clamp: 0.0 }
                }
                _ => wgpu::DepthBiasState::default(),
            };
            wgpu::DepthStencilState {
                format: *fmt,
                depth_write_enabled: has_depth && s.depth_write && s.depth_test.is_some(),
                depth_compare: s.depth_test.filter(|_| has_depth).map_or(wgpu::CompareFunction::Always, format::compare),
                stencil,
                bias,
            }
        });
        let mask = s.color_mask;
        let mut write_mask = wgpu::ColorWrites::empty();
        for (on, bit) in mask.iter().zip([wgpu::ColorWrites::RED, wgpu::ColorWrites::GREEN, wgpu::ColorWrites::BLUE, wgpu::ColorWrites::ALPHA]) {
            if *on {
                write_mask |= bit;
            }
        }
        let targets: Vec<Option<wgpu::ColorTargetState>> = rt
            .colors
            .iter()
            .enumerate()
            .map(|(i, c)| {
                c.as_ref().map(|c| {
                    let written = p.desc.outputs.iter().any(|o| o.location == i as u32);
                    let blend = s
                        .blend
                        .as_ref()
                        .filter(|_| format::class(c.fmt) == Class::Float && format::blendable(c.fmt, g.features))
                        .map(format::blend);
                    wgpu::ColorTargetState {
                        format: c.fmt,
                        blend,
                        write_mask: if written { write_mask } else { wgpu::ColorWrites::empty() },
                    }
                })
            })
            .collect();
        let made = g
            .scoped(|| {
                g.dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("invisium-gl pipeline"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &p.vs,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &buffers,
                    },
                    primitive,
                    depth_stencil,
                    multisample: wgpu::MultisampleState {
                        count: rt.samples,
                        mask: !0,
                        alpha_to_coverage_enabled: s.alpha_to_coverage && rt.samples > 1,
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &p.fs,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &targets,
                    }),
                    multiview: None,
                    cache: None,
                })
            })
            .map_err(|e| g.log(|| format!("pipeline rejected: {e}")))
            .ok();
        self.pipelines.insert(key, made.clone());
        made
    }
}
