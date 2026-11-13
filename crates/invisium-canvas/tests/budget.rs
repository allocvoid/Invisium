mod common;
use common::*;
use invisium_canvas::*;
use std::time::Instant;

const BUDGET: usize = 512 << 20;

fn handle(r: Ret) -> u32 {
    match r {
        Ret::Object(_, h) => h,
        o => panic!("{o:?}"),
    }
}

#[test]
fn memory_bytes_counts_each_kind_and_release_gives_back() {
    let small = T::new(10, 10).c.memory_bytes();
    assert_eq!(T::new(100, 100).c.memory_bytes() - small, 100 * 100 * 4 - 10 * 10 * 4, "bitmap");

    let mut t = T::new(10, 10);
    let m = |t: &T| t.c.memory_bytes();
    // Give each handle map its first slots (counted with the maps, kept after release).
    let px = vec![9u8; 4];
    for r in [
        t.obj(Kind::Path, "Path2D", &[]),
        t.call("createLinearGradient", &[n(0.0), n(0.0), n(1.0), n(0.0)]),
        t.call("createPattern", &[Arg::Pixels { width: 1, height: 1, rgba: &px }, s("repeat")]),
    ] {
        let Ret::Object(k, h) = r else { panic!() };
        t.c.release(k, h);
    }
    let base = m(&t);

    // Current path: counted by allocated capacity.
    t.call("moveTo", &[n(0.0), n(0.0)]);
    let one = m(&t) - base;
    assert!(one > 0);
    for i in 0..100 {
        t.call("lineTo", &[n(i as f64), n(1.0)]);
    }
    assert!(m(&t) - base >= 100 * one / 8, "current path grows");
    t.call("beginPath", &[]);
    assert_eq!(m(&t), base);

    // Path2D objects.
    let p = handle(t.obj(Kind::Path, "Path2D", &[s("M0 0 L1 1 L2 2")]));
    let with_path = m(&t);
    assert!(with_path > base);
    for i in 0..100 {
        t.obj(Kind::Path, "lineTo", &[Arg::Object(Kind::Path, p), n(i as f64), n(3.0)]);
    }
    assert!(m(&t) > with_path, "Path2D segments");
    t.c.release(Kind::Path, p);
    assert_eq!(m(&t), base, "Path2D released");

    // Gradients and their stops (stop lists grow exactly).
    let g = handle(t.call("createLinearGradient", &[n(0.0), n(0.0), n(1.0), n(0.0)]));
    let with_grad = m(&t);
    assert!(with_grad - base >= 64, "gradient object");
    t.obj(Kind::Gradient, "addColorStop", &[Arg::Object(Kind::Gradient, g), n(0.0), col(0, 0, 0, 255)]);
    let stop = m(&t) - with_grad;
    assert!(stop > 0);
    t.obj(Kind::Gradient, "addColorStop", &[Arg::Object(Kind::Gradient, g), n(1.0), col(0, 0, 0, 255)]);
    assert_eq!(m(&t) - with_grad, 2 * stop, "stops");
    t.c.release(Kind::Gradient, g);
    assert_eq!(m(&t), base, "gradient released");

    // Patterns.
    let px = vec![9u8; 20 * 20 * 4];
    let p = handle(t.call("createPattern", &[Arg::Pixels { width: 20, height: 20, rgba: &px }, s("repeat")]));
    assert!(m(&t) - base >= 1600, "pattern pixels");
    t.c.release(Kind::Pattern, p);
    assert_eq!(m(&t), base, "pattern released");

    // Dash arrays.
    let d = [1.0, 2.0, 3.0, 4.0];
    t.call("setLineDash", &[Arg::Nums(&d)]);
    assert_eq!(m(&t) - base, 32, "dash array");
    t.call("setLineDash", &[Arg::Nums(&[])]);
    assert_eq!(m(&t), base);

    // Save stack and clip masks (each distinct mask once).
    let mut t = T::new(50, 50);
    let base = m(&t);
    t.call("save", &[]);
    let saved = m(&t) - base;
    assert!(saved > 0, "saved state");
    t.call("rect", &[n(0.0), n(0.0), n(40.0), n(40.0)]);
    t.call("clip", &[]);
    t.call("beginPath", &[]);
    assert_eq!(m(&t) - base - saved, 41 * 40, "clip mask over the path's box");
    let before = m(&t);
    t.call("save", &[]);
    assert!(m(&t) - before < 200, "a shared mask counts once");
    t.call("restore", &[]);
    t.call("restore", &[]);
    assert!(m(&t) > base && m(&t) < base + saved, "restored (the stack keeps its capacity)");
}

