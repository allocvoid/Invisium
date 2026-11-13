//! Texel conversion: client (format, type) data <-> RGBA values <-> device formats.
//!
//! Values travel as `[f64; 4]`: normalized and float data as floats, integer data as raw
//! integers (every 32-bit integer is exact in an `f64`).

use crate::cmd::TexFormat::{self, *};
use crate::consts1::*;
use crate::consts2::{
    FLOAT_32_UNSIGNED_INT_24_8_REV, HALF_FLOAT, RG, RGBA_INTEGER, RGB_INTEGER, RG_INTEGER,
    UNSIGNED_INT_10F_11F_11F_REV, UNSIGNED_INT_24_8, UNSIGNED_INT_2_10_10_10_REV, UNSIGNED_INT_5_9_9_9_REV,
};
use crate::formats::{class, Class};

pub fn half_to_f32(h: u16) -> f32 {
    let sign = ((h as u32) & 0x8000) << 16;
    let exp = ((h >> 10) & 0x1f) as u32;
    let mant = (h & 0x3ff) as u32;
    if exp == 0 {
        let v = mant as f32 / 16_777_216.0;
        return if sign != 0 { -v } else { v };
    }
    let bits = if exp == 31 { sign | 0x7f80_0000 | (mant << 13) } else { sign | ((exp + 112) << 23) | (mant << 13) };
    f32::from_bits(bits)
}

pub fn f32_to_half(f: f32) -> u16 {
    let x = f.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32;
    let mant = x & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = mant | 0x80_0000;
        let shift = (14 - e) as u32;
        let halfway = 1u32 << (shift - 1);
        let mut r = m >> shift;
        let rem = m & ((1u32 << shift) - 1);
        if rem > halfway || (rem == halfway && r & 1 == 1) {
            r += 1;
        }
        return sign | r as u16;
    }
    let mut r = ((e as u32) << 10) | (mant >> 13);
    let rem = mant & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && r & 1 == 1) {
        r += 1;
    }
    sign | r as u16
}

/// Unsigned small float with a 5-bit exponent and `mbits` (6 or 5) mantissa bits.
fn to_small_float(v: f64, mbits: u32) -> u32 {
    if v.is_nan() {
        return (0x1f << mbits) | 1;
    }
    if v <= 0.0 {
        return 0;
    }
    (f32_to_half(v as f32) as u32 >> (10 - mbits)) & ((1 << (5 + mbits)) - 1)
}

fn from_small_float(bits: u32, mbits: u32) -> f64 {
    half_to_f32((bits << (10 - mbits)) as u16) as f64
}

fn rd(p: &[u8], i: usize, n: usize) -> u32 {
    let mut v = 0u32;
    for k in 0..n {
        v |= (p.get(i * n + k).copied().unwrap_or(0) as u32) << (8 * k);
    }
    v
}

/// Components a client format carries.
pub fn format_comps(format: u32) -> usize {
    match format {
        RGBA | RGBA_INTEGER => 4,
        RGB | RGB_INTEGER => 3,
        RG | RG_INTEGER | LUMINANCE_ALPHA => 2,
        _ => 1,
    }
}

/// Bytes per pixel of client data.
pub fn src_bpp(format: u32, ty: u32) -> usize {
    match ty {
        UNSIGNED_SHORT_5_6_5 | UNSIGNED_SHORT_4_4_4_4 | UNSIGNED_SHORT_5_5_5_1 => 2,
        UNSIGNED_INT_2_10_10_10_REV | UNSIGNED_INT_10F_11F_11F_REV | UNSIGNED_INT_5_9_9_9_REV | UNSIGNED_INT_24_8 => 4,
        FLOAT_32_UNSIGNED_INT_24_8_REV => 8,
        _ => format_comps(format) * type_size(ty),
    }
}

pub fn type_size(ty: u32) -> usize {
    match ty {
        UNSIGNED_BYTE | BYTE => 1,
        UNSIGNED_SHORT | SHORT | HALF_FLOAT | HALF_FLOAT_OES => 2,
        FLOAT_32_UNSIGNED_INT_24_8_REV => 8,
        _ => 4,
    }
}

/// Spread the components of a client format over RGBA.
pub fn expand(format: u32, c: [f64; 4]) -> [f64; 4] {
    match format {
        RGBA | RGBA_INTEGER => c,
        RGB | RGB_INTEGER => [c[0], c[1], c[2], 1.0],
        RG | RG_INTEGER => [c[0], c[1], 0.0, 1.0],
        LUMINANCE => [c[0], c[0], c[0], 1.0],
        LUMINANCE_ALPHA => [c[0], c[0], c[0], c[1]],
        ALPHA => [0.0, 0.0, 0.0, c[0]],
        _ => [c[0], 0.0, 0.0, 1.0],
    }
}

