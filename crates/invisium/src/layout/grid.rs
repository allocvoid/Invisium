//! CSS Grid layout: explicit column tracks (lengths, percentages, `fr`, `auto`, `minmax()`,
//! `repeat()` including auto-fill/auto-fit), row-major auto-placement with spans and explicit
//! lines, named lines, content-sized rows. Subgrid is not supported.

use super::*;
use crate::atom::Atom;
use crate::css::values::{GridAreas, GridLine, LenCtx, Track, TrackBreadth, TrackItem};
use crate::style::Align;

#[derive(Clone, Copy)]
struct Placed {
    id: BoxId,
    col: usize,
    cs: usize,
    row: usize,
    rs: usize,
}

fn items_of(l: &Layout, id: BoxId) -> Vec<BoxId> {
    let mut v: Vec<BoxId> = l.tree.children(id).filter(|&c| !l.bx(c).is_out_of_flow() && l.bx(c).kind != BoxKind::Marker).collect();
    if v.iter().any(|&c| l.styles.style(l.bx(c).style).order != 0) {
        v.sort_by_key(|&c| l.styles.style(l.bx(c).style).order);
    }
    v
}

/// Definite pixel size of a breadth, if it has one.
fn breadth_px(b: &TrackBreadth, ctx: &LenCtx, basis: Option<f32>) -> Option<f32> {
    match b {
        TrackBreadth::Len(len) => {
            let s = len.to_size(ctx);
            match basis {
                Some(bw) => s.resolve(bw),
                None => match s {
                    Size::Px(v) => Some(v),
                    _ => None,
                },
            }
        }
        _ => None,
    }
}

/// Expand `repeat()` into a flat track list. `avail` is the container width when definite.
fn expand_tracks(template: Option<&[TrackItem]>, ctx: &LenCtx, avail: Option<f32>, gap: f32) -> Vec<Track> {
    let mut out = Vec::new();
    let Some(template) = template else { return out };
    for item in template {
        match item {
            TrackItem::One(t) => out.push(t.clone()),
            TrackItem::Names(_) => {}
            TrackItem::Repeat(n, ts) => {
                for _ in 0..(*n).min(200) {
                    out.extend(ts.iter().cloned());
                }
            }
            TrackItem::AutoRepeat { tracks, .. } => {
                // as many repetitions as fit, using each track's definite minimum
                let unit: f32 = tracks
                    .iter()
                    .map(|t| breadth_px(&t.min, ctx, avail).or_else(|| breadth_px(&t.max, ctx, avail)).unwrap_or(0.0))
                    .sum::<f32>()
                    + gap * tracks.len() as f32;
                let count = match avail {
                    Some(a) if unit > 0.5 => (((a + gap) / unit).floor() as usize).clamp(1, 200),
                    _ => 1,
                };
                for _ in 0..count {
                    out.extend(tracks.iter().cloned());
                }
            }
        }
    }
    out
}

/// The named lines of a template: (name, index of the line, counting from 0). Names after a
/// `repeat(auto-fill, ...)` are left out: how many tracks that makes is not known here.
fn line_names(template: Option<&[TrackItem]>) -> Vec<(Atom, usize)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    for item in template.unwrap_or(&[]) {
        match item {
            TrackItem::One(_) => at += 1,
            TrackItem::Repeat(n, ts) => at += (*n).min(200) as usize * ts.len(),
            TrackItem::AutoRepeat { .. } => break,
            TrackItem::Names(names) => out.extend(names.iter().map(|&n| (n, at))),
        }
    }
    out
}

/// A line given by name becomes the number of that line (or `auto` if the template has no
/// such line); anything else stays as it is.
fn numbered(g: GridLine, names: &[(Atom, usize)]) -> GridLine {
    let GridLine::Named(name, nth) = g else { return g };
    let mut hits = names.iter().filter(|n| n.0 == name).map(|n| n.1);
    let found = if nth > 0 { hits.nth(nth as usize - 1) } else { hits.rev().nth((-nth) as usize - 1) };
    found.map_or(GridLine::Auto, |i| GridLine::Line(i as i32 + 1))
}

