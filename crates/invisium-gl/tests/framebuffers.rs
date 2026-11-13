//! L6: framebuffers, clears, reads, blits, copies, present.

mod common;
use common::*;
use invisium_gl::cmd::{Attachment, ClearColor, Cmd, Target, TexFormat};
use invisium_gl::*;

fn h(a: Arg) -> u32 {
    match a {
        Arg::Object(_, h) => h,
        _ => panic!(),
    }
}

/// A renderbuffer with storage.
fn rb(g: &mut Gl, internal: &str, w: f64, hh: f64) -> Arg<'static> {
    let r = g.obj("createRenderbuffer");
    g.call("bindRenderbuffer", &[g.k("RENDERBUFFER"), r]);
    g.call("renderbufferStorage", &[g.k("RENDERBUFFER"), g.k(internal), n(w), n(hh)]);
    r
}

fn fbo(g: &mut Gl) -> Arg<'static> {
    let f = g.obj("createFramebuffer");
    g.call("bindFramebuffer", &[g.k("FRAMEBUFFER"), f]);
    f
}

fn attach_rb(g: &mut Gl, point: &str, r: Arg) {
    g.call("framebufferRenderbuffer", &[g.k("FRAMEBUFFER"), g.k(point), g.k("RENDERBUFFER"), r]);
}

fn status(g: &mut Gl) -> String {
    let Ret::Num(s) = g.call("checkFramebufferStatus", &[g.k("FRAMEBUFFER")]) else { panic!() };
    constants(g.v).iter().find(|c| c.1 as f64 == s && c.0.starts_with("FRAMEBUFFER_")).map(|c| c.0.to_string()).unwrap_or(format!("{s}"))
}

