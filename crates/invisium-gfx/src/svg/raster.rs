//! Scan conversion (non-zero and even-odd, anti-aliased) and stroke outlining.

use super::path::{Poly, Pt};

/// Vertical sub-scanlines per pixel; horizontal coverage is exact.
const SUB: usize = 16;
/// Upper bound on edges in one fill (keeps memory bounded on hostile input).
pub const MAX_EDGES: usize = 400_000;

#[derive(Clone, Copy)]
struct Edge {
    x0: f32,
    y0: f32,
    y1: f32,
    dxdy: f32,
    dir: i32,
}

#[derive(Default)]
pub struct Rasterizer {
    edges: Vec<Edge>,
    active: Vec<usize>,
    xs: Vec<(f32, i32)>,
    part: Vec<f32>,
    delta: Vec<f32>,
}

impl Rasterizer {
    pub fn clear(&mut self) {
        self.edges.clear();
    }

    pub fn line(&mut self, a: Pt, b: Pt) {
        if self.edges.len() >= MAX_EDGES || !(a[0].is_finite() && a[1].is_finite() && b[0].is_finite() && b[1].is_finite()) {
            return;
        }
        if a[1] == b[1] {
            return;
        }
        let (p, q, dir) = if a[1] < b[1] { (a, b, 1) } else { (b, a, -1) };
        let dxdy = (q[0] - p[0]) / (q[1] - p[1]);
        if !dxdy.is_finite() {
            return;
        }
        self.edges.push(Edge { x0: p[0], y0: p[1], y1: q[1], dxdy, dir });
    }

    /// Add a closed polygon (implicitly closed).
    pub fn polygon(&mut self, pts: &[Pt]) {
        if pts.len() < 2 {
            return;
        }
        for i in 0..pts.len() {
            self.line(pts[i], pts[(i + 1) % pts.len()]);
        }
    }

    /// Add a polygon with its orientation normalised, so that the non-zero union of many
    /// such polygons is their geometric union.
    pub fn polygon_oriented(&mut self, pts: &[Pt]) {
        let mut area = 0.0;
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            area += a[0] * b[1] - b[0] * a[1];
        }
        if area >= 0.0 {
            self.polygon(pts);
        } else {
            let rev: Vec<Pt> = pts.iter().rev().copied().collect();
            self.polygon(&rev);
        }
    }

    /// Scan-convert the collected edges and composite `color` (premultiplied, 0..1) onto the
    /// premultiplied RGBA8 `canvas`.
    pub fn fill(&mut self, canvas: &mut [u8], w: usize, h: usize, even_odd: bool, color: [f32; 4]) {
        if self.edges.is_empty() || w == 0 || h == 0 {
            return;
        }
        self.edges.sort_by(|a, b| a.y0.total_cmp(&b.y0));
        let ymin = self.edges[0].y0.max(0.0);
        let ymax = self.edges.iter().fold(f32::MIN, |m, e| m.max(e.y1)).min(h as f32);
        if ymin >= ymax {
            return;
        }
        self.part.clear();
        self.part.resize(w + 1, 0.0);
        self.delta.clear();
        self.delta.resize(w + 2, 0.0);
        self.active.clear();
        let mut next = 0;
        let k = 1.0 / SUB as f32;
        let row0 = ymin.floor() as usize;
        let row1 = (ymax.ceil() as usize).min(h);
        for row in row0..row1 {
            let (top, bottom) = (row as f32, row as f32 + 1.0);
            while next < self.edges.len() && self.edges[next].y0 < bottom {
                self.active.push(next);
                next += 1;
            }
            let edges = &self.edges;
            self.active.retain(|&i| edges[i].y1 > top);
            if self.active.is_empty() {
                continue;
            }
            let mut touched = false;
            for s in 0..SUB {
                let sy = top + (s as f32 + 0.5) * k;
                self.xs.clear();
                for &i in &self.active {
                    let e = &self.edges[i];
                    if e.y0 <= sy && sy < e.y1 {
                        self.xs.push((e.x0 + (sy - e.y0) * e.dxdy, e.dir));
                    }
                }
                if self.xs.is_empty() {
                    continue;
                }
                self.xs.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut wind = 0;
                let mut enter = 0.0;
                for &(x, d) in &self.xs {
                    let was = if even_odd { wind & 1 != 0 } else { wind != 0 };
                    wind += d;
                    let now = if even_odd { wind & 1 != 0 } else { wind != 0 };
                    if !was && now {
                        enter = x;
                    } else if was && !now {
                        span(&mut self.part, &mut self.delta, w, enter, x, k);
                        touched = true;
                    }
                }
            }
            if !touched {
                continue;
            }
            let line = &mut canvas[row * w * 4..(row + 1) * w * 4];
            let mut run = 0.0;
            for x in 0..w {
                run += self.delta[x];
                let cov = (run + self.part[x]).clamp(0.0, 1.0);
                self.part[x] = 0.0;
                self.delta[x] = 0.0;
                if cov <= 0.0 {
                    continue;
                }
                let px = &mut line[x * 4..x * 4 + 4];
                let sa = color[3] * cov;
                for c in 0..4 {
                    let src = color[c] * cov;
                    let dst = px[c] as f32 / 255.0;
                    px[c] = ((src + dst * (1.0 - sa)) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
                }
            }
            self.part[w] = 0.0;
            self.delta[w] = 0.0;
            self.delta[w + 1] = 0.0;
        }
        self.edges.clear();
    }
}

