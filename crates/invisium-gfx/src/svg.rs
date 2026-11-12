//! From-scratch SVG rasteriser for icons: shapes, paths, strokes, groups and transforms.
//! Element and attribute names are matched ASCII-case-insensitively.

mod color;
mod path;
mod raster;
mod xml;

use crate::draw::Rgba;
use path::{Matrix, IDENTITY};
use raster::{Cap, Join};
use std::borrow::Cow;

/// Largest canvas we allocate (4 bytes per pixel: 32 MiB).
const MAX_PIXELS: u64 = 8 * 1024 * 1024;
const MAX_ELEMENTS: usize = 20_000;
const MAX_DEPTH: usize = 256;
const MAX_POINTS: usize = 200_000;
const TOLERANCE: f32 = 0.2;

type Attrs<'a> = [(&'a str, Cow<'a, str>)];

fn attr<'a>(attrs: &'a Attrs, name: &str) -> Option<&'a str> {
    attrs.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_ref())
}

fn length(s: &str) -> Option<f32> {
    let s = s.trim();
    if s.ends_with('%') {
        return None;
    }
    const UNITS: [(&str, f32); 8] =
        [("px", 1.0), ("pt", 4.0 / 3.0), ("pc", 16.0), ("mm", 96.0 / 25.4), ("cm", 96.0 / 2.54), ("in", 96.0), ("em", 16.0), ("ex", 8.0)];
    let (mut num, mut k) = (s, 1.0);
    if s.len() > 2 && s.is_char_boundary(s.len() - 2) {
        let (n, unit) = s.split_at(s.len() - 2);
        if let Some(&(_, f)) = UNITS.iter().find(|(u, _)| unit.eq_ignore_ascii_case(u)) {
            num = n;
            k = f;
        }
    }
    let v: f32 = num.trim().parse().ok()?;
    (v.is_finite()).then_some(v * k)
}

fn numbers(s: &str) -> Vec<f32> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map_while(|p| p.parse::<f32>().ok().filter(|v| v.is_finite()))
        .collect()
}

fn view_box(attrs: &Attrs) -> Option<[f32; 4]> {
    let v = numbers(attr(attrs, "viewBox")?);
    (v.len() == 4 && v[2] > 0.0 && v[3] > 0.0).then(|| [v[0], v[1], v[2], v[3]])
}

fn parse_transform(s: &str) -> Option<Matrix> {
    let mut m = IDENTITY;
    let mut rest = s.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    while !rest.is_empty() {
        let open = rest.find('(')?;
        let close = rest.find(')')?;
        if close < open {
            return None;
        }
        let name = rest[..open].trim().to_ascii_lowercase();
        let a = numbers(&rest[open + 1..close]);
        let n = match (name.as_str(), a.len()) {
            ("matrix", 6) => [a[0], a[1], a[2], a[3], a[4], a[5]],
            ("translate", 1) => [1.0, 0.0, 0.0, 1.0, a[0], 0.0],
            ("translate", 2) => [1.0, 0.0, 0.0, 1.0, a[0], a[1]],
            ("scale", 1) => [a[0], 0.0, 0.0, a[0], 0.0, 0.0],
            ("scale", 2) => [a[0], 0.0, 0.0, a[1], 0.0, 0.0],
            ("rotate", 1) | ("rotate", 3) => {
                let (s, c) = a[0].to_radians().sin_cos();
                let r = [c, s, -s, c, 0.0, 0.0];
                if a.len() == 3 {
                    let t = [1.0, 0.0, 0.0, 1.0, a[1], a[2]];
                    let tb = [1.0, 0.0, 0.0, 1.0, -a[1], -a[2]];
                    path::mul(&path::mul(&t, &r), &tb)
                } else {
                    r
                }
            }
            ("skewx", 1) => [1.0, 0.0, a[0].to_radians().tan(), 1.0, 0.0, 0.0],
            ("skewy", 1) => [1.0, a[0].to_radians().tan(), 0.0, 1.0, 0.0, 0.0],
            _ => return None,
        };
        m = path::mul(&m, &n);
        rest = rest[close + 1..].trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    }
    m.iter().all(|v| v.is_finite()).then_some(m)
}

