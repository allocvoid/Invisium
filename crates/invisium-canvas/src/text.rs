//! `fillText`, `strokeText` (filled for now) and `measureText`.

use crate::geom::{M, P, R};
use crate::raster::Mask;
use crate::state::{num, text_of};
use crate::{Arg, Canvas2d, FontMetrics, Fonts, Glyph, GlyphBitmap, Ret, TextMetrics};

/// Longest text drawn or measured, in characters.
const MAX_TEXT: usize = 65_536;
/// Largest pixel size glyphs are rasterised at; larger text is drawn resampled.
const MAX_PX: f64 = 2048.0;

struct Run {
    glyphs: Vec<(Glyph, f64)>,
    width: f64,
    fm: FontMetrics,
}

enum Placed {
    /// Bitmap at whole-pixel device position.
    At(GlyphBitmap, i64, i64),
    /// Bitmap through a map from bitmap pixels to device pixels (inverse stored).
    Mapped(GlyphBitmap, M, R),
}

/// The text argument, at most [`MAX_TEXT`] characters, with ASCII whitespace as spaces;
/// a long script string is not copied whole first.
fn prep(a: Option<&Arg>) -> String {
    let fix = |c: char| if matches!(c, '\t' | '\n' | '\x0c' | '\r') { ' ' } else { c };
    match a {
        Some(Arg::Str(s)) => s.chars().take(MAX_TEXT).map(fix).collect(),
        other => text_of(other).chars().take(MAX_TEXT).map(fix).collect(),
    }
}

/// Most bytes of glyph bitmaps one call keeps; glyphs past it are not drawn.
const MAX_GLYPH_BYTES: usize = 64 << 20;

impl Canvas2d {
    fn layout(&self, fonts: &mut dyn Fonts, text: &str) -> Run {
        let (font, size) = (self.st.font, self.st.font_size);
        let ls = self.st.letter_spacing;
        let ws = self.st.word_spacing;
        let mut raw: Vec<(Glyph, bool)> = Vec::new();
        let mut buf = Vec::new();
        if ws == 0.0 {
            fonts.shape(font, size, text, &mut buf);
            raw.extend(buf.drain(..).map(|g| (g, false)));
        } else {
            for (i, piece) in text.split(' ').enumerate() {
                if i > 0 {
                    fonts.shape(font, size, " ", &mut buf);
                    raw.extend(buf.drain(..).map(|g| (g, true)));
                }
                fonts.shape(font, size, piece, &mut buf);
                raw.extend(buf.drain(..).map(|g| (g, false)));
            }
        }
        let mut x = 0.0;
        let mut glyphs = Vec::with_capacity(raw.len());
        for (g, space) in raw {
            glyphs.push((g, x));
            let adv = if g.adv.is_finite() { g.adv as f64 } else { 0.0 };
            x += adv + ls + if space { ws } else { 0.0 };
        }
        let fm = fonts.metrics(font, size);
        Run { glyphs, width: x, fm }
    }

    /// How far the alphabetic baseline lies below the `textBaseline` line.
    fn baseline_shift(&self, fm: &FontMetrics) -> f64 {
        let (a, d) = (fm.ascent as f64, fm.descent as f64);
        match self.st.baseline {
            "top" => a,
            "hanging" => 0.8 * a,
            "middle" => (a - d) / 2.0,
            "ideographic" | "bottom" => -d,
            _ => 0.0,
        }
    }

    /// Offset of the run's start from the anchor point.
    fn align_offset(&self, width: f64) -> f64 {
        let rtl = self.st.direction == "rtl";
        match (self.st.align, rtl) {
            ("left", _) | ("start", false) | ("end", true) => 0.0,
            ("center", _) => -width / 2.0,
            _ => -width,
        }
    }

