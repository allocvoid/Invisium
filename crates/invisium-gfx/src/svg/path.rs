//! Path data parsing, affine transforms and curve flattening.

pub type Pt = [f32; 2];

/// Affine matrix `[a, b, c, d, e, f]`: x' = a x + c y + e, y' = b x + d y + f.
pub type Matrix = [f32; 6];
pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

pub fn mul(m: &Matrix, n: &Matrix) -> Matrix {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

pub fn apply(m: &Matrix, p: Pt) -> Pt {
    [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]]
}

#[derive(Clone, Copy, Debug)]
pub enum Seg {
    Move(Pt),
    Line(Pt),
    Quad(Pt, Pt),
    Cubic(Pt, Pt, Pt),
    Close,
}

/// Upper bound on segments taken from one document's path data.
pub const MAX_SEGS: usize = 200_000;

struct Lexer<'a> {
    b: &'a [u8],
    s: &'a str,
    pos: usize,
}

impl Lexer<'_> {
    fn skip_sep(&mut self) {
        while self.pos < self.b.len() && (self.b[self.pos].is_ascii_whitespace() || self.b[self.pos] == b',') {
            self.pos += 1;
        }
    }
    fn number(&mut self) -> Option<f32> {
        self.skip_sep();
        let start = self.pos;
        let b = self.b;
        let mut i = self.pos;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let mut digits = false;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits = true;
        }
        if i < b.len() && b[i] == b'.' {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
                digits = true;
            }
        }
        if !digits {
            return None;
        }
        if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
            let mut j = i + 1;
            if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                j += 1;
            }
            if j < b.len() && b[j].is_ascii_digit() {
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
            }
        }
        let v: f32 = self.s[start..i].parse().ok()?;
        self.pos = i;
        v.is_finite().then_some(v)
    }
    fn flag(&mut self) -> Option<bool> {
        self.skip_sep();
        let c = *self.b.get(self.pos)?;
        self.pos += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

/// Parse SVG path data. On an error, everything parsed so far is kept (as the spec asks).
pub fn parse(d: &str, out: &mut Vec<Seg>) {
    let mut lx = Lexer { b: d.as_bytes(), s: d, pos: 0 };
    let mut cur: Pt = [0.0, 0.0];
    let mut start: Pt = [0.0, 0.0];
    let mut last_ctrl: Option<(u8, Pt)> = None; // (kind 'C' or 'Q', control point)
    let mut cmd = 0u8;
    loop {
        if out.len() >= MAX_SEGS {
            return;
        }
        lx.skip_sep();
        if lx.pos >= lx.b.len() {
            return;
        }
        let c = lx.b[lx.pos];
        if c.is_ascii_alphabetic() {
            cmd = c;
            lx.pos += 1;
        } else if cmd == 0 {
            return;
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { [0.0, 0.0] };
        let pt = |lx: &mut Lexer| -> Option<Pt> { Some([lx.number()? + base[0], lx.number()? + base[1]]) };
        let mut ctrl = None;
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let Some(p) = pt(&mut lx) else { return };
                out.push(Seg::Move(p));
                cur = p;
                start = p;
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                let Some(p) = pt(&mut lx) else { return };
                out.push(Seg::Line(p));
                cur = p;
            }
            b'H' => {
                let Some(x) = lx.number() else { return };
                cur = [x + base[0], cur[1]];
                out.push(Seg::Line(cur));
            }
            b'V' => {
                let Some(y) = lx.number() else { return };
                cur = [cur[0], y + base[1]];
                out.push(Seg::Line(cur));
            }
            b'C' | b'S' => {
                let c1 = if cmd.to_ascii_uppercase() == b'C' {
                    let Some(p) = pt(&mut lx) else { return };
                    p
                } else {
                    match last_ctrl {
                        Some((b'C', q)) => [2.0 * cur[0] - q[0], 2.0 * cur[1] - q[1]],
                        _ => cur,
                    }
                };
                let (Some(c2), Some(p)) = (pt(&mut lx), pt(&mut lx)) else { return };
                out.push(Seg::Cubic(c1, c2, p));
                ctrl = Some((b'C', c2));
                cur = p;
            }
            b'Q' | b'T' => {
                let c1 = if cmd.to_ascii_uppercase() == b'Q' {
                    let Some(p) = pt(&mut lx) else { return };
                    p
                } else {
                    match last_ctrl {
                        Some((b'Q', q)) => [2.0 * cur[0] - q[0], 2.0 * cur[1] - q[1]],
                        _ => cur,
                    }
                };
                let Some(p) = pt(&mut lx) else { return };
                out.push(Seg::Quad(c1, p));
                ctrl = Some((b'Q', c1));
                cur = p;
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(rot)) = (lx.number(), lx.number(), lx.number()) else { return };
                let (Some(large), Some(sweep)) = (lx.flag(), lx.flag()) else { return };
                let Some(p) = pt(&mut lx) else { return };
                arc(out, cur, rx, ry, rot, large, sweep, p);
                cur = p;
            }
            b'Z' => {
                out.push(Seg::Close);
                cur = start;
                // Z takes no arguments; a following number would need a new command.
                cmd = 0;
            }
            _ => return,
        }
        last_ctrl = ctrl;
    }
}

