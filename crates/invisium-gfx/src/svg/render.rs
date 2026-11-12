//! Document walk: element tree (with an explicit stack, no recursion) to filled/stroked paths,
//! with gradient paint servers and `<use>`.

use super::dash;
use super::paint::{self, Gradient};
use super::path::{self, Matrix, Poly, Seg, IDENTITY};
use super::raster::{self, Rasterizer, StrokeStyle};
use super::tree::{Doc, NONE};
use super::{attr, length, numbers, root_transform, view_box, Attrs, Paint, Style, MAX_DEPTH, MAX_ELEMENTS, MAX_POINTS, TOLERANCE};
use crate::draw::Rgba;
use std::collections::HashMap;

/// Bezier constant for quarter-circle arcs.
const KAPPA: f32 = 0.552_284_8;
/// Nested `<use>` levels followed (ends chains and cycles).
const MAX_USE_DEPTH: u8 = 8;
/// Elements visited in one render, `<use>` expansions included.
const MAX_VISITS: usize = 2 * MAX_ELEMENTS;

struct Ctx<'c, 'd> {
    canvas: &'c mut [u8],
    w: usize,
    h: usize,
    rast: Rasterizer,
    segs: Vec<Seg>,
    polys: Vec<Poly>,
    local: Vec<Poly>,
    dashed: Vec<Poly>,
    doc:&'d Doc<'d>,
    viewport: (f32, f32),
    current: Rgba,
    /// Gradients resolved so far, by node index (`None` = paints nothing). Each gradient is
    /// resolved at most once per document; only the shape's box is applied per shape.
    grads: HashMap<u32, Option<Gradient>>,
}

/// One open container: the next child to visit and the style its children inherit.
struct Frame {
    next: u32,
    /// Visit only `next` (the target of a `<use>`), not its siblings.
    one: bool,
    style: Style,
    uses: u8,
}

enum Fill {
    Solid([f32; 4]),
    /// A cached gradient (node index) and the opacity it is painted with.
    Gradient(u32, f32),
}

/// Render into a premultiplied RGBA8 canvas. `None` if the document has no `<svg>` root.
pub(super) fn render(source: &str, canvas: &mut [u8], w: usize, h: usize, current: Rgba) -> Option<()> {
    let doc = Doc::parse(source)?;
    let ids = |id: &str| doc.gradient(id);
    let root_attrs: &Attrs = &doc.node(0).attrs;
    let mut root = Style::root(current).child(root_attrs, &ids);
    root.transform = path::mul(&root_transform(root_attrs, w as f32, h as f32), &root.transform);
    if root.hidden {
        return Some(());
    }
    let viewport = match view_box(root_attrs) {
        Some(v) => (v[2], v[3]),
        None => (
            attr(root_attrs, "width").and_then(length).filter(|v| *v > 0.0).unwrap_or(w as f32),
            attr(root_attrs, "height").and_then(length).filter(|v| *v > 0.0).unwrap_or(h as f32),
        ),
    };
    let mut ctx = Ctx { canvas, w, h, rast: Rasterizer::default(), segs: Vec::new(), polys: Vec::new(), local: Vec::new(), dashed: Vec::new(), doc: &doc, viewport, current, grads: HashMap::new() };
    let mut stack = vec![Frame { next: doc.node(0).first, one: false, style: root, uses: 0 }];
    let mut visits = 0usize;
    while let Some(top) = stack.last_mut() {
        let idx = top.next;
        if idx == NONE {
            stack.pop();
            continue;
        }
        let via_use = top.one;
        top.next = if top.one { NONE } else { doc.node(idx).next };
        let (parent, uses) = (top.style, top.uses);
        visits += 1;
        if visits > MAX_VISITS {
            break;
        }
        let node = doc.node(idx);
        let name = node.name.as_str();
        match name {
            "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" => {
                let st = parent.child(&node.attrs, &ids);
                if !st.hidden {
                    ctx.segs.clear();
                    build_shape(name, &node.attrs, &mut ctx.segs);
                    let path_length = attr(&node.attrs, "pathLength")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .filter(|v| v.is_finite() && *v > 0.0);
                    ctx.draw(&st, path_length);
                }
            }
            "svg" | "g" | "a" | "switch" | "symbol" => {
                if name == "symbol" && !via_use {
                    continue; // only drawn through <use>
                }
                let mut st = parent.child(&node.attrs, &ids);
                if st.hidden || stack.len() >= MAX_DEPTH * (MAX_USE_DEPTH as usize + 1) {
                    continue;
                }
                if name == "svg" {
                    // Nested viewport: honour its position only.
                    let x = attr(&node.attrs, "x").and_then(length).unwrap_or(0.0);
                    let y = attr(&node.attrs, "y").and_then(length).unwrap_or(0.0);
                    st.transform = path::mul(&st.transform, &[1.0, 0.0, 0.0, 1.0, x, y]);
                }
                stack.push(Frame { next: node.first, one: false, style: st, uses });
            }
            "use" => {
                if uses >= MAX_USE_DEPTH {
                    continue;
                }
                let Some(target) = doc.href(idx) else { continue };
                let mut st = parent.child(&node.attrs, &ids);
                if st.hidden {
                    continue;
                }
                let x = attr(&node.attrs, "x").and_then(length).unwrap_or(0.0);
                let y = attr(&node.attrs, "y").and_then(length).unwrap_or(0.0);
                st.transform = path::mul(&st.transform, &[1.0, 0.0, 0.0, 1.0, x, y]);
                stack.push(Frame { next: target, one: true, style: st, uses: uses + 1 });
            }
            _ => {} // defs, gradients, metadata, unsupported elements: not drawn
        }
    }
    Some(())
}

