//! L5: texture uploads, formats and the device texture's life.

mod common;
use common::*;
use invisium_gl::cmd::{Cmd, TexFormat, TexKind};
use invisium_gl::*;

/// texImage2D of a `w` x `h` image; returns the device format and the bytes written.
#[allow(clippy::too_many_arguments)]
fn upload(g: &mut Gl, internal: &str, format: &str, ty: &str, w: u32, h: u32, e: Elem, data: &[u8]) -> (TexFormat, Vec<u8>) {
    g.tex("TEXTURE_2D");
    let args = [g.k("TEXTURE_2D"), n(0.0), g.k(internal), n(w as f64), n(h as f64), n(0.0), g.k(format), g.k(ty), Arg::Bytes(e, data)];
    g.call("texImage2D", &args);
    g.no_err();
    let cmds = g.cmds();
    let fmt = cmds.iter().rev().find_map(|c| match c {
        Cmd::TextureCreate { desc, .. } => Some(desc.format),
        _ => None,
    });
    let w = writes(&cmds);
    (fmt.expect("TextureCreate"), w.last().expect("TextureWrite").8.clone())
}

#[test]
fn format_conversions() {
    let mut g = Gl::new(Version::WebGl1);
    let u8s = Elem::U8;
    assert_eq!(upload(&mut g, "RGB", "RGB", "UNSIGNED_BYTE", 1, 1, u8s, &[10, 20, 30]), (TexFormat::Rgba8, vec![10, 20, 30, 255]));
    assert_eq!(upload(&mut g, "LUMINANCE_ALPHA", "LUMINANCE_ALPHA", "UNSIGNED_BYTE", 1, 1, u8s, &[7, 9]).1, vec![7, 7, 7, 9]);
    assert_eq!(upload(&mut g, "LUMINANCE", "LUMINANCE", "UNSIGNED_BYTE", 1, 1, u8s, &[5]).1, vec![5, 5, 5, 255]);
    assert_eq!(upload(&mut g, "ALPHA", "ALPHA", "UNSIGNED_BYTE", 1, 1, u8s, &[5]).1, vec![0, 0, 0, 5]);
    let p = u16_bytes(&[0x1234]);
    assert_eq!(upload(&mut g, "RGBA", "RGBA", "UNSIGNED_SHORT_4_4_4_4", 1, 1, Elem::U16, &p).1, vec![17, 34, 51, 68]);
    let p = u16_bytes(&[0xF800, 0x07E0]);
    assert_eq!(upload(&mut g, "RGB", "RGB", "UNSIGNED_SHORT_5_6_5", 2, 1, Elem::U16, &p).1, vec![255, 0, 0, 255, 0, 255, 0, 255]);
    let p = u16_bytes(&[0x0001, 0xF83E]);
    assert_eq!(upload(&mut g, "RGBA", "RGBA", "UNSIGNED_SHORT_5_5_5_1", 2, 1, Elem::U16, &p).1, vec![0, 0, 0, 255, 255, 0, 255, 0]);
    g.call("getExtension", &[Arg::Str("OES_texture_float")]);
    let f = f32_bytes(&[0.5, 1.0, 2.0, -1.0]);
    assert_eq!(upload(&mut g, "RGBA", "RGBA", "FLOAT", 1, 1, Elem::F32, &f), (TexFormat::Rgba32F, f.clone()));

    let mut g = Gl::new(Version::WebGl2);
    let half = u16_bytes(&[0x3C00, 0x4000, 0x0000, 0xBC00]);
    assert_eq!(upload(&mut g, "RGBA16F", "RGBA", "HALF_FLOAT", 1, 1, Elem::U16, &half), (TexFormat::Rgba16F, half.clone()));
    let f = f32_bytes(&[1.0, 2.0, 0.5, -1.0]);
    assert_eq!(upload(&mut g, "RGBA16F", "RGBA", "FLOAT", 1, 1, Elem::F32, &f).1, u16_bytes(&[0x3C00, 0x4000, 0x3800, 0xBC00]));
    assert_eq!(upload(&mut g, "RGBA32F", "RGBA", "FLOAT", 1, 1, Elem::F32, &f), (TexFormat::Rgba32F, f.clone()));
    assert_eq!(upload(&mut g, "RGB32F", "RGB", "FLOAT", 1, 1, Elem::F32, &f[..12]), (TexFormat::Rgba32F, f32_bytes(&[1.0, 2.0, 0.5, 1.0])));
    assert_eq!(upload(&mut g, "R8", "RED", "UNSIGNED_BYTE", 2, 1, Elem::U8, &[9, 8]), (TexFormat::R8, vec![9, 8]));
    assert_eq!(upload(&mut g, "RG8", "RG", "UNSIGNED_BYTE", 1, 1, Elem::U8, &[1, 2]), (TexFormat::Rg8, vec![1, 2]));
    assert_eq!(upload(&mut g, "RGB8", "RGB", "UNSIGNED_BYTE", 1, 1, Elem::U8, &[1, 2, 3]), (TexFormat::Rgba8, vec![1, 2, 3, 255]));
    assert_eq!(upload(&mut g, "SRGB8_ALPHA8", "RGBA", "UNSIGNED_BYTE", 1, 1, Elem::U8, &[1, 2, 3, 4]), (TexFormat::Srgb8A8, vec![1, 2, 3, 4]));
    assert_eq!(upload(&mut g, "RGBA8UI", "RGBA_INTEGER", "UNSIGNED_BYTE", 1, 1, Elem::U8, &[200, 2, 3, 4]), (TexFormat::Rgba8U, vec![200, 2, 3, 4]));
    let i = [-5i32, 7].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>();
    assert_eq!(upload(&mut g, "RG32I", "RG_INTEGER", "INT", 1, 1, Elem::I32, &i), (TexFormat::Rg32I, i.clone()));
    let packed = (1023u32 | (512 << 10) | (3 << 30)).to_le_bytes();
    assert_eq!(upload(&mut g, "RGB10_A2", "RGBA", "UNSIGNED_INT_2_10_10_10_REV", 1, 1, Elem::U32, &packed), (TexFormat::Rgb10A2, packed.to_vec()));
    let ones = 0x3C0u32 | (0x3C0 << 11) | (0x1E0 << 22);
    assert_eq!(upload(&mut g, "R11F_G11F_B10F", "RGB", "FLOAT", 1, 1, Elem::F32, &f32_bytes(&[1.0, 1.0, 1.0])), (TexFormat::R11G11B10F, ones.to_le_bytes().to_vec()));
    let rgb9 = (256u32 | (128 << 9) | (15 << 27)).to_le_bytes();
    assert_eq!(upload(&mut g, "RGB9_E5", "RGB", "UNSIGNED_INT_5_9_9_9_REV", 1, 1, Elem::U32, &rgb9).1, u16_bytes(&[0x3800, 0x3400, 0, 0x3C00]));
    assert_eq!(upload(&mut g, "RGBA8_SNORM", "RGBA", "BYTE", 1, 1, Elem::I8, &[0x81, 0x7F, 0, 1]), (TexFormat::Rgba8Snorm, vec![0x81, 0x7F, 0, 1]));
    // Wrong typed array for the type, invalid combinations.
    g.tex("TEXTURE_2D");
    let args = [g.k("TEXTURE_2D"), n(0.0), g.k("RGBA8"), n(1.0), n(1.0), n(0.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U16, &[0; 4])];
    g.call("texImage2D", &args);
    g.expect("INVALID_OPERATION");
    let args = [g.k("TEXTURE_2D"), n(0.0), g.k("RGBA8"), n(1.0), n(1.0), n(0.0), g.k("RGBA"), g.k("FLOAT"), Arg::Null];
    g.call("texImage2D", &args);
    g.expect("INVALID_OPERATION");
}