fn line_index(g: GridLine, ntracks: usize) -> Option<usize> {
    match g {
        GridLine::Line(n) if n > 0 => Some((n - 1) as usize),
        GridLine::Line(n) if n < 0 => {
            let idx = ntracks as i32 + 1 + n;
            if idx >= 0 {
                Some(idx as usize)
            } else {
                Some(0)
            }
        }
        _ => None,
    }
}

fn span_of(start: GridLine, end: GridLine, ntracks: usize) -> usize {
    match (start, end) {
        (GridLine::Span(n), _) | (_, GridLine::Span(n)) => n.max(1) as usize,
        (GridLine::Line(_), GridLine::Line(_)) => {
            let (a, b) = (line_index(start, ntracks).unwrap_or(0), line_index(end, ntracks).unwrap_or(0));
            if b > a {
                b - a
            } else {
                1
            }
        }
        _ => 1,
    }
}

fn place(l: &Layout, items: &[BoxId], ncols: usize, nrows_explicit: usize, areas: Option<&GridAreas>, names: (&[(Atom, usize)], &[(Atom, usize)])) -> Vec<Placed> {
    let ncols = ncols.max(1);
    let mut occ: Vec<Vec<bool>> = Vec::new();
    let mut out = Vec::with_capacity(items.len());
    let free = |occ: &Vec<Vec<bool>>, r: usize, c: usize, rs: usize, cs: usize| -> bool {
        for rr in r..r + rs {
            if let Some(row) = occ.get(rr) {
                for cc in c..c + cs {
                    if row[cc] {
                        return false;
                    }
                }
            }
        }
        true
    };
    let mark = |occ: &mut Vec<Vec<bool>>, r: usize, c: usize, rs: usize, cs: usize| {
        while occ.len() < r + rs {
            occ.push(vec![false; ncols]);
        }
        for row in occ.iter_mut().skip(r).take(rs) {
            for cell in row.iter_mut().skip(c).take(cs) {
                *cell = true;
            }
        }
    };
    let (mut cur_r, mut cur_c) = (0usize, 0usize);
    // what each item asks for: (column span, row span, column, row)
    let reqs: Vec<(usize, usize, Option<usize>, Option<usize>)> = items
        .iter()
        .map(|&id| {
            let st = l.styles.style(l.bx(id).style);
            if !st.grid_area.is_none() {
                if let Some((r, c, rs, cs)) = areas.and_then(|a| a.find(st.grid_area)) {
                    let cs = cs.min(ncols);
                    return (cs, rs, Some(c.min(ncols - cs)), Some(r));
                }
            }
            // lines given by name are looked up in the container's templates first
            let gc = (numbered(st.grid_column.0, names.0), numbered(st.grid_column.1, names.0));
            let gr = (numbered(st.grid_row.0, names.1), numbered(st.grid_row.1, names.1));
            let cs = span_of(gc.0, gc.1, ncols).min(ncols);
            let rs = span_of(gr.0, gr.1, nrows_explicit).min(1000);
            let col = line_index(gc.0, ncols).or_else(|| line_index(gc.1, ncols).map(|e| e.saturating_sub(cs))).map(|c| c.min(ncols - cs));
            let row = line_index(gr.0, nrows_explicit).or_else(|| line_index(gr.1, nrows_explicit).map(|e| e.saturating_sub(rs))).map(|r| r.min(10_000));
            (cs, rs, col, row)
        })
        .collect();
    // items with a definite position claim their cells before anything is auto-placed
    let mut order: Vec<usize> = (0..items.len()).filter(|&i| reqs[i].2.is_some() && reqs[i].3.is_some()).collect();
    order.extend((0..items.len()).filter(|&i| !(reqs[i].2.is_some() && reqs[i].3.is_some())));
    for i in order {
        let id = items[i];
        let (cs, rs, col, row) = reqs[i];
        let (r, c) = match (row, col) {
            (Some(r), Some(c)) => (r, c),
            (Some(r), None) => {
                let mut c = 0;
                while c + cs <= ncols && !free(&occ, r, c, rs, cs) {
                    c += 1;
                }
                (r, c.min(ncols - cs))
            }
            (None, Some(c)) => {
                let mut r = cur_r;
                let mut guard = 0;
                while !free(&occ, r, c, rs, cs) && guard < 100_000 {
                    r += 1;
                    guard += 1;
                }
                (r, c)
            }
            (None, None) => {
                let mut guard = 0;
                loop {
                    if cur_c + cs > ncols {
                        cur_c = 0;
                        cur_r += 1;
                    }
                    if free(&occ, cur_r, cur_c, rs, cs) || guard > 100_000 {
                        break;
                    }
                    cur_c += 1;
                    guard += 1;
                }
                let pos = (cur_r, cur_c);
                cur_c += cs;
                pos
            }
        };
        mark(&mut occ, r, c, rs, cs);
        out.push(Placed { id, col: c, cs, row: r, rs });
    }
    out
}