impl Ctx<'_, '_> {
    fn paint(&mut self, st: &Style, p: Paint, alpha: f32) -> Option<Fill> {
        match p {
            Paint::Server(i) => {
                let a = alpha * st.opacity;
                if a <= 0.0 {
                    return None;
                }
                let (doc, viewport, current) = (self.doc, self.viewport, self.current);
                let g = self.grads.entry(i).or_insert_with(|| paint::resolve(doc, i, viewport, current));
                g.is_some().then_some(Fill::Gradient(i, a))
            }
            _ => st.resolve(p, alpha).map(Fill::Solid),
        }
    }

    /// Flatten the current shape in its own user space (into `local`), with a tolerance that
    /// stays about `TOLERANCE` device pixels under `m`.
    fn flatten_local(&mut self, m: &Matrix) {
        let scale = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
        let tol = if scale.is_finite() && scale > 1e-3 { TOLERANCE / scale } else { TOLERANCE };
        self.local.clear();
        path::flatten(&self.segs, &IDENTITY, tol, MAX_POINTS, &mut self.local);
    }

    /// Cut the stroke into dashes (device space, into `dashed`). `false`: stroke solid.
    fn dashes(&mut self, st: &Style, m: &Matrix, path_length: Option<f32>) -> bool {
        if st.dash.is_solid() {
            return false;
        }
        self.flatten_local(m);
        let k = match path_length {
            Some(pl) => dash::outline_length(&self.local) / pl,
            None => 1.0,
        };
        let (vw, vh) = self.viewport;
        let pct_ref = ((vw * vw + vh * vh) * 0.5).sqrt();
        let Some((pattern, n)) = st.dash.resolve(pct_ref, k) else { return false };
        let (off, pct) = st.dash_offset;
        let offset = if pct { off * pct_ref } else { off } * k;
        self.dashed.clear();
        if dash::dash(&self.local, &pattern[..n], offset, &mut self.dashed).is_none() {
            self.dashed.clear();
            return false;
        }
        for p in &mut self.dashed {
            for q in &mut p.pts {
                *q = path::apply(m, *q);
            }
        }
        true
    }