/// Endpoint arc to cubic Béziers (SVG implementation notes F.6.5 / F.6.6).
#[allow(clippy::too_many_arguments)]
pub fn arc(out: &mut Vec<Seg>, p0: Pt, rx: f32, ry: f32, rot_deg: f32, large: bool, sweep: bool, p1: Pt) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if (p0[0] == p1[0] && p0[1] == p1[1]) || rx == 0.0 || ry == 0.0 {
        out.push(Seg::Line(p1));
        return;
    }
    let phi = rot_deg.to_radians();
    let (sin, cos) = phi.sin_cos();
    let dx = (p0[0] - p1[0]) * 0.5;
    let dy = (p0[1] - p1[1]) * 0.5;
    let x1 = cos * dx + sin * dy;
    let y1 = -sin * dx + cos * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut k = if den > 0.0 { (num / den).max(0.0).sqrt() } else { 0.0 };
    if large == sweep {
        k = -k;
    }
    let cx1 = k * rx * y1 / ry;
    let cy1 = -k * ry * x1 / rx;
    let cx = cos * cx1 - sin * cy1 + (p0[0] + p1[0]) * 0.5;
    let cy = sin * cx1 + cos * cy1 + (p0[1] + p1[1]) * 0.5;
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        if a.is_finite() { a } else { 0.0 }
    };
    let ux = (x1 - cx1) / rx;
    let uy = (y1 - cy1) / ry;
    let vx = (-x1 - cx1) / rx;
    let vy = (-y1 - cy1) / ry;
    let theta1 = angle(1.0, 0.0, ux, uy);
    let mut dtheta = angle(ux, uy, vx, vy);
    let tau = std::f32::consts::TAU;
    if !sweep && dtheta > 0.0 {
        dtheta -= tau;
    } else if sweep && dtheta < 0.0 {
        dtheta += tau;
    }
    let n = (dtheta.abs() / std::f32::consts::FRAC_PI_2).ceil().clamp(1.0, 4.0) as usize;
    let step = dtheta / n as f32;
    let t = 4.0 / 3.0 * (step / 4.0).tan();
    let ell = |a: f32| -> (Pt, Pt) {
        let (s, c) = a.sin_cos();
        let p = [cx + rx * c * cos - ry * s * sin, cy + rx * c * sin + ry * s * cos];
        let d = [-rx * s * cos - ry * c * sin, -rx * s * sin + ry * c * cos];
        (p, d)
    };
    let mut a = theta1;
    for i in 0..n {
        let b = a + step;
        let (pa, da) = ell(a);
        let (pb, db) = ell(b);
        let end = if i + 1 == n { p1 } else { pb };
        out.push(Seg::Cubic([pa[0] + t * da[0], pa[1] + t * da[1]], [pb[0] - t * db[0], pb[1] - t * db[1]], end));
        a = b;
    }
}

