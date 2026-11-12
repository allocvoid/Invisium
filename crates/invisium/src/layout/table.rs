//! Table layout (automatic column sizing, colspan/rowspan, captions, vertical alignment).

use super::*;
use crate::style::VAlign;

struct Cell {
    id: BoxId,
    row: usize,
    col: usize,
    colspan: usize,
    rowspan: usize,
}

struct Structure {
    /// (row box, parent group box or NO_BOX)
    rows: Vec<(BoxId, BoxId)>,
    cells: Vec<Cell>,
    ncols: usize,
    captions: Vec<BoxId>,
}

fn structure(l: &Layout, id: BoxId) -> Structure {
    let mut head: Vec<(BoxId, BoxId)> = Vec::new();
    let mut body: Vec<(BoxId, BoxId)> = Vec::new();
    let mut foot: Vec<(BoxId, BoxId)> = Vec::new();
    let mut captions = Vec::new();
    for c in l.tree.children(id) {
        let b = l.bx(c);
        match b.kind {
            BoxKind::TableCaption => captions.push(c),
            BoxKind::TableRow => body.push((c, NO_BOX)),
            BoxKind::TableRowGroup => {
                let disp = l.styles.style(b.style).display;
                let target = match disp {
                    Display::TableHeaderGroup => &mut head,
                    Display::TableFooterGroup => &mut foot,
                    _ => &mut body,
                };
                for r in l.tree.children(c) {
                    if l.bx(r).kind == BoxKind::TableRow {
                        target.push((r, c));
                    }
                }
            }
            _ => {}
        }
    }
    let mut rows = head;
    rows.append(&mut body);
    rows.append(&mut foot);

    let mut cells = Vec::new();
    let mut span_left: Vec<u32> = Vec::new();
    let mut ncols = 0usize;
    for (ri, &(row, _)) in rows.iter().enumerate() {
        let mut col = 0usize;
        for c in l.tree.children(row) {
            let b = l.bx(c);
            if b.kind != BoxKind::TableCell {
                continue;
            }
            while col < span_left.len() && span_left[col] > 0 {
                col += 1;
            }
            let colspan = ((b.aux & 0xffff) as usize).clamp(1, 1000);
            let rowspan = ((b.aux >> 16) as usize).max(1).min(rows.len() - ri);
            if span_left.len() < col + colspan {
                span_left.resize(col + colspan, 0);
            }
            for k in col..col + colspan {
                span_left[k] = rowspan as u32;
            }
            cells.push(Cell { id: c, row: ri, col, colspan, rowspan });
            col += colspan;
            ncols = ncols.max(col);
        }
        for s in span_left.iter_mut() {
            if *s > 0 {
                *s -= 1;
            }
        }
    }
    Structure { rows, cells, ncols, captions }
}

struct Columns {
    min: Vec<f32>,
    max: Vec<f32>,
    pct: Vec<f32>,
    /// Space before the first / after the last column, and between columns.
    edge: f32,
    gap: f32,
    edge_v: f32,
    gap_v: f32,
}

impl Columns {
    fn spacing_total(&self, n: usize) -> f32 {
        if n == 0 {
            0.0
        } else {
            2.0 * self.edge + self.gap * (n as f32 - 1.0)
        }
    }
}

fn columns(l: &mut Layout, id: BoxId, s: &Structure) -> Columns {
    let st = l.st(id);
    let n = s.ncols;
    let (edge, gap, edge_v, gap_v) = if st.border_collapse {
        // collapsed borders: neighbouring cells overlap by one border width
        let bw = s.cells.first().map_or(0.0, |c| l.styles.style(l.bx(c.id).style).border_width[3]);
        let bh = s.cells.first().map_or(0.0, |c| l.styles.style(l.bx(c.id).style).border_width[0]);
        (0.0, -bw, 0.0, -bh)
    } else {
        (st.border_spacing.0, st.border_spacing.0, st.border_spacing.1, st.border_spacing.1)
    };
    let mut min = vec![0.0f32; n];
    let mut max = vec![0.0f32; n];
    let mut pct = vec![0.0f32; n];
    let mut measured: Vec<(f32, f32)> = Vec::with_capacity(s.cells.len());
    for c in &s.cells {
        let cst = l.st(c.id);
        let ce = l.edges(&cst, 0.0);
        let (mn, mx) = l.intrinsic_content(c.id);
        let extra = ce.pb_w();
        let (mut mn, mut mx) = (mn + extra, mx + extra);
        if cst.overflow_x != Overflow::Visible {
            mn = extra;
        }
        match cst.width {
            Size::Px(w) => {
                let w = if cst.box_sizing == BoxSizing::BorderBox { w } else { w + extra };
                // a specified width is the preferred width, but never below the content minimum
                mx = w.max(mn);
            }
            Size::Pct(p) if c.colspan == 1 => pct[c.col] = pct[c.col].max(p),
            _ => {}
        }
        if let Size::Px(w) = cst.min_width {
            mn = mn.max(w);
            mx = mx.max(mn);
        }
        if let Size::Px(w) = cst.max_width {
            mx = mx.min(w.max(mn));
        }
        if c.colspan == 1 {
            min[c.col] = min[c.col].max(mn);
            max[c.col] = max[c.col].max(mx);
        }
        measured.push((mn, mx));
    }
    // spanning cells: widen the spanned columns if they are too narrow together
    for (c, &(mn, mx)) in s.cells.iter().zip(&measured) {
        if c.colspan < 2 {
            continue;
        }
        let range = c.col..(c.col + c.colspan).min(n);
        let cnt = range.len() as f32;
        let inner = gap * (cnt - 1.0);
        let cur_min: f32 = min[range.clone()].iter().sum::<f32>() + inner;
        if mn > cur_min {
            let add = (mn - cur_min) / cnt;
            for k in range.clone() {
                min[k] += add;
            }
        }
        let cur_max: f32 = max[range.clone()].iter().sum::<f32>() + inner;
        if mx > cur_max {
            let add = (mx - cur_max) / cnt;
            for k in range.clone() {
                max[k] += add;
            }
        }
    }
    for k in 0..n {
        max[k] = max[k].max(min[k]);
    }
    Columns { min, max, pct, edge, gap, edge_v, gap_v }
}

