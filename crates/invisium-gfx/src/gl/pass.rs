//! Render targets, pass batching, clears and draws.

use super::format::{self, Class};
use super::program::{PipeKey, StateKey};
use super::util::{self, ClearKey};
use super::{Ctx, Gpu};
use crate::draw::gl::*;
use std::rc::Rc;

/// Most vertex shader invocations (`count * instances`) one draw may ask for. WebGL accepts
/// draws of up to 2^31-1 vertices (even without attributes), and a GPU busy with billions of
/// invocations hangs the browser or triggers a device reset; 2^27 (134M) is far beyond any real
/// scene, so larger draws are refused and draw nothing.
const MAX_DRAW_VERTICES: u64 = 1 << 27;

pub(crate) struct RtColor {
    pub view: wgpu::TextureView,
    pub fmt: wgpu::TextureFormat,
}

/// A resolved `Target`.
pub(crate) struct Rt {
    pub colors: Vec<Option<RtColor>>,
    pub ds: Option<(wgpu::TextureView, wgpu::TextureFormat)>,
    pub size: [u32; 2],
    pub samples: u32,
    pub textures: Vec<TextureId>,
}

pub(crate) struct ClearReq {
    pub colors: Vec<(u32, ClearColor)>,
    pub depth: Option<f32>,
    pub stencil: Option<u32>,
    pub color_mask: [bool; 4],
    pub stencil_mask: u32,
    pub scissor: Option<Rect4>,
}

enum BlockSrc {
    Buf(wgpu::Buffer, u64, u64),
    Staged(u32, u32),
}

struct PDraw {
    pipeline: wgpu::RenderPipeline,
    bgl0: wgpu::BindGroupLayout,
    g0_key: Vec<u64>,
    uni: Option<(u32, u32)>,
    blocks: Vec<(u32, BlockSrc)>,
    g1: wgpu::BindGroup,
    vbufs: Vec<Option<(wgpu::Buffer, u64)>>,
    const_off: u32,
    index: Option<(wgpu::Buffer, u64, wgpu::IndexFormat)>,
    viewport: [f32; 6],
    scissor: [u32; 4],
    blend: wgpu::Color,
    stencil_ref: u32,
    first: u32,
    count: u32,
    instances: u32,
}

enum Op {
    Draw(Box<PDraw>),
    Clear { pipeline: wgpu::RenderPipeline, offset: u32, scissor: [u32; 4], stencil_ref: u32 },
}

pub(crate) struct Pending {
    target: Target,
    rt: Rt,
    color_load: Vec<Option<wgpu::Color>>,
    depth_load: Option<f32>,
    stencil_load: Option<u32>,
    /// Uniforms, constant attributes and unaligned blocks of this pass.
    staging: Vec<u8>,
    last_uni: Option<(Rc<[u8]>, u32)>,
    ops: Vec<Op>,
}

impl Pending {
    fn stage(&mut self, bytes: &[u8], align: usize) -> u32 {
        let at = self.staging.len().div_ceil(align) * align;
        self.staging.resize(at, 0);
        self.staging.extend_from_slice(bytes);
        at as u32
    }
}

#[derive(Default)]
pub(crate) struct Seg {
    enc: Option<wgpu::CommandEncoder>,
    pass: Option<Pending>,
}

impl Seg {
    pub fn encoder(&mut self, g: &Gpu) -> &mut wgpu::CommandEncoder {
        self.enc.get_or_insert_with(|| g.dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("invisium-gl") }))
    }
}

/// `r` clamped to a `w` x `h` target as `[x, y, w, h]`; `None` if empty.
pub(crate) fn clamp_rect(r: Rect4, w: u32, h: u32) -> Option<[u32; 4]> {
    let x0 = (r[0] as i64).clamp(0, w as i64);
    let y0 = (r[1] as i64).clamp(0, h as i64);
    let x1 = (r[0] as i64 + r[2].max(0) as i64).clamp(0, w as i64);
    let y1 = (r[1] as i64 + r[3].max(0) as i64).clamp(0, h as i64);
    (x1 > x0 && y1 > y0).then(|| [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32])
}

