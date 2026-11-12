//! Milestone 3, part B (C3, C4): `PushTransform` / `PopTransform` in the renderer. Headless only.

use ab_glyph::{Font, FontVec, ScaleFont};
use invisium_gfx::draw::{affine_mul, Affine, DrawList, Glyph, GlyphBitmap, GlyphSource, Item, Layer, Rect, Rgba, Stop, NO_RADII};
use invisium_gfx::{GpuOptions, Renderer};
use std::sync::Mutex;

static GPU: Mutex<()> = Mutex::new(());
const RED: Rgba = Rgba::rgb(255, 0, 0);
const BLUE: Rgba = Rgba::rgb(0, 0, 255);
const GREEN: Rgba = Rgba::rgb(0, 255, 0);

fn gpu() -> (std::sync::MutexGuard<'static, ()>, Option<Renderer>) {
    let g = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let r = match pollster::block_on(Renderer::new_headless(200, 200, GpuOptions::default())) {
        Ok(r) => Some(r),
        Err(e) if e.contains("no GPU adapter") => {
            eprintln!("SKIP: {e}");
            None
        }
        Err(e) => panic!("renderer creation failed: {e}"),
    };
    (g, r)
}

struct NoGlyphs;
impl GlyphSource for NoGlyphs {
    fn rasterize(&self, _: u16, _: u16, _: f32, _: u8) -> Option<GlyphBitmap> {
        None
    }
}

struct Arial(FontVec);
impl Arial {
    fn load() -> Arial {
        Arial(FontVec::try_from_vec(std::fs::read(r"C:\Windows\Fonts\arial.ttf").expect("arial.ttf")).expect("parse"))
    }
    fn run(&self, text: &str, size: f32, out: &mut Vec<Glyph>) -> f32 {
        let scaled = self.0.as_scaled(size);
        text.chars()
            .map(|ch| {
                let id = self.0.glyph_id(ch);
                out.push(Glyph { face: 0, id: id.0, adv: scaled.h_advance(id) });
                scaled.h_advance(id)
            })
            .sum()
    }
}
impl GlyphSource for Arial {
    fn rasterize(&self, _: u16, glyph: u16, px: f32, subpx: u8) -> Option<GlyphBitmap> {
        let g = ab_glyph::GlyphId(glyph).with_scale_and_position(px, ab_glyph::point(subpx as f32 / 4.0, 0.0));
        let o = self.0.outline_glyph(g)?;
        let b = o.px_bounds();
        let (w, h) = (b.width() as u32, b.height() as u32);
        if w == 0 || h == 0 {
            return None;
        }
        let mut data = vec![0u8; (w * h) as usize];
        o.draw(|x, y, c| {
            if x < w && y < h {
                data[(y * w + x) as usize] = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        });
        Some(GlyphBitmap { width: w, height: h, left: b.min.x as i32, top: b.min.y as i32, data })
    }
}

fn layer<'a>(list: &'a DrawList, glyphs: &'a [Glyph]) -> Layer<'a> {
    Layer { list, glyphs, viewport: Rect::new(0.0, 0.0, 200.0, 200.0), scroll: (0.0, 0.0), scale: 1.0 }
}

fn draw(r: &mut Renderer, l: &DrawList) -> Vec<u8> {
    r.render(&[layer(l, &[])], &NoGlyphs, Rgba::WHITE).expect("render");
    r.read_rgba().expect("read")
}

fn px(b: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * 200 + x) * 4) as usize;
    [b[i], b[i + 1], b[i + 2], b[i + 3]]
}

#[track_caller]
fn is(b: &[u8], x: u32, y: u32, want: Rgba, tol: i32) {
    let p = px(b, x, y);
    let ok = (p[0] as i32 - want.r as i32).abs() <= tol
        && (p[1] as i32 - want.g as i32).abs() <= tol
        && (p[2] as i32 - want.b as i32).abs() <= tol;
    assert!(ok, "pixel ({x},{y}) = {p:?}, want {want:?}");
}

fn translate(x: f32, y: f32) -> Affine {
    [1.0, 0.0, 0.0, 1.0, x, y]
}

