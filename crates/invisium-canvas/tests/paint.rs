mod common;
use common::*;
use invisium_canvas::*;
use std::f64::consts::FRAC_PI_2;

fn handle(r: Ret) -> (Kind, u32) {
    match r {
        Ret::Object(k, h) => (k, h),
        o => panic!("not an object: {o:?}"),
    }
}

fn stop(t: &mut T, g: u32, off: f64, c: Arg) -> Ret {
    t.obj(Kind::Gradient, "addColorStop", &[Arg::Object(Kind::Gradient, g), n(off), c])
}

#[test]
fn c7_transforms() {
    let mut t = T::new(40, 40);
    t.call("scale", &[n(2.0), n(2.0)]);
    t.call("fillRect", &[n(1.0), n(1.0), n(2.0), n(2.0)]);
    assert_eq!(t.ink(), Some((2, 2, 6, 6)));
    assert_eq!(t.call("getTransform", &[]), Ret::Matrix([2.0, 0.0, 0.0, 2.0, 0.0, 0.0]));

    // Rotate a quarter turn about (20, 20): (x, y) -> (40 - y, x).
    let mut t = T::new(40, 40);
    t.call("translate", &[n(20.0), n(20.0)]);
    t.call("rotate", &[n(FRAC_PI_2)]);
    t.call("translate", &[n(-20.0), n(-20.0)]);
    t.call("fillRect", &[n(20.0), n(10.0), n(10.0), n(2.0)]);
    assert_eq!(t.ink(), Some((28, 20, 30, 30)));

    let mut t = T::new(20, 20);
    t.call("setTransform", &[n(1.0), n(0.0), n(0.0), n(1.0), n(5.0), n(5.0)]);
    t.call("fillRect", &[n(0.0), n(0.0), n(2.0), n(2.0)]);
    assert_eq!(t.ink(), Some((5, 5, 7, 7)));
    t.call("transform", &[n(1.0), n(0.0), n(0.0), n(1.0), n(1.0), n(2.0)]);
    assert_eq!(t.call("getTransform", &[]), Ret::Matrix([1.0, 0.0, 0.0, 1.0, 6.0, 7.0]));
    t.call("resetTransform", &[]);
    assert_eq!(t.call("getTransform", &[]), Ret::Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));

    // Non-invertible transforms draw nothing.
    let mut t = T::new(20, 20);
    t.call("scale", &[n(0.0), n(1.0)]);
    t.call("fillRect", &[n(0.0), n(0.0), n(20.0), n(20.0)]);
    t.call("setTransform", &[n(0.0), n(0.0), n(0.0), n(0.0), n(0.0), n(0.0)]);
    t.call("fillRect", &[n(0.0), n(0.0), n(20.0), n(20.0)]);
    t.call("rect", &[n(0.0), n(0.0), n(20.0), n(20.0)]);
    t.call("fill", &[]);
    t.call("stroke", &[]);
    assert_eq!(t.ink(), None);
}

