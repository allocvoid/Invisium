mod common;
use common::*;
use invisium_canvas::*;

fn font20(t: &mut T) {
    t.set("font", Arg::Font { font: 0, size: 20.0, css: "20px test" });
}

fn metrics(t: &mut T, text: &str) -> TextMetrics {
    match t.call("measureText", &[s(text)]) {
        Ret::Text(m) => m,
        o => panic!("{o:?}"),
    }
}

#[test]
fn c9_text_box_align_baseline() {
    let mut t = T::new(100, 60);
    font20(&mut t);
    let m = metrics(&mut t, "Hi");
    assert_eq!(m.width, 20.0);
    assert_eq!((m.font_ascent, m.font_descent), (16.0, 4.0));
    assert_eq!(m.alphabetic_baseline, 0.0);
    t.call("fillText", &[s("Hi"), n(10.0), n(30.0)]);
    let ink = t.ink().expect("ink");
    let bx = ((10.0 - m.actual_left) as u32, (30.0 - m.actual_ascent) as u32);
    let bx1 = ((10.0 + m.actual_right) as u32, (30.0 + m.actual_descent) as u32);
    assert!(ink.0 >= bx.0 && ink.1 >= bx.1 && ink.2 <= bx1.0 && ink.3 <= bx1.1, "{ink:?} in {bx:?}-{bx1:?}");
    assert_eq!(ink, (bx.0, bx.1, bx1.0, bx1.1), "ink fills the measured box");

    // Alignment moves the ink by the width (or half of it).
    for (align, dx) in [("right", -20i32), ("center", -10), ("end", -20), ("left", 0)] {
        let mut u = T::new(100, 60);
        font20(&mut u);
        u.set("textAlign", s(align));
        u.call("fillText", &[s("Hi"), n(50.0), n(30.0)]);
        let i = u.ink().unwrap();
        assert_eq!(i.0 as i32, 51 + dx, "{align}");
    }
    // Baselines move it vertically by the font's metrics.
    for (bl, top) in [("top", 32u32), ("bottom", 12), ("middle", 22), ("alphabetic", 16), ("hanging", 29)] {
        let mut u = T::new(100, 60);
        font20(&mut u);
        u.set("textBaseline", s(bl));
        u.call("fillText", &[s("Hi"), n(10.0), n(30.0)]);
        assert_eq!(u.ink().unwrap().1, top, "{bl}");
        let m = metrics(&mut u, "Hi");
        assert_eq!(30.0 - m.actual_ascent, top as f64, "{bl} measured");
    }
}

#[test]
fn c9_spacing_max_width_and_scale() {
    let mut t = T::new(200, 60);
    font20(&mut t);
    t.set("letterSpacing", s("3px"));
    let m = metrics(&mut t, "abcd");
    assert_eq!(m.width, 4.0 * 10.0 + 4.0 * 3.0);
    t.set("letterSpacing", s("0px"));
    t.set("wordSpacing", s("5px"));
    assert_eq!(metrics(&mut t, "ab cd").width, 55.0);
    t.set("wordSpacing", s("0px"));

    // maxWidth squeezes the drawing horizontally.
    t.call("fillText", &[s("abcd"), n(10.0), n(30.0), n(20.0)]);
    let i = t.ink().unwrap();
    assert!(i.2 - i.0 <= 20 && i.2 - i.0 >= 16, "{i:?}");
    t.clear();
    t.call("fillText", &[s("abcd"), n(10.0), n(30.0), n(0.0)]);
    assert_eq!(t.ink(), None);

    // Under scale(2, 2) glyphs are rasterised at twice the size.
    let mut t = T::new(200, 100);
    font20(&mut t);
    t.call("scale", &[n(2.0), n(2.0)]);
    t.f.sizes.clear();
    t.call("fillText", &[s("A"), n(10.0), n(30.0)]);
    assert_eq!(t.f.sizes, vec![40.0]);
    assert_eq!(t.ink(), Some((22, 32, 38, 60)));
    // Rotated text is resampled and still lands where the transform says.
    let mut t = T::new(100, 100);
    font20(&mut t);
    t.call("translate", &[n(50.0), n(50.0)]);
    t.call("rotate", &[n(std::f64::consts::FRAC_PI_2)]);
    t.call("fillText", &[s("A"), n(0.0), n(0.0)]);
    let i = t.ink().unwrap();
    assert!(i.0 >= 50 && i.2 <= 66 && i.1 >= 50 && i.3 <= 61, "{i:?}");
}

const IMG: [u8; 16] = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];

fn img() -> Arg<'static> {
    Arg::Pixels { width: 2, height: 2, rgba: &IMG }
}