/// `m` applied about the point (`cx`, `cy`).
fn about(m: Affine, cx: f32, cy: f32) -> Affine {
    affine_mul(&translate(cx, cy), &affine_mul(&m, &translate(-cx, -cy)))
}

fn rotate(deg: f32) -> Affine {
    let (s, c) = deg.to_radians().sin_cos();
    [c, s, -s, c, 0.0, 0.0]
}

fn wrapped(m: Affine, items: &[Item]) -> DrawList {
    let mut l = DrawList::new();
    l.push_transform(m);
    l.items.extend_from_slice(items);
    l.pop_transform();
    l
}

fn rect(x: f32, y: f32, w: f32, h: f32, color: Rgba, r: f32) -> Item {
    Item::Rect { rect: Rect::new(x, y, w, h), color, radii: [r; 4] }
}

#[test]
fn translate_and_scale_about_a_point() {
    let (_g, Some(mut r)) = gpu() else { return };
    let b = draw(&mut r, &wrapped(translate(50.0, 30.0), &[rect(10.0, 10.0, 20.0, 20.0, RED, 0.0)]));
    is(&b, 65, 45, RED, 0);
    is(&b, 60, 40, RED, 0);
    is(&b, 15, 15, Rgba::WHITE, 0);
    is(&b, 80, 45, Rgba::WHITE, 0);

    // scale(2) about (50,50): (50,50,30,30) r=10 lands on (50..110) with radius 20.
    let b = draw(&mut r, &wrapped(about([2.0, 0.0, 0.0, 2.0, 0.0, 0.0], 50.0, 50.0), &[rect(50.0, 50.0, 30.0, 30.0, RED, 10.0)]));
    is(&b, 109, 80, RED, 2);
    is(&b, 110, 80, Rgba::WHITE, 2);
    is(&b, 80, 109, RED, 2);
    is(&b, 80, 110, Rgba::WHITE, 2);
    is(&b, 107, 107, Rgba::WHITE, 2); // just outside the doubled corner
    let partial = (95..111).flat_map(|y| (95..111).map(move |x| (x, y))).filter(|&(x, y)| (40..215).contains(&px(&b, x, y)[1])).count();
    assert!(partial >= 6, "corner is anti-aliased ({partial} partial pixels)");
}

#[test]
fn rotations() {
    let (_g, Some(mut r)) = gpu() else { return };
    // rotate(90deg) about (100,100): (100..160, 60..80) -> (120..140, 100..160).
    let b = draw(&mut r, &wrapped(about(rotate(90.0), 100.0, 100.0), &[rect(100.0, 60.0, 60.0, 20.0, RED, 0.0)]));
    is(&b, 130, 70, Rgba::WHITE, 2);
    is(&b, 130, 130, RED, 2);
    is(&b, 121, 155, RED, 2);
    is(&b, 110, 130, Rgba::WHITE, 2);
    is(&b, 130, 165, Rgba::WHITE, 2);

    // rotate(45deg) about the centre of (70,70,60,60): a diamond |dx|+|dy| <= 42.4.
    let b = draw(&mut r, &wrapped(about(rotate(45.0), 100.0, 100.0), &[rect(70.0, 70.0, 60.0, 60.0, RED, 0.0)]));
    is(&b, 100, 100, RED, 0);
    is(&b, 139, 100, RED, 2);
    is(&b, 128, 72, Rgba::WHITE, 2);
    let edge = (125..140).map(|x| px(&b, x, 110)[1]).filter(|g| (25..230).contains(g)).count();
    assert!(edge >= 1, "an edge pixel is partially covered");
}

