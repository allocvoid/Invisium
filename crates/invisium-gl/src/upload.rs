//! Turning client pixel data into tightly packed device texels, honouring the unpack state.

use crate::args::R;
use crate::cmd::TexFormat::{self, *};
use crate::consts1::{FLOAT, INVALID_OPERATION, OUT_OF_MEMORY, RGB, RGBA, UNSIGNED_BYTE};
use crate::consts2::{HALF_FLOAT, RED, RG, RGBA_INTEGER, RG_INTEGER, RED_INTEGER, UNSIGNED_INT_10F_11F_11F_REV, UNSIGNED_INT_2_10_10_10_REV};
use crate::formats::{class, Class};
use crate::pixels::*;
use crate::state::Unpack;

/// What the client data is and what it becomes.
#[derive(Clone, Copy)]
pub struct Spec {
    pub format: u32,
    pub ty: u32,
    pub fmt: TexFormat,
}

impl Spec {
    fn raw(&self) -> bool {
        matches!(class(self.fmt), Class::Int | Class::Uint)
    }
}

/// Client data whose bytes are already the device texels.
fn identity(s: Spec) -> bool {
    use crate::consts1::{BYTE, INT, SHORT, UNSIGNED_INT, UNSIGNED_SHORT};
    let comps = match s.fmt {
        R8 | R8Snorm | R16F | R32F | R8I | R8U | R16I | R16U | R32I | R32U => 1,
        Rg8 | Rg8Snorm | Rg16F | Rg32F | Rg8I | Rg8U | Rg16I | Rg16U | Rg32I | Rg32U => 2,
        R11G11B10F => return s.format == RGB && s.ty == UNSIGNED_INT_10F_11F_11F_REV,
        Rgb10A2 => return s.format == RGBA && s.ty == UNSIGNED_INT_2_10_10_10_REV,
        _ => 4,
    };
    let fmt_ok = match (comps, class(s.fmt)) {
        (1, Class::Int | Class::Uint) => s.format == RED_INTEGER,
        (2, Class::Int | Class::Uint) => s.format == RG_INTEGER,
        (4, Class::Int | Class::Uint) => s.format == RGBA_INTEGER,
        (1, _) => s.format == RED,
        (2, _) => s.format == RG,
        _ => s.format == RGBA,
    };
    let ty = match s.fmt {
        R8 | Rg8 | Rgba8 | Srgb8A8 | R8U | Rg8U | Rgba8U => UNSIGNED_BYTE,
        R8Snorm | Rg8Snorm | Rgba8Snorm | R8I | Rg8I | Rgba8I => BYTE,
        R16F | Rg16F | Rgba16F => HALF_FLOAT,
        R32F | Rg32F | Rgba32F => FLOAT,
        R16I | Rg16I | Rgba16I => SHORT,
        R16U | Rg16U | Rgba16U => UNSIGNED_SHORT,
        R32I | Rg32I | Rgba32I => INT,
        R32U | Rg32U | Rgba32U => UNSIGNED_INT,
        _ => return false,
    };
    fmt_ok && s.ty == ty
}

/// Byte layout of client data in memory.
struct Layout {
    bpp: u64,
    stride: u64,
    image: u64,
    skip: u64,
}

fn layout(u: &Unpack, bpp: u64, w: u32, h: u32, three_d: bool, align: bool) -> Option<Layout> {
    let row_len = if u.row_length > 0 { u.row_length as u64 } else { w as u64 };
    let row_bytes = row_len.checked_mul(bpp)?;
    let a = if align { u.alignment.max(1) as u64 } else { 1 };
    let stride = row_bytes.div_ceil(a).checked_mul(a)?;
    let img_h = if three_d && u.image_height > 0 { u.image_height as u64 } else { h as u64 };
    let image = stride.checked_mul(img_h)?;
    let skip_images = if three_d { u.skip_images as u64 } else { 0 };
    let skip = skip_images
        .checked_mul(image)?
        .checked_add((u.skip_rows as u64).checked_mul(stride)?)?
        .checked_add((u.skip_pixels as u64).checked_mul(bpp)?)?;
    Some(Layout { bpp, stride, image, skip })
}

/// Bytes the client data must have for a `w` x `h` x `d` box.
fn needed(l: &Layout, w: u32, h: u32, d: u32) -> Option<u64> {
    if w == 0 || h == 0 || d == 0 {
        return Some(0);
    }
    l.skip
        .checked_add((d as u64 - 1).checked_mul(l.image)?)?
        .checked_add((h as u64 - 1).checked_mul(l.stride)?)?
        .checked_add((w as u64).checked_mul(l.bpp)?)
}

fn out_size(fmt: TexFormat, w: u32, h: u32, d: u32) -> R<usize> {
    (w as u64)
        .checked_mul(h as u64)
        .and_then(|n| n.checked_mul(d as u64))
        .and_then(|n| n.checked_mul(fmt.bytes() as u64))
        .filter(|&n| n <= crate::limits::MEMORY_BUDGET)
        .map(|n| n as usize)
        .ok_or(OUT_OF_MEMORY)
}

