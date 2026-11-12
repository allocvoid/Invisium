//! Painting: walks the layout tree and emits a display list (`invisium_draw::DrawList`).
//!
//! Text items reference the layout tree's glyph buffer directly (no copy); the few glyphs the
//! painter generates itself (form-control text) are appended after it and dropped on repaint.

use crate::atom::a;
use crate::css::values::{BgSize, Color, GradientDir, Image, Len, LenCtx, RadialSize, Size, Stop as CssStop};
use crate::dom::{Document, NodeId, NO_NODE};
use crate::draw::{DrawList, ImageId, Item, Radii, Rect, Rgba, Stop, NO_RADII};
use crate::layout::*;
use crate::style::{BgLayer, BorderStyle, ComputedStyle, ObjectFit, StyleStore, Visibility, DECO_LINE_THROUGH, DECO_OVERLINE, DECO_UNDERLINE};
use crate::text::{FontSystem, LGlyph};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
pub struct PaintImage {
    pub id: ImageId,
    pub width: f32,
    pub height: f32,
}

pub struct PaintInput<'a> {
    pub doc: &'a Document,
    pub styles: &'a StyleStore,
    pub fonts: &'a mut FontSystem,
    /// Decoded images for replaced elements (`<img>`, rasterised `<svg>`), by element.
    pub images: &'a HashMap<NodeId, PaintImage>,
    /// Decoded CSS background images, by absolute URL.
    pub bg_images: &'a HashMap<String, PaintImage>,
    /// Current values of text controls and selects (overrides the `value` attribute).
    pub form_values: &'a HashMap<NodeId, String>,
    pub focus: NodeId,
    pub caret_on: bool,
    pub viewport_w: f32,
    pub viewport_h: f32,
}

pub struct PaintOutput {
    pub list: DrawList,
    /// Colour of the canvas behind the document (the surface is cleared to this).
    pub canvas: Rgba,
}

#[inline]
fn rgba(c: Color, alpha: f32) -> Rgba {
    Rgba { r: c.r, g: c.g, b: c.b, a: (c.a as f32 * alpha).round().clamp(0.0, 255.0) as u8 }
}

type Clip = (Rect, Radii);

struct Deferred {
    id: BoxId,
    ox: f32,
    oy: f32,
    alpha: f32,
    z: i32,
    clips: Vec<Clip>,
    fixed: bool,
}

struct Painter<'a> {
    doc: &'a Document,
    styles: &'a StyleStore,
    tree: &'a LayoutTree,
    fonts: &'a mut FontSystem,
    glyphs: Vec<LGlyph>,
    list: DrawList,
    images: &'a HashMap<NodeId, PaintImage>,
    bg_images: &'a HashMap<String, PaintImage>,
    form_values: &'a HashMap<NodeId, String>,
    focus: NodeId,
    caret_on: bool,
    clips: Vec<Clip>,
    cb_clip_len: usize,
    deferred: Vec<Deferred>,
    in_fixed: bool,
    skip_bg: BoxId,
}

fn radii_of(st: &ComputedStyle, w: f32, h: f32) -> Radii {
    let basis = w.min(h);
    let mut r = [0.0f32; 4];
    for i in 0..4 {
        r[i] = match &st.border_radius[i] {
            Size::Px(v) => *v,
            other => other.px(basis),
        }
        .max(0.0);
    }
    r
}

fn inner_radii(r: Radii, bw: &[f32; 4]) -> Radii {
    // top-left, top-right, bottom-right, bottom-left against (top,right,bottom,left) widths
    [
        (r[0] - bw[0].max(bw[3])).max(0.0),
        (r[1] - bw[0].max(bw[1])).max(0.0),
        (r[2] - bw[2].max(bw[1])).max(0.0),
        (r[3] - bw[2].max(bw[3])).max(0.0),
    ]
}

fn shade(c: Color, k: f32) -> Color {
    let f = |v: u8| ((v as f32) * k).round().clamp(0.0, 255.0) as u8;
    Color { r: f(c.r), g: f(c.g), b: f(c.b), a: c.a }
}

fn lighten(c: Color) -> Color {
    let f = |v: u8| (v as f32 + (255.0 - v as f32) * 0.55).round() as u8;
    Color { r: f(c.r), g: f(c.g), b: f(c.b), a: c.a }
}