impl Ctx {
    pub(crate) fn resolve_target(&mut self, g: &Gpu, t: &Target) -> Option<Rt> {
        match t {
            Target::Surface => {
                let s = self.surface.as_ref()?;
                Some(Rt {
                    colors: vec![Some(RtColor { view: s.color_view.clone(), fmt: wgpu::TextureFormat::Rgba8Unorm })],
                    ds: s.ds.as_ref().map(|(_, v)| (v.clone(), util::SURFACE_DS)),
                    size: [s.cfg.width, s.cfg.height],
                    samples: s.cfg.samples,
                    textures: Vec::new(),
                })
            }
            Target::Fbo { color, depth_stencil } => {
                if color.len() > g.limits.max_color_attachments as usize {
                    return None;
                }
                let mut size = None;
                let mut samples = 0;
                let mut used: Vec<Attachment> = Vec::new();
                let mut textures = Vec::new();
                let mut attach = |me: &mut Ctx, a: &Attachment, want_color: bool| -> Option<(wgpu::TextureView, wgpu::TextureFormat)> {
                    if used.contains(a) {
                        return None;
                    }
                    used.push(*a);
                    let t = me.textures.get_mut(&a.texture)?;
                    if !t.renderable || t.desc.format.is_color() != want_color || a.level >= t.desc.mips || a.layer >= t.layers() {
                        return None;
                    }
                    let [w, h, _] = t.level_size(a.level);
                    if *size.get_or_insert([w, h]) != [w, h] {
                        return None;
                    }
                    if samples == 0 {
                        samples = t.desc.samples;
                    } else if samples != t.desc.samples {
                        return None;
                    }
                    textures.push(a.texture);
                    Some((t.image_view(a.level, a.layer), t.fmt))
                };
                let mut colors = Vec::with_capacity(color.len());
                for c in color {
                    colors.push(match c {
                        Some(a) => Some(attach(self, a, true).map(|(view, fmt)| RtColor { view, fmt })?),
                        None => None,
                    });
                }
                let ds = match depth_stencil {
                    Some(a) => Some(attach(self, a, false)?),
                    None => None,
                };
                Some(Rt { colors, ds, size: size?, samples, textures })
            }
        }
    }

    /// Make the pending pass draw into `target`, flushing a pass on another target.
    fn ensure_pass(&mut self, g: &mut Gpu, target: &Target, rt: Rt) -> &mut Pending {
        if self.seg.pass.as_ref().is_some_and(|p| p.target != *target) {
            self.flush_pass(g);
        }
        self.seg.pass.get_or_insert_with(|| Pending {
            target: target.clone(),
            color_load: rt.colors.iter().map(|_| None).collect(),
            rt,
            depth_load: None,
            stencil_load: None,
            staging: Vec::new(),
            last_uni: None,
            ops: Vec::new(),
        })
    }

