//! Milestone 1 acceptance tests. Headless only: no window is ever created.

use ab_glyph::{Font, FontVec, ScaleFont};
use invisium_gfx::draw::{DrawList, Glyph, GlyphBitmap, GlyphSource, Item, Layer, Rect, Rgba, Stop, NO_RADII};
use invisium_gfx::{GpuOptions, Renderer};
use std::sync::Mutex;

static GPU: Mutex<()> = Mutex::new(());

const WHITE: Rgba = Rgba::WHITE;
const RED: Rgba = Rgba::rgb(255, 0, 0);
const GREEN: Rgba = Rgba::rgb(0, 255, 0);
const BLUE: Rgba = Rgba::rgb(0, 0, 255);

fn gpu_lock() -> std::sync::MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn renderer_opts(w: u32, h: u32, opts: GpuOptions) -> Option<Renderer> {
    match pollster::block_on(Renderer::new_headless(w, h, opts)) {
        Ok(r) => Some(r),
        Err(e) if e.contains("no GPU adapter") => {
            eprintln!("SKIP: {e}");
            None
        }
        Err(e) => panic!("renderer creation failed: {e}"),
    }
}

fn renderer(w: u32, h: u32) -> Option<Renderer> {
    renderer_opts(w, h, GpuOptions::default())
}

struct NoGlyphs;
impl GlyphSource for NoGlyphs {
    fn rasterize(&self, _: u16, _: u16, _: f32, _: u8) -> Option<GlyphBitmap> {
        None
    }
}

struct Arial {
    font: FontVec,
}

impl Arial {
    fn load() -> Arial {
        let bytes = std::fs::read(r"C:\Windows\Fonts\arial.ttf").expect("arial.ttf");
        Arial { font: FontVec::try_from_vec(bytes).expect("parse arial") }
    }
    fn run(&self, text: &str, size: f32, out: &mut Vec<Glyph>) -> f32 {
        let scaled = self.font.as_scaled(size);
        let mut total = 0.0;
        for ch in text.chars() {
            let id = self.font.glyph_id(ch);
            let adv = scaled.h_advance(id);
            out.push(Glyph { face: 0, id: id.0, adv });
            total += adv;
        }
        total
    }
}

impl GlyphSource for Arial {
    fn rasterize(&self, _face: u16, glyph: u16, px: f32, subpx: u8) -> Option<GlyphBitmap> {
        let g = ab_glyph::GlyphId(glyph).with_scale_and_position(px, ab_glyph::point(subpx as f32 / 4.0, 0.0));
        let outline = self.font.outline_glyph(g)?;
        let b = outline.px_bounds();
        let (w, h) = (b.width() as u32, b.height() as u32);
        if w == 0 || h == 0 {
            return None;
        }
        let mut data = vec![0u8; (w * h) as usize];
        outline.draw(|x, y, c| {
            if x < w && y < h {
                data[(y * w + x) as usize] = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        });
        Some(GlyphBitmap { width: w, height: h, left: b.min.x as i32, top: b.min.y as i32, data })
    }
}

fn layer<'a>(list: &'a DrawList, glyphs: &'a [Glyph], w: f32, h: f32) -> Layer<'a> {
    Layer { list, glyphs, viewport: Rect::new(0.0, 0.0, w, h), scroll: (0.0, 0.0), scale: 1.0 }
}

fn draw(r: &mut Renderer, layers: &[Layer], src: &dyn GlyphSource) -> Vec<u8> {
    r.render(layers, src, WHITE).expect("render");
    r.read_rgba().expect("read")
}

fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

#[track_caller]
fn assert_px(buf: &[u8], w: u32, x: u32, y: u32, want: [u8; 3], tol: u8) {
    let p = px(buf, w, x, y);
    let ok = (0..3).all(|i| p[i].abs_diff(want[i]) <= tol);
    assert!(ok, "pixel ({x},{y}) = {p:?}, want {want:?} ±{tol}");
}

fn lum(buf: &[u8], w: u32, x: u32, y: u32) -> u32 {
    let p = px(buf, w, x, y);
    p[0] as u32 + p[1] as u32 + p[2] as u32
}


// ---------------------------------------------------------------- A2: pixel tests

#[test]
fn solid_rect() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let mut l = DrawList::new();
    l.rect(Rect::new(10.0, 10.0, 30.0, 20.0), RED);
    let b = draw(&mut r, &[layer(&l, &[], 100.0, 100.0)], &NoGlyphs);
    assert_eq!(b.len(), 100 * 100 * 4);
    assert_px(&b, 100, 20, 20, [255, 0, 0], 0);
    assert_px(&b, 100, 10, 10, [255, 0, 0], 0);
    assert_px(&b, 100, 39, 29, [255, 0, 0], 0);
    assert_px(&b, 100, 40, 15, [255, 255, 255], 0);
    assert_px(&b, 100, 5, 5, [255, 255, 255], 0);
    assert_px(&b, 100, 20, 30, [255, 255, 255], 0);
}

