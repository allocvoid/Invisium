//! Document walk: element tree (with an explicit stack, no recursion) to filled/stroked paths.

use super::path::{self, Poly, Seg};
use super::raster::{self, Rasterizer, StrokeStyle};
use super::xml::{Event, Reader};
use super::{attr, length, numbers, root_transform, Attrs, Style, MAX_DEPTH, MAX_ELEMENTS, MAX_POINTS, TOLERANCE};
use crate::draw::Rgba;

/// Bezier constant for quarter-circle arcs.
const KAPPA: f32 = 0.552_284_8;

struct Ctx<'c> {
    canvas: &'c mut [u8],
    w: usize,
    h: usize,
    rast: Rasterizer,
    segs: Vec<Seg>,
    polys: Vec<Poly>,
}

/// Render into a premultiplied RGBA8 canvas. `None` if the document has no `<svg>` root.
pub(super) fn render(source: &str, canvas: &mut [u8], w: usize, h: usize, current: Rgba) -> Option<()> {
    let mut reader = Reader::new(source);
    let Some(Event::Start { name, attrs, self_closing }) = reader.next() else { return None };
    if !name.eq_ignore_ascii_case("svg") {
        return None;
    }
    let mut root = Style::root(current).child(&attrs);
    root.transform = path::mul(&root_transform(&attrs, w as f32, h as f32), &root.transform);
    if self_closing || root.hidden {
        return Some(());
    }
    let mut ctx = Ctx { canvas, w, h, rast: Rasterizer::default(), segs: Vec::new(), polys: Vec::new() };
    let mut stack: Vec<Style> = vec![root];
    let mut skip = 0usize;
    let mut count = 0usize;
    for ev in reader {
        match ev {
            Event::End => {
                if skip > 0 {
                    skip -= 1;
                } else if stack.len() > 1 {
                    stack.pop();
                } else {
                    break; // the root closed
                }
            }
            Event::Start { name, attrs, self_closing } => {
                count += 1;
                if count > MAX_ELEMENTS {
                    break;
                }
                if skip > 0 {
                    if !self_closing {
                        skip += 1;
                    }
                    continue;
                }
                let parent = *stack.last()?;
                let lname = name.to_ascii_lowercase();
                let container = matches!(lname.as_str(), "svg" | "g" | "a" | "switch");
                let shape = matches!(lname.as_str(), "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon");
                let style = if container || shape { Some(parent.child(&attrs)) } else { None };
                if shape {
                    if let Some(st) = style.filter(|s| !s.hidden) {
                        ctx.segs.clear();
                        build_shape(&lname, &attrs, &mut ctx.segs);
                        ctx.draw(&st);
                    }
                }
                if self_closing {
                    continue;
                }
                match style {
                    Some(mut st) if container && !st.hidden && stack.len() < MAX_DEPTH => {
                        if lname == "svg" {
                            // Nested viewport: honour its position only.
                            let x = attr(&attrs, "x").and_then(length).unwrap_or(0.0);
                            let y = attr(&attrs, "y").and_then(length).unwrap_or(0.0);
                            st.transform = path::mul(&st.transform, &[1.0, 0.0, 0.0, 1.0, x, y]);
                        }
                        stack.push(st);
                    }
                    _ => skip = 1,
                }
            }
        }
    }
    Some(())
}

impl Ctx<'_> {
    fn draw(&mut self, st: &Style) {
        if self.segs.is_empty() {
            return;
        }
        let m = st.transform;
        self.polys.clear();
        path::flatten(&self.segs, &m, TOLERANCE, MAX_POINTS, &mut self.polys);
        if let Some(color) = st.resolve(st.fill, st.fill_opacity) {
            self.rast.clear();
            for p in &self.polys {
                self.rast.polygon(&p.pts);
            }
            self.rast.fill(self.canvas, self.w, self.h, st.even_odd, color);
        }
        if let Some(color) = st.resolve(st.stroke, st.stroke_opacity) {
            let scale = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
            let style = StrokeStyle { width: st.stroke_width * scale, cap: st.cap, join: st.join, miter_limit: st.miter_limit };
            self.rast.clear();
            for p in &self.polys {
                raster::stroke(&mut self.rast, p, &style);
            }
            self.rast.fill(self.canvas, self.w, self.h, false, color);
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
