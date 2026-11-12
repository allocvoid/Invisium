//! CSS colour parsing: hex, rgb()/rgba(), named colours, `transparent`.

use crate::draw::Rgba;

const NAMED: &str = "aliceblue f0f8ff antiquewhite faebd7 aqua 00ffff aquamarine 7fffd4 azure f0ffff \
beige f5f5dc bisque ffe4c4 black 000000 blanchedalmond ffebcd blue 0000ff blueviolet 8a2be2 \
brown a52a2a burlywood deb887 cadetblue 5f9ea0 chartreuse 7fff00 chocolate d2691e coral ff7f50 \
cornflowerblue 6495ed cornsilk fff8dc crimson dc143c cyan 00ffff darkblue 00008b darkcyan 008b8b \
darkgoldenrod b8860b darkgray a9a9a9 darkgreen 006400 darkgrey a9a9a9 darkkhaki bdb76b \
darkmagenta 8b008b darkolivegreen 556b2f darkorange ff8c00 darkorchid 9932cc darkred 8b0000 \
darksalmon e9967a darkseagreen 8fbc8f darkslateblue 483d8b darkslategray 2f4f4f \
darkslategrey 2f4f4f darkturquoise 00ced1 darkviolet 9400d3 deeppink ff1493 deepskyblue 00bfff \
dimgray 696969 dimgrey 696969 dodgerblue 1e90ff firebrick b22222 floralwhite fffaf0 \
forestgreen 228b22 fuchsia ff00ff gainsboro dcdcdc ghostwhite f8f8ff gold ffd700 \
goldenrod daa520 gray 808080 green 008000 greenyellow adff2f grey 808080 honeydew f0fff0 \
hotpink ff69b4 indianred cd5c5c indigo 4b0082 ivory fffff0 khaki f0e68c lavender e6e6fa \
lavenderblush fff0f5 lawngreen 7cfc00 lemonchiffon fffacd lightblue add8e6 lightcoral f08080 \
lightcyan e0ffff lightgoldenrodyellow fafad2 lightgray d3d3d3 lightgreen 90ee90 lightgrey d3d3d3 \
lightpink ffb6c1 lightsalmon ffa07a lightseagreen 20b2aa lightskyblue 87cefa \
lightslategray 778899 lightslategrey 778899 lightsteelblue b0c4de lightyellow ffffe0 lime 00ff00 \
limegreen 32cd32 linen faf0e6 magenta ff00ff maroon 800000 mediumaquamarine 66cdaa \
mediumblue 0000cd mediumorchid ba55d3 mediumpurple 9370db mediumseagreen 3cb371 \
mediumslateblue 7b68ee mediumspringgreen 00fa9a mediumturquoise 48d1cc mediumvioletred c71585 \
midnightblue 191970 mintcream f5fffa mistyrose ffe4e1 moccasin ffe4b5 navajowhite ffdead \
navy 000080 oldlace fdf5e6 olive 808000 olivedrab 6b8e23 orange ffa500 orangered ff4500 \
orchid da70d6 palegoldenrod eee8aa palegreen 98fb98 paleturquoise afeeee palevioletred db7093 \
papayawhip ffefd5 peachpuff ffdab9 peru cd853f pink ffc0cb plum dda0dd powderblue b0e0e6 \
purple 800080 rebeccapurple 663399 red ff0000 rosybrown bc8f8f royalblue 4169e1 \
saddlebrown 8b4513 salmon fa8072 sandybrown f4a460 seagreen 2e8b57 seashell fff5ee \
sienna a0522d silver c0c0c0 skyblue 87ceeb slateblue 6a5acd slategray 708090 slategrey 708090 \
snow fffafa springgreen 00ff7f steelblue 4682b4 tan d2b48c teal 008080 thistle d8bfd8 \
tomato ff6347 turquoise 40e0d0 violet ee82ee wheat f5deb3 white ffffff whitesmoke f5f5f5 \
yellow ffff00 yellowgreen 9acd32";