fn span(part: &mut [f32], delta: &mut [f32], w: usize, xa: f32, xb: f32, k: f32) {
    let xa = xa.clamp(0.0, w as f32);
    let xb = xb.clamp(0.0, w as f32);
    if xa.is_nan() || xb.is_nan() || xb <= xa {
        return;
    }
    let ia = xa.floor() as usize;
    let ib = xb.floor() as usize;
    if ia == ib {
        part[ia] += (xb - xa) * k;
        return;
    }
    part[ia] += (ia as f32 + 1.0 - xa) * k;
    delta[ia + 1] += k;
    delta[ib] -= k;
    if ib < w {
        part[ib] += (xb - ib as f32) * k;
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Cap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Join {
    Miter,
    Round,
    Bevel,
}

pub struct StrokeStyle {
    pub width: f32,
    pub cap: Cap,
    pub join: Join,
    pub miter_limit: f32,
}

fn circle(r: &mut Rasterizer, c: Pt, rad: f32) {
    let n = ((rad * 2.0).ceil() as usize).clamp(8, 64);
    let pts: Vec<Pt> = (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            [c[0] + rad * a.cos(), c[1] + rad * a.sin()]
        })
        .collect();
    r.polygon_oriented(&pts);
}

fn norm(d: Pt) -> Option<Pt> {
    let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
    (l > 1e-6 && l.is_finite()).then(|| [d[0] / l, d[1] / l])
}

/// Outline the stroke of `poly` as a set of consistently oriented pieces.
pub fn stroke(r: &mut Rasterizer, poly: &Poly, st: &StrokeStyle) {
    let hw = st.width * 0.5;
    if !(hw > 0.0 && hw.is_finite()) {
        return;
    }
    let mut pts: Vec<Pt> = Vec::with_capacity(poly.pts.len());
    for &p in &poly.pts {
        if pts.last().is_none_or(|l: &Pt| (l[0] - p[0]).abs() > 1e-4 || (l[1] - p[1]).abs() > 1e-4) {
            pts.push(p);
        }
    }
    let closed = poly.closed && pts.len() > 2;
    if closed && pts.len() > 1 && pts[0] == pts[pts.len() - 1] {
        pts.pop();
    }
    if pts.len() == 1 {
        let p = pts[0];
        match st.cap {
            Cap::Round => circle(r, p, hw),
            Cap::Square => r.polygon_oriented(&[[p[0] - hw, p[1] - hw], [p[0] + hw, p[1] - hw], [p[0] + hw, p[1] + hw], [p[0] - hw, p[1] + hw]]),
            Cap::Butt => {}
        }
        return;
    }
    let n = pts.len();
    let nseg = if closed { n } else { n - 1 };
    for i in 0..nseg {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let Some(d) = norm([b[0] - a[0], b[1] - a[1]]) else { continue };
        let nn = [-d[1] * hw, d[0] * hw];
        r.polygon_oriented(&[[a[0] + nn[0], a[1] + nn[1]], [b[0] + nn[0], b[1] + nn[1]], [b[0] - nn[0], b[1] - nn[1]], [a[0] - nn[0], a[1] - nn[1]]]);
    }
    // Joins at interior vertices (and every vertex of a closed path).
    let joins: Box<dyn Iterator<Item = usize>> = if closed { Box::new(0..n) } else { Box::new(1..n - 1) };
    for i in joins {
        let v = pts[i];
        let prev = pts[(i + n - 1) % n];
        let next = pts[(i + 1) % n];
        let (Some(d1), Some(d2)) = (norm([v[0] - prev[0], v[1] - prev[1]]), norm([next[0] - v[0], next[1] - v[1]])) else { continue };
        if st.join == Join::Round {
            circle(r, v, hw);
            continue;
        }
        let n1 = [-d1[1], d1[0]];
        let n2 = [-d2[1], d2[0]];
        let side = if n1[0] * d2[0] + n1[1] * d2[1] > 0.0 { -1.0 } else { 1.0 };
        let o1 = [v[0] + n1[0] * hw * side, v[1] + n1[1] * hw * side];
        let o2 = [v[0] + n2[0] * hw * side, v[1] + n2[1] * hw * side];
        if st.join == Join::Miter {
            if let Some(m) = norm([n1[0] + n2[0], n1[1] + n2[1]]) {
                let cos_half = m[0] * n1[0] + m[1] * n1[1];
                if cos_half > 1e-3 && 1.0 / cos_half <= st.miter_limit {
                    let len = hw / cos_half * side;
                    r.polygon_oriented(&[v, o1, [v[0] + m[0] * len, v[1] + m[1] * len], o2]);
                    continue;
                }
            }
        }
        r.polygon_oriented(&[v, o1, o2]);
    }
    if !closed {
        for (p, q) in [(pts[0], pts[1]), (pts[n - 1], pts[n - 2])] {
            match st.cap {
                Cap::Butt => {}
                Cap::Round => circle(r, p, hw),
                Cap::Square => {
                    let Some(d) = norm([p[0] - q[0], p[1] - q[1]]) else { continue };
                    let nn = [-d[1] * hw, d[0] * hw];
                    let e = [p[0] + d[0] * hw, p[1] + d[1] * hw];
                    r.polygon_oriented(&[[p[0] + nn[0], p[1] + nn[1]], [e[0] + nn[0], e[1] + nn[1]], [e[0] - nn[0], e[1] - nn[1]], [p[0] - nn[0], p[1] - nn[1]]]);
                }
            }
        }
    }
}