#[test]
fn nesting_order_matters() {
    let (_g, Some(mut r)) = gpu() else { return };
    let scale2 = [2.0, 0.0, 0.0, 2.0, 0.0, 0.0];
    let mut a = DrawList::new(); // translate(50,0) outside, scale(2) inside: p -> 2p + 50
    a.push_transform(translate(50.0, 0.0));
    a.push_transform(scale2);
    a.items.push(rect(10.0, 10.0, 10.0, 10.0, RED, 0.0));
    a.pop_transform();
    a.pop_transform();
    let b = draw(&mut r, &a);
    is(&b, 80, 30, RED, 0);
    is(&b, 130, 30, Rgba::WHITE, 0);
    let mut c = DrawList::new(); // scale(2) outside, translate inside: p -> 2(p + 50)
    c.push_transform(scale2);
    c.push_transform(translate(50.0, 0.0));
    c.items.push(rect(10.0, 10.0, 10.0, 10.0, RED, 0.0));
    c.pop_transform();
    c.pop_transform();
    let b = draw(&mut r, &c);
    is(&b, 130, 30, RED, 0);
    is(&b, 80, 30, Rgba::WHITE, 0);
}

#[test]
fn clips_inside_and_outside() {
    let (_g, Some(mut r)) = gpu() else { return };
    let clip = Item::PushClip { rect: Rect::new(0.0, 0.0, 50.0, 50.0), radii: NO_RADII };
    // Clip inside the transform moves with it.
    let b = draw(&mut r, &wrapped(translate(100.0, 0.0), &[clip.clone(), rect(0.0, 0.0, 100.0, 100.0, RED, 0.0), Item::PopClip]));
    is(&b, 125, 25, RED, 0);
    is(&b, 160, 25, Rgba::WHITE, 0);
    is(&b, 25, 25, Rgba::WHITE, 0);
    // Clip outside the transform stays put.
    let mut l = DrawList::new();
    l.items.push(clip);
    l.push_transform(translate(30.0, 0.0));
    l.items.push(rect(0.0, 0.0, 100.0, 100.0, RED, 0.0));
    l.pop_transform();
    l.pop_clip();
    let b = draw(&mut r, &l);
    is(&b, 40, 25, RED, 0);
    is(&b, 60, 25, Rgba::WHITE, 0);
    is(&b, 10, 25, Rgba::WHITE, 0);
    // A clip in the item's rotated space is exact.
    let b = draw(
        &mut r,
        &wrapped(
            about(rotate(45.0), 100.0, 100.0),
            &[Item::PushClip { rect: Rect::new(70.0, 70.0, 60.0, 60.0), radii: NO_RADII }, rect(0.0, 0.0, 200.0, 200.0, RED, 0.0), Item::PopClip],
        ),
    );
    is(&b, 100, 100, RED, 0);
    is(&b, 139, 100, RED, 2);
    is(&b, 128, 72, Rgba::WHITE, 2);
    // Rounded clip under scale(2): its corner scales too.
    let b = draw(
        &mut r,
        &wrapped(
            [2.0, 0.0, 0.0, 2.0, 0.0, 0.0],
            &[Item::PushClip { rect: Rect::new(10.0, 10.0, 40.0, 40.0), radii: [10.0; 4] }, rect(0.0, 0.0, 100.0, 100.0, RED, 0.0), Item::PopClip],
        ),
    );
    is(&b, 60, 60, RED, 0);
    is(&b, 22, 22, Rgba::WHITE, 2);
    is(&b, 21, 60, RED, 2);
}

