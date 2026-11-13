mod common;
use common::*;
use invisium_canvas::*;
use std::f64::consts::{FRAC_PI_2, PI};

fn in_stroke(t: &mut T, x: f64, y: f64) -> bool {
    t.call("isPointInStroke", &[n(x), n(y)]) == Ret::Bool(true)
}

fn in_path(t: &mut T, x: f64, y: f64) -> bool {
    t.call("isPointInPath", &[n(x), n(y)]) == Ret::Bool(true)
}

#[test]
fn c4_half_pixel_and_circle_area() {
    let mut t = T::new(10, 10);
    t.call("fillRect", &[n(2.0), n(0.0), n(0.5), n(10.0)]);
    for y in 0..10 {
        assert!(near(t.alpha(2, y), 128, 1), "half coverage {}", t.alpha(2, y));
        assert_eq!(t.alpha(1, y), 0);
        assert_eq!(t.alpha(3, y), 0);
    }
    let mut t = T::new(10, 10);
    t.call("fillRect", &[n(2.25), n(3.0), n(1.0), n(1.0)]);
    assert!(near(t.alpha(2, 3), 191, 1) && near(t.alpha(3, 3), 64, 1));

    let mut t = T::new(64, 64);
    t.call("arc", &[n(32.0), n(32.0), n(20.0), n(0.0), n(2.0 * PI)]);
    t.call("fill", &[]);
    let area = t.alpha_sum();
    let want = PI * 400.0;
    assert!((area - want).abs() / want < 0.01, "area {area} vs {want}");
    let partial = t.rgba().chunks(4).filter(|p| p[3] > 0 && p[3] < 255).count();
    assert!(partial > 40, "anti-aliased edge pixels: {partial}");
    assert_eq!(t.alpha(32, 32), 255);
    assert_eq!(t.alpha(2, 2), 0);
}

fn star(t: &mut T) {
    t.call("beginPath", &[]);
    for i in 0..5 {
        let a = -FRAC_PI_2 + i as f64 * 4.0 * PI / 5.0;
        let (x, y) = (50.0 + 40.0 * a.cos(), 50.0 + 40.0 * a.sin());
        t.call(if i == 0 { "moveTo" } else { "lineTo" }, &[n(x), n(y)]);
    }
    t.call("closePath", &[]);
}

#[test]
fn c4_fill_rules_and_clip_nesting() {
    let mut t = T::new(100, 100);
    star(&mut t);
    t.call("fill", &[]);
    assert_eq!(t.alpha(50, 50), 255, "nonzero fills the centre");
    let mut t = T::new(100, 100);
    star(&mut t);
    t.call("fill", &[s("evenodd")]);
    assert_eq!(t.alpha(50, 50), 0, "evenodd leaves the centre");
    assert_eq!(t.alpha(50, 15), 255, "evenodd fills a point");
    assert!(in_path(&mut t, 50.0, 15.0));
    assert_eq!(t.call("isPointInPath", &[n(50.0), n(50.0), s("evenodd")]), Ret::Bool(false));
    assert_eq!(t.call("isPointInPath", &[n(50.0), n(50.0), s("nonzero")]), Ret::Bool(true));
    assert!(matches!(t.call("fill", &[s("bogus")]), Ret::Throw("TypeError", _)));

    let mut t = T::new(30, 30);
    t.call("rect", &[n(0.0), n(0.0), n(20.0), n(20.0)]);
    t.call("clip", &[]);
    t.call("beginPath", &[]);
    t.call("rect", &[n(10.0), n(10.0), n(20.0), n(20.0)]);
    t.call("clip", &[]);
    t.call("fillRect", &[n(0.0), n(0.0), n(30.0), n(30.0)]);
    assert_eq!(t.ink(), Some((10, 10, 20, 20)), "clips intersect");
}

