//! Internal pipelines: drawn clears, and the copy shader used by present, blit, copies and mips.

use super::format::{self, Class};
use super::pass::Rt;
use super::Gpu;
use crate::draw::gl::ClearColor;
use std::collections::HashMap;
use wgpu::util::DeviceExt;

pub(crate) const SURFACE_DS: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;
pub(crate) const CLEAR_UNIFORM_BYTES: u64 = 144;

#[derive(Default)]
pub(crate) struct Helpers {
    clear_bgl: Option<(wgpu::BindGroupLayout, wgpu::PipelineLayout)>,
    clear: HashMap<ClearKey, Option<wgpu::RenderPipeline>>,
    copy_bgl: HashMap<(Class, bool, bool), (wgpu::BindGroupLayout, wgpu::PipelineLayout)>,
    copy: HashMap<CopyKey, Option<wgpu::RenderPipeline>>,
}

const FULLSCREEN_VS: &str = "
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(p, DEPTH, 1.0);
}
";

fn wgsl_type(c: Class) -> &'static str {
    match c {
        Class::Sint => "i32",
        Class::Uint => "u32",
        _ => "f32",
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct ClearKey {
    /// Per attachment: format and write mask bits (0 when this clear does not touch it).
    colors: Vec<Option<(wgpu::TextureFormat, u32)>>,
    /// Format, whether depth is written, stencil write mask.
    ds: Option<(wgpu::TextureFormat, bool, u32)>,
    samples: u32,
}

impl ClearKey {
    pub fn new(rt: &Rt, values: &[Option<ClearColor>], mask: [bool; 4], depth: bool, stencil: Option<u32>) -> ClearKey {
        let bits = mask.iter().enumerate().filter(|(_, m)| **m).fold(0u32, |a, (i, _)| a | 1 << i);
        ClearKey {
            colors: rt
                .colors
                .iter()
                .zip(values)
                .map(|(c, v)| c.as_ref().map(|c| (c.fmt, if v.is_some() { bits } else { 0 })))
                .collect(),
            ds: rt.ds.as_ref().map(|d| (d.1, depth, stencil.unwrap_or(0))),
            samples: rt.samples,
        }
    }
}

pub(crate) fn load_color(v: ClearColor) -> wgpu::Color {
    let [r, g, b, a] = match v {
        ClearColor::F(f) => f.map(|x| if x.is_finite() { x as f64 } else { 0.0 }),
        ClearColor::I(i) => i.map(|x| x as f64),
        ClearColor::U(u) => u.map(|x| x as f64),
    };
    wgpu::Color { r, g, b, a }
}

pub(crate) fn clear_uniform(values: &[Option<ClearColor>], depth: f32) -> Vec<u8> {
    let mut out = vec![0u8; CLEAR_UNIFORM_BYTES as usize];
    for (i, v) in values.iter().enumerate().take(8) {
        let words: [u32; 4] = match v {
            Some(ClearColor::F(f)) => f.map(f32::to_bits),
            Some(ClearColor::I(x)) => x.map(|v| v as u32),
            Some(ClearColor::U(u)) => *u,
            None => [0; 4],
        };
        for (k, w) in words.iter().enumerate() {
            out[i * 16 + k * 4..i * 16 + k * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
    }
    let d = if depth.is_finite() { depth.clamp(0.0, 1.0) } else { 0.0 };
    out[128..132].copy_from_slice(&d.to_le_bytes());
    out
}

fn clear_layouts(g: &mut Gpu) -> (wgpu::BindGroupLayout, wgpu::PipelineLayout) {
    let dev = &g.dev;
    g.helpers
        .clear_bgl
        .get_or_insert_with(|| {
            let bgl = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("invisium-gl clear"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(CLEAR_UNIFORM_BYTES),
                    },
                    count: None,
                }],
            });
            let layout = dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("invisium-gl clear"),
                bind_group_layouts: &[&bgl],
                push_constant_ranges: &[],
            });
            (bgl, layout)
        })
        .clone()
}

pub(crate) fn clear_bind_group(g: &mut Gpu, staging: &wgpu::Buffer) -> Option<wgpu::BindGroup> {
    let (bgl, _) = clear_layouts(g);
    g.scoped(|| {
        g.dev.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("invisium-gl clear"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: staging,
                    offset: 0,
                    size: wgpu::BufferSize::new(CLEAR_UNIFORM_BYTES),
                }),
            }],
        })
    })
    .ok()
}

