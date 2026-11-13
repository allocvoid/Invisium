//! Paint sources: solid colours, gradients and images (patterns and `drawImage`). Colours
//! here are premultiplied `[r, g, b, a]` in 0..=1.

use crate::geom::{M, P};

pub type C4 = [f32; 4];

pub fn premul(c: [u8; 4]) -> C4 {
    let a = c[3] as f32 / 255.0;
    [c[0] as f32 / 255.0 * a, c[1] as f32 / 255.0 * a, c[2] as f32 / 255.0 * a, a]
}

/// Most colour stops one gradient holds.
pub const MAX_STOPS: usize = 1024;

#[derive(Clone, Copy, Debug)]
pub enum GradKind {
    Linear { p0: P, p1: P },
    Radial { c0: P, r0: f64, c1: P, r1: f64 },
    Conic { angle: f64, c: P },
}

#[derive(Clone, Debug)]
pub struct Gradient {
    pub kind: GradKind,
    pub stops: Vec<(f64, C4)>,
    /// The bytes this gradient holds, charged to its context while it lives.
    pub charge: Option<crate::budget::Charge>,
}

impl Gradient {
    pub fn new(kind: GradKind) -> Gradient {
        Gradient { kind, stops: Vec::new(), charge: None }
    }
    /// Insert after any stops at the same offset (the list grows one slot at a time, so its
    /// capacity is its length).
    pub fn add_stop(&mut self, off: f64, c: [u8; 4]) {
        if self.stops.len() >= MAX_STOPS {
            return;
        }
        if self.stops.len() == self.stops.capacity() {
            self.stops.reserve_exact(1);
        }
        let i = self.stops.partition_point(|s| s.0 <= off);
        self.stops.insert(i, (off, premul(c)));
    }
    fn color_at(&self, t: f64) -> C4 {
        let s = &self.stops;
        let i = s.partition_point(|st| st.0 <= t);
        if i == 0 {
            return s[0].1;
        }
        if i == s.len() {
            return s[s.len() - 1].1;
        }
        let (o0, a) = s[i - 1];
        let (o1, b) = s[i];
        let f = ((t - o0) / (o1 - o0)) as f32;
        [0, 1, 2, 3].map(|k| a[k] + (b[k] - a[k]) * f)
    }
    /// Gradient parameter at user-space point `p`, or `None` where nothing is painted.
    fn param(&self, p: P) -> Option<f64> {
        match self.kind {
            GradKind::Linear { p0, p1 } => {
                let d = p1.sub(p0);
                let l2 = d.dot(d);
                if l2 == 0.0 {
                    return None;
                }
                Some(p.sub(p0).dot(d) / l2)
            }
            GradKind::Radial { c0, r0, c1, r1 } => {
                let cd = c1.sub(c0);
                let pd = p.sub(c0);
                let dr = r1 - r0;
                let a = cd.dot(cd) - dr * dr;
                let b = pd.dot(cd) + r0 * dr;
                let c = pd.dot(pd) - r0 * r0;
                let ok = |w: f64| r0 + w * dr >= 0.0;
                if a.abs() < 1e-12 {
                    if b == 0.0 {
                        return None;
                    }
                    let w = c / (2.0 * b);
                    return if ok(w) { Some(w) } else { None };
                }
                let disc = b * b - a * c;
                if disc < 0.0 {
                    return None;
                }
                let sq = disc.sqrt();
                let (w1, w2) = ((b + sq) / a, (b - sq) / a);
                let (hi, lo) = if w1 > w2 { (w1, w2) } else { (w2, w1) };
                if ok(hi) {
                    Some(hi)
                } else if ok(lo) {
                    Some(lo)
                } else {
                    None
                }
            }
            GradKind::Conic { angle, c } => {
                let a = (p.y - c.y).atan2(p.x - c.x) - angle;
                Some((a / std::f64::consts::TAU).rem_euclid(1.0))
            }
        }
    }
    /// Is the gradient degenerate (paints nothing anywhere)?
    pub fn degenerate(&self) -> bool {
        let d = match self.kind {
            GradKind::Linear { p0, p1 } => p0 == p1,
            GradKind::Radial { c0, r0, c1, r1 } => c0 == c1 && r0 == r1,
            GradKind::Conic { .. } => false,
        };
        d || self.stops.is_empty()
    }
}