#[test]
fn c5_segments_and_arcs() {
    // Quadratic and cubic curves bulge towards their control points.
    let mut t = T::new(100, 100);
    t.call("moveTo", &[n(10.0), n(50.0)]);
    t.call("quadraticCurveTo", &[n(50.0), n(0.0), n(90.0), n(50.0)]);
    t.call("closePath", &[]);
    assert!(in_path(&mut t, 50.0, 30.0));
    assert!(!in_path(&mut t, 50.0, 20.0));
    t.call("beginPath", &[]);
    t.call("moveTo", &[n(10.0), n(50.0)]);
    t.call("bezierCurveTo", &[n(10.0), n(90.0), n(90.0), n(90.0), n(90.0), n(50.0)]);
    t.call("closePath", &[]);
    assert!(in_path(&mut t, 50.0, 75.0));
    assert!(!in_path(&mut t, 50.0, 85.0));

    // Clockwise quarter pie vs anticlockwise three-quarter pie.
    let mut t = T::new(100, 100);
    t.call("moveTo", &[n(50.0), n(50.0)]);
    t.call("arc", &[n(50.0), n(50.0), n(30.0), n(0.0), n(FRAC_PI_2)]);
    t.call("fill", &[]);
    assert_eq!(t.alpha(60, 60), 255);
    assert_eq!(t.alpha(40, 40), 0);
    let mut t = T::new(100, 100);
    t.call("moveTo", &[n(50.0), n(50.0)]);
    t.call("arc", &[n(50.0), n(50.0), n(30.0), n(0.0), n(FRAC_PI_2), Arg::Bool(true)]);
    t.call("fill", &[]);
    assert_eq!(t.alpha(60, 60), 0);
    assert_eq!(t.alpha(40, 40), 255);
    // Beyond a full turn: a whole circle.
    let mut t = T::new(100, 100);
    t.call("arc", &[n(50.0), n(50.0), n(30.0), n(0.0), n(3.0 * PI)]);
    t.call("fill", &[]);
    assert!((t.alpha_sum() - PI * 900.0).abs() < PI * 9.0);
    assert!(matches!(t.call("arc", &[n(0.0), n(0.0), n(-1.0), n(0.0), n(1.0)]), Ret::Throw("IndexSizeError", _)));

    // arcTo: a rounded corner tangent to both lines.
    let mut t = T::new(60, 60);
    t.call("moveTo", &[n(0.0), n(0.0)]);
    t.call("arcTo", &[n(40.0), n(0.0), n(40.0), n(40.0), n(20.0)]);
    t.call("lineTo", &[n(40.0), n(40.0)]);
    let k = 20.0 * std::f64::consts::FRAC_1_SQRT_2;
    assert!(in_stroke(&mut t, 20.0 + k, 20.0 - k));
    assert!(in_stroke(&mut t, 10.0, 0.0));
    assert!(!in_stroke(&mut t, 39.5, 0.5));

    // Ellipse rotated by 90 degrees: tall.
    let mut t = T::new(64, 64);
    t.call("ellipse", &[n(32.0), n(32.0), n(20.0), n(5.0), n(FRAC_PI_2), n(0.0), n(2.0 * PI)]);
    t.call("fill", &[]);
    assert_eq!(t.alpha(32, 48), 255);
    assert_eq!(t.alpha(48, 32), 0);
}

#[test]
fn c5_round_rect() {
    let mut t = T::new(40, 40);
    t.call("roundRect", &[n(0.0), n(0.0), n(40.0), n(40.0), n(10.0)]);
    t.call("fill", &[]);
    assert_eq!(t.alpha(0, 0), 0);
    assert_eq!(t.alpha(39, 39), 0);
    assert_eq!(t.alpha(20, 0), 255);
    let mut t = T::new(40, 40);
    let r = [0.0, 10.0, 0.0, 10.0];
    t.call("roundRect", &[n(0.0), n(0.0), n(40.0), n(40.0), Arg::Nums(&r)]);
    t.call("fill", &[]);
    assert_eq!(t.alpha(0, 0), 255, "top-left square");
    assert_eq!(t.alpha(39, 0), 0, "top-right round");
    assert_eq!(t.alpha(39, 39), 255, "bottom-right square");
    assert_eq!(t.alpha(0, 39), 0, "bottom-left round");
    assert!(matches!(t.call("roundRect", &[n(0.0), n(0.0), n(4.0), n(4.0), n(-1.0)]), Ret::Throw("RangeError", _)));
}

