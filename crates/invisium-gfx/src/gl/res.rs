//! Buffers, textures, views, samplers and the drawing buffer.

use super::format::{self, Class};
use super::{Ctx, Gpu, MAX_ALLOC};
use crate::draw::gl::*;
use std::collections::HashMap;

/// Extra bytes after every GPU buffer so the last element of any vertex format can be fetched.
const BUFFER_PAD: u64 = 16;

pub(crate) struct Buf {
    pub gpu: wgpu::Buffer,
    /// Size the page gave the buffer (the GPU buffer is padded).
    pub len: usize,
    /// A CPU copy of the contents, made (by one readback) only when an operation needs existing
    /// bytes on the CPU: unaligned writes and copies, attribute conversion, unaligned uniform
    /// blocks. Kept exact afterwards.
    pub data: Option<Vec<u8>>,
    pub serial: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct ViewKey {
    pub dim: wgpu::TextureViewDimension,
    pub base_mip: u32,
    pub mips: u32,
    pub base_layer: u32,
    pub layers: u32,
    pub aspect: wgpu::TextureAspect,
}

pub(crate) struct Tex {
    pub tex: wgpu::Texture,
    pub desc: TexDesc,
    pub fmt: wgpu::TextureFormat,
    pub bytes: usize,
    pub serial: u64,
    pub renderable: bool,
    views: HashMap<ViewKey, wgpu::TextureView>,
}

impl Tex {
    pub fn layers(&self) -> u32 {
        match self.desc.kind {
            TexKind::Cube => 6,
            TexKind::Array => self.desc.depth,
            _ => 1,
        }
    }

    /// Size of `level`: width, height, and depth of a 3D texture (1 otherwise).
    pub fn level_size(&self, level: u32) -> [u32; 3] {
        let d = if self.desc.kind == TexKind::D3 { (self.desc.depth >> level).max(1) } else { 1 };
        [(self.desc.width >> level).max(1), (self.desc.height >> level).max(1), d]
    }

    pub fn view(&mut self, key: ViewKey) -> wgpu::TextureView {
        let tex = &self.tex;
        self.views
            .entry(key)
            .or_insert_with(|| {
                tex.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("invisium-gl view"),
                    format: None,
                    dimension: Some(key.dim),
                    usage: None,
                    aspect: key.aspect,
                    base_mip_level: key.base_mip,
                    mip_level_count: Some(key.mips),
                    base_array_layer: key.base_layer,
                    array_layer_count: Some(key.layers),
                })
            })
            .clone()
    }

    /// One image (level, layer) as a render attachment or copy source.
    pub fn image_view(&mut self, level: u32, layer: u32) -> wgpu::TextureView {
        let key = ViewKey {
            dim: wgpu::TextureViewDimension::D2,
            base_mip: level,
            mips: 1,
            base_layer: layer,
            layers: 1,
            aspect: wgpu::TextureAspect::All,
        };
        self.view(key)
    }
}

/// `Cmd::Configure` parameters.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct SurfaceCfg {
    pub width: u32,
    pub height: u32,
    pub alpha: bool,
    pub depth: bool,
    pub stencil: bool,
    pub samples: u32,
    pub premultiplied: bool,
    pub preserve: bool,
}

/// The drawing buffer.
pub(crate) struct Surface {
    pub cfg: SurfaceCfg,
    pub color: wgpu::Texture,
    pub color_view: wgpu::TextureView,
    pub ds: Option<(wgpu::Texture, wgpu::TextureView)>,
    pub bytes: usize,
}

