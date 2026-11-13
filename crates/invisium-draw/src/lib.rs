//! The display-list contract.
//!
//! Three parties meet here and nowhere else:
//!   * producers  — the engine's painter and the browser chrome build a [`DrawList`];
//!   * the renderer — `invisium-gfx` turns [`Layer`]s into GPU draw calls;
//!   * the font system — implements [`GlyphSource`] (bitmaps for the renderer) and [`TextShaper`]
//!     (measurement for producers).
//!
//! This crate is pure data: no dependencies, no I/O, no GPU types. Changing anything here is a
//! contract change and is done by the project lead only.
//!
//! Units: every coordinate in a `DrawList` is in *list units* (CSS pixels for a page). A
//! [`Layer`] says where the list lands on the surface, in physical pixels:
//! `physical = layer.viewport.origin + (list_pos - layer.scroll) * layer.scale`.

/// The WebGL command contract between the GL state machine and the GPU side.
pub mod gl;
pub mod prof;

/// Axis-aligned rectangle: origin at the top-left, y grows downward.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const ZERO: Rect = Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 };

    #[inline]
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    #[inline]
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    #[inline]
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
    #[inline]
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    #[inline]
    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, w: self.w, h: self.h }
    }
    /// Grow (or, with a negative amount, shrink) by `d` on every side.
    #[inline]
    pub fn inflate(&self, d: f32) -> Rect {
        Rect { x: self.x - d, y: self.y - d, w: (self.w + 2.0 * d).max(0.0), h: (self.h + 2.0 * d).max(0.0) }
    }
    /// Intersection; an empty rectangle if the two do not overlap.
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        Rect { x: x0, y: y0, w: (x1 - x0).max(0.0), h: (y1 - y0).max(0.0) }
    }
    #[inline]
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
}

/// Straight (non-premultiplied) 8-bit sRGB colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
#[repr(C)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba { r: 0, g: 0, b: 0, a: 0 };
    pub const BLACK: Rgba = Rgba { r: 0, g: 0, b: 0, a: 255 };
    pub const WHITE: Rgba = Rgba { r: 255, g: 255, b: 255, a: 255 };

    #[inline]
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Rgba {
        Rgba { r, g, b, a }
    }
    #[inline]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r, g, b, a: 255 }
    }
    /// From `0xRRGGBB`.
    #[inline]
    pub const fn hex(v: u32) -> Rgba {
        Rgba { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8, a: 255 }
    }
    /// Multiply the alpha channel by `a` (0..=1).
    #[inline]
    pub fn with_alpha(self, a: f32) -> Rgba {
        Rgba { a: (self.a as f32 * a.clamp(0.0, 1.0)).round() as u8, ..self }
    }
    #[inline]
    pub fn is_transparent(self) -> bool {
        self.a == 0
    }
}

/// Corner radii in list units: top-left, top-right, bottom-right, bottom-left.
pub type Radii = [f32; 4];
pub const NO_RADII: Radii = [0.0; 4];

/// One positioned glyph of a text run. `face` and `id` are opaque to the renderer: it passes
/// them back to [`GlyphSource::rasterize`]. `id == 0` means "advance only, draw nothing".
#[derive(Clone, Copy, PartialEq, Debug, Default)]
#[repr(C)]
pub struct Glyph {
    pub face: u16,
    pub id: u16,
    /// Pen advance after this glyph, in list units.
    pub adv: f32,
}

/// A gradient colour stop; `offset` is normalised to 0..=1 along the gradient line / radius.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Stop {
    pub offset: f32,
    pub color: Rgba,
}

/// A 2D affine map `[a, b, c, d, e, f]`, laid out like CSS `matrix(a, b, c, d, e, f)`:
/// `x' = a*x + c*y + e`, `y' = b*x + d*y + f`.
pub type Affine = [f32; 6];
pub const IDENTITY: Affine = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// The map that applies `inner` first and `outer` second.
#[inline]
pub fn affine_mul(outer: &Affine, inner: &Affine) -> Affine {
    let (o, i) = (outer, inner);
    [
        o[0] * i[0] + o[2] * i[1],
        o[1] * i[0] + o[3] * i[1],
        o[0] * i[2] + o[2] * i[3],
        o[1] * i[2] + o[3] * i[3],
        o[0] * i[4] + o[2] * i[5] + o[4],
        o[1] * i[4] + o[3] * i[5] + o[5],
    ]
}