#[derive(Clone, Copy)]
enum Paint {
    None,
    Color(Rgba),
    Current,
}

fn parse_paint(v: &str) -> Option<Paint> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("none") {
        return Some(Paint::None);
    }
    if v.eq_ignore_ascii_case("currentcolor") {
        return Some(Paint::Current);
    }
    if v.len() >= 4 && v[..4].eq_ignore_ascii_case("url(") {
        // Paint servers are out of scope: use the fallback colour if one is given.
        let fallback = v.find(')').map(|i| v[i + 1..].trim()).unwrap_or("");
        return Some(if fallback.is_empty() { Paint::None } else { parse_paint(fallback)? });
    }
    color::parse(v).map(Paint::Color)
}

#[derive(Clone, Copy)]
struct Style {
    fill: Paint,
    stroke: Paint,
    stroke_width: f32,
    cap: Cap,
    join: Join,
    miter_limit: f32,
    even_odd: bool,
    fill_opacity: f32,
    stroke_opacity: f32,
    opacity: f32,
    color: Rgba,
    transform: Matrix,
    hidden: bool,
}

fn opacity_value(v: &str) -> Option<f32> {
    let v = v.trim();
    let x = match v.strip_suffix('%') {
        Some(p) => p.trim().parse::<f32>().ok()? / 100.0,
        None => v.parse::<f32>().ok()?,
    };
    x.is_finite().then(|| x.clamp(0.0, 1.0))
}

impl Style {
    fn root(current: Rgba) -> Style {
        Style {
            fill: Paint::Color(Rgba::BLACK),
            stroke: Paint::None,
            stroke_width: 1.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter_limit: 4.0,
            even_odd: false,
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            opacity: 1.0,
            color: current,
            transform: IDENTITY,
            hidden: false,
        }
    }

    fn set(&mut self, name: &str, value: &str) {
        let v = value.trim().trim_end_matches("!important").trim();
        if v.eq_ignore_ascii_case("inherit") {
            return;
        }
        match name.trim().to_ascii_lowercase().as_str() {
            "fill" => self.fill = parse_paint(v).unwrap_or(self.fill),
            "stroke" => self.stroke = parse_paint(v).unwrap_or(self.stroke),
            "color" => self.color = color::parse(v).unwrap_or(self.color),
            "stroke-width" => {
                if let Some(w) = length(v).filter(|w| *w >= 0.0) {
                    self.stroke_width = w;
                }
            }
            "stroke-linecap" => match v.to_ascii_lowercase().as_str() {
                "butt" => self.cap = Cap::Butt,
                "round" => self.cap = Cap::Round,
                "square" => self.cap = Cap::Square,
                _ => {}
            },
            "stroke-linejoin" => match v.to_ascii_lowercase().as_str() {
                "miter" | "miter-clip" | "arcs" => self.join = Join::Miter,
                "round" => self.join = Join::Round,
                "bevel" => self.join = Join::Bevel,
                _ => {}
            },
            "stroke-miterlimit" => {
                if let Some(m) = v.parse::<f32>().ok().filter(|m| m.is_finite() && *m >= 1.0) {
                    self.miter_limit = m;
                }
            }
            "fill-rule" => match v.to_ascii_lowercase().as_str() {
                "evenodd" => self.even_odd = true,
                "nonzero" => self.even_odd = false,
                _ => {}
            },
            "fill-opacity" => self.fill_opacity = opacity_value(v).unwrap_or(self.fill_opacity),
            "stroke-opacity" => self.stroke_opacity = opacity_value(v).unwrap_or(self.stroke_opacity),
            "opacity" => self.opacity *= opacity_value(v).unwrap_or(1.0),
            "display" => self.hidden |= v.eq_ignore_ascii_case("none"),
            "visibility" => self.hidden |= v.eq_ignore_ascii_case("hidden") || v.eq_ignore_ascii_case("collapse"),
            _ => {}
        }
    }

