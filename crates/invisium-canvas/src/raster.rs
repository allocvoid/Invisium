//! Scan conversion with exact area coverage (signed-area accumulation, processed in bands),
//! coverage masks, and point-in-polygon tests.

use crate::geom::{P, R};

/// An 8-bit coverage mask over a pixel rectangle; outside it coverage is 0.
#[derive(Clone, Debug)]
pub struct Mask {
    pub r: R,
    pub data: Vec<u8>,
}

impl Mask {
    pub fn empty() -> Mask {
        Mask { r: R::new(0, 0, 0, 0), data: Vec::new() }
    }
    pub fn new(r: R) -> Mask {
        let r = if r.is_empty() { R::new(0, 0, 0, 0) } else { r };
        Mask { r, data: vec![0; r.width() * r.height()] }
    }
    #[inline]
    pub fn get(&self, x: i64, y: i64) -> u8 {
        if x < self.r.x0 || y < self.r.y0 || x >= self.r.x1 || y >= self.r.y1 {
            return 0;
        }
        self.data[(y - self.r.y0) as usize * self.r.width() + (x - self.r.x0) as usize]
    }
    #[inline]
    pub fn add(&mut self, x: i64, y: i64, v: u8) {
        if x < self.r.x0 || y < self.r.y0 || x >= self.r.x1 || y >= self.r.y1 {
            return;
        }
        let i = (y - self.r.y0) as usize * self.r.width() + (x - self.r.x0) as usize;
        self.data[i] = self.data[i].saturating_add(v);
    }
    /// The same mask shifted by whole pixels.
    pub fn shifted(mut self, dx: i64, dy: i64) -> Mask {
        self.r = self.r.shift(dx, dy);
        self
    }
    /// Product of two masks (clip intersection).
    pub fn intersect(&self, o: &Mask) -> Mask {
        let r = self.r.intersect(&o.r);
        let mut m = Mask::new(r);
        if m.r.is_empty() {
            return m;
        }
        let w = m.r.width();
        let row = |k: &Mask, y: i64| {
            let s = (y - k.r.y0) as usize * k.r.width() + (r.x0 - k.r.x0) as usize;
            s..s + w
        };
        for y in r.y0..r.y1 {
            let (a, b) = (&self.data[row(self, y)], &o.data[row(o, y)]);
            let out = &mut m.data[(y - r.y0) as usize * w..(y - r.y0 + 1) as usize * w];
            for ((d, &p), &q) in out.iter_mut().zip(a).zip(b) {
                *d = ((p as u32 * q as u32 + 127) / 255) as u8;
            }
        }
        m
    }
}

/// An edge clipped to the mask's region (so `f32` is precise enough).
#[derive(Clone, Copy)]
struct Edge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    dir: f32,
}

const BAND: usize = 16;
/// Most edges one rasterisation keeps (20 bytes each, plus a 4-byte index while active);
/// edges past it are dropped.
pub const MAX_EDGES: usize = 2_000_000;

/// Rasterise closed polygons (each implicitly closed) inside `clip` (device pixels).
pub fn fill_mask(polys: &[Vec<P>], evenodd: bool, clip: R) -> Mask {
    fill_mask_n(polys, evenodd, clip).0
}

