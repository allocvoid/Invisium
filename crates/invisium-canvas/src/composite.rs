//! The bitmap, compositing operators and blend modes, and the per-pixel draw loop.

use crate::geom::R;
use crate::paint::{Shader, C4};
use crate::raster::Mask;

/// Straight-alpha RGBA8, top row first.
pub struct Bitmap {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Bitmap {
    pub fn rect(&self) -> R {
        R::new(0, 0, self.w as i64, self.h as i64)
    }
    #[inline]
    pub fn get(&self, i: usize) -> C4 {
        let p = &self.px[i * 4..i * 4 + 4];
        let a = p[3] as f32 / 255.0;
        [p[0] as f32 / 255.0 * a, p[1] as f32 / 255.0 * a, p[2] as f32 / 255.0 * a, a]
    }
    #[inline]
    pub fn set(&mut self, i: usize, c: C4) {
        let a = c[3].clamp(0.0, 1.0);
        let p = &mut self.px[i * 4..i * 4 + 4];
        if a <= 0.0 {
            p.copy_from_slice(&[0, 0, 0, 0]);
            return;
        }
        for k in 0..3 {
            p[k] = ((c[k] / a).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
        p[3] = (a * 255.0 + 0.5) as u8;
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    SrcOver,
    SrcIn,
    SrcOut,
    SrcAtop,
    DstOver,
    DstIn,
    DstOut,
    DstAtop,
    Lighter,
    Copy,
    Xor,
    Blend(u8),
}

pub const OP_NAMES: [&str; 11] = [
    "source-over",
    "source-in",
    "source-out",
    "source-atop",
    "destination-over",
    "destination-in",
    "destination-out",
    "destination-atop",
    "lighter",
    "copy",
    "xor",
];
pub const BLEND_NAMES: [&str; 15] = [
    "multiply",
    "screen",
    "overlay",
    "darken",
    "lighten",
    "color-dodge",
    "color-burn",
    "hard-light",
    "soft-light",
    "difference",
    "exclusion",
    "hue",
    "saturation",
    "color",
    "luminosity",
];
const OPS: [Op; 11] = [
    Op::SrcOver,
    Op::SrcIn,
    Op::SrcOut,
    Op::SrcAtop,
    Op::DstOver,
    Op::DstIn,
    Op::DstOut,
    Op::DstAtop,
    Op::Lighter,
    Op::Copy,
    Op::Xor,
];

impl Op {
    pub fn parse(s: &str) -> Option<Op> {
        if let Some(i) = OP_NAMES.iter().position(|n| *n == s) {
            return Some(OPS[i]);
        }
        BLEND_NAMES.iter().position(|n| *n == s).map(|i| Op::Blend(i as u8))
    }
    pub fn name(self) -> &'static str {
        match self {
            Op::Blend(i) => BLEND_NAMES[i as usize % 15],
            o => OP_NAMES[OPS.iter().position(|x| *x == o).unwrap_or(0)],
        }
    }
    /// Operators that change pixels outside the drawn shape.
    pub fn unbounded(self) -> bool {
        matches!(self, Op::SrcIn | Op::SrcOut | Op::DstIn | Op::DstAtop | Op::Copy)
    }
    /// Composite premultiplied `s` onto `d`.
    pub fn apply(self, s: C4, d: C4) -> C4 {
        let (sa, da) = (s[3], d[3]);
        let pd = |fa: f32, fb: f32| [0, 1, 2, 3].map(|k| s[k] * fa + d[k] * fb);
        match self {
            Op::SrcOver => pd(1.0, 1.0 - sa),
            Op::SrcIn => pd(da, 0.0),
            Op::SrcOut => pd(1.0 - da, 0.0),
            Op::SrcAtop => pd(da, 1.0 - sa),
            Op::DstOver => pd(1.0 - da, 1.0),
            Op::DstIn => pd(0.0, sa),
            Op::DstOut => pd(0.0, 1.0 - sa),
            Op::DstAtop => pd(1.0 - da, sa),
            Op::Lighter => pd(1.0, 1.0).map(|v| v.min(1.0)),
            Op::Copy => s,
            Op::Xor => pd(1.0 - da, 1.0 - sa),
            Op::Blend(m) => {
                let un = |c: C4| {
                    if c[3] > 0.0 {
                        [c[0] / c[3], c[1] / c[3], c[2] / c[3]]
                    } else {
                        [0.0; 3]
                    }
                };
                let b = blend(m, un(d), un(s));
                let mut o = [0.0; 4];
                for k in 0..3 {
                    o[k] = s[k] * (1.0 - da) + d[k] * (1.0 - sa) + sa * da * b[k];
                }
                o[3] = sa + da - sa * da;
                o
            }
        }
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    let c = c.map(|v| v + d);
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    c.map(|v| {
        let mut v = v;
        if n < 0.0 && l - n != 0.0 {
            v = l + (v - l) * l / (l - n);
        }
        if x > 1.0 && x - l != 0.0 {
            v = l + (v - l) * (1.0 - l) / (x - l);
        }
        v
    })
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    if mx > mn {
        c.map(|v| (v - mn) * s / (mx - mn))
    } else {
        [0.0; 3]
    }
}

/// Blend function `B(cb, cs)` on straight colours.
fn blend(m: u8, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    let sep = |f: &dyn Fn(f32, f32) -> f32| [f(cb[0], cs[0]), f(cb[1], cs[1]), f(cb[2], cs[2])];
    let screen = |b: f32, s: f32| b + s - b * s;
    let hard = |b: f32, s: f32| if s <= 0.5 { b * 2.0 * s } else { screen(b, 2.0 * s - 1.0) };
    match m {
        0 => sep(&|b, s| b * s),
        1 => sep(&screen),
        2 => sep(&|b, s| hard(s, b)),
        3 => sep(&|b: f32, s: f32| b.min(s)),
        4 => sep(&|b: f32, s: f32| b.max(s)),
        5 => sep(&|b, s| {
            if b == 0.0 {
                0.0
            } else if s >= 1.0 {
                1.0
            } else {
                (b / (1.0 - s)).min(1.0)
            }
        }),
        6 => sep(&|b, s| {
            if b >= 1.0 {
                1.0
            } else if s <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - b) / s).min(1.0)
            }
        }),
        7 => sep(&hard),
        8 => sep(&|b: f32, s: f32| {
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                let d = if b <= 0.25 { ((16.0 * b - 12.0) * b + 4.0) * b } else { b.sqrt() };
                b + (2.0 * s - 1.0) * (d - b)
            }
        }),
        9 => sep(&|b: f32, s: f32| (b - s).abs()),
        10 => sep(&|b, s| b + s - 2.0 * b * s),
        11 => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        12 => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        13 => set_lum(cs, lum(cb)),
        _ => set_lum(cb, lum(cs)),
    }
}