#[test]
fn rect_under_scale_2() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let mut l = DrawList::new();
    l.rect(Rect::new(10.0, 10.0, 20.0, 20.0), RED);
    let mut ly = layer(&l, &[], 100.0, 100.0);
    ly.scale = 2.0;
    let b = draw(&mut r, &[ly], &NoGlyphs);
    assert_px(&b, 100, 21, 21, [255, 0, 0], 0);
    assert_px(&b, 100, 58, 58, [255, 0, 0], 0);
    assert_px(&b, 100, 61, 61, [255, 255, 255], 0);
    assert_px(&b, 100, 18, 18, [255, 255, 255], 0);
}

#[test]
fn scroll_vs_fixed() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let mut l = DrawList::new();
    l.rect(Rect::new(10.0, 30.0, 10.0, 10.0), RED);
    l.push(Item::PushFixed);
    l.rect(Rect::new(30.0, 30.0, 10.0, 10.0), BLUE);
    l.push(Item::PopFixed);
    let mut ly = layer(&l, &[], 100.0, 100.0);
    ly.scroll = (0.0, 20.0);
    let b = draw(&mut r, &[ly], &NoGlyphs);
    assert_px(&b, 100, 15, 15, [255, 0, 0], 0); // scrolled up by 20
    assert_px(&b, 100, 15, 35, [255, 255, 255], 0);
    assert_px(&b, 100, 35, 35, [0, 0, 255], 0); // fixed: not scrolled
    assert_px(&b, 100, 35, 15, [255, 255, 255], 0);
}

#[test]
fn viewport_clipping() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let mut l = DrawList::new();
    l.rect(Rect::new(-500.0, -500.0, 2000.0, 2000.0), RED);
    l.rect(Rect::new(0.0, 0.0, 5.0, 5.0), BLUE);
    let ly = Layer { list: &l, glyphs: &[], viewport: Rect::new(20.0, 20.0, 40.0, 40.0), scroll: (0.0, 0.0), scale: 1.0 };
    let b = draw(&mut r, &[ly], &NoGlyphs);
    assert_px(&b, 100, 10, 10, [255, 255, 255], 0);
    assert_px(&b, 100, 22, 22, [0, 0, 255], 0); // list origin lands on viewport origin
    assert_px(&b, 100, 40, 40, [255, 0, 0], 0);
    assert_px(&b, 100, 59, 59, [255, 0, 0], 0);
    assert_px(&b, 100, 60, 60, [255, 255, 255], 0);
    assert_px(&b, 100, 70, 30, [255, 255, 255], 0);
}

