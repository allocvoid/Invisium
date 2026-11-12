//! Layout: turns the styled DOM into a tree of positioned boxes.
//!
//! Boxes live in one arena (`Vec<LBox>`, ~120 bytes each) with index links. Positions are stored
//! relative to the parent box, so moving a subtree (flex items, table cells, floats) is O(1).
//! Inline content is flattened into `Fragment`s per block, and all glyphs of the document share
//! one buffer.

mod build;
mod flex;
mod grid;
mod inline;
mod table;
#[cfg(test)]
mod tests;

use crate::atom::a;
use crate::dom::{Document, NodeId};
use crate::style::{
    BoxSizing, Clear, ComputedStyle, Display, Float, Overflow, Position, StyleId, StyleStore, TextAlign,
};
use crate::css::values::Size;
use crate::text::{FontSystem, LGlyph};
use std::collections::HashMap;
use std::rc::Rc;

pub type BoxId = u32;
pub const NO_BOX: BoxId = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum BoxKind {
    /// Block container: holds either block-level children or (with `F_IFC`) inline content.
    Block,
    /// Inline box (`<span>`); has no geometry of its own.
    Inline,
    Text,
    Break,
    /// Replaced or widget content: images, form controls, embedded frames.
    Replaced,
    Flex,
    Grid,
    Table,
    TableRowGroup,
    TableRow,
    TableCell,
    TableCaption,
    /// List-item marker.
    Marker,
}

pub const F_INLINE_LEVEL: u16 = 1;
pub const F_IFC: u16 = 2;
pub const F_ANON: u16 = 4;
pub const F_FIXED: u16 = 8;
pub const F_BEFORE: u16 = 16;
pub const F_AFTER: u16 = 32;
pub const F_OUT_OF_FLOW: u16 = 64;
pub const F_FLOAT: u16 = 128;
pub const F_BFC: u16 = 256;
pub const F_POSITIONED: u16 = 512;
pub const F_GEN_TEXT: u16 = 1024;
pub const F_CLIP: u16 = 2048;

#[derive(Clone, Debug)]
pub struct LBox {
    pub kind: BoxKind,
    pub flags: u16,
    pub node: NodeId,
    pub style: StyleId,
    pub parent: BoxId,
    pub first_child: BoxId,
    pub last_child: BoxId,
    pub next: BoxId,
    /// Border-box position relative to the parent box's border-box origin.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Used margins: top, right, bottom, left.
    pub margin: [f32; 4],
    /// Used padding: top, right, bottom, left.
    pub pad: [f32; 4],
    /// Distance from the border-box top to the first baseline (NaN if none).
    pub baseline: f32,
    /// IFC root: fragment range. Otherwise unused.
    pub r0: u32,
    pub r1: u32,
    /// Text: index into `LayoutTree::gen_text` when `F_GEN_TEXT`; cells: colspan | rowspan << 16.
    pub aux: u32,
    iw_min: f32,
    iw_max: f32,
    key_w: f32,
    key_h: f32,
    key_fh: f32,
    key_mode: f32,
}

impl LBox {
    pub fn is_inline_level(&self) -> bool {
        matches!(self.kind, BoxKind::Inline | BoxKind::Text | BoxKind::Break) || self.flags & F_INLINE_LEVEL != 0
    }
    pub fn is_out_of_flow(&self) -> bool {
        self.flags & F_OUT_OF_FLOW != 0
    }
    pub fn is_float(&self) -> bool {
        self.flags & F_FLOAT != 0
    }
    /// Neither block-level nor inline-level for the purpose of choosing a formatting context.
    pub fn is_neutral(&self) -> bool {
        self.flags & (F_OUT_OF_FLOW | F_FLOAT) != 0 || self.kind == BoxKind::Marker
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum FragKind {
    Text,
    /// Background/border of an inline box on one line.
    InlineBox,
    /// An inline-level box with its own geometry (inline-block, image, control).
    Atomic,
}

#[derive(Clone, Copy, Debug)]
pub struct Fragment {
    /// Relative to the IFC root's border-box origin.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub baseline: f32,
    pub kind: FragKind,
    pub box_id: BoxId,
    pub g0: u32,
    pub g1: u32,
}

pub struct LayoutTree {
    pub boxes: Vec<LBox>,
    pub fragments: Vec<Fragment>,
    pub glyphs: Vec<LGlyph>,
    /// Number of glyphs produced by layout; anything after this was appended by the painter.
    pub glyph_base: u32,
    pub gen_text: Vec<String>,
    pub root: BoxId,
    /// Extent of the laid-out document (scrollable area).
    pub width: f32,
    pub height: f32,
    /// `position: sticky` boxes with the position layout gave them, parents before children.
    pub sticky: Vec<StickyBox>,
}

/// A `position: sticky` box and where it sits when nothing makes it stick.
#[derive(Clone, Copy, Debug)]
pub struct StickyBox {
    pub id: BoxId,
    pub base_x: f32,
    pub base_y: f32,
}

impl LayoutTree {
    pub fn empty() -> LayoutTree {
        LayoutTree { boxes: Vec::new(), fragments: Vec::new(), glyphs: Vec::new(), glyph_base: 0, gen_text: Vec::new(), root: NO_BOX, width: 0.0, height: 0.0, sticky: Vec::new() }
    }

    pub fn children(&self, id: BoxId) -> BoxChildren<'_> {
        BoxChildren { tree: self, next: self.boxes[id as usize].first_child }
    }

    pub fn memory_bytes(&self) -> usize {
        self.boxes.capacity() * std::mem::size_of::<LBox>()
            + self.fragments.capacity() * std::mem::size_of::<Fragment>()
            + self.glyphs.capacity() * std::mem::size_of::<LGlyph>()
    }

    /// Absolute position (document coordinates) of a box's border-box origin.
    pub fn abs_pos(&self, id: BoxId) -> (f32, f32) {
        let (mut x, mut y) = (0.0, 0.0);
        let mut b = id;
        while b != NO_BOX {
            let bx = &self.boxes[b as usize];
            x += bx.x;
            y += bx.y;
            b = bx.parent;
        }
        (x, y)
    }

    /// Move every `position: sticky` box to where it belongs for this scroll position (document
    /// scrolling only: a box inside a clipping ancestor sticks to that ancestor, which never
    /// scrolls here, so it stays put). Returns true if any box moved.
    pub fn place_sticky(&mut self, styles: &StyleStore, scroll: (f32, f32), viewport: (f32, f32)) -> bool {
        let mut moved = false;
        for i in 0..self.sticky.len() {
            let s = self.sticky[i];
            let Some(b) = self.boxes.get(s.id as usize) else { continue };
            let (w, h, margin, parent) = (b.w, b.h, b.margin, b.parent);
            let st = styles.style(b.style);
            let (mut dx, mut dy) = (0.0f32, 0.0f32);
            // the scroller is the document unless an ancestor clips or is viewport-fixed
            let mut free = parent != NO_BOX;
            let mut a = parent;
            while a != NO_BOX && free {
                let ab = &self.boxes[a as usize];
                // `overflow: clip` cuts content off without making a scroller, and what the root
                // and the body ask for applies to the viewport itself
                let scroller = |o: Overflow| matches!(o, Overflow::Hidden | Overflow::Scroll | Overflow::Auto);
                let ast = styles.style(ab.style);
                let own_scroller = ab.flags & F_CLIP != 0 && (scroller(ast.overflow_x) || scroller(ast.overflow_y));
                if ab.flags & F_FIXED != 0 || (own_scroller && a != self.root && ab.parent != self.root) {
                    free = false;
                }
                a = ab.parent;
            }
            if free {
                let pb = &self.boxes[parent as usize];
                let pst = styles.style(pb.style);
                let (px, py) = self.abs_pos(parent);
                let bw = pst.border_width;
                // content box of the containing block, in document coordinates
                let (c_top, c_bottom) = (py + bw[0] + pb.pad[0], py + pb.h - bw[2] - pb.pad[2]);
                let (c_left, c_right) = (px + bw[3] + pb.pad[3], px + pb.w - bw[1] - pb.pad[1]);
                let (nx, ny) = (px + s.base_x, py + s.base_y);
                if let Some(t) = st.inset[0].resolve(viewport.1) {
                    let room = (c_bottom - h - margin[2]) - ny;
                    dy = (scroll.1 + t - ny).clamp(0.0, room.max(0.0));
                } else if let Some(bo) = st.inset[2].resolve(viewport.1) {
                    let room = ny - margin[0] - c_top;
                    dy = -((ny + h) - (scroll.1 + viewport.1 - bo)).clamp(0.0, room.max(0.0));
                }
                if let Some(l) = st.inset[3].resolve(viewport.0) {
                    let room = (c_right - w - margin[1]) - nx;
                    dx = (scroll.0 + l - nx).clamp(0.0, room.max(0.0));
                } else if let Some(r) = st.inset[1].resolve(viewport.0) {
                    let room = nx - margin[3] - c_left;
                    dx = -((nx + w) - (scroll.0 + viewport.0 - r)).clamp(0.0, room.max(0.0));
                }
            }
            let b = &mut self.boxes[s.id as usize];
            let (x, y) = (s.base_x + dx, s.base_y + dy);
            if b.x != x || b.y != y {
                b.x = x;
                b.y = y;
                moved = true;
            }
        }
        moved
    }

    /// First box generated for a DOM node.
    pub fn box_for_node(&self, node: NodeId) -> BoxId {
        self.boxes
            .iter()
            .position(|b| b.node == node && b.flags & (F_ANON | F_BEFORE | F_AFTER) == 0 && b.kind != BoxKind::Marker)
            .map_or(NO_BOX, |i| i as BoxId)
    }
}

pub struct BoxChildren<'a> {
    tree: &'a LayoutTree,
    next: BoxId,
}