/// The components a client format would carry of an RGBA value (inverse of `expand`).
pub fn pick(format: u32, v: [f64; 4]) -> [f64; 4] {
    match format {
        LUMINANCE_ALPHA => [v[0], v[3], 0.0, 0.0],
        ALPHA => [v[3], 0.0, 0.0, 0.0],
        _ => v,
    }
}

/// Decode one pixel of client data. `raw`: the image is an integer format.
pub fn decode(format: u32, ty: u32, raw: bool, p: &[u8]) -> [f64; 4] {
    let one = |v: u32, bits: u32| v as f64 / ((1u64 << bits) - 1) as f64;
    let c = match ty {
        UNSIGNED_SHORT_5_6_5 => {
            let v = rd(p, 0, 2);
            [one(v >> 11, 5), one((v >> 5) & 63, 6), one(v & 31, 5), 1.0]
        }
        UNSIGNED_SHORT_4_4_4_4 => {
            let v = rd(p, 0, 2);
            [one(v >> 12, 4), one((v >> 8) & 15, 4), one((v >> 4) & 15, 4), one(v & 15, 4)]
        }
        UNSIGNED_SHORT_5_5_5_1 => {
            let v = rd(p, 0, 2);
            [one(v >> 11, 5), one((v >> 6) & 31, 5), one((v >> 1) & 31, 5), (v & 1) as f64]
        }
        UNSIGNED_INT_2_10_10_10_REV => {
            let v = rd(p, 0, 4);
            let f = [v & 0x3ff, (v >> 10) & 0x3ff, (v >> 20) & 0x3ff, v >> 30];
            if raw {
                f.map(|x| x as f64)
            } else {
                [one(f[0], 10), one(f[1], 10), one(f[2], 10), one(f[3], 2)]
            }
        }
        UNSIGNED_INT_10F_11F_11F_REV => {
            let v = rd(p, 0, 4);
            [from_small_float(v & 0x7ff, 6), from_small_float((v >> 11) & 0x7ff, 6), from_small_float(v >> 22, 5), 1.0]
        }
        UNSIGNED_INT_5_9_9_9_REV => {
            let v = rd(p, 0, 4);
            let scale = 2f64.powi((v >> 27) as i32 - 15 - 9);
            [(v & 0x1ff) as f64 * scale, ((v >> 9) & 0x1ff) as f64 * scale, ((v >> 18) & 0x1ff) as f64 * scale, 1.0]
        }
        _ => {
            let n = format_comps(format);
            let sz = type_size(ty);
            let mut c = [0.0; 4];
            for (i, slot) in c.iter_mut().enumerate().take(n) {
                let v = rd(p, i, sz);
                *slot = match ty {
                    UNSIGNED_BYTE => if raw { v as f64 } else { one(v, 8) },
                    BYTE => if raw { v as u8 as i8 as f64 } else { (v as u8 as i8 as f64 / 127.0).max(-1.0) },
                    UNSIGNED_SHORT => if raw { v as f64 } else { one(v, 16) },
                    SHORT => if raw { v as u16 as i16 as f64 } else { (v as u16 as i16 as f64 / 32767.0).max(-1.0) },
                    UNSIGNED_INT => if raw { v as f64 } else { one(v, 32) },
                    INT => v as i32 as f64,
                    HALF_FLOAT | HALF_FLOAT_OES => half_to_f32(v as u16) as f64,
                    FLOAT => f32::from_bits(v) as f64,
                    _ => 0.0,
                };
            }
            return expand(format, c);
        }
    };
    expand(format, pick(format, c))
}

/// Round a value to what client data of `ty` could hold (DOM sources converted to the
/// requested type before upload).
pub fn quantize(ty: u32, v: [f64; 4]) -> [f64; 4] {
    let q = |x: f64, bits: u32| {
        let m = ((1u64 << bits) - 1) as f64;
        (x.clamp(0.0, 1.0) * m).round() / m
    };
    match ty {
        UNSIGNED_SHORT_5_6_5 => [q(v[0], 5), q(v[1], 6), q(v[2], 5), v[3]],
        UNSIGNED_SHORT_4_4_4_4 => v.map(|x| q(x, 4)),
        UNSIGNED_SHORT_5_5_5_1 => [q(v[0], 5), q(v[1], 5), q(v[2], 5), q(v[3], 1)],
        UNSIGNED_INT_2_10_10_10_REV => [q(v[0], 10), q(v[1], 10), q(v[2], 10), q(v[3], 2)],
        HALF_FLOAT | HALF_FLOAT_OES => v.map(|x| half_to_f32(f32_to_half(x as f32)) as f64),
        _ => v,
    }
}