/// Map the point (`x`, `y`) through `m`.
#[inline]
pub fn affine_point(m: &Affine, x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// The inverse of `m`, or `None` if it collapses the plane (determinant near zero).
pub fn affine_inverse(m: &Affine) -> Option<Affine> {
    let det = m[0] * m[3] - m[1] * m[2];
    if !det.is_finite() || det.abs() < 1e-9 {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])])
}

/// Handle to a texture owned by the renderer (created with `Renderer::create_image`).
pub type ImageId = u32;

/// A half-open range `[start, end)` into a side buffer (`DrawList::stops`, or a layer's glyphs).
pub type Span = (u32, u32);

/// One drawing command. Items are painted in order (painter's algorithm).
#[derive(Clone, PartialEq, Debug)]
pub enum Item {
    /// Filled rectangle with optional rounded corners.
    Rect { rect: Rect, color: Rgba, radii: Radii },
    /// Border ring drawn *inside* `rect`. `widths`/`colors` are top, right, bottom, left.
    /// `radii` are the outer corner radii.
    Border { rect: Rect, widths: [f32; 4], colors: [Rgba; 4], radii: Radii },
    /// Box shadow of the box `rect` (the box itself, not the shadow's extent). For an outer
    /// shadow only the part outside the box is drawn; for `inset` only the part inside.
    Shadow { rect: Rect, radii: Radii, dx: f32, dy: f32, blur: f32, spread: f32, color: Rgba, inset: bool },
    /// Linear gradient filling `rect`. `angle` is the CSS angle in degrees (0 = towards the top,
    /// 90 = towards the right). The gradient line has the CSS length
    /// `|w * sin(angle)| + |h * cos(angle)|` and is centred on the rectangle.
    LinearGradient { rect: Rect, radii: Radii, angle: f32, stops: Span, repeating: bool },
    /// Elliptical radial gradient filling `rect`. `center` is relative to `rect`'s origin;
    /// `radius` gives the horizontal and vertical radius at stop offset 1.
    RadialGradient { rect: Rect, radii: Radii, center: (f32, f32), radius: (f32, f32), stops: Span, repeating: bool },
    /// A run of glyphs. The pen starts at (`x`, `baseline`) and advances by each glyph's `adv`.
    /// `size` is the font size in list units; `w` is the total advance (for culling).
    /// `glyphs` indexes the glyph slice supplied with the [`Layer`].
    Glyphs { x: f32, baseline: f32, w: f32, size: f32, color: Rgba, glyphs: Span },
    /// Textured rectangle. `uv` is the source rectangle in normalised texture coordinates
    /// `[u0, v0, u1, v1]`; with `repeat` set, coordinates outside 0..1 tile the image.
    Image { rect: Rect, image: ImageId, radii: Radii, opacity: f32, uv: [f32; 4], repeat: bool },
    /// Intersect the clip with `rect` (rounded by `radii`) until the matching `PopClip`.
    PushClip { rect: Rect, radii: Radii },
    PopClip,
    /// Until the matching `PopFixed`, items ignore the layer's scroll offset (viewport-fixed).
    /// Clips pushed inside are fixed too; an enclosing scrolling clip does not apply.
    PushFixed,
    PopFixed,
    /// Until the matching `PopTransform`, every item and every clip pushed inside is drawn
    /// through `m`, which is given in list units: a point `p` of an item lands at
    /// `physical = viewport.origin + (m(p) - scroll) * scale`. Nested transforms compose, the
    /// innermost applied first. A clip pushed *before* the transform keeps clipping in its own
    /// (outer) space. Shapes keep their analytic edges in their own space (a scaled rounded
    /// rectangle has scaled corners; a rotated one has rotated, anti-aliased edges). Glyphs are
    /// rasterised at the size they end up on screen. A transform that collapses the plane
    /// (determinant 0, e.g. `scale(0)`) draws nothing.
    PushTransform { m: Affine },
    PopTransform,
}

