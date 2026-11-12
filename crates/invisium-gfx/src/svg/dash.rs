//! `stroke-dasharray` / `stroke-dashoffset`: parsing, and cutting flattened outlines into
//! dashes by arc length.

use super::length;
use super::path::{Poly, Pt};

/// Dash-array entries kept (an odd list is repeated once, within this limit).
const MAX_ENTRIES: usize = 32;
/// Dashes per element; beyond this the stroke is drawn solid.
pub const MAX_DASHES: usize = 20_000;

#[derive(Clone, Copy)]
pub struct Dash {
    vals: [f32; MAX_ENTRIES],
    /// 0 = solid.
    len: u8,
    /// Bit `i` set: entry `i` is a fraction of the viewport's normalised diagonal.
    pct: u32,
}

impl Dash {
    pub const SOLID: Dash = Dash { vals: [0.0; MAX_ENTRIES], len: 0, pct: 0 };

    pub fn is_solid(&self) -> bool {
        self.len == 0
    }

    /// The pattern in user units: percentages against `pct_ref`, everything times `k`.
    /// `None` if it does not dash (solid, zero total, non-finite).
    pub fn resolve(&self, pct_ref: f32, k: f32) -> Option<([f32; MAX_ENTRIES], usize)> {
        let n = self.len as usize;
        if n == 0 {
            return None;
        }
        let mut out = [0.0; MAX_ENTRIES];
        for (i, o) in out.iter_mut().enumerate().take(n) {
            let v = if self.pct & (1 << i) != 0 { self.vals[i] * pct_ref } else { self.vals[i] };
            *o = v * k;
        }
        let total: f32 = out[..n].iter().sum();
        (total > 0.0 && total.is_finite() && out[..n].iter().all(|v| v.is_finite() && *v >= 0.0)).then_some((out, n))
    }
}

/// A length or a percentage (`true`, as a fraction).
pub fn length_or_pct(t: &str) -> Option<(f32, bool)> {
    let t = t.trim();
    if let Some(p) = t.strip_suffix('%') {
        let v: f32 = p.trim().parse().ok()?;
        return v.is_finite().then_some((v / 100.0, true));
    }
    length(t).map(|v| (v, false))
}

/// Parse `stroke-dasharray`. `None` = invalid declaration (ignored).
pub fn parse_array(v: &str) -> Option<Dash> {
    if v.trim().eq_ignore_ascii_case("none") {
        return Some(Dash::SOLID);
    }
    let mut d = Dash::SOLID;
    let mut n = 0usize;
    let mut negative = false;
    for t in v.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()) {
        let (x, pct) = length_or_pct(t)?;
        negative |= x < 0.0;
        if n < MAX_ENTRIES {
            d.vals[n] = x;
            if pct {
                d.pct |= 1 << n;
            }
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    if n % 2 == 1 {
        if 2 * n <= MAX_ENTRIES {
            for i in 0..n {
                d.vals[n + i] = d.vals[i];
            }
            d.pct |= d.pct << n;
            n *= 2;
        } else {
            n -= 1;
        }
    }
    if negative || d.vals[..n].iter().all(|v| *v == 0.0) {
        return Some(Dash::SOLID);
    }
    d.len = n as u8;
    Some(d)
}

/// Length of the outline, closing segments included.
pub fn outline_length(polys: &[Poly]) -> f32 {
    let mut total = 0.0;
    for p in polys {
        let closing = if p.closed { p.pts.first() } else { None };
        let mut prev: Option<Pt> = None;
        for &q in p.pts.iter().chain(closing) {
            if let Some(a) = prev {
                total += ((q[0] - a[0]).powi(2) + (q[1] - a[1]).powi(2)).sqrt();
            }
            prev = Some(q);
        }
    }
    total
}

/// Cut `polys` into open dashes following `pattern` (alternating on/off lengths, total > 0),
/// starting `offset` into it; every sub-path restarts the pattern. `None` when the dash cap is
/// hit (the caller then strokes solid).
pub fn dash(polys: &[Poly], pattern: &[f32], offset: f32, out: &mut Vec<Poly>) -> Option<()> {
    let n = pattern.len();
    let total: f32 = pattern.iter().sum();
    if n == 0 || !(total > 0.0) {
        return None;
    }
    let mut steps = 0usize;
    let step_cap = 4 * MAX_DASHES;
    for poly in polys {
        let Some(&first) = poly.pts.first() else { continue };
        let mut phase = if offset.is_finite() { offset.rem_euclid(total) } else { 0.0 };
        let mut i = 0;
        let mut guard = 0;
        while phase > pattern[i] && guard < 2 * n {
            phase -= pattern[i];
            i = (i + 1) % n;
            guard += 1;
        }
        let mut rem = (pattern[i] - phase).max(0.0);
        let mut on = i % 2 == 0;
        let mut cur: Vec<Pt> = Vec::new();
        if on {
            cur.push(first);
        }
        let closing = if poly.closed { Some(first) } else { None };
        let mut prev = first;
        for &p in poly.pts[1..].iter().chain(closing.iter()) {
            let (dx, dy) = (p[0] - prev[0], p[1] - prev[1]);
            let seg = (dx * dx + dy * dy).sqrt();
            if !seg.is_finite() {
                return None;
            }
            let mut t = 0.0;
            while seg - t > rem {
                t += rem;
                let f = if seg > 0.0 { t / seg } else { 0.0 };
                let q = [prev[0] + dx * f, prev[1] + dy * f];
                if on {
                    cur.push(q);
                    if out.len() >= MAX_DASHES {
                        return None;
                    }
                    out.push(Poly { pts: std::mem::take(&mut cur), closed: false });
                } else {
                    cur.clear();
                    cur.push(q);
                }
                on = !on;
                i = (i + 1) % n;
                rem = pattern[i];
                steps += 1;
                if steps > step_cap {
                    return None;
                }
            }
            rem -= seg - t;
            if on {
                cur.push(p);
            }
            prev = p;
        }
        if on && !cur.is_empty() {
            if out.len() >= MAX_DASHES {
                return None;
            }
            out.push(Poly { pts: cur, closed: false });
        }
    }
    Some(())
}