pub(crate) fn clear_pipeline(g: &mut Gpu, key: &ClearKey) -> Option<wgpu::RenderPipeline> {
    if let Some(p) = g.helpers.clear.get(key) {
        return p.clone();
    }
    let (_, layout) = clear_layouts(g);
    let mut src = String::from(
        "struct U { c: array<vec4<u32>, 8>, depth: vec4<f32> };\n@group(0) @binding(0) var<uniform> u: U;\n",
    );
    src.push_str(&FULLSCREEN_VS.replace("DEPTH", "u.depth.x"));
    let written: Vec<(usize, Class)> = key
        .colors
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.filter(|c| c.1 != 0).map(|c| (i, format::class(c.0))))
        .collect();
    if written.is_empty() {
        src.push_str("@fragment fn fs() {}\n");
    } else {
        src.push_str("struct O {\n");
        for (i, c) in &written {
            src.push_str(&format!("  @location({i}) c{i}: vec4<{}>,\n", wgsl_type(*c)));
        }
        src.push_str("};\n@fragment fn fs() -> O {\n  var o: O;\n");
        for (i, c) in &written {
            src.push_str(&format!("  o.c{i} = bitcast<vec4<{}>>(u.c[{i}]);\n", wgsl_type(*c)));
        }
        src.push_str("  return o;\n}\n");
    }
    let targets: Vec<Option<wgpu::ColorTargetState>> = key
        .colors
        .iter()
        .map(|c| {
            c.map(|(format, bits)| wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::from_bits_truncate(bits),
            })
        })
        .collect();
    let depth_stencil = key.ds.map(|(format, depth, smask)| {
        let face = wgpu::StencilFaceState {
            compare: wgpu::CompareFunction::Always,
            fail_op: wgpu::StencilOperation::Keep,
            depth_fail_op: wgpu::StencilOperation::Keep,
            pass_op: wgpu::StencilOperation::Replace,
        };
        wgpu::DepthStencilState {
            format,
            depth_write_enabled: depth,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: wgpu::StencilState { front: face, back: face, read_mask: 0xff, write_mask: smask },
            bias: Default::default(),
        }
    });
    let made = g
        .scoped(|| {
            let module = g.dev.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("invisium-gl clear"),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            });
            g.dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("invisium-gl clear"),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: Default::default(),
                depth_stencil,
                multisample: wgpu::MultisampleState { count: key.samples, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                multiview: None,
                cache: None,
            })
        })
        .map_err(|e| g.log(|| format!("clear pipeline rejected: {e}")))
        .ok();
    g.helpers.clear.insert(key.clone(), made.clone());
    made
}

/// Copy shader parameters (texel space).
#[derive(Clone, Copy, Default)]
pub(crate) struct CopyU {
    pub src0: [f32; 2],
    pub src1: [f32; 2],
    pub dst0: [f32; 2],
    pub dst1: [f32; 2],
    pub size: [f32; 2],
    pub premultiply: bool,
    pub opaque: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct CopyKey {
    dst: wgpu::TextureFormat,
    class: Class,
    linear: bool,
    msaa: bool,
}

fn copy_layouts(g: &mut Gpu, class: Class, linear: bool, msaa: bool) -> (wgpu::BindGroupLayout, wgpu::PipelineLayout) {
    let dev = &g.dev;
    g.helpers
        .copy_bgl
        .entry((class, linear, msaa))
        .or_insert_with(|| {
            let sample_type = match class {
                Class::Sint => wgpu::TextureSampleType::Sint,
                Class::Uint => wgpu::TextureSampleType::Uint,
                _ => wgpu::TextureSampleType::Float { filterable: linear },
            };
            let frag = wgpu::ShaderStages::FRAGMENT;
            let mut entries = vec![
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: frag,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: frag,
                    ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: msaa },
                    count: None,
                },
            ];
            if linear {
                entries.push(wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: frag,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                });
            }
            let bgl = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("invisium-gl copy"), entries: &entries });
            let layout = dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("invisium-gl copy"),
                bind_group_layouts: &[&bgl],
                push_constant_ranges: &[],
            });
            (bgl, layout)
        })
        .clone()
}