#[test]
fn c10_draw_image() {
    let mut t = T::new(10, 10);
    t.call("drawImage", &[img(), n(1.0), n(1.0)]);
    assert_eq!(t.px(1, 1), [255, 0, 0, 255]);
    assert_eq!(t.px(2, 2), [255, 255, 255, 255]);
    assert_eq!(t.ink(), Some((1, 1, 3, 3)));

    let mut t = T::new(10, 10);
    t.set("imageSmoothingEnabled", Arg::Bool(false));
    t.call("drawImage", &[img(), n(0.0), n(0.0), n(4.0), n(4.0)]);
    assert_eq!(t.px(1, 1), [255, 0, 0, 255]);
    assert_eq!(t.px(2, 1), [0, 255, 0, 255]);
    assert_eq!(t.px(1, 2), [0, 0, 255, 255]);
    let mut t = T::new(10, 10);
    t.call("drawImage", &[img(), n(0.0), n(0.0), n(4.0), n(4.0)]);
    let p = t.px(1, 1);
    assert!(p[0] < 255 && p[1] > 0, "smoothing blends: {p:?}");
    assert_eq!(t.px(0, 0), [255, 0, 0, 255], "edges clamp");

    let mut t = T::new(10, 10);
    t.set("globalAlpha", n(0.5));
    t.call("drawImage", &[img(), n(1.0), n(0.0), n(1.0), n(1.0), n(5.0), n(5.0), n(2.0), n(2.0)]);
    assert!(near4(t.px(5, 5), [0, 255, 0, 128], 1));
    assert_eq!(t.ink(), Some((5, 5, 7, 7)));
    assert!(matches!(t.call("drawImage", &[img(), n(0.0)]), Ret::Throw("TypeError", _)));
}

#[test]
fn c10_image_data() {
    let mut t = T::new(10, 10);
    t.set("fillStyle", col(10, 200, 30, 128));
    t.call("fillRect", &[n(0.0), n(0.0), n(5.0), n(5.0)]);
    let Ret::Pixels { width, height, rgba } = t.call("getImageData", &[n(-1.0), n(-1.0), n(3.0), n(3.0)]) else {
        panic!()
    };
    assert_eq!((width, height), (3, 3));
    assert_eq!(&rgba[0..4], &[0, 0, 0, 0], "outside is transparent");
    assert!(near4([rgba[16], rgba[17], rgba[18], rgba[19]], [10, 200, 30, 128], 1));
    assert!(matches!(t.call("getImageData", &[n(0.0), n(0.0), n(0.0), n(3.0)]), Ret::Throw("IndexSizeError", _)));
    let Ret::Pixels { rgba: blank, .. } = t.call("createImageData", &[n(2.0), n(3.0)]) else { panic!() };
    assert_eq!(blank.len(), 24);

    let mut t = T::new(10, 10);
    t.call("putImageData", &[img(), n(3.0), n(3.0)]);
    assert_eq!(t.px(4, 4), [255, 255, 255, 255]);
    let mut t = T::new(10, 10);
    t.set("globalAlpha", n(0.1));
    t.call("putImageData", &[img(), n(3.0), n(3.0), n(1.0), n(0.0), n(1.0), n(2.0)]);
    assert_eq!(t.ink(), Some((4, 3, 5, 5)), "only the dirty rectangle");
    assert_eq!(t.px(4, 3), [0, 255, 0, 255], "ignores globalAlpha");
    t.call("putImageData", &[img(), n(9.0), n(9.0)]);
    assert_eq!(t.px(9, 9), [255, 0, 0, 255]);
}

#[test]
fn c11_shadows() {
    let mut t = T::new(40, 40);
    t.set("shadowColor", col(255, 0, 0, 255));
    t.set("shadowOffsetX", n(5.0));
    t.set("shadowOffsetY", n(5.0));
    t.call("fillRect", &[n(5.0), n(5.0), n(10.0), n(10.0)]);
    assert_eq!(t.px(8, 8), [0, 0, 0, 255]);
    assert_eq!(t.px(17, 17), [255, 0, 0, 255]);
    assert_eq!(t.alpha(21, 21), 0);

    let mut t = T::new(80, 60);
    t.set("shadowColor", col(0, 0, 255, 255));
    t.set("shadowBlur", n(10.0));
    t.set("shadowOffsetX", n(30.0));
    t.call("fillRect", &[n(5.0), n(20.0), n(20.0), n(20.0)]);
    let row: Vec<u8> = (44..80).map(|x| t.alpha(x, 30)).collect();
    assert!(near(t.alpha(55, 30), 128, 25), "half at the edge");
    assert!(row.windows(2).all(|w| w[1] <= w[0]), "fades: {row:?}");
    assert!(row[0] > 100 && row[row.len() - 1] < 10, "{row:?}");
    assert!(t.alpha(45, 30) > 200);

    let mut t = T::new(40, 40);
    t.set("shadowColor", col(255, 0, 0, 0));
    t.set("shadowOffsetX", n(5.0));
    t.call("fillRect", &[n(5.0), n(5.0), n(10.0), n(10.0)]);
    assert_eq!(t.ink(), Some((5, 5, 15, 15)));
    // Shadows also come from text and images.
    let mut t = T::new(40, 40);
    t.set("shadowColor", col(255, 0, 0, 255));
    t.set("shadowOffsetX", n(20.0));
    t.call("drawImage", &[img(), n(1.0), n(1.0)]);
    assert_eq!(t.px(21, 1), [255, 0, 0, 255]);
}