#[test]
fn flipped_gradient_and_scaled_image() {
    let (_g, Some(mut r)) = gpu() else { return };
    let mut l = DrawList::new();
    let stops = l.add_stops(&[Stop { offset: 0.0, color: RED }, Stop { offset: 1.0, color: BLUE }]);
    let grad = Item::LinearGradient { rect: Rect::new(50.0, 50.0, 100.0, 100.0), radii: NO_RADII, angle: 90.0, stops, repeating: false };
    l.items.push(grad.clone());
    let b = draw(&mut r, &l);
    is(&b, 51, 100, RED, 12);
    is(&b, 148, 100, BLUE, 12);
    l.items.clear();
    l.push_transform(about([-1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 100.0, 100.0));
    l.items.push(grad);
    l.pop_transform();
    let b = draw(&mut r, &l);
    is(&b, 51, 100, BLUE, 12);
    is(&b, 148, 100, RED, 12);

    // 40x40 image, left half green, right half blue, drawn 80x80 under scale(0.5).
    let mut rgba = Vec::with_capacity(40 * 40 * 4);
    for _y in 0..40 {
        for x in 0..40 {
            rgba.extend_from_slice(if x < 20 { &[0, 255, 0, 255] } else { &[0, 0, 255, 255] });
        }
    }
    let img = r.create_image(40, 40, &rgba);
    let item = Item::Image { rect: Rect::new(0.0, 0.0, 80.0, 80.0), image: img, radii: NO_RADII, opacity: 1.0, uv: [0.0, 0.0, 1.0, 1.0], repeat: false };
    let b = draw(&mut r, &wrapped([0.5, 0.0, 0.0, 0.5, 0.0, 0.0], &[item]));
    is(&b, 10, 20, GREEN, 4);
    is(&b, 30, 20, BLUE, 4);
    is(&b, 60, 20, Rgba::WHITE, 0);
    is(&b, 20, 60, Rgba::WHITE, 0);
}

fn ink_bounds(b: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let mut out: Option<(u32, u32, u32, u32)> = None;
    for y in 0..200 {
        for x in 0..200 {
            if px(b, x, y)[0] < 160 {
                let o = out.get_or_insert((x, y, x, y));
                *o = (o.0.min(x), o.1.min(y), o.2.max(x), o.3.max(y));
            }
        }
    }
    out
}

#[test]
fn glyphs_under_scale_2() {
    let (_g, Some(mut r)) = gpu() else { return };
    let font = Arial::load();
    let mut glyphs = Vec::new();
    let w = font.run("HHHH", 20.0, &mut glyphs);
    let run = Item::Glyphs { x: 10.0, baseline: 40.0, w, size: 20.0, color: Rgba::BLACK, glyphs: (0, 4) };
    let mut l = DrawList::new();
    l.items.push(run.clone());
    r.render(&[layer(&l, &glyphs)], &font, Rgba::WHITE).unwrap();
    let (x0, y0, x1, y1) = ink_bounds(&r.read_rgba().unwrap()).expect("ink");
    let l = wrapped([2.0, 0.0, 0.0, 2.0, 0.0, 0.0], &[run]);
    r.render(&[layer(&l, &glyphs)], &font, Rgba::WHITE).unwrap();
    let (a0, b0, a1, b1) = ink_bounds(&r.read_rgba().unwrap()).expect("scaled ink");
    for (got, want) in [(a0, 2 * x0), (b0, 2 * y0), (a1, 2 * x1 + 1), (b1, 2 * y1 + 1)] {
        assert!((got as i32 - want as i32).abs() <= 3, "scaled bounds {:?} vs 2x {:?}", (a0, b0, a1, b1), (x0, y0, x1, y1));
    }
    assert!(b1 <= 81 && b0 >= 45, "ink in the doubled band");
}

#[test]
fn fixed_singular_and_unbalanced() {
    let (_g, Some(mut r)) = gpu() else { return };
    fn scrolled(l: &DrawList) -> Layer<'_> {
        Layer { scroll: (0.0, 50.0), ..layer(l, &[]) }
    }
    // Transform inside PushFixed ignores scroll; so does PushFixed inside a transform.
    let mut l = DrawList::new();
    l.items.push(Item::PushFixed);
    l.push_transform(translate(10.0, 10.0));
    l.items.push(rect(0.0, 0.0, 20.0, 20.0, RED, 0.0));
    l.pop_transform();
    l.items.push(Item::PopFixed);
    l.push_transform(about(rotate(90.0), 150.0, 150.0));
    l.items.push(Item::PushFixed);
    l.items.push(rect(140.0, 120.0, 20.0, 10.0, BLUE, 0.0)); // -> x 170..180, y 140..160
    l.items.push(Item::PopFixed);
    l.pop_transform();
    r.render(&[scrolled(&l)], &NoGlyphs, Rgba::WHITE).unwrap();
    let b = r.read_rgba().unwrap();
    is(&b, 20, 20, RED, 0);
    is(&b, 175, 150, BLUE, 2);
    is(&b, 150, 125, Rgba::WHITE, 0);

    // scale(0) and non-finite matrices draw nothing until their pop; what follows draws.
    let mut l = DrawList::new();
    for m in [[0.0; 6], [f32::NAN, 0.0, 0.0, 1.0, 0.0, 0.0], [1e-6, 0.0, 0.0, 1e-6, 0.0, 0.0]] {
        l.push_transform(m);
        l.items.push(rect(0.0, 0.0, 100.0, 100.0, RED, 0.0));
        l.push_clip(Rect::new(0.0, 0.0, 10.0, 10.0), NO_RADII);
        l.push_transform(translate(5.0, 5.0));
        l.items.push(rect(0.0, 0.0, 100.0, 100.0, RED, 0.0));
        l.pop_transform();
        l.pop_clip();
        l.pop_transform();
    }
    l.items.push(rect(100.0, 100.0, 50.0, 50.0, BLUE, 0.0));
    let b = draw(&mut r, &l);
    is(&b, 50, 50, Rgba::WHITE, 0);
    is(&b, 120, 120, BLUE, 0);

    // Unbalanced: stray pops are ignored; unclosed pushes end with the layer.
    let mut a = DrawList::new();
    a.pop_transform();
    a.items.push(rect(0.0, 0.0, 20.0, 20.0, GREEN, 0.0));
    for _ in 0..100 {
        a.push_transform(translate(1.0, 1.0));
    }
    let mut c = DrawList::new();
    c.pop_transform();
    c.items.push(rect(180.0, 0.0, 20.0, 20.0, RED, 0.0));
    r.render(&[layer(&a, &[]), layer(&c, &[])], &NoGlyphs, Rgba::WHITE).unwrap();
    let b = r.read_rgba().unwrap();
    is(&b, 10, 10, GREEN, 0);
    is(&b, 190, 10, RED, 0);
}

