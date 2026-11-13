//! Drawing through the state: paint selection, shadows, fills, strokes, clips and hit tests.

use crate::budget::ClipMask;
use crate::composite::{clear, composite};
use crate::geom::{M, P, R};
use crate::paint::{premul, Shader, Wrap};
use crate::path::{Path, Poly};
use crate::raster::{contains, fill_mask, fill_mask_n, Mask};
use crate::shadow::shadow_mask;
use crate::state::Style;
use crate::stroke::{self, stroke};
use crate::Canvas2d;
use std::cell::Cell;
use std::rc::Rc;

/// Flattening tolerance in device pixels.
pub const TOL: f64 = 0.1;

pub fn polys_pts(p: Vec<Poly>) -> Vec<Vec<P>> {
    p.into_iter().map(|p| p.pts).collect()
}

/// Bytes polygons hold (point and list capacities).
pub fn pts_bytes(p: &Vec<Vec<P>>) -> usize {
    p.capacity() * std::mem::size_of::<Vec<P>>()
        + p.iter().map(|q| q.capacity()).sum::<usize>() * std::mem::size_of::<P>()
}

impl Canvas2d {
    /// The region drawing can touch: the bitmap, narrowed by the clip's bounds.
    pub(crate) fn draw_region(&self) -> R {
        let r = self.bmp.rect();
        match &self.st.clip {
            Some(c) => r.intersect(&c.mask.r),
            None => r,
        }
    }

    /// Run `f` with a shader for `style` under the current transform. Nothing runs when
    /// the transform is not invertible.
    pub(crate) fn with_style(&mut self, style: &Style, f: &mut dyn FnMut(&mut Canvas2d, &Shader)) {
        let Some(inv) = self.st.m.invert() else { return };
        match style {
            Style::Color(c) => f(self, &Shader::Solid(premul(*c))),
            Style::Grad(_, g) => {
                let g = g.borrow();
                f(self, &Shader::Grad { g: &g, inv });
            }
            Style::Pat(_, p) => {
                let p = p.borrow();
                let Some(pinv) = self.st.m.concat(&p.m).invert() else { return };
                let wx = if p.rx { Wrap::Repeat } else { Wrap::Decal };
                let wy = if p.ry { Wrap::Repeat } else { Wrap::Decal };
                let smooth = self.st.smoothing;
                f(self, &Shader::Image { img: &p.img, inv: pinv, smooth, wx, wy });
            }
        }
    }

    /// Draw `shader` through the coverage `mk` produces (for a region), with shadow,
    /// global alpha, compositing and clip. `bb` bounds (in device pixels) what `mk` can cover.
    pub(crate) fn render(&mut self, mk: &dyn Fn(R) -> Mask, bb: R, shader: &Shader) {
        let canvas = self.bmp.rect();
        if canvas.is_empty() {
            return;
        }
        let clip = self.st.clip.clone();
        let ga = self.st.alpha as f32;
        let op = self.st.op;
        let st = &self.st;
        if st.shadow_color[3] != 0 && (st.shadow_blur > 0.0 || st.sox != 0.0 || st.soy != 0.0) {
            let sm = shadow_mask(mk, bb, shader, ga, st.shadow_blur, st.sox, st.soy, canvas);
            let sc = Shader::Solid(premul(st.shadow_color));
            composite(&mut self.bmp, &sm, &sc, op, 1.0, clip.as_deref().map(|c| &c.mask));
        }
        let m = mk(self.draw_region());
        composite(&mut self.bmp, &m, shader, op, ga, clip.as_deref().map(|c| &c.mask));
        self.dirty = true;
    }

