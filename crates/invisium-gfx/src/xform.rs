//! `PushTransform` / `PopTransform` state: the accumulated list-space transform of a layer and
//! the per-item map from an item's local space to physical pixels.
//!
//! "Local space" is where the renderer already builds every instance: the physical position the
//! item would have without any transform, `L(p) = origin + (p - scroll) * scale`. With an
//! accumulated transform `T` the item really lands at `L(T(p))`, so local → physical is
//! `L ∘ T ∘ L⁻¹`.

use crate::draw::{affine_inverse, affine_mul, affine_point, Affine, Rect, IDENTITY};

/// Nested transforms of one layer. `None` entries are singular (draw nothing until popped).
#[derive(Default)]
pub(crate) struct XStack {
    items: Vec<Option<Affine>>,
}

fn finite(m: &Affine) -> bool {
    m.iter().all(|v| v.is_finite())
}

impl XStack {
    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn push(&mut self, m: &Affine) {
        let det = m[0] * m[3] - m[1] * m[2];
        let next = match self.current() {
            Some(parent) if finite(m) && det.is_finite() && det.abs() >= 1e-9 => {
                let acc = affine_mul(&parent, m);
                (finite(&acc) && affine_inverse(&acc).is_some()).then_some(acc)
            }
            _ => None,
        };
        self.items.push(next);
    }

    /// Unmatched pops are ignored.
    pub fn pop(&mut self) {
        self.items.pop();
    }

    /// The accumulated transform, or `None` while a singular one is active.
    pub fn current(&self) -> Option<Affine> {
        match self.items.last() {
            None => Some(IDENTITY),
            Some(m) => *m,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Local → physical map of an item under a transform with a non-identity linear part.
#[derive(Clone, Copy)]
pub(crate) struct Map {
    pub fwd: Affine,
    pub inv: Affine,
}

impl Map {
    /// `t` is the accumulated list transform, the layer maps `p -> (ox, oy) + (p - scroll) * s`.
    pub fn new(t: &Affine, ox: f32, oy: f32, scroll: (f32, f32), s: f32) -> Option<Map> {
        let (tx, ty) = (ox - scroll.0 * s, oy - scroll.1 * s);
        let l = [s, 0.0, 0.0, s, tx, ty];
        let l_inv = [1.0 / s, 0.0, 0.0, 1.0 / s, -tx / s, -ty / s];
        let fwd = affine_mul(&l, &affine_mul(t, &l_inv));
        let inv = affine_inverse(&fwd)?;
        (finite(&fwd) && finite(&inv)).then_some(Map { fwd, inv })
    }

    /// No rotation or skew: rectangles stay axis-aligned.
    pub fn axis_aligned(&self) -> bool {
        self.fwd[1] == 0.0 && self.fwd[2] == 0.0
    }

    /// Physical pixels per local unit (geometric mean over directions).
    pub fn scale(&self) -> f32 {
        (self.fwd[0] * self.fwd[3] - self.fwd[1] * self.fwd[2]).abs().sqrt()
    }

    /// Physical bounding box of the local rectangle `r`.
    pub fn bounds(&self, r: &Rect) -> Rect {
        aabb(&self.fwd, r)
    }

    /// Local bounding box of the physical rectangle `r`.
    pub fn inv_bounds(&self, r: &Rect) -> Rect {
        aabb(&self.inv, r)
    }
}

fn aabb(m: &Affine, r: &Rect) -> Rect {
    let pts = [(r.x, r.y), (r.right(), r.y), (r.x, r.bottom()), (r.right(), r.bottom())].map(|(x, y)| affine_point(m, x, y));
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}
