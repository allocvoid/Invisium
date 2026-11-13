//! H2: the drawing buffer, Present and compositing.

use super::*;
use crate::draw::{DrawList, GlyphBitmap, GlyphSource, Layer, Rect, Rgba, NO_RADII};

struct NoGlyphs;
impl GlyphSource for NoGlyphs {
    fn rasterize(&self, _: u16, _: u16, _: f32, _: u8) -> Option<GlyphBitmap> {
        None
    }
}

/// Render the context image over a blue page and read the frame (top row first).
pub(super) fn composite(h: &mut H, w: u32, hh: u32) -> Vec<u8> {
    let img = h.dev.borrow().image(h.ctx);
    let mut l = DrawList::new();
    l.image(Rect::new(0.0, 0.0, w as f32, hh as f32), img, NO_RADII, 1.0);
    let layer = Layer { list: &l, glyphs: &[], viewport: Rect::new(0.0, 0.0, w as f32, hh as f32), scroll: (0.0, 0.0), scale: 1.0 };
    h.r.render(&[layer], &NoGlyphs, Rgba::rgb(0, 0, 255)).expect("render");
    h.r.read_rgba().expect("read_rgba")
}

fn at(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

#[test]
fn present_shows_colour_and_nothing_before() {
    let Some(mut h) = h(8, 8) else { return };
    h.run(vec![cfg(8, 8), clear(Target::Surface, [1.0, 0.0, 0.0, 1.0])]);
    let b = composite(&mut h, 8, 8);
    assert_near(at(&b, 8, 4, 4), [0, 0, 255, 255], 0); // not presented yet
    h.run(vec![Cmd::Present]);
    let b = composite(&mut h, 8, 8);
    assert_near(at(&b, 8, 4, 4), [255, 0, 0, 255], 1);
    assert_near(at(&b, 8, 0, 0), [255, 0, 0, 255], 1);
}

#[test]
fn scissored_clear_of_low_rows_is_lower_half_on_screen() {
    let Some(mut h) = h(8, 8) else { return };
    let mut green = clear(Target::Surface, [0.0, 1.0, 0.0, 1.0]);
    if let Cmd::Clear { scissor, .. } = &mut green {
        *scissor = Some([0, 0, 8, 4]);
    }
    h.run(vec![cfg(8, 8), clear(Target::Surface, [0.0, 0.0, 0.0, 1.0]), green, Cmd::Present]);
    let b = composite(&mut h, 8, 8);
    assert_near(at(&b, 8, 3, 0), [0, 0, 0, 255], 1);
    assert_near(at(&b, 8, 3, 3), [0, 0, 0, 255], 1);
    assert_near(at(&b, 8, 3, 4), [0, 255, 0, 255], 1);
    assert_near(at(&b, 8, 3, 7), [0, 255, 0, 255], 1);
}

#[test]
fn opaque_surface_ignores_alpha() {
    let Some(mut h) = h(4, 4) else { return };
    let mut c = clear(Target::Surface, [1.0, 0.0, 0.0, 0.0]);
    if let Cmd::Clear { color_mask, .. } = &mut c {
        *color_mask = [true, true, true, false];
    }
    let configure = Cmd::Configure { width: 4, height: 4, alpha: false, depth: false, stencil: false, samples: 1, premultiplied: true, preserve: false };
    h.run(vec![configure, c, Cmd::Present]);
    let b = composite(&mut h, 4, 4);
    assert_near(at(&b, 4, 1, 1), [255, 0, 0, 255], 1);
    // A fresh opaque drawing buffer reads alpha 1.
    assert_near(h.surf(0, 0), [0, 0, 0, 255], 0);
}

#[test]
fn premultiplied_and_straight_composite_alike() {
    for premultiplied in [true, false] {
        let Some(mut h) = h(4, 4) else { return };
        let c = if premultiplied { [0.5, 0.0, 0.0, 0.5] } else { [1.0, 0.0, 0.0, 0.5] };
        let configure = Cmd::Configure { width: 4, height: 4, alpha: true, depth: false, stencil: false, samples: 1, premultiplied, preserve: false };
        h.run(vec![configure, clear(Target::Surface, c), Cmd::Present]);
        let b = composite(&mut h, 4, 4);
        assert_near(at(&b, 4, 2, 2), [128, 0, 127, 255], 2);
    }
}

#[test]
fn cleared_after_present_unless_preserved() {
    for preserve in [false, true] {
        let Some(h) = h(4, 4) else { return };
        let configure = Cmd::Configure { width: 4, height: 4, alpha: true, depth: true, stencil: false, samples: 1, premultiplied: true, preserve };
        h.run(vec![configure, clear(Target::Surface, [1.0, 0.0, 0.0, 1.0]), Cmd::Present]);
        let want = if preserve { [255, 0, 0, 255] } else { [0, 0, 0, 0] };
        assert_near(h.surf(2, 2), want, 0);
    }
}

#[test]
fn second_configure_resizes() {
    let Some(h) = h(4, 4) else { return };
    h.run(vec![cfg(8, 8), cfg(16, 4), clear(Target::Surface, [0.0, 0.0, 1.0, 1.0])]);
    assert_near(h.surf(15, 3), [0, 0, 255, 255], 0);
    assert!(h.read(&Target::Surface, 0, [0, 4, 1, 1]).is_none());
    assert!(h.read(&Target::Surface, 0, [16, 0, 1, 1]).is_none());
}

#[test]
fn multisampled_surface_edge_has_partial_coverage() {
    let Some(mut h) = h(8, 8) else { return };
    let configure = Cmd::Configure { width: 8, height: 8, alpha: true, depth: false, stencil: false, samples: 4, premultiplied: true, preserve: true };
    let tri = f32s(&[-1.0, -1.0, 1.0, -1.0, -1.0, 1.0]);
    h.run(vec![
        configure,
        basic_program(),
        buffer(1, tri),
        clear(Target::Surface, [0.0, 0.0, 0.0, 1.0]),
        Cmd::Draw(Box::new(draw(Target::Surface, 1, st(8, 8), vec![attr(0, 1, AttrType::F32, 2, 8, 0)], 3, uni([1.0, 0.0, 0.0, 1.0])))),
        Cmd::Present,
    ]);
    let (fmt, px) = h.read(&Target::Surface, 0, [0, 0, 8, 8]).expect("read multisampled surface");
    assert_eq!(fmt, TexFormat::Rgba8);
    let red: Vec<u8> = px.chunks(4).map(|p| p[0]).collect();
    assert_eq!(red[0], 255, "inside");
    assert_eq!(red[7 * 8 + 7], 0, "outside");
    let edge = (0..8).map(|i| red[i * 8 + (7 - i)]).collect::<Vec<_>>();
    assert!(edge.iter().any(|&v| v > 20 && v < 235), "edge pixels {edge:?}");
    // The presented image is resolved too.
    let b = composite(&mut h, 8, 8);
    assert_near(at(&b, 8, 0, 7), [255, 0, 0, 255], 1); // row 0 (inside) is the bottom row
}