    pub(crate) fn clear(&mut self, g: &mut Gpu, target: &Target, c: ClearReq) {
        let Some(rt) = self.resolve_target(g, target) else { return };
        let [w, h] = rt.size;
        let full = [0, 0, w, h];
        let Some(sc) = (match c.scissor {
            Some(r) => clamp_rect(r, w, h),
            None => Some(full),
        }) else {
            return;
        };
        let opaque_surface = *target == Target::Surface && self.surface.as_ref().is_some_and(|s| !s.cfg.alpha);
        let mut mask = c.color_mask;
        if opaque_surface {
            mask[3] = true; // the alpha channel of an opaque drawing buffer always holds 1
        }
        // Colour values per attachment, dropping those whose class does not match.
        let mut values: Vec<Option<ClearColor>> = rt.colors.iter().map(|_| None).collect();
        for &(i, v) in &c.colors {
            let Some(Some(att)) = rt.colors.get(i as usize) else { continue };
            let ok = matches!(
                (format::class(att.fmt), v),
                (Class::Float, ClearColor::F(_)) | (Class::Sint, ClearColor::I(_)) | (Class::Uint, ClearColor::U(_))
            );
            if ok && c.color_mask.iter().any(|&m| m) {
                let v = match v {
                    ClearColor::F(mut f) if opaque_surface => {
                        f[3] = 1.0;
                        ClearColor::F(f)
                    }
                    v => v,
                };
                values[i as usize] = Some(v);
            }
        }
        let ds_fmt = rt.ds.as_ref().map(|d| d.1);
        let depth = c.depth.filter(|_| ds_fmt.is_some_and(|f| f.has_depth_aspect()));
        let stencil = c.stencil.filter(|_| ds_fmt.is_some_and(|f| f.has_stencil_aspect()) && c.stencil_mask & 0xff != 0);
        if values.iter().all(Option::is_none) && depth.is_none() && stencil.is_none() {
            return;
        }
        let loadable = sc == full
            && (values.iter().all(Option::is_none) || mask == [true; 4])
            && stencil.is_none_or(|_| c.stencil_mask & 0xff == 0xff);
        let at_start = self.seg.pass.as_ref().is_some_and(|p| p.target == *target && p.ops.is_empty());
        if loadable && (at_start || self.seg.pass.as_ref().is_none_or(|p| p.target != *target)) {
            let p = self.ensure_pass(g, target, rt);
            for (i, v) in values.iter().enumerate() {
                if let Some(v) = v {
                    p.color_load[i] = Some(util::load_color(*v));
                }
            }
            if let Some(d) = depth {
                p.depth_load = Some(if d.is_finite() { d.clamp(0.0, 1.0) } else { 0.0 });
            }
            if let Some(s) = stencil {
                p.stencil_load = Some(s & 0xff);
            }
            return;
        }
        let key = ClearKey::new(&rt, &values, mask, depth.is_some(), stencil.map(|_| c.stencil_mask & 0xff));
        let Some(pipeline) = util::clear_pipeline(g, &key) else { return };
        let bytes = util::clear_uniform(&values, depth.unwrap_or(0.0));
        let align = g.limits.min_uniform_buffer_offset_alignment as usize;
        let p = self.ensure_pass(g, target, rt);
        let offset = p.stage(&bytes, align);
        p.ops.push(Op::Clear { pipeline, offset, scissor: sc, stencil_ref: stencil.unwrap_or(0) & 0xff });
    }

