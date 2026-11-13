mod common;
use common::*;
use invisium_canvas::*;
use std::collections::HashSet;

#[test]
fn c2_tables_cover_the_interface() {
    let spec = [
        "save", "restore", "reset", "isContextLost", "scale", "rotate", "translate", "transform",
        "getTransform", "setTransform", "resetTransform", "createLinearGradient",
        "createRadialGradient", "createConicGradient", "createPattern", "clearRect", "fillRect",
        "strokeRect", "beginPath", "fill", "stroke", "clip", "isPointInPath", "isPointInStroke",
        "fillText", "strokeText", "measureText", "drawImage", "createImageData", "getImageData",
        "putImageData", "setLineDash", "getLineDash", "roundRect", "arcTo", "ellipse",
        "letterSpacing", "lineDashOffset", "globalCompositeOperation", "imageSmoothingEnabled",
        "bezierCurveTo", "quadraticCurveTo", "fillStyle", "strokeStyle", "shadowBlur", "font",
        "textBaseline", "wordSpacing", "getContextAttributes", "drawFocusIfNeeded",
    ];
    assert!(spec.len() >= 40);
    let names: HashSet<&str> =
        methods().iter().map(|m| m.0).chain(properties().iter().map(|p| p.0)).collect();
    for s in spec {
        assert!(names.contains(s), "missing {s}");
    }
    assert!(object_methods(Kind::Gradient).iter().any(|m| m.0 == "addColorStop"));
    assert!(object_methods(Kind::Pattern).iter().any(|m| m.0 == "setTransform"));
    for m in ["addPath", "moveTo", "lineTo", "arc", "roundRect", "closePath"] {
        assert!(object_methods(Kind::Path).iter().any(|x| x.0 == m), "Path2D.{m}");
    }
    let mut ops = HashSet::new();
    let mut all = Vec::new();
    all.extend(methods().iter().map(|m| m.1));
    for p in properties() {
        all.push(p.1);
        all.push(p.2);
    }
    for k in [Kind::Gradient, Kind::Pattern, Kind::Path] {
        all.extend(object_methods(k).iter().map(|m| m.1));
    }
    for o in all {
        assert!(ops.insert(o), "duplicate op {o}");
    }
    let mut seen = HashSet::new();
    for m in methods() {
        assert!(seen.insert(m.0), "duplicate method name {}", m.0);
    }
}

#[test]
fn c2_properties_default_and_write() {
    let font = Arg::Font { font: 3, size: 20.0, css: "20px serif" };
    let cases: Vec<(&str, Ret, Arg, Ret)> = vec![
        ("globalAlpha", Ret::Num(1.0), n(0.5), Ret::Num(0.5)),
        ("globalCompositeOperation", Ret::Str("source-over".into()), s("multiply"), Ret::Str("multiply".into())),
        ("imageSmoothingEnabled", Ret::Bool(true), Arg::Bool(false), Ret::Bool(false)),
        ("imageSmoothingQuality", Ret::Str("low".into()), s("high"), Ret::Str("high".into())),
        ("strokeStyle", Ret::Str("#000000".into()), col(255, 0, 0, 255), Ret::Str("#ff0000".into())),
        ("fillStyle", Ret::Str("#000000".into()), col(0, 0, 255, 128), Ret::Str("rgba(0, 0, 255, 0.5)".into())),
        ("shadowOffsetX", Ret::Num(0.0), n(3.0), Ret::Num(3.0)),
        ("shadowOffsetY", Ret::Num(0.0), n(-2.0), Ret::Num(-2.0)),
        ("shadowBlur", Ret::Num(0.0), n(4.0), Ret::Num(4.0)),
        ("shadowColor", Ret::Str("rgba(0, 0, 0, 0)".into()), col(255, 0, 0, 255), Ret::Str("#ff0000".into())),
        ("filter", Ret::Str("none".into()), s("blur(2px)"), Ret::Str("blur(2px)".into())),
        ("lineWidth", Ret::Num(1.0), n(3.0), Ret::Num(3.0)),
        ("lineCap", Ret::Str("butt".into()), s("round"), Ret::Str("round".into())),
        ("lineJoin", Ret::Str("miter".into()), s("bevel"), Ret::Str("bevel".into())),
        ("miterLimit", Ret::Num(10.0), n(4.0), Ret::Num(4.0)),
        ("lineDashOffset", Ret::Num(0.0), n(2.5), Ret::Num(2.5)),
        ("font", Ret::Str("10px sans-serif".into()), font, Ret::Str("20px serif".into())),
        ("textAlign", Ret::Str("start".into()), s("center"), Ret::Str("center".into())),
        ("textBaseline", Ret::Str("alphabetic".into()), s("top"), Ret::Str("top".into())),
        ("direction", Ret::Str("inherit".into()), s("rtl"), Ret::Str("rtl".into())),
        ("letterSpacing", Ret::Str("0px".into()), s("3px"), Ret::Str("3px".into())),
        ("fontKerning", Ret::Str("auto".into()), s("none"), Ret::Str("none".into())),
        ("fontStretch", Ret::Str("normal".into()), s("condensed"), Ret::Str("condensed".into())),
        ("fontVariantCaps", Ret::Str("normal".into()), s("small-caps"), Ret::Str("small-caps".into())),
        ("textRendering", Ret::Str("auto".into()), s("optimizeSpeed"), Ret::Str("optimizeSpeed".into())),
        ("wordSpacing", Ret::Str("0px".into()), s("2px"), Ret::Str("2px".into())),
        ("lang", Ret::Str("inherit".into()), s("en"), Ret::Str("en".into())),
    ];
    let mut t = T::new(4, 4);
    let mut covered = HashSet::new();
    for (name, def, val, after) in cases {
        assert_eq!(t.get(name), def, "default of {name}");
        t.set(name, val);
        assert_eq!(t.get(name), after, "written {name}");
        covered.insert(name);
    }
    for p in properties() {
        assert!(covered.contains(p.0), "property {} not tested", p.0);
    }
    // Invalid values leave properties unchanged.
    t.set("lineWidth", n(-1.0));
    t.set("lineCap", s("bogus"));
    t.set("globalAlpha", n(2.0));
    t.set("globalCompositeOperation", s("nope"));
    t.set("miterLimit", n(f64::NAN));
    assert_eq!(t.get("lineWidth"), Ret::Num(3.0));
    assert_eq!(t.get("lineCap"), Ret::Str("round".into()));
    assert_eq!(t.get("globalAlpha"), Ret::Num(0.5));
    assert_eq!(t.get("globalCompositeOperation"), Ret::Str("multiply".into()));
    assert_eq!(t.get("miterLimit"), Ret::Num(4.0));
    // Gradient objects come back as the same handle.
    let g = t.call("createLinearGradient", &[n(0.0), n(0.0), n(1.0), n(0.0)]);
    let Ret::Object(Kind::Gradient, h) = g else { panic!("{g:?}") };
    t.set("fillStyle", Arg::Object(Kind::Gradient, h));
    assert_eq!(t.get("fillStyle"), Ret::Object(Kind::Gradient, h));
}