impl Iterator for BoxChildren<'_> {
    type Item = BoxId;
    fn next(&mut self) -> Option<BoxId> {
        if self.next == NO_BOX {
            return None;
        }
        let c = self.next;
        self.next = self.tree.boxes[c as usize].next;
        Some(c)
    }
}

pub struct LayoutInput<'a> {
    pub doc: &'a Document,
    pub styles: &'a mut StyleStore,
    pub fonts: &'a mut FontSystem,
    pub viewport_w: f32,
    pub viewport_h: f32,
    /// Intrinsic sizes of loaded images, by element.
    pub image_sizes: &'a HashMap<NodeId, (f32, f32)>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum WidthMode {
    /// Fill the containing block (block-level box in normal flow).
    Stretch,
    /// Shrink-to-fit (floats, inline-blocks, absolutely positioned boxes).
    FitContent,
    /// Border-box width imposed by the parent (flex/grid items, table cells).
    Fixed(f32),
}

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Edges {
    pub pad: [f32; 4],
    pub border: [f32; 4],
    pub margin: [f32; 4],
    pub auto_l: bool,
    pub auto_r: bool,
}

impl Edges {
    #[inline]
    pub fn pb_w(&self) -> f32 {
        self.pad[1] + self.pad[3] + self.border[1] + self.border[3]
    }
    #[inline]
    pub fn pb_h(&self) -> f32 {
        self.pad[0] + self.pad[2] + self.border[0] + self.border[2]
    }
    #[inline]
    pub fn m_w(&self) -> f32 {
        self.margin[1] + self.margin[3]
    }
    #[inline]
    pub fn m_h(&self) -> f32 {
        self.margin[0] + self.margin[2]
    }
}

// ───────────────────────────── floats ─────────────────────────────

#[derive(Clone, Copy, Debug)]
struct FRect {
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
}

/// Floats of one block formatting context, in that context's coordinate space.
#[derive(Default, Debug)]
pub(crate) struct FloatCtx {
    left: Vec<FRect>,
    right: Vec<FRect>,
}

impl FloatCtx {
    pub fn is_empty(&self) -> bool {
        self.left.is_empty() && self.right.is_empty()
    }

    /// Horizontal range free of floats for the vertical span `[y, y + h)` within `[x0, x1]`.
    pub fn band(&self, y: f32, h: f32, x0: f32, x1: f32) -> (f32, f32) {
        let (mut l, mut r) = (x0, x1);
        let y2 = y + h.max(0.01);
        for f in &self.left {
            if f.y0 < y2 && f.y1 > y && f.x1 > l {
                l = f.x1;
            }
        }
        for f in &self.right {
            if f.y0 < y2 && f.y1 > y && f.x0 < r {
                r = f.x0;
            }
        }
        (l, r.max(l))
    }

    /// The next y below `y` at which some float ends (where more room may open up).
    pub fn next_edge(&self, y: f32) -> Option<f32> {
        self.left.iter().chain(self.right.iter()).map(|f| f.y1).filter(|&b| b > y + 0.01).fold(None, |acc, b| {
            Some(match acc {
                Some(a) if a <= b => a,
                _ => b,
            })
        })
    }

    /// Place a float with margin-box size `w` x `h` at or below `y`. Returns its margin-box origin.
    pub fn place(&mut self, left: bool, w: f32, h: f32, mut y: f32, x0: f32, x1: f32) -> (f32, f32) {
        // floats may not be placed above earlier floats
        for f in self.left.iter().chain(self.right.iter()) {
            if f.y0 > y {
                y = f.y0;
            }
        }
        let mut guard = 0;
        loop {
            let (l, r) = self.band(y, h, x0, x1);
            if r - l >= w - 0.01 || guard > 200 {
                let x = if left { l } else { (r - w).max(l) };
                let rect = FRect { x0: x, x1: x + w, y0: y, y1: y + h };
                if left {
                    self.left.push(rect);
                } else {
                    self.right.push(rect);
                }
                return (x, y);
            }
            match self.next_edge(y) {
                Some(ny) => y = ny,
                None => {
                    let rect = FRect { x0, x1: x0 + w, y0: y, y1: y + h };
                    if left {
                        self.left.push(rect);
                    } else {
                        self.right.push(rect);
                    }
                    return (x0, y);
                }
            }
            guard += 1;
        }
    }