#[test]
fn c7_transformed_gradient_and_pattern() {
    let mut t = T::new(20, 2);
    let (_, g) = handle(t.call("createLinearGradient", &[n(0.0), n(0.0), n(10.0), n(0.0)]));
    stop(&mut t, g, 0.0, col(0, 0, 0, 255));
    stop(&mut t, g, 1.0, col(255, 255, 255, 255));
    t.set("fillStyle", Arg::Object(Kind::Gradient, g));
    t.call("scale", &[n(2.0), n(1.0)]);
    t.call("fillRect", &[n(0.0), n(0.0), n(10.0), n(2.0)]);
    // Device x = 10 has its centre at user x = 5.25.
    assert!(near(t.px(10, 0)[0], (0.525 * 255.0f64).round() as u8, 1), "{:?}", t.px(10, 0));
    assert!(near(t.px(2, 0)[0], (0.125 * 255.0f64).round() as u8, 1));

    let img = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
    let mut t = T::new(8, 8);
    let (_, p) = handle(t.call("createPattern", &[Arg::Pixels { width: 2, height: 2, rgba: &img }, s("repeat")]));
    t.set("fillStyle", Arg::Object(Kind::Pattern, p));
    t.set("imageSmoothingEnabled", Arg::Bool(false));
    t.call("scale", &[n(2.0), n(2.0)]);
    t.call("fillRect", &[n(0.0), n(0.0), n(4.0), n(4.0)]);
    assert_eq!(t.px(0, 0), [255, 0, 0, 255]);
    assert_eq!(t.px(1, 1), [255, 0, 0, 255]);
    assert_eq!(t.px(2, 0), [0, 255, 0, 255]);
    assert_eq!(t.px(0, 2), [0, 0, 255, 255]);
    assert_eq!(t.px(7, 7), [255, 255, 255, 255]);
    // The pattern's own transform.
    let mut t = T::new(4, 4);
    let (_, p) = handle(t.call("createPattern", &[Arg::Pixels { width: 2, height: 2, rgba: &img }, s("repeat")]));
    let m = [1.0, 0.0, 0.0, 1.0, 1.0, 0.0].map(n);
    let mut a = vec![Arg::Object(Kind::Pattern, p)];
    a.extend_from_slice(&m);
    t.obj(Kind::Pattern, "setTransform", &a);
    t.set("fillStyle", Arg::Object(Kind::Pattern, p));
    t.call("fillRect", &[n(0.0), n(0.0), n(4.0), n(4.0)]);
    assert_eq!(t.px(1, 0), [255, 0, 0, 255]);
    assert_eq!(t.px(0, 0), [0, 255, 0, 255]);
}