    /// Fill device-space polygons with the fill or stroke style. Records the working
    /// memory (polygons plus the rasteriser's) for [`Canvas2d::last_fill_bytes`].
    pub(crate) fn fill_polys(&mut self, polys: Vec<Vec<P>>, evenodd: bool, use_stroke: bool) {
        let held = pts_bytes(&polys);
        self.fill_work.set(held);
        if polys.is_empty() {
            return;
        }
        let style = if use_stroke { self.st.stroke.clone() } else { self.st.fill.clone() };
        let work = Cell::new(0usize);
        let bb = bbox_of(&polys);
        self.with_style(&style, &mut |c, sh| {
            let mk = |r: R| {
                let (m, w) = fill_mask_n(&polys, evenodd, r);
                work.set(work.get().max(w));
                m
            };
            c.render(&mk, bb, sh);
        });
        self.fill_work.set(held + work.get());
    }

    /// Flattened device-space polylines of a `Path2D` (through the current transform, point
    /// by point, without copying the path) or of the context's path.
    pub(crate) fn flat(&self, obj: Option<u32>) -> Option<Vec<Poly>> {
        match obj {
            Some(h) => self.objs.paths.get(&h).map(|p| p.flatten_mapped(Some(&self.st.m), TOL)),
            None => Some(self.path.flatten(TOL)),
        }
    }

    pub(crate) fn fill_flat(&mut self, polys: Vec<Poly>, evenodd: bool) {
        if self.st.m.invert().is_none() {
            return;
        }
        self.fill_polys(polys_pts(polys), evenodd, false);
    }

    /// Device pixels that drawing can affect, including where a shadow is taken from.
    pub(crate) fn reach(&self) -> R {
        let r = self.draw_region();
        let st = &self.st;
        if st.shadow_color[3] == 0 || r.is_empty() {
            return r;
        }
        let lim = 1e9;
        let (dx, dy) = (st.sox.clamp(-lim, lim).round() as i64, st.soy.clamp(-lim, lim).round() as i64);
        let ext = (1.5 * st.shadow_blur.min(600.0)).ceil() as i64 + 2;
        let s = r.shift(-dx, -dy).expand(ext);
        R::new(r.x0.min(s.x0), r.y0.min(s.y0), r.x1.max(s.x1), r.y1.max(s.y1))
    }

    /// Stroke outline of device-space polylines, as device-space polygons; only geometry
    /// that can reach the device rectangle `view` is generated.
    pub(crate) fn stroke_outline(&self, mut polys: Vec<Poly>, view: R) -> Vec<Vec<P>> {
        self.stroke_pts.set(0);
        self.dash_pts.set(0);
        let m = self.st.m;
        let Some(inv) = m.invert() else { return Vec::new() };
        if view.is_empty() {
            return Vec::new();
        }
        let corners = [
            (view.x0 - 1, view.y0 - 1),
            (view.x1 + 1, view.y0 - 1),
            (view.x1 + 1, view.y1 + 1),
            (view.x0 - 1, view.y1 + 1),
        ]
        .map(|(x, y)| inv.apply(P::new(x as f64, y as f64)));
        let uv = stroke::View {
            x0: corners.iter().map(|p| p.x).fold(f64::MAX, f64::min),
            y0: corners.iter().map(|p| p.y).fold(f64::MAX, f64::min),
            x1: corners.iter().map(|p| p.x).fold(f64::MIN, f64::max),
            y1: corners.iter().map(|p| p.y).fold(f64::MIN, f64::max),
        };
        for p in polys.iter_mut() {
            for q in p.pts.iter_mut() {
                *q = inv.apply(*q);
            }
        }
        let style = stroke::Style {
            width: self.st.line_width,
            cap: self.st.cap,
            join: self.st.join,
            miter: self.st.miter,
            dash: &self.st.dash,
            dash_off: self.st.dash_off,
        };
        let tol = TOL / m.max_scale().max(1e-300);
        let (mut out, npts, dpts) = stroke(polys, &style, tol, uv);
        self.stroke_pts.set(npts);
        self.dash_pts.set(dpts);
        for p in out.iter_mut() {
            for q in p.iter_mut() {
                *q = m.apply(*q);
            }
        }
        out
    }

    pub(crate) fn stroke_flat(&mut self, polys: Vec<Poly>) {
        let out = self.stroke_outline(polys, self.reach());
        self.fill_polys(out, false, true);
    }