    pub fn clear_y(&self, clear: Clear) -> f32 {
        let m = |v: &Vec<FRect>| v.iter().map(|f| f.y1).fold(f32::NEG_INFINITY, f32::max);
        match clear {
            Clear::Left => m(&self.left),
            Clear::Right => m(&self.right),
            Clear::Both => m(&self.left).max(m(&self.right)),
            Clear::None => f32::NEG_INFINITY,
        }
    }

    pub fn bottom(&self) -> f32 {
        self.clear_y(Clear::Both)
    }
}

// ───────────────────────────── layout driver ─────────────────────────────

pub(crate) struct Layout<'a> {
    pub doc: &'a Document,
    pub styles: &'a mut StyleStore,
    pub fonts: &'a mut FontSystem,
    pub vw: f32,
    pub vh: f32,
    pub image_sizes: &'a HashMap<NodeId, (f32, f32)>,
    pub tree: LayoutTree,
    pub ifc_items: HashMap<BoxId, Rc<inline::Items>>,
    pub anon_styles: HashMap<(StyleId, Display), StyleId>,
}

#[inline]
pub(crate) fn collapse_margins(a: f32, b: f32) -> f32 {
    if a >= 0.0 && b >= 0.0 {
        a.max(b)
    } else if a < 0.0 && b < 0.0 {
        a.min(b)
    } else {
        a + b
    }
}

impl<'a> Layout<'a> {
    #[inline]
    pub fn st(&self, id: BoxId) -> Rc<ComputedStyle> {
        self.styles.rc(self.tree.boxes[id as usize].style)
    }

    #[inline]
    pub fn bx(&self, id: BoxId) -> &LBox {
        &self.tree.boxes[id as usize]
    }

    #[inline]
    pub fn bxm(&mut self, id: BoxId) -> &mut LBox {
        &mut self.tree.boxes[id as usize]
    }

    pub fn child_ids(&self, id: BoxId) -> Vec<BoxId> {
        self.tree.children(id).collect()
    }

    pub fn edges(&self, st: &ComputedStyle, cb_w: f32) -> Edges {
        let mut e = Edges::default();
        for i in 0..4 {
            e.pad[i] = st.padding[i].px(cb_w).max(0.0);
            e.border[i] = st.border_width[i];
            e.margin[i] = st.margin[i].px(cb_w);
        }
        e.auto_l = st.margin[3].is_auto();
        e.auto_r = st.margin[1].is_auto();
        e
    }

    /// Convert a specified width/height to a content-box size.
    fn content_size(st: &ComputedStyle, v: f32, pb: f32) -> f32 {
        if st.box_sizing == BoxSizing::BorderBox {
            (v - pb).max(0.0)
        } else {
            v.max(0.0)
        }
    }

    pub fn clamp_width(&self, st: &ComputedStyle, content_w: f32, cb_w: f32, pb: f32) -> f32 {
        let mut w = content_w;
        if let Some(mx) = st.max_width.resolve(cb_w) {
            w = w.min(Self::content_size(st, mx, pb));
        }
        if let Some(mn) = st.min_width.resolve(cb_w) {
            w = w.max(Self::content_size(st, mn, pb));
        }
        w.max(0.0)
    }

    pub fn clamp_height(&self, st: &ComputedStyle, content_h: f32, cb_h: Option<f32>, pb: f32) -> f32 {
        let mut h = content_h;
        if let Some(mx) = st.max_height.resolve_opt(cb_h) {
            h = h.min(Self::content_size(st, mx, pb));
        }
        if let Some(mn) = st.min_height.resolve_opt(cb_h) {
            h = h.max(Self::content_size(st, mn, pb));
        }
        h.max(0.0)
    }

    /// The box's specified content-box height, if definite.
    pub fn definite_height(&self, st: &ComputedStyle, cb_h: Option<f32>, pb: f32) -> Option<f32> {
        st.height.resolve_opt(cb_h).map(|h| Self::content_size(st, h, pb))
    }

    fn collapses_top(&self, id: BoxId) -> bool {
        let b = self.bx(id);
        if b.kind != BoxKind::Block || b.flags & (F_IFC | F_BFC) != 0 {
            return false;
        }
        let st = self.styles.style(b.style);
        st.border_width[0] == 0.0 && matches!(st.padding[0], Size::Px(p) if p == 0.0)
    }

    fn collapses_bottom(&self, id: BoxId) -> bool {
        let b = self.bx(id);
        if b.kind != BoxKind::Block || b.flags & (F_IFC | F_BFC) != 0 {
            return false;
        }
        let st = self.styles.style(b.style);
        st.border_width[2] == 0.0
            && matches!(st.padding[2], Size::Px(p) if p == 0.0)
            && st.height.is_auto()
            && matches!(st.min_height, Size::Auto | Size::Px(_))
            && !matches!(st.min_height, Size::Px(p) if p > 0.0)
    }

    fn first_in_flow_block(&self, id: BoxId) -> BoxId {
        let mut c = self.bx(id).first_child;
        while c != NO_BOX {
            let b = self.bx(c);
            if !b.is_neutral() {
                return c;
            }
            c = b.next;
        }
        NO_BOX
    }

    fn last_in_flow_block(&self, id: BoxId) -> BoxId {
        let mut last = NO_BOX;
        let mut c = self.bx(id).first_child;
        while c != NO_BOX {
            let b = self.bx(c);
            if !b.is_neutral() {
                last = c;
            }
            c = b.next;
        }
        last
    }

    /// Top margin a block presents to its container after collapsing with its first child.
    fn presented_top(&self, id: BoxId, cb_w: f32) -> f32 {
        let mut m = self.styles.style(self.bx(id).style).margin[0].px(cb_w);
        let mut cur = id;
        let mut depth = 0;
        while self.collapses_top(cur) && depth < 64 {
            let c = self.first_in_flow_block(cur);
            if c == NO_BOX {
                break;
            }
            m = collapse_margins(m, self.styles.style(self.bx(c).style).margin[0].px(cb_w));
            cur = c;
            depth += 1;
        }
        m
    }

    fn presented_bottom(&self, id: BoxId, cb_w: f32) -> f32 {
        let mut m = self.styles.style(self.bx(id).style).margin[2].px(cb_w);
        let mut cur = id;
        let mut depth = 0;
        while self.collapses_bottom(cur) && depth < 64 {
            let c = self.last_in_flow_block(cur);
            if c == NO_BOX {
                break;
            }
            m = collapse_margins(m, self.styles.style(self.bx(c).style).margin[2].px(cb_w));
            cur = c;
            depth += 1;
        }
        m
    }

    // ───────────── intrinsic widths ─────────────

    /// Min-content and max-content widths of a box's contents (content-box).
    pub fn intrinsic_content(&mut self, id: BoxId) -> (f32, f32) {
        let b = self.bx(id);
        if !b.iw_min.is_nan() {
            return (b.iw_min, b.iw_max);
        }
        let kind = b.kind;
        let flags = b.flags;
        let r = match kind {
            BoxKind::Replaced => {
                let (w, _) = self.replaced_size(id, 0.0, None, true);
                (w, w)
            }
            BoxKind::Table => table::intrinsic(self, id),
            BoxKind::Flex => flex::intrinsic(self, id),
            BoxKind::Grid => grid::intrinsic(self, id),
            _ if flags & F_IFC != 0 => inline::measure(self, id),
            _ => {
                let (mut mn, mut mx) = (0.0f32, 0.0f32);
                for c in self.child_ids(id) {
                    if self.bx(c).is_out_of_flow() || self.bx(c).kind == BoxKind::Marker {
                        continue;
                    }
                    let (a, b2) = self.intrinsic_outer(c);
                    mn = mn.max(a);
                    mx = mx.max(b2);
                }
                (mn, mx)
            }
        };
        let b = self.bxm(id);
        b.iw_min = r.0;
        b.iw_max = r.1;
        r
    }