/// Normalise CSS colour stops to offsets in 0..=1 along a gradient line of `length` pixels.
fn resolve_stops(stops: &[CssStop], length: f32, ctx: &LenCtx, alpha: f32, out: &mut Vec<Stop>) {
    let n = stops.len();
    let mut pos: Vec<Option<f32>> = stops
        .iter()
        .map(|s| s.pos.as_ref().map(|p| if length > 0.0 { p.to_px_with(ctx, length) / length } else { 0.0 }))
        .collect();
    if n == 0 {
        return;
    }
    if pos[0].is_none() {
        pos[0] = Some(0.0);
    }
    if pos[n - 1].is_none() {
        pos[n - 1] = Some(1.0);
    }
    // positions never decrease
    let mut last = f32::NEG_INFINITY;
    for p in pos.iter_mut().flatten() {
        if *p < last {
            *p = last;
        }
        last = *p;
    }
    // spread unpositioned stops evenly between their positioned neighbours
    let mut i = 0;
    while i < n {
        if pos[i].is_none() {
            let start = i - 1;
            let mut end = i;
            while pos[end].is_none() {
                end += 1;
            }
            let (a0, a1) = (pos[start].unwrap(), pos[end].unwrap());
            for k in i..end {
                pos[k] = Some(a0 + (a1 - a0) * (k - start) as f32 / (end - start) as f32);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    for (s, p) in stops.iter().zip(pos) {
        out.push(Stop { offset: p.unwrap_or(0.0), color: rgba(s.color, alpha) });
    }
}

impl<'a> Painter<'a> {
    fn ctx(&self, st: &ComputedStyle) -> LenCtx {
        self.styles.len_ctx(st.font_size)
    }

    fn push_clip(&mut self, rect: Rect, radii: Radii) {
        self.clips.push((rect, radii));
        self.list.push_clip(rect, radii);
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
        self.list.pop_clip();
    }

    /// Paint `id` as a stacking context. Precondition: no clip is open in the list.
    fn paint_context(&mut self, d: Deferred) {
        if d.fixed {
            self.list.push(Item::PushFixed);
        }
        for (r, rad) in &d.clips {
            self.list.push_clip(*r, *rad);
        }
        let n_clips = d.clips.len();
        self.clips = d.clips;
        let saved_cb = std::mem::replace(&mut self.cb_clip_len, n_clips);
        let saved_fixed = std::mem::replace(&mut self.in_fixed, d.fixed);
        let saved_deferred = std::mem::take(&mut self.deferred);

        self.paint_box(d.id, d.ox, d.oy, d.alpha);

        let mut mine = std::mem::replace(&mut self.deferred, saved_deferred);
        for _ in 0..n_clips {
            self.list.pop_clip();
        }
        self.clips.clear();
        if d.fixed {
            self.list.push(Item::PopFixed);
        }
        self.cb_clip_len = saved_cb;
        self.in_fixed = saved_fixed;
        mine.sort_by_key(|x| x.z);
        for child in mine {
            self.paint_context(child);
        }
    }

    /// Paint a child box now, or queue it if it is positioned (painted above normal flow).
    fn paint_child(&mut self, child: BoxId, x: f32, y: f32, alpha: f32) {
        let b = &self.tree.boxes[child as usize];
        if b.flags & (F_POSITIONED | F_OUT_OF_FLOW) != 0 && !matches!(b.kind, BoxKind::Inline | BoxKind::Text | BoxKind::Break) {
            let st = self.styles.style(b.style);
            let z = st.z_index.unwrap_or(0);
            if z >= 0 {
                let fixed = b.flags & F_FIXED != 0 || self.in_fixed;
                let clips = if b.flags & F_FIXED != 0 {
                    Vec::new()
                } else if b.flags & F_OUT_OF_FLOW != 0 {
                    self.clips[..self.cb_clip_len.min(self.clips.len())].to_vec()
                } else {
                    self.clips.clone()
                };
                self.deferred.push(Deferred { id: child, ox: x, oy: y, alpha, z, clips, fixed });
                return;
            }
        }
        self.paint_box(child, x, y, alpha);
    }

    fn paint_box(&mut self, id: BoxId, ox: f32, oy: f32, alpha: f32) {
        let tree = self.tree;
        let b = &tree.boxes[id as usize];
        let st = self.styles.rc(b.style);
        let alpha = alpha * st.opacity;
        if alpha <= 0.003 {
            return;
        }
        let (mut x, mut y) = (ox + b.x, oy + b.y);
        if let Some((tx, ty)) = &st.translate {
            x += tx.px(b.w);
            y += ty.px(b.h);
        }
        let visible = st.visibility == Visibility::Visible;
        match b.kind {
            BoxKind::Marker => {
                if visible && b.r1 > b.r0 {
                    self.list.glyphs(x, y + b.baseline, b.w, st.font_size, rgba(st.color, alpha), (b.r0, b.r1));
                }
                return;
            }
            BoxKind::Inline | BoxKind::Text | BoxKind::Break => {
                let mut c = b.first_child;
                while c != NO_BOX {
                    self.paint_child(c, x, y, alpha);
                    c = tree.boxes[c as usize].next;
                }
                return;
            }
            _ => {}
        }
        let rect = Rect::new(x, y, b.w, b.h);
        if visible {
            self.decorations(id, &st, rect, alpha);
            if b.kind == BoxKind::Replaced {
                self.replaced(id, &st, rect, alpha);
            }
        }
        let clip = b.flags & F_CLIP != 0;
        if clip {
            let bw = &st.border_width;
            let inner = Rect::new(x + bw[3], y + bw[0], (b.w - bw[1] - bw[3]).max(0.0), (b.h - bw[0] - bw[2]).max(0.0));
            let rr = inner_radii(radii_of(&st, b.w, b.h), bw);
            self.push_clip(inner, rr);
        }
        let saved_cb = self.cb_clip_len;
        if b.flags & (F_POSITIONED | F_OUT_OF_FLOW) != 0 {
            self.cb_clip_len = self.clips.len();
        }
        if b.flags & F_IFC != 0 {
            self.fragments(id, x, y, alpha);
            self.out_of_flow_in_inline(id, x, y, alpha);
        } else {
            let mut c = b.first_child;
            while c != NO_BOX {
                self.paint_child(c, x, y, alpha);
                c = tree.boxes[c as usize].next;
            }
        }
        self.cb_clip_len = saved_cb;
        if clip {
            self.pop_clip();
        }
    }

    /// Offset contributed by inline-box ancestors between `id` and the IFC root `root`.
    fn inline_offset(&self, id: BoxId, root: BoxId) -> (f32, f32) {
        let (mut dx, mut dy) = (0.0, 0.0);
        let mut p = self.tree.boxes[id as usize].parent;
        while p != NO_BOX && p != root {
            let pb = &self.tree.boxes[p as usize];
            dx += pb.x;
            dy += pb.y;
            p = pb.parent;
        }
        (dx, dy)
    }

    fn fragments(&mut self, id: BoxId, x: f32, y: f32, alpha: f32) {
        let tree = self.tree;
        let b = &tree.boxes[id as usize];
        for fi in b.r0 as usize..b.r1 as usize {
            let f = tree.fragments[fi];
            let fb = &tree.boxes[f.box_id as usize];
            match f.kind {
                FragKind::InlineBox => {
                    let st = self.styles.rc(fb.style);
                    if st.visibility == Visibility::Visible {
                        self.decorations(NO_BOX, &st, Rect::new(x + f.x, y + f.y, f.w, f.h), alpha * st.opacity);
                    }
                }
                FragKind::Text => {
                    let st = self.styles.style(fb.style);
                    if st.visibility != Visibility::Visible {
                        continue;
                    }
                    let color = rgba(st.color, alpha);
                    self.list.glyphs(x + f.x, y + f.baseline, f.w, st.font_size, color, (f.g0, f.g1));
                    if st.text_decoration != 0 {
                        let t = (st.font_size / 14.0).round().max(1.0);
                        let fx = x + f.x;
                        if st.text_decoration & DECO_UNDERLINE != 0 {
                            self.list.rect(Rect::new(fx, (y + f.baseline + t).round(), f.w, t), color);
                        }
                        if st.text_decoration & DECO_OVERLINE != 0 {
                            self.list.rect(Rect::new(fx, (y + f.y).round(), f.w, t), color);
                        }
                        if st.text_decoration & DECO_LINE_THROUGH != 0 {
                            let mid = y + f.baseline - (f.baseline - f.y) * 0.32;
                            self.list.rect(Rect::new(fx, mid.round(), f.w, t), color);
                        }
                    }
                }
                FragKind::Atomic => {
                    let (dx, dy) = self.inline_offset(f.box_id, id);
                    self.paint_child(f.box_id, x + dx, y + dy, alpha);
                }
            }
        }
    }

    /// Floats and absolutely positioned boxes nested inside inline content.
    fn out_of_flow_in_inline(&mut self, parent: BoxId, x: f32, y: f32, alpha: f32) {
        let tree = self.tree;
        let mut c = tree.boxes[parent as usize].first_child;
        while c != NO_BOX {
            let cb = &tree.boxes[c as usize];
            if cb.flags & (F_OUT_OF_FLOW | F_FLOAT) != 0 {
                self.paint_child(c, x, y, alpha);
            } else if cb.kind == BoxKind::Inline {
                self.out_of_flow_in_inline(c, x + cb.x, y + cb.y, alpha);
            }
            c = cb.next;
        }
    }

    fn decorations(&mut self, id: BoxId, st: &ComputedStyle, rect: Rect, alpha: f32) {
        if rect.w <= 0.0 && rect.h <= 0.0 {
            return;
        }
        let radii = radii_of(st, rect.w, rect.h);
        if let Some(shadows) = &st.box_shadow {
            for s in shadows.iter().rev().filter(|s| !s.inset) {
                self.list.push(Item::Shadow {
                    rect,
                    radii,
                    dx: s.x,
                    dy: s.y,
                    blur: s.blur,
                    spread: s.spread,
                    color: rgba(s.color, alpha),
                    inset: false,
                });
            }
        }
        if (id == NO_BOX || id != self.skip_bg) && !st.masked {
            self.background(st, rect, radii, alpha);
        }
        if let Some(shadows) = &st.box_shadow {
            for s in shadows.iter().rev().filter(|s| s.inset) {
                self.list.push(Item::Shadow {
                    rect,
                    radii,
                    dx: s.x,
                    dy: s.y,
                    blur: s.blur,
                    spread: s.spread,
                    color: rgba(s.color, alpha),
                    inset: true,
                });
            }
        }
        if st.has_visible_border() {
            let mut colors = [Rgba::TRANSPARENT; 4];
            for i in 0..4 {
                let base = st.border_color[i];
                let c = match st.border_style[i] {
                    // two-tone 3D styles: top/left vs bottom/right
                    BorderStyle::Inset | BorderStyle::Groove => {
                        if i == 0 || i == 3 {
                            shade(base, 0.6)
                        } else {
                            lighten(base)
                        }
                    }
                    BorderStyle::Outset | BorderStyle::Ridge => {
                        if i == 0 || i == 3 {
                            lighten(base)
                        } else {
                            shade(base, 0.6)
                        }
                    }
                    _ => base,
                };
                colors[i] = rgba(c, alpha);
            }
            let w = st.border_width;
            if colors.iter().zip(w.iter()).any(|(c, w)| c.a != 0 && *w > 0.0) {
                self.list.push(Item::Border { rect, widths: [w[0], w[1], w[2], w[3]], colors, radii });
            }
        }
        if st.outline_width > 0.0 && st.outline_style != BorderStyle::None {
            let d = st.outline_offset + st.outline_width;
            let r = rect.inflate(d);
            let rr = radii.map(|v| if v > 0.0 { v + d } else { 0.0 });
            self.list.stroke(r, st.outline_width, rgba(st.outline_color, alpha), rr);
        }
    }

    fn background(&mut self, st: &ComputedStyle, rect: Rect, radii: Radii, alpha: f32) {
        if !st.background_color.is_transparent() {
            self.list.rounded_rect(rect, rgba(st.background_color, alpha), radii);
        }
        let Some(layers) = &st.background else { return };
        let ctx = self.ctx(st);
        for layer in layers.iter().rev() {
            self.bg_layer(layer, rect, radii, alpha, &ctx);
        }
    }

    fn bg_layer(&mut self, layer: &BgLayer, rect: Rect, radii: Radii, alpha: f32, ctx: &LenCtx) {
        match &layer.image {
            Image::None => {}
            Image::Linear { dir, stops, repeating } => {
                let angle = match dir {
                    GradientDir::Angle(a) => *a,
                    GradientDir::Corner(h, v) => {
                        let t = rect.h.atan2(rect.w).to_degrees();
                        match (*h > 0, *v > 0) {
                            (true, false) => t,
                            (true, true) => 180.0 - t,
                            (false, true) => 180.0 + t,
                            (false, false) => 360.0 - t,
                        }
                    }
                };
                let rad = angle.to_radians();
                let length = (rect.w * rad.sin()).abs() + (rect.h * rad.cos()).abs();
                let mut out = Vec::with_capacity(stops.len());
                resolve_stops(stops, length, ctx, alpha, &mut out);
                if out.iter().all(|s| s.color.a == 0) {
                    return;
                }
                let span = self.list.add_stops(&out);
                self.list.push(Item::LinearGradient { rect, radii, angle, stops: span, repeating: *repeating });
            }
            Image::Radial { circle, size, center, stops, repeating } => {
                let cxp = center.0.to_px_with(ctx, rect.w);
                let cyp = center.1.to_px_with(ctx, rect.h);
                let (dl, dr, dt, db) = (cxp.abs(), (rect.w - cxp).abs(), cyp.abs(), (rect.h - cyp).abs());
                let (rx, ry) = match size {
                    RadialSize::Explicit(a, b) => (a.to_px_with(ctx, rect.w).max(0.01), b.to_px_with(ctx, rect.h).max(0.01)),
                    RadialSize::ClosestSide => {
                        if *circle {
                            let r = dl.min(dr).min(dt).min(db);
                            (r, r)
                        } else {
                            (dl.min(dr), dt.min(db))
                        }
                    }
                    RadialSize::FarthestSide => {
                        if *circle {
                            let r = dl.max(dr).max(dt).max(db);
                            (r, r)
                        } else {
                            (dl.max(dr), dt.max(db))
                        }
                    }
                    RadialSize::ClosestCorner => {
                        let (ax, ay) = (dl.min(dr), dt.min(db));
                        if *circle {
                            let r = (ax * ax + ay * ay).sqrt();
                            (r, r)
                        } else {
                            (ax * std::f32::consts::SQRT_2, ay * std::f32::consts::SQRT_2)
                        }
                    }
                    RadialSize::FarthestCorner => {
                        let (ax, ay) = (dl.max(dr), dt.max(db));
                        if *circle {
                            let r = (ax * ax + ay * ay).sqrt();
                            (r, r)
                        } else {
                            (ax * std::f32::consts::SQRT_2, ay * std::f32::consts::SQRT_2)
                        }
                    }
                };
                let (rx, ry) = (rx.max(0.01), ry.max(0.01));
                let mut out = Vec::with_capacity(stops.len());
                resolve_stops(stops, rx, ctx, alpha, &mut out);
                if out.iter().all(|s| s.color.a == 0) {
                    return;
                }
                let span = self.list.add_stops(&out);
                self.list.push(Item::RadialGradient { rect, radii, center: (cxp, cyp), radius: (rx, ry), stops: span, repeating: *repeating });
            }
            Image::Url(url) => {
                let Some(img) = self.bg_images.get(&**url).copied() else { return };
                if img.width <= 0.0 || img.height <= 0.0 || rect.is_empty() {
                    return;
                }
                let (tw, th) = match &layer.size {
                    BgSize::Auto => (img.width, img.height),
                    BgSize::Cover => {
                        let k = (rect.w / img.width).max(rect.h / img.height);
                        (img.width * k, img.height * k)
                    }
                    BgSize::Contain => {
                        let k = (rect.w / img.width).min(rect.h / img.height);
                        (img.width * k, img.height * k)
                    }
                    BgSize::Explicit(w, h) => {
                        let w = w.as_ref().map(|l| l.to_px_with(ctx, rect.w));
                        let h = h.as_ref().map(|l| l.to_px_with(ctx, rect.h));
                        match (w, h) {
                            (Some(w), Some(h)) => (w, h),
                            (Some(w), None) => (w, w * img.height / img.width),
                            (None, Some(h)) => (h * img.width / img.height, h),
                            (None, None) => (img.width, img.height),
                        }
                    }
                };
                if tw <= 0.01 || th <= 0.01 {
                    return;
                }
                let pos = |p: &Len, box_len: f32, tile: f32, basis_ctx: &LenCtx| -> f32 {
                    match p {
                        Len::Pct(v) => (box_len - tile) * v / 100.0,
                        other => other.to_px_with(basis_ctx, box_len - tile),
                    }
                };
                let px = pos(&layer.pos.0, rect.w, tw, ctx);
                let py = pos(&layer.pos.1, rect.h, th, ctx);
                // per axis: (draw start, draw length, uv start, uv end)
                let axis = |repeat: bool, box_len: f32, tile: f32, off: f32| -> (f32, f32, f32, f32) {
                    if repeat {
                        (0.0, box_len, -off / tile, (box_len - off) / tile)
                    } else {
                        let s = off.max(0.0);
                        let e = (off + tile).min(box_len);
                        (s, (e - s).max(0.0), (s - off) / tile, (e - off) / tile)
                    }
                };
                let (sx, lw, u0, u1) = axis(layer.repeat.0, rect.w, tw, px);
                let (sy, lh, v0, v1) = axis(layer.repeat.1, rect.h, th, py);
                if lw <= 0.0 || lh <= 0.0 {
                    return;
                }
                let covers = sx <= 0.0 && sy <= 0.0 && lw >= rect.w && lh >= rect.h;
                self.list.push(Item::Image {
                    rect: Rect::new(rect.x + sx, rect.y + sy, lw, lh),
                    image: img.id,
                    radii: if covers { radii } else { NO_RADII },
                    opacity: alpha,
                    uv: [u0, v0, u1, v1],
                    repeat: layer.repeat.0 || layer.repeat.1,
                });
            }
        }
    }

    /// Shape `text` and emit it; returns its width.
    fn text(&mut self, text: &str, st: &ComputedStyle, x: f32, baseline: f32, color: Rgba) -> f32 {
        let font = self.fonts.resolve(&st.font_family, st.font_weight, st.italic);
        let g0 = self.glyphs.len() as u32;
        let w = self.fonts.shape(font, st.font_size, text, st.letter_spacing, &mut self.glyphs);
        self.list.glyphs(x, baseline, w, st.font_size, color, (g0, self.glyphs.len() as u32));
        w
    }

    fn text_width(&mut self, text: &str, st: &ComputedStyle) -> f32 {
        let font = self.fonts.resolve(&st.font_family, st.font_weight, st.italic);
        self.fonts.measure(font, st.font_size, text)
    }

    /// Content of replaced elements and form controls.
    fn replaced(&mut self, id: BoxId, st: &ComputedStyle, rect: Rect, alpha: f32) {
        let b = &self.tree.boxes[id as usize];
        let node = b.node;
        let tag = self.doc.tag(node);
        let bw = &st.border_width;
        let content = Rect::new(
            rect.x + bw[3] + b.pad[3],
            rect.y + bw[0] + b.pad[0],
            (rect.w - bw[1] - bw[3] - b.pad[1] - b.pad[3]).max(0.0),
            (rect.h - bw[0] - bw[2] - b.pad[0] - b.pad[2]).max(0.0),
        );
        if content.is_empty() {
            return;
        }
        let input_type = if tag == a::INPUT { self.doc.attr(node, a::TYPE).unwrap_or("text").to_ascii_lowercase() } else { String::new() };
        let is_image = tag == a::IMG || tag == a::SVG || (tag == a::INPUT && input_type == "image");
        if is_image || self.images.contains_key(&node) {
            let Some(img) = self.images.get(&node).copied() else { return };
            let rr = inner_radii(radii_of(st, rect.w, rect.h), bw);
            let (mut dst, mut uv) = (content, [0.0f32, 0.0, 1.0, 1.0]);
            if img.width > 0.0 && img.height > 0.0 {
                let (iw, ih) = (img.width, img.height);
                match st.object_fit {
                    ObjectFit::Fill => {}
                    ObjectFit::Cover => {
                        // crop the source so it fills the box at its own aspect ratio
                        let k = (content.w / iw).max(content.h / ih);
                        let (vw, vh) = (content.w / (iw * k), content.h / (ih * k));
                        uv = [(1.0 - vw) / 2.0, (1.0 - vh) / 2.0, (1.0 + vw) / 2.0, (1.0 + vh) / 2.0];
                    }
                    ObjectFit::Contain | ObjectFit::ScaleDown | ObjectFit::None => {
                        let mut k = (content.w / iw).min(content.h / ih);
                        if st.object_fit != ObjectFit::Contain {
                            k = k.min(1.0);
                        }
                        let (dw, dh) = (iw * k, ih * k);
                        dst = Rect::new(content.x + (content.w - dw) / 2.0, content.y + (content.h - dh) / 2.0, dw, dh);
                    }
                }
            }
            self.list.push(Item::Image { rect: dst, image: img.id, radii: rr, opacity: alpha, uv, repeat: false });
            return;
        }
        let color = rgba(st.color, alpha);
        let font = self.fonts.resolve(&st.font_family, st.font_weight, st.italic);
        let m = self.fonts.metrics(font, st.font_size);
        let baseline = content.y + (content.h - (m.ascent + m.descent)) / 2.0 + m.ascent;
        match tag {
            a::INPUT => match input_type.as_str() {
                "checkbox" | "radio" => {
                    if self.doc.attr(node, a::CHECKED).is_some() {
                        let inset = rect.inflate(-3.0);
                        let rr = if input_type == "radio" { [inset.w / 2.0; 4] } else { [1.5; 4] };
                        self.list.rounded_rect(inset, rgba(Color::hex(0x2563eb), alpha), rr);
                    }
                }
                "submit" | "button" | "reset" => {
                    let label = self.doc.attr(node, a::VALUE).map(|s| s.to_string()).unwrap_or_else(|| match input_type.as_str() {
                        "submit" => "Submit".into(),
                        "reset" => "Reset".into(),
                        _ => String::new(),
                    });
                    let w = self.text_width(&label, st);
                    self.text(&label, st, content.x + ((content.w - w) / 2.0).max(0.0), baseline, color);
                }
                "range" | "color" | "file" => {}
                _ => {
                    let value = self.form_values.get(&node).cloned().or_else(|| self.doc.attr(node, a::VALUE).map(|s| s.to_string())).unwrap_or_default();
                    self.list.push_clip(content, NO_RADII);
                    let mut end_x = content.x;
                    if value.is_empty() {
                        if let Some(ph) = self.doc.attr(node, a::PLACEHOLDER) {
                            let ph = ph.to_string();
                            self.text(&ph, st, content.x, baseline, rgba(st.color, alpha * 0.5));
                        }
                    } else {
                        let shown = if input_type == "password" { "\u{2022}".repeat(value.chars().count()) } else { value };
                        // keep the end of long values (and the caret) in view
                        let w = self.text_width(&shown, st);
                        let start = if w > content.w { content.x + content.w - w - 1.0 } else { content.x };
                        self.text(&shown, st, start, baseline, color);
                        end_x = start + w;
                    }
                    if self.focus == node && self.caret_on {
                        self.list.rect(Rect::new(end_x.min(content.right() - 1.0), baseline - m.ascent, 1.0, m.ascent + m.descent), color);
                    }
                    self.list.pop_clip();
                }
            },
            a::TEXTAREA => {
                let value = self.form_values.get(&node).cloned().unwrap_or_else(|| self.doc.text_content(node));
                let lh = st.line_height_px(m.natural_line_height());
                self.list.push_clip(content, NO_RADII);
                let mut y = content.y + (lh - (m.ascent + m.descent)) / 2.0 + m.ascent;
                let mut end = (content.x, y);
                if value.is_empty() {
                    if let Some(ph) = self.doc.attr(node, a::PLACEHOLDER) {
                        let ph = ph.to_string();
                        self.text(&ph, st, content.x, y, rgba(st.color, alpha * 0.5));
                    }
                } else {
                    for line in value.split('\n') {
                        let w = self.text(line, st, content.x, y, color);
                        end = (content.x + w, y);
                        y += lh;
                        if y - m.ascent > content.bottom() {
                            break;
                        }
                    }
                }
                if self.focus == node && self.caret_on {
                    self.list.rect(Rect::new(end.0, end.1 - m.ascent, 1.0, m.ascent + m.descent), color);
                }
                self.list.pop_clip();
            }
            a::SELECT => {
                let label = self.form_values.get(&node).cloned().unwrap_or_else(|| {
                    let opts: Vec<NodeId> = self.doc.descendants(node).filter(|&n| self.doc.tag(n) == a::OPTION).collect();
                    let chosen = opts.iter().copied().find(|&o| self.doc.attr(o, a::SELECTED).is_some()).or(opts.first().copied());
                    chosen.map(|o| self.doc.text_content(o).trim().to_string()).unwrap_or_default()
                });
                self.list.push_clip(content, NO_RADII);
                self.text(&label, st, content.x, baseline, color);
                self.list.pop_clip();
                // drop-down arrow in the right padding
                let ax = rect.right() - bw[1] - 14.0;
                self.text("\u{25be}", st, ax, baseline, rgba(st.color, alpha * 0.7));
            }
            a::IFRAME | a::VIDEO | a::AUDIO | a::OBJECT | a::EMBED => {
                // embedded content is not loaded: show a quiet placeholder
                self.list.rect(content, rgba(Color::rgba(128, 128, 128, 36), alpha));
                let label = match tag {
                    a::IFRAME => "frame not loaded",
                    a::VIDEO => "video",
                    a::AUDIO => "audio",
                    _ => "embedded content",
                };
                if content.w > 90.0 && content.h > 20.0 {
                    let mut ls = st.clone();
                    ls.font_size = 12.0;
                    let w = self.text_width(label, &ls);
                    self.text(label, &ls, content.x + ((content.w - w) / 2.0).max(0.0), content.y + content.h / 2.0 + 4.0, rgba(Color::rgba(128, 128, 128, 255), alpha));
                }
            }
            _ => {}
        }
    }
}

/// Paint the document. The returned list's `Glyphs` spans index `tree.glyphs`.
pub fn paint(tree: &mut LayoutTree, input: PaintInput) -> PaintOutput {
    let mut out = PaintOutput { list: DrawList::new(), canvas: Rgba::WHITE };
    if tree.root == NO_BOX {
        return out;
    }
    // drop glyphs generated by the previous paint
    tree.glyphs.truncate(tree.glyph_base as usize);
    let glyphs = std::mem::take(&mut tree.glyphs);

    // The root's background (or the body's, when the root has none) covers the whole canvas.
    let root = tree.root;
    let mut canvas_box = root;
    let has_bg = |b: BoxId| {
        let st = input.styles.style(tree.boxes[b as usize].style);
        !st.background_color.is_transparent() || st.background.is_some()
    };
    if !has_bg(root) {
        let body = input.doc.body();
        if body != NO_NODE {
            if let Some(bb) = tree.children(root).find(|&c| tree.boxes[c as usize].node == body) {
                if has_bg(bb) {
                    canvas_box = bb;
                }
            }
        }
    }
    let canvas_style = input.styles.rc(tree.boxes[canvas_box as usize].style);
    if canvas_style.background_color.a == 255 {
        out.canvas = rgba(canvas_style.background_color, 1.0);
    } else if !canvas_style.background_color.is_transparent() {
        // translucent canvas colours are composited over white
        let c = canvas_style.background_color;
        let a = c.a as f32 / 255.0;
        let mix = |v: u8| (v as f32 * a + 255.0 * (1.0 - a)).round() as u8;
        out.canvas = Rgba::rgb(mix(c.r), mix(c.g), mix(c.b));
    }

    let tree_ref: &LayoutTree = tree;
    let mut p = Painter {
        doc: input.doc,
        styles: input.styles,
        tree: tree_ref,
        fonts: input.fonts,
        glyphs,
        list: DrawList::new(),
        images: input.images,
        bg_images: input.bg_images,
        form_values: input.form_values,
        focus: input.focus,
        caret_on: input.caret_on,
        clips: Vec::new(),
        cb_clip_len: 0,
        deferred: Vec::new(),
        in_fixed: false,
        skip_bg: canvas_box,
    };
    if canvas_style.background.is_some() {
        let area = Rect::new(0.0, 0.0, tree_ref.width.max(input.viewport_w), tree_ref.height.max(input.viewport_h));
        let ctx = p.ctx(&canvas_style);
        if let Some(layers) = &canvas_style.background {
            for layer in layers.iter().rev() {
                p.bg_layer(layer, area, NO_RADII, 1.0, &ctx);
            }
        }
    }
    p.paint_context(Deferred { id: root, ox: 0.0, oy: 0.0, alpha: 1.0, z: 0, clips: Vec::new(), fixed: false });
    let Painter { glyphs, list, .. } = p;
    tree.glyphs = glyphs;
    out.list = list;
    out
}