fn named(name: &str) -> Option<Rgba> {
    let mut it = NAMED.split_ascii_whitespace();
    while let (Some(n), Some(v)) = (it.next(), it.next()) {
        if n.eq_ignore_ascii_case(name) {
            return u32::from_str_radix(v, 16).ok().map(Rgba::hex);
        }
    }
    None
}

fn hex_digit(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// Parse a CSS colour. Returns `None` for anything unrecognised.
pub fn parse(s: &str) -> Option<Rgba> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('#') {
        let b = h.as_bytes();
        let d: Option<Vec<u8>> = b.iter().map(|&c| hex_digit(c)).collect();
        let d = d?;
        return match d.len() {
            3 | 4 => {
                let a = if d.len() == 4 { d[3] * 17 } else { 255 };
                Some(Rgba::new(d[0] * 17, d[1] * 17, d[2] * 17, a))
            }
            6 | 8 => {
                let a = if d.len() == 8 { d[6] * 16 + d[7] } else { 255 };
                Some(Rgba::new(d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5], a))
            }
            _ => None,
        };
    }
    let lower = s.to_ascii_lowercase();
    if lower.contains('(') {
        return functional(&lower);
    }
    if lower == "transparent" {
        return Some(Rgba::TRANSPARENT);
    }
    named(&lower)
}

/// One component of a colour function.
#[derive(Clone, Copy)]
enum Comp {
    Num(f32),
    /// Fraction (50 % = 0.5).
    Pct(f32),
    /// An angle with a unit, in degrees.
    Deg(f32),
}

fn comp(t: &str) -> Option<Comp> {
    let num = |s: &str| s.trim().parse::<f32>().ok().filter(|v| v.is_finite());
    if t == "none" {
        return Some(Comp::Num(0.0));
    }
    if let Some(p) = t.strip_suffix('%') {
        return num(p).map(|v| Comp::Pct(v / 100.0));
    }
    for (unit, k) in [("deg", 1.0), ("grad", 0.9), ("rad", 180.0 / std::f32::consts::PI), ("turn", 360.0)] {
        if let Some(p) = t.strip_suffix(unit) {
            return num(p).map(|v| Comp::Deg(v * k));
        }
    }
    num(t).map(Comp::Num)
}

/// A number, or a percentage of `full`.
fn val(c: Comp, full: f32) -> Option<f32> {
    match c {
        Comp::Num(v) => Some(v),
        Comp::Pct(p) => Some(p * full),
        Comp::Deg(_) => None,
    }
}

fn hue(c: Comp) -> Option<f32> {
    match c {
        Comp::Num(v) | Comp::Deg(v) => Some(v),
        Comp::Pct(_) => None,
    }
}