pub(crate) fn intrinsic(l: &mut Layout, id: BoxId) -> (f32, f32) {
    let s = structure(l, id);
    let cols = columns(l, id, &s);
    let sp = cols.spacing_total(s.ncols);
    let mut mn: f32 = cols.min.iter().sum::<f32>() + sp;
    let mut mx: f32 = cols.max.iter().sum::<f32>() + sp;
    for &cap in &s.captions {
        let (a, _) = l.intrinsic_outer(cap);
        mn = mn.max(a);
        mx = mx.max(a);
    }
    (mn, mx)
}

pub(crate) fn layout_table(l: &mut Layout, id: BoxId, cb_w: f32, cb_h: Option<f32>, mode: WidthMode, e: &Edges) {
    let st = l.st(id);
    let s = structure(l, id);
    let cols = columns(l, id, &s);
    let n = s.ncols;
    let sp = cols.spacing_total(n);
    let sum_min: f32 = cols.min.iter().sum();
    let sum_max: f32 = cols.max.iter().sum();
    let border_box = st.box_sizing == BoxSizing::BorderBox;

    // ── table width ──
    let avail = (cb_w - e.m_w() - e.pb_w()).max(0.0);
    let spec = st.width.resolve(cb_w).map(|w| if border_box { (w - e.pb_w()).max(0.0) } else { w });
    let mut content_w = match mode {
        WidthMode::Fixed(w) => (w - e.pb_w()).max(sum_min + sp),
        _ => match spec {
            Some(w) => w.max(sum_min + sp),
            None => (sum_max + sp).min(avail.max(sum_min + sp)),
        },
    };
    if let Some(mx) = st.max_width.resolve(cb_w) {
        let mx = if border_box { (mx - e.pb_w()).max(0.0) } else { mx };
        content_w = content_w.min(mx.max(sum_min + sp));
    }
    if let Some(mn) = st.min_width.resolve(cb_w) {
        let mn = if border_box { (mn - e.pb_w()).max(0.0) } else { mn };
        content_w = content_w.max(mn);
    }

    // ── distribute to columns ──
    let target = (content_w - sp).max(0.0);
    let mut cmin = cols.min.clone();
    let mut cmax = cols.max.clone();
    for k in 0..n {
        if cols.pct[k] > 0.0 {
            let want = (target * cols.pct[k] / 100.0).max(cmin[k]);
            cmin[k] = want;
            cmax[k] = want;
        }
    }
    let smin: f32 = cmin.iter().sum();
    let smax: f32 = cmax.iter().sum();
    let mut widths = vec![0.0f32; n];
    if n > 0 {
        if target >= smax {
            let extra = target - smax;
            // growth goes to the flexible (non-percentage) columns, in proportion to their content
            let flex_sum: f32 = (0..n).filter(|&k| cols.pct[k] == 0.0).map(|k| cmax[k]).sum();
            let flex_cnt = (0..n).filter(|&k| cols.pct[k] == 0.0).count();
            for k in 0..n {
                let share = if flex_cnt == 0 {
                    extra / n as f32
                } else if cols.pct[k] > 0.0 {
                    0.0
                } else if flex_sum > 0.0 {
                    extra * cmax[k] / flex_sum
                } else {
                    extra / flex_cnt as f32
                };
                widths[k] = cmax[k] + share;
            }
        } else if target <= smin || smax - smin < 0.001 {
            let scale = if smin > 0.0 && target < smin && smax - smin < 0.001 { 1.0 } else { 1.0 };
            for k in 0..n {
                widths[k] = cmin[k] * scale;
            }
        } else {
            let t = (target - smin) / (smax - smin);
            for k in 0..n {
                widths[k] = cmin[k] + (cmax[k] - cmin[k]) * t;
            }
        }
    }
    let used: f32 = widths.iter().sum::<f32>() + sp;
    let content_w = content_w.max(used);

    let cx = e.border[3] + e.pad[3];
    let cy = e.border[0] + e.pad[0];
    let mut col_x = vec![0.0f32; n + 1];
    let mut x = cols.edge;
    for k in 0..n {
        col_x[k] = x;
        x += widths[k] + cols.gap;
    }

    // ── captions ──
    let mut y = 0.0f32;
    for &cap in &s.captions {
        l.layout_box(cap, content_w, None, WidthMode::Stretch, None, None, 0.0, 0.0);
        let (h, m) = (l.bx(cap).h, l.bx(cap).margin);
        let b = l.bxm(cap);
        b.x = cx + m[3];
        b.y = cy + y + m[0];
        y += h + m[0] + m[2];
    }

    // ── cells: first pass at their column width ──
    let nrows = s.rows.len();
    let mut row_h = vec![0.0f32; nrows];
    let mut natural: Vec<f32> = Vec::with_capacity(s.cells.len());
    for c in &s.cells {
        let end = (c.col + c.colspan).min(n);
        let w: f32 = widths[c.col..end].iter().sum::<f32>() + cols.gap * (end - c.col).saturating_sub(1) as f32;
        l.layout_box(c.id, w, None, WidthMode::Fixed(w), None, None, 0.0, 0.0);
        let h = l.bx(c.id).h;
        natural.push(h);
        if c.rowspan == 1 {
            row_h[c.row] = row_h[c.row].max(h);
        }
    }
    for (ri, &(row, _)) in s.rows.iter().enumerate() {
        let rst = l.st(row);
        if let Some(h) = rst.height.resolve_opt(None) {
            row_h[ri] = row_h[ri].max(h);
        }
    }
    for (c, &h) in s.cells.iter().zip(&natural) {
        if c.rowspan > 1 {
            let end = (c.row + c.rowspan).min(nrows);
            let total: f32 = row_h[c.row..end].iter().sum::<f32>() + cols.gap_v * (end - c.row).saturating_sub(1) as f32;
            if h > total && end > c.row {
                row_h[end - 1] += h - total;
            }
        }
    }
    // a specified table height stretches the rows
    let rows_total: f32 = row_h.iter().sum::<f32>() + if nrows > 0 { 2.0 * cols.edge_v + cols.gap_v * (nrows as f32 - 1.0) } else { 0.0 };
    if let Some(h) = st.height.resolve_opt(cb_h) {
        let want = if border_box { (h - e.pb_h()).max(0.0) } else { h } - y;
        if want > rows_total && nrows > 0 {
            let add = (want - rows_total) / nrows as f32;
            for v in row_h.iter_mut() {
                *v += add;
            }
        }
    }

    // ── position rows and cells ──
    let mut row_y = vec![0.0f32; nrows];
    let mut ry = y + cols.edge_v;
    for r in 0..nrows {
        row_y[r] = ry;
        ry += row_h[r] + cols.gap_v;
    }
    let rows_bottom = if nrows > 0 { ry - cols.gap_v + cols.edge_v } else { y };

    // row groups span their rows
    let mut group_y: std::collections::HashMap<BoxId, (f32, f32)> = std::collections::HashMap::new();
    for (ri, &(_, group)) in s.rows.iter().enumerate() {
        if group != NO_BOX {
            let entry = group_y.entry(group).or_insert((row_y[ri], row_y[ri] + row_h[ri]));
            entry.0 = entry.0.min(row_y[ri]);
            entry.1 = entry.1.max(row_y[ri] + row_h[ri]);
        }
    }
    for (&g, &(top, bottom)) in &group_y {
        let b = l.bxm(g);
        b.x = cx;
        b.y = cy + top;
        b.w = content_w;
        b.h = bottom - top;
    }
    for (ri, &(row, group)) in s.rows.iter().enumerate() {
        let gtop = if group != NO_BOX { group_y[&group].0 } else { 0.0 };
        let b = l.bxm(row);
        b.w = content_w;
        b.h = row_h[ri];
        if group != NO_BOX {
            b.x = 0.0;
            b.y = row_y[ri] - gtop;
        } else {
            b.x = cx;
            b.y = cy + row_y[ri];
        }
    }
    let mut table_baseline = f32::NAN;
    for (c, &nat) in s.cells.iter().zip(&natural) {
        let end = (c.row + c.rowspan).min(nrows);
        let total: f32 = row_h[c.row..end].iter().sum::<f32>() + cols.gap_v * (end - c.row).saturating_sub(1) as f32;
        let va = l.styles.style(l.bx(c.id).style).vertical_align;
        let free = (total - nat).max(0.0);
        let dy = match va {
            VAlign::Middle => free / 2.0,
            VAlign::Bottom => free,
            _ => 0.0,
        };
        {
            let b = l.bxm(c.id);
            b.x = col_x[c.col];
            b.y = 0.0;
            b.h = total.max(nat);
        }
        l.shift_contents(c.id, dy);
        if table_baseline.is_nan() && c.row == 0 {
            let base = l.bx(c.id).baseline;
            if !base.is_nan() {
                table_baseline = cy + row_y[0] + base;
            }
        }
    }

    let b = l.bxm(id);
    b.w = content_w + e.pb_w();
    b.h = rows_bottom + e.pb_h();
    b.baseline = table_baseline;
}