    /// Min/max-content contribution of a box including its padding, border and margins.
    pub fn intrinsic_outer(&mut self, id: BoxId) -> (f32, f32) {
        let st = self.st(id);
        let e = self.edges(&st, 0.0);
        let extra = e.pb_w() + e.m_w();
        if self.bx(id).kind != BoxKind::Replaced && self.bx(id).kind != BoxKind::Table {
            if let Size::Px(w) = st.width {
                let cw = self.clamp_width(&st, Self::content_size(&st, w, e.pb_w()), 0.0, e.pb_w());
                return (cw + extra, cw + extra);
            }
        }
        let (mut mn, mut mx) = self.intrinsic_content(id);
        if let Size::Px(m) = st.max_width {
            let m = Self::content_size(&st, m, e.pb_w());
            mn = mn.min(m);
            mx = mx.min(m);
        }
        if let Size::Px(m) = st.min_width {
            let m = Self::content_size(&st, m, e.pb_w());
            mn = mn.max(m);
            mx = mx.max(m);
        }
        // scrollable / clipped boxes can shrink below their content
        if st.overflow_x != Overflow::Visible && self.bx(id).kind != BoxKind::Replaced {
            mn = 0.0;
        }
        (mn + extra, mx.max(mn) + extra)
    }

    // ───────────── replaced elements ─────────────

    fn replaced_intrinsic(&mut self, id: BoxId) -> (f32, f32) {
        let node = self.bx(id).node;
        let st = self.st(id);
        let tag = self.doc.tag(node);
        let lh = {
            let font = self.fonts.resolve(&st.font_family, st.font_weight, st.italic);
            let m = self.fonts.metrics(font, st.font_size);
            st.line_height_px(m.natural_line_height())
        };
        match tag {
            a::IMG => self.image_sizes.get(&node).copied().unwrap_or((0.0, 0.0)),
            a::INPUT => {
                let ty = self.doc.attr(node, a::TYPE).unwrap_or("text").to_ascii_lowercase();
                match ty.as_str() {
                    "checkbox" | "radio" => (13.0, 13.0),
                    "submit" | "button" | "reset" => {
                        let label = self.doc.attr(node, a::VALUE).map(|s| s.to_string()).unwrap_or_else(|| {
                            match ty.as_str() {
                                "submit" => "Submit".to_string(),
                                "reset" => "Reset".to_string(),
                                _ => String::new(),
                            }
                        });
                        let font = self.fonts.resolve(&st.font_family, st.font_weight, st.italic);
                        (self.fonts.measure(font, st.font_size, &label).ceil(), lh)
                    }
                    "image" => self.image_sizes.get(&node).copied().unwrap_or((0.0, 0.0)),
                    "range" => (129.0, 16.0),
                    "color" => (44.0, 23.0),
                    "file" => (st.font_size * 16.0, lh),
                    _ => (st.font_size * 11.0, lh),
                }
            }
            a::TEXTAREA => (st.font_size * 12.0, lh * 2.0),
            a::SELECT => {
                let font = self.fonts.resolve(&st.font_family, st.font_weight, st.italic);
                let mut w = 0.0f32;
                let opts: Vec<NodeId> = self.doc.descendants(node).filter(|&n| self.doc.tag(n) == a::OPTION).collect();
                for o in opts {
                    let text = self.doc.text_content(o);
                    w = w.max(self.fonts.measure(font, st.font_size, text.trim()));
                }
                (w.ceil().max(st.font_size * 2.0), lh)
            }
            a::SVG => {
                // viewBox gives the aspect ratio when no size is specified
                if let Some(vb) = self.doc.attr(node, a::VIEWBOX) {
                    let n: Vec<f32> = vb.split(|c: char| c == ',' || c.is_whitespace()).filter_map(|p| p.parse().ok()).collect();
                    if n.len() == 4 && n[2] > 0.0 && n[3] > 0.0 {
                        return (n[2], n[3]);
                    }
                }
                (300.0, 150.0)
            }
            a::AUDIO => {
                if self.doc.attr(node, crate::atom::Atom::new("controls")).is_some() {
                    (300.0, 54.0)
                } else {
                    (0.0, 0.0)
                }
            }
            _ => (300.0, 150.0),
        }
    }