#[test]
fn culling_and_batching() {
    let (_g, Some(mut r)) = gpu() else { return };
    // Moved into the viewport: drawn. Moved out: no instance.
    let mut l = DrawList::new();
    l.push_transform(translate(-4900.0, 0.0));
    l.items.push(rect(5000.0, 10.0, 20.0, 20.0, RED, 0.0));
    l.pop_transform();
    l.push_transform(about(rotate(30.0), 0.0, 0.0));
    l.items.push(rect(10.0, 10.0, 20.0, 20.0, RED, 0.0));
    l.pop_transform();
    l.push_transform([1.0, 0.0, 0.5, 1.0, 5000.0, 0.0]);
    l.items.push(rect(10.0, 10.0, 20.0, 20.0, RED, 0.0));
    l.pop_transform();
    let b = draw(&mut r, &l);
    is(&b, 110, 20, RED, 0);
    assert_eq!(r.stats().last_frame_instances, 2);

    // The A4 frame (2000 rects + 2000 glyphs) and the same with every rect transformed.
    let font = Arial::load();
    let mut glyphs = Vec::new();
    for _ in 0..50 {
        font.run("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMN", 12.0, &mut glyphs);
    }
    let build = |wrap: bool| {
        let mut l = DrawList::new();
        for i in 0..2000 {
            let (x, y) = ((i % 50) as f32 * 4.0, (i / 50) as f32 * 5.0);
            if wrap {
                l.push_transform(about(rotate(10.0 + i as f32), x + 2.0, y + 2.0));
            }
            l.rect(Rect::new(x, y, 3.0, 3.0), Rgba::rgb((i % 255) as u8, 100, 200));
            if wrap {
                l.pop_transform();
            }
        }
        for line in 0..50u32 {
            let s = line * 40;
            let w: f32 = glyphs[s as usize..(s + 40) as usize].iter().map(|g| g.adv).sum();
            l.glyphs(0.0, 12.0 + line as f32 * 3.5, w, 12.0, Rgba::BLACK, (s, s + 40));
        }
        l
    };
    for wrap in [false, true] {
        let l = build(wrap);
        r.render(&[layer(&l, &glyphs)], &font, Rgba::WHITE).unwrap();
        let s = r.stats();
        assert!(s.last_frame_instances >= 2000, "instances {}", s.last_frame_instances);
        assert!(s.last_frame_draw_calls <= 2, "wrap={wrap}: {} draw calls", s.last_frame_draw_calls);
    }
}