#[test]
fn c5_path2d_from_svg_matches_calls() {
    let mut a = T::new(64, 64);
    let p = a.obj(Kind::Path, "Path2D", &[s("M10 10 L50 10 Q60 30 50 50 C40 60 20 60 10 50 Z M20 20 h10 v10 h-10 z")]);
    let Ret::Object(Kind::Path, h) = p else { panic!("{p:?}") };
    a.call("fill", &[Arg::Object(Kind::Path, h), s("evenodd")]);
    let mut b = T::new(64, 64);
    b.call("moveTo", &[n(10.0), n(10.0)]);
    b.call("lineTo", &[n(50.0), n(10.0)]);
    b.call("quadraticCurveTo", &[n(60.0), n(30.0), n(50.0), n(50.0)]);
    b.call("bezierCurveTo", &[n(40.0), n(60.0), n(20.0), n(60.0), n(10.0), n(50.0)]);
    b.call("closePath", &[]);
    b.call("rect", &[n(20.0), n(20.0), n(10.0), n(10.0)]);
    b.call("fill", &[s("evenodd")]);
    assert_eq!(a.rgba(), b.rgba());
    assert!(a.alpha(15, 15) == 255 && a.alpha(25, 25) == 0);
    // Path2D building methods, addPath and hit tests.
    let q = a.obj(Kind::Path, "Path2D", &[]);
    let Ret::Object(Kind::Path, q) = q else { panic!() };
    a.obj(Kind::Path, "rect", &[Arg::Object(Kind::Path, q), n(0.0), n(0.0), n(5.0), n(5.0)]);
    let six = [1.0, 0.0, 0.0, 1.0, 30.0, 0.0].map(n);
    let mut args = vec![Arg::Object(Kind::Path, h), Arg::Object(Kind::Path, q)];
    args.extend_from_slice(&six);
    a.obj(Kind::Path, "addPath", &args);
    assert_eq!(a.call("isPointInPath", &[Arg::Object(Kind::Path, q), n(2.0), n(2.0)]), Ret::Bool(true));
    assert_eq!(a.call("isPointInPath", &[Arg::Object(Kind::Path, h), n(32.0), n(2.0)]), Ret::Bool(true));
    assert_eq!(a.call("isPointInStroke", &[Arg::Object(Kind::Path, q), n(5.0), n(2.0)]), Ret::Bool(true));
    assert_eq!(a.call("isPointInStroke", &[Arg::Object(Kind::Path, q), n(2.0), n(2.0)]), Ret::Bool(false));
    // An SVG arc command.
    let p = a.obj(Kind::Path, "Path2D", &[s("M10 32 A 22 22 0 0 1 54 32 Z")]);
    let Ret::Object(Kind::Path, h) = p else { panic!() };
    assert_eq!(a.call("isPointInPath", &[Arg::Object(Kind::Path, h), n(32.0), n(15.0)]), Ret::Bool(true));
    assert_eq!(a.call("isPointInPath", &[Arg::Object(Kind::Path, h), n(32.0), n(40.0)]), Ret::Bool(false));
}

fn line(t: &mut T, cap: &str) {
    t.call("beginPath", &[]);
    t.set("lineWidth", n(10.0));
    t.set("lineCap", s(cap));
    t.call("moveTo", &[n(10.0), n(20.0)]);
    t.call("lineTo", &[n(30.0), n(20.0)]);
}

#[test]
fn c6_caps() {
    let mut t = T::new(40, 40);
    line(&mut t, "butt");
    assert!(!in_stroke(&mut t, 9.0, 20.0) && in_stroke(&mut t, 11.0, 20.0));
    line(&mut t, "square");
    assert!(in_stroke(&mut t, 6.0, 24.0) && !in_stroke(&mut t, 4.0, 20.0));
    line(&mut t, "round");
    assert!(in_stroke(&mut t, 6.0, 20.0) && !in_stroke(&mut t, 6.3, 23.7) && in_stroke(&mut t, 7.0, 23.0));
    t.call("stroke", &[]);
    assert_eq!(t.alpha(20, 17), 255);
    assert_eq!(t.alpha(20, 25), 0);
}

