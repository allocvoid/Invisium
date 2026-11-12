//! Milestone 3, part A (C2, C5): SVG gradients, `<defs>`, `<use>`, `preserveAspectRatio`.

use invisium_gfx::draw::Rgba;
use invisium_gfx::svg::rasterize;

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const MID: [u8; 4] = [128, 0, 128, 255];
const NONE: [u8; 4] = [0, 0, 0, 0];

fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

#[track_caller]
fn near(got: [u8; 4], want: [u8; 4], tol: i32, what: &str) {
    let ok = got.iter().zip(want).all(|(&g, w)| (g as i32 - w as i32).abs() <= tol);
    assert!(ok, "{what}: got {got:?}, want {want:?} (±{tol})");
}

fn render(vb: &str, body: &str, w: u32, h: u32) -> Vec<u8> {
    let s = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="{vb}">{body}</svg>"#);
    rasterize(&s, w, h, Rgba::BLACK).expect("rasterize")
}

const RB: &str = r##"<stop offset="0" stop-color="#f00"/><stop offset="100%" stop-color="#00f"/>"##;

#[test]
fn linear_horizontal_bbox() {
    let b = render("0 0 100 10", &format!(r#"<linearGradient id="g">{RB}</linearGradient><rect x="0" y="0" width="100" height="10" fill="url(#g)"/>"#), 100, 10);
    near(px(&b, 100, 0, 5), RED, 4, "start");
    near(px(&b, 100, 50, 5), MID, 6, "middle");
    near(px(&b, 100, 99, 5), BLUE, 4, "end");
}

#[test]
fn linear_diagonal_bbox() {
    // Bounding box of a rect at (20,20) 60x60: the gradient follows the box, not the canvas.
    let g = format!(r#"<linearGradient id="g" x1="0" y1="0" x2="1" y2="1">{RB}</linearGradient>"#);
    let b = render("0 0 100 100", &format!(r#"{g}<rect x="20" y="20" width="60" height="60" fill="url(#g)"/>"#), 100, 100);
    near(px(&b, 100, 20, 20), RED, 8, "top-left");
    near(px(&b, 100, 79, 79), BLUE, 8, "bottom-right");
    near(px(&b, 100, 50, 50), MID, 8, "centre");
    near(px(&b, 100, 79, 20), MID, 8, "top-right is half way");
    assert_eq!(px(&b, 100, 10, 10), NONE);
}

#[test]
fn linear_user_space() {
    let g = format!(r#"<linearGradient id="g" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="100" y2="0">{RB}</linearGradient>"#);
    let b = render("0 0 100 10", &format!(r#"{g}<rect x="50" y="0" width="50" height="10" fill="url(#g)"/>"#), 100, 10);
    assert_eq!(px(&b, 100, 40, 5), NONE);
    near(px(&b, 100, 50, 5), MID, 6, "rect start is mid-gradient");
    near(px(&b, 100, 99, 5), BLUE, 4, "end");
    // Percentages resolve against the viewport.
    let g = format!(r#"<linearGradient id="g" gradientUnits="userSpaceOnUse" x1="0%" x2="100%">{RB}</linearGradient>"#);
    let b = render("0 0 100 10", &format!(r#"{g}<rect x="50" y="0" width="50" height="10" fill="url(#g)"/>"#), 100, 10);
    near(px(&b, 100, 50, 5), MID, 6, "percent user space");
}

#[test]
fn radial_centre_vs_rim() {
    let g = format!(r#"<radialGradient id="g">{RB}</radialGradient>"#);
    let b = render("0 0 100 100", &format!(r#"{g}<rect width="100" height="100" fill="url(#g)"/>"#), 100, 100);
    near(px(&b, 100, 50, 50), RED, 6, "centre");
    near(px(&b, 100, 99, 50), BLUE, 8, "rim");
    near(px(&b, 100, 0, 0), BLUE, 0, "outside the circle pads");
    near(px(&b, 100, 75, 50), MID, 8, "half radius");
    // A focal point shifts the red towards it.
    let g = format!(r#"<radialGradient id="f" fx="0.25" fy="0.5">{RB}</radialGradient>"#);
    let b = render("0 0 100 100", &format!(r#"{g}<rect width="100" height="100" fill="url(#f)"/>"#), 100, 100);
    near(px(&b, 100, 25, 50), RED, 8, "focal point");
}

#[test]
fn gradient_transform_rotates() {
    let g = format!(r#"<linearGradient id="g" gradientTransform="rotate(90 0.5 0.5)">{RB}</linearGradient>"#);
    let b = render("0 0 100 100", &format!(r#"{g}<rect width="100" height="100" fill="url(#g)"/>"#), 100, 100);
    near(px(&b, 100, 50, 0), RED, 6, "top");
    near(px(&b, 100, 50, 99), BLUE, 6, "bottom");
    near(px(&b, 100, 0, 50), px(&b, 100, 99, 50), 2, "row is uniform");
}

#[test]
fn stop_opacity_zero_shows_background() {
    let g = r##"<linearGradient id="g"><stop offset="0" stop-color="red" stop-opacity="0"/><stop offset="1" style="stop-color: red; stop-opacity: 1"/></linearGradient>"##;
    let b = render("0 0 100 10", &format!(r#"{g}<rect width="100" height="10" fill="lime"/><rect width="100" height="10" fill="url(#g)"/>"#), 100, 10);
    near(px(&b, 100, 0, 5), [0, 255, 0, 255], 4, "background visible");
    near(px(&b, 100, 99, 5), RED, 4, "opaque end");
    // Premultiplied interpolation: red over lime half way, not a dark blend.
    near(px(&b, 100, 50, 5), [128, 127, 0, 255], 8, "middle");
}

#[test]
fn href_inherits_stops_and_attributes() {
    let defs = format!(
        r##"<linearGradient id="base" x2="0" y2="1">{RB}</linearGradient>
            <linearGradient id="a" href="#base"/>
            <linearGradient id="b" xlink:href="#a" x2="1" y2="0"/>"##
    );
    let b = render("0 0 100 100", &format!(r#"{defs}<rect width="50" height="100" fill="url(#a)"/><rect x="50" width="50" height="100" fill="url(#b)"/>"#), 100, 100);
    near(px(&b, 100, 25, 0), RED, 6, "a: vertical from base, top");
    near(px(&b, 100, 25, 99), BLUE, 6, "a: bottom");
    near(px(&b, 100, 50, 50), RED, 6, "b: horizontal override, left");
    near(px(&b, 100, 99, 50), BLUE, 6, "b: right");
}

#[test]
fn spread_methods() {
    let make = |spread: &str| {
        let g = format!(r#"<linearGradient id="g" x2="0.5" spreadMethod="{spread}">{RB}</linearGradient>"#);
        render("0 0 100 10", &format!(r#"{g}<rect width="100" height="10" fill="url(#g)"/>"#), 100, 10)
    };
    let pad = make("pad");
    near(px(&pad, 100, 74, 5), BLUE, 0, "pad");
    near(px(&pad, 100, 99, 5), BLUE, 0, "pad end");
    let reflect = make("reflect");
    near(px(&reflect, 100, 74, 5), MID, 8, "reflect middle");
    near(px(&reflect, 100, 99, 5), RED, 8, "reflect returns to red");
    let repeat = make("repeat");
    near(px(&repeat, 100, 74, 5), MID, 8, "repeat middle");
    near(px(&repeat, 100, 51, 5), RED, 12, "repeat restarts");
    near(px(&repeat, 100, 99, 5), BLUE, 8, "repeat end");
}

#[test]
fn gradient_defined_after_use_and_in_style() {
    let tail = format!(r#"<defs><linearGradient id="late">{RB}</linearGradient></defs>"#);
    let b = render("0 0 100 10", &format!(r#"<rect width="100" height="10" style="fill: url('#late')"/>{tail}"#), 100, 10);
    near(px(&b, 100, 0, 5), RED, 4, "late start");
    near(px(&b, 100, 99, 5), BLUE, 4, "late end");
    let b = render("0 0 100 10", &format!(r##"<rect width="100" height="10" fill='url("#late")'/>{tail}"##), 100, 10);
    near(px(&b, 100, 99, 5), BLUE, 4, "double-quoted id");
}

#[test]
fn gradient_on_stroke() {
    let g = format!(r#"<linearGradient id="u" gradientUnits="userSpaceOnUse" x1="0" x2="100">{RB}</linearGradient><linearGradient id="o">{RB}</linearGradient>"#);
    let b = render("0 0 100 100", &format!(r#"{g}<line x1="0" y1="50" x2="100" y2="50" stroke="url(#u)" stroke-width="10"/>"#), 100, 100);
    near(px(&b, 100, 2, 50), RED, 8, "stroke start");
    near(px(&b, 100, 97, 50), BLUE, 8, "stroke end");
    assert_eq!(px(&b, 100, 50, 30), NONE);
    // A bounding-box gradient on a zero-height line draws nothing.
    let b = render("0 0 100 100", &format!(r#"{g}<line x1="0" y1="50" x2="100" y2="50" stroke="url(#o)" stroke-width="10"/>"#), 100, 100);
    assert!(b.chunks_exact(4).all(|p| p[3] == 0));
    // On a rect's stroke the box of the geometry is used.
    let b = render("0 0 100 100", &format!(r#"{g}<rect x="10" y="10" width="80" height="80" fill="none" stroke="url(#o)" stroke-width="6"/>"#), 100, 100);
    near(px(&b, 100, 10, 50), RED, 10, "left side");
    near(px(&b, 100, 89, 50), BLUE, 10, "right side");
}

#[test]
fn missing_id_fallback() {
    let b = render("0 0 10 10", r##"<rect width="10" height="10" fill="url(#missing) #0f0"/>"##, 10, 10);
    near(px(&b, 10, 5, 5), [0, 255, 0, 255], 0, "fallback colour");
    let b = render("0 0 10 10", r##"<rect width="10" height="10" fill="url(#missing)"/>"##, 10, 10);
    assert!(b.chunks_exact(4).all(|p| p[3] == 0), "no fallback: nothing");
    let b = render("0 0 10 10", r##"<rect width="10" height="10" fill="url(#missing) none" stroke="blue"/>"##, 10, 10);
    assert_eq!(px(&b, 10, 5, 5), NONE);
    // A gradient without stops paints nothing; one stop paints a solid colour.
    let b = render("0 0 10 10", r##"<linearGradient id="e"/><rect width="10" height="10" fill="url(#e) red"/>"##, 10, 10);
    assert_eq!(px(&b, 10, 5, 5), NONE);
    let b = render("0 0 10 10", r##"<linearGradient id="s"><stop stop-color="blue"/></linearGradient><rect width="10" height="10" fill="url(#s)"/>"##, 10, 10);
    near(px(&b, 10, 5, 5), BLUE, 0, "single stop");
}

#[test]
fn defs_not_drawn() {
    let b = render("0 0 10 10", r#"<defs><rect width="10" height="10" fill="red"/><g><circle cx="5" cy="5" r="5"/></g></defs>"#, 10, 10);
    assert!(b.chunks_exact(4).all(|p| p[3] == 0));
}

#[test]
fn use_with_offset() {
    let body = r##"<defs><rect id="r" width="10" height="10"/><symbol id="sym"><circle cx="5" cy="5" r="5" fill="red"/></symbol></defs>
        <use href="#r" x="20" y="5" fill="blue"/>
        <use xlink:href="#r" x="0" y="30" style="fill: lime"/>
        <use href="#sym" x="30" y="30"/>"##;
    let b = render("0 0 50 50", body, 50, 50);
    near(px(&b, 50, 25, 10), BLUE, 0, "use with offset inherits fill");
    assert_eq!(px(&b, 50, 5, 5), NONE, "original in defs not drawn");
    near(px(&b, 50, 5, 35), [0, 255, 0, 255], 0, "xlink:href and style");
    near(px(&b, 50, 35, 35), RED, 0, "symbol through use");
}

#[test]
fn preserve_aspect_ratio() {
    // A 2:1 viewBox: left half red, right half blue, into a 100x100 square.
    let make = |par: &str| {
        let s = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100" preserveAspectRatio="{par}"><rect width="100" height="100" fill="red"/><rect x="100" width="100" height="100" fill="blue"/></svg>"#
        );
        rasterize(&s, 100, 100, Rgba::BLACK).unwrap()
    };
    let meet = make("xMidYMid meet");
    assert_eq!(px(&meet, 100, 50, 10), NONE, "letterbox top");
    assert_eq!(px(&meet, 100, 50, 90), NONE, "letterbox bottom");
    near(px(&meet, 100, 25, 50), RED, 0, "meet left");
    near(px(&meet, 100, 75, 50), BLUE, 0, "meet right");
    let slice = make("xMidYMid slice");
    assert!(slice.chunks_exact(4).all(|p| p[3] == 255), "slice covers the target");
    near(px(&slice, 100, 10, 5), RED, 0, "slice left");
    near(px(&slice, 100, 90, 95), BLUE, 0, "slice right");
    let none = make("none");
    assert!(none.chunks_exact(4).all(|p| p[3] == 255), "none stretches");
    near(px(&none, 100, 25, 5), RED, 0, "none left");
    near(px(&none, 100, 75, 95), BLUE, 0, "none right");
    let min_slice = make("xMinYMin slice");
    near(px(&min_slice, 100, 99, 50), RED, 0, "xMinYMin slice shows only the left half");
    let max_meet = make("xMaxYMax meet");
    assert_eq!(px(&max_meet, 100, 50, 40), NONE, "xMaxYMax: empty top");
    near(px(&max_meet, 100, 25, 75), RED, 0, "xMaxYMax: content at the bottom");
}

#[test]
fn shared_gradient_is_resolved_once() {
    // One gradient with 2,000 stops used by 3,000 small rects must not be quadratic.
    let mut body = String::from(r#"<linearGradient id="g">"#);
    for i in 0..2000 {
        let c = if i < 1000 { "red" } else { "blue" };
        body.push_str(&format!(r#"<stop offset="{}" stop-color="{c}"/>"#, i as f32 / 2000.0));
    }
    body.push_str("</linearGradient>");
    for i in 0..3000 {
        let (x, y) = ((i % 60) * 5, (i / 60) * 5);
        body.push_str(&format!(r#"<rect x="{x}" y="{y}" width="4" height="4" fill="url(#g)"/>"#));
    }
    let start = std::time::Instant::now();
    let b = render("0 0 300 250", &body, 300, 250);
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs_f32() < 5.0, "took {elapsed:?}");
    // Bounding-box units per rect: its left edge is red, its right edge blue.
    near(px(&b, 300, 100, 100), RED, 4, "left of a rect");
    near(px(&b, 300, 103, 100), BLUE, 4, "right of a rect");
}

#[test]
fn malformed_gradients_do_not_panic() {
    // Cyclic href, with and without stops.
    let cyc = r##"<linearGradient id="a" href="#b"/><linearGradient id="b" href="#a"/><linearGradient id="c" href="#c"><stop stop-color="red"/></linearGradient>
        <rect width="10" height="10" fill="url(#a)"/><rect width="10" height="10" fill="url(#c)"/>"##;
    let b = render("0 0 10 10", cyc, 10, 10);
    near(px(&b, 10, 5, 5), RED, 0, "self-referencing gradient keeps its own stops");
    // Thousands of stops.
    let mut many = String::from(r#"<linearGradient id="m">"#);
    for i in 0..5000 {
        let c = if i % 2 == 0 { "red" } else { "blue" };
        many.push_str(&format!(r#"<stop offset="{}" stop-color="{c}"/>"#, i as f32 / 5000.0));
    }
    many.push_str(r#"</linearGradient><rect width="100" height="10" fill="url(#m)"/>"#);
    let b = render("0 0 100 10", &many, 100, 10);
    assert!(b.chunks_exact(4).all(|p| p[3] == 255));
    // NaN and huge coordinates, absurd transforms.
    for g in [
        r#"x1="NaN" y1="inf" x2="1e38" y2="-1e38""#,
        r#"gradientTransform="scale(1e30)""#,
        r#"gradientTransform="scale(0)""#,
        r#"gradientUnits="userSpaceOnUse" x1="1e30" x2="-1e30""#,
        r#"x1="0.5" x2="0.5""#,
    ] {
        for kind in ["linearGradient", "radialGradient"] {
            let body = format!(r#"<{kind} id="g" {g} r="1e38" fx="NaN">{RB}</{kind}><rect width="10" height="10" fill="url(#g)"/>"#);
            let _ = render("0 0 10 10", &body, 10, 10);
        }
    }
    // `url(` without an end, odd ids, use cycles and fan-out.
    for fill in ["url(", "url(#g", "url(#)", "url()", "url(  'x\" ) red", "url(#g) url(#g)"] {
        let body = format!(r#"<linearGradient id="g">{RB}</linearGradient><rect width="10" height="10" fill="{fill}" stroke="{fill}"/>"#);
        let _ = render("0 0 10 10", &body, 10, 10);
    }
    let mut fan = String::from(r##"<g id="u0"><rect width="1" height="1"/></g>"##);
    for i in 1..30 {
        fan.push_str(&format!(r##"<g id="u{i}"><use href="#u{p}"/><use href="#u{p}" x="1"/><use href="#u{i}"/></g>"##, p = i - 1));
    }
    let _ = render("0 0 10 10", &fan, 10, 10);
}
