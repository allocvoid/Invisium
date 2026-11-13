//! Points, affine matrices and integer rectangles.

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct P {
    pub x: f64,
    pub y: f64,
}

impl P {
    #[inline]
    pub fn new(x: f64, y: f64) -> P {
        P { x, y }
    }
    #[inline]
    pub fn sub(self, o: P) -> P {
        P::new(self.x - o.x, self.y - o.y)
    }
    #[inline]
    pub fn add(self, o: P) -> P {
        P::new(self.x + o.x, self.y + o.y)
    }
    #[inline]
    pub fn mul(self, k: f64) -> P {
        P::new(self.x * k, self.y * k)
    }
    #[inline]
    pub fn dot(self, o: P) -> f64 {
        self.x * o.x + self.y * o.y
    }
    #[inline]
    pub fn cross(self, o: P) -> f64 {
        self.x * o.y - self.y * o.x
    }
    #[inline]
    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }
    #[inline]
    pub fn finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
    /// Unit vector, or `None` for a zero (or non-finite) vector.
    pub fn unit(self) -> Option<P> {
        let l = self.len();
        if l > 0.0 && l.is_finite() {
            Some(self.mul(1.0 / l))
        } else {
            None
        }
    }
    /// Rotated 90 degrees (left normal in y-down space).
    #[inline]
    pub fn perp(self) -> P {
        P::new(-self.y, self.x)
    }
}

/// Affine map `[a, b, c, d, e, f]`: `x' = a x + c y + e`, `y' = b x + d y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct M(pub [f64; 6]);

impl M {
    pub const ID: M = M([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    #[inline]
    pub fn apply(&self, p: P) -> P {
        let [a, b, c, d, e, f] = self.0;
        P::new(a * p.x + c * p.y + e, b * p.x + d * p.y + f)
    }
    /// `self × m`: apply `m` first, then `self`.
    pub fn concat(&self, m: &M) -> M {
        let [a, b, c, d, e, f] = self.0;
        let [ma, mb, mc, md, me, mf] = m.0;
        M([
            a * ma + c * mb,
            b * ma + d * mb,
            a * mc + c * md,
            b * mc + d * md,
            a * me + c * mf + e,
            b * me + d * mf + f,
        ])
    }
    pub fn translate(x: f64, y: f64) -> M {
        M([1.0, 0.0, 0.0, 1.0, x, y])
    }
    pub fn scale(x: f64, y: f64) -> M {
        M([x, 0.0, 0.0, y, 0.0, 0.0])
    }
    pub fn rotate(a: f64) -> M {
        let (s, c) = a.sin_cos();
        M([c, s, -s, c, 0.0, 0.0])
    }
    pub fn det(&self) -> f64 {
        self.0[0] * self.0[3] - self.0[1] * self.0[2]
    }
    pub fn finite(&self) -> bool {
        self.0.iter().all(|v| v.is_finite())
    }
    pub fn invert(&self) -> Option<M> {
        let [a, b, c, d, e, f] = self.0;
        let det = self.det();
        if det == 0.0 || !det.is_finite() || !self.finite() {
            return None;
        }
        let r = M([
            d / det,
            -b / det,
            -c / det,
            a / det,
            (c * f - d * e) / det,
            (b * e - a * f) / det,
        ]);
        if r.finite() {
            Some(r)
        } else {
            None
        }
    }
    /// Largest stretch the linear part applies to a vector (an upper bound).
    pub fn max_scale(&self) -> f64 {
        let [a, b, c, d, _, _] = self.0;
        let s = (a * a + b * b).sqrt().max((c * c + d * d).sqrt());
        if s.is_finite() {
            s
        } else {
            f64::MAX
        }
    }
}

/// Integer pixel rectangle `[x0, x1) × [y0, y1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct R {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
}

impl R {
    pub fn new(x0: i64, y0: i64, x1: i64, y1: i64) -> R {
        R { x0, y0, x1, y1 }
    }
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    pub fn intersect(&self, o: &R) -> R {
        R::new(self.x0.max(o.x0), self.y0.max(o.y0), self.x1.min(o.x1), self.y1.min(o.y1))
    }
    pub fn expand(&self, d: i64) -> R {
        R::new(self.x0 - d, self.y0 - d, self.x1 + d, self.y1 + d)
    }
    pub fn shift(&self, dx: i64, dy: i64) -> R {
        R::new(self.x0 + dx, self.y0 + dy, self.x1 + dx, self.y1 + dy)
    }
    pub fn width(&self) -> usize {
        (self.x1 - self.x0).max(0) as usize
    }
    pub fn height(&self) -> usize {
        (self.y1 - self.y0).max(0) as usize
    }
}