/// The image the renderer composites for a context.
pub(crate) struct Presented {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub bind_group: wgpu::BindGroup,
    pub bytes: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct SamplerKey {
    filters: [wgpu::FilterMode; 3],
    wrap: [wgpu::AddressMode; 3],
    compare: Option<wgpu::CompareFunction>,
    lod: [u32; 2],
}

pub(crate) fn sampler(g: &mut Gpu, s: &Sampling, nearest_only: bool, comparison: bool) -> wgpu::Sampler {
    let f = |x: Filter| if nearest_only { wgpu::FilterMode::Nearest } else { format::filter(x) };
    let lod_min = if s.lod_min.is_finite() { s.lod_min.clamp(0.0, 32.0) } else { 0.0 };
    let lod_max = if s.lod_max.is_finite() { s.lod_max.clamp(lod_min, 32.0) } else { 32.0 };
    let key = SamplerKey {
        filters: [f(s.mag), f(s.min), f(s.mip.unwrap_or(Filter::Nearest))],
        wrap: s.wrap.map(format::wrap),
        compare: comparison.then(|| format::compare(s.compare.unwrap_or(Compare::LessEqual))),
        lod: [lod_min.to_bits(), lod_max.to_bits()],
    };
    let dev = &g.dev;
    g.samplers
        .entry(key)
        .or_insert_with(|| {
            dev.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("invisium-gl sampler"),
                address_mode_u: key.wrap[0],
                address_mode_v: key.wrap[1],
                address_mode_w: key.wrap[2],
                mag_filter: key.filters[0],
                min_filter: key.filters[1],
                mipmap_filter: key.filters[2],
                lod_min_clamp: lod_min,
                lod_max_clamp: lod_max,
                compare: key.compare,
                anisotropy_clamp: 1,
                border_color: None,
            })
        })
        .clone()
}

pub(crate) fn view_dim(kind: TexKind) -> wgpu::TextureViewDimension {
    match kind {
        TexKind::D2 => wgpu::TextureViewDimension::D2,
        TexKind::Cube => wgpu::TextureViewDimension::Cube,
        TexKind::D3 => wgpu::TextureViewDimension::D3,
        TexKind::Array => wgpu::TextureViewDimension::D2Array,
    }
}

/// A 1x1 all-zero texture of the given kind and class (texture id 0).
pub(crate) fn dummy_view(g: &mut Gpu, kind: TexKind, class: Class) -> wgpu::TextureView {
    let dev = &g.dev;
    g.dummies
        .entry((kind, class))
        .or_insert_with(|| {
            let format = match class {
                Class::Sint => wgpu::TextureFormat::Rgba8Sint,
                Class::Uint => wgpu::TextureFormat::Rgba8Uint,
                Class::Depth if kind != TexKind::D3 => wgpu::TextureFormat::Depth32Float,
                _ => wgpu::TextureFormat::Rgba8Unorm,
            };
            let layers = if kind == TexKind::Cube { 6 } else { 1 };
            let tex = dev.create_texture(&wgpu::TextureDescriptor {
                label: Some("invisium-gl dummy"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: layers },
                mip_level_count: 1,
                sample_count: 1,
                dimension: if kind == TexKind::D3 { wgpu::TextureDimension::D3 } else { wgpu::TextureDimension::D2 },
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(view_dim(kind)),
                aspect: if class == Class::Depth { wgpu::TextureAspect::DepthOnly } else { wgpu::TextureAspect::All },
                ..Default::default()
            });
            (tex, view)
        })
        .1
        .clone()
}

fn max_mips(w: u32, h: u32, d: u32) -> u32 {
    32 - w.max(h).max(d).max(1).leading_zeros()
}

/// Bytes of all levels of a texture.
fn texture_bytes(desc: &TexDesc, layers: u32) -> u64 {
    let mut total = 0u64;
    for l in 0..desc.mips.min(32) {
        let w = (desc.width >> l).max(1) as u64;
        let h = (desc.height >> l).max(1) as u64;
        let d = if desc.kind == TexKind::D3 { (desc.depth >> l).max(1) as u64 } else { layers as u64 };
        total += w * h * d * desc.format.bytes() as u64;
    }
    total * desc.samples.max(1) as u64
}