#[test]
fn completeness() {
    let mut g = Gl::new(Version::WebGl2);
    assert_eq!(status(&mut g), "FRAMEBUFFER_COMPLETE");
    fbo(&mut g);
    assert_eq!(status(&mut g), "FRAMEBUFFER_INCOMPLETE_MISSING_ATTACHMENT");
    let c = rb(&mut g, "RGBA8", 4.0, 4.0);
    attach_rb(&mut g, "COLOR_ATTACHMENT0", c);
    assert_eq!(status(&mut g), "FRAMEBUFFER_COMPLETE");
    let d = rb(&mut g, "DEPTH_COMPONENT16", 4.0, 2.0);
    attach_rb(&mut g, "DEPTH_ATTACHMENT", d);
    assert_eq!(status(&mut g), "FRAMEBUFFER_INCOMPLETE_DIMENSIONS");
    let d = rb(&mut g, "DEPTH_COMPONENT16", 4.0, 4.0);
    attach_rb(&mut g, "DEPTH_ATTACHMENT", d);
    assert_eq!(status(&mut g), "FRAMEBUFFER_COMPLETE");
    let s = rb(&mut g, "STENCIL_INDEX8", 4.0, 4.0);
    attach_rb(&mut g, "STENCIL_ATTACHMENT", s);
    assert_eq!(status(&mut g), "FRAMEBUFFER_UNSUPPORTED");
    let ds = rb(&mut g, "DEPTH24_STENCIL8", 4.0, 4.0);
    attach_rb(&mut g, "DEPTH_STENCIL_ATTACHMENT", ds);
    assert_eq!(status(&mut g), "FRAMEBUFFER_COMPLETE");
    // A depth image on a colour point, a texture without an image, an unrenderable format.
    attach_rb(&mut g, "COLOR_ATTACHMENT1", d);
    assert_eq!(status(&mut g), "FRAMEBUFFER_INCOMPLETE_ATTACHMENT");
    attach_rb(&mut g, "COLOR_ATTACHMENT1", Arg::Null);
    let t = g.tex("TEXTURE_2D");
    g.call("framebufferTexture2D", &[g.k("FRAMEBUFFER"), g.k("COLOR_ATTACHMENT1"), g.k("TEXTURE_2D"), t, n(0.0)]);
    assert_eq!(status(&mut g), "FRAMEBUFFER_INCOMPLETE_ATTACHMENT");
    let a = [g.k("TEXTURE_2D"), n(0.0), g.k("LUMINANCE"), n(4.0), n(4.0), n(0.0), g.k("LUMINANCE"), g.k("UNSIGNED_BYTE"), Arg::Null];
    g.call("texImage2D", &a);
    assert_eq!(status(&mut g), "FRAMEBUFFER_INCOMPLETE_ATTACHMENT");
    let a = [g.k("TEXTURE_2D"), n(0.0), g.k("RGBA16F"), n(4.0), n(4.0), n(0.0), g.k("RGBA"), g.k("HALF_FLOAT"), Arg::Null];
    g.call("texImage2D", &a);
    assert_eq!(status(&mut g), "FRAMEBUFFER_INCOMPLETE_ATTACHMENT");
    g.call("getExtension", &[Arg::Str("EXT_color_buffer_float")]);
    assert_eq!(status(&mut g), "FRAMEBUFFER_COMPLETE");
    // Multisample mismatch.
    let m = g.obj("createRenderbuffer");
    g.call("bindRenderbuffer", &[g.k("RENDERBUFFER"), m]);
    g.call("renderbufferStorageMultisample", &[g.k("RENDERBUFFER"), n(4.0), g.k("RGBA8"), n(4.0), n(4.0)]);
    attach_rb(&mut g, "COLOR_ATTACHMENT2", m);
    assert_eq!(status(&mut g), "FRAMEBUFFER_INCOMPLETE_MULTISAMPLE");
    g.call("renderbufferStorageMultisample", &[g.k("RENDERBUFFER"), n(4.0), g.k("RGBA8UI"), n(4.0), n(4.0)]);
    g.expect("INVALID_OPERATION");
    g.no_err();
    // Clearing an incomplete framebuffer.
    g.call("clear", &[g.k("COLOR_BUFFER_BIT")]);
    g.expect("INVALID_FRAMEBUFFER_OPERATION");
    // WebGL 1: depth and depth-stencil attached together.
    let mut g1 = Gl::new(Version::WebGl1);
    fbo(&mut g1);
    let c = rb(&mut g1, "RGBA4", 2.0, 2.0);
    attach_rb(&mut g1, "COLOR_ATTACHMENT0", c);
    let d = rb(&mut g1, "DEPTH_COMPONENT16", 2.0, 2.0);
    attach_rb(&mut g1, "DEPTH_ATTACHMENT", d);
    assert_eq!(status(&mut g1), "FRAMEBUFFER_COMPLETE");
    let ds = rb(&mut g1, "DEPTH_STENCIL", 2.0, 2.0);
    attach_rb(&mut g1, "DEPTH_STENCIL_ATTACHMENT", ds);
    assert_eq!(status(&mut g1), "FRAMEBUFFER_UNSUPPORTED");
    g1.no_err();
    let ot = g1.k("FRAMEBUFFER_ATTACHMENT_OBJECT_NAME");
    assert_eq!(g1.call("getFramebufferAttachmentParameter", &[g1.k("FRAMEBUFFER"), g1.k("DEPTH_ATTACHMENT"), ot]), Ret::Object(Kind::Renderbuffer, h(d)));
}

