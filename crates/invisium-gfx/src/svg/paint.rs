//! Paint servers: `<linearGradient>` and `<radialGradient>`, resolved through `href` chains
//! into a colour lookup table, and shaded per pixel.

use super::path::{self, Matrix, IDENTITY};
use super::raster::Shade;
use super::tree::Doc;
use super::{attr, color, length, opacity_value, parse_transform};
use crate::draw::Rgba;

/// Lookup-table entries per gradient.
const LUT: usize = 256;
/// Longest `href` chain followed.
const MAX_HREF: usize = 16;

#[derive(Clone, Copy)]
enum Len {
    Num(f32),
    /// Fraction (50 % = 0.5).
    Pct(f32),
}

fn len(s: &str) -> Option<Len> {
    let s = s.trim();
    if let Some(p) = s.strip_suffix('%') {
        let v: f32 = p.trim().parse().ok()?;
        return v.is_finite().then_some(Len::Pct(v / 100.0));
    }
    length(s).map(Len::Num)
}

/// A number or a percentage, for `offset`.
fn fraction(s: &str) -> Option<f32> {
    match len(s)? {
        Len::Num(v) | Len::Pct(v) => Some(v),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Spread {
    Pad,
    Reflect,
    Repeat,
}

pub struct Gradient {
    radial: bool,
    /// Linear: x1 y1 x2 y2 (last unused). Radial: cx cy r fx fy.
    coords: [f32; 5],
    user_space: bool,
    transform: Matrix,
    spread: Spread,
    /// Premultiplied colours for t = 0..=1.
    lut: Vec<[f32; 4]>,
}

fn premul(c: Rgba, opacity: f32) -> [f32; 4] {
    let a = c.a as f32 / 255.0 * opacity;
    [c.r as f32 / 255.0 * a, c.g as f32 / 255.0 * a, c.b as f32 / 255.0 * a, a]
}

/// Collect the stops of gradient `idx` (offsets clamped and non-decreasing).
fn stops(doc: &Doc, idx: u32, current: Rgba) -> Vec<(f32, [f32; 4])> {
    let mut out = Vec::new();
    let mut prev = 0.0f32;
    for c in doc.children(idx) {
        let node = doc.node(c);
        if node.name != "stop" {
            continue;
        }
        let off = attr(&node.attrs, "offset").and_then(fraction).unwrap_or(0.0).clamp(0.0, 1.0).max(prev);
        prev = off;
        let mut color = Rgba::BLACK;
        let mut opacity = 1.0;
        let mut apply = |n: &str, v: &str| {
            let v = v.trim();
            match n.trim().to_ascii_lowercase().as_str() {
                "stop-color" => {
                    if v.eq_ignore_ascii_case("currentcolor") {
                        color = current;
                    } else if let Some(c) = color::parse(v) {
                        color = c;
                    }
                }
                "stop-opacity" => opacity = opacity_value(v).unwrap_or(opacity),
                _ => {}
            }
        };
        for (n, v) in &node.attrs {
            apply(n, v);
        }
        if let Some(css) = attr(&node.attrs, "style") {
            for decl in css.split(';') {
                if let Some((n, v)) = decl.split_once(':') {
                    apply(n, v);
                }
            }
        }
        out.push((off, premul(color, opacity)));
    }
    out
}

fn build_lut(stops: &[(f32, [f32; 4])]) -> Vec<[f32; 4]> {
    let n = stops.len();
    let mut lut = Vec::with_capacity(LUT);
    let mut j = 0;
    for i in 0..LUT {
        let t = i as f32 / (LUT - 1) as f32;
        let c = if t <= stops[0].0 {
            stops[0].1
        } else {
            while j + 1 < n && stops[j + 1].0 <= t {
                j += 1;
            }
            if j + 1 >= n {
                stops[n - 1].1
            } else {
                let ((o0, c0), (o1, c1)) = (stops[j], stops[j + 1]);
                let f = ((t - o0) / (o1 - o0)).clamp(0.0, 1.0);
                [0, 1, 2, 3].map(|k| c0[k] + (c1[k] - c0[k]) * f)
            }
        };
        lut.push(c);
    }
    lut
}

/// Resolve gradient element `idx` (following `href`). `None` = paints nothing (no stops).
/// `viewport` is the size percentages resolve against in `userSpaceOnUse`.
pub fn resolve(doc: &Doc, idx: u32, viewport: (f32, f32), current: Rgba) -> Option<Gradient> {
    let mut chain = vec![idx];
    let mut cur = idx;
    while chain.len() < MAX_HREF {
        let Some(n) = doc.href(cur) else { break };
        if chain.contains(&n) || !doc.is_gradient(n) {
            break;
        }
        chain.push(n);
        cur = n;
    }
    let kind = doc.node(idx).name.as_str();
    let radial = kind == "radialgradient";
    // First parsable value of `name` along the chain (coordinates only from the same kind).
    let find = |name: &str, same_kind: bool| {
        chain
            .iter()
            .filter(|&&i| !same_kind || doc.node(i).name == kind)
            .filter_map(|&i| attr(&doc.node(i).attrs, name))
            .find_map(|v| Some(v.trim().to_string()).filter(|s| !s.is_empty()))
    };
    let coord = |name: &str| find(name, true).as_deref().and_then(len);
    let user_space = find("gradientUnits", false).is_some_and(|v| v.eq_ignore_ascii_case("userspaceonuse"));
    let transform = chain
        .iter()
        .filter_map(|&i| attr(&doc.node(i).attrs, "gradientTransform"))
        .find_map(parse_transform)
        .unwrap_or(IDENTITY);
    let spread = match find("spreadMethod", false).map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("reflect") => Spread::Reflect,
        Some("repeat") => Spread::Repeat,
        _ => Spread::Pad,
    };
    let stop_list = chain.iter().map(|&i| stops(doc, i, current)).find(|s| !s.is_empty())?;

    let (vw, vh) = viewport;
    let diag = ((vw * vw + vh * vh) * 0.5).sqrt();
    let res = |l: Len, extent: f32| match l {
        Len::Num(v) => v,
        Len::Pct(p) => if user_space { p * extent } else { p },
    };
    let coords = if radial {
        let cx = res(coord("cx").unwrap_or(Len::Pct(0.5)), vw);
        let cy = res(coord("cy").unwrap_or(Len::Pct(0.5)), vh);
        let r = res(coord("r").unwrap_or(Len::Pct(0.5)), diag);
        let fx = coord("fx").map_or(cx, |l| res(l, vw));
        let fy = coord("fy").map_or(cy, |l| res(l, vh));
        [cx, cy, r, fx, fy]
    } else {
        let x1 = res(coord("x1").unwrap_or(Len::Num(0.0)), vw);
        let y1 = res(coord("y1").unwrap_or(Len::Num(0.0)), vh);
        let x2 = res(coord("x2").unwrap_or(Len::Pct(1.0)), vw);
        let y2 = res(coord("y2").unwrap_or(Len::Num(0.0)), vh);
        [x1, y1, x2, y2, 0.0]
    };
    Some(Gradient { radial, coords, user_space, transform, spread, lut: build_lut(&stop_list) })
}

fn invert(m: &Matrix) -> Option<Matrix> {
    let det = m[0] * m[3] - m[1] * m[2];
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    let inv = [a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])];
    inv.iter().all(|v| v.is_finite()).then_some(inv)
}