fn premultiply(v: [f64; 4]) -> [f64; 4] {
    [v[0] * v[3], v[1] * v[3], v[2] * v[3], v[3]]
}

/// Unpack an `ArrayBufferView` (or pixel unpack buffer) for a `w` x `h` x `d` box.
/// A view too small for the box is `INVALID_OPERATION`.
pub fn unpack_view(u: &Unpack, src: &[u8], s: Spec, w: u32, h: u32, d: u32, three_d: bool) -> R<Vec<u8>> {
    let bpp = src_bpp(s.format, s.ty) as u64;
    let l = layout(u, bpp, w, h, three_d, true).ok_or(INVALID_OPERATION)?;
    let need = needed(&l, w, h, d).ok_or(INVALID_OPERATION)?;
    if need > src.len() as u64 {
        return Err(INVALID_OPERATION);
    }
    let mut out = Vec::with_capacity(out_size(s.fmt, w, h, d)?);
    let premul = u.premultiply && !s.raw();
    let fast = identity(s) && !premul;
    let raw = s.raw();
    for z in 0..d as u64 {
        for y in 0..h as u64 {
            let sy = if u.flip_y { h as u64 - 1 - y } else { y };
            let row = (l.skip + z * l.image + sy * l.stride) as usize;
            let row_bytes = &src[row..row + (w as u64 * bpp) as usize];
            if fast {
                out.extend_from_slice(row_bytes);
                continue;
            }
            for px in row_bytes.chunks_exact(bpp as usize) {
                let mut v = decode(s.format, s.ty, raw, px);
                if premul {
                    v = premultiply(v);
                }
                encode(s.fmt, v, &mut out);
            }
        }
    }
    Ok(out)
}

/// Unpack a DOM source (straight-alpha RGBA8, top row first) of `sw` x `sh` pixels for a `w` x
/// `h` x `d` box. `sized`: the call gave the box size, so the unpack row length and skips apply.
#[allow(clippy::too_many_arguments)]
pub fn unpack_pixels(u: &Unpack, sw: u32, sh: u32, rgba: &[u8], s: Spec, w: u32, h: u32, d: u32, sized: bool) -> R<Vec<u8>> {
    let mut uu = *u;
    if !sized || uu.row_length == 0 {
        uu.row_length = sw.min(i32::MAX as u32) as i32;
    }
    if !sized {
        uu.skip_pixels = 0;
        uu.skip_rows = 0;
        uu.skip_images = 0;
        uu.image_height = 0;
    }
    let l = layout(&uu, 4, w, h, d > 1 || uu.skip_images > 0, false).ok_or(INVALID_OPERATION)?;
    let need = needed(&l, w, h, d).ok_or(INVALID_OPERATION)?;
    let have = (sw as u64).saturating_mul(sh as u64).saturating_mul(4).min(rgba.len() as u64);
    if need > have {
        return Err(INVALID_OPERATION);
    }
    let mut out = Vec::with_capacity(out_size(s.fmt, w, h, d)?);
    let raw = s.raw();
    for z in 0..d as u64 {
        for y in 0..h as u64 {
            let sy = if u.flip_y { h as u64 - 1 - y } else { y };
            let row = (l.skip + z * l.image + sy * l.stride) as usize;
            for px in rgba[row..row + w as usize * 4].as_chunks::<4>().0 {
                let mut v = px.map(|c| if raw { c as f64 } else { c as f64 / 255.0 });
                if u.premultiply && !raw {
                    v = premultiply(v);
                }
                let v = quantize(s.ty, expand(s.format, pick(s.format, v)));
                encode(s.fmt, v, &mut out);
            }
        }
    }
    Ok(out)
}

/// Zeroed device texels for a box.
pub fn zeros(fmt: TexFormat, w: u32, h: u32, d: u32) -> R<Vec<u8>> {
    Ok(vec![0; out_size(fmt, w, h, d)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_budget_is_out_of_memory() {
        assert_eq!(zeros(Rgba32F, 8192, 8192, 2), Err(OUT_OF_MEMORY));
        assert_eq!(zeros(Rgba32F, u32::MAX, u32::MAX, u32::MAX), Err(OUT_OF_MEMORY));
        let s = Spec { format: RGBA, ty: FLOAT, fmt: Rgba32F };
        let u = Unpack { alignment: 1, row_length: 0, image_height: 0, skip_pixels: 0, skip_rows: 0, skip_images: 0, flip_y: false, premultiply: false, colorspace: 0 };
        assert_eq!(unpack_view(&u, &[], s, 8192, 8192, 0, false).map(|v| v.len()), Ok(0));
        assert_eq!(zeros(R8, 2, 2, 1).map(|v| v.len()), Ok(4));
    }
}