/// Resolve column widths. With `avail == None` (intrinsic sizing) flexible tracks take their
/// content size. Returns (min-content widths, final widths).
fn size_columns(l: &mut Layout, tracks: &[Track], placed: &[Placed], ctx: &LenCtx, avail: Option<f32>, gap: f32) -> (Vec<f32>, Vec<f32>) {
    let n = tracks.len();
    let mut base = vec![0.0f32; n];
    let mut limit = vec![0.0f32; n];
    let mut content_min = vec![0.0f32; n];
    let mut content_max = vec![0.0f32; n];
    for p in placed {
        if p.cs != 1 || p.col >= n {
            continue;
        }
        let (mn, mx) = l.intrinsic_outer(p.id);
        content_min[p.col] = content_min[p.col].max(mn);
        content_max[p.col] = content_max[p.col].max(mx);
    }
    let mut fr = vec![0.0f32; n];
    for (i, t) in tracks.iter().enumerate() {
        let min_px = match &t.min {
            TrackBreadth::Len(_) => breadth_px(&t.min, ctx, avail).unwrap_or(0.0),
            TrackBreadth::MaxContent => content_max[i],
            TrackBreadth::Fr(_) => 0.0,
            _ => content_min[i],
        };
        match &t.max {
            TrackBreadth::Fr(f) => {
                fr[i] = f.max(0.0);
                base[i] = min_px;
                limit[i] = min_px;
            }
            TrackBreadth::Len(_) => {
                let v = breadth_px(&t.max, ctx, avail).unwrap_or(content_max[i]);
                base[i] = min_px.min(v.max(min_px));
                limit[i] = v.max(min_px);
            }
            TrackBreadth::MinContent => {
                base[i] = min_px;
                limit[i] = content_min[i].max(min_px);
            }
            _ => {
                base[i] = min_px;
                limit[i] = content_max[i].max(min_px);
            }
        }
    }
    // items spanning several columns: make sure the spanned tracks can hold their min-content
    for p in placed {
        if p.cs < 2 || p.col >= n {
            continue;
        }
        let end = (p.col + p.cs).min(n);
        let (mn, _) = l.intrinsic_outer(p.id);
        let has_fr = fr[p.col..end].iter().any(|f| *f > 0.0);
        let cur: f32 = limit[p.col..end].iter().sum::<f32>() + gap * (end - p.col - 1) as f32;
        if mn > cur && !has_fr {
            let add = (mn - cur) / (end - p.col) as f32;
            for k in p.col..end {
                limit[k] += add;
            }
        }
    }
    let mins = base.clone();
    let mut widths = base.clone();
    let gaps = gap * n.saturating_sub(1) as f32;
    let Some(avail) = avail else {
        // intrinsic: every track at its growth limit, fr tracks at their content size
        for i in 0..n {
            widths[i] = if fr[i] > 0.0 { content_max[i].max(base[i]) } else { limit[i] };
        }
        let mut mn = mins;
        for i in 0..n {
            if fr[i] > 0.0 {
                mn[i] = mn[i].max(if matches!(tracks[i].min, TrackBreadth::Len(_)) { 0.0 } else { content_min[i] });
            }
        }
        return (mn, widths);
    };
    // 1. grow content-sized tracks towards their limit while space remains
    let mut free = avail - gaps - widths.iter().sum::<f32>();
    if free > 0.0 {
        let want: f32 = (0..n).map(|i| (limit[i] - widths[i]).max(0.0)).sum();
        if want > 0.0 {
            let k = (free / want).min(1.0);
            for i in 0..n {
                widths[i] += (limit[i] - widths[i]).max(0.0) * k;
            }
            free = avail - gaps - widths.iter().sum::<f32>();
        }
    }
    // 2. flexible tracks share what is left by their fr factor
    let fr_sum: f32 = fr.iter().sum();
    if fr_sum > 0.0 {
        let mut flexible: Vec<usize> = (0..n).filter(|&i| fr[i] > 0.0).collect();
        loop {
            let fixed: f32 = (0..n).filter(|i| !flexible.contains(i)).map(|i| widths[i]).sum();
            let space = (avail - gaps - fixed).max(0.0);
            let sum: f32 = flexible.iter().map(|&i| fr[i]).sum::<f32>().max(1.0);
            let unit = space / sum;
            // a track whose minimum exceeds its share keeps its minimum and leaves the pool
            let before = flexible.len();
            flexible.retain(|&i| base[i] <= fr[i] * unit + 0.001);
            if flexible.len() == before || flexible.is_empty() {
                for &i in &flexible {
                    widths[i] = fr[i] * unit;
                }
                break;
            }
        }
    } else if free > 0.5 {
        // no fr tracks: `auto` tracks stretch to fill
        let autos: Vec<usize> = (0..n).filter(|&i| matches!(tracks[i].max, TrackBreadth::Auto)).collect();
        if !autos.is_empty() {
            let add = free / autos.len() as f32;
            for i in autos {
                widths[i] += add;
            }
        }
    }
    (mins, widths)
}