#[test]
fn c8_gradients() {
    let mut t = T::new(100, 1);
    let (_, g) = handle(t.call("createLinearGradient", &[n(0.0), n(0.0), n(100.0), n(0.0)]));
    stop(&mut t, g, 0.0, col(255, 0, 0, 255));
    stop(&mut t, g, 1.0, col(0, 0, 255, 255));
    t.set("fillStyle", Arg::Object(Kind::Gradient, g));
    t.call("fillRect", &[n(0.0), n(0.0), n(100.0), n(1.0)]);
    let tt = 0.495;
    assert!(near4(t.px(49, 0), [((1.0 - tt) * 255.0f64).round() as u8, 0, (tt * 255.0f64).round() as u8, 255], 1));
    // Interpolation in premultiplied space: opaque red to transparent gives pure red midway.
    let mut t = T::new(100, 1);
    let (_, g) = handle(t.call("createLinearGradient", &[n(0.0), n(0.0), n(100.0), n(0.0)]));
    stop(&mut t, g, 0.0, col(255, 0, 0, 255));
    stop(&mut t, g, 1.0, col(0, 0, 255, 0));
    t.set("fillStyle", Arg::Object(Kind::Gradient, g));
    t.call("fillRect", &[n(0.0), n(0.0), n(100.0), n(1.0)]);
    let p = t.px(49, 0);
    assert!(near(p[0], 255, 1) && p[2] == 0 && near(p[3], 129, 1), "{p:?}");
    // Equal offsets: a hard edge.
    let mut t = T::new(100, 1);
    let (_, g) = handle(t.call("createLinearGradient", &[n(0.0), n(0.0), n(100.0), n(0.0)]));
    stop(&mut t, g, 0.0, col(255, 0, 0, 255));
    stop(&mut t, g, 0.5, col(255, 0, 0, 255));
    stop(&mut t, g, 0.5, col(0, 0, 255, 255));
    stop(&mut t, g, 1.0, col(0, 0, 255, 255));
    t.set("fillStyle", Arg::Object(Kind::Gradient, g));
    t.call("fillRect", &[n(0.0), n(0.0), n(100.0), n(1.0)]);
    assert_eq!(t.px(49, 0), [255, 0, 0, 255]);
    assert_eq!(t.px(50, 0), [0, 0, 255, 255]);
    assert!(matches!(stop(&mut t, g, 1.5, col(0, 0, 0, 255)), Ret::Throw("IndexSizeError", _)));
    assert!(matches!(stop(&mut t, g, 0.5, s("nonsense")), Ret::Throw("SyntaxError", _)));

    // Radial: concentric, black at the centre to white at radius 50, padded outside.
    let mut t = T::new(100, 100);
    let (_, g) = handle(t.call("createRadialGradient", &[n(50.0), n(50.0), n(0.0), n(50.0), n(50.0), n(50.0)]));
    stop(&mut t, g, 0.0, col(0, 0, 0, 255));
    stop(&mut t, g, 1.0, col(255, 255, 255, 255));
    t.set("fillStyle", Arg::Object(Kind::Gradient, g));
    t.call("fillRect", &[n(0.0), n(0.0), n(100.0), n(100.0)]);
    let d = ((25.5f64 - 0.0).powi(2) + 0.5f64.powi(2)).sqrt() / 50.0;
    assert!(near(t.px(75, 50)[0], (d * 255.0).round() as u8, 1), "{:?}", t.px(75, 50));
    assert_eq!(t.px(0, 0), [255, 255, 255, 255]);
    assert!(matches!(
        t.call("createRadialGradient", &[n(0.0), n(0.0), n(-1.0), n(0.0), n(0.0), n(1.0)]),
        Ret::Throw("IndexSizeError", _)
    ));

    // Conic: starts at the positive x axis and turns clockwise (y down).
    let mut t = T::new(100, 100);
    let (_, g) = handle(t.call("createConicGradient", &[n(0.0), n(50.0), n(50.0)]));
    stop(&mut t, g, 0.0, col(0, 0, 0, 255));
    stop(&mut t, g, 1.0, col(255, 255, 255, 255));
    t.set("fillStyle", Arg::Object(Kind::Gradient, g));
    t.call("fillRect", &[n(0.0), n(0.0), n(100.0), n(100.0)]);
    let ang = |x: f64, y: f64| ((y + 0.5 - 50.0).atan2(x + 0.5 - 50.0) / std::f64::consts::TAU).rem_euclid(1.0);
    for (x, y) in [(50u32, 90u32), (10, 50), (50, 10), (80, 70)] {
        let want = (ang(x as f64, y as f64) * 255.0).round() as u8;
        assert!(near(t.px(x, y)[0], want, 1), "conic at {x},{y}: {:?} vs {want}", t.px(x, y));
    }
}

#[test]
fn c8_pattern_repetitions() {
    let img = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
    let draw = |rep: &str| {
        let mut t = T::new(6, 6);
        let (_, p) = handle(t.call("createPattern", &[Arg::Pixels { width: 2, height: 2, rgba: &img }, s(rep)]));
        t.set("fillStyle", Arg::Object(Kind::Pattern, p));
        t.call("fillRect", &[n(0.0), n(0.0), n(6.0), n(6.0)]);
        t
    };
    let t = draw("repeat");
    assert_eq!(t.px(2, 0), [255, 0, 0, 255]);
    assert_eq!(t.px(5, 5), [255, 255, 255, 255]);
    let t = draw("repeat-x");
    assert_eq!(t.px(4, 1), [0, 0, 255, 255]);
    assert_eq!(t.alpha(0, 2), 0);
    let t = draw("repeat-y");
    assert_eq!(t.px(1, 4), [0, 255, 0, 255]);
    assert_eq!(t.alpha(2, 0), 0);
    let t = draw("no-repeat");
    assert_eq!(t.px(1, 1), [255, 255, 255, 255]);
    assert_eq!(t.ink(), Some((0, 0, 2, 2)));
    let mut t = T::new(2, 2);
    assert!(matches!(
        t.call("createPattern", &[Arg::Pixels { width: 2, height: 2, rgba: &img }, s("diagonal")]),
        Ret::Throw("SyntaxError", _)
    ));
}

type F = fn(f64, f64) -> f64;