/// [`fill_mask`], also returning the bytes of working memory it held (edges, active list
/// and accumulation band).
pub fn fill_mask_n(polys: &[Vec<P>], evenodd: bool, clip: R) -> (Mask, usize) {
    if clip.is_empty() {
        return (Mask::empty(), 0);
    }
    let (cx0, cx1) = (clip.x0 as f64, clip.x1 as f64);
    let (cy0, cy1) = (clip.y0 as f64, clip.y1 as f64);
    // Each chord keeps at most two pieces (a vertical one at the left edge and one inside);
    // the list is reserved once.
    let pts: usize = polys.iter().map(|p| p.len()).sum();
    let mut edges: Vec<Edge> = Vec::new();
    edges.reserve_exact(pts.saturating_mul(2).min(MAX_EDGES));
    let (mut minx, mut maxx, mut miny, mut maxy) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for poly in polys {
        let n = poly.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let a = poly[i];
            let b = poly[(i + 1) % n];
            if !a.finite() || !b.finite() || a.y == b.y {
                continue;
            }
            let (dir, mut p, mut q) = if a.y < b.y { (1.0f32, a, b) } else { (-1.0, b, a) };
            if q.y <= cy0 || p.y >= cy1 {
                continue;
            }
            // Clip vertically.
            let dxdy = (q.x - p.x) / (q.y - p.y);
            if p.y < cy0 {
                p = P::new(p.x + (cy0 - p.y) * dxdy, cy0);
            }
            if q.y > cy1 {
                q = P::new(p.x + (cy1 - p.y) * dxdy, cy1);
            }
            // Split horizontally at the clip's left and right edges.
            let mut pieces = [(p, q); 3];
            let mut np = 0;
            let mut cut_buf = [0.0f64, 1.0, 1.0, 1.0];
            let mut nc = 2;
            for xc in [cx0, cx1] {
                if (p.x < xc) != (q.x < xc) && p.x != q.x {
                    cut_buf[nc] = (xc - p.x) / (q.x - p.x);
                    nc += 1;
                }
            }
            let cuts = &mut cut_buf[..nc];
            cuts.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            for w in cuts.windows(2) {
                if w[1] <= w[0] || np >= 3 {
                    continue;
                }
                let lerp = |t: f64| P::new(p.x + (q.x - p.x) * t, p.y + (q.y - p.y) * t);
                pieces[np] = (lerp(w[0]), lerp(w[1]));
                np += 1;
            }
            for &(s, e) in &pieces[..np] {
                let mid = (s.x + e.x) / 2.0;
                if mid >= cx1 {
                    maxx = cx1;
                    continue;
                }
                let (sx, ex) = if mid <= cx0 { (cx0, cx0) } else { (s.x.clamp(cx0, cx1), e.x.clamp(cx0, cx1)) };
                if e.y <= s.y {
                    continue;
                }
                if edges.len() >= MAX_EDGES {
                    break;
                }
                minx = minx.min(sx.min(ex));
                maxx = maxx.max(sx.max(ex));
                miny = miny.min(s.y);
                maxy = maxy.max(e.y);
                edges.push(Edge { x0: sx as f32, y0: s.y as f32, x1: ex as f32, y1: e.y as f32, dir });
            }
        }
    }
    let edge_bytes = edges.capacity() * std::mem::size_of::<Edge>();
    if edges.is_empty() {
        return (Mask::empty(), edge_bytes);
    }
    let r = R::new(
        (minx.floor() as i64).max(clip.x0),
        (miny.floor() as i64).max(clip.y0),
        ((maxx.ceil() as i64) + 1).min(clip.x1),
        (maxy.ceil() as i64).min(clip.y1),
    );
    let mut mask = Mask::new(r);
    if mask.r.is_empty() {
        return (mask, edge_bytes);
    }
    let w = r.width();
    let h = r.height();
    let stride = w + 2;
    edges.sort_unstable_by(|a, b| a.y0.partial_cmp(&b.y0).unwrap_or(std::cmp::Ordering::Equal));
    let mut acc = vec![0.0f32; stride * BAND];
    let mut next = 0;
    let mut active: Vec<u32> = Vec::new();
    active.reserve_exact(edges.len());
    let mut by = 0;
    while by < h {
        let bh = BAND.min(h - by);
        let yb0 = (r.y0 + by as i64) as f64;
        let yb1 = yb0 + bh as f64;
        active.retain(|&i| edges[i as usize].y1 as f64 > yb0);
        while next < edges.len() && (edges[next].y0 as f64) < yb1 {
            if edges[next].y1 as f64 > yb0 {
                active.push(next as u32);
            }
            next += 1;
        }
        for v in acc.iter_mut() {
            *v = 0.0;
        }
        for &i in &active {
            draw_edge(&mut acc, stride, r.x0 as f64, yb0, bh, &edges[i as usize]);
        }
        for row in 0..bh {
            let a = &acc[row * stride..row * stride + w];
            let out = &mut mask.data[(by + row) * w..(by + row + 1) * w];
            let mut s = 0.0f32;
            for (o, v) in out.iter_mut().zip(a) {
                s += v;
                let c = if evenodd {
                    let t = s.abs() % 2.0;
                    if t > 1.0 {
                        2.0 - t
                    } else {
                        t
                    }
                } else {
                    s.abs().min(1.0)
                };
                *o = (c * 255.0 + 0.5) as u8;
            }
        }
        by += bh;
    }
    let work = edge_bytes + active.capacity() * 4 + acc.capacity() * 4;
    (mask, work)
}

