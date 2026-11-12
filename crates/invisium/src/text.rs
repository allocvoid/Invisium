//! Fonts and text measurement.
//!
//! Privacy: the engine never enumerates installed fonts. Pages can only ever reach a fixed
//! allowlist of fonts that ship with the operating system (plus fonts bundled with the browser),
//! so "which fonts are installed" is not observable.
//!
//! Memory: font files are memory-mapped (file-backed pages, not private RAM) and loaded lazily,
//! the first time a face is actually needed.

use crate::atom::Atom;
use ab_glyph::{Font, FontRef, FontVec, Glyph, GlyphId, OutlinedGlyph, PxScale, VariableFont};
use std::collections::HashMap;
use std::rc::Rc;

pub type FaceId = u16;
pub use invisium_draw::{FontId, FontMetrics};

/// One shaped glyph in the layout tree (8 bytes). The same type the renderer consumes, so the
/// layout tree's glyph buffer is handed to the GPU layer without copying.
pub use invisium_draw::Glyph as LGlyph;
pub use invisium_draw::GlyphBitmap as RasterGlyph;

#[derive(Clone, Copy, Debug)]
pub struct GlyphInfo {
    pub face: FaceId,
    pub id: u16,
    /// Advance as a fraction of the em.
    pub adv: f32,
}

/// A face's font data: borrowed for the process lifetime (system and bundled fonts), owned (web
/// fonts, freed with their page), or a tombstone left behind when a web font was dropped.
enum FontKind {
    Borrowed(FontRef<'static>),
    Owned(FontVec),
    Dropped,
}

impl FontKind {
    fn glyph_id(&self, ch: char) -> GlyphId {
        match self {
            FontKind::Borrowed(f) => f.glyph_id(ch),
            FontKind::Owned(f) => f.glyph_id(ch),
            FontKind::Dropped => GlyphId(0),
        }
    }

    fn h_advance_unscaled(&self, g: GlyphId) -> f32 {
        match self {
            FontKind::Borrowed(f) => f.h_advance_unscaled(g),
            FontKind::Owned(f) => f.h_advance_unscaled(g),
            FontKind::Dropped => 0.0,
        }
    }

    fn outline_glyph(&self, g: Glyph) -> Option<OutlinedGlyph> {
        match self {
            FontKind::Borrowed(f) => f.outline_glyph(g),
            FontKind::Owned(f) => f.outline_glyph(g),
            FontKind::Dropped => None,
        }
    }

