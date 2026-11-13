//! Path construction (the `CanvasPath` methods) and flattening into polylines.
//!
//! Points are mapped through a matrix when they are added: the context's own path stores
//! device coordinates, `Path2D` objects store their own (user) coordinates.

use crate::geom::{M, P};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

/// Most segments one path holds; later additions are dropped.
pub const MAX_SEGS: usize = 1_100_000;
/// Points one flattening produces before curves fall back to chords.
const MAX_FLAT: usize = 1_000_000;
/// Most points one flattening produces (with every segment of a full path as a chord).
pub const MAX_FLAT_PTS: usize = 2_200_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move(P),
    Line(P),
    Quad(P, P),
    Cubic(P, P, P),
    Close,
}

#[derive(Clone, Debug)]
pub struct Path {
    pub segs: Vec<Seg>,
    start: P,
    last: P,
    has_sub: bool,
    /// Most segments this path may allocate room for (the owner lowers it to fit its budget).
    pub limit: usize,
}

impl Default for Path {
    fn default() -> Path {
        Path { segs: Vec::new(), start: P::default(), last: P::default(), has_sub: false, limit: MAX_SEGS }
    }
}

/// Bytes one stored segment counts for.
pub const SEG_BYTES: usize = std::mem::size_of::<Seg>();

/// One flattened subpath.
#[derive(Clone, Debug, Default)]
pub struct Poly {
    pub pts: Vec<P>,
    pub closed: bool,
}

impl Path {
    pub fn new() -> Path {
        Path::default()
    }
    pub fn clear(&mut self) {
        *self = Path::default();
    }
    pub fn len(&self) -> usize {
        self.segs.len()
    }
    fn push(&mut self, s: Seg) {
        let lim = self.limit.min(MAX_SEGS);
        let len = self.segs.len();
        if len >= lim {
            return;
        }
        if len == self.segs.capacity() {
            // Grow geometrically, but never past the limit (which bounds the capacity).
            self.segs.reserve_exact(len.max(8).min(lim - len));
        }
        self.segs.push(s);
    }
    pub fn move_to(&mut self, m: &M, x: f64, y: f64) {
        let p = m.apply(P::new(x, y));
        self.move_dev(p);
    }
    fn move_dev(&mut self, p: P) {
        if let Some(Seg::Move(_)) = self.segs.last() {
            self.segs.pop();
        }
        self.push(Seg::Move(p));
        self.start = p;
        self.last = p;
        self.has_sub = true;
    }
    fn ensure(&mut self, m: &M, x: f64, y: f64) {
        if !self.has_sub {
            self.move_to(m, x, y);
        }
    }
    pub fn line_to(&mut self, m: &M, x: f64, y: f64) {
        let p = m.apply(P::new(x, y));
        self.line_dev(p);
    }
    fn line_dev(&mut self, p: P) {
        if !self.has_sub {
            self.move_dev(p);
            return;
        }
        self.push(Seg::Line(p));
        self.last = p;
    }
    pub fn quad_to(&mut self, m: &M, cx: f64, cy: f64, x: f64, y: f64) {
        self.ensure(m, cx, cy);
        let p = m.apply(P::new(x, y));
        self.push(Seg::Quad(m.apply(P::new(cx, cy)), p));
        self.last = p;
    }
    pub fn cubic_to(&mut self, m: &M, c: [f64; 6]) {
        self.ensure(m, c[0], c[1]);
        let p = m.apply(P::new(c[4], c[5]));
        self.push(Seg::Cubic(m.apply(P::new(c[0], c[1])), m.apply(P::new(c[2], c[3])), p));
        self.last = p;
    }
    pub fn close(&mut self) {
        if !self.has_sub {
            return;
        }
        if let Some(Seg::Move(_)) = self.segs.last() {
            return;
        }
        self.push(Seg::Close);
        let s = self.start;
        self.push(Seg::Move(s));
        self.last = s;
    }
    pub fn rect(&mut self, m: &M, x: f64, y: f64, w: f64, h: f64) {
        self.move_to(m, x, y);
        self.line_to(m, x + w, y);
        self.line_to(m, x + w, y + h);
        self.line_to(m, x, y + h);
        self.close();
    }

