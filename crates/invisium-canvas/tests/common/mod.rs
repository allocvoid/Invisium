#![allow(dead_code)]

use invisium_canvas::*;

/// Box glyphs: every character but space is a solid box `0.4 em` wide and `0.7 em` tall
/// sitting on the baseline, `0.05 em` right of the pen; advance `0.5 em`.
#[derive(Default)]
pub struct TestFonts {
    pub sizes: Vec<f32>,
}

impl Fonts for TestFonts {
    fn metrics(&mut self, _font: FontId, size: f32) -> FontMetrics {
        FontMetrics { ascent: 0.8 * size, descent: 0.2 * size, line_gap: 0.0, x_height: 0.5 * size }
    }
    fn shape(&mut self, _font: FontId, size: f32, text: &str, out: &mut Vec<Glyph>) -> f32 {
        let mut w = 0.0;
        for c in text.chars() {
            let id = if c == ' ' { 0 } else { (c as u32 % 60000) as u16 + 1 };
            out.push(Glyph { face: 0, id, adv: 0.5 * size });
            w += 0.5 * size;
        }
        w
    }
    fn rasterize(&mut self, _face: u16, _glyph: u16, px: f32, _subpx: u8) -> Option<GlyphBitmap> {
        self.sizes.push(px);
        let w = (px * 0.4).round().max(1.0) as u32;
        let h = (px * 0.7).round().max(1.0) as u32;
        Some(GlyphBitmap {
            width: w,
            height: h,
            left: (px * 0.05).round() as i32,
            top: -(h as i32),
            data: vec![255; (w * h) as usize],
        })
    }
}

pub fn n(v: f64) -> Arg<'static> {
    Arg::Num(v)
}

pub fn s(v: &str) -> Arg<'_> {
    Arg::Str(v)
}

pub fn col(r: u8, g: u8, b: u8, a: u8) -> Arg<'static> {
    Arg::Color([r, g, b, a])
}

pub struct T {
    pub c: Canvas2d,
    pub f: TestFonts,
}

pub fn method_op(name: &str) -> u16 {
    methods().iter().find(|m| m.0 == name).unwrap_or_else(|| panic!("no method {name}")).1
}

pub fn obj_op(kind: Kind, name: &str) -> u16 {
    object_methods(kind).iter().find(|m| m.0 == name).unwrap_or_else(|| panic!("no {name}")).1
}

impl T {
    pub fn new(w: u32, h: u32) -> T {
        T { c: Canvas2d::new(w, h), f: TestFonts::default() }
    }
    pub fn call(&mut self, name: &str, args: &[Arg]) -> Ret {
        let op = method_op(name);
        self.c.call(&mut self.f, op, args)
    }
    pub fn obj(&mut self, kind: Kind, name: &str, args: &[Arg]) -> Ret {
        let op = obj_op(kind, name);
        self.c.call(&mut self.f, op, args)
    }
    pub fn set(&mut self, prop: &str, a: Arg) {
        let p = properties().iter().find(|p| p.0 == prop).unwrap_or_else(|| panic!("no prop {prop}"));
        let op = p.2;
        self.c.call(&mut self.f, op, &[a]);
    }
    pub fn get(&mut self, prop: &str) -> Ret {
        let p = properties().iter().find(|p| p.0 == prop).unwrap_or_else(|| panic!("no prop {prop}"));
        let op = p.1;
        self.c.call(&mut self.f, op, &[])
    }
    pub fn rgba(&self) -> Vec<u8> {
        let mut v = Vec::new();
        self.c.read_rgba(&mut v);
        v
    }
    pub fn px(&self, x: u32, y: u32) -> [u8; 4] {
        let v = self.rgba();
        let w = self.c.size().0;
        let i = ((y * w + x) * 4) as usize;
        [v[i], v[i + 1], v[i + 2], v[i + 3]]
    }
    pub fn alpha(&self, x: u32, y: u32) -> u8 {
        self.px(x, y)[3]
    }
    /// Sum of alpha over the bitmap, in pixels.
    pub fn alpha_sum(&self) -> f64 {
        self.rgba().chunks(4).map(|p| p[3] as f64 / 255.0).sum()
    }
    /// Bounding box (x0, y0, x1, y1) of pixels with alpha > 0.
    pub fn ink(&self) -> Option<(u32, u32, u32, u32)> {
        let (w, h) = self.c.size();
        let v = self.rgba();
        let mut bb: Option<(u32, u32, u32, u32)> = None;
        for y in 0..h {
            for x in 0..w {
                if v[((y * w + x) * 4 + 3) as usize] > 0 {
                    bb = Some(match bb {
                        None => (x, y, x + 1, y + 1),
                        Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x + 1), d.max(y + 1)),
                    });
                }
            }
        }
        bb
    }
    pub fn clear(&mut self) {
        let (w, h) = self.c.size();
        self.call("clearRect", &[n(0.0), n(0.0), n(w as f64), n(h as f64)]);
    }
}

pub fn near(a: u8, b: u8, tol: u8) -> bool {
    (a as i32 - b as i32).abs() <= tol as i32
}

pub fn near4(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    (0..4).all(|i| near(a[i], b[i], tol))
}