#[test]
fn released_styles_stay_counted_until_dropped() {
    let mut t = T::new(16, 16);
    let base = t.c.memory_bytes();
    let px = vec![5u8; 1 << 20];
    let mut rounds = 0;
    let mut last = base;
    loop {
        let r = t.call("createPattern", &[Arg::Pixels { width: 512, height: 512, rgba: &px }, s("repeat")]);
        let Ret::Object(Kind::Pattern, h) = r else {
            assert!(matches!(r, Ret::Throw("NotSupportedError", _)), "{r:?}");
            break;
        };
        t.set("fillStyle", Arg::Object(Kind::Pattern, h));
        t.c.release(Kind::Pattern, h);
        t.call("save", &[]);
        let now = t.c.memory_bytes();
        assert!(now >= last + (1 << 20), "round {rounds}: {last} -> {now}");
        last = now;
        rounds += 1;
        assert!(rounds < 600, "never refused");
    }
    assert!(rounds > 400, "{rounds}");
    assert!(t.c.memory_bytes() <= BUDGET);
    for _ in 0..rounds {
        t.call("restore", &[]);
    }
    t.set("fillStyle", col(1, 2, 3, 255));
    assert!(t.c.memory_bytes() < base + (1 << 20), "{}", t.c.memory_bytes());
}

#[test]
fn huge_shadows_drop_the_blur() {
    let mut t = T::new(6000, 6000);
    t.set("shadowColor", col(255, 0, 0, 255));
    t.set("shadowOffsetX", n(500.0));
    t.set("shadowOffsetY", n(500.0));
    t.set("shadowBlur", n(1200.0));
    t.call("fillRect", &[n(0.0), n(0.0), n(5000.0), n(5000.0)]);
    assert_eq!(t.px(5200, 5200), [255, 0, 0, 255], "drawn sharp");
    assert_eq!(t.alpha(5600, 5600), 0);
    assert_eq!(t.px(100, 100), [0, 0, 0, 255]);
}