#[test]
fn clears() {
    let attrs = Attributes { alpha: false, ..Attributes::default() };
    let mut g = Gl::with(Version::WebGl2, attrs, 8, 8);
    g.call("clearColor", &[n(0.25), n(0.5), n(0.75), n(1.0)]);
    g.call("colorMask", &[Arg::Bool(true), Arg::Bool(false), Arg::Bool(true), Arg::Bool(true)]);
    g.call("enable", &[g.k("SCISSOR_TEST")]);
    g.call("scissor", &[n(1.0), n(2.0), n(3.0), n(4.0)]);
    g.call("stencilMask", &[n(3.0)]);
    let all = g.kv("COLOR_BUFFER_BIT") + g.kv("DEPTH_BUFFER_BIT") + g.kv("STENCIL_BUFFER_BIT");
    g.call("clear", &[n(all)]);
    g.no_err();
    let cmds = g.cmds();
    let want = Cmd::Clear {
        target: Target::Surface,
        colors: vec![(0, ClearColor::F([0.25, 0.5, 0.75, 1.0]))],
        depth: Some(1.0),
        stencil: None,
        color_mask: [true, false, true, false],
        stencil_mask: 3,
        scissor: Some([1, 2, 3, 4]),
    };
    assert_eq!(cmds.last(), Some(&want));
    g.call("clear", &[n(0x10000 as f64)]);
    g.expect("INVALID_VALUE");
    g.call("disable", &[g.k("SCISSOR_TEST")]);
    g.call("colorMask", &[Arg::Bool(true), Arg::Bool(true), Arg::Bool(true), Arg::Bool(true)]);

    // An FBO with two colour buffers (one integer) and depth-stencil; draw buffers 0 and 2.
    fbo(&mut g);
    let (c0, c2, ds) = (rb(&mut g, "RGBA8", 4.0, 4.0), rb(&mut g, "RGBA8UI", 4.0, 4.0), rb(&mut g, "DEPTH24_STENCIL8", 4.0, 4.0));
    attach_rb(&mut g, "COLOR_ATTACHMENT0", c0);
    attach_rb(&mut g, "COLOR_ATTACHMENT2", c2);
    attach_rb(&mut g, "DEPTH_STENCIL_ATTACHMENT", ds);
    let list = [g.kv("COLOR_ATTACHMENT0"), 0.0, g.kv("COLOR_ATTACHMENT2")];
    g.call("drawBuffers", &[Arg::Nums(&list)]);
    g.no_err();
    assert_eq!(g.param("DRAW_BUFFER2"), Ret::Num(g.kv("COLOR_ATTACHMENT2")));
    assert_eq!(g.param("DRAW_BUFFER1"), Ret::Num(0.0));
    g.call("drawBuffers", &[Arg::Nums(&[g.kv("COLOR_ATTACHMENT1")])]);
    g.expect("INVALID_OPERATION");
    g.call("clear", &[g.k("COLOR_BUFFER_BIT")]);
    g.expect("INVALID_OPERATION");
    g.cmds();
    let att = |r: Arg| Some(Attachment { texture: h(r), level: 0, layer: 0 });
    let target = Target::Fbo { color: vec![att(c0), None, att(c2), None, None, None, None, None], depth_stencil: att(ds) };
    let u = [1u32, 2, 3, 4].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>();
    g.call("clearBufferuiv", &[g.k("COLOR"), n(2.0), Arg::Bytes(Elem::U32, &u)]);
    g.call("clearBufferfv", &[g.k("COLOR"), n(0.0), Arg::Nums(&[0.0, 0.5, 0.0, 9.0, 1.0]), n(1.0)]);
    g.call("clearBufferfi", &[g.k("DEPTH_STENCIL"), n(0.0), n(0.5), n(7.0)]);
    g.call("clearBufferiv", &[g.k("STENCIL"), n(0.0), Arg::Nums(&[5.0])]);
    g.no_err();
    g.call("clearBufferfv", &[g.k("COLOR"), n(2.0), Arg::Nums(&[0.0; 4])]);
    g.expect("INVALID_OPERATION");
    g.call("clearBufferfv", &[g.k("COLOR"), n(0.0), Arg::Nums(&[0.0; 3])]);
    g.expect("INVALID_VALUE");
    g.call("clearBufferfv", &[g.k("DEPTH"), n(1.0), Arg::Nums(&[0.0])]);
    g.expect("INVALID_VALUE");
    let base = |colors, depth, stencil| Cmd::Clear { target: target.clone(), colors, depth, stencil, color_mask: [true; 4], stencil_mask: 3, scissor: None };
    assert_eq!(
        g.cmds(),
        vec![
            base(vec![(2, ClearColor::U([1, 2, 3, 4]))], None, None),
            base(vec![(0, ClearColor::F([0.5, 0.0, 9.0, 1.0]))], None, None),
            base(vec![], Some(0.5), Some(7)),
            base(vec![], None, Some(5)),
        ]
    );
}