#[test]
fn nested_clips_and_unbalanced_pops() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let mut l = DrawList::new();
    l.push(Item::PopClip); // unbalanced: ignored
    l.push(Item::PopFixed); // unbalanced: ignored
    l.push_clip(Rect::new(10.0, 10.0, 50.0, 50.0), NO_RADII);
    l.push_clip(Rect::new(30.0, 30.0, 50.0, 50.0), NO_RADII);
    l.rect(Rect::new(0.0, 0.0, 100.0, 100.0), RED);
    l.pop_clip();
    l.pop_clip();
    l.pop_clip(); // extra
    l.rect(Rect::new(80.0, 80.0, 10.0, 10.0), BLUE);
    l.push_clip(Rect::new(0.0, 0.0, 5.0, 5.0), NO_RADII); // never popped
    let b = draw(&mut r, &[layer(&l, &[], 100.0, 100.0)], &NoGlyphs);
    assert_px(&b, 100, 35, 35, [255, 0, 0], 0);
    assert_px(&b, 100, 59, 59, [255, 0, 0], 0);
    assert_px(&b, 100, 20, 20, [255, 255, 255], 0);
    assert_px(&b, 100, 65, 65, [255, 255, 255], 0);
    assert_px(&b, 100, 85, 85, [0, 0, 255], 0);
}

#[test]
fn rounded_rect_and_rounded_clip() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let mut l = DrawList::new();
    l.rounded_rect(Rect::new(10.5, 10.5, 60.0, 60.0), RED, [20.0; 4]);
    let b = draw(&mut r, &[layer(&l, &[], 100.0, 100.0)], &NoGlyphs);
    assert_px(&b, 100, 11, 11, [255, 255, 255], 0); // corner: background
    assert_px(&b, 100, 40, 40, [255, 0, 0], 0); // centre: fill
    let edge = px(&b, 100, 10, 40); // left edge at x = 10.5: half covered
    assert_eq!(edge[0], 255);
    assert!((100..=155).contains(&edge[1]), "edge pixel {edge:?}");

    // Rounded clip: a full rect clipped by a circle.
    let mut l = DrawList::new();
    l.push_clip(Rect::new(20.0, 20.0, 40.0, 40.0), [20.0; 4]);
    l.rect(Rect::new(0.0, 0.0, 100.0, 100.0), BLUE);
    l.pop_clip();
    let b = draw(&mut r, &[layer(&l, &[], 100.0, 100.0)], &NoGlyphs);
    assert_px(&b, 100, 21, 21, [255, 255, 255], 0);
    assert_px(&b, 100, 40, 40, [0, 0, 255], 0);
}

#[test]
fn border_four_colours() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let mut l = DrawList::new();
    let yellow = Rgba::rgb(255, 255, 0);
    l.push(Item::Border {
        rect: Rect::new(10.0, 10.0, 60.0, 60.0),
        widths: [10.0; 4],
        colors: [RED, GREEN, BLUE, yellow],
        radii: NO_RADII,
    });
    let b = draw(&mut r, &[layer(&l, &[], 100.0, 100.0)], &NoGlyphs);
    assert_px(&b, 100, 40, 14, [255, 0, 0], 0);
    assert_px(&b, 100, 66, 40, [0, 255, 0], 0);
    assert_px(&b, 100, 40, 66, [0, 0, 255], 0);
    assert_px(&b, 100, 14, 40, [255, 255, 0], 0);
    assert_px(&b, 100, 40, 40, [255, 255, 255], 0); // inside: untouched
    assert_px(&b, 100, 5, 40, [255, 255, 255], 0); // outside
    // Corner diagonal: just above the diagonal in the top-left corner is top (red), below is left.
    assert_px(&b, 100, 15, 12, [255, 0, 0], 0);
    assert_px(&b, 100, 12, 15, [255, 255, 0], 0);
}