    /// Intersect the clip with a `Path2D` or the context's path. Only the bounding box of the
    /// path within the current clip's box is rasterised; a clip whose mask would not fit the
    /// memory budget is ignored before any of that work (flattening included).
    pub(crate) fn clip_obj(&mut self, obj: Option<u32>, evenodd: bool) {
        let region = self.draw_region();
        let bb = match obj {
            Some(h) => self.objs.paths.get(&h).and_then(|p| p.bbox_mapped(Some(&self.st.m))),
            None => self.path.bbox(),
        };
        let r = match bb {
            Some(b) => region.intersect(&b),
            None => R::new(0, 0, 0, 0),
        };
        if r.is_empty() {
            self.st.clip = Some(Rc::new(ClipMask::new(Mask::empty(), &self.held)));
            return;
        }
        if r.width() * r.height() > self.room() {
            return;
        }
        let Some(flat) = self.flat(obj) else { return };
        let polys = polys_pts(flat);
        let held = pts_bytes(&polys);
        let (mask, work) = fill_mask_n(&polys, evenodd, r);
        self.fill_work.set(held + work);
        drop(polys);
        let new = match &self.st.clip {
            Some(old) => old.mask.intersect(&mask),
            None => mask,
        };
        self.st.clip = Some(Rc::new(ClipMask::new(new, &self.held)));
    }

    pub(crate) fn point_in_path(&self, polys: Vec<Poly>, x: f64, y: f64, evenodd: bool) -> bool {
        if !(x.is_finite() && y.is_finite()) {
            return false;
        }
        contains(&polys_pts(polys), P::new(x, y), evenodd)
    }

    pub(crate) fn point_in_stroke(&self, polys: Vec<Poly>, x: f64, y: f64) -> bool {
        if !(x.is_finite() && y.is_finite()) {
            return false;
        }
        let lim = 1e12;
        let (px, py) = (x.clamp(-lim, lim).floor() as i64, y.clamp(-lim, lim).floor() as i64);
        let out = self.stroke_outline(polys, R::new(px, py, px + 1, py + 1));
        contains(&out, P::new(x, y), false)
    }

    /// The rectangle `x, y, w, h` through the current transform, as a device path.
    pub(crate) fn rect_path(&self, x: f64, y: f64, w: f64, h: f64) -> Path {
        let mut p = Path::new();
        p.rect(&self.st.m, x, y, w, h);
        p
    }

    pub(crate) fn clear_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if self.st.m.invert().is_none() {
            return;
        }
        let p = self.rect_path(x, y, w, h);
        let polys = polys_pts(p.flatten(TOL));
        let region = self.draw_region();
        let mask = fill_mask(&polys, false, region);
        let clip = self.st.clip.clone();
        clear(&mut self.bmp, &mask, clip.as_deref().map(|c| &c.mask));
        self.dirty = true;
    }
}

/// Whole-pixel box around device-space polygons (empty if none are finite).
pub fn bbox_of(polys: &[Vec<P>]) -> R {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for q in polys.iter().flatten().filter(|q| q.finite()) {
        x0 = x0.min(q.x);
        y0 = y0.min(q.y);
        x1 = x1.max(q.x);
        y1 = y1.max(q.y);
    }
    if x0 > x1 {
        return R::new(0, 0, 0, 0);
    }
    let l = 1e12;
    R::new(
        x0.clamp(-l, l).floor() as i64,
        y0.clamp(-l, l).floor() as i64,
        x1.clamp(-l, l).ceil() as i64 + 1,
        y1.clamp(-l, l).ceil() as i64 + 1,
    )
}

/// `M` from six numeric arguments, if all are finite.
pub fn matrix_of(v: &[f64]) -> Option<M> {
    if v.len() == 6 && v.iter().all(|x| x.is_finite()) {
        Some(M([v[0], v[1], v[2], v[3], v[4], v[5]]))
    } else {
        None
    }
}