fn auto_track() -> Track {
    Track { min: TrackBreadth::Auto, max: TrackBreadth::Auto }
}

pub(crate) fn intrinsic(l: &mut Layout, id: BoxId) -> (f32, f32) {
    let st = l.st(id);
    let ctx = l.styles.len_ctx(st.font_size);
    let gap = st.column_gap.px(0.0);
    let mut tracks = expand_tracks(st.grid_template_columns.as_deref(), &ctx, None, gap);
    let areas = st.grid_template_areas.clone();
    while tracks.len() < areas.as_ref().map_or(1, |a| a.cols).max(1) {
        tracks.push(auto_track());
    }
    let items = items_of(l, id);
    let (col_names, row_names) = (line_names(st.grid_template_columns.as_deref()), line_names(st.grid_template_rows.as_deref()));
    let placed = place(l, &items, tracks.len(), 0, areas.as_deref(), (&col_names, &row_names));
    let (mins, maxs) = size_columns(l, &tracks, &placed, &ctx, None, gap);
    let gaps = gap * tracks.len().saturating_sub(1) as f32;
    let auto_repeat = st.grid_template_columns.as_deref().map_or(false, |t| t.iter().any(|i| matches!(i, TrackItem::AutoRepeat { .. })));
    let mn = if auto_repeat { mins.iter().cloned().fold(0.0, f32::max) } else { mins.iter().sum::<f32>() + gaps };
    let mx = if auto_repeat {
        // an auto-repeat grid would like to show all items in one row
        let per: f32 = maxs.iter().cloned().fold(0.0, f32::max);
        per * items.len().max(1) as f32 + gap * items.len().saturating_sub(1) as f32
    } else {
        maxs.iter().sum::<f32>() + gaps
    };
    (mn, mx.max(mn))
}