#[test]
fn one_budget_refuses_every_kind() {
    let mut t = T::new(64, 64);
    let clip = handle(t.obj(Kind::Path, "Path2D", &[s("M0 0 h32 v32 h-32 z")]));
    // Fill the budget with patterns of decreasing size, then path segments.
    let mut pats = Vec::new();
    for side in [2048u32, 512, 128, 32] {
        let px = vec![1u8; (side * side * 4) as usize];
        loop {
            match t.call("createPattern", &[Arg::Pixels { width: side, height: side, rgba: &px }, s("repeat")]) {
                Ret::Object(_, h) => pats.push(h),
                Ret::Throw("NotSupportedError", _) => break,
                o => panic!("{o:?}"),
            }
        }
    }
    // Then gradients with stops (24 bytes each), then a dash array for the last bytes.
    while let Ret::Object(_, g) = t.call("createLinearGradient", &[n(0.0), n(0.0), n(1.0), n(0.0)]) {
        for i in 0..1024 {
            let before = t.c.memory_bytes();
            let a = [Arg::Object(Kind::Gradient, g), n(i as f64 / 1024.0), col(0, 0, 0, 255)];
            t.obj(Kind::Gradient, "addColorStop", &a);
            if t.c.memory_bytes() == before {
                break;
            }
        }
    }
    let dash = [1.0; 64];
    for k in (2..=64).rev().step_by(2) {
        t.call("setLineDash", &[Arg::Nums(&dash[..k])]);
        if t.call("getLineDash", &[]) != Ret::Nums(vec![]) {
            break;
        }
    }
    let kept = t.call("getLineDash", &[]);
    let full = t.c.memory_bytes();
    assert!(full <= BUDGET && full > BUDGET - 4096, "{full}");

    // Every kind is now refused and nothing grows.
    let one = [0u8; 4];
    assert!(matches!(
        t.call("createPattern", &[Arg::Pixels { width: 1, height: 1, rgba: &one }, s("repeat")]),
        Ret::Throw(..)
    ));
    // A Path2D may still take a free map slot, but its segments are refused.
    let p = t.obj(Kind::Path, "Path2D", &[s("M0 0 L1 1 L2 2")]);
    if let Ret::Object(_, h) = p {
        assert_eq!(t.call("isPointInStroke", &[Arg::Object(Kind::Path, h), n(1.0), n(1.0)]), Ret::Bool(false));
    }
    assert!(matches!(t.call("createLinearGradient", &[n(0.0), n(0.0), n(1.0), n(0.0)]), Ret::Throw(..)));
    let d = [1.0; 66];
    t.call("setLineDash", &[Arg::Nums(&d)]);
    t.call("save", &[]);
    t.call("lineTo", &[n(5.0), n(5.0)]);
    t.call("clip", &[Arg::Object(Kind::Path, clip)]);
    assert_eq!(t.call("getLineDash", &[]), kept, "longer dash refused");
    assert_eq!(t.c.memory_bytes(), full, "nothing grew");
    t.call("fillRect", &[n(0.0), n(0.0), n(64.0), n(64.0)]);
    assert_eq!(t.alpha(50, 50), 255, "refused clip is ignored");

    // Releasing gives room back.
    t.c.release(Kind::Pattern, pats[0]);
    let g = handle(t.call("createLinearGradient", &[n(0.0), n(0.0), n(1.0), n(0.0)]));
    let before = t.c.memory_bytes();
    t.obj(Kind::Gradient, "addColorStop", &[Arg::Object(Kind::Gradient, g), n(0.5), col(1, 2, 3, 255)]);
    assert!(t.c.memory_bytes() > before);
    assert!(t.c.memory_bytes() <= BUDGET);
}

#[test]
fn nested_clips_are_bounded() {
    let mut t = T::new(1024, 1024);
    let start = Instant::now();
    for i in 0..2000 {
        t.call("save", &[]);
        t.call("beginPath", &[]);
        t.call("rect", &[n(i as f64 * 0.1), n(0.0), n(1000.0), n(1000.0)]);
        t.call("clip", &[]);
    }
    let el = start.elapsed();
    assert!(t.c.memory_bytes() <= BUDGET);
    assert!(el.as_secs() < 30, "2000 nested clips took {el:?}");
    // Disjoint clips: an empty intersection costs nothing.
    let mut u = T::new(1024, 1024);
    u.call("rect", &[n(0.0), n(0.0), n(10.0), n(10.0)]);
    u.call("clip", &[]);
    for _ in 0..2000 {
        u.call("save", &[]);
        u.call("beginPath", &[]);
        u.call("rect", &[n(500.0), n(500.0), n(500.0), n(500.0)]);
        u.call("clip", &[]);
    }
    u.call("fillRect", &[n(0.0), n(0.0), n(1024.0), n(1024.0)]);
    assert_eq!(u.ink(), None);
    eprintln!("2000 nested full-size clips: {el:?}");
}