#[test]
fn outer_shadow() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let mut l = DrawList::new();
    l.push(Item::Shadow {
        rect: Rect::new(40.0, 40.0, 20.0, 20.0),
        radii: NO_RADII,
        dx: 0.0,
        dy: 0.0,
        blur: 10.0,
        spread: 0.0,
        color: Rgba::BLACK,
        inset: false,
    });
    let b = draw(&mut r, &[layer(&l, &[], 100.0, 100.0)], &NoGlyphs);
    assert_px(&b, 100, 50, 50, [255, 255, 255], 0); // nothing under the box
    assert_px(&b, 100, 41, 41, [255, 255, 255], 0);
    let near = lum(&b, 100, 38, 50);
    let mid = lum(&b, 100, 33, 50);
    let far = lum(&b, 100, 28, 50);
    assert!(near < mid && mid < far, "falloff {near} {mid} {far}");
    assert!(near < 3 * 200, "near box should be dark-ish: {near}");
    assert_eq!(lum(&b, 100, 5, 50), 765);

    // Inset shadow: only inside the box.
    let mut l = DrawList::new();
    l.push(Item::Shadow {
        rect: Rect::new(20.0, 20.0, 60.0, 60.0),
        radii: NO_RADII,
        dx: 0.0,
        dy: 0.0,
        blur: 8.0,
        spread: 0.0,
        color: Rgba::BLACK,
        inset: true,
    });
    let b = draw(&mut r, &[layer(&l, &[], 100.0, 100.0)], &NoGlyphs);
    assert_eq!(lum(&b, 100, 10, 50), 765);
    assert!(lum(&b, 100, 21, 50) < lum(&b, 100, 30, 50));
    assert!(lum(&b, 100, 50, 50) > 700);
}

fn gradient_frame(r: &mut Renderer, angle: f32, stops: &[Stop]) -> Vec<u8> {
    let mut l = DrawList::new();
    let span = l.add_stops(stops);
    l.push(Item::LinearGradient { rect: Rect::new(0.0, 0.0, 100.0, 100.0), radii: NO_RADII, angle, stops: span, repeating: false });
    draw(r, &[layer(&l, &[], 100.0, 100.0)], &NoGlyphs)
}

#[test]
fn linear_gradients() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let bw = [Stop { offset: 0.0, color: Rgba::BLACK }, Stop { offset: 1.0, color: WHITE }];
    // 0deg: towards the top, so black at the bottom.
    let b = gradient_frame(&mut r, 0.0, &bw);
    assert_px(&b, 100, 50, 99, [0, 0, 0], 4);
    assert_px(&b, 100, 50, 0, [255, 255, 255], 4);
    assert_px(&b, 100, 50, 50, [126, 126, 126], 4);
    // 90deg: towards the right.
    let b = gradient_frame(&mut r, 90.0, &bw);
    assert_px(&b, 100, 0, 50, [0, 0, 0], 4);
    assert_px(&b, 100, 99, 50, [255, 255, 255], 4);
    assert_px(&b, 100, 50, 50, [129, 129, 129], 4);
    // 135deg: towards the bottom-right; the anti-diagonal is the middle.
    let b = gradient_frame(&mut r, 135.0, &bw);
    assert_px(&b, 100, 0, 0, [0, 0, 0], 4);
    assert_px(&b, 100, 99, 99, [255, 255, 255], 4);
    assert_px(&b, 100, 50, 50, [129, 129, 129], 4);
    assert_px(&b, 100, 98, 1, [128, 128, 128], 4);
    // Three stops.
    let three = [
        Stop { offset: 0.0, color: RED },
        Stop { offset: 0.5, color: GREEN },
        Stop { offset: 1.0, color: BLUE },
    ];
    let b = gradient_frame(&mut r, 90.0, &three);
    assert_px(&b, 100, 0, 50, [255, 0, 0], 6);
    assert_px(&b, 100, 50, 50, [0, 255, 0], 6);
    assert_px(&b, 100, 99, 50, [0, 0, 255], 6);
    // Transparent stop does not darken (premultiplied interpolation).
    let fade = [Stop { offset: 0.0, color: RED }, Stop { offset: 1.0, color: Rgba::TRANSPARENT }];
    let b = gradient_frame(&mut r, 90.0, &fade);
    assert_px(&b, 100, 50, 50, [255, 127, 127], 5);
}