/// Lay out a grid container. Returns the content height.
pub(crate) fn layout_grid(l: &mut Layout, id: BoxId, content_w: f32, definite_h: Option<f32>, e: &Edges) -> f32 {
    let st = l.st(id);
    let ctx = l.styles.len_ctx(st.font_size);
    let cx = e.border[3] + e.pad[3];
    let cy = e.border[0] + e.pad[0];
    let col_gap = st.column_gap.px(content_w);
    let row_gap = st.row_gap.px(definite_h.unwrap_or(0.0));

    for c in l.child_ids(id) {
        if l.bx(c).is_out_of_flow() {
            let b = l.bxm(c);
            b.x = cx;
            b.y = cy;
        }
    }

    let mut tracks = expand_tracks(st.grid_template_columns.as_deref(), &ctx, Some(content_w), col_gap);
    let areas = st.grid_template_areas.clone();
    while tracks.len() < areas.as_ref().map_or(1, |a| a.cols).max(1) {
        tracks.push(auto_track());
    }
    let row_tracks = expand_tracks(st.grid_template_rows.as_deref(), &ctx, definite_h, row_gap);
    let items = items_of(l, id);
    let (col_names, row_names) = (line_names(st.grid_template_columns.as_deref()), line_names(st.grid_template_rows.as_deref()));
    let placed = place(l, &items, tracks.len(), row_tracks.len(), areas.as_deref(), (&col_names, &row_names));
    let (_, widths) = size_columns(l, &tracks, &placed, &ctx, Some(content_w), col_gap);
    let ncols = widths.len();
    let mut col_x = vec![0.0f32; ncols];
    let mut x = 0.0;
    for i in 0..ncols {
        col_x[i] = x;
        x += widths[i] + col_gap;
    }
    // justify-content: centre/end the whole track set when it is narrower than the container
    let used_w = (x - col_gap).max(0.0);
    let jc_off = match st.justify_content {
        Align::Center => ((content_w - used_w) / 2.0).max(0.0),
        Align::End => (content_w - used_w).max(0.0),
        _ => 0.0,
    };

    let nrows = placed.iter().map(|p| p.row + p.rs).max().unwrap_or(0).max(row_tracks.len());
    let implicit_min = st
        .grid_auto_rows
        .as_deref()
        .and_then(|t| t.first())
        .and_then(|t| match t {
            TrackItem::One(tr) => breadth_px(&tr.min, &ctx, definite_h).or_else(|| breadth_px(&tr.max, &ctx, definite_h)),
            _ => None,
        })
        .unwrap_or(0.0);
    let mut row_h = vec![0.0f32; nrows];
    let mut row_fr = vec![0.0f32; nrows];
    for r in 0..nrows {
        match row_tracks.get(r) {
            Some(t) => {
                if let TrackBreadth::Fr(f) = &t.max {
                    row_fr[r] = *f;
                }
                row_h[r] = breadth_px(&t.min, &ctx, definite_h).or_else(|| breadth_px(&t.max, &ctx, definite_h)).unwrap_or(0.0);
            }
            None => row_h[r] = implicit_min,
        }
    }

    // ── first pass: lay out items at their column width ──
    struct Info {
        w: f32,
        stretch_w: bool,
        stretch_h: bool,
        jalign: Align,
        aalign: Align,
        edges: Edges,
    }
    let mut infos: Vec<Info> = Vec::with_capacity(placed.len());
    for p in &placed {
        let cst = l.st(p.id);
        let ce = l.edges(&cst, content_w);
        let end = (p.col + p.cs).min(ncols);
        let w: f32 = widths[p.col.min(ncols - 1)..end].iter().sum::<f32>() + col_gap * (end.saturating_sub(p.col + 1)) as f32;
        let jalign = match cst.justify_self {
            Align::Auto | Align::Normal => match st.justify_items {
                Align::Normal | Align::Auto => Align::Stretch,
                a => a,
            },
            a => a,
        };
        let aalign = match cst.align_self {
            Align::Auto | Align::Normal => match st.align_items {
                Align::Normal | Align::Auto => Align::Stretch,
                a => a,
            },
            a => a,
        };
        let is_replaced = l.bx(p.id).kind == BoxKind::Replaced;
        let auto_margin = cst.margin[1].is_auto() || cst.margin[3].is_auto();
        let stretch_w = jalign == Align::Stretch && cst.width.is_auto() && !is_replaced && !auto_margin;
        let stretch_h = aalign == Align::Stretch && cst.height.is_auto() && !is_replaced;
        let mode = if stretch_w { WidthMode::Fixed((w - ce.m_w()).max(0.0)) } else { WidthMode::FitContent };
        l.layout_box(p.id, w, None, mode, None, None, 0.0, 0.0);
        let h = l.bx(p.id).h + ce.m_h();
        if p.rs == 1 && p.row < nrows {
            row_h[p.row] = row_h[p.row].max(h);
        }
        infos.push(Info { w, stretch_w, stretch_h, jalign, aalign, edges: ce });
    }
    for (p, _) in placed.iter().zip(&infos) {
        if p.rs > 1 {
            let end = (p.row + p.rs).min(nrows);
            let total: f32 = row_h[p.row..end].iter().sum::<f32>() + row_gap * (end - p.row - 1) as f32;
            let h = l.bx(p.id).h + l.bx(p.id).margin[0] + l.bx(p.id).margin[2];
            if h > total && end > p.row {
                row_h[end - 1] += h - total;
            }
        }
    }
    // fr rows share a definite container height
    let fr_sum: f32 = row_fr.iter().sum();
    if let (Some(dh), true) = (definite_h, fr_sum > 0.0) {
        let fixed: f32 = (0..nrows).filter(|&r| row_fr[r] == 0.0).map(|r| row_h[r]).sum::<f32>() + row_gap * nrows.saturating_sub(1) as f32;
        let unit = ((dh - fixed) / fr_sum).max(0.0);
        for r in 0..nrows {
            if row_fr[r] > 0.0 {
                row_h[r] = row_h[r].max(row_fr[r] * unit);
            }
        }
    }
    let mut row_y = vec![0.0f32; nrows];
    let mut y = 0.0;
    for r in 0..nrows {
        row_y[r] = y;
        y += row_h[r] + row_gap;
    }
    let content_h = if nrows > 0 { y - row_gap } else { 0.0 };

    // ── second pass: stretch and position ──
    let mut baseline = f32::NAN;
    for (p, info) in placed.iter().zip(&infos) {
        let cst = l.st(p.id);
        let end = (p.row + p.rs).min(nrows);
        let area_h: f32 = row_h[p.row.min(nrows.saturating_sub(1))..end].iter().sum::<f32>() + row_gap * end.saturating_sub(p.row + 1) as f32;
        if info.stretch_h {
            let target = (area_h - info.edges.m_h()).max(0.0);
            if (l.bx(p.id).h - target).abs() > 0.5 {
                let mode = if info.stretch_w { WidthMode::Fixed((info.w - info.edges.m_w()).max(0.0)) } else { WidthMode::FitContent };
                l.layout_box(p.id, info.w, None, mode, Some(target), None, 0.0, 0.0);
            }
        }
        let (bw, bh, m, base) = {
            let b = l.bx(p.id);
            (b.w, b.h, b.margin, b.baseline)
        };
        let free_w = info.w - bw - m[1] - m[3];
        let free_h = area_h - bh - m[0] - m[2];
        let auto_l = cst.margin[3].is_auto();
        let auto_r = cst.margin[1].is_auto();
        let dx = if auto_l && auto_r {
            (free_w / 2.0).max(0.0)
        } else if auto_l {
            free_w.max(0.0)
        } else {
            match info.jalign {
                Align::Center => free_w / 2.0,
                Align::End => free_w,
                _ => 0.0,
            }
        };
        let dy = match info.aalign {
            Align::Center => free_h / 2.0,
            Align::End => free_h,
            _ => 0.0,
        };
        let bx_x = cx + jc_off + col_x[p.col.min(ncols - 1)] + m[3] + dx;
        let bx_y = cy + row_y[p.row.min(nrows.saturating_sub(1))] + m[0] + dy;
        {
            let b = l.bxm(p.id);
            b.x = bx_x;
            b.y = bx_y;
        }
        l.apply_relative(p.id, &cst, content_w, definite_h);
        if baseline.is_nan() && p.row == 0 && !base.is_nan() {
            baseline = bx_y + base;
        }
    }
    l.bxm(id).baseline = baseline;
    content_h
}