impl Ctx {
    pub(crate) fn buffer_create(&mut self, g: &mut Gpu, id: BufferId, _index: bool, bytes: Vec<u8>) {
        self.buffers.remove(&id);
        let len = bytes.len() as u64;
        let size = (len + BUFFER_PAD + 3) & !3;
        if id == 0 || len > MAX_ALLOC || size > g.limits.max_buffer_size || !self.fits(g, size, 0) {
            return;
        }
        let usage = wgpu::BufferUsages::VERTEX
            | wgpu::BufferUsages::INDEX
            | wgpu::BufferUsages::UNIFORM
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC;
        let gpu = g.dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("invisium-gl buffer"),
            size,
            usage,
            mapped_at_creation: true,
        });
        gpu.slice(..).get_mapped_range_mut()[..bytes.len()].copy_from_slice(&bytes);
        gpu.unmap();
        let serial = g.next_serial();
        self.buffers.insert(id, Buf { gpu, len: bytes.len(), data: None, serial });
    }

    /// Make sure buffer `id` has a CPU copy, reading it back from the GPU once if needed.
    pub(crate) fn ensure_copy(&mut self, g: &mut Gpu, id: BufferId) -> bool {
        match self.buffers.get(&id) {
            None => return false,
            Some(b) if b.data.is_some() => return true,
            Some(_) => {}
        }
        let len = self.buffers[&id].len;
        let Some(data) = self.read_back(g, id, 0, len) else { return false };
        if let Some(b) = self.buffers.get_mut(&id) {
            b.data = Some(data);
        }
        true
    }

    /// `len` bytes of buffer `id` from `offset` as the GPU holds them after all work so far.
    fn read_back(&mut self, g: &mut Gpu, id: BufferId, offset: usize, len: usize) -> Option<Vec<u8>> {
        let size = self.buffers.get(&id)?.len;
        let end = offset.checked_add(len).filter(|&e| e <= size)?;
        if len == 0 {
            return Some(Vec::new());
        }
        self.submit(g);
        let b = self.buffers.get(&id)?;
        // Copies need 4-byte alignment; the GPU buffer is padded past `len`, so this fits.
        let (a, z) = ((offset & !3) as u64, ((end + 3) & !3) as u64);
        if z > b.gpu.size() {
            return None;
        }
        let rb = g.dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("invisium-gl buffer readback"),
            size: z - a,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = g.dev.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&b.gpu, a, &rb, 0, z - a);
        g.queue.submit([enc.finish()]);
        let slice = rb.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = g.dev.poll(wgpu::Maintain::Wait);
        if !matches!(rx.recv(), Ok(Ok(()))) {
            return None;
        }
        let skip = offset - a as usize;
        let data = slice.get_mapped_range()[skip..skip + len].to_vec();
        rb.unmap();
        Some(data)
    }

    /// `GlDevice::read_buffer`: from the CPU copy if there is one, else read back (no copy kept).
    pub(crate) fn read_buffer(&mut self, g: &mut Gpu, id: BufferId, offset: u32, len: u32) -> Option<Vec<u8>> {
        let (offset, len) = (offset as usize, len as usize);
        let b = self.buffers.get(&id)?;
        let end = offset.checked_add(len).filter(|&e| e <= b.len)?;
        match &b.data {
            Some(d) => Some(d[offset..end].to_vec()),
            None => self.read_back(g, id, offset, len),
        }
    }

    /// Upload `data[start..end]` of buffer `id` (after its CPU copy was changed).
    fn upload_span(&mut self, g: &mut Gpu, id: BufferId, start: usize, end: usize) {
        if start >= end {
            return;
        }
        self.before_queue_write(g);
        let Some(b) = self.buffers.get(&id) else { return };
        let Some(data) = &b.data else { return };
        let a = start & !3;
        let z = (end + 3) & !3;
        let mut tmp = vec![0u8; z - a];
        let have = data.len().min(z);
        tmp[..have - a].copy_from_slice(&data[a..have]);
        g.queue.write_buffer(&b.gpu, a as u64, &tmp);
    }

    pub(crate) fn buffer_write(&mut self, g: &mut Gpu, id: BufferId, offset: u32, bytes: &[u8]) {
        let Some(b) = self.buffers.get(&id) else { return };
        let start = offset as usize;
        let Some(end) = start.checked_add(bytes.len()).filter(|&e| e <= b.len) else { return };
        if start == end {
            return;
        }
        if start % 4 == 0 && bytes.len() % 4 == 0 {
            self.before_queue_write(g);
            let b = self.buffers.get_mut(&id).expect("checked above");
            g.queue.write_buffer(&b.gpu, start as u64, bytes);
            if let Some(d) = &mut b.data {
                d[start..end].copy_from_slice(bytes);
            }
            return;
        }
        if !self.ensure_copy(g, id) {
            return;
        }
        let b = self.buffers.get_mut(&id).expect("checked above");
        b.data.as_mut().expect("ensured")[start..end].copy_from_slice(bytes);
        self.upload_span(g, id, start, end);
    }

    pub(crate) fn buffer_copy(&mut self, g: &mut Gpu, src: BufferId, so: u32, dst: BufferId, d_o: u32, size: u32) {
        let (so, d_o, size) = (so as usize, d_o as usize, size as usize);
        let (Some(s), Some(d)) = (self.buffers.get(&src), self.buffers.get(&dst)) else { return };
        if size == 0 || so.checked_add(size).is_none_or(|e| e > s.len) || d_o.checked_add(size).is_none_or(|e| e > d.len) {
            return;
        }
        if src != dst && (so | d_o | size) % 4 == 0 {
            // A GPU copy, ordered in the command stream; the CPU copy of `dst` follows along.
            self.flush_pass(g);
            let (s, d) = (self.buffers[&src].gpu.clone(), self.buffers[&dst].gpu.clone());
            self.seg.encoder(g).copy_buffer_to_buffer(&s, so as u64, &d, d_o as u64, size as u64);
            let bytes = self.buffers[&src].data.as_ref().map(|v| v[so..so + size].to_vec());
            let d = self.buffers.get_mut(&dst).expect("checked above");
            match (bytes, &mut d.data) {
                (Some(b), Some(v)) => v[d_o..d_o + size].copy_from_slice(&b),
                (None, data) => *data = None,
                _ => {}
            }
            return;
        }
        if !self.ensure_copy(g, src) {
            return;
        }
        let bytes = self.buffers[&src].data.as_ref().expect("ensured")[so..so + size].to_vec();
        self.buffer_write(g, dst, d_o as u32, &bytes);
    }

    pub(crate) fn texture_create(&mut self, g: &mut Gpu, id: TextureId, desc: TexDesc) {
        self.flush_pass(g);
        self.textures.remove(&id);
        if self.bind_groups.len() > 512 {
            self.bind_groups.clear();
        }
        if id == 0 {
            return;
        }
        let budget = g.ctx_cap.saturating_sub(self.gpu_bytes());
        if let Some(t) = make_texture(g, desc, budget) {
            self.textures.insert(id, t);
        }
    }

    pub(crate) fn texture_grow(&mut self, g: &mut Gpu, id: TextureId, mips: u32) {
        let Some(old) = self.textures.get(&id) else { return };
        if mips <= old.desc.mips || old.desc.samples > 1 {
            return;
        }
        let desc = TexDesc { mips, ..old.desc };
        let budget = g.ctx_cap.saturating_sub(self.gpu_bytes() - old.bytes as u64);
        let Some(new) = make_texture(g, desc, budget) else { return };
        self.flush_pass(g);
        let old = self.textures.remove(&id).expect("checked above");
        let enc = self.seg.encoder(g);
        for level in 0..old.desc.mips {
            let [w, h, d] = old.level_size(level);
            let depth = if old.desc.kind == TexKind::D3 { d } else { old.layers() };
            enc.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo { texture: &old.tex, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyTextureInfo { texture: &new.tex, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: depth },
            );
        }
        self.textures.insert(id, new);
    }

    pub(crate) fn texture_write(&mut self, g: &mut Gpu, id: TextureId, level: u32, layer: u32, r: [u32; 5], bytes: &[u8]) {
        let [x, y, w, h, d] = r;
        let Some(t) = self.textures.get(&id) else { return };
        if !t.desc.format.is_color() || t.desc.samples > 1 || level >= t.desc.mips || w == 0 || h == 0 || d == 0 {
            return;
        }
        let [lw, lh, ld] = t.level_size(level);
        let slices = if t.desc.kind == TexKind::D3 { ld } else { t.layers() };
        let fits = |o: u32, n: u32, max: u32| o.checked_add(n).is_some_and(|e| e <= max);
        if !fits(x, w, lw) || !fits(y, h, lh) || !fits(layer, d, slices) {
            return;
        }
        let bpp = t.desc.format.bytes() as u64;
        let need = w as u64 * h as u64 * d as u64 * bpp;
        if (bytes.len() as u64) < need {
            return;
        }
        self.before_queue_write(g);
        let t = &self.textures[&id];
        g.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t.tex,
                mip_level: level,
                origin: wgpu::Origin3d { x, y, z: layer },
                aspect: wgpu::TextureAspect::All,
            },
            &bytes[..need as usize],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * bpp as u32), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: d },
        );
    }
}