fn copy_pipeline(g: &mut Gpu, key: CopyKey) -> Option<wgpu::RenderPipeline> {
    if let Some(p) = g.helpers.copy.get(&key) {
        return p.clone();
    }
    let (_, layout) = copy_layouts(g, key.class, key.linear, key.msaa);
    let t = wgsl_type(key.class);
    let tex = if key.msaa { "texture_multisampled_2d<f32>".to_string() } else { format!("texture_2d<{t}>") };
    let out = wgsl_type(format::class(key.dst));
    let fetch = if key.linear {
        "textureSampleLevel(t, s, sc / u.size, 0.0)".to_string()
    } else if key.msaa {
        "(textureLoad(t, ip, 0) + textureLoad(t, ip, 1) + textureLoad(t, ip, 2) + textureLoad(t, ip, 3)) * 0.25".to_string()
    } else {
        "textureLoad(t, ip, 0)".to_string()
    };
    let src = format!(
        "struct U {{ src0: vec2<f32>, src1: vec2<f32>, dst0: vec2<f32>, dst1: vec2<f32>, size: vec2<f32>, flags: vec2<u32> }};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var t: {tex};
{sampler}
{vs}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<{out}> {{
    let k = (p.xy - u.dst0) / (u.dst1 - u.dst0);
    let sc = u.src0 + k * (u.src1 - u.src0);
    if (any(sc < vec2<f32>(0.0)) || any(sc >= u.size)) {{ discard; }}
    let ip = vec2<i32>(floor(sc));
    var c = {fetch};
    {post}
    return vec4<{out}>(c);
}}
",
        sampler = if key.linear { "@group(0) @binding(2) var s: sampler;" } else { "" },
        vs = FULLSCREEN_VS.replace("DEPTH", "0.0"),
        post = if key.class == Class::Float {
            "if (u.flags.x != 0u) { c = vec4<f32>(c.rgb * c.a, c.a); }\n    if (u.flags.y != 0u) { c.a = 1.0; }"
        } else {
            ""
        },
    );
    let made = g
        .scoped(|| {
            let module = g.dev.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("invisium-gl copy"),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            });
            g.dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("invisium-gl copy"),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: key.dst, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview: None,
                cache: None,
            })
        })
        .map_err(|e| g.log(|| format!("copy pipeline rejected: {e}")))
        .ok();
    g.helpers.copy.insert(key, made.clone());
    made
}

/// Draw `src` (a single-level 2D view, or a 4x multisampled one) into the `scissor` part of
/// `dst` with the mapping in `u`. Returns false if it could not be encoded.
#[allow(clippy::too_many_arguments)]
pub(crate) fn copy_draw(
    g: &mut Gpu,
    enc: &mut wgpu::CommandEncoder,
    src: &wgpu::TextureView,
    src_class: Class,
    msaa: bool,
    dst: &wgpu::TextureView,
    dst_fmt: wgpu::TextureFormat,
    dst_size: [u32; 2],
    u: CopyU,
    scissor: [u32; 4],
    linear: bool,
) -> bool {
    let linear = linear && !msaa && src_class == Class::Float;
    let key = CopyKey { dst: dst_fmt, class: if msaa { Class::Float } else { src_class }, linear, msaa };
    let Some(pipeline) = copy_pipeline(g, key) else { return false };
    let (bgl, _) = copy_layouts(g, key.class, linear, msaa);
    let mut words: Vec<u32> = [u.src0, u.src1, u.dst0, u.dst1, u.size].iter().flatten().map(|f| f.to_bits()).collect();
    words.extend([u.premultiply as u32, u.opaque as u32]);
    let ubuf = g.dev.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("invisium-gl copy params"),
        contents: bytemuck::cast_slice(&words),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let sampler = linear.then(|| {
        g.dev.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        })
    });
    let mut entries = vec![
        wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(src) },
    ];
    if let Some(s) = &sampler {
        entries.push(wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(s) });
    }
    let Ok(bg) = g.scoped(|| {
        g.dev.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("invisium-gl copy"), layout: &bgl, entries: &entries })
    }) else {
        return false;
    };
    {
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("invisium-gl copy"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        rp.set_pipeline(&pipeline);
        rp.set_bind_group(0, &bg, &[]);
        rp.set_viewport(0.0, 0.0, dst_size[0] as f32, dst_size[1] as f32, 0.0, 1.0);
        rp.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
        rp.draw(0..3, 0..1);
    }
    g.passes += 1;
    true
}