    /// Bounding box of the current shape's geometry in its own user space.
    fn bbox(&mut self, m: &Matrix) -> Option<[f32; 4]> {
        self.flatten_local(m);
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for p in self.local.iter().flat_map(|p| p.pts.iter()) {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
        (b.iter().all(|v| v.is_finite()) && b[0] <= b[2] && b[1] <= b[3]).then_some(b)
    }

    /// Composite the edges collected in the rasterizer with `fill`.
    fn composite(&mut self, fill: &Fill, m: &Matrix, even_odd: bool) {
        match fill {
            Fill::Solid(c) => self.rast.fill(self.canvas, self.w, self.h, even_odd, c),
            Fill::Gradient(i, a) => {
                let needs_bbox = self.grads.get(i).is_some_and(|g| g.as_ref().is_some_and(Gradient::needs_bbox));
                let bbox = if needs_bbox { self.bbox(m) } else { None };
                let Some(Some(g)) = self.grads.get(i) else { return self.rast.clear() };
                match g.shader(m, bbox, *a) {
                    Some(s) => self.rast.fill(self.canvas, self.w, self.h, even_odd, &s),
                    None => self.rast.clear(),
                }
            }
        }
    }

    fn draw(&mut self, st: &Style, path_length: Option<f32>) {
        if self.segs.is_empty() {
            return;
        }
        let m = st.transform;
        self.polys.clear();
        path::flatten(&self.segs, &m, TOLERANCE, MAX_POINTS, &mut self.polys);
        if let Some(fill) = self.paint(st, st.fill, st.fill_opacity) {
            self.rast.clear();
            for p in &self.polys {
                self.rast.polygon(&p.pts);
            }
            self.composite(&fill, &m, st.even_odd);
        }
        if let Some(fill) = self.paint(st, st.stroke, st.stroke_opacity) {
            let scale = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
            let style = StrokeStyle { width: st.stroke_width * scale, cap: st.cap, join: st.join, miter_limit: st.miter_limit };
            let dashed = self.dashes(st, &m, path_length);
            self.rast.clear();
            let pieces = if dashed { &self.dashed } else { &self.polys };
            for p in pieces {
                raster::stroke(&mut self.rast, p, &style);
            }
            self.composite(&fill, &m, false);
        }
    }
}

fn num(attrs: &Attrs, name: &str) -> f32 {
    attr(attrs, name).and_then(length).unwrap_or(0.0)
}

fn ellipse(out: &mut Vec<Seg>, cx: f32, cy: f32, rx: f32, ry: f32) {
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    out.push(Seg::Move([cx + rx, cy]));
    out.push(Seg::Cubic([cx + rx, cy + ky], [cx + kx, cy + ry], [cx, cy + ry]));
    out.push(Seg::Cubic([cx - kx, cy + ry], [cx - rx, cy + ky], [cx - rx, cy]));
    out.push(Seg::Cubic([cx - rx, cy - ky], [cx - kx, cy - ry], [cx, cy - ry]));
    out.push(Seg::Cubic([cx + kx, cy - ry], [cx + rx, cy - ky], [cx + rx, cy]));
    out.push(Seg::Close);
}

fn build_shape(name: &str, attrs: &Attrs, out: &mut Vec<Seg>) {
    match name {
        "path" => {
            if let Some(d) = attr(attrs, "d") {
                path::parse(d, out);
            }
        }
        "rect" => {
            let (x, y, w, h) = (num(attrs, "x"), num(attrs, "y"), num(attrs, "width"), num(attrs, "height"));
            if !(w > 0.0 && h > 0.0) {
                return;
            }
            let rx = attr(attrs, "rx").and_then(length).filter(|v| *v >= 0.0);
            let ry = attr(attrs, "ry").and_then(length).filter(|v| *v >= 0.0);
            let (rx, ry) = match (rx, ry) {
                (Some(a), Some(b)) => (a, b),
                (Some(a), None) | (None, Some(a)) => (a, a),
                (None, None) => (0.0, 0.0),
            };
            let (rx, ry) = (rx.min(w / 2.0), ry.min(h / 2.0));
            if rx <= 0.0 || ry <= 0.0 {
                out.extend([Seg::Move([x, y]), Seg::Line([x + w, y]), Seg::Line([x + w, y + h]), Seg::Line([x, y + h]), Seg::Close]);
                return;
            }
            let (kx, ky) = (rx * KAPPA, ry * KAPPA);
            let (r, b) = (x + w, y + h);
            out.extend([
                Seg::Move([x + rx, y]),
                Seg::Line([r - rx, y]),
                Seg::Cubic([r - rx + kx, y], [r, y + ry - ky], [r, y + ry]),
                Seg::Line([r, b - ry]),
                Seg::Cubic([r, b - ry + ky], [r - rx + kx, b], [r - rx, b]),
                Seg::Line([x + rx, b]),
                Seg::Cubic([x + rx - kx, b], [x, b - ry + ky], [x, b - ry]),
                Seg::Line([x, y + ry]),
                Seg::Cubic([x, y + ry - ky], [x + rx - kx, y], [x + rx, y]),
                Seg::Close,
            ]);
        }
        "circle" => {
            let r = num(attrs, "r");
            if r > 0.0 {
                ellipse(out, num(attrs, "cx"), num(attrs, "cy"), r, r);
            }
        }
        "ellipse" => {
            let (rx, ry) = (num(attrs, "rx"), num(attrs, "ry"));
            if rx > 0.0 && ry > 0.0 {
                ellipse(out, num(attrs, "cx"), num(attrs, "cy"), rx, ry);
            }
        }
        "line" => {
            out.push(Seg::Move([num(attrs, "x1"), num(attrs, "y1")]));
            out.push(Seg::Line([num(attrs, "x2"), num(attrs, "y2")]));
        }
        "polyline" | "polygon" => {
            let v = numbers(attr(attrs, "points").unwrap_or(""));
            let pairs = (v.len() / 2).min(path::MAX_SEGS);
            for i in 0..pairs {
                let p = [v[2 * i], v[2 * i + 1]];
                out.push(if i == 0 { Seg::Move(p) } else { Seg::Line(p) });
            }
            if name == "polygon" && pairs > 0 {
                out.push(Seg::Close);
            }
        }
        _ => {}
    }
}