#[test]
fn take_dirty_only_when_pixels_change() {
    let mut t = T::new(20, 20);
    t.c.take_dirty();
    let img = [255u8; 16];
    let pix = Arg::Pixels { width: 2, height: 2, rgba: &img };
    t.call("save", &[]);
    t.call("restore", &[]);
    t.get("fillStyle");
    t.set("lineWidth", n(3.0));
    t.call("measureText", &[s("abc")]);
    t.call("getImageData", &[n(0.0), n(0.0), n(5.0), n(5.0)]);
    t.call("rect", &[n(0.0), n(0.0), n(5.0), n(5.0)]);
    t.call("isPointInPath", &[n(1.0), n(1.0)]);
    let g = handle(t.call("createLinearGradient", &[n(0.0), n(0.0), n(1.0), n(0.0)]));
    t.obj(Kind::Gradient, "addColorStop", &[Arg::Object(Kind::Gradient, g), n(0.5), col(1, 2, 3, 255)]);
    assert!(!t.c.take_dirty(), "no pixels changed");

    let ops: Vec<(&str, Vec<Arg>)> = vec![
        ("fillRect", vec![n(0.0), n(0.0), n(5.0), n(5.0)]),
        ("clearRect", vec![n(0.0), n(0.0), n(5.0), n(5.0)]),
        ("putImageData", vec![pix, n(1.0), n(1.0)]),
        ("drawImage", vec![pix, n(3.0), n(3.0)]),
        ("fillText", vec![s("abc"), n(2.0), n(15.0)]),
    ];
    for (name, args) in ops {
        t.call(name, &args);
        assert!(t.c.take_dirty(), "{name} sets dirty");
        assert!(!t.c.take_dirty());
    }
    t.c.resize(20, 20);
    assert!(t.c.take_dirty(), "resize sets dirty");
}

#[test]
fn strokes_are_bounded() {
    const MAX_STROKE_PTS: usize = 750_000;
    let zigzag = |t: &mut T| {
        t.call("moveTo", &[n(0.0), n(0.0)]);
        for i in 0..200_000 {
            t.call("lineTo", &[n((i % 16) as f64), n(if i % 2 == 0 { 15.0 } else { 1.0 })]);
        }
    };
    // One piece that covers everything that can show ends the outline.
    let mut t = T::new(16, 16);
    t.set("lineWidth", n(1e6));
    t.call("moveTo", &[n(-100.0), n(8.0)]);
    for _ in 0..1000 {
        t.call("lineTo", &[n(100.0), n(8.0)]);
        t.call("lineTo", &[n(-100.0), n(8.0)]);
    }
    t.call("stroke", &[]);
    assert_eq!(t.c.last_stroke_points(), 4);
    assert_eq!(t.ink(), Some((0, 0, 16, 16)));
    // Huge width, round joins and caps, sharp turns: quick and capped.
    let mut t = T::new(16, 16);
    t.set("lineJoin", s("round"));
    t.set("lineCap", s("round"));
    t.set("lineWidth", n(1e6));
    zigzag(&mut t);
    let start = Instant::now();
    t.call("stroke", &[]);
    assert!(start.elapsed().as_secs() < 10, "{:?}", start.elapsed());
    assert!(t.c.last_stroke_points() <= MAX_STROKE_PTS, "{}", t.c.last_stroke_points());
    assert_eq!(t.alpha(0, 0), 255);
    assert_eq!(t.alpha(15, 15), 255);

    // Thin lines that all show: the outline stops at the cap (16 bytes per point, < 64 MiB).
    let mut t = T::new(16, 16);
    t.set("lineJoin", s("round"));
    t.set("lineWidth", n(1.0));
    zigzag(&mut t);
    let start = Instant::now();
    t.call("stroke", &[]);
    assert!(start.elapsed().as_secs() < 20, "{:?}", start.elapsed());
    let pts = t.c.last_stroke_points();
    assert!(pts > 100_000 && pts <= MAX_STROKE_PTS, "{pts}");
    assert!(pts * 16 < 64 << 20);

    // Dashes: the pieces stop at their own cap.
    let mut t = T::new(16, 16);
    t.set("lineWidth", n(1.0));
    t.call("setLineDash", &[Arg::Nums(&[0.5, 0.5])]);
    zigzag(&mut t);
    let start = Instant::now();
    t.call("stroke", &[]);
    assert!(start.elapsed().as_secs() < 20, "{:?}", start.elapsed());
    let d = t.c.last_dash_points();
    assert!(d > MAX_STROKE_PTS - 10 && d <= MAX_STROKE_PTS, "dash cap reached: {d}");
    assert!(t.c.last_stroke_points() <= MAX_STROKE_PTS);

    // Geometry far from the canvas is skipped; a huge round cap crossing it is still accurate.
    let mut t = T::new(16, 16);
    t.set("lineCap", s("round"));
    t.set("lineWidth", n(2e6));
    t.call("moveTo", &[n(-1e6 - 8.0), n(-5e6)]);
    t.call("lineTo", &[n(-1e6 - 8.0), n(-5e6 - 1.0)]);
    for i in 0..100 {
        t.call("lineTo", &[n(-5e7 - i as f64), n(-5e7)]);
    }
    t.call("stroke", &[]);
    assert!(t.c.last_stroke_points() < 3000, "{}", t.c.last_stroke_points());
    // A round cap of radius 1e6 whose edge crosses the canvas at x = 8.5: still exact.
    let mut v = T::new(16, 16);
    v.set("lineCap", s("round"));
    v.set("lineWidth", n(2e6));
    v.call("moveTo", &[n(-1e6 + 8.5), n(8.0)]);
    v.call("lineTo", &[n(-1e6 + 8.5 - 1e-3), n(8.0)]);
    v.call("stroke", &[]);
    for y in [0, 8, 15] {
        assert_eq!(v.alpha(7, y), 255);
        assert_eq!(v.alpha(9, y), 0);
        assert!(near(v.alpha(8, y), 128, 3), "edge pixel {}", v.alpha(8, y));
    }
    assert!(v.c.last_stroke_points() < 3000);
}