    pub(crate) fn draw(&mut self, g: &mut Gpu, d: &Draw) {
        if d.count == 0 || d.instances == 0 {
            return;
        }
        if d.count as u64 * d.instances as u64 > MAX_DRAW_VERTICES || d.first.checked_add(d.count).is_none() {
            g.log(|| format!("draw of {} vertices x {} instances refused", d.count, d.instances));
            return;
        }
        let Some(Some(p)) = self.programs.get(&d.program).cloned() else { return };
        let Some(rt) = self.resolve_target(g, &d.target) else { return };
        if d.textures.iter().any(|t| t.texture != 0 && rt.textures.contains(&t.texture)) {
            return; // feedback loop
        }
        for o in &p.desc.outputs {
            if let Some(Some(c)) = rt.colors.get(o.location as usize) {
                if format::scalar_of(format::class(c.fmt)) != Some(o.base) {
                    return;
                }
            }
        }
        let [w, h] = rt.size;
        let s = &d.state;
        let Some(scissor) = (match s.scissor {
            Some(r) => clamp_rect(r, w, h),
            None => Some([0, 0, w, h]),
        }) else {
            return;
        };
        // wgpu needs the viewport inside the target: clamp it (see the module docs).
        let Some(vp) = clamp_rect(s.viewport, w, h) else { return };
        let dr = s.depth_range.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
        // Blocks bound at unaligned offsets are copied from the CPU copy: make it first.
        let align = g.limits.min_uniform_buffer_offset_alignment as u64;
        for bb in &d.blocks {
            if !(bb.offset as u64).is_multiple_of(align) && p.desc.blocks.iter().any(|s| s.binding == bb.binding) {
                self.ensure_copy(g, bb.buffer);
            }
        }
        let mut tries = 0;
        let vs = loop {
            match self.vertex_setup(g, &p, d) {
                Ok(vs) => break vs,
                Err(Some(id)) if tries < 16 && self.ensure_copy(g, id) => tries += 1,
                Err(_) => return,
            }
        };
        let Some((variants, g1)) = self.bind_textures(g, &p, d) else { return };
        let max_binding = g.limits.max_uniform_buffer_binding_size as u64;
        let mut blocks = Vec::new();
        let mut staged_blocks = Vec::new();
        for slot in &p.desc.blocks {
            let bb = d.blocks.iter().find(|b| b.binding == slot.binding);
            let Some(bb) = bb else { return };
            let Some(buf) = self.buffers.get(&bb.buffer) else { return };
            let (off, size) = (bb.offset as u64, (bb.size as u64).min(max_binding));
            if size < slot.bytes as u64 || off + size > buf.len as u64 {
                return;
            }
            if off.is_multiple_of(align) {
                blocks.push((slot.binding, BlockSrc::Buf(buf.gpu.clone(), off, size), buf.serial));
            } else {
                let Some(data) = &buf.data else { return };
                staged_blocks.push((slot.binding, data[off as usize..(off + size) as usize].to_vec()));
            }
        }
        let key = PipeKey {
            prog: p.serial,
            variants,
            slots: vs.slots,
            state: StateKey::new(s),
            colors: rt.colors.iter().map(|c| c.as_ref().map(|c| c.fmt)).collect(),
            ds: rt.ds.as_ref().map(|d| d.1),
            samples: rt.samples,
            strip: vs.index.as_ref().map(|i| i.2),
        };
        let Some(pipeline) = self.pipeline(g, &p, key, s, &rt) else { return };
        let blend = format::blend_constant(&s.blend, s.blend_color);
        let stencil_ref = s.stencil.map_or(0, |st| st.reference & 0xff);
        let align = align as usize;
        let pend = self.ensure_pass(g, &d.target, rt);
        let mut g0_key = vec![p.serial];
        let uni = (p.desc.uniform_bytes > 0).then(|| {
            let n = p.desc.uniform_bytes;
            let off = match &pend.last_uni {
                Some((rc, off)) if Rc::ptr_eq(rc, &d.uniforms) => *off,
                _ => {
                    let mut bytes = d.uniforms.to_vec();
                    bytes.resize(n as usize, 0);
                    let off = pend.stage(&bytes, align);
                    pend.last_uni = Some((d.uniforms.clone(), off));
                    off
                }
            };
            (off, n)
        });
        let mut all_blocks = Vec::new();
        for (binding, src, serial) in blocks {
            if let BlockSrc::Buf(_, off, size) = &src {
                g0_key.extend([binding as u64, serial, *off, *size]);
            }
            all_blocks.push((binding, src));
        }
        for (binding, bytes) in staged_blocks {
            let off = pend.stage(&bytes, align);
            g0_key.extend([binding as u64, u64::MAX, off as u64, bytes.len() as u64]);
            all_blocks.push((binding, BlockSrc::Staged(off, bytes.len() as u32)));
        }
        let const_off = if vs.consts.is_empty() { 0 } else { pend.stage(&vs.consts, 16) };
        pend.ops.push(Op::Draw(Box::new(PDraw {
            pipeline,
            bgl0: p.bgl0.clone(),
            g0_key,
            uni,
            blocks: all_blocks,
            g1,
            vbufs: vs.bufs,
            const_off,
            index: vs.index,
            viewport: [vp[0] as f32, vp[1] as f32, vp[2] as f32, vp[3] as f32, dr[0], dr[1]],
            scissor,
            blend,
            stencil_ref,
            first: d.first,
            count: d.count,
            instances: d.instances,
        })));
    }

