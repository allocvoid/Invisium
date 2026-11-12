//! Inline formatting: text shaping into words, line breaking, baseline alignment, fragments.
//!
//! Text is shaped once per block (the item list is width-independent and cached), so the
//! measure-then-place passes of flex and table layout do not re-shape anything.

use super::*;
use crate::style::{BorderStyle, TextTransform, VAlign};
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ItemKind {
    Word,
    Space,
    Open,
    Close,
    Atomic,
    Break,
    Float,
    Abs,
}

const I_BREAK_BEFORE: u8 = 1;
const I_PRESERVED: u8 = 2;
const I_NOBREAK: u8 = 4;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Item {
    pub kind: ItemKind,
    pub flags: u8,
    pub box_id: BoxId,
    pub g0: u32,
    pub g1: u32,
    pub w: f32,
    /// Open: left margin; Close: right margin.
    pub aux: f32,
    pub run: u32,
}

pub(crate) struct Run {
    pub ascent: f32,
    pub descent: f32,
    /// Half-leading model: distance above/below the baseline this run needs on a line.
    pub above: f32,
    pub below: f32,
    /// Baseline shift (positive = up) from vertical-align on inline ancestors.
    pub shift: f32,
    pub break_word: bool,
}

pub(crate) struct Items {
    pub items: Vec<Item>,
    pub runs: Vec<Run>,
}

fn is_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}')
}

fn is_cjk(ch: char) -> bool {
    let c = ch as u32;
    (0x2E80..=0x9FFF).contains(&c)
        || (0xAC00..=0xD7AF).contains(&c)
        || (0xF900..=0xFAFF).contains(&c)
        || (0xFF00..=0xFFEF).contains(&c)
        || (0x20000..=0x3FFFF).contains(&c)
}

struct Collector<'x, 'a> {
    l: &'x mut Layout<'a>,
    items: Vec<Item>,
    runs: Vec<Run>,
    prev_space: bool,
    break_next: bool,
    col: usize,
}

impl Collector<'_, '_> {
    fn push(&mut self, kind: ItemKind, box_id: BoxId, flags: u8, w: f32, aux: f32) {
        self.items.push(Item { kind, flags, box_id, g0: 0, g1: 0, w, aux, run: 0 });
    }

