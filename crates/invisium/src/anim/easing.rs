//! Timing functions: how far along an animation is, given how much of its time has passed.

use crate::css::tokenizer::{split_commas, tokenize_value, Cur, Tok};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StepPos {
    Start,
    End,
    None,
    Both,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Easing {
    Linear,
    /// `cubic-bezier(x1, y1, x2, y2)`.
    Bezier(f32, f32, f32, f32),
    Steps(u32, StepPos),
    /// `linear(…)`: (input, output) points with ascending inputs.
    Points(Rc<[(f32, f32)]>),
}

impl Easing {
    pub const EASE: Easing = Easing::Bezier(0.25, 0.1, 0.25, 1.0);

    /// Output progress for input progress `t` (0..=1).
    pub fn at(&self, t: f32) -> f32 {
        match self {
            Easing::Linear => t,
            Easing::Bezier(x1, y1, x2, y2) => bezier(*x1, *y1, *x2, *y2, t),
            Easing::Steps(n, pos) => {
                let n = (*n).max(1) as f32;
                let t = t.clamp(0.0, 1.0);
                let k = (t * n).floor().min(n);
                let v = match pos {
                    StepPos::End => k / n,
                    StepPos::Start => (k + 1.0) / n,
                    StepPos::None => {
                        if n <= 1.0 {
                            t
                        } else {
                            k.min(n - 1.0) / (n - 1.0)
                        }
                    }
                    StepPos::Both => (k + 1.0) / (n + 1.0),
                };
                if t >= 1.0 {
                    1.0
                } else {
                    v.min(1.0)
                }
            }
            Easing::Points(pts) => {
                if pts.is_empty() {
                    return t;
                }
                if pts.len() == 1 {
                    return pts[0].1;
                }
                // the segment that contains t (the outer segments also cover values beyond them)
                let mut i = 0;
                while i + 2 < pts.len() && t >= pts[i + 1].0 {
                    i += 1;
                }
                let (x0, y0) = pts[i];
                let (x1, y1) = pts[i + 1];
                if x1 <= x0 {
                    return if t < x0 { y0 } else { y1 };
                }
                y0 + (y1 - y0) * (t - x0) / (x1 - x0)
            }
        }
    }

    /// One timing function at the cursor.
    pub fn parse(c: &mut Cur) -> Option<Easing> {
        c.skip_ws();
        match c.next()? {
            Tok::Ident(i) => Some(match i.to_ascii_lowercase().as_str() {
                "linear" => Easing::Linear,
                "ease" => Easing::EASE,
                "ease-in" => Easing::Bezier(0.42, 0.0, 1.0, 1.0),
                "ease-out" => Easing::Bezier(0.0, 0.0, 0.58, 1.0),
                "ease-in-out" => Easing::Bezier(0.42, 0.0, 0.58, 1.0),
                "step-start" => Easing::Steps(1, StepPos::Start),
                "step-end" => Easing::Steps(1, StepPos::End),
                _ => return None,
            }),
            Tok::Function(f) => {
                let mut a = c.block();
                match f.to_ascii_lowercase().as_str() {
                    "cubic-bezier" => {
                        let mut v = [0.0f32; 4];
                        for (i, slot) in v.iter_mut().enumerate() {
                            if i > 0 && !a.comma() {
                                return None;
                            }
                            a.skip_ws();
                            let Some(Tok::Number(n)) = a.next() else { return None };
                            *slot = *n;
                        }
                        if !a.at_end() || !(0.0..=1.0).contains(&v[0]) || !(0.0..=1.0).contains(&v[2]) {
                            return None;
                        }
                        Some(Easing::Bezier(v[0], v[1], v[2], v[3]))
                    }
                    "steps" => {
                        a.skip_ws();
                        let Some(Tok::Number(n)) = a.next() else { return None };
                        let pos = if a.comma() {
                            match a.ident()?.to_ascii_lowercase().as_str() {
                                "start" | "jump-start" => StepPos::Start,
                                "end" | "jump-end" => StepPos::End,
                                "jump-none" => StepPos::None,
                                "jump-both" => StepPos::Both,
                                _ => return None,
                            }
                        } else {
                            StepPos::End
                        };
                        if *n < 1.0 || n.fract() != 0.0 || !a.at_end() {
                            return None;
                        }
                        Some(Easing::Steps(*n as u32, pos))
                    }
                    "linear" => linear_points(a.rest()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// A timing function given as text (`"ease-out"`, `"cubic-bezier(…)"`, `"linear(…)"`).
    pub fn parse_str(s: &str) -> Option<Easing> {
        let toks = tokenize_value(s);
        let mut c = Cur::new(&toks);
        let e = Easing::parse(&mut c)?;
        c.at_end().then_some(e)
    }
}

/// The points of `linear(a, b 25%, c 50% 75%, d)`.
fn linear_points(toks: &[Tok]) -> Option<Easing> {
    // (output, input if given)
    let mut raw: Vec<(f32, Option<f32>)> = Vec::new();
    for part in split_commas(toks) {
        let mut c = Cur::new(part);
        let mut out: Option<f32> = None;
        let mut ins: Vec<f32> = Vec::new();
        loop {
            c.skip_ws();
            match c.next() {
                None => break,
                Some(Tok::Number(n)) if out.is_none() => out = Some(*n),
                Some(Tok::Percentage(p)) if ins.len() < 2 => ins.push(*p / 100.0),
                _ => return None,
            }
        }
        let out = out?;
        if ins.is_empty() {
            raw.push((out, None));
        } else {
            for i in ins {
                raw.push((out, Some(i)));
            }
        }
        if raw.len() > 4096 {
            return None;
        }
    }
    if raw.len() < 2 {
        return None;
    }
    let n = raw.len();
    if raw[0].1.is_none() {
        raw[0].1 = Some(0.0);
    }
    if raw[n - 1].1.is_none() {
        raw[n - 1].1 = Some(1.0);
    }
    // inputs never decrease
    let mut hi = f32::NEG_INFINITY;
    for r in raw.iter_mut() {
        if let Some(x) = r.1.as_mut() {
            if *x < hi {
                *x = hi;
            }
            hi = *x;
        }
    }
    // spread the points without an input evenly between their neighbours
    let mut i = 1;
    while i < n {
        if raw[i].1.is_none() {
            let start = i - 1;
            let mut end = i;
            while raw[end].1.is_none() {
                end += 1;
            }
            let (a, b) = (raw[start].1.unwrap_or(0.0), raw[end].1.unwrap_or(1.0));
            for k in i..end {
                raw[k].1 = Some(a + (b - a) * (k - start) as f32 / (end - start) as f32);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    if raw.iter().any(|(o, i)| !o.is_finite() || !i.map_or(false, f32::is_finite)) {
        return None;
    }
    Some(Easing::Points(raw.iter().map(|(o, i)| (i.unwrap_or(0.0), *o)).collect()))
}

/// y of the cubic Bézier through (0,0), (x1,y1), (x2,y2), (1,1) at the point whose x is `x`.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    if x <= 0.0 {
        // continue along the starting tangent
        let slope = if x1 > 0.0 {
            y1 / x1
        } else if y1 == 0.0 && x2 > 0.0 {
            y2 / x2
        } else {
            0.0
        };
        return slope * x;
    }
    if x >= 1.0 {
        let slope = if x2 < 1.0 {
            (y2 - 1.0) / (x2 - 1.0)
        } else if y2 == 1.0 && x1 < 1.0 {
            (y1 - 1.0) / (x1 - 1.0)
        } else {
            0.0
        };
        return 1.0 + slope * (x - 1.0);
    }
    let coord = |p1: f32, p2: f32, t: f32| {
        let u = 1.0 - t;
        3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t
    };
    let slope = |p1: f32, p2: f32, t: f32| {
        let u = 1.0 - t;
        3.0 * u * u * p1 + 6.0 * u * t * (p2 - p1) + 3.0 * t * t * (1.0 - p2)
    };
    // Newton's method, then bisection if it wanders off
    let mut t = x;
    for _ in 0..8 {
        let err = coord(x1, x2, t) - x;
        if err.abs() < 1e-5 {
            return coord(y1, y2, t);
        }
        let d = slope(x1, x2, t);
        if d.abs() < 1e-6 {
            break;
        }
        t -= err / d;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    t = x;
    for _ in 0..24 {
        let v = coord(x1, x2, t);
        if (v - x).abs() < 1e-5 {
            break;
        }
        if v < x {
            lo = t;
        } else {
            hi = t;
        }
        t = (lo + hi) / 2.0;
    }
    coord(y1, y2, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves() {
        let ease = Easing::parse_str("ease").unwrap();
        assert!((ease.at(0.0)).abs() < 1e-4 && (ease.at(1.0) - 1.0).abs() < 1e-4);
        // the standard ease curve is ahead of linear in the middle
        assert!((ease.at(0.5) - 0.8024).abs() < 0.01, "{}", ease.at(0.5));
        let io = Easing::parse_str("cubic-bezier(0.42, 0, 0.58, 1)").unwrap();
        assert!((io.at(0.5) - 0.5).abs() < 0.001);
        assert!(io.at(0.25) < 0.25 && io.at(0.75) > 0.75);
        // overshoot is allowed on y
        let back = Easing::parse_str("cubic-bezier(0.34, 1.56, 0.64, 1)").unwrap();
        assert!(back.at(0.7) > 1.0);
        assert!(Easing::parse_str("cubic-bezier(2, 0, 0, 1)").is_none());
        assert_eq!(Easing::parse_str("linear"), Some(Easing::Linear));
    }

    #[test]
    fn steps_and_points() {
        let s = Easing::parse_str("steps(4, end)").unwrap();
        assert_eq!((s.at(0.0), s.at(0.24), s.at(0.25), s.at(0.99), s.at(1.0)), (0.0, 0.0, 0.25, 0.75, 1.0));
        let s = Easing::parse_str("steps(2, jump-start)").unwrap();
        assert_eq!((s.at(0.1), s.at(0.6)), (0.5, 1.0));
        assert_eq!(Easing::parse_str("step-end").unwrap().at(0.9), 0.0);
        // what an animation library writes for a spring
        let l = Easing::parse_str("linear(0, 0.5 25%, 1.2 50%, 1)").unwrap();
        assert!((l.at(0.125) - 0.25).abs() < 1e-4);
        assert!((l.at(0.5) - 1.2).abs() < 1e-4);
        assert!((l.at(0.75) - 1.1).abs() < 1e-4);
        assert!((l.at(1.0) - 1.0).abs() < 1e-4);
        // inputs spread evenly when not given
        let l = Easing::parse_str("linear(0, 0.2, 1)").unwrap();
        assert!((l.at(0.5) - 0.2).abs() < 1e-4 && (l.at(0.75) - 0.6).abs() < 1e-4);
        assert!(Easing::parse_str("linear(0)").is_none());
        assert!(Easing::parse_str("steps(0)").is_none());
    }
}