impl Gradient {
    pub fn needs_bbox(&self) -> bool {
        !self.user_space
    }

    /// A shader for a shape drawn through `m` whose geometry has bounding box `bbox`
    /// (`[x0, y0, x1, y1]` in its own user space). `None` = nothing is painted.
    pub fn shader(&self, m: &Matrix, bbox: Option<[f32; 4]>, alpha: f32) -> Option<Shader<'_>> {
        let mut g = *m;
        if !self.user_space {
            let [x0, y0, x1, y1] = bbox?;
            let (w, h) = (x1 - x0, y1 - y0);
            if !(w > 0.0 && h > 0.0) {
                return None;
            }
            g = path::mul(&g, &[w, 0.0, 0.0, h, x0, y0]);
        }
        g = path::mul(&g, &self.transform);
        let inv = invert(&g)?;
        let [a, b, c, d, e] = self.coords;
        let geom = if self.radial {
            let (cx, cy, r) = (a, b, c);
            let (mut fx, mut fy) = (d, e);
            let (dx, dy) = (cx - fx, cy - fy);
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > r * 0.99 && dist > 0.0 {
                let k = r * 0.99 / dist;
                fx = cx - dx * k;
                fy = cy - dy * k;
            }
            Geom::Radial { f: [fx, fy], d: [cx - fx, cy - fy], r }
        } else {
            let (dx, dy) = (c - a, d - b);
            Geom::Linear { p: [a, b], d: [dx, dy], dd: dx * dx + dy * dy }
        };
        let finite = match geom {
            Geom::Linear { p, d, dd } => p.iter().chain(&d).all(|v| v.is_finite()) && dd.is_finite(),
            Geom::Radial { f, d, r } => f.iter().chain(&d).all(|v| v.is_finite()) && r.is_finite(),
        };
        finite.then_some(Shader { g: self, inv, geom, alpha })
    }
}

#[derive(Clone, Copy)]
enum Geom {
    Linear { p: [f32; 2], d: [f32; 2], dd: f32 },
    Radial { f: [f32; 2], d: [f32; 2], r: f32 },
}

pub struct Shader<'g> {
    g: &'g Gradient,
    inv: Matrix,
    geom: Geom,
    alpha: f32,
}

impl Shader<'_> {
    fn t(&self, p: [f32; 2]) -> f32 {
        match self.geom {
            Geom::Linear { p: p0, d, dd } => {
                if dd <= 1e-12 {
                    return 1.0;
                }
                ((p[0] - p0[0]) * d[0] + (p[1] - p0[1]) * d[1]) / dd
            }
            Geom::Radial { f, d, r } => {
                if r <= 0.0 {
                    return 1.0;
                }
                // Circle through p centred on f + t*d with radius t*r.
                let q = [p[0] - f[0], p[1] - f[1]];
                let qd = q[0] * d[0] + q[1] * d[1];
                let qq = q[0] * q[0] + q[1] * q[1];
                let a = d[0] * d[0] + d[1] * d[1] - r * r;
                if a.abs() < 1e-9 {
                    return if qd.abs() > 1e-12 { qq / (2.0 * qd) } else { 0.0 };
                }
                (qd - (qd * qd - a * qq).max(0.0).sqrt()) / a
            }
        }
    }
}

impl Shade for Shader<'_> {
    fn at(&self, x: usize, y: usize) -> [f32; 4] {
        let p = path::apply(&self.inv, [x as f32 + 0.5, y as f32 + 0.5]);
        let t = self.t(p);
        let t = if t.is_finite() { t } else { 0.0 };
        let t = match self.g.spread {
            Spread::Pad => t.clamp(0.0, 1.0),
            Spread::Repeat => t - t.floor(),
            Spread::Reflect => {
                let m = t.rem_euclid(2.0);
                if m > 1.0 { 2.0 - m } else { m }
            }
        };
        let i = ((t * (LUT - 1) as f32).round() as usize).min(LUT - 1);
        let c = self.g.lut[i];
        [c[0] * self.alpha, c[1] * self.alpha, c[2] * self.alpha, c[3] * self.alpha]
    }
}