/// A flattened subpath in device space.
pub struct Poly {
    pub pts: Vec<Pt>,
    pub closed: bool,
}

/// Transform `segs` by `m` and flatten curves to within roughly `tol` device pixels.
/// Stops adding points once `max_pts` is reached.
pub fn flatten(segs: &[Seg], m: &Matrix, tol: f32, max_pts: usize, out: &mut Vec<Poly>) {
    let mut total = 0usize;
    let mut cur: Option<Poly> = None;
    let mut last: Pt = [0.0, 0.0];
    let mut start: Pt = [0.0, 0.0];
    let finish = |p: Option<Poly>, out: &mut Vec<Poly>| {
        if let Some(p) = p {
            if !p.pts.is_empty() {
                out.push(p);
            }
        }
    };
    for s in segs {
        if total >= max_pts {
            break;
        }
        if !matches!(s, Seg::Move(_) | Seg::Close) && cur.is_none() {
            cur = Some(Poly { pts: vec![last], closed: false });
        }
        match *s {
            Seg::Move(p) => {
                finish(cur.take(), out);
                last = apply(m, p);
                start = last;
                cur = Some(Poly { pts: vec![last], closed: false });
            }
            Seg::Line(p) => {
                last = apply(m, p);
                if let Some(c) = cur.as_mut() {
                    c.pts.push(last);
                }
            }
            Seg::Quad(c1, p) => {
                let (a, b, c) = (last, apply(m, c1), apply(m, p));
                let dd = ((a[0] - 2.0 * b[0] + c[0]).powi(2) + (a[1] - 2.0 * b[1] + c[1]).powi(2)).sqrt();
                let n = steps(dd, tol);
                let poly = cur.as_mut().map(|c| &mut c.pts);
                if let Some(pts) = poly {
                    for i in 1..=n {
                        let t = i as f32 / n as f32;
                        let u = 1.0 - t;
                        pts.push([u * u * a[0] + 2.0 * u * t * b[0] + t * t * c[0], u * u * a[1] + 2.0 * u * t * b[1] + t * t * c[1]]);
                    }
                }
                total += n;
                last = c;
            }
            Seg::Cubic(c1, c2, p) => {
                let (a, b, c, d) = (last, apply(m, c1), apply(m, c2), apply(m, p));
                let d1 = ((a[0] - 2.0 * b[0] + c[0]).powi(2) + (a[1] - 2.0 * b[1] + c[1]).powi(2)).sqrt();
                let d2 = ((b[0] - 2.0 * c[0] + d[0]).powi(2) + (b[1] - 2.0 * c[1] + d[1]).powi(2)).sqrt();
                let n = steps(1.5 * d1.max(d2), tol);
                if let Some(cp) = cur.as_mut() {
                    for i in 1..=n {
                        let t = i as f32 / n as f32;
                        let u = 1.0 - t;
                        let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                        cp.pts.push([
                            w0 * a[0] + w1 * b[0] + w2 * c[0] + w3 * d[0],
                            w0 * a[1] + w1 * b[1] + w2 * c[1] + w3 * d[1],
                        ]);
                    }
                }
                total += n;
                last = d;
            }
            Seg::Close => {
                if let Some(mut c) = cur.take() {
                    c.closed = true;
                    finish(Some(c), out);
                }
                last = start;
            }
        }
        total += 1;
    }
    finish(cur, out);
}

fn steps(dd: f32, tol: f32) -> usize {
    if !dd.is_finite() {
        return 1;
    }
    ((dd / (8.0 * tol)).sqrt().ceil() as usize * 2).clamp(1, 256)
}