fn channels(f: TexFormat) -> usize {
    match f {
        R8 | R8Snorm | R16F | R32F | R8I | R8U | R16I | R16U | R32I | R32U => 1,
        Rg8 | Rg8Snorm | Rg16F | Rg32F | Rg8I | Rg8U | Rg16I | Rg16U | Rg32I | Rg32U => 2,
        _ => 4,
    }
}

/// Encode a value in a device format (colour formats only).
pub fn encode(f: TexFormat, v: [f64; 4], out: &mut Vec<u8>) {
    let unorm = |x: f64, m: f64| (x.clamp(0.0, 1.0) * m).round() as u32;
    match f {
        R11G11B10F => {
            let p = to_small_float(v[0], 6) | (to_small_float(v[1], 6) << 11) | (to_small_float(v[2], 5) << 22);
            out.extend_from_slice(&p.to_le_bytes());
            return;
        }
        Rgb10A2 => {
            let p = unorm(v[0], 1023.0) | (unorm(v[1], 1023.0) << 10) | (unorm(v[2], 1023.0) << 20) | (unorm(v[3], 3.0) << 30);
            out.extend_from_slice(&p.to_le_bytes());
            return;
        }
        _ => {}
    }
    for &x in &v[..channels(f)] {
        match f {
            R8 | Rg8 | Rgba8 | Srgb8A8 => out.push(unorm(x, 255.0) as u8),
            R8Snorm | Rg8Snorm | Rgba8Snorm => out.push((x.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8),
            R16F | Rg16F | Rgba16F => out.extend_from_slice(&f32_to_half(x as f32).to_le_bytes()),
            R32F | Rg32F | Rgba32F => out.extend_from_slice(&(x as f32).to_le_bytes()),
            R8I | Rg8I | Rgba8I => out.push(x.clamp(-128.0, 127.0) as i8 as u8),
            R8U | Rg8U | Rgba8U => out.push(x.clamp(0.0, 255.0) as u8),
            R16I | Rg16I | Rgba16I => out.extend_from_slice(&(x.clamp(-32768.0, 32767.0) as i16).to_le_bytes()),
            R16U | Rg16U | Rgba16U => out.extend_from_slice(&(x.clamp(0.0, 65535.0) as u16).to_le_bytes()),
            R32I | Rg32I | Rgba32I => out.extend_from_slice(&(x as i32).to_le_bytes()),
            R32U | Rg32U | Rgba32U => out.extend_from_slice(&(x.max(0.0) as u32).to_le_bytes()),
            _ => out.push(0),
        }
    }
}

/// Decode one texel of a device format (as returned by `read_pixels`).
pub fn decode_dev(f: TexFormat, p: &[u8]) -> [f64; 4] {
    let n = channels(f);
    let mut c = [0.0, 0.0, 0.0, 1.0];
    match f {
        R11G11B10F => {
            let v = rd(p, 0, 4);
            return [from_small_float(v & 0x7ff, 6), from_small_float((v >> 11) & 0x7ff, 6), from_small_float(v >> 22, 5), 1.0];
        }
        Rgb10A2 => {
            let v = rd(p, 0, 4);
            return [(v & 0x3ff) as f64 / 1023.0, ((v >> 10) & 0x3ff) as f64 / 1023.0, ((v >> 20) & 0x3ff) as f64 / 1023.0, (v >> 30) as f64 / 3.0];
        }
        _ => {}
    }
    let sz = (f.bytes() as usize / n).max(1);
    for (i, slot) in c.iter_mut().enumerate().take(n) {
        let v = rd(p, i, sz);
        *slot = match (class(f), sz) {
            (Class::Norm, _) if matches!(f, R8Snorm | Rg8Snorm | Rgba8Snorm) => (v as u8 as i8 as f64 / 127.0).max(-1.0),
            (Class::Norm, _) => v as f64 / 255.0,
            (Class::Float, 2) => half_to_f32(v as u16) as f64,
            (Class::Float, _) => f32::from_bits(v) as f64,
            (Class::Int, 1) => v as u8 as i8 as f64,
            (Class::Int, 2) => v as u16 as i16 as f64,
            (Class::Int, _) => v as i32 as f64,
            _ => v as f64,
        };
    }
    c
}