    /// Used content-box size of a replaced element.
    pub fn replaced_size(&mut self, id: BoxId, cb_w: f32, cb_h: Option<f32>, intrinsic_only: bool) -> (f32, f32) {
        let st = self.st(id);
        let e = self.edges(&st, cb_w);
        let (iw, ih) = self.replaced_intrinsic(id);
        // only pictures have an intrinsic aspect ratio; a text field's height never follows its width
        let ratio = st.aspect_ratio.or(if self.has_intrinsic_ratio(id) && iw > 0.0 && ih > 0.0 { Some(iw / ih) } else { None });
        let w_spec = if intrinsic_only {
            match st.width {
                Size::Px(w) => Some(Self::content_size(&st, w, e.pb_w())),
                _ => None,
            }
        } else {
            st.width.resolve(cb_w).map(|w| Self::content_size(&st, w, e.pb_w()))
        };
        let h_spec = st.height.resolve_opt(cb_h).map(|h| Self::content_size(&st, h, e.pb_h()));
        let (mut w, mut h) = match (w_spec, h_spec) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, ratio.map_or(ih, |r| w / r)),
            (None, Some(h)) => (ratio.map_or(iw, |r| h * r), h),
            (None, None) => (iw, ih),
        };
        let max_w = if intrinsic_only {
            match st.max_width {
                Size::Px(m) => Some(m),
                _ => None,
            }
        } else {
            st.max_width.resolve(cb_w)
        };
        if let Some(mw) = max_w.map(|m| Self::content_size(&st, m, e.pb_w())) {
            if w > mw {
                if h_spec.is_none() {
                    if let Some(r) = ratio {
                        h = mw / r;
                    }
                }
                w = mw;
            }
        }
        if let Some(mw) = st.min_width.resolve(cb_w).map(|m| Self::content_size(&st, m, e.pb_w())) {
            if w < mw {
                if h_spec.is_none() {
                    if let Some(r) = ratio {
                        h = mw / r;
                    }
                }
                w = mw;
            }
        }
        if let Some(mh) = st.max_height.resolve_opt(cb_h).map(|m| Self::content_size(&st, m, e.pb_h())) {
            if h > mh {
                if w_spec.is_none() {
                    if let Some(r) = ratio {
                        w = mh * r;
                    }
                }
                h = mh;
            }
        }
        if let Some(mh) = st.min_height.resolve_opt(cb_h).map(|m| Self::content_size(&st, m, e.pb_h())) {
            if h < mh {
                h = mh;
            }
        }
        (w.max(0.0), h.max(0.0))
    }

    // ───────────── the main box layout routine ─────────────

    /// Lay out box `id`: computes its size, used margins/padding, and lays out its contents.
    /// `cb_x`/`top` give the containing block's content-left edge and this box's border-box top
    /// in the coordinate space of the enclosing block formatting context (for float avoidance).
    #[allow(clippy::too_many_arguments)]
    pub fn layout_box(
        &mut self,
        id: BoxId,
        cb_w: f32,
        cb_h: Option<f32>,
        mode: WidthMode,
        forced_h: Option<f32>,
        fc: Option<&mut FloatCtx>,
        cb_x: f32,
        top: f32,
    ) {
        let kind = self.bx(id).kind;
        let flags = self.bx(id).flags;
        let is_bfc = flags & F_BFC != 0 || !matches!(kind, BoxKind::Block);
        let shares_floats = !is_bfc && fc.as_ref().map_or(false, |f| !f.is_empty());

        // Re-layout with identical inputs is a no-op (flex/table measure-then-place passes).
        let mode_key = match mode {
            WidthMode::Stretch => -1.0,
            WidthMode::FitContent => -2.0,
            WidthMode::Fixed(w) => w,
        };
        let key = (cb_w, cb_h.unwrap_or(-1.0), forced_h.unwrap_or(-1.0), mode_key);
        if !shares_floats {
            let b = self.bx(id);
            if b.key_w == key.0 && b.key_h == key.1 && b.key_fh == key.2 && b.key_mode == key.3 {
                return;
            }
        }

        let st = self.st(id);
        let mut e = self.edges(&st, cb_w);
        let parent = self.bx(id).parent;

        match kind {
            BoxKind::Replaced => {
                let (mut w, mut h) = self.replaced_size(id, cb_w, cb_h, false);
                if let WidthMode::Fixed(fw) = mode {
                    let nw = (fw - e.pb_w()).max(0.0);
                    // an imposed width keeps the aspect ratio of pictures (not of form controls)
                    // unless the height is fixed too
                    if w > 0.0 && h > 0.0 && st.height.is_auto() && forced_h.is_none() && (nw - w).abs() > 0.01 && self.has_intrinsic_ratio(id) {
                        h = h * nw / w;
                    }
                    w = nw;
                }
                if let Some(fh) = forced_h {
                    h = (fh - e.pb_h()).max(0.0);
                }
                let b = self.bxm(id);
                b.w = w + e.pb_w();
                b.h = h + e.pb_h();
                b.baseline = f32::NAN;
            }
            BoxKind::Table => {
                table::layout_table(self, id, cb_w, cb_h, mode, &e);
            }
            _ => {
                // ── width ──
                let spec_w = st.width.resolve(cb_w).map(|w| Self::content_size(&st, w, e.pb_w()));
                let mut content_w = match mode {
                    WidthMode::Fixed(w) => (w - e.pb_w()).max(0.0),
                    _ => {
                        let w = match spec_w {
                            Some(w) => w,
                            None => {
                                let avail = (cb_w - e.m_w() - e.pb_w()).max(0.0);
                                let fit = mode == WidthMode::FitContent
                                    || matches!(st.width, Size::MinContent | Size::MaxContent | Size::FitContent);
                                if fit {
                                    let (mn, mx) = self.intrinsic_content(id);
                                    match st.width {
                                        Size::MinContent => mn,
                                        Size::MaxContent => mx,
                                        _ => mx.min(avail).max(mn),
                                    }
                                } else {
                                    avail
                                }
                            }
                        };
                        self.clamp_width(&st, w, cb_w, e.pb_w())
                    }
                };
                if !content_w.is_finite() {
                    content_w = 0.0;
                }
                let border_w = content_w + e.pb_w();

                // ── horizontal auto margins ──
                if mode == WidthMode::Stretch {
                    let free = cb_w - border_w - e.m_w();
                    if free > 0.0 {
                        let centered = parent != NO_BOX && self.styles.style(self.bx(parent).style).text_align == TextAlign::BlockCenter;
                        if e.auto_l && e.auto_r {
                            e.margin[3] = free / 2.0;
                            e.margin[1] = free / 2.0;
                        } else if e.auto_l {
                            e.margin[3] = free;
                        } else if e.auto_r {
                            e.margin[1] = free;
                        } else if centered {
                            e.margin[3] += free / 2.0;
                        }
                    }
                }

                // ── contents ──
                // In a table cell `height` is a minimum: content is never clipped by it.
                let cell_min_h = if kind == BoxKind::TableCell { self.definite_height(&st, cb_h, e.pb_h()) } else { None };
                let definite_h = match forced_h {
                    Some(fh) => Some((fh - e.pb_h()).max(0.0)),
                    None if kind == BoxKind::TableCell => None,
                    None => self.definite_height(&st, cb_h, e.pb_h()).map(|h| self.clamp_height(&st, h, cb_h, e.pb_h())),
                };
                {
                    let b = self.bxm(id);
                    b.w = border_w;
                    b.pad = e.pad;
                    b.margin = e.margin;
                    b.baseline = f32::NAN;
                }
                let mut own_fc = FloatCtx::default();
                let (fcr, ox, oy): (&mut FloatCtx, f32, f32) = match fc {
                    Some(f) if !is_bfc => (f, cb_x + e.margin[3], top),
                    _ => (&mut own_fc, 0.0, 0.0),
                };
                let mut content_h = match kind {
                    BoxKind::Flex => flex::layout_flex(self, id, content_w, definite_h, &e, cb_h),
                    BoxKind::Grid => grid::layout_grid(self, id, content_w, definite_h, &e),
                    _ if flags & F_IFC != 0 => inline::layout_inline(self, id, content_w, &e, fcr, ox, oy),
                    _ => self.layout_block_children(id, content_w, definite_h, &e, fcr, ox, oy),
                };
                if is_bfc && matches!(kind, BoxKind::Block | BoxKind::TableCell | BoxKind::TableCaption) {
                    // a BFC root grows to contain its floats
                    let fb = own_fc.bottom() - (e.border[0] + e.pad[0]);
                    if fb > content_h {
                        content_h = fb;
                    }
                }
                // ── height ──
                let h = match forced_h {
                    Some(fh) => (fh - e.pb_h()).max(0.0),
                    None => {
                        let h = match definite_h {
                            Some(h) => h,
                            None => match st.aspect_ratio {
                                Some(r) if st.height.is_auto() && r > 0.0 && spec_w.is_some() => (border_w / r - e.pb_h()).max(content_h),
                                _ => content_h,
                            },
                        };
                        self.clamp_height(&st, h.max(cell_min_h.unwrap_or(0.0)), cb_h, e.pb_h())
                    }
                };
                self.bxm(id).h = h + e.pb_h();
                // a <button> centres its content vertically whatever its display type
                if kind == BoxKind::Block && flags & F_ANON == 0 && h > content_h + 0.5 && self.doc.tag(self.bx(id).node) == a::BUTTON {
                    self.shift_contents(id, (h - content_h) / 2.0);
                }
                self.place_marker(id, &e);
            }
        }

        let b = self.bxm(id);
        if kind == BoxKind::Replaced || kind == BoxKind::Table {
            b.pad = e.pad;
            // auto margins for block-level replaced boxes and tables
            if mode == WidthMode::Stretch {
                let free = cb_w - b.w - e.m_w();
                if free > 0.0 {
                    if e.auto_l && e.auto_r {
                        e.margin[3] = free / 2.0;
                        e.margin[1] = free / 2.0;
                    } else if e.auto_l {
                        e.margin[3] = free;
                    }
                }
            }
            b.margin = e.margin;
        }
        b.key_w = key.0;
        b.key_h = key.1;
        b.key_fh = key.2;
        b.key_mode = key.3;
        if kind == BoxKind::Table && mode == WidthMode::Stretch && parent != NO_BOX {
            // <center> / align=center on the parent centres tables too
            let centered = self.styles.style(self.bx(parent).style).text_align == TextAlign::BlockCenter;
            let b = self.bxm(id);
            let free = cb_w - b.w - b.margin[1] - b.margin[3];
            if centered && free > 0.0 && !e.auto_l && !e.auto_r {
                b.margin[3] += free / 2.0;
            }
        }
    }

    /// Pictures scale proportionally; form controls and frames do not.
    fn has_intrinsic_ratio(&self, id: BoxId) -> bool {
        let node = self.bx(id).node;
        let tag = self.doc.tag(node);
        matches!(tag, a::IMG | a::SVG | a::VIDEO | a::CANVAS)
            || (tag == a::INPUT && self.doc.attr(node, a::TYPE).map_or(false, |t| t.eq_ignore_ascii_case("image")))
    }

    /// Move a box's in-flow contents down by `dy` (vertical alignment in table cells and buttons).
    pub(crate) fn shift_contents(&mut self, id: BoxId, dy: f32) {
        if dy.abs() < 0.01 {
            return;
        }
        for c in self.child_ids(id) {
            if !self.bx(c).is_out_of_flow() {
                self.bxm(c).y += dy;
            }
        }
        let (r0, r1, ifc) = {
            let b = self.bx(id);
            (b.r0 as usize, b.r1 as usize, b.flags & F_IFC != 0)
        };
        if ifc {
            for f in &mut self.tree.fragments[r0..r1] {
                f.y += dy;
                f.baseline += dy;
            }
        }
        let b = self.bxm(id);
        if !b.baseline.is_nan() {
            b.baseline += dy;
        }
    }

    /// Position the list marker of a list item (it hangs to the left of the first line).
    fn place_marker(&mut self, id: BoxId, e: &Edges) {
        let first = self.bx(id).first_child;
        if first == NO_BOX || self.bx(first).kind != BoxKind::Marker {
            return;
        }
        let st = self.st(first);
        let font = self.fonts.resolve(&st.font_family, st.font_weight, st.italic);
        let m = self.fonts.metrics(font, st.font_size);
        let base = self.bx(id).baseline;
        let (mw, _) = (self.bx(first).w, 0.0);
        let baseline_y = if base.is_nan() {
            e.border[0] + e.pad[0] + (st.line_height_px(m.natural_line_height()) - m.ascent - m.descent) / 2.0 + m.ascent
        } else {
            base
        };
        let b = self.bxm(first);
        b.x = e.border[3] + e.pad[3] - mw - st.font_size * 0.4;
        b.y = baseline_y - m.ascent;
        b.h = m.ascent + m.descent;
        b.baseline = m.ascent;
    }

    /// Lay out the block-level children of a block container. Returns the content height.
    #[allow(clippy::too_many_arguments)]
    fn layout_block_children(
        &mut self,
        id: BoxId,
        content_w: f32,
        content_h: Option<f32>,
        e: &Edges,
        fc: &mut FloatCtx,
        ox: f32,
        oy: f32,
    ) -> f32 {
        let cx = e.border[3] + e.pad[3];
        let cy = e.border[0] + e.pad[0];
        let collapse_top = self.collapses_top(id);
        let mut y = 0.0f32;
        let mut prev_mb = 0.0f32;
        let mut first = true;
        let mut c = self.bx(id).first_child;
        while c != NO_BOX {
            let (next, flags, kind) = {
                let b = self.bx(c);
                (b.next, b.flags, b.kind)
            };
            if kind == BoxKind::Marker {
                c = next;
                continue;
            }
            if flags & F_OUT_OF_FLOW != 0 {
                // remember the static position; sized and placed in the positioned pass
                let sy = cy + y + if first { 0.0 } else { prev_mb.max(0.0) };
                let b = self.bxm(c);
                b.x = cx;
                b.y = sy;
                c = next;
                continue;
            }
            let cst = self.st(c);
            if flags & F_FLOAT != 0 {
                self.layout_box(c, content_w, content_h, WidthMode::FitContent, None, None, 0.0, 0.0);
                let (bw, bh, m) = {
                    let b = self.bx(c);
                    (b.w, b.h, b.margin)
                };
                let fy = oy + cy + y + if first { 0.0 } else { prev_mb.max(0.0) };
                let (fx, fy2) = fc.place(cst.float == Float::Left, bw + m[1] + m[3], bh + m[0] + m[2], fy, ox + cx, ox + cx + content_w);
                let b = self.bxm(c);
                b.x = fx - ox + m[3];
                b.y = fy2 - oy + m[0];
                c = next;
                continue;
            }

            let mt = self.presented_top(c, content_w);
            let mb = self.presented_bottom(c, content_w);
            let mut top = if first {
                if collapse_top {
                    0.0
                } else {
                    mt
                }
            } else {
                y + collapse_margins(prev_mb, mt)
            };
            if cst.clear != Clear::None {
                let clear_y = fc.clear_y(cst.clear) - (oy + cy);
                if clear_y > top {
                    top = clear_y;
                }
            }
            let child_bfc = flags & F_BFC != 0 || kind != BoxKind::Block;
            let (mut ax, mut aw) = (0.0f32, content_w);
            if child_bfc && !fc.is_empty() {
                // a new formatting context sits beside floats instead of under them
                let mut ty = oy + cy + top;
                let mut guard = 0;
                loop {
                    let (l, r) = fc.band(ty, 1.0, ox + cx, ox + cx + content_w);
                    let need = if cst.width.is_auto() { 40.0f32.min(content_w) } else { cst.width.resolve(content_w).unwrap_or(0.0).min(content_w) };
                    if r - l >= need || guard > 50 {
                        ax = l - (ox + cx);
                        aw = r - l;
                        break;
                    }
                    match fc.next_edge(ty) {
                        Some(ny) => ty = ny,
                        None => break,
                    }
                    guard += 1;
                }
                top = ty - (oy + cy);
            }
            self.layout_box(c, aw, content_h, WidthMode::Stretch, None, Some(&mut *fc), ox + cx + ax, oy + cy + top);
            let (bh, base) = {
                let b = self.bxm(c);
                b.x = cx + ax + b.margin[3];
                b.y = cy + top;
                (b.h, b.baseline)
            };
            self.apply_relative(c, &cst, content_w, content_h);
            if !base.is_nan() && self.bx(id).baseline.is_nan() {
                self.bxm(id).baseline = cy + top + base;
            }
            y = top + bh;
            prev_mb = mb;
            first = false;
            c = next;
        }
        if first {
            return 0.0;
        }
        if self.collapses_bottom(id) {
            y.max(0.0)
        } else {
            (y + prev_mb).max(0.0)
        }
    }

    /// Apply `position: relative` offsets to an already placed box.
    pub fn apply_relative(&mut self, id: BoxId, st: &ComputedStyle, cb_w: f32, cb_h: Option<f32>) {
        if !matches!(st.position, Position::Relative) {
            return;
        }
        let dx = match (st.inset[3].resolve(cb_w), st.inset[1].resolve(cb_w)) {
            (Some(l), _) => l,
            (None, Some(r)) => -r,
            _ => 0.0,
        };
        let dy = match (st.inset[0].resolve_opt(cb_h), st.inset[2].resolve_opt(cb_h)) {
            (Some(t), _) => t,
            (None, Some(b)) => -b,
            _ => 0.0,
        };
        let b = self.bxm(id);
        b.x += dx;
        b.y += dy;
    }

    // ───────────── absolutely positioned boxes ─────────────

    /// Second pass: size and place `position: absolute/fixed` boxes now that every containing
    /// block has its final size.
    fn positioned_pass(&mut self, id: BoxId, cb: BoxId, off_x: f32, off_y: f32, abs_x: f32, abs_y: f32) {
        let mut c = self.bx(id).first_child;
        while c != NO_BOX {
            let (next, flags, kind) = {
                let b = self.bx(c);
                (b.next, b.flags, b.kind)
            };
            if flags & F_OUT_OF_FLOW != 0 {
                let st = self.st(c);
                let fixed = st.position == Position::Fixed;
                // containing block rectangle, relative to the cb box origin (or the viewport)
                let (cbx, cby, cbw, cbh) = if fixed {
                    (0.0, 0.0, self.vw, self.vh)
                } else {
                    let b = self.bx(cb);
                    let bs = self.styles.style(b.style);
                    let (bl, bt, br, bb) = (bs.border_width[3], bs.border_width[0], bs.border_width[1], bs.border_width[2]);
                    (bl, bt, (b.w - bl - br).max(0.0), (b.h - bt - bb).max(0.0))
                };
                let (sx, sy) = {
                    let b = self.bx(c);
                    if fixed {
                        (abs_x + b.x, abs_y + b.y)
                    } else {
                        (off_x + b.x - cbx, off_y + b.y - cby)
                    }
                };
                let (px, py) = self.layout_abs(c, &st, cbw, cbh, sx, sy);
                let b = self.bxm(c);
                if fixed {
                    b.x = px - abs_x;
                    b.y = py - abs_y;
                    b.flags |= F_FIXED;
                } else {
                    b.x = px + cbx - off_x;
                    b.y = py + cby - off_y;
                }
                let (nx, ny) = (abs_x + self.bx(c).x, abs_y + self.bx(c).y);
                self.positioned_pass(c, c, 0.0, 0.0, nx, ny);
            } else {
                let (bx, by) = {
                    let b = self.bx(c);
                    (b.x, b.y)
                };
                let establishes = flags & F_POSITIONED != 0 && !matches!(kind, BoxKind::Inline | BoxKind::Text | BoxKind::Break);
                if establishes {
                    self.positioned_pass(c, c, 0.0, 0.0, abs_x + bx, abs_y + by);
                } else {
                    self.positioned_pass(c, cb, off_x + bx, off_y + by, abs_x + bx, abs_y + by);
                }
            }
            c = next;
        }
    }

    /// Size an absolutely positioned box and return its border-box origin in containing-block
    /// (padding box) coordinates.
    fn layout_abs(&mut self, id: BoxId, st: &ComputedStyle, cbw: f32, cbh: f32, static_x: f32, static_y: f32) -> (f32, f32) {
        let e = self.edges(st, cbw);
        let left = st.inset[3].resolve(cbw);
        let right = st.inset[1].resolve(cbw);
        let top = st.inset[0].resolve(cbh);
        let bottom = st.inset[2].resolve(cbh);
        let kind = self.bx(id).kind;

        let stretch_w = left.is_some() && right.is_some() && st.width.is_auto() && kind != BoxKind::Replaced && kind != BoxKind::Table;
        let mode = if stretch_w {
            WidthMode::Fixed((cbw - left.unwrap() - right.unwrap() - e.m_w()).max(0.0))
        } else {
            WidthMode::FitContent
        };
        let avail_w = match (left, right) {
            (Some(l), None) => (cbw - l).max(0.0),
            (None, Some(r)) => (cbw - r).max(0.0),
            (None, None) => (cbw - static_x.max(0.0)).max(0.0),
            _ => cbw,
        };
        let forced_h = if top.is_some() && bottom.is_some() && st.height.is_auto() && kind != BoxKind::Replaced {
            Some((cbh - top.unwrap() - bottom.unwrap() - e.m_h()).max(0.0))
        } else {
            None
        };
        // Percent widths resolve against the containing block; shrink-to-fit uses what is left.
        let cb_for_layout = if st.width.is_auto() && !stretch_w { avail_w } else { cbw };
        self.layout_box(id, cb_for_layout, Some(cbh), mode, forced_h, None, 0.0, 0.0);
        let (w, h) = (self.bx(id).w, self.bx(id).h);

        let x = match (left, right) {
            (Some(l), Some(r)) => {
                let free = cbw - l - r - w;
                if e.auto_l && e.auto_r {
                    l + (free / 2.0).max(0.0)
                } else if e.auto_l {
                    l + (free - e.margin[1]).max(0.0)
                } else {
                    l + e.margin[3]
                }
            }
            (Some(l), None) => l + e.margin[3],
            (None, Some(r)) => cbw - r - e.margin[1] - w,
            (None, None) => static_x + e.margin[3],
        };
        let y = match (top, bottom) {
            (Some(t), Some(b)) => {
                let free = cbh - t - b - h;
                let auto_t = st.margin[0].is_auto();
                let auto_b = st.margin[2].is_auto();
                if auto_t && auto_b {
                    t + (free / 2.0).max(0.0)
                } else if auto_t {
                    t + (free - e.margin[2]).max(0.0)
                } else {
                    t + e.margin[0]
                }
            }
            (Some(t), None) => t + e.margin[0],
            (None, Some(b)) => cbh - b - e.margin[2] - h,
            (None, None) => static_y + e.margin[0],
        };
        (x, y)
    }

    /// Compute the scrollable extent of the document.
    fn extent(&self, id: BoxId, ox: f32, oy: f32, max: &mut (f32, f32)) {
        let b = self.bx(id);
        if b.flags & F_FIXED != 0 {
            return;
        }
        let x = ox + b.x;
        let y = oy + b.y;
        if !matches!(b.kind, BoxKind::Inline | BoxKind::Text | BoxKind::Break) {
            let st = self.styles.style(b.style);
            if st.display != Display::None && (b.w > 0.0 || b.h > 0.0) {
                max.0 = max.0.max(x + b.w);
                max.1 = max.1.max(y + b.h + b.margin[2].max(0.0));
            }
            if b.flags & F_CLIP != 0 && id != self.tree.root {
                return;
            }
        }
        let mut c = b.first_child;
        while c != NO_BOX {
            self.extent(c, x, y, max);
            c = self.bx(c).next;
        }
    }

    /// Drop fragments orphaned by re-layout so the final tree holds only live ones.
    fn compact_fragments(&mut self) {
        let mut out: Vec<Fragment> = Vec::with_capacity(self.tree.fragments.len() / 2 + 16);
        for b in self.tree.boxes.iter_mut() {
            if b.flags & F_IFC != 0 && b.r1 > b.r0 {
                let start = out.len() as u32;
                out.extend_from_slice(&self.tree.fragments[b.r0 as usize..b.r1 as usize]);
                b.r0 = start;
                b.r1 = out.len() as u32;
            }
        }
        out.shrink_to_fit();
        self.tree.fragments = out;
    }
}