/// Accumulate one edge's signed area into the band buffer (rows `yb0 .. yb0 + bh`).
fn draw_edge(acc: &mut [f32], stride: usize, ox: f64, yb0: f64, bh: usize, e: &Edge) {
    let yb1 = yb0 + bh as f64;
    let (ex0, ey0, ex1, ey1) = (e.x0 as f64, e.y0 as f64, e.x1 as f64, e.y1 as f64);
    let y0 = ey0.max(yb0);
    let y1 = ey1.min(yb1);
    if y1 <= y0 {
        return;
    }
    let dxdy = (ex1 - ex0) / (ey1 - ey0);
    let mut x = ex0 + (y0 - ey0) * dxdy - ox;
    let maxi = stride as i64 - 1;
    let mut y = y0;
    let mut row = (y0 - yb0).floor() as usize;
    while y < y1 && row < bh {
        let ynext = ((yb0 + row as f64) + 1.0).min(y1);
        let dy = ynext - y;
        let xnext = x + dxdy * dy;
        let d = (dy as f32) * e.dir;
        let ls = row * stride;
        let (xa, xb) = if x < xnext { (x, xnext) } else { (xnext, x) };
        let xa = xa.max(0.0);
        let xb = xb.max(0.0);
        let x0f = xa.floor();
        let x0i = x0f as i64;
        let x1c = xb.ceil();
        let x1i = (x1c as i64).max(x0i + 1);
        let at = |i: i64| ls + i.clamp(0, maxi) as usize;
        if x1i <= x0i + 1 {
            let xmf = (0.5 * (xa + xb) - x0f) as f32;
            acc[at(x0i)] += d - d * xmf;
            acc[at(x0i + 1)] += d * xmf;
        } else {
            let s = (1.0 / (xb - xa)) as f32;
            let x0frac = (xa - x0f) as f32;
            let a0 = 0.5 * s * (1.0 - x0frac) * (1.0 - x0frac);
            let x1frac = (xb - x1c + 1.0) as f32;
            let am = 0.5 * s * x1frac * x1frac;
            acc[at(x0i)] += d * a0;
            if x1i == x0i + 2 {
                acc[at(x0i + 1)] += d * (1.0 - a0 - am);
            } else {
                let a1 = s * (1.5 - x0frac);
                acc[at(x0i + 1)] += d * (a1 - a0);
                for xi in x0i + 2..x1i - 1 {
                    acc[at(xi)] += d * s;
                }
                let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                acc[at(x1i - 1)] += d * (1.0 - a2 - am);
            }
            acc[at(x1i)] += d * am;
        }
        x = xnext;
        y = ynext;
        row += 1;
    }
}

/// Winding number of `p` against closed polygons.
pub fn winding(polys: &[Vec<P>], p: P) -> i32 {
    let mut w = 0i32;
    for poly in polys {
        let n = poly.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let a = poly[i];
            let b = poly[(i + 1) % n];
            if a.y <= p.y {
                if b.y > p.y && b.sub(a).cross(p.sub(a)) > 0.0 {
                    w += 1;
                }
            } else if b.y <= p.y && b.sub(a).cross(p.sub(a)) < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

/// Is `p` inside, by the given rule?
pub fn contains(polys: &[Vec<P>], p: P, evenodd: bool) -> bool {
    let w = winding(polys, p);
    if evenodd {
        w % 2 != 0
    } else {
        w != 0
    }
}