    fn text(&mut self, id: BoxId, shift: f32) {
        let st = self.l.st(id);
        let b = self.l.bx(id);
        let doc = self.l.doc;
        let raw: Cow<str> = if b.flags & F_GEN_TEXT != 0 {
            Cow::Owned(self.l.tree.gen_text[b.aux as usize].clone())
        } else {
            Cow::Borrowed(doc.text(b.node).unwrap_or(""))
        };
        let text: Cow<str> = match st.text_transform {
            TextTransform::None => raw,
            TextTransform::Uppercase => Cow::Owned(raw.to_uppercase()),
            TextTransform::Lowercase => Cow::Owned(raw.to_lowercase()),
            TextTransform::Capitalize => {
                let mut out = String::with_capacity(raw.len());
                let mut start = true;
                for ch in raw.chars() {
                    if start && ch.is_alphabetic() {
                        out.extend(ch.to_uppercase());
                    } else {
                        out.push(ch);
                    }
                    start = !ch.is_alphanumeric() && ch != '\'';
                }
                Cow::Owned(out)
            }
        };
        let font = self.l.fonts.resolve(&st.font_family, st.font_weight, st.italic);
        let m = self.l.fonts.metrics(font, st.font_size);
        let lh = st.line_height_px(m.natural_line_height());
        let above = m.ascent + (lh - m.ascent - m.descent) / 2.0;
        let run = self.runs.len() as u32;
        self.runs.push(Run {
            ascent: m.ascent,
            descent: m.descent,
            above,
            below: lh - above,
            shift,
            break_word: st.overflow_wrap != crate::style::OverflowWrap::Normal || st.break_all,
        });
        let preserve_ws = st.preserves_ws();
        let preserve_nl = st.preserves_newlines();
        let wraps = st.wraps();
        let break_all = st.break_all && wraps;
        let size = st.font_size;
        let ls = st.letter_spacing;
        let ws = st.word_spacing;
        let space_flags = if wraps { 0 } else { I_NOBREAK };

        let mut word = String::new();
        macro_rules! flush {
            () => {
                if !word.is_empty() {
                    let g0 = self.l.tree.glyphs.len() as u32;
                    let w = self.l.fonts.shape(font, size, &word, ls, &mut self.l.tree.glyphs);
                    let g1 = self.l.tree.glyphs.len() as u32;
                    let flags = if self.break_next && wraps { I_BREAK_BEFORE } else { 0 };
                    self.items.push(Item { kind: ItemKind::Word, flags, box_id: id, g0, g1, w, aux: 0.0, run });
                    self.break_next = false;
                    self.col += word.chars().count();
                    word.clear();
                }
            };
        }
        for ch in text.chars() {
            if ch == '\n' && preserve_nl {
                flush!();
                self.items.push(Item { kind: ItemKind::Break, flags: 0, box_id: id, g0: 0, g1: 0, w: 0.0, aux: 0.0, run });
                self.prev_space = true;
                self.break_next = false;
                self.col = 0;
                continue;
            }
            if is_space(ch) {
                flush!();
                if preserve_ws {
                    let n = if ch == '\t' { 8 - (self.col % 8) } else { 1 };
                    let g0 = self.l.tree.glyphs.len() as u32;
                    let mut w = 0.0;
                    for _ in 0..n {
                        w += self.l.fonts.shape(font, size, " ", ls, &mut self.l.tree.glyphs) + ws;
                    }
                    let g1 = self.l.tree.glyphs.len() as u32;
                    self.items.push(Item { kind: ItemKind::Space, flags: I_PRESERVED | space_flags, box_id: id, g0, g1, w, aux: 0.0, run });
                    self.col += n;
                    self.prev_space = false;
                } else if !self.prev_space {
                    let g0 = self.l.tree.glyphs.len() as u32;
                    let w = self.l.fonts.shape(font, size, " ", ls, &mut self.l.tree.glyphs) + ws;
                    let g1 = self.l.tree.glyphs.len() as u32;
                    self.items.push(Item { kind: ItemKind::Space, flags: space_flags, box_id: id, g0, g1, w, aux: 0.0, run });
                    self.prev_space = true;
                    self.col += 1;
                }
                self.break_next = false;
                continue;
            }
            self.prev_space = false;
            if is_cjk(ch) || break_all {
                flush!();
                self.break_next = true;
                word.push(ch);
                flush!();
                self.break_next = true;
                continue;
            }
            if ch == '\u{200b}' {
                flush!();
                self.break_next = true;
                continue;
            }
            word.push(ch);
            if (ch == '-' && word.chars().count() > 1) || ch == '\u{ad}' {
                flush!();
                self.break_next = true;
            }
        }
        flush!();
    }

    fn walk(&mut self, id: BoxId, shift: f32) {
        let mut c = self.l.bx(id).first_child;
        while c != NO_BOX {
            let (next, kind, flags) = {
                let b = self.l.bx(c);
                (b.next, b.kind, b.flags)
            };
            match kind {
                BoxKind::Marker => {}
                BoxKind::Text => self.text(c, shift),
                BoxKind::Break => {
                    self.push(ItemKind::Break, c, 0, 0.0, 0.0);
                    self.prev_space = true;
                    self.break_next = false;
                    self.col = 0;
                }
                BoxKind::Inline => {
                    let st = self.l.st(c);
                    let e = self.l.edges(&st, 0.0);
                    let s2 = shift
                        + match st.vertical_align {
                            VAlign::Sub => -st.font_size * 0.25,
                            VAlign::Super => st.font_size * 0.35,
                            VAlign::Length(v) => v,
                            _ => 0.0,
                        };
                    self.push(ItemKind::Open, c, 0, e.margin[3] + e.border[3] + e.pad[3], e.margin[3]);
                    self.walk(c, s2);
                    self.push(ItemKind::Close, c, 0, e.pad[1] + e.border[1] + e.margin[1], e.margin[1]);
                }
                _ => {
                    if flags & F_OUT_OF_FLOW != 0 {
                        self.push(ItemKind::Abs, c, I_BREAK_BEFORE, 0.0, 0.0);
                    } else if flags & F_FLOAT != 0 {
                        self.push(ItemKind::Float, c, I_BREAK_BEFORE, 0.0, 0.0);
                    } else {
                        let parent_wraps = self.l.st(id).wraps();
                        self.push(ItemKind::Atomic, c, if parent_wraps { I_BREAK_BEFORE } else { 0 }, 0.0, 0.0);
                        self.prev_space = false;
                        self.break_next = parent_wraps;
                        self.col += 1;
                    }
                }
            }
            c = next;
        }
    }
}

