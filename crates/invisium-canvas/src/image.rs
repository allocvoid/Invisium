//! `drawImage`, `createPattern` and the `ImageData` methods.

use crate::ctx::throw;
use crate::draw::{bbox_of, polys_pts, TOL};
use crate::geom::M;
use crate::paint::{Img, Shader, Wrap};
use crate::raster::fill_mask;
use crate::budget::{Charge, PAT_OBJ};
use crate::state::{num, Pattern};
use crate::{Arg, Canvas2d, Kind, Ret};
use std::cell::RefCell;
use std::rc::Rc;

/// Largest `ImageData` handed out, in bytes.
const MAX_IMAGE_BYTES: u64 = 256 << 20;

fn pixels<'a>(a: Option<&Arg<'a>>) -> Option<(u32, u32, &'a [u8])> {
    match a {
        Some(Arg::Pixels { width, height, rgba }) => {
            let need = *width as u64 * *height as u64 * 4;
            if (rgba.len() as u64) < need {
                return None;
            }
            Some((*width, *height, &rgba[..need as usize]))
        }
        _ => None,
    }
}

/// `long` conversion (ToInt32 without the modulo; out-of-range values saturate).
fn long(a: Option<&Arg>) -> i64 {
    let v = num(a);
    if v.is_finite() {
        v.trunc().clamp(-2147483648.0, 2147483647.0) as i64
    } else {
        0
    }
}

impl Canvas2d {
    pub(crate) fn create_pattern(&mut self, a: &[Arg]) -> Ret {
        let Some((w, h, px)) = pixels(a.first()) else {
            return throw("TypeError", "not an image");
        };
        if w == 0 || h == 0 {
            return throw("InvalidStateError", "image has no pixels");
        }
        let (rx, ry) = match a.get(1) {
            None | Some(Arg::Null) | Some(Arg::Undefined) | Some(Arg::Str("")) | Some(Arg::Str("repeat")) => {
                (true, true)
            }
            Some(Arg::Str("repeat-x")) => (true, false),
            Some(Arg::Str("repeat-y")) => (false, true),
            Some(Arg::Str("no-repeat")) => (false, false),
            _ => return throw("SyntaxError", "invalid repetition"),
        };
        if self.objs.full() || px.len() + PAT_OBJ + self.objs.grow_bytes(Kind::Pattern) > self.room() {
            return throw("NotSupportedError", "canvas memory budget exhausted");
        }
        let img = Img { w, h, px: px.to_vec().into() };
        let charge = Charge::new(PAT_OBJ + img.px.len(), &self.held);
        let hd = self.objs.handle();
        self.objs.pats.insert(hd, Rc::new(RefCell::new(Pattern { img, rx, ry, m: M::ID, _charge: charge })));
        Ret::Object(Kind::Pattern, hd)
    }