    fn data(&self) -> &[u8] {
        match self {
            FontKind::Owned(f) => f.as_slice(),
            _ => &[],
        }
    }
}

/// (upem, ascent, descent, line gap, x-height, height) of a font.
fn measure_font<F: Font>(font: &F) -> (f32, f32, f32, f32, f32, f32) {
    let upem = font.units_per_em().unwrap_or(1000.0);
    let height = font.height_unscaled();
    // x-height estimated from the 'x' glyph bounds
    let x_height = {
        let g = font.glyph_id('x').with_scale(PxScale::from(height));
        font.outline_glyph(g).map(|o| -o.px_bounds().min.y).unwrap_or(upem * 0.5)
    };
    (upem, font.ascent_unscaled(), -font.descent_unscaled(), font.line_gap_unscaled(), x_height, height)
}

const MAX_WEB_INPUT: usize = 16 << 20;
const MAX_WEB_DECODED: usize = 64 << 20;
const MAX_WEB_TOTAL: usize = 256 << 20;
const MAX_WEB_FACES_PER_SCOPE: usize = 64;

/// One `@font-face` registered by a page.
struct WebFont {
    scope: u32,
    /// Lower-case CSS family name.
    family: String,
    weight: (u16, u16),
    italic: bool,
    ranges: Rc<[(u32, u32)]>,
    variable: bool,
    /// Face holding the data; for a variable font also the instance at its weight in `instances`.
    base: FaceId,
    /// Variable fonts: (quantised weight, face) instances.
    instances: Vec<(u16, FaceId)>,
}

fn in_ranges(ranges: &[(u32, u32)], ch: char) -> bool {
    let c = ch as u32;
    ranges.is_empty() || ranges.iter().any(|&(a, b)| a <= c && c <= b)
}

struct Face {
    font: FontKind,
    /// Bytes of owned (web font) data this face holds.
    bytes: usize,
    upem: f32,
    ascent: f32,
    descent: f32,
    line_gap: f32,
    x_height: f32,
    /// ab_glyph scales by ascent-descent height; this converts an em size to that scale.
    height_ratio: f32,
}

#[derive(Clone, Copy)]
struct Files {
    regular: &'static str,
    bold: &'static str,
    italic: &'static str,
    bold_italic: &'static str,
    light: &'static str,
    semibold: &'static str,
}

const fn fam(regular: &'static str, bold: &'static str, italic: &'static str, bold_italic: &'static str) -> Files {
    Files { regular, bold, italic, bold_italic, light: "", semibold: "" }
}

/// Fonts that ship with every supported Windows release. This list is the complete set of
/// system fonts the engine will ever open.
static SYSTEM_FAMILIES: &[(&str, Files)] = &[
    (
        "segoe ui",
        Files {
            regular: "segoeui.ttf",
            bold: "segoeuib.ttf",
            italic: "segoeuii.ttf",
            bold_italic: "segoeuiz.ttf",
            light: "segoeuil.ttf",
            semibold: "seguisb.ttf",
        },
    ),
    ("arial", fam("arial.ttf", "arialbd.ttf", "ariali.ttf", "arialbi.ttf")),
    ("times new roman", fam("times.ttf", "timesbd.ttf", "timesi.ttf", "timesbi.ttf")),
    ("georgia", fam("georgia.ttf", "georgiab.ttf", "georgiai.ttf", "georgiaz.ttf")),
    ("verdana", fam("verdana.ttf", "verdanab.ttf", "verdanai.ttf", "verdanaz.ttf")),
    ("tahoma", fam("tahoma.ttf", "tahomabd.ttf", "tahoma.ttf", "tahomabd.ttf")),
    ("trebuchet ms", fam("trebuc.ttf", "trebucbd.ttf", "trebucit.ttf", "trebucbi.ttf")),
    ("courier new", fam("cour.ttf", "courbd.ttf", "couri.ttf", "courbi.ttf")),
    ("consolas", fam("consola.ttf", "consolab.ttf", "consolai.ttf", "consolaz.ttf")),
    ("comic sans ms", fam("comic.ttf", "comicbd.ttf", "comici.ttf", "comicz.ttf")),
    ("impact", fam("impact.ttf", "impact.ttf", "impact.ttf", "impact.ttf")),
    ("calibri", fam("calibri.ttf", "calibrib.ttf", "calibrii.ttf", "calibriz.ttf")),
    ("segoe ui symbol", fam("seguisym.ttf", "seguisym.ttf", "seguisym.ttf", "seguisym.ttf")),
    ("segoe ui emoji", fam("seguiemj.ttf", "seguiemj.ttf", "seguiemj.ttf", "seguiemj.ttf")),
    ("segoe mdl2 assets", fam("segmdl2.ttf", "segmdl2.ttf", "segmdl2.ttf", "segmdl2.ttf")),
    ("microsoft yahei", fam("msyh.ttc", "msyhbd.ttc", "msyh.ttc", "msyhbd.ttc")),
    ("yu gothic", fam("YuGothR.ttc", "YuGothB.ttc", "YuGothR.ttc", "YuGothB.ttc")),
    ("malgun gothic", fam("malgun.ttf", "malgunbd.ttf", "malgun.ttf", "malgunbd.ttf")),
    ("nirmala ui", fam("Nirmala.ttf", "NirmalaB.ttf", "Nirmala.ttf", "NirmalaB.ttf")),
    ("leelawadee ui", fam("LeelawUI.ttf", "LeelaUIb.ttf", "LeelawUI.ttf", "LeelaUIb.ttf")),
    ("microsoft sans serif", fam("micross.ttf", "micross.ttf", "micross.ttf", "micross.ttf")),
    ("ebrima", fam("ebrima.ttf", "ebrimabd.ttf", "ebrima.ttf", "ebrimabd.ttf")),
    ("gadugi", fam("gadugi.ttf", "gadugib.ttf", "gadugi.ttf", "gadugib.ttf")),
    ("segoe ui historic", fam("seguihis.ttf", "seguihis.ttf", "seguihis.ttf", "seguihis.ttf")),
];

/// CSS family names (lower-case) mapped to an available family.
fn alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "sans-serif" | "helvetica" | "helvetica neue" | "arial" | "liberation sans" | "nimbus sans" => "arial",
        "serif" | "times" | "times new roman" | "liberation serif" | "nimbus roman" => "times new roman",
        "monospace" | "ui-monospace" | "consolas" => "consolas",
        "courier" | "courier new" | "liberation mono" => "courier new",
        "system-ui" | "ui-sans-serif" | "segoe ui" | "segoe ui variable" | "segoe ui variable text"
        | "segoe ui variable display" | "-apple-system-fallback" => "segoe ui",
        "ui-serif" | "georgia" => "georgia",
        "verdana" => "verdana",
        "tahoma" | "ui-rounded" => "tahoma",
        "trebuchet ms" | "trebuchet" => "trebuchet ms",
        "cursive" | "comic sans ms" | "comic sans" => "comic sans ms",
        "fantasy" | "impact" => "impact",
        "calibri" => "calibri",
        "segoe ui symbol" => "segoe ui symbol",
        "segoe ui emoji" | "emoji" | "apple color emoji" | "noto color emoji" => "segoe ui emoji",
        "segoe mdl2 assets" => "segoe mdl2 assets",
        "microsoft yahei" | "microsoft yahei ui" => "microsoft yahei",
        "yu gothic" | "yu gothic ui" | "meiryo" => "yu gothic",
        "malgun gothic" => "malgun gothic",
        _ => return None,
    })
}