fn items_for(l: &mut Layout, root: BoxId) -> Rc<Items> {
    if let Some(it) = l.ifc_items.get(&root) {
        return it.clone();
    }
    let mut c = Collector { l, items: Vec::new(), runs: Vec::new(), prev_space: true, break_next: false, col: 0 };
    c.walk(root, 0.0);
    let items = Rc::new(Items { items: c.items, runs: c.runs });
    l.ifc_items.insert(root, items.clone());
    items
}

/// Min-content and max-content widths of an inline formatting context.
pub(crate) fn measure(l: &mut Layout, root: BoxId) -> (f32, f32) {
    let items = items_for(l, root);
    let (mut min, mut max) = (0.0f32, 0.0f32);
    let (mut chunk, mut line) = (0.0f32, 0.0f32);
    for it in &items.items {
        let breakable_space = it.kind == ItemKind::Space && it.flags & I_NOBREAK == 0;
        if it.flags & I_BREAK_BEFORE != 0 || breakable_space || it.kind == ItemKind::Break {
            min = min.max(chunk);
            chunk = 0.0;
        }
        match it.kind {
            ItemKind::Break => {
                max = max.max(line);
                line = 0.0;
            }
            ItemKind::Space => {
                line += it.w;
                if !breakable_space {
                    chunk += it.w;
                }
            }
            ItemKind::Atomic | ItemKind::Float => {
                let (mn, mx) = l.intrinsic_outer(it.box_id);
                chunk += mn;
                line += mx;
                min = min.max(chunk);
                chunk = 0.0;
            }
            ItemKind::Abs => {}
            _ => {
                chunk += it.w;
                line += it.w;
            }
        }
    }
    min = min.max(chunk);
    max = max.max(line);
    (min.ceil(), max.ceil())
}

#[derive(Clone, Copy)]
struct LineItem {
    idx: usize,
    g0: u32,
    g1: u32,
    w: f32,
}

struct Builder<'x, 'a, 'f> {
    l: &'x mut Layout<'a>,
    fc: &'f mut FloatCtx,
    items: Rc<Items>,
    st: Rc<ComputedStyle>,
    cx: f32,
    cy: f32,
    ox: f32,
    oy: f32,
    content_w: f32,
    strut_above: f32,
    strut_below: f32,
    strut_asc: f32,
    strut_desc: f32,
    strut_xh: f32,
    line: Vec<LineItem>,
    line_w: f32,
    has_content: bool,
    y: f32,
    first_line: bool,
    first_baseline: f32,
    x0: f32,
    avail: f32,
    /// Inline boxes still open when the current line started.
    open: Vec<BoxId>,
    pending_floats: Vec<BoxId>,
    /// This block's fragments. They are collected here and appended to the tree in one piece at
    /// the end: nested blocks (inline-blocks, floats) lay themselves out while we are still
    /// building lines, and their fragments must not land inside our range.
    frags: Vec<Fragment>,
}

fn needs_fragment(st: &ComputedStyle) -> bool {
    !st.background_color.is_transparent()
        || st.background.is_some()
        || st.has_visible_border()
        || st.box_shadow.is_some()
        || (st.outline_width > 0.0 && st.outline_style != BorderStyle::None)
}

