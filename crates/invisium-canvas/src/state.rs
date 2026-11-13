//! The drawing state, the objects scripts hold, argument conversion and the properties.

use crate::budget::{grow_bytes, map_bytes, path_obj_bytes, Charge, ClipMask};
use crate::composite::Op;
use crate::geom::M;
use crate::paint::{Gradient, Img};
use crate::path::Path;
use crate::stroke::{Cap, Join};
use crate::tables::PROP_NAMES;
use crate::{Arg, FontId, Kind, Ret};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub struct Pattern {
    pub img: Img<'static>,
    pub rx: bool,
    pub ry: bool,
    pub m: M,
    /// The bytes this pattern holds, charged to its context while it lives.
    pub _charge: Charge,
}

#[derive(Clone)]
pub enum Style {
    Color([u8; 4]),
    Grad(u32, Rc<RefCell<Gradient>>),
    Pat(u32, Rc<RefCell<Pattern>>),
}

#[derive(Clone)]
pub struct State {
    pub m: M,
    pub clip: Option<Rc<ClipMask>>,
    pub fill: Style,
    pub stroke: Style,
    pub alpha: f64,
    pub op: Op,
    pub smoothing: bool,
    pub smoothing_q: &'static str,
    pub line_width: f64,
    pub cap: Cap,
    pub join: Join,
    pub miter: f64,
    pub dash: Vec<f64>,
    pub dash_off: f64,
    pub shadow_color: [u8; 4],
    pub shadow_blur: f64,
    pub sox: f64,
    pub soy: f64,
    pub font: FontId,
    pub font_size: f32,
    pub font_css: String,
    pub align: &'static str,
    pub baseline: &'static str,
    pub direction: &'static str,
    pub letter_spacing: f64,
    pub word_spacing: f64,
    pub filter: String,
    pub kerning: &'static str,
    pub stretch: &'static str,
    pub caps: &'static str,
    pub rendering: &'static str,
    pub lang: String,
}

impl Default for State {
    fn default() -> State {
        State {
            m: M::ID,
            clip: None,
            fill: Style::Color([0, 0, 0, 255]),
            stroke: Style::Color([0, 0, 0, 255]),
            alpha: 1.0,
            op: Op::SrcOver,
            smoothing: true,
            smoothing_q: "low",
            line_width: 1.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter: 10.0,
            dash: Vec::new(),
            dash_off: 0.0,
            shadow_color: [0, 0, 0, 0],
            shadow_blur: 0.0,
            sox: 0.0,
            soy: 0.0,
            font: 0,
            font_size: 10.0,
            font_css: "10px sans-serif".into(),
            align: "start",
            baseline: "alphabetic",
            direction: "inherit",
            letter_spacing: 0.0,
            word_spacing: 0.0,
            filter: "none".into(),
            kerning: "auto",
            stretch: "normal",
            caps: "normal",
            rendering: "auto",
            lang: "inherit".into(),
        }
    }
}

/// Most objects (gradients, patterns, paths) alive at once.
pub const MAX_OBJECTS: usize = 1 << 20;

/// The objects. Gradients and patterns carry their own charge (they may outlive their handle
/// inside a style); `Path2D` objects live only here, with a running byte total.
#[derive(Default)]
pub struct Objects {
    pub next: u32,
    pub grads: HashMap<u32, Rc<RefCell<Gradient>>>,
    pub pats: HashMap<u32, Rc<RefCell<Pattern>>>,
    pub paths: HashMap<u32, Path>,
    /// Bytes of all `Path2D` objects (see `budget::path_obj_bytes`).
    pub path_bytes: usize,
}