/// An ordered list of drawing commands plus the gradient stops they reference.
#[derive(Clone, Debug, Default)]
pub struct DrawList {
    pub items: Vec<Item>,
    pub stops: Vec<Stop>,
}

impl DrawList {
    pub fn new() -> DrawList {
        DrawList::default()
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.stops.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[inline]
    pub fn push(&mut self, item: Item) {
        self.items.push(item);
    }

    /// Append gradient stops and return their span.
    pub fn add_stops(&mut self, stops: &[Stop]) -> Span {
        let start = self.stops.len() as u32;
        self.stops.extend_from_slice(stops);
        (start, self.stops.len() as u32)
    }

    pub fn rect(&mut self, rect: Rect, color: Rgba) {
        if color.a != 0 && !rect.is_empty() {
            self.items.push(Item::Rect { rect, color, radii: NO_RADII });
        }
    }

    pub fn rounded_rect(&mut self, rect: Rect, color: Rgba, radii: Radii) {
        if color.a != 0 && !rect.is_empty() {
            self.items.push(Item::Rect { rect, color, radii });
        }
    }

    /// A uniform border of `width` in one colour.
    pub fn stroke(&mut self, rect: Rect, width: f32, color: Rgba, radii: Radii) {
        if color.a != 0 && width > 0.0 && !rect.is_empty() {
            self.items.push(Item::Border { rect, widths: [width; 4], colors: [color; 4], radii });
        }
    }

    pub fn glyphs(&mut self, x: f32, baseline: f32, w: f32, size: f32, color: Rgba, glyphs: Span) {
        if color.a != 0 && glyphs.1 > glyphs.0 {
            self.items.push(Item::Glyphs { x, baseline, w, size, color, glyphs });
        }
    }

    pub fn image(&mut self, rect: Rect, image: ImageId, radii: Radii, opacity: f32) {
        if opacity > 0.0 && !rect.is_empty() {
            self.items.push(Item::Image { rect, image, radii, opacity, uv: [0.0, 0.0, 1.0, 1.0], repeat: false });
        }
    }

    pub fn push_clip(&mut self, rect: Rect, radii: Radii) {
        self.items.push(Item::PushClip { rect, radii });
    }

    pub fn pop_clip(&mut self) {
        self.items.push(Item::PopClip);
    }

    pub fn push_transform(&mut self, m: Affine) {
        self.items.push(Item::PushTransform { m });
    }

    pub fn pop_transform(&mut self) {
        self.items.push(Item::PopTransform);
    }

    /// Approximate heap bytes held by this list.
    pub fn memory_bytes(&self) -> usize {
        self.items.capacity() * std::mem::size_of::<Item>() + self.stops.capacity() * std::mem::size_of::<Stop>()
    }
}

/// One display list placed on the surface. A frame is an ordered slice of layers (first is
/// drawn first), e.g. `[page, chrome]`.
#[derive(Clone, Copy)]
pub struct Layer<'a> {
    pub list: &'a DrawList,
    /// Glyph storage that `Item::Glyphs` spans index into.
    pub glyphs: &'a [Glyph],
    /// Where the layer lands on the surface, in physical pixels. Nothing is drawn outside it.
    pub viewport: Rect,
    /// Scroll offset in list units (subtracted from every non-fixed item).
    pub scroll: (f32, f32),
    /// List units to physical pixels (device scale factor x zoom).
    pub scale: f32,
}

/// An 8-bit coverage bitmap for one glyph.
#[derive(Clone, Debug)]
pub struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    /// Offset of the bitmap's left edge from the pen position, in physical pixels.
    pub left: i32,
    /// Offset of the bitmap's top edge from the baseline (negative = above), in physical pixels.
    pub top: i32,
    /// `width * height` coverage values, row-major, top row first.
    pub data: Vec<u8>,
}

