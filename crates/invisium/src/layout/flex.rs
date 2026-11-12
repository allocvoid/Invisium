//! Flexbox layout.

use super::*;
use crate::style::{Align, FlexDirection, FlexWrap};

fn in_flow_items(l: &Layout, id: BoxId) -> Vec<BoxId> {
    let mut v: Vec<BoxId> = l.tree.children(id).filter(|&c| !l.bx(c).is_out_of_flow() && l.bx(c).kind != BoxKind::Marker).collect();
    // `order` reorders items; the sort is stable so document order breaks ties
    if v.iter().any(|&c| l.styles.style(l.bx(c).style).order != 0) {
        v.sort_by_key(|&c| l.styles.style(l.bx(c).style).order);
    }
    v
}

pub(crate) fn intrinsic(l: &mut Layout, id: BoxId) -> (f32, f32) {
    let st = l.st(id);
    let row = matches!(st.flex_direction, FlexDirection::Row | FlexDirection::RowReverse);
    let wrap = st.flex_wrap != FlexWrap::Nowrap;
    let gap = st.column_gap.px(0.0);
    let items = in_flow_items(l, id);
    let n = items.len() as f32;
    let (mut mn, mut mx) = (0.0f32, 0.0f32);
    for c in items {
        let (a, b) = l.intrinsic_outer(c);
        if row {
            if wrap {
                mn = mn.max(a);
            } else {
                mn += a;
            }
            mx += b;
        } else {
            mn = mn.max(a);
            mx = mx.max(b);
        }
    }
    if row && n > 1.0 {
        mx += gap * (n - 1.0);
        if !wrap {
            mn += gap * (n - 1.0);
        }
    }
    (mn, mx)
}

struct FItem {
    id: BoxId,
    /// Flex base size, min and max (content-box main sizes).
    base: f32,
    min: f32,
    max: f32,
    grow: f32,
    shrink: f32,
    /// Padding + border + (non-auto) margins along the main axis.
    extra: f32,
    /// Resolved content-box main size.
    main: f32,
    frozen: bool,
    /// Outer cross size after layout.
    cross: f32,
    auto_start: bool,
    auto_end: bool,
    edges: Edges,
    align: Align,
    main_definite: bool,
}

