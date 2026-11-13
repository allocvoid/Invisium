//! Drawing buffer set-up, present, blits, copies, mip generation and reads.

use super::format::{self, Class};
use super::pass::clamp_rect;
use super::res::{Presented, Surface, SurfaceCfg};
use super::util::{self, CopyU, SURFACE_DS};
use super::{Ctx, Gpu, MAX_ALLOC};
use crate::draw::gl::*;

/// One image (level, layer) of a texture or of the drawing buffer.
struct Img {
    tex: wgpu::Texture,
    level: u32,
    layer: u32,
    fmt: wgpu::TextureFormat,
    tf: TexFormat,
    size: [u32; 2],
    samples: u32,
    /// Identifies the texture (the drawing buffer uses 0).
    id: TextureId,
}

impl Img {
    fn view(&self) -> wgpu::TextureView {
        self.tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_mip_level: self.level,
            mip_level_count: Some(1),
            base_array_layer: self.layer,
            array_layer_count: Some(1),
            ..Default::default()
        })
    }

    fn copy_info(&self, x: u32, y: u32) -> wgpu::TexelCopyTextureInfo<'_> {
        wgpu::TexelCopyTextureInfo {
            texture: &self.tex,
            mip_level: self.level,
            origin: wgpu::Origin3d { x, y, z: self.layer },
            aspect: wgpu::TextureAspect::All,
        }
    }
}