/// Lay out a styled document for the given viewport.
pub fn layout_document(input: LayoutInput) -> LayoutTree {
    let mut l = Layout {
        doc: input.doc,
        styles: input.styles,
        fonts: input.fonts,
        vw: input.viewport_w,
        vh: input.viewport_h,
        image_sizes: input.image_sizes,
        tree: LayoutTree::empty(),
        ifc_items: HashMap::new(),
        anon_styles: HashMap::new(),
    };
    build::build_tree(&mut l);
    let root = l.tree.root;
    if root == NO_BOX {
        l.tree.width = l.vw;
        l.tree.height = 0.0;
        return l.tree;
    }
    let (vw, vh) = (l.vw, l.vh);
    l.layout_box(root, vw, Some(vh), WidthMode::Stretch, None, None, 0.0, 0.0);
    {
        let st = l.st(root);
        let mt = st.margin[0].px(vw);
        let b = l.bxm(root);
        b.x = b.margin[3];
        b.y = mt;
    }
    let (rx, ry) = (l.bx(root).x, l.bx(root).y);
    l.positioned_pass(root, root, 0.0, 0.0, rx, ry);
    let mut max = (vw, 0.0f32);
    l.extent(root, 0.0, 0.0, &mut max);
    l.compact_fragments();
    l.tree.width = max.0;
    l.tree.height = max.1;
    l.tree.boxes.shrink_to_fit();
    l.tree.glyph_base = l.tree.glyphs.len() as u32;
    for (i, b) in l.tree.boxes.iter().enumerate() {
        if b.flags & F_POSITIONED != 0
            && !matches!(b.kind, BoxKind::Inline | BoxKind::Text | BoxKind::Break)
            && l.styles.style(b.style).position == Position::Sticky
        {
            l.tree.sticky.push(StickyBox { id: i as BoxId, base_x: b.x, base_y: b.y });
        }
    }
    l.tree
}