/// Tried in order when the chosen font has no glyph for a character.
static FALLBACKS: &[&str] = &[
    "segoe ui",
    "segoe ui symbol",
    "segoe ui emoji",
    "microsoft yahei",
    "malgun gothic",
    "yu gothic",
    "nirmala ui",
    "leelawadee ui",
    "microsoft sans serif",
    "ebrima",
    "gadugi",
    "segoe ui historic",
];

struct Stack {
    primary: FaceId,
    bold: bool,
    cache: HashMap<char, GlyphInfo>,
    /// Scope the stack was resolved in (0 unless that scope had web fonts).
    scope: u32,
    /// Web faces of the chosen family with their `unicode-range`s, in registration order.
    web: Vec<(FaceId, Rc<[(u32, u32)]>)>,
}

/// A font bundled with the application (variable fonts get one face per weight on demand).
struct Bundled {
    data: &'static [u8],
    variable: bool,
}

pub struct FontSystem {
    faces: Vec<Face>,
    /// (file or bundled key, weight variation) -> face
    face_by_key: HashMap<(String, u16), Option<FaceId>>,
    file_data: HashMap<String, Option<&'static [u8]>>,
    bundled: HashMap<Atom, Bundled>,
    stacks: Vec<Stack>,
    /// (scope, family list hash, weight, italic) -> stack
    stack_by_key: HashMap<(u32, u64, u16, bool), FontId>,
    fallback_cache: HashMap<(char, bool), Option<(FaceId, u16, f32)>>,
    fonts_dir: std::path::PathBuf,
    mapped_bytes: usize,
    /// Page whose web fonts `resolve` sees (0 = none).
    scope: u32,
    web: Vec<WebFont>,
    web_count: HashMap<u32, usize>,
    web_bytes: usize,
}

fn zero_width(ch: char) -> bool {
    matches!(
        ch,
        '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{feff}' | '\u{ad}'
            | '\u{fe00}'..='\u{fe0f}' | '\u{0}'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{7f}'..='\u{9f}'
    )
}

impl Default for FontSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl FontSystem {
    pub fn new() -> FontSystem {
        let windir = std::env::var("WINDIR").or_else(|_| std::env::var("SystemRoot")).unwrap_or_else(|_| "C:\\Windows".into());
        FontSystem {
            faces: Vec::new(),
            face_by_key: HashMap::new(),
            file_data: HashMap::new(),
            bundled: HashMap::new(),
            stacks: Vec::new(),
            stack_by_key: HashMap::new(),
            fallback_cache: HashMap::new(),
            fonts_dir: std::path::Path::new(&windir).join("Fonts"),
            mapped_bytes: 0,
            scope: 0,
            web: Vec::new(),
            web_count: HashMap::new(),
            web_bytes: 0,
        }
    }

    /// Register a font shipped with the application under a CSS family name. Accepts TTF/OTF or
    /// WOFF2 data. Returns false if the data cannot be decoded.
    pub fn register_bundled(&mut self, family: &str, data: &[u8]) -> bool {
        let ttf: Vec<u8> = if data.starts_with(b"wOF2") {
            let mut input: &[u8] = data;
            match woff2_patched::convert_woff2_to_ttf(&mut input) {
                Ok(v) => v,
                Err(_) => return false,
            }
        } else {
            data.to_vec()
        };
        let leaked: &'static [u8] = Box::leak(ttf.into_boxed_slice());
        let Ok(font) = FontRef::try_from_slice(leaked) else { return false };
        let variable = font.variations().iter().any(|v| &v.tag == b"wght");
        self.bundled.insert(Atom::new(&family.to_ascii_lowercase()), Bundled { data: leaked, variable });
        self.stack_by_key.clear();
        true
    }

