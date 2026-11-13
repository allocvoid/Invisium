//! Vertex buffer slots: grouping attributes, and converting data wgpu cannot read directly.

use super::format;
use super::program::Prog;
use super::{Ctx, Gpu};
use crate::draw::gl::*;
use wgpu::util::DeviceExt;

/// Converted attribute data larger than this skips the draw.
const MAX_REPACK: u64 = 64 << 20;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct SlotKey {
    pub stride: u64,
    pub step: wgpu::VertexStepMode,
    /// (format, offset in the slot, shader location)
    pub attrs: Vec<(wgpu::VertexFormat, u64, u32)>,
}

pub(crate) struct VSetup {
    pub slots: Vec<SlotKey>,
    /// Buffer and byte offset for each slot; `None` for the constant slot (its bytes are `consts`).
    pub bufs: Vec<Option<(wgpu::Buffer, u64)>>,
    pub consts: Vec<u8>,
    /// Index buffer, byte offset, format.
    pub index: Option<(wgpu::Buffer, u64, wgpu::IndexFormat)>,
}

/// A direct slot under construction: (buffer id, stride, step, base offset).
struct Direct {
    buffer: BufferId,
    base: u64,
}

fn const_bytes(v: Option<ConstValue>, base: Scalar) -> [u8; 16] {
    let v = v.unwrap_or(match base {
        Scalar::F32 => ConstValue::F([0.0, 0.0, 0.0, 1.0]),
        Scalar::I32 => ConstValue::I([0, 0, 0, 1]),
        Scalar::U32 => ConstValue::U([0, 0, 0, 1]),
    });
    let words: [u32; 4] = match (v, base) {
        (ConstValue::F(f), Scalar::F32) => f.map(f32::to_bits),
        (ConstValue::I(i), Scalar::F32) => i.map(|x| (x as f32).to_bits()),
        (ConstValue::U(u), Scalar::F32) => u.map(|x| (x as f32).to_bits()),
        (ConstValue::F(f), Scalar::I32) => f.map(|x| x as i32 as u32),
        (ConstValue::F(f), Scalar::U32) => f.map(|x| x as u32),
        (ConstValue::I(i), _) => i.map(|x| x as u32),
        (ConstValue::U(u), _) => u,
    };
    let mut out = [0u8; 16];
    for (k, w) in words.iter().enumerate() {
        out[k * 4..k * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

fn half(h: u16) -> f32 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f32;
    s * match e {
        0 => m * 2f32.powi(-24),
        31 => {
            if m == 0.0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Component `j` of an element whose bytes start `e` (zero-padded): raw value and, for
/// integer types, (bits, signed) for normalisation.
fn component(ty: AttrType, e: &[u8], j: usize) -> (f64, Option<(u32, bool)>) {
    let b = |i: usize| e.get(i).copied().unwrap_or(0);
    let u16_at = |i: usize| u16::from_le_bytes([b(i), b(i + 1)]);
    let u32_at = |i: usize| u32::from_le_bytes([b(i), b(i + 1), b(i + 2), b(i + 3)]);
    match ty {
        AttrType::I8 => (b(j) as i8 as f64, Some((8, true))),
        AttrType::U8 => (b(j) as f64, Some((8, false))),
        AttrType::I16 => (u16_at(j * 2) as i16 as f64, Some((16, true))),
        AttrType::U16 => (u16_at(j * 2) as f64, Some((16, false))),
        AttrType::I32 => (u32_at(j * 4) as i32 as f64, Some((32, true))),
        AttrType::U32 => (u32_at(j * 4) as f64, Some((32, false))),
        AttrType::F32 => (f32::from_bits(u32_at(j * 4)) as f64, None),
        AttrType::F16 => (half(u16_at(j * 2)) as f64, None),
        AttrType::I1010102 | AttrType::U1010102 => {
            let w = u32_at(0);
            let (shift, bits) = [(0, 10), (10, 10), (20, 10), (30, 2)][j.min(3)];
            let raw = (w >> shift) & ((1u32 << bits) - 1);
            if ty == AttrType::I1010102 {
                let v = ((raw << (32 - bits)) as i32) >> (32 - bits);
                (v as f64, Some((bits, true)))
            } else {
                (raw as f64, Some((bits, false)))
            }
        }
    }
}

/// Convert element `e` of `a` into `a.size` 32-bit values of class `base`.
fn convert(a: &VertexAttr, e: &[u8], base: Scalar, out: &mut Vec<u8>) {
    for j in 0..a.size as usize {
        let (v, int) = component(a.ty, e, j);
        let v = match (a.normalized && !a.integer, int) {
            (true, Some((bits, false))) => v / ((1u64 << bits) - 1) as f64,
            (true, Some((bits, true))) => (v / ((1u64 << (bits - 1)) - 1) as f64).max(-1.0),
            _ => v,
        };
        let w = match base {
            Scalar::F32 => (v as f32).to_bits(),
            Scalar::I32 => v as i32 as u32,
            Scalar::U32 => v as i64 as u32,
        };
        out.extend_from_slice(&w.to_le_bytes());
    }
}

impl Ctx {
    /// Vertex buffer slots for `d`. `Err(Some(id))`: buffer `id` needs a CPU copy first (then
    /// call again); `Err(None)`: the draw cannot run.
    pub(crate) fn vertex_setup(&self, g: &Gpu, p: &Prog, d: &Draw) -> Result<VSetup, Option<BufferId>> {
        let mut index = None;
        let mut known_max: Option<u64> = None;
        if let Some(ix) = &d.index {
            let b = self.buffers.get(&ix.buffer).ok_or(None)?;
            let isz: u64 = if ix.wide { 4 } else { 2 };
            let start = ix.offset as u64 + d.first as u64 * isz;
            let end = start + d.count as u64 * isz;
            if !(ix.offset as u64).is_multiple_of(isz) || end > b.len as u64 {
                return Err(None);
            }
            let fmt = if ix.wide { wgpu::IndexFormat::Uint32 } else { wgpu::IndexFormat::Uint16 };
            index = Some((b.gpu.clone(), ix.offset as u64, fmt));
        } else {
            known_max = Some((d.first as u64 + d.count as u64).saturating_sub(1));
        }

        let mut slots: Vec<SlotKey> = Vec::new();
        let mut bufs: Vec<Option<(wgpu::Buffer, u64)>> = Vec::new();
        let mut direct: Vec<Option<Direct>> = Vec::new();
        let mut consts = Vec::new();
        let mut const_attrs = Vec::new();
        let max_stride = g.limits.max_vertex_buffer_array_stride as u64;

        let mut seen = Vec::new();
        for slot in &p.desc.attributes {
            if seen.contains(&slot.location) || slot.location >= g.limits.max_vertex_attributes {
                continue;
            }
            seen.push(slot.location);
            let loc = slot.location;
            let Some(a) = d.attrs.iter().find(|a| a.location == loc) else {
                let v = d.consts.iter().find(|c| c.location == loc).map(|c| c.value);
                const_attrs.push((format::wide_format(slot.base, 4), consts.len() as u64, loc));
                consts.extend_from_slice(&const_bytes(v, slot.base));
                continue;
            };
            let b = self.buffers.get(&a.buffer).ok_or(None)?;
            if !(1..=4).contains(&a.size) || a.stride == 0 {
                return Err(None);
            }
            let elem = format::attr_bytes(a) as u64;
            let stride = a.stride as u64;
            let step = if a.divisor == 0 { wgpu::VertexStepMode::Vertex } else { wgpu::VertexStepMode::Instance };
            let direct_fmt = format::direct_vertex_format(a, slot.base)
                .filter(|_| a.divisor <= 1 && stride.is_multiple_of(4) && stride <= max_stride && elem <= stride);
            if let Some(fmt) = direct_fmt {
                // Elements this draw reads, where that is known without scanning indices.
                let n = if a.divisor == 0 { known_max.map(|v| v + 1) } else { Some(d.instances as u64) };
                if let Some(n) = n {
                    let last = n.saturating_sub(1).saturating_mul(stride);
                    if (a.offset as u64).saturating_add(last).saturating_add(elem) > b.len as u64 {
                        return Err(None);
                    }
                }
                let off = a.offset as u64;
                let size = fmt.size();
                let fits = |base: u64| {
                    off >= base && (off - base) + size <= stride && (off - base).is_multiple_of(size.min(4))
                };
                let existing = (0..slots.len()).find(|&i| {
                    direct[i].as_ref().is_some_and(|s| {
                        s.buffer == a.buffer && slots[i].stride == stride && slots[i].step == step && fits(s.base)
                    })
                });
                if let Some(i) = existing {
                    let base = direct[i].as_ref().map_or(0, |s| s.base);
                    slots[i].attrs.push((fmt, off - base, loc));
                    continue;
                }
                let base = off & !3;
                if fits(base) {
                    slots.push(SlotKey { stride, step, attrs: vec![(fmt, off - base, loc)] });
                    bufs.push(Some((b.gpu.clone(), base)));
                    direct.push(Some(Direct { buffer: a.buffer, base }));
                    continue;
                }
            }
            // Convert on the CPU into tightly packed 32-bit components. Indexed draws convert
            // every element the buffer holds, so indices never need scanning.
            let count = match (a.divisor, known_max) {
                (0, Some(v)) => v + 1,
                (0, None) => (b.len as u64).saturating_sub(a.offset as u64).div_ceil(stride).max(1),
                _ => d.instances as u64,
            };
            let out_stride = 4 * a.size as u64;
            if count * out_stride > MAX_REPACK {
                return Err(None);
            }
            let Some(data) = &b.data else { return Err(Some(a.buffer)) };
            let mut out = Vec::with_capacity((count * out_stride) as usize);
            for i in 0..count {
                let src = if a.divisor == 0 { i } else { i / a.divisor as u64 };
                let at = a.offset as u64 + src * stride;
                let e = if at < data.len() as u64 {
                    let s = at as usize;
                    &data[s..(s + elem as usize).min(data.len())]
                } else {
                    &[][..]
                };
                convert(a, e, slot.base, &mut out);
            }
            let buf = g.dev.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("invisium-gl converted attribute"),
                contents: &out,
                usage: wgpu::BufferUsages::VERTEX,
            });
            slots.push(SlotKey { stride: out_stride, step, attrs: vec![(format::wide_format(slot.base, a.size), 0, loc)] });
            bufs.push(Some((buf, 0)));
            direct.push(None);
        }
        if !const_attrs.is_empty() {
            slots.push(SlotKey { stride: 0, step: wgpu::VertexStepMode::Vertex, attrs: const_attrs });
            bufs.push(None);
        }
        if slots.len() > g.limits.max_vertex_buffers as usize {
            return Err(None);
        }
        Ok(VSetup { slots, bufs, consts, index })
    }
}