impl Objects {
    pub fn count(&self) -> usize {
        self.grads.len() + self.pats.len() + self.paths.len()
    }
    pub fn full(&self) -> bool {
        self.count() >= MAX_OBJECTS
    }
    pub fn handle(&mut self) -> u32 {
        self.next = self.next.wrapping_add(1);
        while self.next == 0
            || self.grads.contains_key(&self.next)
            || self.pats.contains_key(&self.next)
            || self.paths.contains_key(&self.next)
        {
            self.next = self.next.wrapping_add(1);
        }
        self.next
    }
    /// Bytes the three handle maps hold.
    pub fn map_bytes(&self) -> usize {
        map_bytes(&self.grads) + map_bytes(&self.pats) + map_bytes(&self.paths)
    }
    /// Extra bytes inserting one more object of `kind` may allocate.
    pub fn grow_bytes(&self, kind: Kind) -> usize {
        match kind {
            Kind::Gradient => grow_bytes(&self.grads),
            Kind::Pattern => grow_bytes(&self.pats),
            Kind::Path => grow_bytes(&self.paths),
        }
    }
    /// Give back map capacity once a map is mostly empty.
    fn shrink(&mut self) {
        fn fit<V>(m: &mut HashMap<u32, V>) {
            if m.capacity() > 64 && m.len() * 4 < m.capacity() {
                m.shrink_to(m.len() * 2);
            }
        }
        fit(&mut self.grads);
        fit(&mut self.pats);
        fit(&mut self.paths);
    }
    pub fn insert_path(&mut self, h: u32, p: Path) {
        self.path_bytes += path_obj_bytes(&p);
        self.paths.insert(h, p);
    }
    /// Drop the handle. Gradients and patterns give their bytes back when the last style
    /// holding them lets go.
    pub fn release(&mut self, kind: Kind, h: u32) {
        match kind {
            Kind::Gradient => {
                self.grads.remove(&h);
            }
            Kind::Pattern => {
                self.pats.remove(&h);
            }
            Kind::Path => {
                if let Some(p) = self.paths.remove(&h) {
                    self.path_bytes = self.path_bytes.saturating_sub(path_obj_bytes(&p));
                }
            }
        }
        self.shrink();
    }
}

/// ECMAScript-like ToNumber of an argument (absent = `undefined` = NaN).
pub fn num(a: Option<&Arg>) -> f64 {
    match a {
        Some(Arg::Num(v)) => *v,
        Some(Arg::Bool(b)) => *b as u8 as f64,
        Some(Arg::Null) => 0.0,
        Some(Arg::Str(s)) => {
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else {
                t.parse().unwrap_or(f64::NAN)
            }
        }
        _ => f64::NAN,
    }
}

/// ECMAScript-like ToString for text arguments.
pub fn text_of(a: Option<&Arg>) -> String {
    match a {
        Some(Arg::Str(s)) => (*s).to_string(),
        Some(Arg::Num(v)) => fmt_num(*v),
        Some(Arg::Bool(b)) => b.to_string(),
        Some(Arg::Null) => "null".into(),
        Some(Arg::Undefined) | None => "undefined".into(),
        Some(Arg::Nums(v)) => v.iter().take(65_536).map(|x| fmt_num(*x)).collect::<Vec<_>>().join(","),
        Some(_) => "[object Object]".into(),
    }
}