/// Draw `shader` through `mask` (times `alpha`) with `op`, limited by `clip`.
pub fn composite(bmp: &mut Bitmap, mask: &Mask, shader: &Shader, op: Op, alpha: f32, clip: Option<&Mask>) {
    let mut region = bmp.rect();
    if !op.unbounded() {
        region = region.intersect(&mask.r);
    }
    if let Some(c) = clip {
        region = region.intersect(&c.r);
    }
    if region.is_empty() || (alpha.is_nan() || alpha <= 0.0) && !op.unbounded() {
        return;
    }
    let alpha = if alpha.is_finite() { alpha.clamp(0.0, 1.0) } else { 0.0 };
    let w = region.width();
    let mut row: Vec<C4> = vec![[0.0; 4]; w];
    let bw = bmp.w as usize;
    for y in region.y0..region.y1 {
        let any = (region.x0..region.x1).any(|x| mask.get(x, y) != 0);
        if any {
            shader.shade_row(y, region.x0, &mut row);
        } else if !op.unbounded() {
            continue;
        }
        for (i, x) in (region.x0..region.x1).enumerate() {
            let cov = mask.get(x, y) as f32 / 255.0 * alpha;
            let cl = clip.map_or(1.0, |c| c.get(x, y) as f32 / 255.0);
            if cl == 0.0 || (cov == 0.0 && !op.unbounded()) {
                continue;
            }
            let s = if cov > 0.0 { row[i].map(|v| v * cov) } else { [0.0; 4] };
            let idx = y as usize * bw + x as usize;
            let d = bmp.get(idx);
            let r = op.apply(s, d);
            let out = if cl >= 1.0 { r } else { [0, 1, 2, 3].map(|k| d[k] + (r[k] - d[k]) * cl) };
            bmp.set(idx, out);
        }
    }
}

/// Erase through `mask` (clearRect), limited by `clip`.
pub fn clear(bmp: &mut Bitmap, mask: &Mask, clip: Option<&Mask>) {
    let mut region = bmp.rect().intersect(&mask.r);
    if let Some(c) = clip {
        region = region.intersect(&c.r);
    }
    let bw = bmp.w as usize;
    for y in region.y0..region.y1 {
        for x in region.x0..region.x1 {
            let k = mask.get(x, y) as f32 / 255.0 * clip.map_or(1.0, |c| c.get(x, y) as f32 / 255.0);
            if k <= 0.0 {
                continue;
            }
            let idx = y as usize * bw + x as usize;
            let d = bmp.get(idx);
            bmp.set(idx, d.map(|v| v * (1.0 - k)));
        }
    }
}
