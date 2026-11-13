//! CSS value types and their parsers: colours, lengths, calc(), gradients, shadows, grid tracks.

use super::tokenizer::{split_commas, Cur, Tok};
use crate::atom::Atom;
use std::rc::Rc;

// ───────────────────────────── colour ─────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Color = Color { r: 0, g: 0, b: 0, a: 0 };
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0, a: 255 };
    pub const WHITE: Color = Color { r: 255, g: 255, b: 255, a: 255 };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }
    pub const fn hex(v: u32) -> Color {
        Color { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8, a: 255 }
    }
    pub fn with_alpha(self, a: f32) -> Color {
        Color { a: ((self.a as f32) * a.clamp(0.0, 1.0)).round() as u8, ..self }
    }
    pub fn is_transparent(self) -> bool {
        self.a == 0
    }
    pub fn to_f32(self) -> [f32; 4] {
        [self.r as f32 / 255.0, self.g as f32 / 255.0, self.b as f32 / 255.0, self.a as f32 / 255.0]
    }
    pub fn lerp(self, o: Color, t: f32) -> Color {
        // interpolate in premultiplied space so transparent stops do not darken the ramp
        let (a0, a1) = (self.a as f32 / 255.0, o.a as f32 / 255.0);
        let a = a0 + (a1 - a0) * t;
        let ch = |x: u8, y: u8| -> u8 {
            let p = x as f32 * a0 + (y as f32 * a1 - x as f32 * a0) * t;
            if a <= 0.0001 {
                0
            } else {
                (p / a).round().clamp(0.0, 255.0) as u8
            }
        };
        Color { r: ch(self.r, o.r), g: ch(self.g, o.g), b: ch(self.b, o.b), a: (a * 255.0).round() as u8 }
    }
    /// Relative luminance in 0..1 (used to pick readable foregrounds).
    pub fn luma(self) -> f32 {
        (0.2126 * self.r as f32 + 0.7152 * self.g as f32 + 0.0722 * self.b as f32) / 255.0
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CssColor {
    Rgba(Color),
    Current,
}

static NAMED_COLORS: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff), ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff), ("beige", 0xf5f5dc), ("bisque", 0xffe4c4), ("black", 0x000000),
    ("blanchedalmond", 0xffebcd), ("blue", 0x0000ff), ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0), ("chartreuse", 0x7fff00), ("chocolate", 0xd2691e),
    ("coral", 0xff7f50), ("cornflowerblue", 0x6495ed), ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c),
    ("cyan", 0x00ffff), ("darkblue", 0x00008b), ("darkcyan", 0x008b8b), ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9), ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9), ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f), ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc), ("darkred", 0x8b0000), ("darksalmon", 0xe9967a), ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b), ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1), ("darkviolet", 0x9400d3), ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff), ("dimgray", 0x696969), ("dimgrey", 0x696969), ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0), ("forestgreen", 0x228b22), ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc), ("ghostwhite", 0xf8f8ff), ("gold", 0xffd700), ("goldenrod", 0xdaa520),
    ("gray", 0x808080), ("green", 0x008000), ("greenyellow", 0xadff2f), ("grey", 0x808080),
    ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4), ("indianred", 0xcd5c5c), ("indigo", 0x4b0082),
    ("ivory", 0xfffff0), ("khaki", 0xf0e68c), ("lavender", 0xe6e6fa), ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00), ("lemonchiffon", 0xfffacd), ("lightblue", 0xadd8e6), ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff), ("lightgoldenrodyellow", 0xfafad2), ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90), ("lightgrey", 0xd3d3d3), ("lightpink", 0xffb6c1), ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa), ("lightskyblue", 0x87cefa), ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899), ("lightsteelblue", 0xb0c4de), ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00), ("limegreen", 0x32cd32), ("linen", 0xfaf0e6), ("magenta", 0xff00ff),
    ("maroon", 0x800000), ("mediumaquamarine", 0x66cdaa), ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3), ("mediumpurple", 0x9370db), ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee), ("mediumspringgreen", 0x00fa9a), ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585), ("midnightblue", 0x191970), ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1), ("moccasin", 0xffe4b5), ("navajowhite", 0xffdead), ("navy", 0x000080),
    ("oldlace", 0xfdf5e6), ("olive", 0x808000), ("olivedrab", 0x6b8e23), ("orange", 0xffa500),
    ("orangered", 0xff4500), ("orchid", 0xda70d6), ("palegoldenrod", 0xeee8aa), ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee), ("palevioletred", 0xdb7093), ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9), ("peru", 0xcd853f), ("pink", 0xffc0cb), ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6), ("purple", 0x800080), ("rebeccapurple", 0x663399), ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1), ("saddlebrown", 0x8b4513), ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460), ("seagreen", 0x2e8b57), ("seashell", 0xfff5ee), ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0), ("skyblue", 0x87ceeb), ("slateblue", 0x6a5acd), ("slategray", 0x708090),
    ("slategrey", 0x708090), ("snow", 0xfffafa), ("springgreen", 0x00ff7f), ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c), ("teal", 0x008080), ("thistle", 0xd8bfd8), ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0), ("violet", 0xee82ee), ("wheat", 0xf5deb3), ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00), ("yellowgreen", 0x9acd32),
    // system colours, mapped to a light scheme
    ("canvas", 0xffffff), ("canvastext", 0x000000), ("linktext", 0x0000ee), ("visitedtext", 0x551a8b),
    ("buttonface", 0xf0f0f0), ("buttontext", 0x000000), ("field", 0xffffff), ("fieldtext", 0x000000),
    ("graytext", 0x6d6d6d), ("highlight", 0x3390ff), ("highlighttext", 0xffffff), ("buttonborder", 0x767676),
    ("activetext", 0xee0000), ("mark", 0xffff00), ("marktext", 0x000000),
];

pub fn named_color(name: &str) -> Option<Color> {
    let lower = name.to_ascii_lowercase();
    if lower == "transparent" {
        return Some(Color::TRANSPARENT);
    }
    NAMED_COLORS.iter().find(|(n, _)| *n == lower).map(|&(_, v)| Color::hex(v))
}

pub fn parse_hex_color(h: &str) -> Option<Color> {
    let d: Option<Vec<u8>> = h.chars().map(|c| c.to_digit(16).map(|v| v as u8)).collect();
    let d = d?;
    match d.len() {
        3 => Some(Color::rgb(d[0] * 17, d[1] * 17, d[2] * 17)),
        4 => Some(Color::rgba(d[0] * 17, d[1] * 17, d[2] * 17, d[3] * 17)),
        6 => Some(Color::rgb(d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5])),
        8 => Some(Color::rgba(d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5], d[6] * 16 + d[7])),
        _ => None,
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let h = ((h % 360.0) + 360.0) % 360.0 / 360.0;
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

fn oklab_to_srgb(l: f32, a: f32, b: f32) -> (f32, f32, f32) {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3;
    let bl = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;
    (srgb_gamma(r), srgb_gamma(g), srgb_gamma(bl))
}