#[test]
fn read_pixels() {
    let mut g = Gl::new(Version::WebGl2);
    g.ctx.resize(4, 4);
    let mut img = Vec::new();
    for y in 0..4u8 {
        for x in 0..4u8 {
            img.extend_from_slice(&[x, y, 7, 255]);
        }
    }
    g.dev.image = Some((TexFormat::Rgba8, 4, 4, img));
    g.call("pixelStorei", &[g.k("PACK_ALIGNMENT"), n(8.0)]);
    let view = [0xEEu8; 44];
    let r = g.call("readPixels", &[n(-1.0), n(2.0), n(3.0), n(3.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &view)]);
    g.no_err();
    assert_eq!(g.dev.reads.last().map(|r| (r.0.clone(), r.1, r.2)), Some((Target::Surface, 0, [0, 2, 2, 2])));
    let Ret::Fill { arg: 6, offset: 0, bytes } = r else { panic!("{r:?}") };
    let mut want = view.to_vec();
    for (row, y) in [(0usize, 2u8), (1, 3)] {
        for (col, x) in [(1usize, 0u8), (2, 1)] {
            want[row * 16 + col * 4..row * 16 + col * 4 + 4].copy_from_slice(&[x, y, 7, 255]);
        }
    }
    assert_eq!(bytes, want);
    g.call("readPixels", &[n(0.0), n(0.0), n(3.0), n(3.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[0; 43])]);
    g.expect("INVALID_OPERATION");
    g.call("readPixels", &[n(0.0), n(0.0), n(1.0), n(1.0), g.k("RGBA"), g.k("FLOAT"), Arg::Bytes(Elem::F32, &[0; 16])]);
    g.expect("INVALID_OPERATION");
    g.call("pixelStorei", &[g.k("PACK_ALIGNMENT"), n(4.0)]);

    // An Rgba16F source read as RGBA / FLOAT.
    g.call("getExtension", &[Arg::Str("EXT_color_buffer_float")]);
    fbo(&mut g);
    let f = rb(&mut g, "RGBA16F", 1.0, 1.0);
    attach_rb(&mut g, "COLOR_ATTACHMENT0", f);
    g.dev.image = Some((TexFormat::Rgba16F, 1, 1, u16_bytes(&[0x3C00, 0x3800, 0xC000, 0x0000])));
    let r = g.call("readPixels", &[n(0.0), n(0.0), n(1.0), n(1.0), g.k("RGBA"), g.k("FLOAT"), Arg::Bytes(Elem::F32, &[0; 16])]);
    g.no_err();
    assert_eq!(r, Ret::Fill { arg: 6, offset: 0, bytes: f32_bytes(&[1.0, 0.5, -2.0, 0.0]) });
    // An Rgba8U source read as RGBA_INTEGER / UNSIGNED_INT, with dstOffset.
    let u = rb(&mut g, "RGBA8UI", 1.0, 1.0);
    attach_rb(&mut g, "COLOR_ATTACHMENT0", u);
    g.dev.image = Some((TexFormat::Rgba8U, 1, 1, vec![200, 1, 2, 3]));
    let r = g.call("readPixels", &[n(0.0), n(0.0), n(1.0), n(1.0), g.k("RGBA_INTEGER"), g.k("UNSIGNED_INT"), Arg::Bytes(Elem::U32, &[0; 24]), n(2.0)]);
    g.no_err();
    let want = [200u32, 1, 2, 3].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>();
    assert_eq!(r, Ret::Fill { arg: 6, offset: 8, bytes: want });
    g.call("readPixels", &[n(0.0), n(0.0), n(1.0), n(1.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[0; 4])]);
    g.expect("INVALID_OPERATION");
}