fn hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    let (h, s, l) = (h.rem_euclid(360.0), s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
    let f = |n: f32| {
        let k = (n + h / 30.0) % 12.0;
        let a = s * l.min(1.0 - l);
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

fn mat(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

/// Linear-light sRGB to gamma-encoded sRGB.
fn encode(v: [f32; 3]) -> [f32; 3] {
    v.map(|c| {
        let a = c.abs();
        let e = if a <= 0.003_130_8 { 12.92 * a } else { 1.055 * a.powf(1.0 / 2.4) - 0.055 };
        e.copysign(c)
    })
}

fn oklab(l: f32, a: f32, b: f32) -> [f32; 3] {
    let lms = mat(&[[1.0, 0.396_337_78, 0.215_803_76], [1.0, -0.105_561_35, -0.063_854_17], [1.0, -0.089_484_18, -1.291_485_5]], [l, a, b]);
    let lms = lms.map(|v| v * v * v);
    encode(mat(
        &[[4.076_741_7, -3.307_711_6, 0.230_969_94], [-1.268_438, 2.609_757_4, -0.341_319_4], [-0.004_196_086, -0.703_418_6, 1.707_614_7]],
        lms,
    ))
}

/// CIE Lab (D50) to sRGB.
fn lab(l: f32, a: f32, b: f32) -> [f32; 3] {
    const K: f32 = 24389.0 / 27.0;
    const E: f32 = 216.0 / 24389.0;
    let fy = (l + 16.0) / 116.0;
    let (fx, fz) = (fy + a / 500.0, fy - b / 200.0);
    let inv = |f: f32| if f * f * f > E { f * f * f } else { (116.0 * f - 16.0) / K };
    let y = if l > K * E { fy * fy * fy } else { l / K };
    let xyz50 = [inv(fx) * 0.964_22, y, inv(fz) * 0.825_21];
    let xyz65 = mat(&[[0.955_473_4, -0.023_098_46, 0.063_259_24], [-0.028_369_71, 1.009_995_4, 0.021_041_44], [0.012_314_01, -0.020_507_65, 1.330_365_9]], xyz50);
    encode(mat(&[[3.240_97, -1.537_383_2, -0.498_610_76], [-0.969_243_6, 1.875_967_5, 0.041_555_06], [0.055_630_08, -0.203_976_96, 1.056_971_5]], xyz65))
}

/// `rgb()`, `hsl()`, `hwb()`, `lab()`, `lch()`, `oklab()`, `oklch()`, comma or space syntax.
fn functional(s: &str) -> Option<Rgba> {
    let open = s.find('(')?;
    let name = s[..open].trim();
    let inner = s[open + 1..].trim_end().strip_suffix(')')?;
    let (main, slash) = match inner.split_once('/') {
        Some((m, a)) => (m, Some(a)),
        None => (inner, None),
    };
    let comps: Vec<Comp> = main.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()).map(comp).collect::<Option<_>>()?;
    let alpha = match (comps.len(), slash) {
        (3, None) => Comp::Num(1.0),
        (4, None) => comps[3],
        (3, Some(a)) => comp(a.trim())?,
        _ => return None,
    };
    let alpha = val(alpha, 1.0)?;
    let [c0, c1, c2] = [comps[0], comps[1], comps[2]];
    let polar = |c: f32, h: f32| {
        let (sn, cs) = h.to_radians().sin_cos();
        (c.max(0.0) * cs, c.max(0.0) * sn)
    };
    let rgb = match name {
        "rgb" | "rgba" => [c0, c1, c2].map(|c| val(c, 255.0).map(|v| v / 255.0)),
        "hsl" | "hsla" => {
            let frac = |c: Comp| val(c, 100.0).map(|v| v / 100.0);
            hsl(hue(c0)?, frac(c1)?, frac(c2)?).map(Some)
        }
        "hwb" => {
            let frac = |c: Comp| val(c, 100.0).map(|v| (v / 100.0).clamp(0.0, 1.0));
            let (h, w, b) = (hue(c0)?, frac(c1)?, frac(c2)?);
            if w + b >= 1.0 {
                [w / (w + b); 3].map(Some)
            } else {
                hsl(h, 1.0, 0.5).map(|v| Some(v * (1.0 - w - b) + w))
            }
        }
        "lab" => lab(val(c0, 100.0)?, val(c1, 125.0)?, val(c2, 125.0)?).map(Some),
        "lch" => {
            let (a, b) = polar(val(c1, 150.0)?, hue(c2)?);
            lab(val(c0, 100.0)?, a, b).map(Some)
        }
        "oklab" => oklab(val(c0, 1.0)?, val(c1, 0.4)?, val(c2, 0.4)?).map(Some),
        "oklch" => {
            let (a, b) = polar(val(c1, 0.4)?, hue(c2)?);
            oklab(val(c0, 1.0)?, a, b).map(Some)
        }
        _ => return None,
    };
    let [r, g, b] = rgb;
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(Rgba::new(to8(r?), to8(g?), to8(b?), to8(alpha)))
}
