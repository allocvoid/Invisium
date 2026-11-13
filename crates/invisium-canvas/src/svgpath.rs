//! SVG path data (`new Path2D("M 0 0 L 10 10 Z")`). Parsing stops at the first error and
//! keeps what came before it, as SVG specifies.

use crate::geom::{M, P};
use crate::path::{Path, MAX_SEGS};

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Lexer<'a> {
    fn skip_ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r' | 0x0c) {
            self.i += 1;
        }
    }
    fn skip_sep(&mut self) {
        self.skip_ws();
        if self.i < self.s.len() && self.s[self.i] == b',' {
            self.i += 1;
            self.skip_ws();
        }
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn at_number(&self) -> bool {
        matches!(self.peek(), Some(b'0'..=b'9' | b'.' | b'-' | b'+'))
    }
    fn number(&mut self) -> Option<f64> {
        self.skip_ws();
        let st = self.i;
        let s = self.s;
        let mut i = self.i;
        if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
            i += 1;
        }
        let mut digits = 0;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
        if i < s.len() && s[i] == b'.' {
            i += 1;
            while i < s.len() && s[i].is_ascii_digit() {
                i += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            return None;
        }
        if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
            let mut j = i + 1;
            if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
                j += 1;
            }
            if j < s.len() && s[j].is_ascii_digit() {
                while j < s.len() && s[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
            }
        }
        let txt = std::str::from_utf8(&s[st..i]).ok()?;
        let v: f64 = txt.parse().ok()?;
        self.i = i;
        self.skip_sep();
        if v.is_finite() {
            Some(v)
        } else {
            None
        }
    }
    fn flag(&mut self) -> Option<bool> {
        self.skip_ws();
        let r = match self.peek()? {
            b'0' => false,
            b'1' => true,
            _ => return None,
        };
        self.i += 1;
        self.skip_sep();
        Some(r)
    }
}

/// Parse `d` into a new path of at most `limit` segments.
pub fn parse(d: &str, limit: usize) -> Path {
    let mut path = Path::new();
    path.limit = limit;
    let m = M::ID;
    let mut lx = Lexer { s: d.as_bytes(), i: 0 };
    let mut cur = P::default();
    let mut start = P::default();
    let mut last_ctrl: Option<(u8, P)> = None;
    let mut cmd: u8 = 0;
    let mut first = true;
    loop {
        if path.len() >= limit.min(MAX_SEGS) {
            break;
        }
        lx.skip_ws();
        let Some(c) = lx.peek() else { break };
        if c.is_ascii_alphabetic() {
            cmd = c;
            lx.i += 1;
            lx.skip_ws();
        } else if !(lx.at_number() && cmd != 0 && cmd != b'z' && cmd != b'Z') {
            break;
        }
        if first && cmd != b'M' && cmd != b'm' {
            break;
        }
        first = false;
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { P::default() };
        let mut ctrl: Option<(u8, P)> = None;
        macro_rules! num {
            () => {
                match lx.number() {
                    Some(v) => v,
                    None => break,
                }
            };
        }
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let p = P::new(num!(), num!()).add(base);
                path.move_to(&m, p.x, p.y);
                cur = p;
                start = p;
                // Further pairs are implicit line-tos.
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                let p = P::new(num!(), num!()).add(base);
                path.line_to(&m, p.x, p.y);
                cur = p;
            }
            b'H' => {
                let x = num!() + if rel { cur.x } else { 0.0 };
                cur = P::new(x, cur.y);
                path.line_to(&m, cur.x, cur.y);
            }
            b'V' => {
                let y = num!() + if rel { cur.y } else { 0.0 };
                cur = P::new(cur.x, y);
                path.line_to(&m, cur.x, cur.y);
            }
            b'C' | b'S' => {
                let c1 = if cmd.eq_ignore_ascii_case(&b'C') {
                    P::new(num!(), num!()).add(base)
                } else {
                    match last_ctrl {
                        Some((b'C', q)) => cur.mul(2.0).sub(q),
                        _ => cur,
                    }
                };
                let c2 = P::new(num!(), num!()).add(base);
                let p = P::new(num!(), num!()).add(base);
                path.cubic_to(&m, [c1.x, c1.y, c2.x, c2.y, p.x, p.y]);
                ctrl = Some((b'C', c2));
                cur = p;
            }
            b'Q' | b'T' => {
                let c1 = if cmd.eq_ignore_ascii_case(&b'Q') {
                    P::new(num!(), num!()).add(base)
                } else {
                    match last_ctrl {
                        Some((b'Q', q)) => cur.mul(2.0).sub(q),
                        _ => cur,
                    }
                };
                let p = P::new(num!(), num!()).add(base);
                path.quad_to(&m, c1.x, c1.y, p.x, p.y);
                ctrl = Some((b'Q', c1));
                cur = p;
            }
            b'A' => {
                let rx = num!().abs();
                let ry = num!().abs();
                let rot = num!();
                let Some(large) = lx.flag() else { break };
                let Some(sweep) = lx.flag() else { break };
                let p = P::new(num!(), num!()).add(base);
                svg_arc(&mut path, cur, p, rx, ry, rot, large, sweep);
                cur = p;
            }
            b'Z' => {
                path.close();
                cur = start;
                cmd = 0;
            }
            _ => break,
        }
        last_ctrl = ctrl;
    }
    path
}

/// SVG endpoint arc → centre parameterisation (SVG 1.1 implementation notes F.6).
#[allow(clippy::too_many_arguments)]
fn svg_arc(path: &mut Path, p0: P, p1: P, mut rx: f64, mut ry: f64, rot_deg: f64, large: bool, sweep: bool) {
    let m = M::ID;
    if p0 == p1 {
        return;
    }
    if rx == 0.0 || ry == 0.0 {
        path.line_to(&m, p1.x, p1.y);
        return;
    }
    let phi = rot_deg.to_radians();
    let (s, c) = phi.sin_cos();
    let dx = (p0.x - p1.x) / 2.0;
    let dy = (p0.y - p1.y) / 2.0;
    let x1 = c * dx + s * dy;
    let y1 = -s * dx + c * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let k = lambda.sqrt();
        rx *= k;
        ry *= k;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut co = if den > 0.0 { (num / den).max(0.0).sqrt() } else { 0.0 };
    if large == sweep {
        co = -co;
    }
    let cxp = co * rx * y1 / ry;
    let cyp = -co * ry * x1 / rx;
    let cx = c * cxp - s * cyp + (p0.x + p1.x) / 2.0;
    let cy = s * cxp + c * cyp + (p0.y + p1.y) / 2.0;
    let ang = |ux: f64, uy: f64| uy.atan2(ux);
    let t0 = ang((x1 - cxp) / rx, (y1 - cyp) / ry);
    let t1 = ang((-x1 - cxp) / rx, (-y1 - cyp) / ry);
    let mut dt = t1 - t0;
    if sweep && dt < 0.0 {
        dt += std::f64::consts::TAU;
    } else if !sweep && dt > 0.0 {
        dt -= std::f64::consts::TAU;
    }
    if !(cx.is_finite() && cy.is_finite() && dt.is_finite()) {
        path.line_to(&m, p1.x, p1.y);
        return;
    }
    path.ellipse(&m, P::new(cx, cy), rx, ry, phi, t0, t0 + dt, !sweep);
}
