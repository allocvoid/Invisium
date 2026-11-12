//! Milestone 2 (B2): raster image decoding.

use image::{ImageFormat, RgbImage, RgbaImage};
use invisium_gfx::images::{decode, probe};
use std::io::Cursor;

fn encode(img: image::DynamicImage, fmt: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, fmt).expect("encode");
    out.into_inner()
}

fn test_rgba(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 6) as u8, (y * 12) as u8, 100, if x < 3 { 128 } else { 255 }]))
}

fn smooth_rgb(w: u32, h: u32) -> RgbImage {
    RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 4) as u8, (y * 8) as u8, 120]))
}

#[test]
fn png_round_trip_and_probe() {
    let src = test_rgba(40, 20);
    let png = encode(src.clone().into(), ImageFormat::Png);
    assert_eq!(probe(&png), Some((40, 20)));
    let d = decode(&png, 1000, 1000, 1_000_000).unwrap();
    assert_eq!((d.width, d.height, d.orig_width, d.orig_height), (40, 20, 40, 20));
    assert_eq!(d.rgba, src.into_raw());
}

#[test]
fn jpeg_round_trip_and_probe() {
    let src = smooth_rgb(32, 16);
    let jpg = encode(src.clone().into(), ImageFormat::Jpeg);
    assert_eq!(probe(&jpg), Some((32, 16)));
    let d = decode(&jpg, 64, 64, 10_000).unwrap();
    assert_eq!((d.width, d.height), (32, 16));
    assert_eq!(d.rgba.len(), 32 * 16 * 4);
    for (i, p) in src.pixels().enumerate() {
        let q = &d.rgba[i * 4..i * 4 + 4];
        for c in 0..3 {
            assert!(p.0[c].abs_diff(q[c]) <= 24, "pixel {i}: {:?} vs {:?}", p.0, q);
        }
        assert_eq!(q[3], 255);
    }
}

#[test]
fn gif_first_frame() {
    let src = RgbaImage::from_pixel(8, 6, image::Rgba([0, 0, 255, 255]));
    let gif = encode(src.into(), ImageFormat::Gif);
    assert_eq!(probe(&gif), Some((8, 6)));
    let d = decode(&gif, 100, 100, 1000).unwrap();
    assert_eq!((d.width, d.height), (8, 6));
    assert_eq!(&d.rgba[..4], &[0, 0, 255, 255]);
}

#[test]
fn downscale_to_fit_and_never_upscale() {
    let png = encode(RgbaImage::from_pixel(40, 20, image::Rgba([200, 50, 25, 255])).into(), ImageFormat::Png);
    let d = decode(&png, 10, 10, 1_000_000).unwrap();
    assert_eq!((d.width, d.height), (10, 5));
    assert_eq!((d.orig_width, d.orig_height), (40, 20));
    assert!(d.rgba.chunks_exact(4).all(|p| p == [200, 50, 25, 255]));

    let d = decode(&png, 400, 400, 1_000_000).unwrap();
    assert_eq!((d.width, d.height), (40, 20));

    let d = decode(&png, 1000, 4, 1_000_000).unwrap();
    assert_eq!((d.width, d.height), (8, 4));

    // Transparent pixels do not darken the average (premultiplied box filter).
    let mut half = RgbaImage::from_pixel(2, 1, image::Rgba([255, 0, 0, 255]));
    half.put_pixel(1, 0, image::Rgba([0, 0, 0, 0]));
    let d = decode(&encode(half.into(), ImageFormat::Png), 1, 1, 100).unwrap();
    assert_eq!(d.rgba[0], 255);
    assert!((127..=129).contains(&d.rgba[3]));
}

#[test]
fn max_pixels_rejected_before_decoding() {
    let png = encode(test_rgba(40, 20).into(), ImageFormat::Png);
    assert!(decode(&png, 100, 100, 799).is_err());
    assert!(decode(&png, 100, 100, 800).is_ok());

    // A header claiming an enormous image must be rejected without allocating for it.
    let bmp = encode(smooth_rgb(4, 4).into(), ImageFormat::Bmp);
    let mut huge = bmp.clone();
    huge[18..22].copy_from_slice(&60_000i32.to_le_bytes());
    huge[22..26].copy_from_slice(&60_000i32.to_le_bytes());
    assert!(decode(&huge, 100, 100, 10_000_000).is_err());
}

#[test]
fn garbage_and_truncated_input() {
    let png = encode(test_rgba(40, 20).into(), ImageFormat::Png);
    let jpg = encode(smooth_rgb(32, 16).into(), ImageFormat::Jpeg);
    for bytes in [&png[..png.len() / 2], &jpg[..jpg.len() / 2], &png[..20], &[][..], b"not an image at all", &[0xff, 0xd8, 0xff][..]] {
        assert!(decode(bytes, 100, 100, 1_000_000).is_err());
    }
    assert_eq!(probe(b"garbage"), None);
    assert_eq!(probe(&[]), None);
    assert!(decode(&png, 0, 10, 1_000_000).is_err());
    let mut junk = vec![0u8; 4096];
    for (i, b) in junk.iter_mut().enumerate() {
        *b = (i * 131 % 251) as u8;
    }
    junk[..8].copy_from_slice(&png[..8]); // valid PNG signature, garbage after it
    assert!(decode(&junk, 100, 100, 1_000_000).is_err());
}
