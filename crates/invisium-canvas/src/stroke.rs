//! The stroker: dashes, then one polygon per segment, join and cap. The polygons all wind
//! the same way, so filling them with the nonzero rule gives their union.
//!
//! Work is bounded by a view box (user space) that holds everything that can show: geometry
//! that cannot reach it is skipped, round joins and caps are only fine where they cross it,
//! a convex piece that covers the whole box ends the stroke, and a stroke stops after
//! [`MAX_STROKE_PTS`] points.

use crate::geom::P;
use crate::path::Poly;
use std::f64::consts::{FRAC_PI_4, PI, TAU};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Join {
    Miter,
    Round,
    Bevel,
}

pub struct Style<'a> {
    pub width: f64,
    pub cap: Cap,
    pub join: Join,
    pub miter: f64,
    pub dash: &'a [f64],
    pub dash_off: f64,
}

/// Most points one stroke generates (outline polygons, and dash pieces separately).
pub const MAX_STROKE_PTS: usize = 750_000;
/// Most points in one round join or cap.
const MAX_ARC_PTS: f64 = 1024.0;

/// An axis-aligned box in user space.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl View {
    fn seg_near(&self, a: P, b: P, d: f64) -> bool {
        a.x.min(b.x) - d <= self.x1
            && a.x.max(b.x) + d >= self.x0
            && a.y.min(b.y) - d <= self.y1
            && a.y.max(b.y) + d >= self.y0
    }
    fn corners(&self) -> [P; 4] {
        [P::new(self.x0, self.y0), P::new(self.x1, self.y0), P::new(self.x1, self.y1), P::new(self.x0, self.y1)]
    }
    /// Nearest and farthest distance from `c` to the box.
    fn dist(&self, c: P) -> (f64, f64) {
        let dx = (self.x0 - c.x).max(c.x - self.x1).max(0.0);
        let dy = (self.y0 - c.y).max(c.y - self.y1).max(0.0);
        let fx = (c.x - self.x0).abs().max((c.x - self.x1).abs());
        let fy = (c.y - self.y0).abs().max((c.y - self.y1).abs());
        (dx.hypot(dy), fx.hypot(fy))
    }
}

struct Gen<'a> {
    out: Vec<Vec<P>>,
    pts: usize,
    stop: bool,
    v: View,
    tol: f64,
    st: &'a Style<'a>,
    hw: f64,
}

/// Stroke outline polygons for `polys` (user space), the number of outline points and the
/// number of dash-piece points generated; `tol` is the flattening tolerance, `v` the view.
pub fn stroke(polys: Vec<Poly>, st: &Style, tol: f64, v: View) -> (Vec<Vec<P>>, usize, usize) {
    let hw = st.width / 2.0;
    let mut g = Gen { out: Vec::new(), pts: 0, stop: false, v, tol, st, hw };
    if !(hw > 0.0 && hw.is_finite()) {
        return (g.out, 0, 0);
    }
    let mut sub: Vec<(Vec<P>, bool)> = Vec::new();
    // Duplicate points are removed in place: the input is not copied.
    for p in polys {
        let raw = p.pts.len();
        let mut pts = p.pts;
        pts.dedup_by(|q, l| l.sub(*q).len() <= 1e-9);
        if p.closed && pts.len() > 1 && pts[0].sub(pts[pts.len() - 1]).len() <= 1e-9 {
            pts.pop();
        }
        if pts.len() == 1 && raw > 1 {
            g.dot(pts[0]);
        } else if pts.len() >= 2 {
            sub.push((pts, p.closed));
        }
    }
    let mut dash_pts = 0;
    let sum: f64 = st.dash.iter().sum();
    if !st.dash.is_empty() && sum > 0.0 && sum.is_finite() {
        let mut pieces = Vec::new();
        let mut budget = MAX_STROKE_PTS;
        for (pts, closed) in &sub {
            dash(pts, *closed, st.dash, st.dash_off, &mut pieces, &mut budget);
        }
        dash_pts = MAX_STROKE_PTS - budget;
        for pc in pieces {
            if g.stop {
                break;
            }
            if pc.iter().all(|q| q.sub(pc[0]).len() <= 1e-9) {
                g.dot(pc[0]);
            } else {
                let mut d: Vec<P> = Vec::with_capacity(pc.len());
                for q in pc {
                    if d.last().is_none_or(|l: &P| l.sub(q).len() > 1e-9) {
                        d.push(q);
                    }
                }
                g.outline(&d, false);
            }
        }
    } else {
        for (pts, closed) in &sub {
            if g.stop {
                break;
            }
            g.outline(pts, *closed);
        }
    }
    let n = g.pts;
    (g.out, n, dash_pts)
}

fn area(p: &[P]) -> f64 {
    let n = p.len();
    let mut a = 0.0;
    for i in 0..n {
        a += p[i].cross(p[(i + 1) % n]);
    }
    a
}

