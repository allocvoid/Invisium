//! The 2D canvas: `CanvasRenderingContext2D` as a state machine plus a software rasteriser
//! that draws into an RGBA bitmap.
//!
//! Pure CPU code. The script bindings (the engine core) call it through one generic entry
//! point; they parse CSS colours and fonts before calling, so nothing here reads CSS text
//! except the canvas-specific keywords (`"round"`, `"evenodd"`, `"source-over"` ...).
//! The public items in this file are the contract; see `docs/agents/CANVAS.md`.

pub use invisium_draw::{FontId, FontMetrics, Glyph, GlyphBitmap};

mod budget;
mod composite;
mod ctx;
mod draw;
mod geom;
mod image;
mod paint;
mod path;
mod raster;
mod shadow;
mod state;
mod stroke;
mod svgpath;
mod tables;
mod text;

/// What the canvas needs from the engine's font system.
pub trait Fonts {
    /// Metrics of `font` at `size` pixels per em.
    fn metrics(&mut self, font: FontId, size: f32) -> FontMetrics;
    /// Append the glyphs of `text` to `out`; returns the total advance in pixels.
    fn shape(&mut self, font: FontId, size: f32, text: &str, out: &mut Vec<Glyph>) -> f32;
    /// Coverage bitmap of one glyph at `px_size` pixels per em; `subpx` is the horizontal pen
    /// offset in quarter pixels (0..=3).
    fn rasterize(&mut self, face: u16, glyph: u16, px_size: f32, subpx: u8) -> Option<GlyphBitmap>;
}

/// Objects scripts hold besides the context itself.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Gradient,
    Pattern,
    Path,
}