impl Builder<'_, '_, '_> {
    fn update_band(&mut self) {
        if self.fc.is_empty() {
            self.x0 = 0.0;
            self.avail = self.content_w;
        } else {
            let left = self.ox + self.cx;
            let (a, b) = self.fc.band(self.oy + self.cy + self.y, self.strut_above + self.strut_below, left, left + self.content_w);
            self.x0 = a - left;
            self.avail = b - a;
        }
    }

    fn place_float(&mut self, b: BoxId) {
        let st = self.l.st(b);
        self.l.layout_box(b, self.content_w, None, WidthMode::FitContent, None, None, 0.0, 0.0);
        let (bw, bh, m) = {
            let bx = self.l.bx(b);
            (bx.w, bx.h, bx.margin)
        };
        let left = self.ox + self.cx;
        let (fx, fy) = self.fc.place(
            st.float == Float::Left,
            bw + m[1] + m[3],
            bh + m[0] + m[2],
            self.oy + self.cy + self.y,
            left,
            left + self.content_w,
        );
        let bx = self.l.bxm(b);
        bx.x = fx - self.ox + m[3];
        bx.y = fy - self.oy + m[0];
    }

    /// Emit the current line as fragments and advance to the next one. `forced` = the line ends
    /// at a hard break; `last` = it is the final line of the block.
    fn finish(&mut self, forced: bool, last: bool) {
        let items = self.items.clone();
        // trailing collapsible space does not take part in the line
        while let Some(last) = self.line.last() {
            let it = &items.items[last.idx];
            if it.kind == ItemKind::Space && it.flags & I_PRESERVED == 0 {
                self.line_w -= last.w;
                self.line.pop();
            } else {
                break;
            }
        }
        let line = std::mem::take(&mut self.line);
        // A line with no text, no atomic boxes and no inline box edges has zero height.
        let visible = line.iter().any(|li| {
            let it = &items.items[li.idx];
            match it.kind {
                ItemKind::Word | ItemKind::Atomic => true,
                ItemKind::Space => it.flags & I_PRESERVED != 0,
                ItemKind::Open | ItemKind::Close => it.w != 0.0,
                _ => false,
            }
        });
        if !visible && !forced {
            let (sx, sy) = (self.cx + self.x0, self.cy + self.y);
            for li in &line {
                let it = &items.items[li.idx];
                match it.kind {
                    ItemKind::Open => self.open.push(it.box_id),
                    ItemKind::Close => {
                        if let Some(pos) = self.open.iter().rposition(|b| *b == it.box_id) {
                            self.open.remove(pos);
                        }
                    }
                    ItemKind::Abs => {
                        let b = self.l.bxm(it.box_id);
                        b.x = sx;
                        b.y = sy;
                    }
                    _ => {}
                }
            }
            self.line = line;
            self.line.clear();
            self.line_w = 0.0;
            self.has_content = false;
            return;
        }
        // ── vertical metrics ──
        let mut above = self.strut_above;
        let mut below = self.strut_below;
        let mut edge_h = 0.0f32;
        for li in &line {
            let it = &items.items[li.idx];
            match it.kind {
                ItemKind::Word | ItemKind::Space => {
                    let r = &items.runs[it.run as usize];
                    above = above.max(r.above + r.shift);
                    below = below.max(r.below - r.shift);
                }
                ItemKind::Atomic => {
                    let b = self.l.bx(it.box_id);
                    let mh = b.h + b.margin[0] + b.margin[2];
                    let va = self.l.styles.style(b.style).vertical_align;
                    match va {
                        VAlign::Top | VAlign::Bottom => edge_h = edge_h.max(mh),
                        VAlign::Middle => {
                            above = above.max(mh / 2.0 + self.strut_xh / 2.0);
                            below = below.max(mh / 2.0 - self.strut_xh / 2.0);
                        }
                        VAlign::TextTop => {
                            above = above.max(self.strut_asc);
                            below = below.max(mh - self.strut_asc);
                        }
                        VAlign::TextBottom => {
                            below = below.max(self.strut_desc);
                            above = above.max(mh - self.strut_desc);
                        }
                        _ => {
                            let shift = match va {
                                VAlign::Sub => -self.st.font_size * 0.25,
                                VAlign::Super => self.st.font_size * 0.35,
                                VAlign::Length(v) => v,
                                _ => 0.0,
                            };
                            let base = if b.baseline.is_nan() { mh } else { b.margin[0] + b.baseline };
                            above = above.max(base + shift);
                            below = below.max(mh - base - shift);
                        }
                    }
                }
                _ => {}
            }
        }
        let mut line_h = (above + below).max(0.0);
        if edge_h > line_h {
            line_h = edge_h;
        }

        // ── horizontal alignment ──
        let free = self.avail - self.line_w;
        let mut justify_extra = 0.0;
        let dx = if free > 0.0 {
            match self.st.text_align {
                TextAlign::Center | TextAlign::BlockCenter => free / 2.0,
                TextAlign::Right => free,
                TextAlign::Justify if !forced && !last => {
                    let spaces = line.iter().filter(|li| items.items[li.idx].kind == ItemKind::Space).count();
                    if spaces > 0 && free < self.avail * 0.4 {
                        justify_extra = free / spaces as f32;
                    }
                    0.0
                }
                _ => 0.0,
            }
        } else {
            0.0
        };
        let indent = if self.first_line { self.st.text_indent } else { 0.0 };
        let mut x = self.cx + self.x0 + dx + indent;
        let line_top = self.cy + self.y;
        let baseline = line_top + above;

        // ── fragments ──
        // (box, fragment index, start x) for inline boxes that paint a background/border
        let mut open: Vec<(BoxId, Option<usize>)> = Vec::new();
        let carried = std::mem::take(&mut self.open);
        for b in carried {
            let idx = self.open_fragment(b, x);
            open.push((b, idx));
        }
        let mut last_text: Option<usize> = None;
        for li in &line {
            let it = &items.items[li.idx];
            match it.kind {
                ItemKind::Word | ItemKind::Space => {
                    let r = &items.runs[it.run as usize];
                    let w = li.w + if it.kind == ItemKind::Space { justify_extra } else { 0.0 };
                    let merged = match last_text {
                        Some(fi) => {
                            let f = &mut self.frags[fi];
                            if f.box_id == it.box_id && f.g1 == li.g0 && justify_extra == 0.0 {
                                f.g1 = li.g1;
                                f.w += w;
                                true
                            } else {
                                false
                            }
                        }
                        None => false,
                    };
                    if !merged && li.g1 > li.g0 {
                        self.frags.push(Fragment {
                            x,
                            y: baseline - r.shift - r.ascent,
                            w,
                            h: r.ascent + r.descent,
                            baseline: baseline - r.shift,
                            kind: FragKind::Text,
                            box_id: it.box_id,
                            g0: li.g0,
                            g1: li.g1,
                        });
                        last_text = Some(self.frags.len() - 1);
                    }
                    x += w;
                }
                ItemKind::Open => {
                    x += it.aux;
                    let idx = self.open_fragment(it.box_id, x);
                    open.push((it.box_id, idx));
                    x += it.w - it.aux;
                    last_text = None;
                }
                ItemKind::Close => {
                    x += it.w - it.aux;
                    if let Some(pos) = open.iter().rposition(|(b, _)| *b == it.box_id) {
                        let (b, idx) = open.remove(pos);
                        if let Some(fi) = idx {
                            self.close_fragment(b, fi, x, baseline);
                        }
                    }
                    x += it.aux;
                    last_text = None;
                }
                ItemKind::Atomic => {
                    let (bw, bh, m, base, va) = {
                        let b = self.l.bx(it.box_id);
                        (b.w, b.h, b.margin, b.baseline, self.l.styles.style(b.style).vertical_align)
                    };
                    let mh = bh + m[0] + m[2];
                    let top = match va {
                        VAlign::Top => line_top,
                        VAlign::Bottom => line_top + line_h - mh,
                        VAlign::Middle => baseline - self.strut_xh / 2.0 - mh / 2.0,
                        VAlign::TextTop => baseline - self.strut_asc,
                        VAlign::TextBottom => baseline + self.strut_desc - mh,
                        _ => {
                            let shift = match va {
                                VAlign::Sub => -self.st.font_size * 0.25,
                                VAlign::Super => self.st.font_size * 0.35,
                                VAlign::Length(v) => v,
                                _ => 0.0,
                            };
                            let b0 = if base.is_nan() { mh } else { m[0] + base };
                            baseline - shift - b0
                        }
                    };
                    let (bx, by) = (x + m[3], top + m[0]);
                    {
                        let b = self.l.bxm(it.box_id);
                        b.x = bx;
                        b.y = by;
                    }
                    let st = self.l.st(it.box_id);
                    self.l.apply_relative(it.box_id, &st, self.content_w, None);
                    self.frags.push(Fragment {
                        x: bx,
                        y: by,
                        w: bw,
                        h: bh,
                        baseline,
                        kind: FragKind::Atomic,
                        box_id: it.box_id,
                        g0: 0,
                        g1: 0,
                    });
                    x += bw + m[1] + m[3];
                    last_text = None;
                }
                ItemKind::Abs => {
                    let b = self.l.bxm(it.box_id);
                    b.x = x;
                    b.y = line_top;
                }
                _ => {}
            }
        }
        // inline boxes still open continue on the next line
        for (b, idx) in open {
            if let Some(fi) = idx {
                self.close_fragment(b, fi, x, baseline);
            }
            self.open.push(b);
        }

        if self.first_baseline.is_nan() {
            self.first_baseline = baseline;
        }
        self.y += line_h;
        self.first_line = false;
        self.line = line;
        self.line.clear();
        self.line_w = 0.0;
        self.has_content = false;
        let floats = std::mem::take(&mut self.pending_floats);
        for f in floats {
            self.place_float(f);
        }
        self.update_band();
    }

    fn open_fragment(&mut self, b: BoxId, x: f32) -> Option<usize> {
        let st = self.l.st(b);
        if !needs_fragment(&st) {
            return None;
        }
        self.frags.push(Fragment { x, y: 0.0, w: 0.0, h: 0.0, baseline: 0.0, kind: FragKind::InlineBox, box_id: b, g0: 0, g1: 0 });
        Some(self.frags.len() - 1)
    }

    fn close_fragment(&mut self, b: BoxId, fi: usize, x: f32, baseline: f32) {
        let st = self.l.st(b);
        let font = self.l.fonts.resolve(&st.font_family, st.font_weight, st.italic);
        let m = self.l.fonts.metrics(font, st.font_size);
        let e = self.l.edges(&st, 0.0);
        let f = &mut self.frags[fi];
        f.w = (x - f.x).max(0.0);
        f.y = baseline - m.ascent - e.pad[0] - e.border[0];
        f.h = m.ascent + m.descent + e.pad[0] + e.pad[2] + e.border[0] + e.border[2];
        f.baseline = baseline;
    }
}