/// Straight-alpha RGBA8 image: owned (patterns) or borrowed from the caller (`drawImage`).
#[derive(Clone, Debug)]
pub struct Img<'a> {
    pub w: u32,
    pub h: u32,
    pub px: std::borrow::Cow<'a, [u8]>,
}

impl Img<'_> {
    #[inline]
    fn texel(&self, x: i64, y: i64) -> C4 {
        let i = (y as usize * self.w as usize + x as usize) * 4;
        premul([self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]])
    }
}

/// How sampling treats coordinates outside the image along one axis.
#[derive(Clone, Copy, Debug)]
pub enum Wrap {
    Repeat,
    /// Transparent outside the image.
    Decal,
    /// Clamp to `lo..=hi` (texel indices).
    Clamp(i64, i64),
}

impl Wrap {
    #[inline]
    fn fix(self, v: i64, n: i64) -> Option<i64> {
        match self {
            Wrap::Repeat => Some(v.rem_euclid(n)),
            Wrap::Decal => {
                if v >= 0 && v < n {
                    Some(v)
                } else {
                    None
                }
            }
            Wrap::Clamp(lo, hi) => Some(v.clamp(lo, hi).clamp(0, n - 1)),
        }
    }
}

pub enum Shader<'a> {
    Solid(C4),
    Grad { g: &'a Gradient, inv: M },
    Image { img: &'a Img<'a>, inv: M, smooth: bool, wx: Wrap, wy: Wrap },
}

impl Shader<'_> {
    /// Colours of the pixels `x0 .. x0 + out.len()` of row `y`.
    pub fn shade_row(&self, y: i64, x0: i64, out: &mut [C4]) {
        match self {
            Shader::Solid(c) => out.iter_mut().for_each(|o| *o = *c),
            Shader::Grad { g, inv } => {
                if g.degenerate() {
                    out.iter_mut().for_each(|o| *o = [0.0; 4]);
                    return;
                }
                for (i, o) in out.iter_mut().enumerate() {
                    let p = inv.apply(P::new((x0 + i as i64) as f64 + 0.5, y as f64 + 0.5));
                    *o = match g.param(p) {
                        Some(t) if t.is_finite() => g.color_at(t),
                        _ => [0.0; 4],
                    };
                }
            }
            Shader::Image { img, inv, smooth, wx, wy } => {
                let (w, h) = (img.w as i64, img.h as i64);
                if w == 0 || h == 0 {
                    out.iter_mut().for_each(|o| *o = [0.0; 4]);
                    return;
                }
                let get = |x: i64, y: i64| -> C4 {
                    match (wx.fix(x, w), wy.fix(y, h)) {
                        (Some(x), Some(y)) => img.texel(x, y),
                        _ => [0.0; 4],
                    }
                };
                for (i, o) in out.iter_mut().enumerate() {
                    let p = inv.apply(P::new((x0 + i as i64) as f64 + 0.5, y as f64 + 0.5));
                    if !p.finite() || p.x.abs() > 1e15 || p.y.abs() > 1e15 {
                        *o = [0.0; 4];
                        continue;
                    }
                    if !smooth {
                        *o = get(p.x.floor() as i64, p.y.floor() as i64);
                        continue;
                    }
                    let (u, v) = (p.x - 0.5, p.y - 0.5);
                    let (fu, fv) = (u.floor(), v.floor());
                    let (tx, ty) = ((u - fu) as f32, (v - fv) as f32);
                    let (ix, iy) = (fu as i64, fv as i64);
                    let c00 = get(ix, iy);
                    let c10 = get(ix + 1, iy);
                    let c01 = get(ix, iy + 1);
                    let c11 = get(ix + 1, iy + 1);
                    *o = [0, 1, 2, 3].map(|k| {
                        let top = c00[k] + (c10[k] - c00[k]) * tx;
                        let bot = c01[k] + (c11[k] - c01[k]) * tx;
                        top + (bot - top) * ty
                    });
                }
            }
        }
    }
}
