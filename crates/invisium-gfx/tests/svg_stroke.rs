//! SVG stroke dashing (`stroke-dasharray`, `stroke-dashoffset`, `pathLength`) and CSS colour
//! functions.

use invisium_gfx::draw::Rgba;
use invisium_gfx::svg::rasterize;
use std::time::Instant;

fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

fn render(vb: &str, body: &str, w: u32, h: u32) -> Vec<u8> {
    let s = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vb}">{body}</svg>"#);
    rasterize(&s, w, h, Rgba::BLACK).expect("rasterize")
}

fn ink(b: &[u8]) -> usize {
    b.chunks_exact(4).filter(|p| p[3] > 128).count()
}

fn line(dash: &str) -> Vec<u8> {
    render("0 0 100 10", &format!(r#"<line x1="0" y1="5" x2="100" y2="5" stroke="black" stroke-width="4" {dash}/>"#), 100, 10)
}

#[test]
fn dashed_line() {
    let b = line(r#"stroke-dasharray="10 10""#);
    for x in [2, 5, 8, 22, 25, 45, 85] {
        assert_eq!(px(&b, 100, x, 5)[3], 255, "dash at x={x}");
    }
    for x in [12, 15, 18, 35, 55, 95] {
        assert_eq!(px(&b, 100, x, 5)[3], 0, "gap at x={x}");
    }
    // Comma syntax, units, and an odd list repeated: "5" == "5 5".
    let b = line(r#"style="stroke-dasharray: 5px, 5px""#);
    assert_eq!(px(&b, 100, 2, 5)[3], 255);
    assert_eq!(px(&b, 100, 7, 5)[3], 0);
    let b = line(r#"stroke-dasharray="5""#);
    assert_eq!(px(&b, 100, 12, 5)[3], 255);
    assert_eq!(px(&b, 100, 17, 5)[3], 0);
    // none, negative or all-zero lists are solid.
    for d in ["none", "5 -1", "0 0", "0"] {
        let b = line(&format!(r#"stroke-dasharray="{d}""#));
        assert_eq!(ink(&b), 400, "{d} is solid");
    }
}

#[test]
fn dash_offset_shifts() {
    // Offset 5: on [0,5), off [5,15), on [15,25) ...
    let b = line(r#"stroke-dasharray="10 10" stroke-dashoffset="5""#);
    for x in [2, 17, 22, 37] {
        assert_eq!(px(&b, 100, x, 5)[3], 255, "dash at x={x}");
    }
    for x in [7, 12, 27, 32] {
        assert_eq!(px(&b, 100, x, 5)[3], 0, "gap at x={x}");
    }
    // Negative offset -5: off [0,5), on [5,15), off [15,25) ...
    let b = line(r#"stroke-dasharray="10 10" style="stroke-dashoffset: -5px""#);
    assert_eq!(px(&b, 100, 2, 5)[3], 0);
    assert_eq!(px(&b, 100, 10, 5)[3], 255);
    assert_eq!(px(&b, 100, 20, 5)[3], 0);
    // Inherited from a group.
    let b = render("0 0 100 10", r#"<g stroke-dasharray="10 10" stroke-dashoffset="5"><line x1="0" y1="5" x2="100" y2="5" stroke="black" stroke-width="4"/></g>"#, 100, 10);
    assert_eq!(px(&b, 100, 10, 5)[3], 0);
    assert_eq!(px(&b, 100, 20, 5)[3], 255);
}

fn ring(extra: &str) -> Vec<u8> {
    render("0 0 100 100", &format!(r#"<circle cx="50" cy="50" r="40" fill="none" stroke="black" stroke-width="4" pathLength="1" {extra}/>"#), 100, 100)
}

#[test]
fn path_length_scales_the_pattern() {
    // The circle starts at its right-most point and runs through the bottom first.
    let b = ring(r#"stroke-dasharray="0.5 0.5""#);
    assert_eq!(px(&b, 100, 50, 90)[3], 255, "bottom half inked");
    assert_eq!(px(&b, 100, 50, 10)[3], 0, "top half empty");
    let full = ink(&ring(""));
    let half = ink(&b);
    assert!(half * 10 > full * 4 && half * 10 < full * 6, "half the ring: {half} of {full}");

    let dots = ink(&ring(r#"stroke-dasharray="0px 1px" stroke-linecap="round""#));
    assert!(dots < 30, "0px 1px draws (almost) nothing: {dots}");
    let butt = ink(&ring(r#"stroke-dasharray="0 1""#));
    assert_eq!(butt, 0, "zero-length dashes with butt caps draw nothing");
    let whole = ring(r#"stroke-dasharray="1px 0px" stroke-linecap="round""#);
    for (x, y) in [(50, 10), (50, 90), (10, 50), (90, 50), (78, 22)] {
        assert_eq!(px(&whole, 100, x, y)[3], 255, "whole ring at ({x},{y})");
    }

    // The real-site ring.
    let s = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 56 56"><circle cx="28" cy="28" r="27" fill="none" stroke="oklch(0.42 0.15 255)" stroke-width="2" stroke-linecap="round" pathLength="1" stroke-dashoffset="0px" stroke-dasharray="1px 0px"></circle></svg>"#;
    let b = rasterize(s, 56, 56, Rgba::BLACK).unwrap();
    for (x, y) in [(28, 1), (28, 54), (1, 28), (54, 28)] {
        let p = px(&b, 56, x, y);
        assert!(p[3] > 200 && p[2] > p[0] && p[2] > p[1], "ring at ({x},{y}): {p:?}");
    }
}

#[test]
fn dashes_follow_curves() {
    let path = |extra: &str| {
        render("0 0 100 100", &format!(r#"<path d="M 10 50 C 30 0, 70 100, 90 50" fill="none" stroke="black" stroke-width="3" {extra}/>"#), 100, 100)
    };
    let solid = ink(&path(""));
    let dashed = ink(&path(r#"stroke-dasharray="4 4""#));
    assert!(solid > 100);
    assert!(dashed * 100 > solid * 35 && dashed * 100 < solid * 65, "about half: {dashed} of {solid}");
    // Closed sub-paths continue the pattern across the closing segment; each sub-path restarts.
    let b = render("0 0 100 100", r#"<path d="M10 10 H90 V90 H10 Z M 20 20 H 80" fill="none" stroke="black" stroke-width="2" stroke-dasharray="7 3"/>"#, 100, 100);
    assert!(ink(&b) > 0);
    // Closing edge (10,90)->(10,10) spans arc length 240..320: y=15.5 is at 314.5 (dash),
    // y=12.5 at 317.5 (gap).
    assert_eq!(px(&b, 100, 10, 15)[3], 255, "closing edge: dash");
    assert_eq!(px(&b, 100, 10, 12)[3], 0, "closing edge: gap");
    assert_eq!(px(&b, 100, 23, 20)[3], 255, "second sub-path restarts with a dash");
}

#[test]
fn dash_cap_returns_quickly() {
    let start = Instant::now();
    let b = render("0 0 100 10", r#"<path d="M 0 5 L 100000 5 L 0 5 L 100000 5" fill="none" stroke="black" stroke-width="2" stroke-dasharray="0.001"/>"#, 100, 10);
    assert!(start.elapsed().as_secs_f32() < 2.0, "took {:?}", start.elapsed());
    assert_eq!(px(&b, 100, 50, 5)[3], 255, "drawn solid when the cap is hit");
}

fn colour(c: &str) -> [u8; 4] {
    let b = render("0 0 4 4", &format!(r#"<g fill="lime" color="lime"><rect width="4" height="4" fill="{c}"/></g>"#), 4, 4);
    px(&b, 4, 2, 2)
}

#[track_caller]
fn near(got: [u8; 4], want: [u8; 4], tol: i32) {
    assert!(got.iter().zip(want).all(|(&g, w)| (g as i32 - w as i32).abs() <= tol), "got {got:?}, want {want:?} (±{tol})");
}

#[test]
fn colour_functions() {
    let blue = colour("oklch(0.42 0.15 255)");
    assert!(blue[2] > blue[0] && blue[2] > blue[1] && blue[2] < 220, "dark blue: {blue:?}");
    near(colour("oklch(1 0 0)"), [255, 255, 255, 255], 1);
    near(colour("oklch(0 0 0)"), [0, 0, 0, 255], 1);
    near(colour("oklch(100% 0 none)"), [255, 255, 255, 255], 1);
    near(colour("oklab(0.628 0.2249 0.1258)"), [255, 0, 0, 255], 3);
    near(colour("hsl(120 100% 25%)"), [0, 128, 0, 255], 1);
    near(colour("hsla(120, 100%, 25%, 1)"), [0, 128, 0, 255], 1);
    near(colour("hsl(0.5turn 100% 50%)"), [0, 255, 255, 255], 1);
    near(colour("hwb(0 0% 0%)"), [255, 0, 0, 255], 1);
    near(colour("hwb(0 60% 60%)"), [128, 128, 128, 255], 1);
    let half = colour("rgb(255 0 0 / 50%)");
    assert!((half[3] as i32 - 128).abs() <= 1 && half[0] > 250, "alpha: {half:?}");
    near(colour("rgb(100% 0% 0%)"), [255, 0, 0, 255], 0);
    near(colour("rgba(0, 0, 255, 1)"), [0, 0, 255, 255], 0);
    near(colour("lab(54.29% 80.81 69.89)"), [255, 0, 0, 255], 8);
    near(colour("lch(54.29 106.84 40.85deg)"), [255, 0, 0, 255], 8);
    // Garbage is not a colour: the inherited fill (lime) stays.
    for bad in ["oklch(", "oklch(a b c)", "oklch(1 2)", "hsl(1 2 3 4 5)", "foo(1 2 3)", "rgb(1 2 3 / x)", "lab(10% 1deg 2)", "oklch(0.5 0.1 10%)"] {
        near(colour(bad), [0, 255, 0, 255], 0);
    }
    // stop-color and color take the new syntax too.
    let b = render(
        "0 0 4 4",
        r#"<linearGradient id="g"><stop stop-color="oklch(0 0 0)"/></linearGradient><rect width="2" height="4" fill="url(#g)"/><rect x="2" width="2" height="4" color="hsl(240 100% 50%)" fill="currentColor"/>"#,
        4,
        4,
    );
    near(px(&b, 4, 0, 2), [0, 0, 0, 255], 1);
    near(px(&b, 4, 3, 2), [0, 0, 255, 255], 1);
}
