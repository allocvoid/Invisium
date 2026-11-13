//! Shadows: the drawing's alpha, offset and blurred (three box blurs ≈ a Gaussian with
//! standard deviation `shadowBlur / 2`).

use crate::geom::R;
use crate::paint::{Shader, C4};
use crate::raster::Mask;

/// Largest standard deviation used; larger blurs are drawn with this one.
const MAX_SIGMA: f64 = 300.0;
/// Most working bytes a shadow may use; above it the shadow is drawn without blur, and if
/// that is still too much, not at all.
const MAX_WORK: usize = 64 << 20;

/// The shadow's coverage mask, already offset; `mk` rasterises the drawing inside a region
/// and `bb` bounds what it can cover. Working memory is checked before anything is allocated.
#[allow(clippy::too_many_arguments)]
pub fn shadow_mask(
    mk: &dyn Fn(R) -> Mask,
    bb: R,
    shader: &Shader,
    alpha: f32,
    blur: f64,
    ox: f64,
    oy: f64,
    canvas: R,
) -> Mask {
    let sigma = (blur / 2.0).clamp(0.0, MAX_SIGMA);
    let d = (sigma * 3.0 * (2.0 * std::f64::consts::PI).sqrt() / 4.0 + 0.5).floor() as i64;
    let mut r = if blur > 0.0 { (d / 2).max(if sigma > 0.25 { 1 } else { 0 }) } else { 0 };
    let lim = (canvas.width() + canvas.height()) as f64 + (3 * r + 1) as f64 + 1.0;
    if !(ox.abs() < lim && oy.abs() < lim) {
        return Mask::empty();
    }
    let (dx, dy) = (ox.round() as i64, oy.round() as i64);
    // Region the shadow is taken from, the drawing's mask inside it and the shadow layer.
    let plan = |r: i64| {
        let ext = 3 * r + 1;
        let region = canvas.shift(-dx, -dy).expand(ext);
        let mr = bb.intersect(&region);
        let lr = mr.expand(ext).intersect(&region);
        (ext, region, mr, lr)
    };
    // Working memory: the drawing's mask, the shadow layer and one row of colours.
    let area = |q: R| if q.is_empty() { 0 } else { q.width() * q.height() };
    let work = |mr: R, lr: R| area(mr) + area(lr) + mr.width() * std::mem::size_of::<C4>();
    let (mut ext, mut region, mut mr, mut lr) = plan(r);
    if work(mr, lr) > MAX_WORK && r > 0 {
        r = 0;
        (ext, region, mr, lr) = plan(0);
    }
    if mr.is_empty() || work(mr, lr) > MAX_WORK {
        return Mask::empty();
    }
    let m = mk(region);
    if m.r.is_empty() {
        return Mask::empty();
    }
    let lr = m.r.expand(ext).intersect(&region);
    let mut layer = Mask::new(lr);
    let w = lr.width();
    let mut row: Vec<C4> = vec![[0.0; 4]; m.r.width()];
    for y in m.r.y0..m.r.y1 {
        shader.shade_row(y, m.r.x0, &mut row);
        for (i, x) in (m.r.x0..m.r.x1).enumerate() {
            let c = m.get(x, y) as f32 * row[i][3] * alpha;
            if c > 0.0 && x >= lr.x0 && x < lr.x1 && y >= lr.y0 && y < lr.y1 {
                layer.data[(y - lr.y0) as usize * w + (x - lr.x0) as usize] = (c + 0.5).min(255.0) as u8;
            }
        }
    }
    if r > 0 {
        let h = lr.height();
        for _ in 0..3 {
            box_blur(&mut layer.data, w, h, 1, w, r as usize);
            box_blur(&mut layer.data, h, w, w, 1, r as usize);
        }
    }
    layer.shifted(dx, dy)
}

/// One box blur pass over `lines` lines of `n` samples; sample `i` of line `l` is at
/// `l * line_step + i * step`.
fn box_blur(data: &mut [u8], n: usize, lines: usize, step: usize, line_step: usize, r: usize) {
    let win = (2 * r + 1) as u32;
    let mut tmp = vec![0u8; n];
    for l in 0..lines {
        let base = l * line_step;
        let at = |i: usize| base + i * step;
        let mut sum: u32 = 0;
        for i in 0..r.min(n) {
            sum += data[at(i)] as u32;
        }
        for (i, t) in tmp.iter_mut().enumerate() {
            if i + r < n {
                sum += data[at(i + r)] as u32;
            }
            *t = ((sum + win / 2) / win) as u8;
            if i >= r {
                sum -= data[at(i - r)] as u32;
            }
        }
        for (i, t) in tmp.iter().enumerate() {
            data[at(i)] = *t;
        }
    }
}
