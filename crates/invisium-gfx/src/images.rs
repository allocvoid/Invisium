//! Pure-CPU raster image decoding (PNG, JPEG, GIF first frame, WebP, BMP, ICO).

use std::io::Cursor;

/// A decoded image as straight-alpha RGBA8, ready for `Renderer::create_image`.
#[derive(Clone, Debug)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub orig_width: u32,
    pub orig_height: u32,
}

fn reader(bytes: &[u8]) -> Option<image::ImageReader<Cursor<&[u8]>>> {
    let r = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    r.format()?;
    Some(r)
}

/// Dimensions from the header only (cheap).
pub fn probe(bytes: &[u8]) -> Option<(u32, u32)> {
    reader(bytes)?.into_dimensions().ok()
}

/// Decode and downscale so the result fits in `max_w x max_h` (aspect preserved, never
/// upscaled). Rejects images whose decoded size would exceed `max_pixels` before allocating.
pub fn decode(bytes: &[u8], max_w: u32, max_h: u32, max_pixels: u64) -> Result<Decoded, String> {
    if max_w == 0 || max_h == 0 {
        return Err("target size is zero".into());
    }
    let (w, h) = probe(bytes).ok_or_else(|| "unrecognised or corrupt image header".to_string())?;
    if w == 0 || h == 0 {
        return Err("image has zero size".into());
    }
    if w as u64 * h as u64 > max_pixels {
        return Err(format!("image {w}x{h} exceeds the pixel limit"));
    }
    let mut r = reader(bytes).ok_or_else(|| "unrecognised image".to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(w);
    limits.max_image_height = Some(h);
    // Worst case intermediate: 16 bytes per pixel (RGBA f32), plus slack for decoder state.
    limits.max_alloc = Some((w as u64 * h as u64).saturating_mul(16).saturating_add(1 << 20));
    r.limits(limits);
    let img = r.decode().map_err(|e| format!("decode failed: {e}"))?;
    let rgba = img.into_rgba8();
    let (iw, ih) = rgba.dimensions();
    let src = rgba.into_raw();
    let (tw, th) = fit(iw, ih, max_w, max_h);
    let out = if (tw, th) == (iw, ih) { src } else { downscale(&src, iw, ih, tw, th) };
    Ok(Decoded { width: tw, height: th, rgba: out, orig_width: w, orig_height: h })
}

fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w <= max_w && h <= max_h {
        return (w, h);
    }
    let s = (max_w as f64 / w as f64).min(max_h as f64 / h as f64);
    let tw = ((w as f64 * s).round() as u32).clamp(1, max_w);
    let th = ((h as f64 * s).round() as u32).clamp(1, max_h);
    (tw, th)
}

/// Box filter over premultiplied colour, so transparent pixels do not darken edges.
fn downscale(src: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    let mut out = vec![0u8; tw as usize * th as usize * 4];
    let span = |i: u32, n: u32, t: u32| {
        let a = (i as u64 * n as u64 / t as u64) as u32;
        let b = (((i as u64 + 1) * n as u64 / t as u64) as u32).max(a + 1).min(n);
        (a, b)
    };
    for ty in 0..th {
        let (y0, y1) = span(ty, h, th);
        for tx in 0..tw {
            let (x0, x1) = span(tx, w, tw);
            let mut acc = [0u64; 4];
            for y in y0..y1 {
                let row = (y as usize * w as usize) * 4;
                for x in x0..x1 {
                    let p = &src[row + x as usize * 4..row + x as usize * 4 + 4];
                    let a = p[3] as u64;
                    acc[0] += p[0] as u64 * a;
                    acc[1] += p[1] as u64 * a;
                    acc[2] += p[2] as u64 * a;
                    acc[3] += a;
                }
            }
            let n = (y1 - y0) as u64 * (x1 - x0) as u64;
            let o = (ty as usize * tw as usize + tx as usize) * 4;
            if acc[3] > 0 {
                for c in 0..3 {
                    out[o + c] = ((acc[c] + acc[3] / 2) / acc[3]).min(255) as u8;
                }
            }
            out[o + 3] = ((acc[3] + n / 2) / n) as u8;
        }
    }
    out
}