#[test]
fn radial_gradient() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let mut l = DrawList::new();
    let span = l.add_stops(&[Stop { offset: 0.0, color: WHITE }, Stop { offset: 1.0, color: Rgba::BLACK }]);
    l.push(Item::RadialGradient {
        rect: Rect::new(0.0, 0.0, 100.0, 100.0),
        radii: NO_RADII,
        center: (50.0, 50.0),
        radius: (50.0, 50.0),
        stops: span,
        repeating: false,
    });
    let b = draw(&mut r, &[layer(&l, &[], 100.0, 100.0)], &NoGlyphs);
    assert_px(&b, 100, 50, 50, [255, 255, 255], 6);
    assert_px(&b, 100, 99, 50, [0, 0, 0], 6);
    assert_px(&b, 100, 0, 0, [0, 0, 0], 0);
    assert_px(&b, 100, 75, 50, [128, 128, 128], 6);
}

fn quadrant_image() -> Vec<u8> {
    // 4x4: TL red, TR green, BL blue, BR white (2x2 blocks).
    let mut v = Vec::new();
    for y in 0..4 {
        for x in 0..4 {
            let c = match (x < 2, y < 2) {
                (true, true) => RED,
                (false, true) => GREEN,
                (true, false) => BLUE,
                (false, false) => WHITE,
            };
            v.extend_from_slice(&[c.r, c.g, c.b, c.a]);
        }
    }
    v
}

#[test]
fn images_uv_opacity_rounded() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let img = r.create_image(4, 4, &quadrant_image());
    let solid: Vec<u8> = [0u8, 255, 0, 255].repeat(64);
    let green = r.create_image(8, 8, &solid);
    let mut l = DrawList::new();
    // uv crop to the green quadrant.
    l.push(Item::Image { rect: Rect::new(0.0, 0.0, 40.0, 40.0), image: img, radii: NO_RADII, opacity: 1.0, uv: [0.5, 0.0, 1.0, 0.5], repeat: false });
    // full image at half opacity.
    l.image(Rect::new(50.0, 0.0, 40.0, 40.0), img, NO_RADII, 0.5);
    // rounded image (circle).
    l.image(Rect::new(0.0, 50.0, 40.0, 40.0), green, [20.0; 4], 1.0);
    // unknown image id draws nothing.
    l.image(Rect::new(50.0, 50.0, 40.0, 40.0), 9999, NO_RADII, 1.0);
    let b = draw(&mut r, &[layer(&l, &[], 100.0, 100.0)], &NoGlyphs);
    assert_px(&b, 100, 20, 20, [0, 255, 0], 2);
    assert_px(&b, 100, 10, 10, [0, 255, 0], 2);
    assert_px(&b, 100, 55, 5, [255, 128, 128], 3); // red at 50% over white
    assert_px(&b, 100, 85, 35, [255, 255, 255], 3); // white at 50% over white
    assert_px(&b, 100, 55, 35, [128, 128, 255], 3); // blue at 50%
    assert_px(&b, 100, 1, 51, [255, 255, 255], 0); // rounded corner cut away
    assert_px(&b, 100, 20, 70, [0, 255, 0], 0);
    assert_px(&b, 100, 70, 70, [255, 255, 255], 0);
}

#[test]
fn alpha_blending() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let mut l = DrawList::new();
    l.rect(Rect::new(10.0, 10.0, 50.0, 50.0), BLUE);
    l.rect(Rect::new(30.0, 30.0, 50.0, 50.0), Rgba::new(255, 0, 0, 128));
    let b = draw(&mut r, &[layer(&l, &[], 100.0, 100.0)], &NoGlyphs);
    assert_px(&b, 100, 40, 40, [128, 0, 127], 2);
    assert_px(&b, 100, 70, 70, [255, 127, 127], 2);
    assert_px(&b, 100, 20, 20, [0, 0, 255], 0);
}