/// Lay out the inline content of block `root`. Returns the content height.
pub(crate) fn layout_inline(l: &mut Layout, root: BoxId, content_w: f32, e: &Edges, fc: &mut FloatCtx, ox: f32, oy: f32) -> f32 {
    let items = items_for(l, root);
    let st = l.st(root);
    let font = l.fonts.resolve(&st.font_family, st.font_weight, st.italic);
    let m = l.fonts.metrics(font, st.font_size);
    let lh = st.line_height_px(m.natural_line_height());
    let strut_above = m.ascent + (lh - m.ascent - m.descent) / 2.0;
    let n = items.items.len();

    let mut b = Builder {
        l,
        fc,
        items: items.clone(),
        st: st.clone(),
        cx: e.border[3] + e.pad[3],
        cy: e.border[0] + e.pad[0],
        ox,
        oy,
        content_w,
        strut_above,
        strut_below: lh - strut_above,
        strut_asc: m.ascent,
        strut_desc: m.descent,
        strut_xh: m.x_height,
        line: Vec::new(),
        line_w: 0.0,
        has_content: false,
        y: 0.0,
        first_line: true,
        first_baseline: f32::NAN,
        x0: 0.0,
        avail: content_w,
        open: Vec::new(),
        pending_floats: Vec::new(),
        frags: Vec::new(),
    };
    b.update_band();

    // remainder of a word that was split across lines: (item index, first glyph, width)
    let mut split: Option<(usize, u32, f32)> = None;
    let mut i = 0;
    let mut guard = 0usize;
    while i < n {
        guard += 1;
        if guard > n * 8 + 10_000 {
            break; // defensive: never loop forever on pathological input
        }
        let it = items.items[i];
        match it.kind {
            ItemKind::Break => {
                b.finish(true, false);
                i += 1;
                continue;
            }
            ItemKind::Space if it.flags & I_NOBREAK == 0 => {
                if b.has_content || it.flags & I_PRESERVED != 0 {
                    b.line.push(LineItem { idx: i, g0: it.g0, g1: it.g1, w: it.w });
                    b.line_w += it.w;
                }
                i += 1;
                continue;
            }
            ItemKind::Float => {
                if b.has_content {
                    b.pending_floats.push(it.box_id);
                } else {
                    b.place_float(it.box_id);
                    b.update_band();
                }
                i += 1;
                continue;
            }
            ItemKind::Abs => {
                b.line.push(LineItem { idx: i, g0: 0, g1: 0, w: 0.0 });
                i += 1;
                continue;
            }
            _ => {}
        }

        // gather the unbreakable chunk [i, j)
        let mut j = i;
        let mut cw = 0.0f32;
        let mut chunk_has_content = false;
        while j < n {
            let t = &items.items[j];
            if j > i {
                let stop = t.flags & I_BREAK_BEFORE != 0
                    || matches!(t.kind, ItemKind::Break | ItemKind::Float | ItemKind::Abs)
                    || (t.kind == ItemKind::Space && t.flags & I_NOBREAK == 0);
                if stop {
                    break;
                }
            }
            let w = match t.kind {
                ItemKind::Atomic => {
                    b.l.layout_box(t.box_id, content_w, None, WidthMode::FitContent, None, None, 0.0, 0.0);
                    let bx = b.l.bx(t.box_id);
                    chunk_has_content = true;
                    bx.w + bx.margin[1] + bx.margin[3]
                }
                ItemKind::Word | ItemKind::Space => {
                    chunk_has_content = true;
                    match split {
                        Some((si, _, sw)) if si == j => sw,
                        _ => t.w,
                    }
                }
                _ => t.w,
            };
            cw += w;
            j += 1;
        }

        let fits = b.line_w + cw <= b.avail + 0.01;
        if !fits && b.has_content {
            b.finish(false, false);
            continue; // retry the chunk on a fresh line
        }
        if !fits {
            // The chunk alone is wider than the line.
            if !b.fc.is_empty() && b.avail < content_w - 0.5 {
                // floats are in the way: move below the next float edge and retry
                let left_y = b.oy + b.cy + b.y;
                if let Some(ny) = b.fc.next_edge(left_y) {
                    b.y = ny - b.oy - b.cy;
                    b.update_band();
                    continue;
                }
            }
            let first = &items.items[i];
            let can_split = j == i + 1 && first.kind == ItemKind::Word && items.runs[first.run as usize].break_word && b.avail > 1.0;
            if can_split {
                let (g0, total) = match split {
                    Some((si, sg, sw)) if si == i => (sg, sw),
                    _ => (first.g0, first.w),
                };
                // take as many glyphs as fit (at least one)
                let mut w = 0.0f32;
                let mut g = g0;
                while g < first.g1 {
                    let adv = b.l.tree.glyphs[g as usize].adv;
                    if g > g0 && w + adv > b.avail {
                        break;
                    }
                    w += adv;
                    g += 1;
                }
                b.line.push(LineItem { idx: i, g0, g1: g, w });
                b.line_w += w;
                b.has_content = true;
                if g < first.g1 {
                    split = Some((i, g, (total - w).max(0.0)));
                    b.finish(false, false);
                } else {
                    split = None;
                    i = j;
                }
                continue;
            }
        }
        // place the chunk
        for k in i..j {
            let t = &items.items[k];
            let (g0, w) = match split {
                Some((si, sg, sw)) if si == k => (sg, sw),
                _ => (t.g0, t.w),
            };
            let w = if t.kind == ItemKind::Atomic {
                let bx = b.l.bx(t.box_id);
                bx.w + bx.margin[1] + bx.margin[3]
            } else {
                w
            };
            b.line.push(LineItem { idx: k, g0, g1: t.g1, w });
        }
        if split.map_or(false, |(si, _, _)| si >= i && si < j) {
            split = None;
        }
        b.line_w += cw;
        b.has_content |= chunk_has_content;
        i = j;
    }
    b.finish(false, true);

    let (height, first_baseline) = (b.y, b.first_baseline);
    // floats placed after the last line still need a home
    let floats = std::mem::take(&mut b.pending_floats);
    for f in floats {
        b.place_float(f);
    }
    let frags = std::mem::take(&mut b.frags);
    let l = b.l;
    let frag_start = l.tree.fragments.len() as u32;
    l.tree.fragments.extend_from_slice(&frags);
    let frag_end = l.tree.fragments.len() as u32;
    let bx = l.bxm(root);
    bx.r0 = frag_start;
    bx.r1 = frag_end;
    bx.baseline = first_baseline;
    height.max(0.0)
}