    pub(crate) fn fill_text(&mut self, fonts: &mut dyn Fonts, a: &[Arg], stroke: bool) -> Ret {
        let text = prep(a.first());
        let (x, y) = (num(a.get(1)), num(a.get(2)));
        if !(x.is_finite() && y.is_finite()) {
            return Ret::Undefined;
        }
        let max_w = match a.get(3) {
            None | Some(Arg::Undefined) => None,
            v => Some(num(v)),
        };
        if let Some(mw) = max_w {
            if mw.is_nan() || mw <= 0.0 {
                return Ret::Undefined;
            }
        }
        let size = self.st.font_size as f64;
        if self.st.m.invert().is_none() || size.is_nan() || size <= 0.0 {
            return Ret::Undefined;
        }
        let run = self.layout(fonts, &text);
        if run.glyphs.is_empty() {
            return Ret::Undefined;
        }
        let hs = match max_w {
            Some(mw) if run.width > mw => mw / run.width,
            _ => 1.0,
        };
        let shift = self.baseline_shift(&run.fm);
        let ax = self.align_offset(run.width);
        let t = self
            .st
            .m
            .concat(&M::translate(x, y + shift))
            .concat(&M::scale(hs, 1.0))
            .concat(&M::translate(ax, 0.0));
        let [ta, tb, tc, td, _, _] = t.0;
        let canvas = self.bmp.rect();
        let mut placed = Vec::new();
        let mut kept = 0usize;
        let fast = tb == 0.0 && tc == 0.0 && ta > 0.0 && td > 0.0 && (ta - td).abs() <= 1e-9 * td && size * td <= MAX_PX;
        if fast {
            let px = size * td;
            let margin = (px * 4.0 + 16.0) as i64;
            let view = canvas.expand(margin);
            for &(g, gx) in &run.glyphs {
                if g.id == 0 {
                    continue;
                }
                let pen = t.apply(P::new(gx, 0.0));
                if !pen.finite() || pen.x.abs() > 1e12 || pen.y.abs() > 1e12 {
                    continue;
                }
                let (px0, py0) = (pen.x.floor() as i64, pen.y.round() as i64);
                if px0 < view.x0 || px0 > view.x1 || py0 < view.y0 || py0 > view.y1 {
                    continue;
                }
                let sub = (((pen.x - pen.x.floor()) * 4.0).floor() as u8) & 3;
                if let Some(b) = fonts.rasterize(g.face, g.id, px as f32, sub) {
                    if b.data.len() >= b.width as usize * b.height as usize {
                        let (gx0, gy0) = (px0 + b.left as i64, py0 + b.top as i64);
                        kept += b.data.len();
                        if kept > MAX_GLYPH_BYTES {
                            break;
                        }
                        placed.push(Placed::At(b, gx0, gy0));
                    }
                }
            }
        } else {
            let s = t.det().abs().sqrt();
            let px = (size * s).clamp(1.0, MAX_PX);
            let k = px / size;
            for &(g, gx) in &run.glyphs {
                if g.id == 0 {
                    continue;
                }
                let Some(b) = fonts.rasterize(g.face, g.id, px as f32, 0) else { continue };
                if b.data.len() < b.width as usize * b.height as usize || b.width == 0 || b.height == 0 {
                    continue;
                }
                let gm = t
                    .concat(&M::translate(gx, 0.0))
                    .concat(&M::scale(1.0 / k, 1.0 / k))
                    .concat(&M::translate(b.left as f64, b.top as f64));
                let Some(inv) = gm.invert() else { continue };
                let (bw, bh) = (b.width as f64, b.height as f64);
                let pts = [P::new(0.0, 0.0), P::new(bw, 0.0), P::new(0.0, bh), P::new(bw, bh)].map(|p| gm.apply(p));
                let x0 = pts.iter().map(|p| p.x).fold(f64::MAX, f64::min);
                let x1 = pts.iter().map(|p| p.x).fold(f64::MIN, f64::max);
                let y0 = pts.iter().map(|p| p.y).fold(f64::MAX, f64::min);
                let y1 = pts.iter().map(|p| p.y).fold(f64::MIN, f64::max);
                if !(x0.is_finite() && x1.is_finite() && y0.is_finite() && y1.is_finite()) {
                    continue;
                }
                let lim = 1e9;
                let r = R::new(
                    x0.clamp(-lim, lim).floor() as i64,
                    y0.clamp(-lim, lim).floor() as i64,
                    x1.clamp(-lim, lim).ceil() as i64 + 1,
                    y1.clamp(-lim, lim).ceil() as i64 + 1,
                );
                if !r.intersect(&canvas.expand(2048)).is_empty() {
                    kept += b.data.len();
                    if kept > MAX_GLYPH_BYTES {
                        break;
                    }
                    placed.push(Placed::Mapped(b, inv, r));
                }
            }
        }
        if placed.is_empty() {
            return Ret::Undefined;
        }
        let mk = |region: R| glyph_mask(&placed, region);
        let Some(bb) = placed_bbox(&placed) else { return Ret::Undefined };
        let style = if stroke { self.st.stroke.clone() } else { self.st.fill.clone() };
        self.with_style(&style, &mut |c, sh| c.render(&mk, bb, sh));
        Ret::Undefined
    }