    /// Encode the pending pass (if any) into the segment's command encoder.
    pub(crate) fn flush_pass(&mut self, g: &mut Gpu) {
        let Some(p) = self.seg.pass.take() else { return };
        let staging = (!p.staging.is_empty()).then(|| {
            use wgpu::util::DeviceExt;
            g.dev.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("invisium-gl uniforms"),
                contents: &p.staging,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::VERTEX,
            })
        });
        let mut cache: std::collections::HashMap<Vec<u64>, Option<wgpu::BindGroup>> = Default::default();
        let mut g0s = Vec::with_capacity(p.ops.len());
        for op in &p.ops {
            g0s.push(match (op, &staging) {
                (Op::Draw(d), st) => cache
                    .entry(d.g0_key.clone())
                    .or_insert_with(|| {
                        let mut entries = Vec::new();
                        let binding = |buffer, offset, size: u64| {
                            wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer, offset, size: wgpu::BufferSize::new(size) })
                        };
                        if let (Some((_, n)), Some(st)) = (d.uni, st) {
                            entries.push(wgpu::BindGroupEntry { binding: 0, resource: binding(st, 0, n as u64) });
                        }
                        for (b, src) in &d.blocks {
                            let resource = match (src, st) {
                                (BlockSrc::Buf(buf, off, size), _) => binding(buf, *off, *size),
                                (BlockSrc::Staged(off, size), Some(st)) => binding(st, *off as u64, *size as u64),
                                _ => return None,
                            };
                            entries.push(wgpu::BindGroupEntry { binding: *b, resource });
                        }
                        g.scoped(|| {
                            g.dev.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &d.bgl0, entries: &entries })
                        })
                        .ok()
                    })
                    .clone(),
                (Op::Clear { .. }, Some(st)) => cache
                    .entry(vec![u64::MAX])
                    .or_insert_with(|| util::clear_bind_group(g, st))
                    .clone(),
                _ => None,
            });
        }
        let enc = self.seg.encoder(g);
        {
            let color_attachments: Vec<Option<wgpu::RenderPassColorAttachment>> = p
                .rt
                .colors
                .iter()
                .zip(&p.color_load)
                .map(|(c, load)| {
                    c.as_ref().map(|c| wgpu::RenderPassColorAttachment {
                        view: &c.view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: load.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                            store: wgpu::StoreOp::Store,
                        },
                    })
                })
                .collect();
            let ds = p.rt.ds.as_ref().map(|(view, fmt)| wgpu::RenderPassDepthStencilAttachment {
                view,
                depth_ops: fmt.has_depth_aspect().then(|| wgpu::Operations {
                    load: p.depth_load.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: fmt.has_stencil_aspect().then(|| wgpu::Operations {
                    load: p.stencil_load.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                    store: wgpu::StoreOp::Store,
                }),
            });
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("invisium-gl pass"),
                color_attachments: &color_attachments,
                depth_stencil_attachment: ds,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            for (op, g0) in p.ops.iter().zip(g0s) {
                let Some(g0) = g0 else { continue };
                match op {
                    Op::Clear { pipeline, offset, scissor: s, stencil_ref } => {
                        rp.set_pipeline(pipeline);
                        rp.set_bind_group(0, &g0, &[*offset]);
                        rp.set_viewport(0.0, 0.0, p.rt.size[0] as f32, p.rt.size[1] as f32, 0.0, 1.0);
                        rp.set_scissor_rect(s[0], s[1], s[2], s[3]);
                        rp.set_stencil_reference(*stencil_ref);
                        rp.draw(0..3, 0..1);
                    }
                    Op::Draw(d) => {
                        rp.set_pipeline(&d.pipeline);
                        let dyn_off: Vec<u32> = d.uni.iter().map(|u| u.0).collect();
                        rp.set_bind_group(0, &g0, &dyn_off);
                        rp.set_bind_group(1, &d.g1, &[]);
                        for (i, vb) in d.vbufs.iter().enumerate() {
                            match (vb, &staging) {
                                (Some((buf, off)), _) => rp.set_vertex_buffer(i as u32, buf.slice(*off..)),
                                (None, Some(st)) => rp.set_vertex_buffer(i as u32, st.slice(d.const_off as u64..)),
                                (None, None) => {}
                            }
                        }
                        let [x, y, w, h, n, f] = d.viewport;
                        rp.set_viewport(x, y, w, h, n, f);
                        rp.set_scissor_rect(d.scissor[0], d.scissor[1], d.scissor[2], d.scissor[3]);
                        rp.set_blend_constant(d.blend);
                        rp.set_stencil_reference(d.stencil_ref);
                        match &d.index {
                            Some((buf, off, fmt)) => {
                                rp.set_index_buffer(buf.slice(*off..), *fmt);
                                rp.draw_indexed(d.first..d.first + d.count, 0, 0..d.instances);
                            }
                            None => rp.draw(d.first..d.first + d.count, 0..d.instances),
                        }
                    }
                }
            }
        }
        g.passes += 1;
    }

    /// Flush the pending pass and submit everything encoded so far.
    pub(crate) fn submit(&mut self, g: &mut Gpu) {
        self.flush_pass(g);
        if let Some(enc) = self.seg.enc.take() {
            g.queue.submit([enc.finish()]);
        }
    }

    /// Queue writes run before the next submission: submit earlier commands first.
    pub(crate) fn before_queue_write(&mut self, g: &mut Gpu) {
        if self.seg.pass.is_some() || self.seg.enc.is_some() {
            self.submit(g);
        }
    }
}