pub fn fmt_num(v: f64) -> String {
    if v.is_nan() {
        "NaN".into()
    } else if v.is_infinite() {
        if v > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else if v == v.trunc() && v.abs() < 1e21 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

pub fn fmt_color(c: [u8; 4]) -> String {
    if c[3] == 255 {
        return format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    }
    let a = if c[3] == 0 {
        "0".to_string()
    } else {
        let mut s = String::new();
        for d in 1..=6 {
            let k = 10f64.powi(d);
            let v = (c[3] as f64 / 255.0 * k).round() / k;
            if (v * 255.0).round() as u8 == c[3] {
                s = fmt_num(v);
                break;
            }
        }
        s
    };
    format!("rgba({}, {}, {}, {})", c[0], c[1], c[2], a)
}

pub fn keyword(a: Option<&Arg>, list: &[&'static str]) -> Option<&'static str> {
    match a {
        Some(Arg::Str(s)) => list.iter().copied().find(|k| *k == *s),
        _ => None,
    }
}

fn spacing(a: Option<&Arg>, font_size: f32) -> Option<f64> {
    let Some(Arg::Str(s)) = a else { return None };
    let s = s.trim().to_ascii_lowercase();
    let (n, k) = if let Some(n) = s.strip_suffix("px") {
        (n, 1.0)
    } else if let Some(n) = s.strip_suffix("em") {
        (n, font_size as f64)
    } else if s.parse::<f64>() == Ok(0.0) {
        (s.as_str(), 1.0)
    } else {
        return None;
    };
    let v: f64 = n.trim().parse().ok()?;
    if v.is_finite() && (v * k).is_finite() {
        Some(v * k)
    } else {
        None
    }
}

const ALIGN: [&str; 5] = ["start", "end", "left", "right", "center"];
const BASELINE: [&str; 6] = ["top", "hanging", "middle", "alphabetic", "ideographic", "bottom"];
const DIRECTION: [&str; 3] = ["ltr", "rtl", "inherit"];
const QUALITY: [&str; 3] = ["low", "medium", "high"];
const KERNING: [&str; 3] = ["auto", "normal", "none"];
const STRETCH: [&str; 9] = [
    "ultra-condensed",
    "extra-condensed",
    "condensed",
    "semi-condensed",
    "normal",
    "semi-expanded",
    "expanded",
    "extra-expanded",
    "ultra-expanded",
];
const CAPS: [&str; 7] =
    ["normal", "small-caps", "all-small-caps", "petite-caps", "all-petite-caps", "unicase", "titling-caps"];
const RENDERING: [&str; 4] = ["auto", "optimizeSpeed", "optimizeLegibility", "geometricPrecision"];

fn style_ret(s: &Style) -> Ret {
    match s {
        Style::Color(c) => Ret::Str(fmt_color(*c)),
        Style::Grad(h, _) => Ret::Object(Kind::Gradient, *h),
        Style::Pat(h, _) => Ret::Object(Kind::Pattern, *h),
    }
}

fn style_of(a: Option<&Arg>, objs: &Objects) -> Option<Style> {
    match a {
        Some(Arg::Color(c)) => Some(Style::Color(*c)),
        Some(Arg::Object(Kind::Gradient, h)) => objs.grads.get(h).map(|g| Style::Grad(*h, g.clone())),
        Some(Arg::Object(Kind::Pattern, h)) => objs.pats.get(h).map(|p| Style::Pat(*h, p.clone())),
        _ => None,
    }
}

impl State {
    pub fn get_prop(&self, i: usize) -> Ret {
        let n = |v: f64| Ret::Num(v);
        let s = |v: &str| Ret::Str(v.to_string());
        match PROP_NAMES.get(i).copied().unwrap_or("") {
            "globalAlpha" => n(self.alpha),
            "globalCompositeOperation" => s(self.op.name()),
            "imageSmoothingEnabled" => Ret::Bool(self.smoothing),
            "imageSmoothingQuality" => s(self.smoothing_q),
            "strokeStyle" => style_ret(&self.stroke),
            "fillStyle" => style_ret(&self.fill),
            "shadowOffsetX" => n(self.sox),
            "shadowOffsetY" => n(self.soy),
            "shadowBlur" => n(self.shadow_blur),
            "shadowColor" => Ret::Str(fmt_color(self.shadow_color)),
            "filter" => s(&self.filter),
            "lineWidth" => n(self.line_width),
            "lineCap" => s(match self.cap {
                Cap::Butt => "butt",
                Cap::Round => "round",
                Cap::Square => "square",
            }),
            "lineJoin" => s(match self.join {
                Join::Miter => "miter",
                Join::Round => "round",
                Join::Bevel => "bevel",
            }),
            "miterLimit" => n(self.miter),
            "lineDashOffset" => n(self.dash_off),
            "font" => s(&self.font_css),
            "textAlign" => s(self.align),
            "textBaseline" => s(self.baseline),
            "direction" => s(self.direction),
            "letterSpacing" => Ret::Str(format!("{}px", fmt_num(self.letter_spacing))),
            "fontKerning" => s(self.kerning),
            "fontStretch" => s(self.stretch),
            "fontVariantCaps" => s(self.caps),
            "textRendering" => s(self.rendering),
            "wordSpacing" => Ret::Str(format!("{}px", fmt_num(self.word_spacing))),
            "lang" => s(&self.lang),
            _ => Ret::Undefined,
        }
    }

    /// Write property `i`; invalid values leave it unchanged.
    pub fn set_prop(&mut self, i: usize, a: Option<&Arg>, objs: &Objects) {
        let v = num(a);
        let fin = v.is_finite();
        match PROP_NAMES.get(i).copied().unwrap_or("") {
            "globalAlpha" if fin && (0.0..=1.0).contains(&v) => self.alpha = v,
            "globalCompositeOperation" => {
                if let Some(Arg::Str(s)) = a {
                    if let Some(op) = Op::parse(s) {
                        self.op = op;
                    }
                }
            }
            "imageSmoothingEnabled" => {
                self.smoothing = match a {
                    Some(Arg::Str(s)) => !s.is_empty(),
                    Some(Arg::Object(..)) | Some(Arg::Nums(_)) | Some(Arg::Pixels { .. }) => true,
                    Some(Arg::Color(_)) | Some(Arg::Font { .. }) => true,
                    _ => v != 0.0 && !v.is_nan(),
                }
            }
            "imageSmoothingQuality" => {
                if let Some(k) = keyword(a, &QUALITY) {
                    self.smoothing_q = k;
                }
            }
            "strokeStyle" => {
                if let Some(st) = style_of(a, objs) {
                    self.stroke = st;
                }
            }
            "fillStyle" => {
                if let Some(st) = style_of(a, objs) {
                    self.fill = st;
                }
            }
            "shadowOffsetX" if fin => self.sox = v,
            "shadowOffsetY" if fin => self.soy = v,
            "shadowBlur" if fin && v >= 0.0 => self.shadow_blur = v,
            "shadowColor" => {
                if let Some(Arg::Color(c)) = a {
                    self.shadow_color = *c;
                }
            }
            "filter" => {
                if let Some(Arg::Str(s)) = a {
                    self.filter = s.chars().take(4096).collect();
                }
            }
            "lineWidth" if fin && v > 0.0 => self.line_width = v,
            "lineCap" => {
                self.cap = match keyword(a, &["butt", "round", "square"]) {
                    Some("butt") => Cap::Butt,
                    Some("round") => Cap::Round,
                    Some("square") => Cap::Square,
                    _ => self.cap,
                }
            }
            "lineJoin" => {
                self.join = match keyword(a, &["miter", "round", "bevel"]) {
                    Some("miter") => Join::Miter,
                    Some("round") => Join::Round,
                    Some("bevel") => Join::Bevel,
                    _ => self.join,
                }
            }
            "miterLimit" if fin && v > 0.0 => self.miter = v,
            "lineDashOffset" if fin => self.dash_off = v,
            "font" => {
                if let Some(Arg::Font { font, size, css }) = a {
                    if size.is_finite() && *size >= 0.0 {
                        self.font = *font;
                        self.font_size = *size;
                        self.font_css = css.chars().take(4096).collect();
                    }
                }
            }
            "textAlign" => self.align = keyword(a, &ALIGN).unwrap_or(self.align),
            "textBaseline" => self.baseline = keyword(a, &BASELINE).unwrap_or(self.baseline),
            "direction" => self.direction = keyword(a, &DIRECTION).unwrap_or(self.direction),
            "letterSpacing" => self.letter_spacing = spacing(a, self.font_size).unwrap_or(self.letter_spacing),
            "fontKerning" => self.kerning = keyword(a, &KERNING).unwrap_or(self.kerning),
            "fontStretch" => self.stretch = keyword(a, &STRETCH).unwrap_or(self.stretch),
            "fontVariantCaps" => self.caps = keyword(a, &CAPS).unwrap_or(self.caps),
            "textRendering" => self.rendering = keyword(a, &RENDERING).unwrap_or(self.rendering),
            "wordSpacing" => self.word_spacing = spacing(a, self.font_size).unwrap_or(self.word_spacing),
            "lang" => {
                if let Some(Arg::Str(s)) = a {
                    self.lang = s.chars().take(256).collect();
                }
            }
            _ => {}
        }
    }
}