    pub(crate) fn draw_image(&mut self, a: &[Arg]) -> Ret {
        let Some((iw, ih, px)) = pixels(a.first()) else {
            return throw("TypeError", "not an image");
        };
        let n = a.len();
        if !matches!(n, 3 | 5 | 9) {
            return throw("TypeError", "drawImage takes 3, 5 or 9 arguments");
        }
        let v: Vec<f64> = (1..n).map(|i| num(a.get(i))).collect();
        let (iwf, ihf) = (iw as f64, ih as f64);
        let (mut sx, mut sy, mut sw, mut sh, mut dx, mut dy, mut dw, mut dh) = match n {
            3 => (0.0, 0.0, iwf, ihf, v[0], v[1], iwf, ihf),
            5 => (0.0, 0.0, iwf, ihf, v[0], v[1], v[2], v[3]),
            9 => (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]),
            _ => return throw("TypeError", "drawImage takes 3, 5 or 9 arguments"),
        };
        if v.iter().any(|x| !x.is_finite()) || iw == 0 || ih == 0 {
            return Ret::Undefined;
        }
        if sw < 0.0 {
            sx += sw;
            sw = -sw;
        }
        if sh < 0.0 {
            sy += sh;
            sh = -sh;
        }
        if dw < 0.0 {
            dx += dw;
            dw = -dw;
        }
        if dh < 0.0 {
            dy += dh;
            dh = -dh;
        }
        if sw == 0.0 || sh == 0.0 || dw == 0.0 || dh == 0.0 {
            return Ret::Undefined;
        }
        // Clip the source rectangle to the image, the destination in proportion.
        let (kx, ky) = (dw / sw, dh / sh);
        let (cx0, cy0) = (sx.max(0.0), sy.max(0.0));
        let (cx1, cy1) = ((sx + sw).min(iwf), (sy + sh).min(ihf));
        if cx1 <= cx0 || cy1 <= cy0 {
            return Ret::Undefined;
        }
        dx += (cx0 - sx) * kx;
        dy += (cy0 - sy) * ky;
        dw = (cx1 - cx0) * kx;
        dh = (cy1 - cy0) * ky;
        let (sx, sy, sw, sh) = (cx0, cy0, cx1 - cx0, cy1 - cy0);
        let m = self.st.m;
        if m.invert().is_none() {
            return Ret::Undefined;
        }
        let img_m = m.concat(&M::translate(dx, dy)).concat(&M::scale(dw / sw, dh / sh)).concat(&M::translate(-sx, -sy));
        let Some(inv) = img_m.invert() else { return Ret::Undefined };
        let path = self.rect_path(dx, dy, dw, dh);
        let polys = polys_pts(path.flatten(TOL));
        // The caller's pixels are sampled in place.
        let img = Img { w: iw, h: ih, px: px.into() };
        let wx = Wrap::Clamp(sx.floor() as i64, ((sx + sw).ceil() as i64 - 1).max(sx.floor() as i64));
        let wy = Wrap::Clamp(sy.floor() as i64, ((sy + sh).ceil() as i64 - 1).max(sy.floor() as i64));
        let shader = Shader::Image { img: &img, inv, smooth: self.st.smoothing, wx, wy };
        self.render(&|r| fill_mask(&polys, false, r), bbox_of(&polys), &shader);
        Ret::Undefined
    }

    pub(crate) fn create_image_data(&mut self, a: &[Arg]) -> Ret {
        if let Some((w, h, _)) = pixels(a.first()) {
            if w as u64 * h as u64 * 4 > MAX_IMAGE_BYTES {
                return throw("RangeError", "ImageData too large");
            }
            return Ret::Pixels { width: w, height: h, rgba: vec![0; w as usize * h as usize * 4] };
        }
        let (w, h) = (num(a.first()), num(a.get(1)));
        if !(w.is_finite() && h.is_finite()) {
            return throw("TypeError", "non-finite size");
        }
        let (w, h) = (w.trunc().abs(), h.trunc().abs());
        if w == 0.0 || h == 0.0 {
            return throw("IndexSizeError", "zero size");
        }
        if w * h * 4.0 > MAX_IMAGE_BYTES as f64 {
            return throw("RangeError", "ImageData too large");
        }
        Ret::Pixels { width: w as u32, height: h as u32, rgba: vec![0; (w * h * 4.0) as usize] }
    }

    pub(crate) fn get_image_data(&mut self, a: &[Arg]) -> Ret {
        let v: Vec<f64> = (0..4).map(|i| num(a.get(i))).collect();
        if v.iter().any(|x| !x.is_finite()) {
            return throw("TypeError", "non-finite argument");
        }
        let (mut x, mut y, mut w, mut h) = (v[0].trunc(), v[1].trunc(), v[2].trunc(), v[3].trunc());
        if w == 0.0 || h == 0.0 {
            return throw("IndexSizeError", "zero size");
        }
        if w < 0.0 {
            x += w;
            w = -w;
        }
        if h < 0.0 {
            y += h;
            h = -h;
        }
        if w * h * 4.0 > MAX_IMAGE_BYTES as f64 {
            return throw("RangeError", "ImageData too large");
        }
        let (wu, hu) = (w as usize, h as usize);
        let mut out = vec![0u8; wu * hu * 4];
        let (bw, bh) = (self.bmp.w as f64, self.bmp.h as f64);
        let (x0, x1) = (x.max(0.0), (x + w).min(bw));
        let (y0, y1) = (y.max(0.0), (y + h).min(bh));
        if x1 > x0 && y1 > y0 {
            let n = (x1 - x0) as usize * 4;
            for yy in y0 as usize..y1 as usize {
                let src = (yy * self.bmp.w as usize + x0 as usize) * 4;
                let dst = ((yy as f64 - y) as usize * wu + (x0 - x) as usize) * 4;
                out[dst..dst + n].copy_from_slice(&self.bmp.px[src..src + n]);
            }
        }
        Ret::Pixels { width: wu as u32, height: hu as u32, rgba: out }
    }

    pub(crate) fn put_image_data(&mut self, a: &[Arg]) -> Ret {
        let Some((iw, ih, px)) = pixels(a.first()) else {
            return throw("TypeError", "not an ImageData");
        };
        let (dx, dy) = (long(a.get(1)), long(a.get(2)));
        let (mut rx, mut ry, mut rw, mut rh) = if a.len() >= 7 {
            (long(a.get(3)), long(a.get(4)), long(a.get(5)), long(a.get(6)))
        } else {
            (0, 0, iw as i64, ih as i64)
        };
        if rw < 0 {
            rx += rw;
            rw = -rw;
        }
        if rh < 0 {
            ry += rh;
            rh = -rh;
        }
        if rx < 0 {
            rw += rx;
            rx = 0;
        }
        if ry < 0 {
            rh += ry;
            ry = 0;
        }
        rw = rw.min(iw as i64 - rx);
        rh = rh.min(ih as i64 - ry);
        if rw <= 0 || rh <= 0 {
            return Ret::Undefined;
        }
        let (bw, bh) = (self.bmp.w as i64, self.bmp.h as i64);
        let x0 = (dx + rx).max(0);
        let y0 = (dy + ry).max(0);
        let x1 = (dx + rx + rw).min(bw);
        let y1 = (dy + ry + rh).min(bh);
        if x1 <= x0 || y1 <= y0 {
            return Ret::Undefined;
        }
        let n = (x1 - x0) as usize * 4;
        for y in y0..y1 {
            let src = (((y - dy) * iw as i64 + (x0 - dx)) * 4) as usize;
            let dst = ((y * bw + x0) * 4) as usize;
            self.bmp.px[dst..dst + n].copy_from_slice(&px[src..src + n]);
            // Fully transparent pixels are stored as transparent black.
            for p in self.bmp.px[dst..dst + n].as_chunks_mut::<4>().0 {
                if p[3] == 0 {
                    p.copy_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        self.dirty = true;
        Ret::Undefined
    }
}