impl Gen<'_> {
    /// Add one convex polygon.
    fn push(&mut self, mut p: Vec<P>) {
        if self.stop || p.len() < 3 {
            return;
        }
        if self.pts + p.len() > MAX_STROKE_PTS {
            self.stop = true;
            return;
        }
        let a = area(&p);
        if a < 0.0 {
            p.reverse();
        }
        let n = p.len();
        let covers = a != 0.0
            && self.v.corners().iter().all(|&q| (0..n).all(|i| p[(i + 1) % n].sub(p[i]).cross(q.sub(p[i])) >= 0.0));
        if covers {
            // This piece alone covers everything that can show.
            self.out.clear();
            self.pts = n;
            self.out.push(p);
            self.stop = true;
            return;
        }
        self.pts += n;
        self.out.push(p);
    }

    fn dot(&mut self, c: P) {
        let hw = self.hw;
        if !self.v.seg_near(c, c, 1.5 * hw) {
            return;
        }
        match self.st.cap {
            Cap::Butt => {}
            Cap::Round => {
                let p = self.arc(c, hw, 0.0, TAU, false);
                self.push(p);
            }
            Cap::Square => self.push(vec![
                P::new(c.x - hw, c.y - hw),
                P::new(c.x + hw, c.y - hw),
                P::new(c.x + hw, c.y + hw),
                P::new(c.x - hw, c.y + hw),
            ]),
        }
    }

    /// Points on a circle from angle `a0` sweeping `sweep` (centre first with `center`):
    /// fine where the arc crosses the view, coarse where the difference cannot show.
    fn arc(&self, c: P, r: f64, a0: f64, sweep: f64, center: bool) -> Vec<P> {
        let fine = if self.tol < r { 2.0 * (1.0 - self.tol / r).acos() } else { PI / 2.0 };
        let fine = fine.max(sweep.abs() / MAX_ARC_PTS);
        let (dmin, dmax) = self.v.dist(c);
        let mut v = Vec::new();
        if center {
            v.push(c);
        }
        let at = |t: f64| P::new(c.x + r * t.cos(), c.y + r * t.sin());
        let uniform = |step: f64, v: &mut Vec<P>| {
            let n = (sweep.abs() / step).ceil().clamp(1.0, MAX_ARC_PTS) as usize;
            let n = n.max(if sweep.abs() >= TAU - 1e-9 { 3 } else { 1 });
            for i in 0..=n {
                v.push(at(a0 + sweep * i as f64 / n as f64));
            }
        };
        if r <= dmin {
            // The circle misses the view: any inscribed polygon does too.
            uniform(FRAC_PI_4, &mut v);
        } else if r >= dmax {
            // The view lies inside the circle: inscribed chords must stay beyond it.
            let step = if dmax > 0.0 { 2.0 * (dmax / r).clamp(0.0, 1.0).acos() } else { PI / 2.0 };
            uniform(step.max(fine).min(FRAC_PI_4), &mut v);
        } else if dmin == 0.0 {
            uniform(fine, &mut v);
        } else {
            // Angular window of the view as seen from `c` (narrower than a half turn).
            let cen = P::new((self.v.x0 + self.v.x1) / 2.0, (self.v.y0 + self.v.y1) / 2.0);
            let phi = (cen.y - c.y).atan2(cen.x - c.x);
            let (mut lo, mut hi) = (f64::MAX, f64::MIN);
            for q in self.v.corners() {
                let d = ((q.y - c.y).atan2(q.x - c.x) - phi + PI).rem_euclid(TAU) - PI;
                lo = lo.min(d);
                hi = hi.max(d);
            }
            let (w0, ww) = (phi + lo, hi - lo);
            // Does the angle interval [t, t + len] meet the window?
            let meets = |t: f64, len: f64| (t - w0).rem_euclid(TAU) <= ww || (w0 - t).rem_euclid(TAU) <= len;
            let dir = sweep.signum();
            let end = a0 + sweep;
            let mut t = a0;
            v.push(at(t));
            let mut guard = 0;
            while (end - t) * dir > 1e-12 && guard < 2 * MAX_ARC_PTS as usize {
                let rest = (end - t).abs();
                let coarse = FRAC_PI_4.min(rest);
                let lo_t = if dir > 0.0 { t } else { t - coarse };
                let step = if meets(lo_t, coarse) { fine.min(rest) } else { coarse };
                t += dir * step;
                v.push(at(t));
                guard += 1;
            }
            if (end - t) * dir > 1e-12 {
                v.push(at(end));
            }
        }
        v
    }

    fn outline(&mut self, pts: &[P], closed: bool) {
        let n = pts.len();
        if n < 2 {
            return;
        }
        let hw = self.hw;
        let nseg = if closed { n } else { n - 1 };
        let dir = |i: usize| pts[(i + 1) % n].sub(pts[i]).unit().unwrap_or(P::new(1.0, 0.0));
        for i in 0..nseg {
            if self.stop {
                return;
            }
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            if !self.v.seg_near(a, b, hw) {
                continue;
            }
            let nn = dir(i).perp().mul(hw);
            self.push(vec![a.add(nn), b.add(nn), b.sub(nn), a.sub(nn)]);
        }
        let reach = hw * if self.st.join == Join::Miter { self.st.miter.max(1.5) } else { 1.5 };
        let joins = if closed { 0..n } else { 1..n - 1 };
        for i in joins {
            if self.stop {
                return;
            }
            if self.v.seg_near(pts[i], pts[i], reach) {
                self.join(pts[i], dir((i + n - 1) % n), dir(i));
            }
        }
        if !closed {
            self.cap(pts[n - 1], dir(n - 2));
            self.cap(pts[0], dir(0).mul(-1.0));
        }
    }

    fn join(&mut self, p: P, d0: P, d1: P) {
        let cross = d0.cross(d1);
        let dotp = d0.dot(d1);
        if cross.abs() < 1e-12 && dotp > 0.0 {
            return;
        }
        let hw = self.hw;
        let sgn = if cross > 0.0 { -1.0 } else { 1.0 };
        let o0 = p.add(d0.perp().mul(hw * sgn));
        let o1 = p.add(d1.perp().mul(hw * sgn));
        match self.st.join {
            Join::Round => {
                if cross.abs() < 1e-12 {
                    let c = self.arc(p, hw, 0.0, TAU, false);
                    self.push(c);
                    return;
                }
                let a0 = (o0.y - p.y).atan2(o0.x - p.x);
                let a1 = (o1.y - p.y).atan2(o1.x - p.x);
                let mut sw = a1 - a0;
                while sw > PI {
                    sw -= TAU;
                }
                while sw < -PI {
                    sw += TAU;
                }
                let c = self.arc(p, hw, a0, sw, true);
                self.push(c);
            }
            Join::Bevel => self.push(vec![p, o0, o1]),
            Join::Miter => {
                let s = ((1.0 + dotp) / 2.0).max(0.0).sqrt();
                if cross.abs() < 1e-12 || s == 0.0 || 1.0 / s > self.st.miter {
                    self.push(vec![p, o0, o1]);
                    return;
                }
                let t = o1.sub(o0).cross(d1) / cross;
                let tip = o0.add(d0.mul(t));
                if tip.finite() {
                    self.push(vec![p, o0, tip, o1]);
                } else {
                    self.push(vec![p, o0, o1]);
                }
            }
        }
    }

    /// Cap at `b`, where `d` points out of the line.
    fn cap(&mut self, b: P, d: P) {
        let hw = self.hw;
        if self.stop || !self.v.seg_near(b, b, 1.5 * hw) {
            return;
        }
        let nn = d.perp().mul(hw);
        match self.st.cap {
            Cap::Butt => {}
            Cap::Square => {
                let e = d.mul(hw);
                self.push(vec![b.add(nn), b.add(nn).add(e), b.sub(nn).add(e), b.sub(nn)]);
            }
            Cap::Round => {
                let a0 = nn.y.atan2(nn.x);
                // From +normal through the outward direction to -normal.
                let sw = if d.perp().cross(d) > 0.0 { PI } else { -PI };
                let c = self.arc(b, hw, a0, sw, true);
                self.push(c);
            }
        }
    }
}

