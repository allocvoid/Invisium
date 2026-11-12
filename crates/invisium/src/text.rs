//! Fonts and text measurement.
//!
//! Privacy: the engine never enumerates installed fonts. Pages can only ever reach a fixed
//! allowlist of fonts that ship with the operating system (plus fonts bundled with the browser),
//! so "which fonts are installed" is not observable.
//!
//! Memory: font files are memory-mapped (file-backed pages, not private RAM) and loaded lazily,
//! the first time a face is actually needed.

use crate::atom::Atom;
use ab_glyph::{Font, FontRef, GlyphId, PxScale, VariableFont};
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

struct Face {
    font: FontRef<'static>,
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
    stack_by_key: HashMap<(u64, u16, bool), FontId>,
    fallback_cache: HashMap<(char, bool), Option<(FaceId, u16, f32)>>,
    fonts_dir: std::path::PathBuf,
    mapped_bytes: usize,
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
        let upem = font.units_per_em().unwrap_or(1000.0);
        let ascent = font.ascent_unscaled();
        let descent = -font.descent_unscaled();
        let line_gap = font.line_gap_unscaled();
        let height = font.height_unscaled();
        // x-height estimated from the 'x' glyph bounds
        let x_height = {
            let gid = font.glyph_id('x');
            let g = gid.with_scale(PxScale::from(height));
            font.outline_glyph(g).map(|o| -o.px_bounds().min.y).unwrap_or(upem * 0.5)
        };
        if self.faces.len() >= u16::MAX as usize {
            return None;
        }
        self.faces.push(Face { font, upem, ascent, descent, line_gap, x_height, height_ratio: height / upem });
        Some((self.faces.len() - 1) as FaceId)
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

    /// Resolve a CSS font-family list to a font.
    pub fn resolve(&mut self, families: &Rc<[Atom]>, weight: u16, italic: bool) -> FontId {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for f in families.iter() {
            f.0.hash(&mut h);
        }
        let key = (h.finish(), weight, italic);
        if let Some(&id) = self.stack_by_key.get(&key) {
            return id;
        }
        let mut primary: Option<FaceId> = None;
        for f in families.iter() {
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
        self.stacks.push(Stack { primary, bold: weight >= 600, cache: HashMap::new() });
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
            let f = &self.faces[primary as usize];
            // non-breaking and exotic spaces render as a plain space
            let lookup = match ch {
                '\u{a0}' | '\u{2007}' | '\u{202f}' => ' ',
                c => c,
            };
            let gid = f.font.glyph_id(lookup);
            if gid.0 != 0 {
                GlyphInfo { face: primary, id: gid.0, adv: f.font.h_advance_unscaled(gid) / f.upem }
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
        self.resolve_name(family, weight, italic)
    }
    fn metrics(&mut self, font: FontId, size: f32) -> FontMetrics {
        FontSystem::metrics(self, font, size)
    }
    fn shape(&mut self, font: FontId, size: f32, text: &str, out: &mut Vec<LGlyph>) -> f32 {
        FontSystem::shape(self, font, size, text, 0.0, out)
    }
}