/// One argument of a call, already taken out of the script engine.
#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    /// `undefined`, or an argument that was not passed.
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(&'a str),
    /// A plain array of numbers (`setLineDash`).
    Nums(&'a [f64]),
    /// A CSS colour the binding has parsed: straight-alpha RGBA.
    Color([u8; 4]),
    /// A `font` value the binding has parsed and resolved. `css` is the text the `font` getter
    /// gives back.
    Font { font: FontId, size: f32, css: &'a str },
    /// A gradient, pattern or path by handle.
    Object(Kind, u32),
    /// Pixels of an image, another canvas, a video frame or an `ImageData`: straight-alpha
    /// RGBA8, top row first.
    Pixels { width: u32, height: u32, rgba: &'a [u8] },
}

/// The numbers of a `TextMetrics` object, in CSS pixels.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct TextMetrics {
    pub width: f64,
    pub actual_left: f64,
    pub actual_right: f64,
    pub font_ascent: f64,
    pub font_descent: f64,
    pub actual_ascent: f64,
    pub actual_descent: f64,
    pub em_ascent: f64,
    pub em_descent: f64,
    pub hanging_baseline: f64,
    pub alphabetic_baseline: f64,
    pub ideographic_baseline: f64,
}

/// The result of a call.
#[derive(Clone, Debug, PartialEq)]
pub enum Ret {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    /// A plain array of numbers.
    Nums(Vec<f64>),
    /// A gradient, pattern or path. The same handle must map to the same script object.
    Object(Kind, u32),
    /// An `ImageData`: straight-alpha RGBA8, top row first.
    Pixels { width: u32, height: u32, rgba: Vec<u8> },
    Text(TextMetrics),
    /// A `DOMMatrix` `[a, b, c, d, e, f]`.
    Matrix([f64; 6]),
    /// Throw: the name of a `DOMException` (`"IndexSizeError"`, `"SyntaxError"`,
    /// `"InvalidStateError"`, ...) or `"TypeError"`, and a message.
    Throw(&'static str, String),
}

/// One 2D context and the bitmap it draws into.
pub struct Canvas2d {
    size: (u32, u32),
    pub(crate) bmp: composite::Bitmap,
    pub(crate) dirty: bool,
    pub(crate) st: state::State,
    pub(crate) stack: Vec<state::State>,
    /// Bytes the saved states hold outside their structs (see `budget::state_extra`).
    pub(crate) stack_extra: usize,
    pub(crate) path: path::Path,
    pub(crate) objs: state::Objects,
    /// Bytes held by shared data: clip masks, gradients and patterns (see `budget::Charge`).
    pub(crate) held: std::rc::Rc<std::cell::Cell<usize>>,
    /// Points the last stroke outline generated (see [`Canvas2d::last_stroke_points`]).
    pub(crate) stroke_pts: std::cell::Cell<usize>,
    /// Dash-piece points the last stroke generated.
    pub(crate) dash_pts: std::cell::Cell<usize>,
    /// Working bytes the last fill, stroke or clip held (see [`Canvas2d::last_fill_bytes`]).
    pub(crate) fill_work: std::cell::Cell<usize>,
}

/// Largest bitmap side and size; larger canvases behave as 0 x 0.
const MAX_SIDE: u32 = 8192;
const MAX_BYTES: u64 = 256 << 20;

/// A transparent bitmap, or a 0 x 0 one when over the limits or over `room` bytes.
fn bitmap(width: u32, height: u32, room: usize) -> composite::Bitmap {
    let bytes = (width as u64).saturating_mul(height as u64).saturating_mul(4);
    let over = width > MAX_SIDE || height > MAX_SIDE || bytes > MAX_BYTES || bytes > room as u64;
    let (w, h) = if over { (0, 0) } else { (width, height) };
    composite::Bitmap { w, h, px: vec![0; w as usize * h as usize * 4] }
}

impl Canvas2d {
    /// A transparent bitmap with the context in its default state.
    pub fn new(width: u32, height: u32) -> Canvas2d {
        let mut c = Canvas2d {
            size: (0, 0),
            bmp: composite::Bitmap { w: 0, h: 0, px: Vec::new() },
            dirty: false,
            st: state::State::default(),
            stack: Vec::new(),
            stack_extra: 0,
            path: path::Path::new(),
            objs: state::Objects::default(),
            held: Default::default(),
            stroke_pts: Default::default(),
            dash_pts: Default::default(),
            fill_work: Default::default(),
        };
        c.bmp = bitmap(width, height, c.room());
        c.size = (c.bmp.w, c.bmp.h);
        c
    }
    /// The canvas's `width` / `height` were set: a new transparent bitmap and the default
    /// state (as the specification says, even if the size did not change). A bitmap that
    /// would not fit the memory budget leaves the canvas 0 x 0.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.bmp = composite::Bitmap { w: 0, h: 0, px: Vec::new() };
        self.st = state::State::default();
        self.stack = Vec::new();
        self.stack_extra = 0;
        self.path.clear();
        self.bmp = bitmap(width, height, self.room());
        self.size = (self.bmp.w, self.bmp.h);
        self.dirty = true;
    }
    /// The bitmap's size; (0, 0) when the requested size was over the limits.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
    /// Run one method, or read or write one property; `op` comes from [`methods`],
    /// [`properties`] or [`object_methods`]. Never panics, whatever it is given.
    pub fn call(&mut self, fonts: &mut dyn Fonts, op: u16, args: &[Arg]) -> Ret {
        self.dispatch(fonts, op, args)
    }
    /// The bitmap as straight-alpha RGBA8, top row first, appended to `out` (after clearing it).
    pub fn read_rgba(&self, out: &mut Vec<u8>) {
        out.clear();
        out.extend_from_slice(&self.bmp.px);
    }
    /// True if the bitmap changed since the last call of this method.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }
    /// Bytes this context holds: exactly what its memory budget (512 MiB) counts.
    pub fn memory_bytes(&self) -> usize {
        self.used_bytes()
    }
    /// Points the last `stroke`, `strokeRect` or `isPointInStroke` generated for its outline
    /// (bounded by the stroker's cap; exposed so tests can check that bound).
    #[doc(hidden)]
    pub fn last_stroke_points(&self) -> usize {
        self.stroke_pts.get()
    }
    /// Dash-piece points the last stroke generated (bounded like the outline; for tests).
    #[doc(hidden)]
    pub fn last_dash_points(&self) -> usize {
        self.dash_pts.get()
    }
    /// Working bytes the last `fill`, `stroke` or `clip` held at the end of the call: the
    /// polygons' point capacity plus the rasteriser's edges, active list and band (for tests).
    #[doc(hidden)]
    pub fn last_fill_bytes(&self) -> usize {
        self.fill_work.get()
    }
    /// The script dropped its last reference to an object handed out earlier (API addition:
    /// without it, handles would accumulate until the object cap). Gives its bytes back.
    pub fn release(&mut self, kind: Kind, handle: u32) {
        self.objs.release(kind, handle);
    }
}

/// Every method of `CanvasRenderingContext2D`, with its op.
pub fn methods() -> &'static [(&'static str, u16)] {
    tables::methods()
}

/// Every property of `CanvasRenderingContext2D`: name, the op that reads it (no arguments) and
/// the op that writes it (one argument).
pub fn properties() -> &'static [(&'static str, u16, u16)] {
    tables::properties()
}

/// Methods of the objects a context hands out (`CanvasGradient.addColorStop`,
/// `CanvasPattern.setTransform`, the path methods of `Path2D`). Each op takes the object as its
/// first argument, except `Path2D` (the constructor), which takes the constructor's argument.
pub fn object_methods(kind: Kind) -> &'static [(&'static str, u16)] {
    tables::object_methods(kind)
}