pub(crate) fn new_box(kind: BoxKind, node: NodeId, style: StyleId, flags: u16) -> LBox {
    LBox {
        kind,
        flags,
        node,
        style,
        parent: NO_BOX,
        first_child: NO_BOX,
        last_child: NO_BOX,
        next: NO_BOX,
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
        margin: [0.0; 4],
        pad: [0.0; 4],
        baseline: f32::NAN,
        r0: 0,
        r1: 0,
        aux: 0,
        iw_min: f32::NAN,
        iw_max: f32::NAN,
        key_w: f32::NAN,
        key_h: f32::NAN,
        key_fh: f32::NAN,
        key_mode: f32::NAN,
    }
}

impl LayoutTree {
    /// Find the deepest box under a point (document coordinates). Returns the box and, for text,
    /// the fragment index.
    pub fn hit_test(&self, styles: &StyleStore, px: f32, py: f32, scroll_x: f32, scroll_y: f32) -> Option<BoxId> {
        if self.root == NO_BOX {
            return None;
        }
        let mut best: Option<(i32, u32, BoxId)> = None;
        let mut seq = 0u32;
        self.hit(styles, self.root, 0.0, 0.0, px, py, scroll_x, scroll_y, 0, &mut seq, &mut best, false);
        best.map(|b| b.2)
    }