    pub(crate) fn measure_text(&mut self, fonts: &mut dyn Fonts, a: &[Arg]) -> Ret {
        let text = prep(a.first());
        let run = self.layout(fonts, &text);
        let fm = run.fm;
        let shift = self.baseline_shift(&fm);
        let ax = self.align_offset(run.width);
        let size = self.st.font_size as f64;
        let (mut l, mut r, mut t, mut b) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        if size > 0.0 && size <= MAX_PX {
            for &(g, gx) in &run.glyphs {
                if g.id == 0 {
                    continue;
                }
                let pen = ax + gx;
                let sub = (((pen - pen.floor()) * 4.0).floor() as u8) & 3;
                if let Some(bm) = fonts.rasterize(g.face, g.id, size as f32, sub) {
                    if bm.width == 0 || bm.height == 0 {
                        continue;
                    }
                    let x0 = pen.floor() + bm.left as f64;
                    let y0 = shift.round() + bm.top as f64;
                    l = l.min(x0);
                    r = r.max(x0 + bm.width as f64);
                    t = t.min(y0);
                    b = b.max(y0 + bm.height as f64);
                }
            }
        }
        if l > r {
            (l, r, t, b) = (ax, ax + run.width, shift, shift);
        }
        let (fa, fd) = (fm.ascent as f64, fm.descent as f64);
        let em = if fa + fd > 0.0 { fa / (fa + fd) } else { 1.0 };
        Ret::Text(TextMetrics {
            width: run.width,
            actual_left: -l,
            actual_right: r,
            font_ascent: fa - shift,
            font_descent: fd + shift,
            actual_ascent: -t,
            actual_descent: b,
            em_ascent: size * em - shift,
            em_descent: size * (1.0 - em) + shift,
            hanging_baseline: 0.8 * fa - shift,
            alphabetic_baseline: -shift,
            ideographic_baseline: -fd - shift,
        })
    }
}

fn placed_bbox(placed: &[Placed]) -> Option<R> {
    let mut bb: Option<R> = None;
    for p in placed {
        let r = match p {
            Placed::At(b, x, y) => R::new(*x, *y, x + b.width as i64, y + b.height as i64),
            Placed::Mapped(_, _, r) => *r,
        };
        bb = Some(match bb {
            None => r,
            Some(o) => R::new(o.x0.min(r.x0), o.y0.min(r.y0), o.x1.max(r.x1), o.y1.max(r.y1)),
        });
    }
    bb
}

fn glyph_mask(placed: &[Placed], region: R) -> Mask {
    let Some(bb) = placed_bbox(placed) else { return Mask::empty() };
    let mut mask = Mask::new(bb.intersect(&region));
    if mask.r.is_empty() {
        return mask;
    }
    let mr = mask.r;
    for p in placed {
        match p {
            Placed::At(b, x, y) => {
                let r = R::new(*x, *y, x + b.width as i64, y + b.height as i64).intersect(&mr);
                for yy in r.y0..r.y1 {
                    for xx in r.x0..r.x1 {
                        let v = b.data[(yy - y) as usize * b.width as usize + (xx - x) as usize];
                        mask.add(xx, yy, v);
                    }
                }
            }
            Placed::Mapped(b, inv, r) => {
                let r = r.intersect(&mr);
                let (w, h) = (b.width as i64, b.height as i64);
                let get = |x: i64, y: i64| -> f64 {
                    if x < 0 || y < 0 || x >= w || y >= h {
                        0.0
                    } else {
                        b.data[(y * w + x) as usize] as f64
                    }
                };
                for yy in r.y0..r.y1 {
                    for xx in r.x0..r.x1 {
                        let q = inv.apply(P::new(xx as f64 + 0.5, yy as f64 + 0.5));
                        let (u, v) = (q.x - 0.5, q.y - 0.5);
                        if !(u > -1.0 && v > -1.0 && u < w as f64 && v < h as f64) {
                            continue;
                        }
                        let (fu, fv) = (u.floor(), v.floor());
                        let (tx, ty) = (u - fu, v - fv);
                        let (ix, iy) = (fu as i64, fv as i64);
                        let top = get(ix, iy) * (1.0 - tx) + get(ix + 1, iy) * tx;
                        let bot = get(ix, iy + 1) * (1.0 - tx) + get(ix + 1, iy + 1) * tx;
                        let c = top * (1.0 - ty) + bot * ty;
                        mask.add(xx, yy, (c + 0.5).min(255.0) as u8);
                    }
                }
            }
        }
    }
    mask
}