fn tex_image(g: &mut Gl, internal: &str, format: &str, ty: &str, w: u32, h: u32, data: Arg) {
    let args = [g.k("TEXTURE_2D"), n(0.0), g.k(internal), n(w as f64), n(h as f64), n(0.0), g.k(format), g.k(ty), data];
    g.call("texImage2D", &args);
}

#[test]
fn unpack_state() {
    let mut g = Gl::new(Version::WebGl2);
    g.tex("TEXTURE_2D");
    // Alignment 4: RGB rows of one pixel are padded to 4 bytes.
    tex_image(&mut g, "RGB", "RGB", "UNSIGNED_BYTE", 1, 2, Arg::Bytes(Elem::U8, &[1, 2, 3, 99, 4, 5, 6]));
    g.no_err();
    assert_eq!(writes(&g.cmds())[0].8, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    tex_image(&mut g, "RGB", "RGB", "UNSIGNED_BYTE", 1, 2, Arg::Bytes(Elem::U8, &[1, 2, 3, 4, 5, 6]));
    g.expect("INVALID_OPERATION");
    assert!(writes(&g.cmds()).is_empty());
    // Row length 4, skip 1 pixel and 1 row, 2x2 of R8.
    let s = |g: &mut Gl, p: &str, v: f64| {
        let k = g.k(p);
        g.call("pixelStorei", &[k, n(v)]);
    };
    s(&mut g, "UNPACK_ALIGNMENT", 1.0);
    s(&mut g, "UNPACK_ROW_LENGTH", 4.0);
    s(&mut g, "UNPACK_SKIP_PIXELS", 1.0);
    s(&mut g, "UNPACK_SKIP_ROWS", 1.0);
    let src: Vec<u8> = (0..12).collect();
    tex_image(&mut g, "R8", "RED", "UNSIGNED_BYTE", 2, 2, Arg::Bytes(Elem::U8, &src));
    g.no_err();
    assert_eq!(writes(&g.cmds())[0].8, vec![5, 6, 9, 10]);
    tex_image(&mut g, "R8", "RED", "UNSIGNED_BYTE", 2, 2, Arg::Bytes(Elem::U8, &src[..10]));
    g.expect("INVALID_OPERATION");
    for p in ["UNPACK_ROW_LENGTH", "UNPACK_SKIP_PIXELS", "UNPACK_SKIP_ROWS"] {
        s(&mut g, p, 0.0);
    }
    // FLIP_Y and PREMULTIPLY_ALPHA on a view.
    s(&mut g, "UNPACK_FLIP_Y_WEBGL", 1.0);
    tex_image(&mut g, "R8", "RED", "UNSIGNED_BYTE", 1, 2, Arg::Bytes(Elem::U8, &[1, 2]));
    assert_eq!(writes(&g.cmds())[0].8, vec![2, 1]);
    s(&mut g, "UNPACK_FLIP_Y_WEBGL", 0.0);
    s(&mut g, "UNPACK_PREMULTIPLY_ALPHA_WEBGL", 1.0);
    tex_image(&mut g, "RGBA", "RGBA", "UNSIGNED_BYTE", 1, 1, Arg::Bytes(Elem::U8, &[200, 100, 50, 128]));
    assert_eq!(writes(&g.cmds())[0].8, vec![100, 50, 25, 128]);
    // DOM sources: top row first = row 0; FLIP_Y reverses; premultiply; conversion to the type.
    let rgba = [255, 0, 0, 255, 0, 0, 255, 128];
    let px = Arg::Pixels { width: 1, height: 2, rgba: &rgba };
    s(&mut g, "UNPACK_PREMULTIPLY_ALPHA_WEBGL", 0.0);
    g.call("texImage2D", &[g.k("TEXTURE_2D"), n(0.0), g.k("RGBA"), g.k("RGBA"), g.k("UNSIGNED_BYTE"), px]);
    g.no_err();
    assert_eq!(writes(&g.cmds())[0].8, rgba.to_vec());
    s(&mut g, "UNPACK_FLIP_Y_WEBGL", 1.0);
    s(&mut g, "UNPACK_PREMULTIPLY_ALPHA_WEBGL", 1.0);
    g.call("texImage2D", &[g.k("TEXTURE_2D"), n(0.0), g.k("RGBA"), g.k("RGBA"), g.k("UNSIGNED_BYTE"), px]);
    assert_eq!(writes(&g.cmds())[0].8, vec![0, 0, 128, 128, 255, 0, 0, 255]);
    s(&mut g, "UNPACK_FLIP_Y_WEBGL", 0.0);
    s(&mut g, "UNPACK_PREMULTIPLY_ALPHA_WEBGL", 0.0);
    let grey = [100, 100, 100, 255];
    g.call("texImage2D", &[g.k("TEXTURE_2D"), n(0.0), g.k("RGB"), g.k("RGB"), g.k("UNSIGNED_SHORT_5_6_5"), Arg::Pixels { width: 1, height: 1, rgba: &grey }]);
    assert_eq!(writes(&g.cmds())[0].8, vec![99, 101, 99, 255]);
    g.call("texImage2D", &[g.k("TEXTURE_2D"), n(0.0), g.k("LUMINANCE"), g.k("LUMINANCE"), g.k("UNSIGNED_BYTE"), Arg::Pixels { width: 1, height: 1, rgba: &[7, 8, 9, 10] }]);
    assert_eq!(writes(&g.cmds())[0].8, vec![7, 7, 7, 255]);
    g.no_err();
}

#[test]
fn sub_images_storage_mips_redefinition() {
    let mut g = Gl::new(Version::WebGl2);
    let t = g.tex("TEXTURE_2D");
    let Arg::Object(_, id) = t else { panic!() };
    tex_image(&mut g, "RGBA", "RGBA", "UNSIGNED_BYTE", 4, 4, Arg::Null);
    let cmds = g.cmds();
    assert!(matches!(cmds[..], [Cmd::Configure { .. }, Cmd::TextureCreate { id: i, desc }] if i == id && desc.mips == 1 && desc.width == 4));
    let args = [g.k("TEXTURE_2D"), n(0.0), n(1.0), n(2.0), n(2.0), n(1.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[1, 2, 3, 4, 5, 6, 7, 8])];
    g.call("texSubImage2D", &args);
    g.no_err();
    assert_eq!(writes(&g.cmds()), vec![(id, 0, 0, 1, 2, 2, 1, 1, vec![1, 2, 3, 4, 5, 6, 7, 8])]);
    let args = [g.k("TEXTURE_2D"), n(0.0), n(3.0), n(3.0), n(2.0), n(1.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[0; 8])];
    g.call("texSubImage2D", &args);
    g.expect("INVALID_VALUE");
    let args = [g.k("TEXTURE_2D"), n(0.0), n(0.0), n(0.0), n(2.0), n(2.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[0; 15])];
    g.call("texSubImage2D", &args);
    g.expect("INVALID_OPERATION");
    // Level 1 grows the device texture; a level of the wrong size is dropped.
    let a = [g.k("TEXTURE_2D"), n(1.0), g.k("RGBA"), n(2.0), n(2.0), n(0.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Null];
    g.call("texImage2D", &a);
    assert!(g.cmds().contains(&Cmd::TextureGrow { id, mips: 2 }));
    let a = [g.k("TEXTURE_2D"), n(2.0), g.k("RGBA"), n(3.0), n(3.0), n(0.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[0; 36])];
    g.call("texImage2D", &a);
    g.no_err();
    assert!(g.cmds().is_empty());
    // generateMipmap: all levels.
    g.call("generateMipmap", &[g.k("TEXTURE_2D")]);
    g.no_err();
    assert_eq!(g.cmds(), vec![Cmd::TextureGrow { id, mips: 3 }, Cmd::GenerateMips { id }]);
    // Same size: no new device texture; another size: a new one under the same id.
    tex_image(&mut g, "RGBA", "RGBA", "UNSIGNED_BYTE", 4, 4, Arg::Null);
    assert!(!g.cmds().iter().any(|c| matches!(c, Cmd::TextureCreate { .. })));
    tex_image(&mut g, "RGBA", "RGBA", "UNSIGNED_BYTE", 8, 8, Arg::Null);
    let cmds = g.cmds();
    assert!(matches!(cmds[..], [Cmd::TextureCreate { id: i, desc }] if i == id && desc.width == 8 && desc.mips == 1));
    // texStorage2D: immutable.
    let t2 = g.tex("TEXTURE_2D");
    let Arg::Object(_, id2) = t2 else { panic!() };
    g.call("texStorage2D", &[g.k("TEXTURE_2D"), n(3.0), g.k("RGBA8"), n(4.0), n(4.0)]);
    g.no_err();
    assert!(matches!(g.cmds()[..], [Cmd::TextureCreate { id: i, desc }] if i == id2 && desc.mips == 3 && desc.format == TexFormat::Rgba8));
    tex_image(&mut g, "RGBA", "RGBA", "UNSIGNED_BYTE", 4, 4, Arg::Null);
    g.expect("INVALID_OPERATION");
    g.call("texStorage2D", &[g.k("TEXTURE_2D"), n(1.0), g.k("RGBA8"), n(4.0), n(4.0)]);
    g.expect("INVALID_OPERATION");
    g.call("texStorage2D", &[g.k("TEXTURE_2D"), n(1.0), g.k("RGBA"), n(4.0), n(4.0)]);
    g.expect("INVALID_ENUM");
    assert_eq!(g.call("getTexParameter", &[g.k("TEXTURE_2D"), g.k("TEXTURE_IMMUTABLE_FORMAT")]), Ret::Bool(true));
    assert_eq!(g.call("getTexParameter", &[g.k("TEXTURE_2D"), g.k("TEXTURE_IMMUTABLE_LEVELS")]), Ret::Num(3.0));
    let a = [g.k("TEXTURE_2D"), n(2.0), n(0.0), n(0.0), n(1.0), n(1.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[9; 4])];
    g.call("texSubImage2D", &a);
    g.no_err();
    assert_eq!(writes(&g.cmds()), vec![(id2, 2, 0, 0, 0, 1, 1, 1, vec![9; 4])]);
    g.call("texStorage2D", &[g.k("TEXTURE_2D"), n(4.0), g.k("RGBA8"), n(4.0), n(4.0)]);
    g.expect("INVALID_OPERATION");
    // Texture parameters.
    g.call("texParameteri", &[g.k("TEXTURE_2D"), g.k("TEXTURE_MIN_FILTER"), g.k("NEAREST")]);
    assert_eq!(g.call("getTexParameter", &[g.k("TEXTURE_2D"), g.k("TEXTURE_MIN_FILTER")]), Ret::Num(g.kv("NEAREST")));
    g.call("texParameteri", &[g.k("TEXTURE_2D"), g.k("TEXTURE_WRAP_S"), g.k("NEAREST")]);
    g.expect("INVALID_ENUM");
}

#[test]
fn cube_3d_and_array() {
    let mut g = Gl::new(Version::WebGl2);
    let Arg::Object(_, cube) = g.tex("TEXTURE_CUBE_MAP") else { panic!() };
    for f in 0..6 {
        let t = n(g.kv("TEXTURE_CUBE_MAP_POSITIVE_X") + f as f64);
        let a = [t, n(0.0), g.k("RGBA"), n(2.0), n(2.0), n(0.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[f as u8; 16])];
        g.call("texImage2D", &a);
    }
    g.no_err();
    let cmds = g.cmds();
    let creates: Vec<_> = cmds.iter().filter_map(|c| if let Cmd::TextureCreate { desc, .. } = c { Some(*desc) } else { None }).collect();
    assert_eq!(creates.len(), 1);
    assert_eq!((creates[0].kind, creates[0].depth), (TexKind::Cube, 1));
    let w = writes(&cmds);
    assert_eq!(w.iter().map(|x| (x.0, x.2, x.8[0])).collect::<Vec<_>>(), (0..6).map(|f| (cube, f, f as u8)).collect::<Vec<_>>());
    // A face of another size is kept off the device (the cube is incomplete).
    let t = n(g.kv("TEXTURE_CUBE_MAP_NEGATIVE_Z"));
    let a = [t, n(0.0), g.k("RGBA"), n(4.0), n(4.0), n(0.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[0; 64])];
    g.call("texImage2D", &a);
    g.no_err();
    assert!(g.cmds().is_empty());
    let a = [g.k("TEXTURE_CUBE_MAP_POSITIVE_X"), n(0.0), g.k("RGBA"), n(2.0), n(4.0), n(0.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Null];
    g.call("texImage2D", &a);
    g.expect("INVALID_VALUE");

    // 3D: one write with depth 2; sub-image into slice 1.
    let Arg::Object(_, t3) = g.tex("TEXTURE_3D") else { panic!() };
    let data: Vec<u8> = (0..32).collect();
    let a = [g.k("TEXTURE_3D"), n(0.0), g.k("RGBA8"), n(2.0), n(2.0), n(2.0), n(0.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &data)];
    g.call("texImage3D", &a);
    g.no_err();
    let cmds = g.cmds();
    assert!(cmds.iter().any(|c| matches!(c, Cmd::TextureCreate { id, desc } if *id == t3 && desc.kind == TexKind::D3 && desc.depth == 2)));
    assert_eq!(writes(&cmds), vec![(t3, 0, 0, 0, 0, 2, 2, 2, data.clone())]);
    let a = [g.k("TEXTURE_3D"), n(0.0), n(1.0), n(0.0), n(1.0), n(1.0), n(1.0), n(1.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[7; 4])];
    g.call("texSubImage3D", &a);
    g.no_err();
    assert_eq!(writes(&g.cmds()), vec![(t3, 0, 1, 1, 0, 1, 1, 1, vec![7; 4])]);
    // FLIP_Y with a view is refused for 3D uploads.
    g.call("pixelStorei", &[g.k("UNPACK_FLIP_Y_WEBGL"), n(1.0)]);
    g.call("texSubImage3D", &a);
    g.expect("INVALID_OPERATION");
    g.call("pixelStorei", &[g.k("UNPACK_FLIP_Y_WEBGL"), n(0.0)]);

    // 2D array with 3 layers, image height and skip images.
    let Arg::Object(_, ta) = g.tex("TEXTURE_2D_ARRAY") else { panic!() };
    g.call("pixelStorei", &[g.k("UNPACK_IMAGE_HEIGHT"), n(2.0)]);
    g.call("pixelStorei", &[g.k("UNPACK_SKIP_IMAGES"), n(1.0)]);
    let src: Vec<u8> = (0..8).collect();
    let a = [g.k("TEXTURE_2D_ARRAY"), n(0.0), g.k("R8"), n(1.0), n(1.0), n(3.0), n(0.0), g.k("RED"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &src)];
    g.call("pixelStorei", &[g.k("UNPACK_ALIGNMENT"), n(1.0)]);
    g.call("texImage3D", &a);
    g.no_err();
    let cmds = g.cmds();
    assert!(cmds.iter().any(|c| matches!(c, Cmd::TextureCreate { id, desc } if *id == ta && desc.kind == TexKind::Array && desc.depth == 3)));
    assert_eq!(writes(&cmds), vec![(ta, 0, 0, 0, 0, 1, 1, 3, vec![2, 4, 6])]);
}