    #[allow(clippy::too_many_arguments)]
    fn hit(
        &self,
        styles: &StyleStore,
        id: BoxId,
        ox: f32,
        oy: f32,
        px: f32,
        py: f32,
        sx: f32,
        sy: f32,
        z: i32,
        seq: &mut u32,
        best: &mut Option<(i32, u32, BoxId)>,
        in_fixed: bool,
    ) {
        let b = &self.boxes[id as usize];
        let st = styles.style(b.style);
        let mut z = z;
        if b.flags & (F_POSITIONED | F_OUT_OF_FLOW) != 0 {
            z = z.max(st.z_index.unwrap_or(0)).max(if b.flags & F_OUT_OF_FLOW != 0 { 1 } else { 0 });
        }
        let fixed_now = b.flags & F_FIXED != 0 && !in_fixed;
        let (mut x, mut y) = (ox + b.x, oy + b.y);
        if fixed_now {
            x += sx;
            y += sy;
        }
        let has_geom = !matches!(b.kind, BoxKind::Inline | BoxKind::Text | BoxKind::Break);
        let (mut px, mut py) = (px, py);
        if let Some(m) = st.transform_matrix(b.w, b.h) {
            if crate::style::is_translation(&m) {
                x += m[4];
                y += m[5];
            } else if has_geom {
                // look at the point in the box's own, untransformed space
                use crate::draw::{affine_inverse, affine_mul, affine_point};
                let world = affine_mul(&affine_mul(&[1.0, 0.0, 0.0, 1.0, x, y], &m), &[1.0, 0.0, 0.0, 1.0, -x, -y]);
                match affine_inverse(&world) {
                    Some(inv) => (px, py) = affine_point(&inv, px, py),
                    None => return,
                }
            }
        }
        let inside = has_geom && px >= x && px < x + b.w && py >= y && py < y + b.h;
        if has_geom && b.flags & F_CLIP != 0 && !inside {
            return;
        }
        let visible = st.visibility == crate::style::Visibility::Visible && st.pointer_events;
        if inside && visible {
            *seq += 1;
            if best.map_or(true, |(bz, bs, _)| (z, *seq) >= (bz, bs)) {
                *best = Some((z, *seq, id));
            }
        }
        if b.flags & F_IFC != 0 {
            for f in &self.fragments[b.r0 as usize..b.r1 as usize] {
                if f.kind == FragKind::Atomic {
                    continue;
                }
                let (fx, fy) = (x + f.x, y + f.y);
                if px >= fx && px < fx + f.w && py >= fy && py < fy + f.h {
                    let fb = &self.boxes[f.box_id as usize];
                    let fst = styles.style(fb.style);
                    if fst.visibility == crate::style::Visibility::Visible && fst.pointer_events {
                        *seq += 1;
                        if best.map_or(true, |(bz, bs, _)| (z, *seq) >= (bz, bs)) {
                            *best = Some((z, *seq, f.box_id));
                        }
                    }
                }
            }
        }
        let mut c = b.first_child;
        while c != NO_BOX {
            self.hit(styles, c, x, y, px, py, sx, sy, z, seq, best, in_fixed || fixed_now);
            c = self.boxes[c as usize].next;
        }
    }
}