/// Source of glyph bitmaps for the renderer's atlas. Implemented by the engine's font system.
pub trait GlyphSource {
    /// Rasterise glyph `glyph` of face `face` at `px_size` physical pixels per em. `subpx` is the
    /// horizontal sub-pixel pen offset in quarter pixels (0..=3). `None` = nothing to draw.
    fn rasterize(&self, face: u16, glyph: u16, px_size: f32, subpx: u8) -> Option<GlyphBitmap>;
}

/// A resolved font (family + weight + style), as handed out by a [`TextShaper`].
pub type FontId = u32;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct FontMetrics {
    /// Distance from the baseline up to the top of the em box content (positive).
    pub ascent: f32,
    /// Distance from the baseline down to the bottom (positive).
    pub descent: f32,
    pub line_gap: f32,
    pub x_height: f32,
}

impl FontMetrics {
    #[inline]
    pub fn natural_line_height(&self) -> f32 {
        self.ascent + self.descent + self.line_gap
    }
}

/// Text measurement and shaping for display-list producers. Implemented by the engine's font
/// system; UI code and tests can use any implementation (e.g. a fixed-advance mock).
pub trait TextShaper {
    /// Resolve a font. `family` is a CSS family name such as `"Geist"`, `"Segoe UI"`,
    /// `"monospace"` or `"Segoe MDL2 Assets"` (case-insensitive); unknown names fall back.
    fn font(&mut self, family: &str, weight: u16, italic: bool) -> FontId;
    /// Metrics of `font` at `size` list units per em.
    fn metrics(&mut self, font: FontId, size: f32) -> FontMetrics;
    /// Append the glyphs for `text` to `out` and return the total advance in list units.
    fn shape(&mut self, font: FontId, size: f32, text: &str, out: &mut Vec<Glyph>) -> f32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_math() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersect(&b), Rect::new(5.0, 5.0, 5.0, 5.0));
        assert!(a.intersects(&b));
        assert!(a.contains(9.9, 0.0) && !a.contains(10.0, 0.0));
        assert!(a.intersect(&Rect::new(20.0, 0.0, 5.0, 5.0)).is_empty());
        assert_eq!(a.inflate(2.0), Rect::new(-2.0, -2.0, 14.0, 14.0));
    }

    #[test]
    fn affine_math() {
        let scale: Affine = [2.0, 0.0, 0.0, 3.0, 0.0, 0.0];
        let shift: Affine = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];
        // scale first, then shift
        let m = affine_mul(&shift, &scale);
        assert_eq!(affine_point(&m, 1.0, 1.0), (12.0, 23.0));
        // shift first, then scale
        let m = affine_mul(&scale, &shift);
        assert_eq!(affine_point(&m, 1.0, 1.0), (22.0, 63.0));
        let inv = affine_inverse(&m).unwrap();
        let (x, y) = affine_point(&inv, 22.0, 63.0);
        assert!((x - 1.0).abs() < 1e-5 && (y - 1.0).abs() < 1e-5);
        assert!(affine_inverse(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0]).is_none());
        assert_eq!(affine_mul(&IDENTITY, &m), m);
    }

    #[test]
    fn list_helpers_skip_invisible() {
        let mut l = DrawList::new();
        l.rect(Rect::new(0.0, 0.0, 0.0, 5.0), Rgba::WHITE);
        l.rect(Rect::new(0.0, 0.0, 5.0, 5.0), Rgba::TRANSPARENT);
        assert!(l.is_empty());
        l.rect(Rect::new(0.0, 0.0, 5.0, 5.0), Rgba::hex(0x3b82f6));
        let span = l.add_stops(&[Stop { offset: 0.0, color: Rgba::BLACK }, Stop { offset: 1.0, color: Rgba::WHITE }]);
        assert_eq!(span, (0, 2));
        assert_eq!(l.items.len(), 1);
    }
}