fn corner(t: &mut T, join: &str) {
    t.call("beginPath", &[]);
    t.set("lineWidth", n(10.0));
    t.set("lineJoin", s(join));
    t.call("moveTo", &[n(10.0), n(40.0)]);
    t.call("lineTo", &[n(30.0), n(40.0)]);
    t.call("lineTo", &[n(30.0), n(60.0)]);
}

#[test]
fn c6_joins_and_miter_limit() {
    let mut t = T::new(80, 80);
    corner(&mut t, "miter");
    assert!(in_stroke(&mut t, 34.5, 35.5));
    corner(&mut t, "bevel");
    assert!(!in_stroke(&mut t, 34.0, 36.0) && in_stroke(&mut t, 32.0, 38.0));
    corner(&mut t, "round");
    assert!(!in_stroke(&mut t, 34.0, 36.0) && in_stroke(&mut t, 33.0, 37.0));
    // A sharp join: tip ratio 1 / sin(10 degrees) = 5.76.
    let a: f64 = 20f64.to_radians();
    let sharp = |t: &mut T, limit: f64| {
        t.call("beginPath", &[]);
        t.set("lineJoin", s("miter"));
        t.set("lineWidth", n(4.0));
        t.set("miterLimit", n(limit));
        t.call("moveTo", &[n(10.0), n(40.0)]);
        t.call("lineTo", &[n(60.0), n(40.0)]);
        t.call("lineTo", &[n(60.0 - 50.0 * a.cos()), n(40.0 - 50.0 * a.sin())]);
    };
    // The miter tip lies beyond the vertex along the bisector, about 11.5 px out.
    let tip = (60.0 + 10.0 * (a / 2.0).cos(), 40.0 + 10.0 * (a / 2.0).sin());
    sharp(&mut t, 10.0);
    assert!(in_stroke(&mut t, tip.0, tip.1));
    sharp(&mut t, 5.0);
    assert!(!in_stroke(&mut t, tip.0, tip.1));
    assert!(in_stroke(&mut t, 60.3, 40.0), "bevel remains");
}

#[test]
fn c6_dashes_and_closed_paths() {
    let mut t = T::new(100, 20);
    let d = [10.0, 10.0];
    t.call("setLineDash", &[Arg::Nums(&d)]);
    assert_eq!(t.call("getLineDash", &[]), Ret::Nums(vec![10.0, 10.0]));
    t.set("lineWidth", n(2.0));
    t.call("moveTo", &[n(0.0), n(10.0)]);
    t.call("lineTo", &[n(100.0), n(10.0)]);
    t.call("stroke", &[]);
    assert_eq!(t.alpha(5, 10), 255);
    assert_eq!(t.alpha(15, 10), 0);
    assert_eq!(t.alpha(25, 10), 255);
    t.clear();
    t.set("lineDashOffset", n(5.0));
    t.call("stroke", &[]);
    assert_eq!(t.alpha(2, 10), 255);
    assert_eq!(t.alpha(7, 10), 0);
    assert_eq!(t.alpha(17, 10), 255);
    let odd = [3.0];
    t.call("setLineDash", &[Arg::Nums(&odd)]);
    assert_eq!(t.call("getLineDash", &[]), Ret::Nums(vec![3.0, 3.0]));

    // A closed path has no caps; the same path left open does.
    let mut t = T::new(60, 60);
    let tri = |t: &mut T, close: bool| {
        t.call("beginPath", &[]);
        t.set("lineWidth", n(10.0));
        t.set("lineCap", s("square"));
        t.set("lineJoin", s("bevel"));
        t.call("moveTo", &[n(10.0), n(10.0)]);
        t.call("lineTo", &[n(50.0), n(10.0)]);
        t.call("lineTo", &[n(30.0), n(40.0)]);
        t.call("lineTo", &[n(10.0), n(10.0)]);
        if close {
            t.call("closePath", &[]);
        }
    };
    tri(&mut t, true);
    assert!(!in_stroke(&mut t, 6.0, 10.0));
    tri(&mut t, false);
    assert!(in_stroke(&mut t, 6.0, 10.0));
}