    /// `ellipse()` / `arc()` after argument checks: rotation `rot`, angles `a0` → `a1`.
    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(&mut self, m: &M, c: P, rx: f64, ry: f64, rot: f64, a0: f64, a1: f64, ccw: bool) {
        let mut sweep = a1 - a0;
        if !ccw {
            sweep = if sweep >= TAU { TAU } else { sweep.rem_euclid(TAU) };
        } else {
            sweep = if -sweep >= TAU { -TAU } else { -((-sweep).rem_euclid(TAU)) };
        }
        if !sweep.is_finite() {
            return;
        }
        let (rs, rc) = rot.sin_cos();
        // Unit circle → ellipse → current map.
        let em = m.concat(&M([rx * rc, rx * rs, -ry * rs, ry * rc, c.x, c.y]));
        let a0 = a0.rem_euclid(TAU);
        let start = em.apply(P::new(a0.cos(), a0.sin()));
        if self.has_sub {
            self.line_dev(start);
        } else {
            self.move_dev(start);
        }
        if sweep == 0.0 {
            return;
        }
        let n = (sweep.abs() / FRAC_PI_2).ceil().clamp(1.0, 4.0) as usize;
        let step = sweep / n as f64;
        let k = 4.0 / 3.0 * (step / 4.0).tan();
        let mut t = a0;
        for _ in 0..n {
            let t1 = t + step;
            let (s0, c0) = t.sin_cos();
            let (s1, c1) = t1.sin_cos();
            let p1 = P::new(c0 - k * s0, s0 + k * c0);
            let p2 = P::new(c1 + k * s1, s1 - k * c1);
            let p3 = P::new(c1, s1);
            self.push(Seg::Cubic(em.apply(p1), em.apply(p2), em.apply(p3)));
            self.last = em.apply(p3);
            t = t1;
        }
    }

    /// `arcTo()` after argument checks; `inv` maps stored coordinates back to user space.
    pub fn arc_to(&mut self, m: &M, inv: Option<M>, p1: P, p2: P, r: f64) {
        self.ensure(m, p1.x, p1.y);
        let p0 = match inv {
            Some(inv) => inv.apply(self.last),
            None => {
                self.line_to(m, p1.x, p1.y);
                return;
            }
        };
        let (u0, u2) = match (p0.sub(p1).unit(), p2.sub(p1).unit()) {
            (Some(a), Some(b)) => (a, b),
            _ => {
                self.line_to(m, p1.x, p1.y);
                return;
            }
        };
        let cross = u0.cross(u2);
        if r == 0.0 || cross.abs() < 1e-12 {
            self.line_to(m, p1.x, p1.y);
            return;
        }
        let cos = u0.dot(u2).clamp(-1.0, 1.0);
        let theta = cos.acos();
        let d = r / (theta / 2.0).tan();
        let t1 = p1.add(u0.mul(d));
        let t2 = p1.add(u2.mul(d));
        let bis = match u0.add(u2).unit() {
            Some(b) => b,
            None => {
                self.line_to(m, p1.x, p1.y);
                return;
            }
        };
        let c = p1.add(bis.mul(r / (theta / 2.0).sin()));
        if !c.finite() || !t1.finite() {
            self.line_to(m, p1.x, p1.y);
            return;
        }
        self.line_to(m, t1.x, t1.y);
        let a0 = (t1.y - c.y).atan2(t1.x - c.x);
        let a1 = (t2.y - c.y).atan2(t2.x - c.x);
        // Turning direction of p0 → p1 → p2 in y-down space.
        let turn = p1.sub(p0).cross(p2.sub(p1));
        let mut sweep = a1 - a0;
        if turn > 0.0 {
            if sweep < 0.0 {
                sweep += TAU;
            }
        } else if sweep > 0.0 {
            sweep -= TAU;
        }
        self.ellipse(m, c, r, r, 0.0, a0, a0 + sweep, sweep < 0.0);
    }

    /// `roundRect()` after argument checks; `r` = top-left, top-right, bottom-right, bottom-left.
    pub fn round_rect(&mut self, m: &M, mut x: f64, mut y: f64, mut w: f64, mut h: f64, mut r: [f64; 4]) {
        if w < 0.0 {
            x += w;
            w = -w;
            r = [r[1], r[0], r[3], r[2]];
        }
        if h < 0.0 {
            y += h;
            h = -h;
            r = [r[3], r[2], r[1], r[0]];
        }
        let mut s: f64 = 1.0;
        for (side, a, b) in [(w, r[0], r[1]), (w, r[3], r[2]), (h, r[0], r[3]), (h, r[1], r[2])] {
            let sum = a + b;
            if sum > 0.0 {
                s = s.min(side / sum);
            }
        }
        let r = r.map(|v| v * s);
        let [tl, tr, br, bl] = r;
        self.move_to(m, x + tl, y);
        self.line_to(m, x + w - tr, y);
        self.ellipse(m, P::new(x + w - tr, y + tr), tr, tr, 0.0, -FRAC_PI_2, 0.0, false);
        self.line_to(m, x + w, y + h - br);
        self.ellipse(m, P::new(x + w - br, y + h - br), br, br, 0.0, 0.0, FRAC_PI_2, false);
        self.line_to(m, x + bl, y + h);
        self.ellipse(m, P::new(x + bl, y + h - bl), bl, bl, 0.0, FRAC_PI_2, PI, false);
        self.line_to(m, x, y + tl);
        self.ellipse(m, P::new(x + tl, y + tl), tl, tl, 0.0, PI, PI + FRAC_PI_2, false);
        self.close();
        self.move_to(m, x, y);
    }