#[test]
fn c3_rects() {
    let mut t = T::new(10, 10);
    t.set("fillStyle", col(255, 0, 0, 255));
    t.call("fillRect", &[n(2.0), n(2.0), n(4.0), n(4.0)]);
    assert_eq!(t.px(3, 3), [255, 0, 0, 255]);
    assert_eq!(t.px(1, 1), [0, 0, 0, 0]);
    assert_eq!(t.px(6, 6), [0, 0, 0, 0]);
    t.call("clearRect", &[n(3.0), n(3.0), n(1.0), n(1.0)]);
    assert_eq!(t.px(3, 3), [0, 0, 0, 0]);
    assert_eq!(t.px(4, 4), [255, 0, 0, 255]);

    let mut t = T::new(10, 10);
    t.set("lineWidth", n(2.0));
    t.call("strokeRect", &[n(2.0), n(2.0), n(6.0), n(6.0)]);
    assert_eq!(t.alpha(1, 5), 255);
    assert_eq!(t.alpha(2, 5), 255);
    assert_eq!(t.alpha(0, 5), 0);
    assert_eq!(t.alpha(4, 5), 0);
    assert_eq!(t.alpha(8, 8), 255);
    assert_eq!(t.alpha(9, 9), 0);

    let mut t = T::new(4, 4);
    t.set("globalAlpha", n(0.5));
    t.call("fillRect", &[n(0.0), n(0.0), n(4.0), n(4.0)]);
    assert!(near(t.alpha(1, 1), 128, 1));
}

#[test]
fn c3_save_restore_resize_dirty() {
    let mut t = T::new(20, 20);
    assert!(!t.c.take_dirty());
    t.call("save", &[]);
    t.set("fillStyle", col(0, 255, 0, 255));
    t.call("translate", &[n(10.0), n(0.0)]);
    t.call("rect", &[n(0.0), n(0.0), n(5.0), n(5.0)]);
    t.call("clip", &[]);
    t.call("fillRect", &[n(-10.0), n(0.0), n(20.0), n(20.0)]);
    assert_eq!(t.px(12, 2), [0, 255, 0, 255]);
    assert_eq!(t.alpha(12, 8), 0, "clip limits drawing");
    assert_eq!(t.alpha(2, 2), 0, "clip limits drawing");
    assert!(t.c.take_dirty());
    assert!(!t.c.take_dirty());
    t.call("restore", &[]);
    assert_eq!(t.get("fillStyle"), Ret::Str("#000000".into()));
    assert_eq!(t.call("getTransform", &[]), Ret::Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
    t.call("fillRect", &[n(0.0), n(15.0), n(5.0), n(5.0)]);
    assert_eq!(t.px(2, 17), [0, 0, 0, 255], "clip restored away");

    t.set("fillStyle", col(1, 2, 3, 255));
    t.c.resize(20, 20);
    assert!(t.c.take_dirty());
    assert_eq!(t.alpha(12, 2), 0);
    assert_eq!(t.get("fillStyle"), Ret::Str("#000000".into()));
    t.c.resize(30, 10);
    assert_eq!(t.c.size(), (30, 10));
    assert_eq!(t.rgba().len(), 30 * 10 * 4);
}