/// Split a polyline into dashes (open pieces), spending at most `budget` points.
fn dash(pts: &[P], closed: bool, pat: &[f64], off: f64, out: &mut Vec<Vec<P>>, budget: &mut usize) {
    let total: f64 = pat.iter().sum();
    let mut idx = 0usize;
    let mut rem = pat[0];
    let mut ph = if off.is_finite() { off.rem_euclid(total) } else { 0.0 };
    let mut guard = 0;
    while ph >= rem && guard < 1_000_000 {
        ph -= rem;
        idx = (idx + 1) % pat.len();
        rem = pat[idx];
        guard += 1;
    }
    rem -= ph;
    let mut on = idx.is_multiple_of(2);
    let mut cur: Vec<P> = Vec::new();
    if on {
        cur.push(pts[0]);
    }
    let n = pts.len();
    let nseg = if closed { n } else { n - 1 };
    for i in 0..nseg {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let l = b.sub(a).len();
        let mut pos = 0.0;
        while l - pos > rem {
            if *budget < 2 {
                return;
            }
            pos += rem;
            let q = a.add(b.sub(a).mul(pos / l));
            if on {
                cur.push(q);
                *budget = budget.saturating_sub(cur.len());
                out.push(std::mem::take(&mut cur));
            } else {
                cur = vec![q];
            }
            on = !on;
            idx = (idx + 1) % pat.len();
            rem = pat[idx];
        }
        rem -= l - pos;
        if on {
            cur.push(b);
        }
    }
    if on && cur.len() >= 2 && *budget >= cur.len() {
        *budget -= cur.len();
        out.push(cur);
    }
}