/// Create a texture for `desc`, or `None` if the description is invalid, too large, or needs
/// more than `budget` bytes.
pub(crate) fn make_texture(g: &mut Gpu, desc: TexDesc, budget: u64) -> Option<Tex> {
    let l = &g.limits;
    let fmt = format::tex_format(desc.format, g.features);
    let (w, h) = (desc.width, desc.height);
    let (dim, layers, ok) = match desc.kind {
        TexKind::D2 => (wgpu::TextureDimension::D2, 1, w <= l.max_texture_dimension_2d && h <= l.max_texture_dimension_2d),
        TexKind::Cube => (wgpu::TextureDimension::D2, 6, w == h && w <= l.max_texture_dimension_2d),
        TexKind::Array => (
            wgpu::TextureDimension::D2,
            desc.depth,
            w <= l.max_texture_dimension_2d && h <= l.max_texture_dimension_2d && desc.depth <= l.max_texture_array_layers,
        ),
        TexKind::D3 => {
            let m = l.max_texture_dimension_3d;
            (wgpu::TextureDimension::D3, desc.depth, w <= m && h <= m && desc.depth <= m && !fmt.is_depth_stencil_format())
        }
    };
    let d3 = if desc.kind == TexKind::D3 { desc.depth } else { 1 };
    let samples_ok = match desc.samples {
        1 => true,
        4 => desc.kind == TexKind::D2 && desc.mips == 1 && format::msaa4(fmt, g.features),
        _ => false,
    };
    if !ok || !samples_ok || w == 0 || h == 0 || layers == 0 || desc.mips == 0 || desc.mips > max_mips(w, h, d3) {
        return None;
    }
    let bytes = texture_bytes(&desc, layers);
    if bytes > MAX_ALLOC {
        return None;
    }
    if bytes > budget {
        g.log(|| format!("texture of {bytes} bytes refused: context over its memory cap"));
        return None;
    }
    let renderable = format::renderable(fmt, g.features) && desc.kind != TexKind::D3;
    let mut usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST;
    if renderable {
        usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
    }
    usage &= format::features(fmt, g.features).allowed_usages;
    if desc.samples > 1 {
        if !renderable {
            return None;
        }
        usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    }
    let tex = g
        .scoped(|| {
            g.dev.create_texture(&wgpu::TextureDescriptor {
                label: Some("invisium-gl texture"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: layers },
                mip_level_count: desc.mips,
                sample_count: desc.samples,
                dimension: dim,
                format: fmt,
                usage,
                view_formats: &[],
            })
        })
        .map_err(|e| g.log(|| format!("texture create failed: {e}")))
        .ok()?;
    let serial = g.next_serial();
    Some(Tex { tex, desc, fmt, bytes: bytes as usize, serial, renderable, views: HashMap::new() })
}