#[test]
fn layers_drawn_in_order() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let mut page = DrawList::new();
    page.rect(Rect::new(0.0, 0.0, 100.0, 100.0), RED);
    let mut chrome = DrawList::new();
    chrome.rect(Rect::new(0.0, 0.0, 100.0, 100.0), GREEN);
    let chrome_layer = Layer { list: &chrome, glyphs: &[], viewport: Rect::new(0.0, 0.0, 100.0, 20.0), scroll: (0.0, 0.0), scale: 1.0 };
    let b = draw(&mut r, &[layer(&page, &[], 100.0, 100.0), chrome_layer], &NoGlyphs);
    assert_px(&b, 100, 50, 10, [0, 255, 0], 0);
    assert_px(&b, 100, 50, 50, [255, 0, 0], 0);
    let b = draw(&mut r, &[chrome_layer, layer(&page, &[], 100.0, 100.0)], &NoGlyphs);
    assert_px(&b, 100, 50, 10, [255, 0, 0], 0);
}

// ---------------------------------------------------------------- A3: glyphs

fn has_ink_in_rows(b: &[u8], w: u32, rows: std::ops::Range<u32>) -> bool {
    rows.into_iter().any(|y| (0..w).any(|x| lum(b, w, x, y) < 765 - 30))
}

