//! Milestone 2 (B3, B4): the from-scratch SVG rasteriser.

use invisium_gfx::draw::Rgba;
use invisium_gfx::svg::{intrinsic_size, rasterize};

const BLACK: Rgba = Rgba::BLACK;

fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

fn svg(vb: &str, body: &str) -> String {
    format!(r#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" viewBox="{vb}">{body}</svg>"#)
}

#[test]
fn filled_rect() {
    let s = svg("0 0 10 10", r#"<rect x="2" y="2" width="6" height="6" fill="red"/>"#);
    let b = rasterize(&s, 20, 20, BLACK).unwrap();
    assert_eq!(b.len(), 20 * 20 * 4);
    assert_eq!(px(&b, 20, 10, 10), [255, 0, 0, 255]);
    assert_eq!(px(&b, 20, 4, 4), [255, 0, 0, 255]);
    assert_eq!(px(&b, 20, 15, 15), [255, 0, 0, 255]);
    assert_eq!(px(&b, 20, 3, 3)[3], 0);
    assert_eq!(px(&b, 20, 16, 10)[3], 0);
    assert_eq!(intrinsic_size(&s), Some((10.0, 10.0)));
}

#[test]
fn circle_centre_filled_corners_empty() {
    let s = svg("0 0 24 24", r##"<circle cx="12" cy="12" r="10" fill="#00f"/>"##);
    let b = rasterize(&s, 24, 24, BLACK).unwrap();
    assert_eq!(px(&b, 24, 12, 12), [0, 0, 255, 255]);
    for (x, y) in [(0, 0), (23, 0), (0, 23), (23, 23), (3, 3)] {
        assert_eq!(px(&b, 24, x, y)[3], 0, "corner ({x},{y})");
    }
    // Anti-aliased rim: some partially covered pixels exist.
    assert!(b.chunks_exact(4).any(|p| p[3] > 20 && p[3] < 235));
}

#[test]
fn stroked_line_thickness() {
    let s = svg("0 0 20 20", r#"<line x1="0" y1="10" x2="20" y2="10" stroke="black" stroke-width="4"/>"#);
    let b = rasterize(&s, 20, 20, BLACK).unwrap();
    let rows: Vec<u32> = (0..20).filter(|&y| px(&b, 20, 10, y)[3] > 128).collect();
    assert_eq!(rows, vec![8, 9, 10, 11]);
    assert_eq!(px(&b, 20, 10, 9), [0, 0, 0, 255]);
    // Fill is black by default but a line has no area.
    assert_eq!(px(&b, 20, 10, 2)[3], 0);
}

#[test]
fn path_with_cubic_and_arc() {
    let s = svg("0 0 20 20", r#"<path d="M2 10 C2 2 18 2 18 10 A8 8 0 0 1 2 10 Z" fill="black"/>"#);
    let b = rasterize(&s, 20, 20, BLACK).unwrap();
    assert_eq!(px(&b, 20, 10, 10)[3], 255);
    assert_eq!(px(&b, 20, 10, 5)[3], 255); // under the cubic's peak (y = 4)
    assert_eq!(px(&b, 20, 10, 2)[3], 0);
    assert_eq!(px(&b, 20, 10, 16)[3], 255); // the arc bulges downwards to y = 18
    assert_eq!(px(&b, 20, 10, 19)[3], 0);
    // Relative commands, implicit repeats, smooth curves, and number packing.
    let s = svg("0 0 20 20", r#"<path d="m2,2h16v4l-4.5.5q-2-1-4 0t-4 0s-2 2-3.5-.5z"/>"#);
    let b = rasterize(&s, 20, 20, BLACK).unwrap();
    assert_eq!(px(&b, 20, 10, 3)[3], 255);
    assert_eq!(px(&b, 20, 10, 15)[3], 0);
}

#[test]
fn even_odd_vs_non_zero_star() {
    let star = "M10 1 L15.9 19 L0.5 7.5 H19.5 L4.1 19 Z";
    let nz = rasterize(&svg("0 0 20 20", &format!(r#"<path d="{star}"/>"#)), 20, 20, BLACK).unwrap();
    let eo = rasterize(&svg("0 0 20 20", &format!(r#"<path fill-rule="evenodd" d="{star}"/>"#)), 20, 20, BLACK).unwrap();
    assert_eq!(px(&nz, 20, 10, 10)[3], 255, "non-zero fills the centre");
    assert_eq!(px(&eo, 20, 10, 10)[3], 0, "even-odd leaves the centre empty");
    assert_eq!(px(&nz, 20, 9, 5)[3], 255);
    assert_eq!(px(&eo, 20, 9, 5)[3], 255);
    // style="" overrides the attribute.
    let eo2 = rasterize(&svg("0 0 20 20", &format!(r#"<path fill-rule="nonzero" style="fill-rule: evenodd" d="{star}"/>"#)), 20, 20, BLACK).unwrap();
    assert_eq!(eo, eo2);
}

#[test]
fn group_transform_and_inherited_fill() {
    let s = svg(
        "0 0 20 20",
        r#"<g fill="blue" transform="translate(10 0)"><g opacity="1"><rect width="5" height="5"/></g></g>
           <g transform="scale(2) rotate(90 5 5)"><rect x="0" y="0" width="2" height="2" fill="lime"/></g>"#,
    );
    let b = rasterize(&s, 20, 20, BLACK).unwrap();
    assert_eq!(px(&b, 20, 12, 2), [0, 0, 255, 255]);
    assert_eq!(px(&b, 20, 2, 2)[3], 0);
    // rotate(90 5 5) maps (0..2, 0..2) to (8..10, 0..2); scale(2) doubles it: (16..20, 0..4).
    assert_eq!(px(&b, 20, 17, 2), [0, 255, 0, 255]);
}

#[test]
fn current_color_fill_and_stroke() {
    let s = svg(
        "0 0 20 20",
        r#"<rect width="10" height="10" fill="currentColor"/>
           <line x1="10" y1="15" x2="20" y2="15" stroke="currentColor" stroke-width="2"/>"#,
    );
    let green = Rgba::rgb(0, 200, 0);
    let b = rasterize(&s, 20, 20, green).unwrap();
    assert_eq!(px(&b, 20, 5, 5), [0, 200, 0, 255]);
    assert_eq!(px(&b, 20, 15, 15), [0, 200, 0, 255]);
    // The `color` property overrides the caller's current colour.
    let s = svg("0 0 20 20", r#"<g color="red"><rect width="10" height="10" fill="currentColor"/></g>"#);
    let b = rasterize(&s, 20, 20, green).unwrap();
    assert_eq!(px(&b, 20, 5, 5), [255, 0, 0, 255]);
}

#[test]
fn view_box_into_non_square_target() {
    let s = r#"<svg viewbox="0 0 10 10"><rect width="10" height="10" fill="rgb(255,0,0)"/></svg>"#;
    let b = rasterize(s, 40, 20, BLACK).unwrap();
    assert_eq!(px(&b, 40, 20, 10), [255, 0, 0, 255]);
    assert_eq!(px(&b, 40, 10, 10), [255, 0, 0, 255]);
    assert_eq!(px(&b, 40, 29, 19), [255, 0, 0, 255]);
    assert_eq!(px(&b, 40, 5, 10)[3], 0);
    assert_eq!(px(&b, 40, 35, 10)[3], 0);
    // Upper-case and lower-case names are equivalent (HTML re-serialisation).
    let upper = r##"<SVG VIEWBOX="0 0 10 10"><RECT WIDTH="10" HEIGHT="10" FILL="#F00"/></SVG>"##;
    assert_eq!(rasterize(upper, 40, 20, BLACK).unwrap(), b);
    assert_eq!(intrinsic_size(r#"<svg width="48px" viewBox="0 0 24 12"></svg>"#), Some((48.0, 24.0)));
    assert_eq!(intrinsic_size(r#"<svg width="30" height="20"/>"#), Some((30.0, 20.0)));
    assert_eq!(intrinsic_size("<svg/>"), None);
}

const ICONS: [&str; 5] = [
    // x
    r#"<line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>"#,
    // check
    r#"<polyline points="20 6 9 17 4 12"/>"#,
    // home
    r#"<path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><polyline points="9 22 9 12 15 12 15 22"/>"#,
    // search
    r#"<circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>"#,
    // heart
    r#"<path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z"/>"#,
];

#[test]
fn real_world_icons() {
    for (i, body) in ICONS.iter().enumerate() {
        let s = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
        );
        assert_eq!(intrinsic_size(&s), Some((24.0, 24.0)));
        // 48x72 target: the viewBox maps to rows 12..60; nothing may land outside it.
        let b = rasterize(&s, 48, 72, Rgba::rgb(20, 30, 40)).unwrap();
        let mut ink = 0;
        for y in 0..72 {
            for x in 0..48 {
                let p = px(&b, 48, x, y);
                if p[3] > 0 {
                    assert!((12..60).contains(&y), "icon {i}: ink outside viewBox at ({x},{y})");
                    if p[3] >= 128 {
                        assert!(p[..3].iter().zip([20u8, 30, 40]).all(|(a, b)| a.abs_diff(b) <= 2), "icon {i}: colour {p:?}");
                    }
                    ink += 1;
                }
            }
        }
        assert!(ink > 40, "icon {i}: too little ink ({ink})");
        assert!(b.chunks_exact(4).any(|p| p[3] == 255), "icon {i}: no solid stroke pixels");
        // Small sizes still produce anti-aliased ink.
        let small = rasterize(&s, 16, 16, BLACK).unwrap();
        assert!(small.chunks_exact(4).filter(|p| p[3] > 0).count() > 10, "icon {i} at 16px");
    }
}

#[test]
fn malformed_input_never_panics() {
    let deep = format!("<svg viewBox='0 0 10 10'>{}<rect width='5' height='5'/>{}</svg>", "<g>".repeat(5000), "</g>".repeat(5000));
    let many = format!("<svg viewBox='0 0 10 10'>{}</svg>", "<rect width='1' height='1'/>".repeat(30_000));
    let huge_path = format!("<svg viewBox='0 0 10 10'><path d='M0 0{}'/></svg>", " L5 5 L0 1e3".repeat(50_000));
    let cases: Vec<String> = vec![
        String::new(),
        "<".into(),
        "<svg".into(),
        "<svg viewBox='0 0 10 10'><rect width='5' height='5'".into(),
        "<svg><g><g><path d='M0 0 L10 10 Z'>".into(),
        "<svg viewBox='0 0 10 10'><path d='M 1 1 L abc 5 Z'/><path d='M'/><path d='L 5 5'/><path d='M1e999 0 L2 2'/></svg>".into(),
        "<svg viewBox='0 0 10 10'><path d='M1e30 1e30 L-1e30 1e30 L0 -1e38 Z' stroke='red' stroke-width='1e30'/></svg>".into(),
        "<svg viewBox='0 0 10 10'><path d='M0 0 A0 0 0 0 1 5 5 A1e-30 5 0 1 1 1 1 A5 5 0 1 1 1 1'/></svg>".into(),
        "<svg viewBox='0 0 1e-30 1e-30'><circle r='5'/></svg>".into(),
        "<svg viewBox='nan nan 10 10' width='-5' height='inf'><rect width='NaN' height='5'/></svg>".into(),
        "<svg viewBox='0 0 10 10'><rect width='5' height='5' transform='matrix(1e30 0 0 1e30 0 0) rotate(nan)' fill='url(#g)'/></svg>".into(),
        "<svg viewBox='0 0 10 10'><polygon points='1 2 3'/><polyline points=''/><ellipse rx='-1' ry='3'/></svg>".into(),
        "<svg viewBox='0 0 10 10'><!-- unclosed comment <rect width='5' height='5'/>".into(),
        "<svg viewBox='0 0 10 10'><rect width='5' height='5' fill='&#xFFFFFFFF;&amp;&bogus;' style=';;:;fill:;'/></svg>".into(),
        "\u{feff}<svg viewBox='0 0 10 10'><path d='M0 0 L10 0 L10 10 Z' stroke='#zzz' stroke-width='-3'/>é€</svg>".into(),
        "<html><body>not svg</body></html>".into(),
        deep,
        many,
        huge_path,
    ];
    for c in &cases {
        let _ = intrinsic_size(c);
        for (w, h) in [(1, 1), (16, 16), (37, 5)] {
            if let Some(b) = rasterize(c, w, h, BLACK) {
                assert_eq!(b.len(), (w * h * 4) as usize);
            }
        }
    }
    // Absurd target sizes are refused rather than allocated.
    let ok = svg("0 0 10 10", "<rect width='5' height='5'/>");
    assert!(rasterize(&ok, 100_000, 100_000, BLACK).is_none());
    assert!(rasterize(&ok, 0, 10, BLACK).is_none());
    assert!(rasterize(&ok, 20_000, 20_000, BLACK).is_none());
    assert!(rasterize("<html/>", 10, 10, BLACK).is_none());
}