fn surface_texture(g: &Gpu, w: u32, h: u32, fmt: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages) -> wgpu::Texture {
    g.dev.create_texture(&wgpu::TextureDescriptor {
        label: Some("invisium-gl drawing buffer"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format: fmt,
        usage,
        view_formats: &[],
    })
}

impl Ctx {
    pub(crate) fn configure(&mut self, g: &mut Gpu, cfg: SurfaceCfg) {
        self.flush_pass(g);
        self.surface = None;
        let max = g.limits.max_texture_dimension_2d;
        let (w, h) = (cfg.width, cfg.height);
        let bytes = w as u64 * h as u64 * 8 * cfg.samples as u64;
        if w == 0 || h == 0 || w > max || h > max || !matches!(cfg.samples, 1 | 4) || bytes > MAX_ALLOC || !self.fits(g, bytes, 0) {
            return;
        }
        use wgpu::TextureUsages as U;
        let usage = if cfg.samples > 1 { U::RENDER_ATTACHMENT | U::TEXTURE_BINDING } else { U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC | U::COPY_DST };
        let color = surface_texture(g, w, h, wgpu::TextureFormat::Rgba8Unorm, cfg.samples, usage);
        let color_view = color.create_view(&Default::default());
        let ds = (cfg.depth || cfg.stencil).then(|| {
            let t = surface_texture(g, w, h, SURFACE_DS, cfg.samples, U::RENDER_ATTACHMENT | U::COPY_SRC | U::COPY_DST);
            let v = t.create_view(&Default::default());
            (t, v)
        });
        let px = w as usize * h as usize * cfg.samples as usize * 4;
        let bytes = px + if ds.is_some() { px } else { 0 };
        self.surface = Some(Surface { cfg, color, color_view, ds, bytes });
        self.clear_drawing_buffer(g);
    }

    /// Clear the drawing buffer as `Configure` defines (as the load operation of a new pass).
    fn clear_drawing_buffer(&mut self, g: &mut Gpu) {
        let Some(s) = &self.surface else { return };
        let a = if s.cfg.alpha { 0.0 } else { 1.0 };
        let req = super::pass::ClearReq {
            colors: vec![(0, ClearColor::F([0.0, 0.0, 0.0, a]))],
            depth: Some(1.0),
            stencil: Some(0),
            color_mask: [true; 4],
            stencil_mask: 0xff,
            scissor: None,
        };
        self.flush_pass(g);
        self.clear(g, &Target::Surface, req);
    }

    pub(crate) fn present(&mut self, g: &mut Gpu) {
        let Some(s) = &self.surface else { return };
        let cfg = s.cfg;
        let src = s.color_view.clone();
        self.flush_pass(g);
        let (w, h) = (cfg.width, cfg.height);
        if self.presented.as_ref().is_none_or(|p| p.texture.width() != w || p.texture.height() != h) {
            let texture = surface_texture(
                g,
                w,
                h,
                wgpu::TextureFormat::Rgba8Unorm,
                1,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let view = texture.create_view(&Default::default());
            let bind_group = g.dev.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("invisium-gl presented"),
                layout: &g.image_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }],
            });
            self.presented = Some(Presented { texture, view, bind_group, bytes: w as usize * h as usize * 4 });
        }
        let dst = self.presented.as_ref().map(|p| p.view.clone()).expect("created above");
        // Row 0 of the drawing buffer is GL's bottom row: flip while copying.
        let u = CopyU {
            src0: [0.0, h as f32],
            src1: [w as f32, 0.0],
            dst0: [0.0, 0.0],
            dst1: [w as f32, h as f32],
            size: [w as f32, h as f32],
            premultiply: !cfg.premultiplied,
            opaque: !cfg.alpha,
        };
        let enc = self.seg.encoder(g);
        util::copy_draw(g, enc, &src, Class::Float, cfg.samples > 1, &dst, wgpu::TextureFormat::Rgba8Unorm, [w, h], u, [0, 0, w, h], false);
        if !cfg.preserve {
            self.clear_drawing_buffer(g);
        }
    }

    /// Colour buffer `color` (or the depth-stencil image when `None`) of `t`.
    fn image_of(&mut self, t: &Target, color: Option<u32>) -> Option<Img> {
        match t {
            Target::Surface => {
                let s = self.surface.as_ref()?;
                let (tex, fmt, tf) = match color {
                    Some(0) => (s.color.clone(), wgpu::TextureFormat::Rgba8Unorm, TexFormat::Rgba8),
                    None => (s.ds.as_ref()?.0.clone(), SURFACE_DS, TexFormat::Depth24Stencil8),
                    Some(_) => return None,
                };
                Some(Img { tex, level: 0, layer: 0, fmt, tf, size: [s.cfg.width, s.cfg.height], samples: s.cfg.samples, id: 0 })
            }
            Target::Fbo { color: cs, depth_stencil } => {
                let a = match color {
                    Some(i) => (*cs.get(i as usize)?)?,
                    None => (*depth_stencil)?,
                };
                let t = self.textures.get(&a.texture)?;
                if a.level >= t.desc.mips || a.layer >= t.layers() || t.desc.kind == TexKind::D3 {
                    return None;
                }
                let [w, h, _] = t.level_size(a.level);
                Some(Img {
                    tex: t.tex.clone(),
                    level: a.level,
                    layer: a.layer,
                    fmt: t.fmt,
                    tf: t.desc.format,
                    size: [w, h],
                    samples: t.desc.samples,
                    id: a.texture,
                })
            }
        }
    }

    /// A single-sampled copy of a multisampled colour image (or the image itself).
    fn resolved(&mut self, g: &mut Gpu, img: Img) -> Option<Img> {
        if img.samples == 1 {
            return Some(img);
        }
        if !img.tf.is_color() || !format::features(img.fmt, g.features).flags.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE) {
            return None;
        }
        use wgpu::TextureUsages as U;
        let tex = surface_texture(g, img.size[0], img.size[1], img.fmt, 1, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC);
        let resolve = tex.create_view(&Default::default());
        let src = img.view();
        let enc = self.seg.encoder(g);
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("invisium-gl resolve"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &src,
                resolve_target: Some(&resolve),
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        g.passes += 1;
        Some(Img { tex, level: 0, layer: 0, samples: 1, id: u32::MAX, ..img })
    }

    /// A private copy of `[x, y, w, h]` of `img` (texel positions kept).
    fn detached(&mut self, g: &mut Gpu, img: Img) -> Img {
        use wgpu::TextureUsages as U;
        let tex = surface_texture(g, img.size[0], img.size[1], img.fmt, 1, U::TEXTURE_BINDING | U::COPY_DST);
        let enc = self.seg.encoder(g);
        enc.copy_texture_to_texture(
            img.copy_info(0, 0),
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: img.size[0], height: img.size[1], depth_or_array_layers: 1 },
        );
        Img { tex, level: 0, layer: 0, id: u32::MAX, ..img }
    }

    /// Draw `src` scaled into `dst` with GL blit rules; rectangles are `[x0, y0, x1, y1]`.
    fn blit_color(&mut self, g: &mut Gpu, src: &Img, dst: &Img, s: [f32; 4], d: [f32; 4], linear: bool) {
        let (sc, dc) = (format::class(src.fmt), format::class(dst.fmt));
        if sc != dc || d[0] == d[2] || d[1] == d[3] || s[0] == s[2] || s[1] == s[3] {
            return;
        }
        let r = [d[0].min(d[2]) as i32, d[1].min(d[3]) as i32, (d[2] - d[0]).abs() as i32, (d[3] - d[1]).abs() as i32];
        let Some(scissor) = clamp_rect(r, dst.size[0], dst.size[1]) else { return };
        let u = CopyU {
            src0: [s[0], s[1]],
            src1: [s[2], s[3]],
            dst0: [d[0], d[1]],
            dst1: [d[2], d[3]],
            size: [src.size[0] as f32, src.size[1] as f32],
            premultiply: false,
            opaque: false,
        };
        let linear = linear && format::filterable(src.fmt, g.features);
        let (sv, dv) = (src.view(), dst.view());
        let enc = self.seg.encoder(g);
        util::copy_draw(g, enc, &sv, sc, false, &dv, dst.fmt, dst.size, u, scissor, linear);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn blit(&mut self, g: &mut Gpu, src: &Target, src_color: u32, sr: Rect4, dst: &Target, dr: Rect4, planes: [bool; 3], linear: bool) {
        self.flush_pass(g);
        let f = |r: Rect4| [r[0] as f32, r[1] as f32, r[0] as f32 + r[2] as f32, r[1] as f32 + r[3] as f32];
        let (s, d) = (f(sr), f(dr));
        if planes[0] {
            if let Some(img) = self.image_of(src, Some(src_color)).and_then(|i| self.resolved(g, i)) {
                let ndst = match dst {
                    Target::Surface => 1,
                    Target::Fbo { color, .. } => color.len() as u32,
                };
                let mut src_img = Some(img);
                for i in 0..ndst {
                    let Some(di) = self.image_of(dst, Some(i)) else { continue };
                    if di.samples > 1 {
                        continue;
                    }
                    let si = src_img.take().expect("kept below");
                    let si = if si.id == di.id { self.detached(g, si) } else { si };
                    self.blit_color(g, &si, &di, s, d, linear);
                    src_img = Some(si);
                }
            }
        }
        if planes[1] || planes[2] {
            let (Some(si), Some(di)) = (self.image_of(src, None), self.image_of(dst, None)) else { return };
            let both = si.fmt.has_depth_aspect() && si.fmt.has_stencil_aspect();
            let wanted_all = !both || (planes[1] && planes[2]);
            let full = [0, 0, si.size[0] as i32, si.size[1] as i32];
            if si.fmt == di.fmt && si.size == di.size && si.samples == di.samples && sr == full && dr == full && wanted_all && si.id != di.id {
                let enc = self.seg.encoder(g);
                enc.copy_texture_to_texture(
                    si.copy_info(0, 0),
                    di.copy_info(0, 0),
                    wgpu::Extent3d { width: si.size[0], height: si.size[1], depth_or_array_layers: 1 },
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn copy_to_texture(&mut self, g: &mut Gpu, src: &Target, color: u32, s: [u32; 2], dst: Attachment, d: [u32; 2], size: [u32; 2]) {
        self.flush_pass(g);
        let [w, h] = size;
        if w == 0 || h == 0 {
            return;
        }
        let Some(si) = self.image_of(src, Some(color)).and_then(|i| self.resolved(g, i)) else { return };
        let Some(t) = self.textures.get(&dst.texture) else { return };
        if dst.level >= t.desc.mips || t.desc.samples > 1 {
            return;
        }
        let [lw, lh, ld] = t.level_size(dst.level);
        let slices = if t.desc.kind == TexKind::D3 { ld } else { t.layers() };
        let inside = |o: u32, n: u32, m: u32| o.checked_add(n).is_some_and(|e| e <= m);
        if dst.layer >= slices || !inside(s[0], w, si.size[0]) || !inside(s[1], h, si.size[1]) || !inside(d[0], w, lw) || !inside(d[1], h, lh) {
            return;
        }
        let same = si.fmt.remove_srgb_suffix() == t.fmt.remove_srgb_suffix();
        let di = Img {
            tex: t.tex.clone(),
            level: dst.level,
            layer: dst.layer,
            fmt: t.fmt,
            tf: t.desc.format,
            size: [lw, lh],
            samples: 1,
            id: dst.texture,
        };
        if same && si.fmt == t.fmt {
            let enc = self.seg.encoder(g);
            enc.copy_texture_to_texture(
                si.copy_info(s[0], s[1]),
                di.copy_info(d[0], d[1]),
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        } else if t.renderable && t.desc.kind != TexKind::D3 {
            let si = if si.id == di.id { self.detached(g, si) } else { si };
            let sr = [s[0] as f32, s[1] as f32, (s[0] + w) as f32, (s[1] + h) as f32];
            let dr = [d[0] as f32, d[1] as f32, (d[0] + w) as f32, (d[1] + h) as f32];
            self.blit_color(g, &si, &di, sr, dr, false);
        }
    }

    pub(crate) fn generate_mips(&mut self, g: &mut Gpu, id: TextureId) {
        self.flush_pass(g);
        let Some(t) = self.textures.get(&id) else { return };
        if t.desc.kind == TexKind::D3 || t.desc.samples > 1 || !t.renderable || !format::filterable(t.fmt, g.features) {
            return;
        }
        for level in 1..t.desc.mips {
            for layer in 0..t.layers() {
                let img = |l: u32| Img {
                    tex: t.tex.clone(),
                    level: l,
                    layer,
                    fmt: t.fmt,
                    tf: t.desc.format,
                    size: { let [w, h, _] = t.level_size(l); [w, h] },
                    samples: 1,
                    id,
                };
                let (src, dst) = (img(level - 1), img(level));
                let s = [0.0, 0.0, src.size[0] as f32, src.size[1] as f32];
                let d = [0.0, 0.0, dst.size[0] as f32, dst.size[1] as f32];
                let u = CopyU { src0: [s[0], s[1]], src1: [s[2], s[3]], dst0: [d[0], d[1]], dst1: [d[2], d[3]], size: [s[2], s[3]], ..Default::default() };
                let (sv, dv) = (src.view(), dst.view());
                let enc = self.seg.encoder(g);
                util::copy_draw(g, enc, &sv, Class::Float, false, &dv, dst.fmt, dst.size, u, [0, 0, dst.size[0], dst.size[1]], true);
            }
        }
    }

    pub(crate) fn read_pixels(&mut self, g: &mut Gpu, target: &Target, color: u32, rect: Rect4) -> Option<(TexFormat, Vec<u8>)> {
        self.submit(g);
        let img = self.image_of(target, Some(color))?;
        if !img.tf.is_color() {
            return None;
        }
        let [x, y, w, h] = rect;
        if x < 0 || y < 0 || w <= 0 || h <= 0 || x as u64 + w as u64 > img.size[0] as u64 || y as u64 + h as u64 > img.size[1] as u64 {
            return None;
        }
        let img = self.resolved(g, img)?;
        let (x, y, w, h) = (x as u32, y as u32, w as u32, h as u32);
        let bpp = img.tf.bytes();
        let row = w * bpp;
        let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buf = g.dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("invisium-gl readback"),
            size: padded as u64 * h as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.seg.encoder(g).copy_texture_to_buffer(
            img.copy_info(x, y),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.submit(g);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = g.dev.poll(wgpu::Maintain::Wait);
        rx.recv().ok()?.ok()?;
        let mut out = Vec::with_capacity((row * h) as usize);
        {
            let data = slice.get_mapped_range();
            for r in 0..h as usize {
                let start = r * padded as usize;
                out.extend_from_slice(&data[start..start + row as usize]);
            }
        }
        buf.unmap();
        Some((img.tf, out))
    }
}