/// Reference compositing on straight colours (0..1): returns straight RGBA.
fn reference(op: &str, s: [f64; 4], d: [f64; 4]) -> [f64; 4] {
    let (sa, da) = (s[3], d[3]);
    let pd = |fa: f64, fb: f64| {
        let a = sa * fa + da * fb;
        let mut o = [0.0; 4];
        for k in 0..3 {
            let c = s[k] * sa * fa + d[k] * da * fb;
            o[k] = if a > 0.0 { c / a } else { 0.0 };
        }
        o[3] = a;
        o
    };
    let blend = |b: F| {
        let a = sa + da - sa * da;
        let mut o = [0.0; 4];
        for k in 0..3 {
            let c = s[k] * sa * (1.0 - da) + d[k] * da * (1.0 - sa) + sa * da * b(d[k], s[k]);
            o[k] = c / a;
        }
        o[3] = a;
        o
    };
    match op {
        "source-over" => pd(1.0, 1.0 - sa),
        "source-in" => pd(da, 0.0),
        "source-out" => pd(1.0 - da, 0.0),
        "source-atop" => pd(da, 1.0 - sa),
        "destination-over" => pd(1.0 - da, 1.0),
        "destination-in" => pd(0.0, sa),
        "destination-out" => pd(0.0, 1.0 - sa),
        "destination-atop" => pd(1.0 - da, sa),
        "lighter" => {
            let a = (sa + da).min(1.0);
            let mut o = [0.0; 4];
            for k in 0..3 {
                o[k] = (s[k] * sa + d[k] * da).min(1.0) / a;
            }
            o[3] = a;
            o
        }
        "copy" => s,
        "xor" => pd(1.0 - da, 1.0 - sa),
        "multiply" => blend(|b, s| b * s),
        "screen" => blend(|b, s| b + s - b * s),
        "overlay" => blend(|b, s| if b <= 0.5 { 2.0 * b * s } else { 1.0 - 2.0 * (1.0 - b) * (1.0 - s) }),
        "darken" => blend(|b, s| b.min(s)),
        "lighten" => blend(|b, s| b.max(s)),
        "difference" => blend(|b, s| (b - s).abs()),
        "exclusion" => blend(|b, s| b + s - 2.0 * b * s),
        _ => unreachable!(),
    }
}

#[test]
fn c8_every_operator_and_blend_mode() {
    let (d8, s8) = ([200u8, 100, 50, 153], [50u8, 150, 250, 204]);
    let ops = [
        "source-over", "source-in", "source-out", "source-atop", "destination-over", "destination-in",
        "destination-out", "destination-atop", "lighter", "copy", "xor", "multiply", "screen", "overlay",
        "darken", "lighten", "difference", "exclusion",
    ];
    let f = |c: [u8; 4]| c.map(|v| v as f64 / 255.0);
    for op in ops {
        let mut t = T::new(4, 4);
        t.set("fillStyle", Arg::Color(d8));
        t.call("fillRect", &[n(0.0), n(0.0), n(4.0), n(4.0)]);
        t.set("globalCompositeOperation", s(op));
        assert_eq!(t.get("globalCompositeOperation"), Ret::Str(op.into()));
        t.set("fillStyle", Arg::Color(s8));
        t.call("fillRect", &[n(0.0), n(0.0), n(4.0), n(4.0)]);
        let r = reference(op, f(s8), f(d8));
        let want = r.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
        let got = t.px(1, 1);
        let want = if want[3] == 0 { [0, 0, 0, 0] } else { want };
        assert!(near4(got, want, 2), "{op}: got {got:?} want {want:?}");
    }
    // Unbounded operators clear outside the shape.
    let mut t = T::new(4, 4);
    t.call("fillRect", &[n(0.0), n(0.0), n(4.0), n(4.0)]);
    t.set("globalCompositeOperation", s("copy"));
    t.call("fillRect", &[n(0.0), n(0.0), n(2.0), n(2.0)]);
    assert_eq!(t.alpha(3, 3), 0);
    assert_eq!(t.alpha(1, 1), 255);
}