fn ink_bounds(b: &[u8], w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let mut bb: Option<(u32, u32, u32, u32)> = None;
    for y in 0..h {
        for x in 0..w {
            if lum(b, w, x, y) < 765 - 30 {
                bb = Some(match bb {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    bb
}

#[test]
fn glyph_runs() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(200, 100) else { return };
    let font = Arial::load();
    let mut glyphs = Vec::new();
    let w = font.run("Hello", 24.0, &mut glyphs);
    let mut l = DrawList::new();
    l.glyphs(10.0, 50.0, w, 24.0, Rgba::BLACK, (0, glyphs.len() as u32));
    let b = draw(&mut r, &[layer(&l, &glyphs, 200.0, 100.0)], &font);
    let (x0, y0, x1, y1) = ink_bounds(&b, 200, 100).expect("text produced ink");
    assert!(x0 >= 10 && x1 <= (10.0 + w + 1.0) as u32, "x ink {x0}..{x1}, run width {w}");
    assert!(y0 >= 50 - 24 && y1 <= 50 + 6, "y ink {y0}..{y1}");
    assert!(y1 >= 45 && y0 <= 35, "ink spans cap height: {y0}..{y1}");
    let s1 = r.stats();
    assert!(s1.atlas_glyphs >= 4 && s1.atlas_bytes > 0);

    // Same run again: atlas does not grow.
    let b2 = draw(&mut r, &[layer(&l, &glyphs, 200.0, 100.0)], &font);
    let s2 = r.stats();
    assert_eq!(s1.atlas_glyphs, s2.atlas_glyphs);
    assert_eq!(s1.atlas_bytes, s2.atlas_bytes);
    assert_eq!(b, b2);

    // id == 0 advances without drawing.
    let h = font.font.glyph_id('H').0;
    let gl = [Glyph { face: 0, id: 0, adv: 60.0 }, Glyph { face: 0, id: h, adv: 18.0 }];
    let mut l = DrawList::new();
    l.glyphs(10.0, 50.0, 78.0, 24.0, Rgba::BLACK, (0, 2));
    let b = draw(&mut r, &[layer(&l, &gl, 200.0, 100.0)], &font);
    let (x0, _, x1, _) = ink_bounds(&b, 200, 100).expect("H drawn");
    assert!(x0 >= 70 && x1 <= 90, "H ink at {x0}..{x1}");
}

#[test]
fn atlas_growth_and_eviction() {
    let _g = gpu_lock();
    let opts = GpuOptions { max_atlas_size: 2048, ..GpuOptions::default() };
    let Some(mut r) = renderer_opts(256, 256, opts) else { return };
    let font = Arial::load();
    let chars: Vec<char> = ('A'..='Z').chain('a'..='z').collect();
    let mut glyphs = Vec::new();
    for &c in &chars {
        glyphs.push(Glyph { face: 0, id: font.font.glyph_id(c).0, adv: 0.0 });
    }
    let mut keys = 0;
    let mut max_bytes = 0;
    let mut evicted = false;
    let mut prev_glyphs = 0;
    for k in 0..97 {
        let size = 12.0 + k as f32;
        let mut l = DrawList::new();
        for i in 0..glyphs.len() as u32 {
            l.glyphs(20.0, 150.0, 0.0, size, Rgba::BLACK, (i, i + 1));
        }
        r.render(&[layer(&l, &glyphs, 256.0, 256.0)], &font, WHITE).expect("render");
        keys += glyphs.len();
        let s = r.stats();
        max_bytes = max_bytes.max(s.atlas_bytes);
        if s.atlas_glyphs < prev_glyphs {
            evicted = true;
        }
        prev_glyphs = s.atlas_glyphs;
    }
    assert!(keys >= 5000);
    assert_eq!(max_bytes, 2048 * 2048, "atlas grew to its maximum");
    assert!(evicted, "atlas was evicted at least once");
    assert!(r.stats().atlas_bytes <= 2048 * 2048);

    // Text still renders afterwards.
    let mut run = Vec::new();
    let w = font.run("Hello", 24.0, &mut run);
    let mut l = DrawList::new();
    l.glyphs(10.0, 50.0, w, 24.0, Rgba::BLACK, (0, run.len() as u32));
    let b = draw(&mut r, &[layer(&l, &run, 256.0, 256.0)], &font);
    let (x0, y0, x1, y1) = ink_bounds(&b, 256, 256).expect("ink after eviction");
    assert!(x0 >= 10 && x1 <= 10 + w as u32 + 1 && y0 >= 26 && y1 <= 56);
}

// ---------------------------------------------------------------- A4: batching and culling

#[test]
fn batching_and_culling() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(800, 600) else { return };
    let font = Arial::load();
    let mut glyphs = Vec::new();
    for _ in 0..50 {
        font.run("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMN", 12.0, &mut glyphs);
    }
    assert_eq!(glyphs.len(), 2000);
    let mut l = DrawList::new();
    for i in 0..2000 {
        let (x, y) = ((i % 50) as f32 * 16.0, (i / 50) as f32 * 14.0);
        l.rect(Rect::new(x, y, 10.0, 10.0), Rgba::rgb((i % 255) as u8, 100, 200));
    }
    for line in 0..50u32 {
        let start = line * 40;
        let w: f32 = glyphs[start as usize..(start + 40) as usize].iter().map(|g| g.adv).sum();
        l.glyphs(0.0, 20.0 + line as f32 * 11.0, w, 12.0, Rgba::BLACK, (start, start + 40));
    }
    let src = &font;
    r.render(&[layer(&l, &glyphs, 800.0, 600.0)], src, WHITE).unwrap();
    let s = r.stats();
    let ink_glyphs = glyphs
        .iter()
        .filter(|g| font.font.outline_glyph(ab_glyph::GlyphId(g.id).with_scale(12.0)).is_some())
        .count() as u32;
    assert_eq!(s.last_frame_instances, 2000 + ink_glyphs);
    assert!(s.last_frame_draw_calls <= 2, "draw calls: {}", s.last_frame_draw_calls);

    // Items outside the viewport produce no instances.
    for i in 0..1000 {
        l.rect(Rect::new(5000.0 + i as f32, 10.0, 10.0, 10.0), RED);
    }
    l.glyphs(0.0, 5000.0, 400.0, 12.0, Rgba::BLACK, (0, 40));
    l.glyphs(-3000.0, 100.0, 400.0, 12.0, Rgba::BLACK, (0, 40));
    l.push_clip(Rect::new(0.0, 0.0, 0.0, 0.0), NO_RADII);
    l.rect(Rect::new(0.0, 0.0, 100.0, 100.0), RED);
    l.pop_clip();
    r.render(&[layer(&l, &glyphs, 800.0, 600.0)], src, WHITE).unwrap();
    let s2 = r.stats();
    assert_eq!(s2.last_frame_instances, s.last_frame_instances);
    assert!(s2.last_frame_draw_calls <= 2);
}

// ---------------------------------------------------------------- A5: resize, trim, drop_image

#[test]
fn resize_trim_drop_image() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(100, 100) else { return };
    let font = Arial::load();
    let img = r.create_image(4, 4, &quadrant_image());
    let img2 = r.create_image(8, 8, &[255u8; 256]);
    let mut run = Vec::new();
    let w = font.run("Hi", 20.0, &mut run);
    let mut l = DrawList::new();
    l.rect(Rect::new(0.0, 0.0, 10.0, 10.0), RED);
    l.image(Rect::new(10.0, 0.0, 10.0, 10.0), img, NO_RADII, 1.0);
    l.glyphs(0.0, 40.0, w, 20.0, Rgba::BLACK, (0, run.len() as u32));
    let b = draw(&mut r, &[layer(&l, &run, 100.0, 100.0)], &font);
    assert_px(&b, 100, 5, 5, [255, 0, 0], 0);

    // drop_image
    let before = r.stats();
    assert_eq!(before.images, 2);
    r.drop_image(img);
    let after = r.stats();
    assert_eq!(after.images, 1);
    assert!(after.image_bytes < before.image_bytes);
    r.drop_image(img); // twice: harmless
    let b = draw(&mut r, &[layer(&l, &run, 100.0, 100.0)], &font);
    assert_px(&b, 100, 15, 5, [255, 255, 255], 0); // dropped image draws nothing
    assert!(has_ink_in_rows(&b, 100, 20..45));

    // trim
    let before = r.stats();
    r.trim();
    let after = r.stats();
    assert!(after.atlas_bytes < before.atlas_bytes);
    assert!(after.instance_buffer_bytes < before.instance_buffer_bytes);
    assert_eq!(after.atlas_glyphs, 0);
    let b = draw(&mut r, &[layer(&l, &run, 100.0, 100.0)], &font);
    assert_px(&b, 100, 5, 5, [255, 0, 0], 0);
    assert!(has_ink_in_rows(&b, 100, 20..45), "text after trim");

    // resize
    r.resize(50, 30);
    assert_eq!(r.size(), (50, 30));
    let mut l2 = DrawList::new();
    l2.rect(Rect::new(0.0, 0.0, 10.0, 10.0), RED);
    l2.image(Rect::new(20.0, 0.0, 10.0, 10.0), img2, NO_RADII, 1.0);
    let b = draw(&mut r, &[layer(&l2, &[], 50.0, 30.0)], &font);
    assert_eq!(b.len(), 50 * 30 * 4);
    assert_px(&b, 50, 5, 5, [255, 0, 0], 0);
    assert_px(&b, 50, 45, 25, [255, 255, 255], 0);
    r.resize(0, 0);
    r.render(&[layer(&l2, &[], 50.0, 30.0)], &font, WHITE).expect("zero-size render is skipped");
    r.resize(64, 64);
    let b = draw(&mut r, &[layer(&l2, &[], 64.0, 64.0)], &font);
    assert_px(&b, 64, 5, 5, [255, 0, 0], 0);
}

// ---------------------------------------------------------------- A6: PNG

#[test]
fn save_png_round_trip() {
    let _g = gpu_lock();
    let Some(mut r) = renderer(64, 48) else { return };
    let mut l = DrawList::new();
    l.rounded_rect(Rect::new(5.0, 5.0, 40.0, 30.0), Rgba::rgb(30, 140, 220), [8.0; 4]);
    l.rect(Rect::new(20.0, 20.0, 30.0, 20.0), Rgba::new(200, 40, 40, 160));
    r.render(&[layer(&l, &[], 64.0, 48.0)], &NoGlyphs, WHITE).unwrap();
    let pixels = r.read_rgba().unwrap();
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("invisium_gfx_frame.png");
    r.save_png(&path).unwrap();
    let decoded = image::open(&path).expect("decode png").to_rgba8();
    assert_eq!(decoded.dimensions(), (64, 48));
    assert_eq!(decoded.into_raw(), pixels);
    let _ = std::fs::remove_file(&path);
}