#[test]
fn handle_maps_shrink_after_release() {
    let mut t = T::new(4, 4);
    let base = t.c.memory_bytes();
    let hs: Vec<u32> = (0..100_000).map(|_| handle(t.obj(Kind::Path, "Path2D", &[]))).collect();
    let full = t.c.memory_bytes();
    assert!(full - base > 100_000 * 8, "map slots counted: {}", full - base);
    for h in hs {
        t.c.release(Kind::Path, h);
    }
    assert!(t.c.memory_bytes() - base < 16 << 10, "{}", t.c.memory_bytes() - base);
}

#[test]
fn fills_and_clips_are_bounded() {
    const LIMIT: usize = 128 << 20;
    // 4,000 cubics that would flatten to 1,000 points each, sweeping across both sides of
    // the canvas and through all of its rows.
    let mut t = T::new(64, 64);
    t.call("moveTo", &[n(-1e7), n(-10.0)]);
    for i in 0..4000 {
        let s = if i % 2 == 0 { 1.0 } else { -1.0 };
        t.call("bezierCurveTo", &[n(s * 1e7), n(74.0), n(-s * 1e7), n(-10.0), n(s * 1e7), n(74.0)]);
    }
    let start = Instant::now();
    t.call("fill", &[]);
    let fill = t.c.last_fill_bytes();
    assert!(fill > 1 << 20 && fill <= LIMIT, "fill held {fill}");
    t.call("clip", &[s("evenodd")]);
    let clip = t.c.last_fill_bytes();
    assert!(clip > 1 << 20 && clip <= LIMIT, "clip held {clip}");
    eprintln!("fill held {fill} bytes, clip held {clip} bytes");
    assert!(start.elapsed().as_secs() < 30, "{:?}", start.elapsed());
    // The same through a Path2D under a transform.
    let p = handle(t.obj(Kind::Path, "Path2D", &[]));
    let po = Arg::Object(Kind::Path, p);
    t.obj(Kind::Path, "moveTo", &[po, n(-1e7), n(-10.0)]);
    for i in 0..4000 {
        let s = if i % 2 == 0 { 1.0 } else { -1.0 };
        t.obj(Kind::Path, "bezierCurveTo", &[po, n(s * 1e7), n(74.0), n(-s * 1e7), n(-10.0), n(s * 1e7), n(74.0)]);
    }
    t.call("scale", &[n(2.0), n(1.0)]);
    t.call("fill", &[po]);
    assert!(t.c.last_fill_bytes() <= LIMIT, "{}", t.c.last_fill_bytes());
}