/// Lay out a flex container's items. Returns the content height.
pub(crate) fn layout_flex(l: &mut Layout, id: BoxId, content_w: f32, definite_h: Option<f32>, e: &Edges, cb_h: Option<f32>) -> f32 {
    let st = l.st(id);
    let row = matches!(st.flex_direction, FlexDirection::Row | FlexDirection::RowReverse);
    let reverse = matches!(st.flex_direction, FlexDirection::RowReverse | FlexDirection::ColumnReverse);
    let wrap = st.flex_wrap != FlexWrap::Nowrap;
    let cx = e.border[3] + e.pad[3];
    let cy = e.border[0] + e.pad[0];
    let col_gap = st.column_gap.px(content_w);
    let row_gap = st.row_gap.px(definite_h.unwrap_or(0.0));
    let (main_gap, cross_gap) = if row { (col_gap, row_gap) } else { (row_gap, col_gap) };

    // static position for out-of-flow children
    for c in l.child_ids(id) {
        if l.bx(c).is_out_of_flow() {
            let b = l.bxm(c);
            b.x = cx;
            b.y = cy;
        }
    }

    // A column container's main size is definite only if its height is; min-height can still
    // create free space to distribute.
    let min_main_col = if row {
        None
    } else {
        st.min_height.resolve_opt(cb_h).map(|m| if st.box_sizing == BoxSizing::BorderBox { (m - e.pb_h()).max(0.0) } else { m })
    };
    let container_main: Option<f32> = if row { Some(content_w) } else { definite_h };

    let ids = in_flow_items(l, id);
    let mut items: Vec<FItem> = Vec::with_capacity(ids.len());
    for &c in &ids {
        let cst = l.st(c);
        let ce = l.edges(&cst, content_w);
        let (pb_main, m_main) = if row { (ce.pb_w(), ce.m_w()) } else { (ce.pb_h(), ce.m_h()) };
        let border_box = cst.box_sizing == BoxSizing::BorderBox;
        let to_content = |v: f32| if border_box { (v - pb_main).max(0.0) } else { v.max(0.0) };
        let main_prop = if row { &cst.width } else { &cst.height };
        let basis_src = if cst.flex_basis.is_auto() { main_prop } else { &cst.flex_basis };
        let definite_basis = match container_main {
            Some(cm) => basis_src.resolve(cm),
            None => match basis_src {
                Size::Px(v) => Some(*v),
                _ => None,
            },
        };
        let is_replaced = l.bx(c).kind == BoxKind::Replaced;
        let mut main_definite = definite_basis.is_some();
        let base = match definite_basis {
            Some(v) if !is_replaced || !basis_src.is_auto() => to_content(v),
            _ => {
                main_definite = false;
                if row {
                    if is_replaced {
                        l.replaced_size(c, content_w, definite_h, false).0
                    } else {
                        l.intrinsic_content(c).1
                    }
                } else {
                    // content height at the width the item will get
                    measure_column_item(l, c, &cst, &ce, content_w, definite_h)
                }
            }
        };
        // automatic minimum size: content cannot be squeezed below its min-content size
        let min_prop = if row { &cst.min_width } else { &cst.min_height };
        let min = match min_prop {
            Size::Auto => {
                let clips = if row { cst.overflow_x != Overflow::Visible } else { cst.overflow_y != Overflow::Visible };
                if clips {
                    0.0
                } else if row {
                    let mc = if is_replaced { 0.0 } else { l.intrinsic_content(c).0 };
                    match definite_basis {
                        Some(v) if cst.flex_basis.is_auto() => mc.min(to_content(v)),
                        _ => mc,
                    }
                } else if container_main.is_some() {
                    measure_column_item(l, c, &cst, &ce, content_w, definite_h).min(if main_definite && cst.flex_basis.is_auto() { base } else { f32::INFINITY })
                } else {
                    0.0
                }
            }
            other => container_main.and_then(|cm| other.resolve(cm)).or(match other {
                Size::Px(v) => Some(*v),
                _ => None,
            })
            .map_or(0.0, to_content),
        };
        let max_prop = if row { &cst.max_width } else { &cst.max_height };
        let max = container_main
            .and_then(|cm| max_prop.resolve(cm))
            .or(match max_prop {
                Size::Px(v) => Some(*v),
                _ => None,
            })
            .map_or(f32::INFINITY, to_content);
        let (auto_start, auto_end) = if row { (cst.margin[3].is_auto(), cst.margin[1].is_auto()) } else { (cst.margin[0].is_auto(), cst.margin[2].is_auto()) };
        let align = match cst.align_self {
            Align::Auto => match st.align_items {
                Align::Normal | Align::Auto => Align::Stretch,
                a => a,
            },
            Align::Normal => Align::Stretch,
            a => a,
        };
        let max = max.max(min);
        items.push(FItem {
            id: c,
            base,
            min,
            max,
            grow: cst.flex_grow,
            shrink: cst.flex_shrink,
            extra: pb_main + m_main,
            main: base.clamp(min, max),
            frozen: false,
            cross: 0.0,
            auto_start,
            auto_end,
            edges: ce,
            align,
            main_definite,
        });
    }

    // ── break into lines ──
    let mut lines: Vec<(usize, usize)> = Vec::new();
    match container_main {
        Some(cm) if wrap => {
            let mut start = 0;
            let mut used = 0.0f32;
            for (i, it) in items.iter().enumerate() {
                let outer = it.main + it.extra;
                if i > start && used + main_gap + outer > cm + 0.01 {
                    lines.push((start, i));
                    start = i;
                    used = outer;
                } else {
                    used += outer + if i > start { main_gap } else { 0.0 };
                }
            }
            lines.push((start, items.len()));
        }
        _ => lines.push((0, items.len())),
    }

    // ── resolve flexible lengths ──
    let mut col_main_total = 0.0f32;
    for &(s, en) in &lines {
        let n = en - s;
        if n == 0 {
            continue;
        }
        let gaps = main_gap * (n as f32 - 1.0);
        let hyp_sum: f32 = items[s..en].iter().map(|it| it.main + it.extra).sum::<f32>() + gaps;
        let cm = match container_main {
            Some(cm) => cm,
            None => match min_main_col {
                Some(m) if m > hyp_sum => m,
                _ => {
                    col_main_total = col_main_total.max(hyp_sum);
                    continue; // indefinite main size: nothing to distribute
                }
            },
        };
        let growing = hyp_sum < cm;
        for it in items[s..en].iter_mut() {
            let factor = if growing { it.grow } else { it.shrink };
            it.frozen = factor == 0.0 || (growing && it.base > it.main) || (!growing && it.base < it.main);
        }
        for _ in 0..(n + 2) {
            let used: f32 = items[s..en].iter().map(|it| (if it.frozen { it.main } else { it.base }) + it.extra).sum::<f32>() + gaps;
            let remaining = cm - used;
            let factor_sum: f32 = items[s..en]
                .iter()
                .filter(|it| !it.frozen)
                .map(|it| if growing { it.grow } else { it.shrink * it.base.max(0.0001) })
                .sum();
            if factor_sum <= 0.0 {
                break;
            }
            // flex factors summing to less than 1 only distribute that fraction of the space
            let remaining = if growing {
                let gsum: f32 = items[s..en].iter().filter(|it| !it.frozen).map(|it| it.grow).sum();
                if gsum < 1.0 {
                    remaining * gsum
                } else {
                    remaining
                }
            } else {
                remaining
            };
            let mut total_violation = 0.0f32;
            let mut tentative: Vec<(usize, f32, f32)> = Vec::new();
            for (i, it) in items[s..en].iter().enumerate() {
                if it.frozen {
                    continue;
                }
                let share = if growing { it.grow / factor_sum } else { it.shrink * it.base.max(0.0001) / factor_sum };
                let t = it.base + remaining * share;
                let clamped = t.clamp(it.min, it.max).max(0.0);
                total_violation += clamped - t;
                tentative.push((s + i, t, clamped));
            }
            let mut froze_any = false;
            for (i, t, clamped) in &tentative {
                let it = &mut items[*i];
                it.main = *clamped;
                let freeze = if total_violation.abs() < 0.001 {
                    true
                } else if total_violation > 0.0 {
                    *clamped > *t
                } else {
                    *clamped < *t
                };
                if freeze {
                    it.frozen = true;
                    froze_any = true;
                }
            }
            if total_violation.abs() < 0.001 || !froze_any {
                break;
            }
        }
        if !row {
            let total: f32 = items[s..en].iter().map(|it| it.main + it.extra).sum::<f32>() + gaps;
            col_main_total = col_main_total.max(total.max(cm));
        }
    }

    // ── lay out items at their main size, collect cross sizes ──
    for it in items.iter_mut() {
        if row {
            let w = it.main + it.edges.pb_w();
            l.layout_box(it.id, content_w, definite_h, WidthMode::Fixed(w), None, None, 0.0, 0.0);
            it.cross = l.bx(it.id).h + it.edges.m_h();
        } else {
            let cst = l.st(it.id);
            let mode = column_item_mode(l, it.id, &cst, &it.edges, content_w, it.align);
            // only force the height when flexing actually changed it (keeps the cached layout)
            let natural = it.base.clamp(it.min, it.max);
            let force = it.main_definite || (it.main - natural).abs() > 0.5 || container_main.is_some();
            let fh = if force { Some(it.main + it.edges.pb_h()) } else { None };
            l.layout_box(it.id, content_w, definite_h, mode, fh, None, 0.0, 0.0);
            if !force {
                it.main = (l.bx(it.id).h - it.edges.pb_h()).max(0.0);
            }
            it.cross = l.bx(it.id).w + it.edges.m_w();
        }
    }

    // ── cross sizes of lines ──
    let container_cross: Option<f32> = if row { definite_h } else { Some(content_w) };
    let mut line_cross: Vec<f32> = lines.iter().map(|&(s, en)| items[s..en].iter().map(|it| it.cross).fold(0.0f32, f32::max)).collect();
    if let Some(cc) = container_cross {
        if lines.len() == 1 {
            line_cross[0] = cc;
        } else {
            let total: f32 = line_cross.iter().sum::<f32>() + cross_gap * (lines.len() as f32 - 1.0);
            if cc > total && matches!(st.align_content, Align::Normal | Align::Stretch | Align::Auto) {
                let add = (cc - total) / lines.len() as f32;
                for v in line_cross.iter_mut() {
                    *v += add;
                }
            }
        }
    }

    // ── position ──
    let total_cross: f32 = line_cross.iter().sum::<f32>() + cross_gap * (lines.len().max(1) as f32 - 1.0);
    let mut cross_pos = match (container_cross, st.align_content) {
        (Some(cc), Align::Center) if lines.len() > 1 => (cc - total_cross) / 2.0,
        (Some(cc), Align::End) if lines.len() > 1 => cc - total_cross,
        _ => 0.0,
    };
    let mut container_baseline = f32::NAN;
    let main_size_of = |cm_default: f32| container_main.unwrap_or(cm_default);
    for (li, &(s, en)) in lines.iter().enumerate() {
        let n = en - s;
        if n == 0 {
            continue;
        }
        let lc = line_cross[li];
        let gaps = main_gap * (n as f32 - 1.0);
        let used: f32 = items[s..en].iter().map(|it| it.main + it.extra).sum::<f32>() + gaps;
        let cm = if row { content_w } else { main_size_of(col_main_total).max(min_main_col.unwrap_or(0.0)) };
        let free = cm - used;
        let auto_count = items[s..en].iter().map(|it| it.auto_start as u32 + it.auto_end as u32).sum::<u32>();
        let auto_margin = if auto_count > 0 && free > 0.0 { free / auto_count as f32 } else { 0.0 };
        let (mut pos, between) = if auto_count > 0 && free > 0.0 {
            (0.0, 0.0)
        } else {
            match st.justify_content {
                Align::Center => (free / 2.0, 0.0),
                Align::End => (free, 0.0),
                Align::SpaceBetween if n > 1 && free > 0.0 => (0.0, free / (n as f32 - 1.0)),
                Align::SpaceAround if free > 0.0 => (free / n as f32 / 2.0, free / n as f32),
                Align::SpaceEvenly if free > 0.0 => (free / (n as f32 + 1.0), free / (n as f32 + 1.0)),
                _ => (0.0, 0.0),
            }
        };
        // baseline alignment (row only): align first baselines within the line
        let mut max_base = 0.0f32;
        if row {
            for it in &items[s..en] {
                if it.align == Align::Baseline {
                    let b = l.bx(it.id);
                    let base = if b.baseline.is_nan() { b.h } else { b.baseline };
                    max_base = max_base.max(base + it.edges.margin[0]);
                }
            }
        }
        for it in items[s..en].iter() {
            if it.auto_start {
                pos += auto_margin;
            }
            let outer_main = it.main + it.extra;
            let main_pos = if reverse { cm - pos - outer_main } else { pos };
            pos += outer_main + main_gap + between + if it.auto_end { auto_margin } else { 0.0 };

            // stretch
            let cst = l.st(it.id);
            let cross_prop_auto = if row { cst.height.is_auto() } else { cst.width.is_auto() };
            let (cm_start, cm_end) = if row { (cst.margin[0].is_auto(), cst.margin[2].is_auto()) } else { (cst.margin[3].is_auto(), cst.margin[1].is_auto()) };
            if row && it.align == Align::Stretch && cross_prop_auto && !cm_start && !cm_end {
                let target = (lc - it.edges.m_h()).max(0.0);
                let target = {
                    // respect min/max-height while stretching
                    let content = (target - it.edges.pb_h()).max(0.0);
                    l.clamp_height(&cst, content, definite_h, it.edges.pb_h()) + it.edges.pb_h()
                };
                if (l.bx(it.id).h - target).abs() > 0.5 {
                    let w = it.main + it.edges.pb_w();
                    l.layout_box(it.id, content_w, definite_h, WidthMode::Fixed(w), Some(target), None, 0.0, 0.0);
                }
            }
            let (bw, bh, base) = {
                let b = l.bx(it.id);
                (b.w, b.h, b.baseline)
            };
            let outer_cross = if row { bh + it.edges.m_h() } else { bw + it.edges.m_w() };
            let free_cross = lc - outer_cross;
            let cross_off = if cm_start && cm_end {
                (free_cross / 2.0).max(0.0)
            } else if cm_start {
                free_cross.max(0.0)
            } else if cm_end {
                0.0
            } else {
                match it.align {
                    Align::Center => free_cross / 2.0,
                    Align::End => free_cross,
                    Align::Baseline if row => {
                        let b0 = if base.is_nan() { bh } else { base };
                        max_base - (b0 + it.edges.margin[0])
                    }
                    _ => 0.0,
                }
            };
            let (x, y) = if row {
                (cx + main_pos + it.edges.margin[3], cy + cross_pos + cross_off + it.edges.margin[0])
            } else {
                (cx + cross_pos + cross_off + it.edges.margin[3], cy + main_pos + it.edges.margin[0])
            };
            {
                let b = l.bxm(it.id);
                b.x = x;
                b.y = y;
            }
            l.apply_relative(it.id, &cst, content_w, definite_h);
            if container_baseline.is_nan() && li == 0 {
                container_baseline = y + if base.is_nan() { bh } else { base };
            }
        }
        cross_pos += lc + cross_gap;
    }
    l.bxm(id).baseline = container_baseline;

    if row {
        total_cross
    } else {
        col_main_total.max(min_main_col.unwrap_or(0.0))
    }
}

/// Width mode of an item in a column container: stretched across the container unless it has
/// its own width or is aligned otherwise.
fn column_item_mode(l: &Layout, id: BoxId, cst: &ComputedStyle, ce: &Edges, content_w: f32, align: Align) -> WidthMode {
    let _ = l.bx(id);
    let auto_margins = cst.margin[3].is_auto() || cst.margin[1].is_auto();
    if cst.width.is_auto() && align == Align::Stretch && !auto_margins {
        WidthMode::Fixed((content_w - ce.m_w()).max(0.0))
    } else {
        WidthMode::FitContent
    }
}

/// Content-box height an item takes in a column container (lays it out at its cross size).
fn measure_column_item(l: &mut Layout, c: BoxId, cst: &ComputedStyle, ce: &Edges, content_w: f32, definite_h: Option<f32>) -> f32 {
    let align = Align::Stretch;
    let mode = column_item_mode(l, c, cst, ce, content_w, align);
    l.layout_box(c, content_w, definite_h, mode, None, None, 0.0, 0.0);
    (l.bx(c).h - ce.pb_h()).max(0.0)
}