    /// Append `o` mapped through `m` (`addPath`).
    pub fn add_path(&mut self, o: &Path, m: &M) {
        for s in &o.segs {
            if self.segs.len() >= self.limit.min(MAX_SEGS) {
                break;
            }
            self.push(map_seg(s, m));
        }
        if o.has_sub {
            self.start = m.apply(o.start);
            self.last = m.apply(o.last);
            self.has_sub = true;
        }
    }

    /// Whole-pixel box around every point (control points bound the curves), or `None`
    /// for an empty or non-finite path.
    pub fn bbox(&self) -> Option<crate::geom::R> {
        self.bbox_mapped(None)
    }

    /// [`Path::bbox`] with every point first mapped through `m`.
    pub fn bbox_mapped(&self, m: Option<&M>) -> Option<crate::geom::R> {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        let mut add = |p: P| {
            let p = m.map_or(p, |m| m.apply(p));
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        };
        for s in &self.segs {
            match *s {
                Seg::Move(p) | Seg::Line(p) => add(p),
                Seg::Quad(a, b) => {
                    add(a);
                    add(b)
                }
                Seg::Cubic(a, b, c) => {
                    add(a);
                    add(b);
                    add(c)
                }
                Seg::Close => {}
            }
        }
        if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) || x0 > x1 {
            return None;
        }
        let l = 1e12;
        Some(crate::geom::R::new(
            x0.clamp(-l, l).floor() as i64,
            y0.clamp(-l, l).floor() as i64,
            x1.clamp(-l, l).ceil() as i64 + 1,
            y1.clamp(-l, l).ceil() as i64 + 1,
        ))
    }

    /// Flatten to polylines with tolerance `tol` (in stored units). Returns nothing if any
    /// point is not finite.
    pub fn flatten(&self, tol: f64) -> Vec<Poly> {
        self.flatten_mapped(None, tol)
    }

    /// Flatten with every point first mapped through `m` (`tol` is in mapped units). At
    /// most [`MAX_FLAT_PTS`] points are produced: past [`MAX_FLAT`] curves become chords,
    /// past the hard cap the rest of the path is dropped.
    pub fn flatten_mapped(&self, m: Option<&M>, tol: f64) -> Vec<Poly> {
        let tol = if tol > 0.0 && tol.is_finite() { tol } else { 0.1 };
        let mp = |p: P| m.map_or(p, |m| m.apply(p));
        let mut out: Vec<Poly> = Vec::new();
        let mut cur = Poly::default();
        let mut total = 0usize;
        let mut last = P::default();
        for s in &self.segs {
            if total >= MAX_FLAT_PTS {
                break;
            }
            let before = cur.pts.len();
            match *s {
                Seg::Move(p) => {
                    if cur.pts.len() > 1 {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.pts.clear();
                    cur.closed = false;
                    cur.pts.push(mp(p));
                }
                Seg::Line(p) => cur.pts.push(mp(p)),
                Seg::Quad(c, p) => {
                    let (c, p) = (mp(c), mp(p));
                    let dd = last.sub(c.mul(2.0)).add(p).len();
                    let n = segments(dd / (4.0 * tol), total);
                    for i in 1..=n {
                        let t = i as f64 / n as f64;
                        let mt = 1.0 - t;
                        cur.pts.push(last.mul(mt * mt).add(c.mul(2.0 * mt * t)).add(p.mul(t * t)));
                    }
                }
                Seg::Cubic(c1, c2, p) => {
                    let (c1, c2, p) = (mp(c1), mp(c2), mp(p));
                    let d1 = last.sub(c1.mul(2.0)).add(c2).len();
                    let d2 = c1.sub(c2.mul(2.0)).add(p).len();
                    let n = segments(0.75 * d1.max(d2) / tol, total);
                    for i in 1..=n {
                        let t = i as f64 / n as f64;
                        let mt = 1.0 - t;
                        let q = last
                            .mul(mt * mt * mt)
                            .add(c1.mul(3.0 * mt * mt * t))
                            .add(c2.mul(3.0 * mt * t * t))
                            .add(p.mul(t * t * t));
                        cur.pts.push(q);
                    }
                }
                Seg::Close => cur.closed = true,
            }
            if let Some(&p) = cur.pts.last() {
                if !p.finite() {
                    return Vec::new();
                }
                last = p;
            }
            total += cur.pts.len().saturating_sub(before).max(1);
        }
        if cur.pts.len() > 1 {
            out.push(cur);
        }
        out
    }
}

fn segments(sq: f64, total: usize) -> usize {
    if total > MAX_FLAT || !sq.is_finite() {
        return 1;
    }
    (sq.sqrt().ceil() as usize).clamp(1, 1000)
}

fn map_seg(s: &Seg, m: &M) -> Seg {
    match *s {
        Seg::Move(p) => Seg::Move(m.apply(p)),
        Seg::Line(p) => Seg::Line(m.apply(p)),
        Seg::Quad(a, b) => Seg::Quad(m.apply(a), m.apply(b)),
        Seg::Cubic(a, b, c) => Seg::Cubic(m.apply(a), m.apply(b), m.apply(c)),
        Seg::Close => Seg::Close,
    }
}