    fn file(&mut self, name: &str) -> Option<&'static [u8]> {
        if let Some(d) = self.file_data.get(name) {
            return *d;
        }
        let path = self.fonts_dir.join(name);
        let data: Option<&'static [u8]> = std::fs::File::open(&path).ok().and_then(|f| {
            // SAFETY: system font files are not modified while the browser runs; the mapping is
            // read-only and lives for the rest of the process.
            let map = unsafe { memmap2::Mmap::map(&f).ok()? };
            let leaked: &'static memmap2::Mmap = Box::leak(Box::new(map));
            Some(&leaked[..])
        });
        if let Some(d) = data {
            self.mapped_bytes += d.len();
        }
        self.file_data.insert(name.to_string(), data);
        data
    }

    fn add_face(&mut self, data: &'static [u8], wght: Option<f32>) -> Option<FaceId> {
        let mut font = FontRef::try_from_slice_and_index(data, 0).ok()?;
        if let Some(w) = wght {
            font.set_variation(b"wght", w);
        }
        let m = measure_font(&font);
        self.push_face(FontKind::Borrowed(font), 0, m)
    }

    fn push_face(&mut self, font: FontKind, bytes: usize, m: (f32, f32, f32, f32, f32, f32)) -> Option<FaceId> {
        let (upem, ascent, descent, line_gap, x_height, height) = m;
        if self.faces.len() >= u16::MAX as usize {
            return None;
        }
        self.faces.push(Face { font, bytes, upem, ascent, descent, line_gap, x_height, height_ratio: height / upem });
        Some((self.faces.len() - 1) as FaceId)
    }

    /// Add an owned web font face, instanced at `wght` if it is a variable font; counts its
    /// bytes. Returns the face and whether the font has a `wght` axis.
    fn add_owned_face(&mut self, ttf: Vec<u8>, wght: f32) -> Option<(FaceId, bool)> {
        if self.faces.len() >= u16::MAX as usize || self.web_bytes + ttf.len() > MAX_WEB_TOTAL {
            return None;
        }
        let mut font = FontVec::try_from_vec(ttf).ok()?;
        let variable = font.variations().iter().any(|v| &v.tag == b"wght");
        if variable {
            font.set_variation(b"wght", wght);
        }
        let m = measure_font(&font);
        if !(m.0.is_finite() && m.0 > 0.0) {
            return None;
        }
        let bytes = font.as_slice().len();
        let id = self.push_face(FontKind::Owned(font), bytes, m)?;
        self.web_bytes += bytes;
        Some((id, variable))
    }

    fn system_face(&mut self, file: &str) -> Option<FaceId> {
        if file.is_empty() {
            return None;
        }
        let key = (file.to_string(), 0u16);
        if let Some(f) = self.face_by_key.get(&key) {
            return *f;
        }
        let face = self.file(file).and_then(|d| self.add_face(d, None));
        self.face_by_key.insert(key, face);
        face
    }

    fn family_face(&mut self, family: &str, weight: u16, italic: bool) -> Option<FaceId> {
        let (_, files) = SYSTEM_FAMILIES.iter().find(|(n, _)| *n == family)?;
        let bold = weight >= 650 || (weight >= 550 && files.semibold.is_empty());
        let file = if italic {
            if bold {
                files.bold_italic
            } else {
                files.italic
            }
        } else if bold {
            files.bold
        } else if weight >= 550 && !files.semibold.is_empty() {
            files.semibold
        } else if weight < 350 && !files.light.is_empty() {
            files.light
        } else {
            files.regular
        };
        self.system_face(file).or_else(|| self.system_face(files.regular))
    }

    fn bundled_face(&mut self, family: Atom, weight: u16) -> Option<FaceId> {
        let (data, variable) = {
            let b = self.bundled.get(&family)?;
            (b.data, b.variable)
        };
        let w = if variable { (weight / 50) * 50 } else { 0 };
        let key = (format!("bundled:{}", family), w);
        if let Some(f) = self.face_by_key.get(&key) {
            return *f;
        }
        let face = self.add_face(data, if variable { Some(w as f32) } else { None });
        self.face_by_key.insert(key, face);
        face
    }

    /// Fonts resolved from now on are looked up for this page. 0 = no page (browser UI).
    pub fn set_scope(&mut self, scope: u32) {
        self.scope = scope;
    }

    /// Register a web font for `scope` under a CSS family name (case-insensitive).
    /// `weight` is the inclusive range the face covers (a single weight is (w, w)); `ranges`
    /// are the `unicode-range` code point ranges it is meant for (empty = all). `data` is the
    /// file as downloaded: TrueType/OpenType, or WOFF2. Returns false if it cannot be used.
    pub fn add_web_font(&mut self, scope: u32, family: &str, weight: (u16, u16), italic: bool, ranges: &[(u32, u32)], data: &[u8]) -> bool {
        let family = family.trim().to_ascii_lowercase();
        if scope == 0 || family.is_empty() || data.len() > MAX_WEB_INPUT || data.starts_with(b"wOFF") {
            return false;
        }
        if self.web_count.get(&scope).copied().unwrap_or(0) >= MAX_WEB_FACES_PER_SCOPE {
            return false;
        }
        let ttf: Vec<u8> = if data.starts_with(b"wOF2") {
            // totalSfntSize (header offset 16) bounds the decoded size before decoding
            let total = data.get(16..20).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
            if total.is_none_or(|t| t > MAX_WEB_DECODED) {
                return false;
            }
            let decoded = std::panic::catch_unwind(|| {
                let mut input: &[u8] = data;
                woff2_patched::convert_woff2_to_ttf(&mut input).ok()
            });
            match decoded {
                Ok(Some(v)) => v,
                _ => return false,
            }
        } else {
            data.to_vec()
        };
        if ttf.len() > MAX_WEB_DECODED || self.web_bytes + ttf.len() > MAX_WEB_TOTAL {
            return false;
        }
        let (a, b) = (weight.0.clamp(1, 1000), weight.1.clamp(1, 1000));
        let weight = (a.min(b), a.max(b));
        let q = Self::web_weight(weight, 400);
        let Some((base, variable)) = self.add_owned_face(ttf, q as f32) else { return false };
        let instances = if variable { vec![(q, base)] } else { Vec::new() };
        self.web.push(WebFont { scope, family, weight, italic, ranges: Rc::from(ranges), variable, base, instances });
        *self.web_count.entry(scope).or_insert(0) += 1;
        self.stack_by_key.retain(|k, _| k.0 != scope);
        true
    }

    /// True if `scope` has at least one usable web font for this family name.
    pub fn has_web_family(&self, scope: u32, family: &str) -> bool {
        let family = family.trim();
        scope != 0 && self.web.iter().any(|w| w.scope == scope && w.family.eq_ignore_ascii_case(family))
    }

    /// Forget every web font of `scope` and free its data.
    pub fn drop_scope(&mut self, scope: u32) {
        if scope == 0 {
            return;
        }
        let (gone, kept): (Vec<WebFont>, Vec<WebFont>) = std::mem::take(&mut self.web).into_iter().partition(|w| w.scope == scope);
        self.web = kept;
        for w in gone {
            let ids = std::iter::once(w.base).chain(w.instances.iter().map(|i| i.1));
            for id in ids {
                if let Some(f) = self.faces.get_mut(id as usize) {
                    if matches!(f.font, FontKind::Owned(_)) {
                        self.web_bytes = self.web_bytes.saturating_sub(f.bytes);
                        *f = Face { font: FontKind::Dropped, bytes: 0, upem: 1.0, ascent: 0.0, descent: 0.0, line_gap: 0.0, x_height: 0.0, height_ratio: 0.0 };
                    }
                }
            }
        }
        self.web_count.remove(&scope);
        self.stack_by_key.retain(|k, _| k.0 != scope);
        for s in self.stacks.iter_mut().filter(|s| s.scope == scope) {
            s.web.clear();
            s.cache.clear();
        }
    }

    /// Bytes of web font data currently held (all scopes).
    pub fn web_font_bytes(&self) -> usize {
        self.web_bytes
    }

    /// Requested weight clamped into a face's range, quantised to 50.
    fn web_weight(range: (u16, u16), weight: u16) -> u16 {
        let w = weight.clamp(range.0, range.1);
        ((w / 50) * 50).clamp(range.0, range.1)
    }

    /// Faces of a web family in `scope` for this style, with their ranges; empty if none.
    fn web_faces(&mut self, scope: u32, family: &str, weight: u16, italic: bool) -> Vec<(FaceId, Rc<[(u32, u32)]>)> {
        let all: Vec<usize> = (0..self.web.len()).filter(|&i| self.web[i].scope == scope && self.web[i].family.eq_ignore_ascii_case(family)).collect();
        if all.is_empty() {
            return Vec::new();
        }
        let styled: Vec<usize> = all.iter().copied().filter(|&i| self.web[i].italic == italic).collect();
        let pool = if styled.is_empty() { all } else { styled };
        let dist = |r: (u16, u16)| if weight < r.0 { r.0 - weight } else { weight.saturating_sub(r.1) };
        let best = pool.iter().map(|&i| dist(self.web[i].weight)).min().unwrap_or(0);
        let chosen: Vec<usize> = pool.into_iter().filter(|&i| dist(self.web[i].weight) == best).collect();
        let mut out = Vec::new();
        for i in chosen {
            let face = self.web_instance(i, weight);
            out.push((face, self.web[i].ranges.clone()));
        }
        out
    }

    /// The face of web font `i` to use at `weight` (variable fonts get one instance per weight).
    fn web_instance(&mut self, i: usize, weight: u16) -> FaceId {
        let w = &self.web[i];
        if !w.variable {
            return w.base;
        }
        let q = Self::web_weight(w.weight, weight);
        if let Some(&(_, f)) = w.instances.iter().find(|(iw, _)| *iw == q) {
            return f;
        }
        let base = w.base;
        let data = self.faces[base as usize].font.data().to_vec();
        match self.add_owned_face(data, q as f32) {
            Some((f, _)) => {
                self.web[i].instances.push((q, f));
                f
            }
            None => base,
        }
    }

    /// Resolve a CSS font-family list to a font.
    pub fn resolve(&mut self, families: &Rc<[Atom]>, weight: u16, italic: bool) -> FontId {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for f in families.iter() {
            f.0.hash(&mut h);
        }
        // pages without web fonts share the scope-0 stacks
        let scope = if self.scope != 0 && self.web_count.contains_key(&self.scope) { self.scope } else { 0 };
        let key = (scope, h.finish(), weight, italic);
        if let Some(&id) = self.stack_by_key.get(&key) {
            return id;
        }
        let mut primary: Option<FaceId> = None;
        let mut web = Vec::new();
        for f in families.iter() {
            if scope != 0 {
                web = self.web_faces(scope, f.as_str(), weight, italic);
                if !web.is_empty() {
                    let a = web.iter().find(|(_, r)| in_ranges(r, 'a')).unwrap_or(&web[0]);
                    primary = Some(a.0);
                    break;
                }
            }
            if self.bundled.contains_key(f) {
                primary = self.bundled_face(*f, weight);
            } else if let Some(sys) = alias(f.as_str()) {
                primary = self.family_face(sys, weight, italic);
            }
            if primary.is_some() {
                break;
            }
        }
        let primary = primary
            .or_else(|| self.family_face("times new roman", weight, italic))
            .or_else(|| self.family_face("segoe ui", weight, italic))
            .or_else(|| self.family_face("arial", weight, italic))
            .or_else(|| if self.faces.is_empty() { None } else { Some(0) })
            .expect("no usable system font found");
        self.stacks.push(Stack { primary, bold: weight >= 600, cache: HashMap::new(), scope, web });
        let id = (self.stacks.len() - 1) as FontId;
        self.stack_by_key.insert(key, id);
        id
    }

    /// Convenience: resolve by a single family name.
    pub fn resolve_name(&mut self, family: &str, weight: u16, italic: bool) -> FontId {
        let list: Rc<[Atom]> = Rc::from(vec![Atom::new(&family.to_ascii_lowercase())]);
        self.resolve(&list, weight, italic)
    }

    pub fn metrics(&self, font: FontId, size: f32) -> FontMetrics {
        let f = &self.faces[self.stacks[font as usize].primary as usize];
        let k = size / f.upem;
        FontMetrics { ascent: f.ascent * k, descent: f.descent * k, line_gap: f.line_gap * k, x_height: f.x_height * k }
    }

    fn fallback(&mut self, ch: char, bold: bool) -> Option<(FaceId, u16, f32)> {
        if let Some(r) = self.fallback_cache.get(&(ch, bold)) {
            return *r;
        }
        let mut found = None;
        for fam in FALLBACKS {
            if let Some(face) = self.family_face(fam, if bold { 700 } else { 400 }, false) {
                let f = &self.faces[face as usize];
                let gid = f.font.glyph_id(ch);
                if gid.0 != 0 {
                    found = Some((face, gid.0, f.font.h_advance_unscaled(gid) / f.upem));
                    break;
                }
            }
        }
        self.fallback_cache.insert((ch, bold), found);
        found
    }

    pub fn glyph(&mut self, font: FontId, ch: char) -> GlyphInfo {
        if let Some(g) = self.stacks[font as usize].cache.get(&ch) {
            return *g;
        }
        let (primary, bold) = {
            let s = &self.stacks[font as usize];
            (s.primary, s.bold)
        };
        let info = if zero_width(ch) {
            GlyphInfo { face: primary, id: 0, adv: 0.0 }
        } else {
            // non-breaking and exotic spaces render as a plain space
            let lookup = match ch {
                '\u{a0}' | '\u{2007}' | '\u{202f}' => ' ',
                c => c,
            };
            let own = {
                let web = &self.stacks[font as usize].web;
                // a web stack tries its family's faces whose unicode-range covers the character
                let single = [(primary, Rc::from(&[][..]))];
                let cands = if web.is_empty() { &single[..] } else { &web[..] };
                cands.iter().filter(|(_, r)| in_ranges(r, lookup)).find_map(|&(face, _)| {
                    let f = &self.faces[face as usize];
                    let gid = f.font.glyph_id(lookup);
                    (gid.0 != 0).then(|| GlyphInfo { face, id: gid.0, adv: f.font.h_advance_unscaled(gid) / f.upem })
                })
            };
            if let Some(g) = own {
                g
            } else if let Some((face, id, adv)) = self.fallback(lookup, bold) {
                GlyphInfo { face, id, adv }
            } else if ch.is_whitespace() {
                let f = &self.faces[primary as usize];
                let sp = f.font.glyph_id(' ');
                let adv = f.font.h_advance_unscaled(sp) / f.upem;
                let scale = match ch {
                    '\u{2002}' => 0.5 / adv.max(0.01),
                    '\u{2003}' => 1.0 / adv.max(0.01),
                    '\u{2009}' | '\u{200a}' => 0.6,
                    _ => 1.0,
                };
                GlyphInfo { face: primary, id: 0, adv: adv * scale }
            } else {
                // no glyph anywhere: reserve the width of '?' but draw nothing
                let f = &self.faces[primary as usize];
                let q = f.font.glyph_id('?');
                GlyphInfo { face: primary, id: 0, adv: f.font.h_advance_unscaled(q) / f.upem }
            }
        };
        self.stacks[font as usize].cache.insert(ch, info);
        info
    }

    /// Append the glyphs of `text` to `out`; returns the total advance in pixels.
    pub fn shape(&mut self, font: FontId, size: f32, text: &str, letter_spacing: f32, out: &mut Vec<LGlyph>) -> f32 {
        let mut total = 0.0;
        for ch in text.chars() {
            let g = self.glyph(font, ch);
            let adv = g.adv * size + if g.adv > 0.0 { letter_spacing } else { 0.0 };
            total += adv;
            out.push(LGlyph { face: g.face, id: g.id, adv });
        }
        total
    }

    pub fn measure(&mut self, font: FontId, size: f32, text: &str) -> f32 {
        text.chars().map(|ch| self.glyph(font, ch).adv * size).sum()
    }

    /// Rasterise one glyph at a physical pixel size. `subpx` is the horizontal sub-pixel offset
    /// in quarter pixels (0..=3).
    pub fn rasterize(&self, face: FaceId, glyph: u16, px_size: f32, subpx: u8) -> Option<RasterGlyph> {
        let f = self.faces.get(face as usize)?;
        let scale = PxScale::from(px_size * f.height_ratio);
        let g = GlyphId(glyph).with_scale_and_position(scale, ab_glyph::point(subpx as f32 * 0.25, 0.0));
        let outline = f.font.outline_glyph(g)?;
        let b = outline.px_bounds();
        let w = (b.max.x - b.min.x).ceil().max(0.0) as u32;
        let h = (b.max.y - b.min.y).ceil().max(0.0) as u32;
        if w == 0 || h == 0 || w > 1024 || h > 1024 {
            return None;
        }
        let mut data = vec![0u8; (w * h) as usize];
        outline.draw(|x, y, c| {
            if x < w && y < h {
                data[(y * w + x) as usize] = (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        });
        Some(RasterGlyph { width: w, height: h, left: b.min.x.floor() as i32, top: b.min.y.floor() as i32, data })
    }

    /// (number of loaded faces, bytes of font files mapped)
    pub fn stats(&self) -> (usize, usize) {
        (self.faces.len(), self.mapped_bytes)
    }
}

impl invisium_draw::GlyphSource for FontSystem {
    fn rasterize(&self, face: u16, glyph: u16, px_size: f32, subpx: u8) -> Option<RasterGlyph> {
        FontSystem::rasterize(self, face, glyph, px_size, subpx)
    }
}

impl invisium_draw::TextShaper for FontSystem {
    fn font(&mut self, family: &str, weight: u16, italic: bool) -> FontId {
        // the browser UI never sees a page's web fonts
        let scope = std::mem::replace(&mut self.scope, 0);
        let id = self.resolve_name(family, weight, italic);
        self.scope = scope;
        id
    }
    fn metrics(&mut self, font: FontId, size: f32) -> FontMetrics {
        FontSystem::metrics(self, font, size)
    }
    fn shape(&mut self, font: FontId, size: f32, text: &str, out: &mut Vec<LGlyph>) -> f32 {
        FontSystem::shape(self, font, size, text, 0.0, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use invisium_draw::TextShaper;

    /// A Windows system font read from disk as a stand-in for a downloaded web font.
    fn file(name: &str) -> Option<Vec<u8>> {
        std::fs::read(std::path::Path::new("C:\\Windows\\Fonts").join(name)).ok()
    }

    /// Owned data length of the face that draws `ch` (0 = not a web face).
    fn web_len(fs: &mut FontSystem, font: FontId, ch: char) -> usize {
        let face = fs.glyph(font, ch).face;
        match &fs.faces[face as usize].font {
            FontKind::Owned(f) => f.as_slice().len(),
            _ => 0,
        }
    }

    #[test]
    fn web_family_is_scoped() {
        let Some(arial) = file("arial.ttf") else { return };
        let mut fs = FontSystem::new();
        assert!(fs.add_web_font(1, "MyWeb", (400, 400), false, &[], &arial));
        assert!(fs.has_web_family(1, "myweb") && !fs.has_web_family(2, "myweb") && !fs.has_web_family(0, "myweb"));
        fs.set_scope(1);
        let id = fs.resolve_name("myweb", 400, false);
        assert_eq!(web_len(&mut fs, id, 'a'), arial.len());
        for other in [0, 2] {
            fs.set_scope(other);
            let id = fs.resolve_name("myweb", 400, false);
            assert_eq!(web_len(&mut fs, id, 'a'), 0);
        }
    }

    #[test]
    fn web_family_overrides_system_and_changes_layout() {
        let (Some(times), Some(arial)) = (file("times.ttf"), file("arial.ttf")) else { return };
        let mut fs = FontSystem::new();
        let sys = fs.resolve_name("arial", 400, false);
        let sys_w = fs.measure(sys, 16.0, "Hello web fonts");
        assert!(fs.add_web_font(1, "arial", (400, 400), false, &[], &times));
        fs.set_scope(1);
        let web = fs.resolve_name("arial", 400, false);
        assert_eq!(web_len(&mut fs, web, 'a'), times.len());
        assert!((fs.measure(web, 16.0, "Hello web fonts") - sys_w).abs() > 1.0);
        // a web Arial under another name differs from the default (serif) fallback
        assert!(fs.add_web_font(1, "brand", (400, 400), false, &[], &arial));
        let brand = fs.resolve_name("brand", 400, false);
        fs.set_scope(0);
        let fallback = fs.resolve_name("brand", 400, false);
        assert!((fs.measure(brand, 16.0, "eeeeeeeeee") - fs.measure(fallback, 16.0, "eeeeeeeeee")).abs() > 1.0);
    }

    #[test]
    fn weight_and_italic_selection() {
        let (Some(r), Some(b), Some(i)) = (file("arial.ttf"), file("arialbd.ttf"), file("ariali.ttf")) else { return };
        let mut fs = FontSystem::new();
        assert!(fs.add_web_font(3, "w", (400, 400), false, &[], &r));
        assert!(fs.add_web_font(3, "w", (700, 700), false, &[], &b));
        assert!(fs.add_web_font(3, "w", (400, 400), true, &[], &i));
        fs.set_scope(3);
        let cases = [(400, false, r.len()), (700, false, b.len()), (600, false, b.len()), (300, false, r.len()), (400, true, i.len()), (700, true, i.len())];
        for (w, it, want) in cases {
            let id = fs.resolve_name("w", w, it);
            assert_eq!(web_len(&mut fs, id, 'a'), want, "weight {w} italic {it}");
        }
    }

    #[test]
    fn variable_web_font_instances() {
        let Some(v) = file("bahnschrift.ttf") else { return };
        let mut fs = FontSystem::new();
        assert!(fs.add_web_font(1, "var", (300, 700), false, &[], &v));
        fs.set_scope(1);
        let thin = fs.resolve_name("var", 300, false);
        let bold = fs.resolve_name("var", 700, false);
        let (ft, fb) = (fs.glyph(thin, 'a').face, fs.glyph(bold, 'a').face);
        assert_ne!(ft, fb);
        let heavy = fs.resolve_name("var", 900, false);
        assert_eq!(fs.glyph(heavy, 'a').face, fb);
    }

    #[test]
    fn unicode_range_routing() {
        let (Some(arial), Some(times)) = (file("arial.ttf"), file("times.ttf")) else { return };
        let mut fs = FontSystem::new();
        assert!(fs.add_web_font(1, "mix", (400, 400), false, &[(0x41, 0x5A)], &arial));
        assert!(fs.add_web_font(1, "mix", (400, 400), false, &[], &times));
        fs.set_scope(1);
        let id = fs.resolve_name("mix", 400, false);
        assert_eq!(web_len(&mut fs, id, 'A'), arial.len());
        assert_eq!(web_len(&mut fs, id, 'a'), times.len());
        let primary = fs.stacks[id as usize].primary;
        assert_eq!(fs.faces[primary as usize].bytes, times.len());
    }

    #[test]
    fn drop_scope_frees_data() {
        let Some(arial) = file("arial.ttf") else { return };
        let mut fs = FontSystem::new();
        let before = fs.web_font_bytes();
        assert!(fs.add_web_font(5, "gone", (400, 400), false, &[], &arial));
        assert!(fs.web_font_bytes() >= before + arial.len());
        fs.set_scope(5);
        let id = fs.resolve_name("gone", 400, false);
        let g = fs.glyph(id, 'a');
        fs.drop_scope(5);
        assert_eq!(fs.web_font_bytes(), before);
        assert!(!fs.has_web_family(5, "gone"));
        // stale ids are harmless
        assert!(fs.rasterize(g.face, g.id, 16.0, 0).is_none());
        let _ = fs.metrics(id, 16.0);
        let mut out = Vec::new();
        fs.shape(id, 16.0, "abc", 0.0, &mut out);
        let again = fs.resolve_name("gone", 400, false);
        assert_eq!(web_len(&mut fs, again, 'a'), 0);
    }

    #[test]
    fn bad_web_font_data_is_rejected() {
        let mut fs = FontSystem::new();
        assert!(!fs.add_web_font(1, "x", (400, 400), false, &[], b"not a font at all"));
        let mut woff1 = b"wOFF".to_vec();
        woff1.extend_from_slice(&[0u8; 60]);
        assert!(!fs.add_web_font(1, "x", (400, 400), false, &[], &woff1));
        let mut woff2 = b"wOF2".to_vec();
        woff2.extend_from_slice(&[0xffu8; 60]);
        assert!(!fs.add_web_font(1, "x", (400, 400), false, &[], &woff2));
        assert!(!fs.add_web_font(1, "x", (400, 400), false, &[], &vec![0u8; MAX_WEB_INPUT + 1]));
        assert!(!fs.has_web_family(1, "x"));
        assert_eq!(fs.web_font_bytes(), 0);
        if let Some(arial) = file("arial.ttf") {
            assert!(!fs.add_web_font(0, "x", (400, 400), false, &[], &arial));
            for _ in 0..MAX_WEB_FACES_PER_SCOPE {
                assert!(fs.add_web_font(2, "x", (400, 400), false, &[], &arial));
            }
            assert!(!fs.add_web_font(2, "x", (400, 400), false, &[], &arial));
            fs.drop_scope(2);
            assert_eq!(fs.web_font_bytes(), 0);
        }
    }

    #[test]
    fn ui_path_ignores_web_fonts() {
        let Some(times) = file("times.ttf") else { return };
        let mut fs = FontSystem::new();
        assert!(fs.add_web_font(1, "Segoe UI", (400, 400), false, &[], &times));
        fs.set_scope(1);
        let ui = TextShaper::font(&mut fs, "segoe ui", 400, false);
        assert_eq!(web_len(&mut fs, ui, 'a'), 0);
        assert_eq!(fs.scope, 1);
        let page = fs.resolve_name("segoe ui", 400, false);
        assert_eq!(web_len(&mut fs, page, 'a'), times.len());
    }
}