/// Linear light to the sRGB transfer curve, clamped to the gamut.
fn srgb_gamma(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// CIE Lab (D50 white, as CSS `lab()` defines it) to sRGB.
fn lab_to_srgb(l: f32, a: f32, b: f32) -> (f32, f32, f32) {
    const EPS: f32 = 216.0 / 24389.0;
    const KAPPA: f32 = 24389.0 / 27.0;
    let fy = (l + 16.0) / 116.0;
    let fx = a / 500.0 + fy;
    let fz = fy - b / 200.0;
    let cube = |f: f32| if f * f * f > EPS { f * f * f } else { (116.0 * f - 16.0) / KAPPA };
    let x = cube(fx) * 0.964_22;
    let y = if l > KAPPA * EPS { fy * fy * fy } else { l / KAPPA };
    let z = cube(fz) * 0.825_21;
    // XYZ (D50) to linear sRGB, chromatic adaptation to D65 included (Bradford)
    let r = 3.133_856_1 * x - 1.616_866_7 * y - 0.490_614_6 * z;
    let g = -0.978_768_4 * x + 1.916_141_5 * y + 0.033_454 * z;
    let bl = 0.071_945_3 * x - 0.228_991_4 * y + 1.405_242_7 * z;
    (srgb_gamma(r), srgb_gamma(g), srgb_gamma(bl))
}

/// One numeric component of a colour function: number, percentage, or `none`.
fn color_component(c: &mut Cur) -> Option<(f32, bool)> {
    c.skip_ws();
    match c.next()? {
        Tok::Number(v) => Some((*v, false)),
        Tok::Percentage(v) => Some((*v, true)),
        Tok::Dimension(v, u) => {
            let deg = match u.as_str() {
                "deg" => *v,
                "rad" => v.to_degrees(),
                "grad" => v * 0.9,
                "turn" => v * 360.0,
                _ => return None,
            };
            Some((deg, false))
        }
        Tok::Ident(i) if i.eq_ignore_ascii_case("none") => Some((0.0, false)),
        Tok::Function(f) if f.eq_ignore_ascii_case("calc") => {
            let mut inner = c.block();
            let v = parse_calc_sum(&mut inner)?;
            match v {
                Calc::Num(n) => Some((n, false)),
                Calc::Leaf(Len::Pct(p)) => Some((p, true)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn parse_color_function(name: &str, args: &mut Cur) -> Option<Color> {
    let lname = name.to_ascii_lowercase();
    let mut comps: Vec<(f32, bool)> = Vec::with_capacity(4);
    let mut alpha: Option<(f32, bool)> = None;
    for _ in 0..3 {
        comps.push(color_component(args)?);
        args.comma();
    }
    args.skip_ws();
    if args.delim('/') {
        alpha = Some(color_component(args)?);
    } else if !args.at_end() {
        alpha = Some(color_component(args)?);
    }
    if !args.at_end() {
        return None;
    }
    let a = match alpha {
        None => 1.0,
        Some((v, true)) => v / 100.0,
        Some((v, false)) => v,
    }
    .clamp(0.0, 1.0);
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let (r, g, b) = match lname.as_str() {
        "rgb" | "rgba" => {
            let ch = |(v, pct): (f32, bool)| if pct { v / 100.0 } else { v / 255.0 };
            (ch(comps[0]), ch(comps[1]), ch(comps[2]))
        }
        "hsl" | "hsla" => {
            let pc = |(v, _): (f32, bool)| (v / 100.0).clamp(0.0, 1.0);
            hsl_to_rgb(comps[0].0, pc(comps[1]), pc(comps[2]))
        }
        "hwb" => {
            let w = (comps[1].0 / 100.0).clamp(0.0, 1.0);
            let bk = (comps[2].0 / 100.0).clamp(0.0, 1.0);
            if w + bk >= 1.0 {
                let g = w / (w + bk);
                (g, g, g)
            } else {
                let (r, g, b) = hsl_to_rgb(comps[0].0, 1.0, 0.5);
                let f = |c: f32| c * (1.0 - w - bk) + w;
                (f(r), f(g), f(b))
            }
        }
        "oklab" => {
            let l = if comps[0].1 { comps[0].0 / 100.0 } else { comps[0].0 };
            let s = |(v, pct): (f32, bool)| if pct { v / 100.0 * 0.4 } else { v };
            oklab_to_srgb(l, s(comps[1]), s(comps[2]))
        }
        "oklch" => {
            let l = if comps[0].1 { comps[0].0 / 100.0 } else { comps[0].0 };
            let ch = if comps[1].1 { comps[1].0 / 100.0 * 0.4 } else { comps[1].0 };
            let h = comps[2].0.to_radians();
            oklab_to_srgb(l, ch * h.cos(), ch * h.sin())
        }
        // lightness 0..100 (a percentage is the same number); 100% of a/b is 125, of chroma 150
        "lab" => {
            let s = |(v, pct): (f32, bool)| if pct { v * 1.25 } else { v };
            lab_to_srgb(comps[0].0.clamp(0.0, 100.0), s(comps[1]), s(comps[2]))
        }
        "lch" => {
            let ch = if comps[1].1 { comps[1].0 * 1.5 } else { comps[1].0 }.max(0.0);
            let h = comps[2].0.to_radians();
            lab_to_srgb(comps[0].0.clamp(0.0, 100.0), ch * h.cos(), ch * h.sin())
        }
        _ => return None,
    };
    Some(Color { r: to8(r), g: to8(g), b: to8(b), a: to8(a) })
}

/// Parse one colour at the cursor. Leaves the cursor untouched on failure.
pub fn parse_color(c: &mut Cur) -> Option<CssColor> {
    let save = c.i;
    c.skip_ws();
    let r = match c.next() {
        Some(Tok::Hash(h)) => parse_hex_color(h).map(CssColor::Rgba),
        Some(Tok::Ident(i)) => {
            if i.eq_ignore_ascii_case("currentcolor") {
                Some(CssColor::Current)
            } else {
                named_color(i).map(CssColor::Rgba)
            }
        }
        Some(Tok::Function(f)) => {
            let mut args = c.block();
            if f.eq_ignore_ascii_case("color-mix") {
                parse_color_mix(&mut args).map(CssColor::Rgba)
            } else if f.eq_ignore_ascii_case("light-dark") {
                // we always report a light scheme
                let first = parse_color(&mut args);
                first
            } else {
                parse_color_function(f, &mut args).map(CssColor::Rgba)
            }
        }
        _ => None,
    };
    if r.is_none() {
        c.i = save;
    }
    r
}

fn parse_color_mix(args: &mut Cur) -> Option<Color> {
    // color-mix(in <space>, <color> [p%]?, <color> [p%]?) — mixed in sRGB regardless of the space
    let parts = split_commas(args.rest());
    if parts.len() != 3 {
        return None;
    }
    let one = |t: &[Tok]| -> Option<(Color, Option<f32>)> {
        let mut c = Cur::new(t);
        let mut pct = None;
        c.skip_ws();
        if let Some(Tok::Percentage(p)) = c.peek() {
            pct = Some(*p / 100.0);
            c.next();
        }
        let col = match parse_color(&mut c)? {
            CssColor::Rgba(c) => c,
            CssColor::Current => Color::BLACK,
        };
        c.skip_ws();
        if let Some(Tok::Percentage(p)) = c.peek() {
            pct = Some(*p / 100.0);
        }
        Some((col, pct))
    };
    let (c1, p1) = one(parts[1])?;
    let (c2, p2) = one(parts[2])?;
    let t = match (p1, p2) {
        (Some(a), _) => 1.0 - a,
        (None, Some(b)) => b,
        (None, None) => 0.5,
    };
    Some(c1.lerp(c2, t.clamp(0.0, 1.0)))
}

// ───────────────────────────── lengths ─────────────────────────────

/// A specified length (before font-relative and viewport units are resolved).
#[derive(Clone, Debug, PartialEq)]
pub enum Len {
    Px(f32),
    Em(f32),
    Rem(f32),
    Pct(f32),
    Vw(f32),
    Vh(f32),
    Vmin(f32),
    Vmax(f32),
    Ch(f32),
    Ex(f32),
    Calc(Rc<Calc>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Calc {
    Leaf(Len),
    Num(f32),
    Add(Box<Calc>, Box<Calc>),
    Sub(Box<Calc>, Box<Calc>),
    Mul(Box<Calc>, Box<Calc>),
    Div(Box<Calc>, Box<Calc>),
    Min(Vec<Calc>),
    Max(Vec<Calc>),
    Clamp(Box<Calc>, Box<Calc>, Box<Calc>),
    /// `round(strategy, value, step)`.
    Round(Rounding, Box<Calc>, Box<Calc>),
}

/// How `round()` picks a multiple of its step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rounding {
    Nearest,
    Up,
    Down,
    ToZero,
}

impl Rounding {
    /// `value` as a multiple of `step` (a step of zero, or anything not finite, leaves the
    /// value as it is: a page must not be able to make a length that is not a number).
    pub fn apply(self, value: f32, step: f32) -> f32 {
        let step = step.abs();
        if !value.is_finite() || !step.is_finite() || step == 0.0 {
            return value;
        }
        let q = value / step;
        step * match self {
            Rounding::Nearest => q.round(),
            Rounding::Up => q.ceil(),
            Rounding::Down => q.floor(),
            Rounding::ToZero => q.trunc(),
        }
    }
}

impl Len {
    pub const ZERO: Len = Len::Px(0.0);
}

pub fn dimension_to_len(v: f32, unit: &str) -> Option<Len> {
    Some(match unit {
        "px" => Len::Px(v),
        "em" => Len::Em(v),
        "rem" | "rlh" => Len::Rem(v),
        "pt" => Len::Px(v * 96.0 / 72.0),
        "pc" => Len::Px(v * 16.0),
        "in" => Len::Px(v * 96.0),
        "cm" => Len::Px(v * 96.0 / 2.54),
        "mm" => Len::Px(v * 96.0 / 25.4),
        "q" => Len::Px(v * 96.0 / 101.6),
        "vw" | "svw" | "lvw" | "dvw" | "vi" | "cqw" | "cqi" => Len::Vw(v),
        "vh" | "svh" | "lvh" | "dvh" | "vb" | "cqh" | "cqb" => Len::Vh(v),
        "vmin" | "svmin" | "dvmin" | "lvmin" | "cqmin" => Len::Vmin(v),
        "vmax" | "svmax" | "dvmax" | "lvmax" | "cqmax" => Len::Vmax(v),
        "ch" | "ic" => Len::Ch(v),
        "ex" | "cap" => Len::Ex(v),
        "lh" => Len::Em(v * 1.2),
        _ => return None,
    })
}

fn parse_calc_value(c: &mut Cur) -> Option<Calc> {
    c.skip_ws();
    match c.next()? {
        Tok::Number(v) => Some(Calc::Num(*v)),
        Tok::Percentage(v) => Some(Calc::Leaf(Len::Pct(*v))),
        Tok::Dimension(v, u) => dimension_to_len(*v, u).map(Calc::Leaf),
        Tok::LParen => {
            let mut inner = c.block();
            let v = parse_calc_sum(&mut inner)?;
            if inner.at_end() {
                Some(v)
            } else {
                None
            }
        }
        Tok::Function(f) => {
            let mut inner = c.block();
            parse_math_function(f, &mut inner)
        }
        Tok::Ident(i) => match i.to_ascii_lowercase().as_str() {
            "pi" => Some(Calc::Num(std::f32::consts::PI)),
            "e" => Some(Calc::Num(std::f32::consts::E)),
            _ => None,
        },
        _ => None,
    }
}

fn parse_calc_product(c: &mut Cur) -> Option<Calc> {
    let mut left = parse_calc_value(c)?;
    loop {
        c.skip_ws();
        match c.peek() {
            Some(Tok::Delim('*')) => {
                c.next();
                let right = parse_calc_value(c)?;
                left = Calc::Mul(Box::new(left), Box::new(right));
            }
            Some(Tok::Delim('/')) => {
                c.next();
                let right = parse_calc_value(c)?;
                left = Calc::Div(Box::new(left), Box::new(right));
            }
            _ => return Some(left),
        }
    }
}

pub fn parse_calc_sum(c: &mut Cur) -> Option<Calc> {
    let mut left = parse_calc_product(c)?;
    loop {
        c.skip_ws();
        match c.peek() {
            Some(Tok::Delim('+')) => {
                c.next();
                let right = parse_calc_product(c)?;
                left = Calc::Add(Box::new(left), Box::new(right));
            }
            Some(Tok::Delim('-')) => {
                c.next();
                let right = parse_calc_product(c)?;
                left = Calc::Sub(Box::new(left), Box::new(right));
            }
            // "1px -2px" tokenizes the minus into the number; treat a signed number here as addition
            Some(Tok::Number(v)) if *v < 0.0 => {
                let right = parse_calc_product(c)?;
                left = Calc::Add(Box::new(left), Box::new(right));
            }
            Some(Tok::Dimension(v, _)) if *v < 0.0 => {
                let right = parse_calc_product(c)?;
                left = Calc::Add(Box::new(left), Box::new(right));
            }
            _ => return Some(left),
        }
    }
}

fn parse_math_function(name: &str, inner: &mut Cur) -> Option<Calc> {
    match name.to_ascii_lowercase().as_str() {
        "calc" | "-webkit-calc" | "-moz-calc" => {
            let v = parse_calc_sum(inner)?;
            if inner.at_end() {
                Some(v)
            } else {
                None
            }
        }
        f @ ("min" | "max") => {
            let mut items = Vec::new();
            for part in split_commas(inner.rest()) {
                let mut pc = Cur::new(part);
                let v = parse_calc_sum(&mut pc)?;
                if !pc.at_end() {
                    return None;
                }
                items.push(v);
            }
            if items.is_empty() {
                return None;
            }
            Some(if f == "min" { Calc::Min(items) } else { Calc::Max(items) })
        }
        "clamp" => {
            let parts = split_commas(inner.rest());
            if parts.len() != 3 {
                return None;
            }
            let mut vals = Vec::with_capacity(3);
            for part in parts {
                let mut pc = Cur::new(part);
                // `none` is allowed for min/max bounds
                if let Some(id) = pc.clone().ident() {
                    if id.eq_ignore_ascii_case("none") {
                        vals.push(None);
                        continue;
                    }
                }
                vals.push(Some(parse_calc_sum(&mut pc)?));
            }
            let mid = vals[1].clone()?;
            let lo = vals[0].clone().unwrap_or(Calc::Leaf(Len::Px(-1.0e9)));
            let hi = vals[2].clone().unwrap_or(Calc::Leaf(Len::Px(1.0e9)));
            Some(Calc::Clamp(Box::new(lo), Box::new(mid), Box::new(hi)))
        }
        "round" => {
            // round( <strategy>? , value , step? ): the step is 1 when left out
            let mut parts = split_commas(inner.rest());
            let mut mode = Rounding::Nearest;
            if let Some(id) = parts.first().and_then(|p| Cur::new(p).ident()) {
                mode = match id.to_ascii_lowercase().as_str() {
                    "nearest" => Rounding::Nearest,
                    "up" => Rounding::Up,
                    "down" => Rounding::Down,
                    "to-zero" => Rounding::ToZero,
                    _ => return None,
                };
                parts.remove(0);
            }
            if parts.is_empty() || parts.len() > 2 {
                return None;
            }
            let mut vals = Vec::with_capacity(2);
            for part in parts {
                let mut pc = Cur::new(part);
                let v = parse_calc_sum(&mut pc)?;
                if !pc.at_end() {
                    return None;
                }
                vals.push(v);
            }
            let step = if vals.len() == 2 { vals.pop()? } else { Calc::Num(1.0) };
            Some(Calc::Round(mode, Box::new(vals.pop()?), Box::new(step)))
        }
        _ => None,
    }
}

fn is_math_fn(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "calc" | "-webkit-calc" | "-moz-calc" | "min" | "max" | "clamp" | "round"
    )
}

/// Parse a `<length-percentage>`. A bare `0` is a length; other bare numbers are accepted only
/// when `unitless_px` is set (legacy quirk used by presentational attributes and SVG).
pub fn parse_len(c: &mut Cur, unitless_px: bool) -> Option<Len> {
    let save = c.i;
    c.skip_ws();
    let r = match c.next() {
        Some(Tok::Dimension(v, u)) => dimension_to_len(*v, u),
        Some(Tok::Percentage(v)) => Some(Len::Pct(*v)),
        Some(Tok::Number(v)) if *v == 0.0 || unitless_px => Some(Len::Px(*v)),
        Some(Tok::Function(f)) if is_math_fn(f) => {
            let mut inner = c.block();
            parse_math_function(f, &mut inner).map(|calc| match calc {
                Calc::Leaf(l) => l,
                Calc::Num(n) => Len::Px(n),
                other => Len::Calc(Rc::new(other)),
            })
        }
        _ => None,
    };
    if r.is_none() {
        c.i = save;
    }
    r
}

/// Parse a plain number (also accepts `calc()` that reduces to a number).
pub fn parse_number(c: &mut Cur) -> Option<f32> {
    let save = c.i;
    c.skip_ws();
    let r = match c.next() {
        Some(Tok::Number(v)) => Some(*v),
        Some(Tok::Function(f)) if is_math_fn(f) => {
            let mut inner = c.block();
            match parse_math_function(f, &mut inner) {
                Some(calc) => eval_number(&calc),
                None => None,
            }
        }
        _ => None,
    };
    if r.is_none() {
        c.i = save;
    }
    r
}

fn eval_number(c: &Calc) -> Option<f32> {
    Some(match c {
        Calc::Num(n) => *n,
        Calc::Leaf(_) => return None,
        Calc::Add(a, b) => eval_number(a)? + eval_number(b)?,
        Calc::Sub(a, b) => eval_number(a)? - eval_number(b)?,
        Calc::Mul(a, b) => eval_number(a)? * eval_number(b)?,
        Calc::Div(a, b) => eval_number(a)? / eval_number(b)?,
        Calc::Min(v) => v.iter().map(eval_number).collect::<Option<Vec<_>>>()?.into_iter().fold(f32::INFINITY, f32::min),
        Calc::Max(v) => v.iter().map(eval_number).collect::<Option<Vec<_>>>()?.into_iter().fold(f32::NEG_INFINITY, f32::max),
        Calc::Clamp(lo, mid, hi) => eval_number(mid)?.max(eval_number(lo)?).min(eval_number(hi)?),
        Calc::Round(mode, v, step) => mode.apply(eval_number(v)?, eval_number(step)?),
    })
}

/// Angle in degrees.
pub fn parse_angle(c: &mut Cur) -> Option<f32> {
    let save = c.i;
    c.skip_ws();
    let r = match c.next() {
        Some(Tok::Dimension(v, u)) => match u.as_str() {
            "deg" => Some(*v),
            "rad" => Some(v.to_degrees()),
            "grad" => Some(v * 0.9),
            "turn" => Some(v * 360.0),
            _ => None,
        },
        Some(Tok::Number(v)) if *v == 0.0 => Some(0.0),
        Some(Tok::Function(f)) if f.eq_ignore_ascii_case("calc") => {
            let mut inner = c.block();
            match angle_sum(&mut inner) {
                Some((v, true)) if inner.at_end() => Some(v),
                Some((v, false)) if v == 0.0 && inner.at_end() => Some(0.0),
                _ => None,
            }
        }
        _ => None,
    };
    if r.is_none() {
        c.i = save;
    }
    r
}

fn angle_unit(v: f32, unit: &str) -> Option<f32> {
    Some(match unit.to_ascii_lowercase().as_str() {
        "deg" => v,
        "rad" => v.to_degrees(),
        "grad" => v * 0.9,
        "turn" => v * 360.0,
        _ => return None,
    })
}

/// `calc()` over angles and plain numbers (`calc(90deg * -1)`): the value in degrees and
/// whether it is an angle (as opposed to a bare number).
fn angle_sum(c: &mut Cur) -> Option<(f32, bool)> {
    let mut acc = angle_product(c)?;
    loop {
        c.skip_ws();
        let sign = match c.peek() {
            Some(Tok::Delim('+')) => 1.0,
            Some(Tok::Delim('-')) => -1.0,
            _ => break,
        };
        c.next();
        let rhs = angle_product(c)?;
        if rhs.1 != acc.1 {
            return None;
        }
        acc.0 += sign * rhs.0;
    }
    Some(acc)
}

fn angle_product(c: &mut Cur) -> Option<(f32, bool)> {
    let mut acc = angle_atom(c)?;
    loop {
        c.skip_ws();
        match c.peek() {
            Some(Tok::Delim('*')) => {
                c.next();
                let r = angle_atom(c)?;
                if acc.1 && r.1 {
                    return None;
                }
                acc = (acc.0 * r.0, acc.1 || r.1);
            }
            Some(Tok::Delim('/')) => {
                c.next();
                let r = angle_atom(c)?;
                if r.1 || r.0 == 0.0 {
                    return None;
                }
                acc.0 /= r.0;
            }
            _ => break,
        }
    }
    Some(acc)
}

fn angle_atom(c: &mut Cur) -> Option<(f32, bool)> {
    c.skip_ws();
    match c.next()? {
        Tok::Number(v) => Some((*v, false)),
        Tok::Dimension(v, u) => angle_unit(*v, u).map(|d| (d, true)),
        Tok::LParen => {
            let mut inner = c.block();
            angle_sum(&mut inner)
        }
        Tok::Function(f) if f.eq_ignore_ascii_case("calc") => {
            let mut inner = c.block();
            angle_sum(&mut inner)
        }
        _ => None,
    }
}

/// One function of a `transform` list, as written (lengths not yet resolved).
#[derive(Clone, Debug, PartialEq)]
pub enum TransformFn {
    Translate(Len, Len),
    Scale(f32, f32),
    /// Clockwise, in degrees.
    Rotate(f32),
    /// Degrees along x and y.
    Skew(f32, f32),
    Matrix([f32; 6]),
}

/// A number or a percentage as a factor (`scale(110%)`).
pub fn parse_factor(c: &mut Cur) -> Option<f32> {
    c.skip_ws();
    if let Some(Tok::Percentage(v)) = c.peek() {
        c.next();
        return Some(*v / 100.0);
    }
    parse_number(c)
}

/// The functions of a `transform` value. Three-dimensional functions are flattened onto the
/// page: z offsets are dropped, and a rotation about the x or y axis becomes the squash it
/// would look like without perspective.
pub fn parse_transform_list(toks: &[Tok]) -> Option<Vec<TransformFn>> {
    let mut c = Cur::new(toks);
    let mut out = Vec::new();
    loop {
        c.skip_ws();
        let Some(t) = c.next() else { break };
        let Tok::Function(f) = t else { return None };
        let mut a = c.block();
        let name = f.to_ascii_lowercase();
        match name.as_str() {
            "translate" => {
                let x = parse_len(&mut a, true)?;
                let y = if a.comma() { parse_len(&mut a, true)? } else { Len::Px(0.0) };
                out.push(TransformFn::Translate(x, y));
            }
            "translate3d" => {
                let x = parse_len(&mut a, true)?;
                a.comma();
                let y = parse_len(&mut a, true)?;
                out.push(TransformFn::Translate(x, y));
            }
            "translatex" => out.push(TransformFn::Translate(parse_len(&mut a, true)?, Len::Px(0.0))),
            "translatey" => out.push(TransformFn::Translate(Len::Px(0.0), parse_len(&mut a, true)?)),
            "translatez" | "perspective" => {}
            "scale" => {
                let x = parse_factor(&mut a)?;
                let y = if a.comma() { parse_factor(&mut a)? } else { x };
                out.push(TransformFn::Scale(x, y));
            }
            "scale3d" => {
                let x = parse_factor(&mut a)?;
                a.comma();
                let y = parse_factor(&mut a)?;
                out.push(TransformFn::Scale(x, y));
            }
            "scalex" => out.push(TransformFn::Scale(parse_factor(&mut a)?, 1.0)),
            "scaley" => out.push(TransformFn::Scale(1.0, parse_factor(&mut a)?)),
            "scalez" => {}
            "rotate" | "rotatez" => out.push(TransformFn::Rotate(parse_angle(&mut a)?)),
            "rotatex" => out.push(TransformFn::Scale(1.0, parse_angle(&mut a)?.to_radians().cos())),
            "rotatey" => out.push(TransformFn::Scale(parse_angle(&mut a)?.to_radians().cos(), 1.0)),
            "rotate3d" => {
                let x = parse_number(&mut a)?;
                a.comma();
                let y = parse_number(&mut a)?;
                a.comma();
                let z = parse_number(&mut a)?;
                a.comma();
                let deg = parse_angle(&mut a)?;
                if z != 0.0 && x == 0.0 && y == 0.0 {
                    out.push(TransformFn::Rotate(if z < 0.0 { -deg } else { deg }));
                } else if x != 0.0 && y == 0.0 && z == 0.0 {
                    out.push(TransformFn::Scale(1.0, deg.to_radians().cos()));
                } else if y != 0.0 && x == 0.0 && z == 0.0 {
                    out.push(TransformFn::Scale(deg.to_radians().cos(), 1.0));
                }
            }
            "skew" => {
                let x = parse_angle(&mut a)?;
                let y = if a.comma() { parse_angle(&mut a)? } else { 0.0 };
                out.push(TransformFn::Skew(x, y));
            }
            "skewx" => out.push(TransformFn::Skew(parse_angle(&mut a)?, 0.0)),
            "skewy" => out.push(TransformFn::Skew(0.0, parse_angle(&mut a)?)),
            "matrix" => {
                let mut m = [0.0f32; 6];
                for (i, v) in m.iter_mut().enumerate() {
                    if i > 0 {
                        a.comma();
                    }
                    *v = parse_number(&mut a)?;
                }
                out.push(TransformFn::Matrix(m));
            }
            "matrix3d" => {
                let mut m = [0.0f32; 16];
                for (i, v) in m.iter_mut().enumerate() {
                    if i > 0 {
                        a.comma();
                    }
                    *v = parse_number(&mut a)?;
                }
                out.push(TransformFn::Matrix([m[0], m[1], m[4], m[5], m[12], m[13]]));
            }
            _ => return None,
        }
    }
    if out.iter().any(|f| match f {
        TransformFn::Scale(x, y) | TransformFn::Skew(x, y) => !x.is_finite() || !y.is_finite(),
        TransformFn::Rotate(d) => !d.is_finite(),
        TransformFn::Matrix(m) => m.iter().any(|v| !v.is_finite()),
        TransformFn::Translate(..) => false,
    }) {
        return None;
    }
    Some(out)
}

// ───────────────────── computed sizes (px / percentage / calc) ─────────────────────

/// Context for resolving font-relative and viewport-relative units to pixels.
#[derive(Clone, Copy)]
pub struct LenCtx {
    pub font_size: f32,
    pub root_font_size: f32,
    pub vw: f32,
    pub vh: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SizeCalc {
    Px(f32),
    Pct(f32),
    Add(Box<SizeCalc>, Box<SizeCalc>),
    Sub(Box<SizeCalc>, Box<SizeCalc>),
    Scale(Box<SizeCalc>, f32),
    Min(Vec<SizeCalc>),
    Max(Vec<SizeCalc>),
    Clamp(Box<SizeCalc>, Box<SizeCalc>, Box<SizeCalc>),
    Round(Rounding, Box<SizeCalc>, Box<SizeCalc>),
}

impl SizeCalc {
    pub fn eval(&self, basis: f32) -> f32 {
        match self {
            SizeCalc::Px(v) => *v,
            SizeCalc::Pct(p) => basis * p / 100.0,
            SizeCalc::Add(a, b) => a.eval(basis) + b.eval(basis),
            SizeCalc::Sub(a, b) => a.eval(basis) - b.eval(basis),
            SizeCalc::Scale(a, k) => a.eval(basis) * k,
            SizeCalc::Min(v) => v.iter().map(|x| x.eval(basis)).fold(f32::INFINITY, f32::min),
            SizeCalc::Max(v) => v.iter().map(|x| x.eval(basis)).fold(f32::NEG_INFINITY, f32::max),
            SizeCalc::Clamp(lo, mid, hi) => mid.eval(basis).max(lo.eval(basis)).min(hi.eval(basis)),
            SizeCalc::Round(mode, v, step) => mode.apply(v.eval(basis), step.eval(basis)),
        }
    }
    fn has_pct(&self) -> bool {
        match self {
            SizeCalc::Px(_) => false,
            SizeCalc::Pct(_) => true,
            SizeCalc::Add(a, b) | SizeCalc::Sub(a, b) => a.has_pct() || b.has_pct(),
            SizeCalc::Scale(a, _) => a.has_pct(),
            SizeCalc::Min(v) | SizeCalc::Max(v) => v.iter().any(|x| x.has_pct()),
            SizeCalc::Clamp(a, b, c) => a.has_pct() || b.has_pct() || c.has_pct(),
            SizeCalc::Round(_, a, b) => a.has_pct() || b.has_pct(),
        }
    }
}

/// A computed length: absolute, a percentage of some basis, or `auto`.
#[derive(Clone, Debug, PartialEq)]
pub enum Size {
    Auto,
    Px(f32),
    Pct(f32),
    Calc(Rc<SizeCalc>),
    MinContent,
    MaxContent,
    FitContent,
}

impl Size {
    pub const ZERO: Size = Size::Px(0.0);

    #[inline]
    pub fn is_auto(&self) -> bool {
        matches!(self, Size::Auto)
    }

    /// Resolve against a definite basis. `auto` and intrinsic keywords give `None`.
    #[inline]
    pub fn resolve(&self, basis: f32) -> Option<f32> {
        match self {
            Size::Px(v) => Some(*v),
            Size::Pct(p) => Some(basis * p / 100.0),
            Size::Calc(c) => Some(c.eval(basis)),
            _ => None,
        }
    }

    /// Resolve against a basis that may be indefinite: percentages of an indefinite basis are `None`.
    #[inline]
    pub fn resolve_opt(&self, basis: Option<f32>) -> Option<f32> {
        match self {
            Size::Px(v) => Some(*v),
            Size::Pct(_) | Size::Calc(_) => basis.and_then(|b| self.resolve(b)),
            _ => None,
        }
    }

    /// Resolve, treating `auto` as zero (margins, padding, offsets).
    #[inline]
    pub fn px(&self, basis: f32) -> f32 {
        self.resolve(basis).unwrap_or(0.0)
    }

    pub fn has_pct(&self) -> bool {
        matches!(self, Size::Pct(_) | Size::Calc(_))
    }
}

impl Len {
    pub fn to_px(&self, ctx: &LenCtx) -> Option<f32> {
        Some(match self {
            Len::Px(v) => *v,
            Len::Em(v) => v * ctx.font_size,
            Len::Rem(v) => v * ctx.root_font_size,
            Len::Vw(v) => v * ctx.vw / 100.0,
            Len::Vh(v) => v * ctx.vh / 100.0,
            Len::Vmin(v) => v * ctx.vw.min(ctx.vh) / 100.0,
            Len::Vmax(v) => v * ctx.vw.max(ctx.vh) / 100.0,
            Len::Ch(v) => v * ctx.font_size * 0.5,
            Len::Ex(v) => v * ctx.font_size * 0.5,
            Len::Pct(_) => return None,
            Len::Calc(c) => match calc_to_size(c, ctx) {
                SizeCalc::Px(v) => v,
                other if !other.has_pct() => other.eval(0.0),
                _ => return None,
            },
        })
    }

    pub fn to_size(&self, ctx: &LenCtx) -> Size {
        match self {
            Len::Pct(p) => Size::Pct(*p),
            Len::Calc(c) => {
                let sc = calc_to_size(c, ctx);
                if sc.has_pct() {
                    match sc {
                        SizeCalc::Pct(p) => Size::Pct(p),
                        other => Size::Calc(Rc::new(other)),
                    }
                } else {
                    Size::Px(sc.eval(0.0))
                }
            }
            other => Size::Px(other.to_px(ctx).unwrap_or(0.0)),
        }
    }

    /// Resolve to pixels with a known percentage basis.
    pub fn to_px_with(&self, ctx: &LenCtx, basis: f32) -> f32 {
        self.to_size(ctx).px(basis)
    }
}

fn calc_to_size(c: &Calc, ctx: &LenCtx) -> SizeCalc {
    match c {
        Calc::Num(n) => SizeCalc::Px(*n),
        Calc::Leaf(Len::Pct(p)) => SizeCalc::Pct(*p),
        Calc::Leaf(Len::Calc(inner)) => calc_to_size(inner, ctx),
        Calc::Leaf(l) => SizeCalc::Px(l.to_px(ctx).unwrap_or(0.0)),
        Calc::Add(a, b) => SizeCalc::Add(Box::new(calc_to_size(a, ctx)), Box::new(calc_to_size(b, ctx))),
        Calc::Sub(a, b) => SizeCalc::Sub(Box::new(calc_to_size(a, ctx)), Box::new(calc_to_size(b, ctx))),
        Calc::Mul(a, b) => {
            if let Some(k) = eval_number(a) {
                SizeCalc::Scale(Box::new(calc_to_size(b, ctx)), k)
            } else if let Some(k) = eval_number(b) {
                SizeCalc::Scale(Box::new(calc_to_size(a, ctx)), k)
            } else {
                SizeCalc::Px(0.0)
            }
        }
        Calc::Div(a, b) => match eval_number(b) {
            Some(k) if k != 0.0 => SizeCalc::Scale(Box::new(calc_to_size(a, ctx)), 1.0 / k),
            _ => SizeCalc::Px(0.0),
        },
        Calc::Min(v) => SizeCalc::Min(v.iter().map(|x| calc_to_size(x, ctx)).collect()),
        Calc::Max(v) => SizeCalc::Max(v.iter().map(|x| calc_to_size(x, ctx)).collect()),
        Calc::Clamp(lo, mid, hi) => SizeCalc::Clamp(
            Box::new(calc_to_size(lo, ctx)),
            Box::new(calc_to_size(mid, ctx)),
            Box::new(calc_to_size(hi, ctx)),
        ),
        Calc::Round(mode, v, step) => SizeCalc::Round(*mode, Box::new(calc_to_size(v, ctx)), Box::new(calc_to_size(step, ctx))),
    }
}

// ───────────────────────────── images / gradients ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct Stop {
    pub color: Color,
    pub pos: Option<Len>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GradientDir {
    /// CSS angle in degrees: 0 = to top, 90 = to right.
    Angle(f32),
    /// `to <corner>`: (horizontal sign, vertical sign) each in {-1, 0, 1}.
    Corner(i8, i8),
}

#[derive(Clone, Debug, PartialEq)]
pub enum RadialSize {
    ClosestSide,
    FarthestSide,
    ClosestCorner,
    FarthestCorner,
    Explicit(Len, Len),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Image {
    None,
    Url(Rc<str>),
    Linear { dir: GradientDir, stops: Vec<Stop>, repeating: bool },
    Radial { circle: bool, size: RadialSize, center: (Len, Len), stops: Vec<Stop>, repeating: bool },
}

fn parse_stops(parts: &[&[Tok]]) -> Option<Vec<Stop>> {
    let mut stops = Vec::new();
    for part in parts {
        let mut c = Cur::new(part);
        c.skip_ws();
        let color = match parse_color(&mut c) {
            Some(CssColor::Rgba(col)) => col,
            Some(CssColor::Current) => Color::BLACK,
            None => {
                // a bare position is a colour hint; ignore it
                if parse_len(&mut c, false).is_some() && c.at_end() {
                    continue;
                }
                return None;
            }
        };
        let p1 = parse_len(&mut c, false);
        let p2 = parse_len(&mut c, false);
        stops.push(Stop { color, pos: p1 });
        if let Some(p2) = p2 {
            stops.push(Stop { color, pos: Some(p2) });
        }
    }
    if stops.len() < 2 {
        if stops.len() == 1 {
            let s = stops[0].clone();
            stops.push(s);
        } else {
            return None;
        }
    }
    Some(stops)
}

/// Parse a position like `center`, `left top`, `50% 20px`, `right 10px bottom`.
pub fn parse_position(c: &mut Cur) -> Option<(Len, Len)> {
    let mut x: Option<Len> = None;
    let mut y: Option<Len> = None;
    let mut count = 0;
    loop {
        c.skip_ws();
        let save = c.i;
        match c.peek() {
            Some(Tok::Ident(i)) => {
                let (is_x, is_y, v) = match i.to_ascii_lowercase().as_str() {
                    "left" => (true, false, 0.0),
                    "right" => (true, false, 100.0),
                    "top" => (false, true, 0.0),
                    "bottom" => (false, true, 100.0),
                    "center" => (false, false, 50.0),
                    _ => break,
                };
                c.next();
                // optional offset after an edge keyword (e.g. `right 12px`)
                let mut len = Len::Pct(v);
                if is_x || is_y {
                    let s2 = c.i;
                    if let Some(off) = parse_len(c, false) {
                        if v == 0.0 {
                            len = off;
                        } else {
                            len = Len::Calc(Rc::new(Calc::Sub(
                                Box::new(Calc::Leaf(Len::Pct(100.0))),
                                Box::new(Calc::Leaf(off)),
                            )));
                        }
                    } else {
                        c.i = s2;
                    }
                }
                if is_x {
                    x = Some(len);
                } else if is_y {
                    y = Some(len);
                } else if x.is_none() && count == 0 {
                    x = Some(len);
                } else if y.is_none() {
                    y = Some(len);
                } else {
                    x = Some(len);
                }
            }
            Some(_) => match parse_len(c, false) {
                Some(l) => {
                    if x.is_none() {
                        x = Some(l);
                    } else if y.is_none() {
                        y = Some(l);
                    } else {
                        c.i = save;
                        break;
                    }
                }
                None => break,
            },
            None => break,
        }
        count += 1;
        if count >= 4 {
            break;
        }
    }
    if count == 0 {
        return None;
    }
    Some((x.unwrap_or(Len::Pct(50.0)), y.unwrap_or(Len::Pct(50.0))))
}

fn parse_gradient(name: &str, args: &mut Cur) -> Option<Image> {
    let lname = name.to_ascii_lowercase();
    let lname = lname.trim_start_matches("-webkit-").trim_start_matches("-moz-");
    let repeating = lname.starts_with("repeating-");
    let kind = lname.trim_start_matches("repeating-");
    let parts = split_commas(args.rest());
    if parts.is_empty() {
        return None;
    }
    match kind {
        "linear-gradient" => {
            let mut first = Cur::new(parts[0]);
            let mut dir = GradientDir::Angle(180.0);
            let mut consumed = false;
            first.skip_ws();
            if let Some(a) = parse_angle(&mut first) {
                if first.at_end() {
                    dir = GradientDir::Angle(a);
                    consumed = true;
                }
            } else if first.clone().ident().map_or(false, |i| i.eq_ignore_ascii_case("to")) {
                first.ident();
                let (mut h, mut v) = (0i8, 0i8);
                while let Some(i) = first.ident() {
                    match i.to_ascii_lowercase().as_str() {
                        "left" => h = -1,
                        "right" => h = 1,
                        "top" => v = -1,
                        "bottom" => v = 1,
                        _ => return None,
                    }
                }
                dir = match (h, v) {
                    (0, -1) => GradientDir::Angle(0.0),
                    (0, 1) => GradientDir::Angle(180.0),
                    (1, 0) => GradientDir::Angle(90.0),
                    (-1, 0) => GradientDir::Angle(270.0),
                    (0, 0) => return None,
                    _ => GradientDir::Corner(h, v),
                };
                consumed = true;
            }
            let stops = parse_stops(if consumed { &parts[1..] } else { &parts[..] })?;
            Some(Image::Linear { dir, stops, repeating })
        }
        "radial-gradient" => {
            let mut first = Cur::new(parts[0]);
            let mut circle = false;
            let mut size = RadialSize::FarthestCorner;
            let mut center = (Len::Pct(50.0), Len::Pct(50.0));
            let mut consumed = false;
            // Is the first part a shape/size/position prelude rather than a colour stop?
            let mut probe = first.clone();
            if parse_color(&mut probe).is_none() {
                consumed = true;
                loop {
                    first.skip_ws();
                    let Some(tok) = first.peek() else { break };
                    match tok {
                        Tok::Ident(i) => {
                            let l = i.to_ascii_lowercase();
                            first.next();
                            match l.as_str() {
                                "circle" => circle = true,
                                "ellipse" => circle = false,
                                "closest-side" => size = RadialSize::ClosestSide,
                                "farthest-side" => size = RadialSize::FarthestSide,
                                "closest-corner" => size = RadialSize::ClosestCorner,
                                "farthest-corner" => size = RadialSize::FarthestCorner,
                                "at" => {
                                    center = parse_position(&mut first)?;
                                }
                                _ => return None,
                            }
                        }
                        _ => {
                            let a = parse_len(&mut first, false)?;
                            let b = parse_len(&mut first, false);
                            match b {
                                Some(b) => size = RadialSize::Explicit(a, b),
                                None => {
                                    circle = true;
                                    size = RadialSize::Explicit(a.clone(), a);
                                }
                            }
                        }
                    }
                }
            }
            let stops = parse_stops(if consumed { &parts[1..] } else { &parts[..] })?;
            Some(Image::Radial { circle, size, center, stops, repeating })
        }
        _ => None,
    }
}

/// Parse an `<image>`: `none`, `url(...)`, or a gradient. `resolve` absolutizes URLs.
pub fn parse_image(c: &mut Cur, resolve: &dyn Fn(&str) -> Option<String>) -> Option<Image> {
    let save = c.i;
    c.skip_ws();
    let r = match c.next() {
        Some(Tok::Ident(i)) if i.eq_ignore_ascii_case("none") => Some(Image::None),
        Some(Tok::Url(u)) => Some(match resolve(u) {
            Some(abs) => Image::Url(abs.into()),
            None => Image::None,
        }),
        Some(Tok::Function(f)) => {
            let mut args = c.block();
            if f.eq_ignore_ascii_case("url") {
                args.skip_ws();
                match args.next() {
                    Some(Tok::Str(u)) => Some(match resolve(u) {
                        Some(abs) => Image::Url(abs.into()),
                        None => Image::None,
                    }),
                    _ => None,
                }
            } else if f.eq_ignore_ascii_case("image-set") || f.eq_ignore_ascii_case("-webkit-image-set") {
                // take the first candidate
                let parts = split_commas(args.rest());
                let mut first = Cur::new(parts[0]);
                first.skip_ws();
                match first.peek() {
                    Some(Tok::Str(u)) => Some(match resolve(u) {
                        Some(abs) => Image::Url(abs.into()),
                        None => Image::None,
                    }),
                    _ => parse_image(&mut first, resolve),
                }
            } else {
                parse_gradient(f, &mut args)
            }
        }
        _ => None,
    };
    if r.is_none() {
        c.i = save;
    }
    r
}

// ───────────────────────────── shadows ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct ShadowSpec {
    pub x: Len,
    pub y: Len,
    pub blur: Len,
    pub spread: Len,
    pub color: Option<Color>,
    pub inset: bool,
}

pub fn parse_shadows(t: &[Tok]) -> Option<Vec<ShadowSpec>> {
    let mut out = Vec::new();
    for part in split_commas(t) {
        let mut c = Cur::new(part);
        let mut lens: Vec<Len> = Vec::new();
        let mut color = None;
        let mut inset = false;
        loop {
            if c.at_end() {
                break;
            }
            if let Some(l) = parse_len(&mut c, false) {
                lens.push(l);
                continue;
            }
            if let Some(col) = parse_color(&mut c) {
                color = match col {
                    CssColor::Rgba(c) => Some(c),
                    CssColor::Current => None,
                };
                continue;
            }
            match c.ident() {
                Some(i) if i.eq_ignore_ascii_case("inset") => inset = true,
                _ => return None,
            }
        }
        if lens.len() < 2 || lens.len() > 4 {
            return None;
        }
        let mut it = lens.into_iter();
        out.push(ShadowSpec {
            x: it.next().unwrap(),
            y: it.next().unwrap(),
            blur: it.next().unwrap_or(Len::ZERO),
            spread: it.next().unwrap_or(Len::ZERO),
            color,
            inset,
        });
    }
    Some(out)
}

// ───────────────────────────── grid tracks ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum TrackBreadth {
    Len(Len),
    Fr(f32),
    Auto,
    MinContent,
    MaxContent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub min: TrackBreadth,
    pub max: TrackBreadth,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TrackItem {
    One(Track),
    Repeat(u32, Vec<Track>),
    /// repeat(auto-fill | auto-fit, ...)
    AutoRepeat { fit: bool, tracks: Vec<Track> },
    /// `[a b]`: names of the grid line at this place in the list (no track of its own).
    Names(Vec<Atom>),
}

fn parse_breadth(c: &mut Cur) -> Option<TrackBreadth> {
    c.skip_ws();
    match c.peek()? {
        Tok::Dimension(v, u) if u == "fr" => {
            let v = *v;
            c.next();
            Some(TrackBreadth::Fr(v))
        }
        Tok::Ident(i) => {
            let r = match i.to_ascii_lowercase().as_str() {
                "auto" => TrackBreadth::Auto,
                "min-content" => TrackBreadth::MinContent,
                "max-content" => TrackBreadth::MaxContent,
                _ => return None,
            };
            c.next();
            Some(r)
        }
        _ => parse_len(c, false).map(TrackBreadth::Len),
    }
}

fn parse_track(c: &mut Cur) -> Option<Track> {
    c.skip_ws();
    match c.peek()? {
        Tok::Function(f) if f.eq_ignore_ascii_case("minmax") => {
            c.next();
            let mut inner = c.block();
            let min = parse_breadth(&mut inner)?;
            if !inner.comma() {
                return None;
            }
            let max = parse_breadth(&mut inner)?;
            Some(Track { min, max })
        }
        Tok::Function(f) if f.eq_ignore_ascii_case("fit-content") => {
            c.next();
            let mut inner = c.block();
            let l = parse_len(&mut inner, false)?;
            Some(Track { min: TrackBreadth::Auto, max: TrackBreadth::Len(l) })
        }
        _ => {
            let b = parse_breadth(c)?;
            Some(match b {
                TrackBreadth::Fr(_) => Track { min: TrackBreadth::Auto, max: b },
                _ => Track { min: b.clone(), max: b },
            })
        }
    }
}

fn skip_line_names(c: &mut Cur) {
    loop {
        c.skip_ws();
        if matches!(c.peek(), Some(Tok::LBracket)) {
            while let Some(t) = c.next() {
                if matches!(t, Tok::RBracket) {
                    break;
                }
            }
        } else {
            break;
        }
    }
}

/// Line names in brackets at the cursor (`[a b] [c]`), if there are any.
fn take_line_names(c: &mut Cur) -> Vec<Atom> {
    let mut names = Vec::new();
    loop {
        c.skip_ws();
        if !matches!(c.peek(), Some(Tok::LBracket)) {
            return names;
        }
        while let Some(t) = c.next() {
            match t {
                Tok::RBracket => break,
                Tok::Ident(i) if names.len() < 64 => names.push(Atom::new(i)),
                _ => {}
            }
        }
    }
}

pub fn parse_track_list(t: &[Tok]) -> Option<Vec<TrackItem>> {
    let mut c = Cur::new(t);
    let mut out = Vec::new();
    loop {
        let names = take_line_names(&mut c);
        if !names.is_empty() {
            out.push(TrackItem::Names(names));
        }
        if c.at_end() {
            break;
        }
        match c.peek()? {
            Tok::Function(f) if f.eq_ignore_ascii_case("repeat") => {
                c.next();
                let mut inner = c.block();
                inner.skip_ws();
                let count = match inner.next()? {
                    Tok::Number(n) if *n >= 1.0 => Some(*n as u32),
                    Tok::Ident(i) if i.eq_ignore_ascii_case("auto-fill") => None,
                    Tok::Ident(i) if i.eq_ignore_ascii_case("auto-fit") => None,
                    _ => return None,
                };
                let fit = matches!(&inner.t[inner.i - 1], Tok::Ident(i) if i.eq_ignore_ascii_case("auto-fit"));
                if !inner.comma() {
                    return None;
                }
                let mut tracks = Vec::new();
                loop {
                    skip_line_names(&mut inner);
                    if inner.at_end() {
                        break;
                    }
                    tracks.push(parse_track(&mut inner)?);
                }
                if tracks.is_empty() {
                    return None;
                }
                out.push(match count {
                    Some(n) => TrackItem::Repeat(n.min(1000), tracks),
                    None => TrackItem::AutoRepeat { fit, tracks },
                });
            }
            _ => out.push(TrackItem::One(parse_track(&mut c)?)),
        }
    }
    // names alone are not a track list
    if out.iter().all(|i| matches!(i, TrackItem::Names(_))) {
        None
    } else {
        Some(out)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GridLine {
    Auto,
    Line(i32),
    Span(u32),
    /// The `n`-th line with this name in the container's template (from the end if negative).
    Named(Atom, i32),
}

pub fn parse_grid_line(c: &mut Cur) -> Option<GridLine> {
    c.skip_ws();
    let mut span = false;
    let mut num: Option<i32> = None;
    let mut name: Option<Atom> = None;
    let mut any = false;
    loop {
        c.skip_ws();
        match c.peek() {
            Some(Tok::Ident(i)) if i.eq_ignore_ascii_case("auto") && !any => {
                c.next();
                return Some(GridLine::Auto);
            }
            Some(Tok::Ident(i)) if i.eq_ignore_ascii_case("span") => {
                c.next();
                span = true;
            }
            Some(Tok::Number(n)) => {
                num = Some(*n as i32);
                c.next();
            }
            Some(Tok::Ident(i)) => {
                name = Some(Atom::new(i));
                c.next();
            }
            _ => break,
        }
        any = true;
    }
    if !any {
        return None;
    }
    Some(match (span, num, name) {
        // (a span counted in named lines is taken as a span in plain lines)
        (true, n, _) => GridLine::Span(n.unwrap_or(1).max(1) as u32),
        (false, n, Some(name)) => GridLine::Named(name, n.filter(|n| *n != 0).unwrap_or(1)),
        (false, Some(n), None) if n != 0 => GridLine::Line(n),
        _ => GridLine::Auto,
    })
}

/// `grid-template-areas`: each named area's rectangle in the grid.
#[derive(Clone, Debug, PartialEq)]
pub struct GridAreas {
    pub rows: usize,
    pub cols: usize,
    /// (name, row, column, row span, column span)
    pub areas: Vec<(Atom, usize, usize, usize, usize)>,
}

impl GridAreas {
    pub fn find(&self, name: Atom) -> Option<(usize, usize, usize, usize)> {
        self.areas.iter().find(|a| a.0 == name).map(|a| (a.1, a.2, a.3, a.4))
    }
}

/// Parse the row strings of `grid-template-areas`.
pub fn parse_grid_areas(rows: &[&str]) -> Option<GridAreas> {
    let mut cols = 0usize;
    // name -> (min row, min col, max row, max col)
    let mut found: Vec<(Atom, usize, usize, usize, usize)> = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let cells: Vec<&str> = row.split_whitespace().collect();
        if cells.is_empty() || cells.len() > 64 {
            return None;
        }
        if r == 0 {
            cols = cells.len();
        } else if cells.len() != cols {
            return None;
        }
        for (c, cell) in cells.iter().enumerate() {
            if cell.chars().all(|ch| ch == '.') {
                continue;
            }
            let name = Atom::new(cell);
            match found.iter_mut().find(|f| f.0 == name) {
                Some(f) => {
                    f.1 = f.1.min(r);
                    f.2 = f.2.min(c);
                    f.3 = f.3.max(r);
                    f.4 = f.4.max(c);
                }
                None => found.push((name, r, c, r, c)),
            }
        }
    }
    if rows.is_empty() || rows.len() > 256 {
        return None;
    }
    Some(GridAreas {
        rows: rows.len(),
        cols,
        areas: found.into_iter().map(|(n, r0, c0, r1, c1)| (n, r0, c0, r1 - r0 + 1, c1 - c0 + 1)).collect(),
    })
}

// ───────────────────────────── misc ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum ContentItem {
    Str(Rc<str>),
    Attr(Atom),
    OpenQuote,
    CloseQuote,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BgSize {
    Auto,
    Cover,
    Contain,
    Explicit(Option<Len>, Option<Len>),
}

#[cfg(test)]
mod tests {
    use super::super::tokenizer::tokenize_value;
    use super::*;

    fn color(s: &str) -> Option<CssColor> {
        let t = tokenize_value(s);
        parse_color(&mut Cur::new(&t))
    }

    #[test]
    fn colors() {
        assert_eq!(color("#fff"), Some(CssColor::Rgba(Color::WHITE)));
        assert_eq!(color("rgba(59, 130, 246, 0.5)"), Some(CssColor::Rgba(Color::rgba(59, 130, 246, 128))));
        assert_eq!(color("rgb(59 130 246 / 50%)"), Some(CssColor::Rgba(Color::rgba(59, 130, 246, 128))));
        assert_eq!(color("hsl(0, 100%, 50%)"), Some(CssColor::Rgba(Color::rgb(255, 0, 0))));
        assert_eq!(color("RebeccaPurple"), Some(CssColor::Rgba(Color::hex(0x663399))));
        assert_eq!(color("currentColor"), Some(CssColor::Current));
        assert_eq!(color("notacolor"), None);
    }

    #[test]
    fn lab_and_lch_colors() {
        let near = |s: &str, want: (u8, u8, u8), tol: i32| {
            let Some(CssColor::Rgba(c)) = color(s) else { panic!("{s} did not parse") };
            let d = |a: u8, b: u8| (a as i32 - b as i32).abs();
            assert!(d(c.r, want.0) <= tol && d(c.g, want.1) <= tol && d(c.b, want.2) <= tol && c.a == 255, "{s}: got {c:?}, want {want:?}");
        };
        near("lab(100% 0 0)", (255, 255, 255), 1);
        near("lab(0 0 0)", (0, 0, 0), 1);
        // reference values from the CSS Color specification (D50 white)
        near("lab(54.2917% 80.8125 69.8851)", (255, 0, 0), 1);
        near("lch(54.2917% 106.839 40.853)", (255, 0, 0), 1);
        near("lab(46.2775% -47.5621 48.5837)", (0, 128, 0), 1);
        // pairs a real stylesheet ships side by side: the hex is a gamut-mapped fallback for a
        // colour slightly outside sRGB, so the clipped conversion is only close to it
        near("lab(31.4218% 6.04931 -51.333)", (0x00, 0x4b, 0x97), 6);
        near("lab(46.3624% 5.18757 -61.7046)", (0x02, 0x6f, 0xd7), 6);
        assert_eq!(color("lab(50% 0 0 / 50%)").map(|c| matches!(c, CssColor::Rgba(c) if c.a == 128)), Some(true));
    }

    #[test]
    fn calc_resolution() {
        let t = tokenize_value("calc(100% - 2 * 10px)");
        let l = parse_len(&mut Cur::new(&t), false).unwrap();
        let ctx = LenCtx { font_size: 16.0, root_font_size: 16.0, vw: 1000.0, vh: 800.0 };
        assert_eq!(l.to_size(&ctx).resolve(500.0), Some(480.0));
        let t = tokenize_value("clamp(1rem, 2vw, 3rem)");
        let l = parse_len(&mut Cur::new(&t), false).unwrap();
        assert_eq!(l.to_size(&ctx), Size::Px(20.0));
        let t = tokenize_value("min(100%, 600px)");
        let l = parse_len(&mut Cur::new(&t), false).unwrap();
        assert_eq!(l.to_size(&ctx).resolve(400.0), Some(400.0));
        assert_eq!(l.to_size(&ctx).resolve(900.0), Some(600.0));
    }

    #[test]
    fn gradients() {
        let t = tokenize_value("linear-gradient(135deg, #2563eb, #1e40af)");
        let img = parse_image(&mut Cur::new(&t), &|u| Some(u.to_string())).unwrap();
        match img {
            Image::Linear { dir, stops, .. } => {
                assert_eq!(dir, GradientDir::Angle(135.0));
                assert_eq!(stops.len(), 2);
            }
            _ => panic!(),
        }
        let t = tokenize_value("radial-gradient(circle, rgba(30, 64, 175, 0.15) 0%, transparent 70%)");
        let img = parse_image(&mut Cur::new(&t), &|u| Some(u.to_string())).unwrap();
        assert!(matches!(img, Image::Radial { circle: true, .. }));
    }

    #[test]
    fn tracks() {
        let t = tokenize_value("repeat(auto-fill, minmax(200px, 1fr))");
        let tl = parse_track_list(&t).unwrap();
        assert!(matches!(tl[0], TrackItem::AutoRepeat { fit: false, .. }));
        let t = tokenize_value("1fr 2fr 100px");
        assert_eq!(parse_track_list(&t).unwrap().len(), 3);
    }
}