#[test]
fn blit_into_antialiased_drawing_buffer_is_refused() {
    let mut g = Gl::new(Version::WebGl2);
    let f = g.obj("createFramebuffer");
    g.call("bindFramebuffer", &[g.k("READ_FRAMEBUFFER"), f]);
    let c = rb(&mut g, "RGBA8", 4.0, 4.0);
    g.call("framebufferRenderbuffer", &[g.k("READ_FRAMEBUFFER"), g.k("COLOR_ATTACHMENT0"), g.k("RENDERBUFFER"), c]);
    g.cmds();
    let mut args: Vec<Arg> = [0.0, 0.0, 4.0, 4.0, 0.0, 0.0, 4.0, 4.0].iter().map(|&v| n(v)).collect();
    args.extend([g.k("COLOR_BUFFER_BIT"), g.k("NEAREST")]);
    g.call("blitFramebuffer", &args);
    g.expect("INVALID_OPERATION");
    assert!(g.cmds().is_empty());
    assert!(!g.ctx.present(&mut g.dev));
}

#[test]
fn copies_into_fewer_channels_convert_on_the_cpu() {
    let mut g = Gl::new(Version::WebGl1);
    g.ctx.resize(4, 4);
    let mut img = Vec::new();
    for y in 0..4u8 {
        for x in 0..4u8 {
            img.extend_from_slice(&[10 * x + y, 50, 60, 70]);
        }
    }
    g.dev.image = Some((TexFormat::Rgba8, 4, 4, img));
    let t = g.tex("TEXTURE_2D");
    g.cmds();
    // LUMINANCE: (L, L, L, 1) from the source's red.
    g.call("copyTexImage2D", &[g.k("TEXTURE_2D"), n(0.0), g.k("LUMINANCE"), n(1.0), n(2.0), n(2.0), n(2.0), n(0.0)]);
    g.no_err();
    let cmds = g.cmds();
    assert!(!cmds.iter().any(|c| matches!(c, Cmd::CopyToTexture { .. })));
    assert_eq!(g.dev.reads.last().map(|r| r.2), Some([1, 2, 2, 2]));
    assert_eq!(writes(&cmds), vec![(h(t), 0, 0, 0, 0, 2, 2, 1, vec![12, 12, 12, 255, 22, 22, 22, 255, 13, 13, 13, 255, 23, 23, 23, 255])]);
    // RGB: alpha 1, partly outside the source.
    let a = [g.k("TEXTURE_2D"), n(0.0), g.k("RGB"), n(2.0), n(2.0), n(0.0), g.k("RGB"), g.k("UNSIGNED_BYTE"), Arg::Null];
    g.call("texImage2D", &a);
    g.cmds();
    g.call("copyTexSubImage2D", &[g.k("TEXTURE_2D"), n(0.0), n(0.0), n(0.0), n(3.0), n(3.0), n(2.0), n(2.0)]);
    g.no_err();
    let cmds = g.cmds();
    assert_eq!(writes(&cmds), vec![(h(t), 0, 0, 0, 0, 1, 1, 1, vec![33, 50, 60, 255])]);
    // Matching channels still use the device copy.
    let a = [g.k("TEXTURE_2D"), n(0.0), g.k("RGBA"), n(2.0), n(2.0), n(0.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Null];
    g.call("texImage2D", &a);
    g.cmds();
    g.call("copyTexSubImage2D", &[g.k("TEXTURE_2D"), n(0.0), n(0.0), n(0.0), n(0.0), n(0.0), n(2.0), n(2.0)]);
    let cmds = g.cmds();
    assert!(cmds.iter().any(|c| matches!(c, Cmd::CopyToTexture { .. })));
    assert!(writes(&cmds).is_empty());
}

#[test]
fn blit_copy_and_present() {
    let mut g = Gl::with(Version::WebGl2, Attributes { antialias: false, ..Attributes::default() }, 16, 16);
    assert!(!g.ctx.present(&mut g.dev));
    let f = g.obj("createFramebuffer");
    g.call("bindFramebuffer", &[g.k("READ_FRAMEBUFFER"), f]);
    let c = rb(&mut g, "RGBA8", 4.0, 4.0);
    g.call("framebufferRenderbuffer", &[g.k("READ_FRAMEBUFFER"), g.k("COLOR_ATTACHMENT0"), g.k("RENDERBUFFER"), c]);
    let cb = g.k("COLOR_BUFFER_BIT");
    let rect = [n(0.0), n(0.0), n(4.0), n(4.0), n(0.0), n(4.0), n(4.0), n(0.0)];
    let mut args = rect.to_vec();
    args.extend([cb, g.k("NEAREST")]);
    g.cmds();
    g.call("blitFramebuffer", &args);
    g.no_err();
    let att = Some(Attachment { texture: h(c), level: 0, layer: 0 });
    let src = Target::Fbo { color: vec![att, None, None, None, None, None, None, None], depth_stencil: None };
    let want = Cmd::Blit { src, src_color: 0, src_rect: [0, 0, 4, 4], dst: Target::Surface, dst_rect: [0, 4, 4, -4], color: true, depth: false, stencil: false, linear: false };
    assert_eq!(g.cmds(), vec![want]);
    args[9] = n(0x1234 as f64);
    g.call("blitFramebuffer", &args);
    g.expect("INVALID_ENUM");
    args[8] = g.k("DEPTH_BUFFER_BIT");
    args[9] = g.k("LINEAR");
    g.call("blitFramebuffer", &args);
    g.expect("INVALID_OPERATION");
    args[8] = n(0x10000 as f64);
    g.call("blitFramebuffer", &args);
    g.expect("INVALID_VALUE");
    // Same image read and drawn.
    args[8] = cb;
    args[9] = g.k("NEAREST");
    g.call("bindFramebuffer", &[g.k("DRAW_FRAMEBUFFER"), f]);
    g.call("blitFramebuffer", &args);
    g.expect("INVALID_OPERATION");
    assert!(g.cmds().is_empty());
    assert!(g.ctx.present(&mut g.dev), "the blit drew to the drawing buffer");
    assert!(!g.ctx.present(&mut g.dev));

    // copyTexSubImage2D from the drawing buffer, partly outside.
    g.call("bindFramebuffer", &[g.k("FRAMEBUFFER"), Arg::Null]);
    let t = g.tex("TEXTURE_2D");
    let a = [g.k("TEXTURE_2D"), n(0.0), g.k("RGBA"), n(4.0), n(4.0), n(0.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Null];
    g.call("texImage2D", &a);
    g.cmds();
    g.call("copyTexSubImage2D", &[g.k("TEXTURE_2D"), n(0.0), n(1.0), n(1.0), n(-1.0), n(0.0), n(2.0), n(2.0)]);
    g.no_err();
    let want = Cmd::CopyToTexture { src: Target::Surface, src_color: 0, src_x: 0, src_y: 0, dst: Attachment { texture: h(t), level: 0, layer: 0 }, dst_x: 2, dst_y: 1, width: 1, height: 2 };
    assert_eq!(g.cmds(), vec![want]);
    g.call("copyTexSubImage2D", &[g.k("TEXTURE_2D"), n(0.0), n(3.0), n(3.0), n(0.0), n(0.0), n(2.0), n(2.0)]);
    g.expect("INVALID_VALUE");
    assert!(!g.ctx.present(&mut g.dev), "copies do not draw to the drawing buffer");

    // present: true once after a clear, one Present command.
    g.call("clear", &[cb]);
    assert!(g.ctx.present(&mut g.dev));
    assert!(!g.ctx.present(&mut g.dev));
    let presents = g.cmds().iter().filter(|c| **c == Cmd::Present).count();
    assert_eq!(presents, 1);
}