    /// Inherit from `self` and apply an element's presentation attributes, `style` and
    /// `transform`.
    fn child(&self, attrs: &Attrs) -> Style {
        let mut s = *self;
        for (n, v) in attrs {
            if !n.eq_ignore_ascii_case("style") && !n.eq_ignore_ascii_case("transform") {
                s.set(n, v);
            }
        }
        if let Some(css) = attr(attrs, "style") {
            for decl in css.split(';') {
                if let Some((n, v)) = decl.split_once(':') {
                    s.set(n, v);
                }
            }
        }
        if let Some(m) = attr(attrs, "transform").and_then(parse_transform) {
            s.transform = path::mul(&s.transform, &m);
        }
        s
    }

    fn resolve(&self, p: Paint, alpha: f32) -> Option<[f32; 4]> {
        let c = match p {
            Paint::None => return None,
            Paint::Color(c) => c,
            Paint::Current => self.color,
        };
        let a = c.a as f32 / 255.0 * alpha * self.opacity;
        (a > 0.0).then(|| [c.r as f32 / 255.0 * a, c.g as f32 / 255.0 * a, c.b as f32 / 255.0 * a, a])
    }
}

/// Intrinsic size from width/height/viewBox, if any.
pub fn intrinsic_size(source: &str) -> Option<(f32, f32)> {
    let Some(xml::Event::Start { name, attrs, .. }) = xml::Reader::new(source).next() else { return None };
    if !name.eq_ignore_ascii_case("svg") {
        return None;
    }
    let w = attr(&attrs, "width").and_then(length).filter(|v| *v > 0.0);
    let h = attr(&attrs, "height").and_then(length).filter(|v| *v > 0.0);
    let vb = view_box(&attrs);
    match (w, h, vb) {
        (Some(w), Some(h), _) => Some((w, h)),
        (Some(w), None, Some(v)) => Some((w, w * v[3] / v[2])),
        (None, Some(h), Some(v)) => Some((h * v[2] / v[3], h)),
        (None, None, Some(v)) => Some((v[2], v[3])),
        _ => None,
    }
}

fn root_transform(attrs: &Attrs, w: f32, h: f32) -> Matrix {
    if let Some([vx, vy, vw, vh]) = view_box(attrs) {
        let (sx, sy) = (w / vw, h / vh);
        let par = attr(attrs, "preserveAspectRatio").unwrap_or("").trim().to_ascii_lowercase();
        if par.starts_with("none") {
            return [sx, 0.0, 0.0, sy, -vx * sx, -vy * sy];
        }
        let s = if par.contains("slice") { sx.max(sy) } else { sx.min(sy) };
        let align = |key_min: &str, key_max: &str, free: f32| {
            if par.contains(key_min) {
                0.0
            } else if par.contains(key_max) {
                free
            } else {
                free * 0.5
            }
        };
        let tx = align("xmin", "xmax", w - vw * s) - vx * s;
        let ty = align("ymin", "ymax", h - vh * s) - vy * s;
        return [s, 0.0, 0.0, s, tx, ty];
    }
    let iw = attr(attrs, "width").and_then(length).filter(|v| *v > 0.0);
    let ih = attr(attrs, "height").and_then(length).filter(|v| *v > 0.0);
    let sx = iw.map(|v| w / v);
    let sy = ih.map(|v| h / v);
    let (sx, sy) = match (sx, sy) {
        (Some(x), Some(y)) => (x, y),
        (Some(x), None) => (x, x),
        (None, Some(y)) => (y, y),
        (None, None) => (1.0, 1.0),
    };
    [sx, 0.0, 0.0, sy, 0.0, 0.0]
}

/// Rasterise an SVG document to straight RGBA8 at `width x height` physical pixels.
/// `current_color` resolves `currentColor`.
pub fn rasterize(source: &str, width: u32, height: u32, current_color: Rgba) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || width as u64 * height as u64 > MAX_PIXELS {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    let mut canvas = vec![0u8; w * h * 4];
    render(source, &mut canvas, w, h, current_color)?;
    for px in canvas.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Some(canvas)
}

mod render;
use render::render;
