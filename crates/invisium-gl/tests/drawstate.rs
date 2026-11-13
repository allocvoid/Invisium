//! P5: each `DrawState` field from its GL state; textures, feedback, outputs, targets.

mod common;
use common::scene::*;
use common::*;
use invisium_gl::cmd::*;
use invisium_gl::*;

fn set(s: &mut Scene, method: &str, args: &[&str]) {
    let a: Vec<Arg> = args.iter().map(|x| x.parse::<f64>().map(n).unwrap_or_else(|_| s.g.k(x))).collect();
    s.g.call(method, &a);
    s.g.no_err();
}

#[test]
fn every_state_field() {
    let mut s = scene(Attributes { stencil: true, ..Attributes::default() });
    assert_eq!(s.tri().state, default_state());
    set(&mut s, "enable", &["CULL_FACE"]);
    set(&mut s, "cullFace", &["FRONT"]);
    assert_eq!(s.tri().state.cull, Some(Face::Front));
    set(&mut s, "cullFace", &["FRONT_AND_BACK"]);
    assert!(s.draw("drawArrays", &[s.g.k("TRIANGLES"), n(0.0), n(3.0)]).is_empty(), "everything culled");
    assert_eq!(s.one("drawArrays", &[s.g.k("LINES"), n(0.0), n(2.0)]).state.cull, None);
    set(&mut s, "disable", &["CULL_FACE"]);
    set(&mut s, "frontFace", &["CW"]);
    assert!(s.tri().state.front_ccw, "the GL front face, swapped");
    set(&mut s, "enable", &["BLEND"]);
    set(&mut s, "blendFuncSeparate", &["SRC_ALPHA", "ONE_MINUS_SRC_ALPHA", "ONE", "ZERO"]);
    set(&mut s, "blendEquationSeparate", &["FUNC_ADD", "MAX"]);
    let want = Blend { color_op: BlendOp::Add, alpha_op: BlendOp::Max, src_color: BlendFactor::SrcAlpha, dst_color: BlendFactor::OneMinusSrcAlpha, src_alpha: BlendFactor::One, dst_alpha: BlendFactor::Zero };
    assert_eq!(s.tri().state.blend, Some(want));
    set(&mut s, "blendColor", &["0.5", "0.25", "1", "0"]);
    assert_eq!(s.tri().state.blend_color, [0.5, 0.25, 1.0, 0.0]);
    s.g.call("colorMask", &[Arg::Bool(false), Arg::Bool(true), Arg::Bool(true), Arg::Bool(false)]);
    assert_eq!(s.tri().state.color_mask, [false, true, true, false]);
    set(&mut s, "enable", &["DEPTH_TEST"]);
    set(&mut s, "depthFunc", &["GREATER"]);
    s.g.call("depthMask", &[Arg::Bool(true)]);
    let st = s.tri().state;
    assert_eq!((st.depth_test, st.depth_write), (Some(Compare::Greater), true));
    set(&mut s, "enable", &["POLYGON_OFFSET_FILL"]);
    set(&mut s, "polygonOffset", &["2", "3"]);
    assert_eq!(s.tri().state.depth_bias, Some((2.0, 3.0)));
    set(&mut s, "enable", &["STENCIL_TEST"]);
    set(&mut s, "stencilFunc", &["EQUAL", "3", "15"]);
    set(&mut s, "stencilOpSeparate", &["BACK", "ZERO", "INCR", "REPLACE"]);
    set(&mut s, "stencilMask", &["7"]);
    let face = |f, fail, d, p| StencilFace { func: f, fail, depth_fail: d, pass: p };
    let want = Stencil {
        front: face(Compare::Equal, StencilOp::Keep, StencilOp::Keep, StencilOp::Keep),
        back: face(Compare::Equal, StencilOp::Zero, StencilOp::Incr, StencilOp::Replace),
        reference: 3,
        read_mask: 15,
        write_mask: 7,
    };
    assert_eq!(s.tri().state.stencil, Some(want));
    set(&mut s, "enable", &["SAMPLE_ALPHA_TO_COVERAGE"]);
    assert!(s.tri().state.alpha_to_coverage);
    set(&mut s, "viewport", &["1", "2", "3", "4"]);
    assert_eq!(s.tri().state.viewport, [1, 2, 3, 4]);
    set(&mut s, "depthRange", &["0.25", "0.5"]);
    assert_eq!(s.tri().state.depth_range, [0.25, 0.5]);
    set(&mut s, "enable", &["SCISSOR_TEST"]);
    set(&mut s, "scissor", &["5", "6", "7", "8"]);
    assert_eq!(s.tri().state.scissor, Some([5, 6, 7, 8]));
    set(&mut s, "enable", &["RASTERIZER_DISCARD"]);
    assert!(s.draw("drawArrays", &[s.g.k("TRIANGLES"), n(0.0), n(3.0)]).is_empty());
    // A surface without alpha, depth and stencil.
    let mut s = scene(Attributes { alpha: false, depth: false, ..Attributes::default() });
    set(&mut s, "enable", &["DEPTH_TEST"]);
    set(&mut s, "enable", &["STENCIL_TEST"]);
    let st = s.tri().state;
    assert_eq!((st.color_mask[3], st.depth_test, st.depth_write, st.stencil), (false, None, false, None));
}

#[test]
fn textures_and_samplers() {
    let mut s = scene(Attributes::default());
    let t = s.g.tex("TEXTURE_2D");
    let a = [s.g.k("TEXTURE_2D"), n(0.0), s.g.k("RGBA"), n(2.0), n(2.0), n(0.0), s.g.k("RGBA"), s.g.k("UNSIGNED_BYTE"), Arg::Null];
    s.g.call("texImage2D", &a);
    assert_eq!(s.tri().textures[0].texture, 0, "mipmap-incomplete");
    set(&mut s, "generateMipmap", &["TEXTURE_2D"]);
    let tb = s.tri().textures[0];
    assert_eq!((tb.texture, tb.sampling.max_level), (h(t), 1));
    set(&mut s, "texParameteri", &["TEXTURE_2D", "TEXTURE_MIN_FILTER", "NEAREST"]);
    set(&mut s, "texParameteri", &["TEXTURE_2D", "TEXTURE_WRAP_S", "CLAMP_TO_EDGE"]);
    let tb = s.tri().textures[0];
    assert_eq!((tb.sampling.min, tb.sampling.mip, tb.sampling.wrap[0], tb.sampling.max_level), (Filter::Nearest, None, Wrap::Clamp, 0));
    // A sampler object overrides the texture's parameters.
    let smp = s.g.obj("createSampler");
    s.g.call("bindSampler", &[n(0.0), smp]);
    s.g.call("samplerParameteri", &[smp, s.g.k("TEXTURE_MAG_FILTER"), s.g.k("NEAREST")]);
    s.g.call("samplerParameteri", &[smp, s.g.k("TEXTURE_MIN_FILTER"), s.g.k("LINEAR")]);
    s.g.call("samplerParameterf", &[smp, s.g.k("TEXTURE_MAX_LOD"), n(3.5)]);
    let tb = s.tri().textures[0];
    assert_eq!((tb.texture, tb.sampling.mag, tb.sampling.min, tb.sampling.wrap[0], tb.sampling.lod_max), (h(t), Filter::Nearest, Filter::Linear, Wrap::Repeat, 3.5));
    assert_eq!(s.g.call("getSamplerParameter", &[smp, s.g.k("TEXTURE_MAX_LOD")]), Ret::Num(3.5));
    s.g.call("samplerParameteri", &[smp, s.g.k("TEXTURE_BASE_LEVEL"), n(0.0)]);
    s.g.expect("INVALID_ENUM");
    s.g.call("bindSampler", &[n(0.0), Arg::Null]);
    // A sampler pointing at a unit without a 2D texture; a texture of the wrong kind.
    let l = s.g.loc(s.prog, "uTex");
    s.g.call("uniform1i", &[l, n(1.0)]);
    s.g.call("activeTexture", &[n(s.g.kv("TEXTURE0") + 1.0)]);
    s.g.tex("TEXTURE_CUBE_MAP");
    assert_eq!(s.tri().textures[0].texture, 0);
    s.g.call("uniform1i", &[l, n(0.0)]);
    // Feedback loop: the sampled texture is attached to the draw framebuffer.
    let fb = s.g.obj("createFramebuffer");
    s.g.call("bindFramebuffer", &[s.g.k("FRAMEBUFFER"), fb]);
    s.g.call("framebufferTexture2D", &[s.g.k("FRAMEBUFFER"), s.g.k("COLOR_ATTACHMENT0"), s.g.k("TEXTURE_2D"), t, n(0.0)]);
    s.draw("drawArrays", &[s.g.k("TRIANGLES"), n(0.0), n(3.0)]);
    s.g.expect("INVALID_OPERATION");
    // Into another texture of the FBO: a Target::Fbo draw, not drawn to the surface.
    s.g.call("activeTexture", &[s.g.k("TEXTURE0")]);
    let t2 = s.g.tex("TEXTURE_2D");
    let a = [s.g.k("TEXTURE_2D"), n(0.0), s.g.k("RGBA"), n(2.0), n(2.0), n(0.0), s.g.k("RGBA"), s.g.k("UNSIGNED_BYTE"), Arg::Null];
    s.g.call("texImage2D", &a);
    s.g.call("framebufferTexture2D", &[s.g.k("FRAMEBUFFER"), s.g.k("COLOR_ATTACHMENT0"), s.g.k("TEXTURE_2D"), t2, n(0.0)]);
    s.g.call("bindTexture", &[s.g.k("TEXTURE_2D"), t]);
    s.g.ctx.present(&mut s.g.dev);
    let d = s.tri();
    let att = Some(Attachment { texture: h(t2), level: 0, layer: 0 });
    assert_eq!(d.target, Target::Fbo { color: vec![att, None, None, None, None, None, None, None], depth_stencil: None });
    assert!(!s.g.ctx.present(&mut s.g.dev));
    // An incomplete draw framebuffer.
    s.g.call("framebufferTexture2D", &[s.g.k("FRAMEBUFFER"), s.g.k("COLOR_ATTACHMENT0"), s.g.k("TEXTURE_2D"), Arg::Null, n(0.0)]);
    s.draw("drawArrays", &[s.g.k("TRIANGLES"), n(0.0), n(3.0)]);
    s.g.expect("INVALID_FRAMEBUFFER_OPERATION");
}

#[test]
fn output_types_must_match_attachments() {
    let mut g = Gl::new(Version::WebGl2);
    let vs = "#version 300 es\nvoid main() { gl_Position = vec4(0.0); gl_PointSize = 1.0; }\n";
    let fs_u = "#version 300 es\nprecision highp float;\nout uvec4 o;\nvoid main() { o = uvec4(1u); }\n";
    let fs_f = "#version 300 es\nprecision highp float;\nout vec4 o;\nvoid main() { o = vec4(1.0); }\n";
    let pu = g.program(vs, fs_u);
    let fb = g.obj("createFramebuffer");
    g.call("bindFramebuffer", &[g.k("FRAMEBUFFER"), fb]);
    let rb_of = |g: &mut Gl, f: &str| {
        let r = g.obj("createRenderbuffer");
        g.call("bindRenderbuffer", &[g.k("RENDERBUFFER"), r]);
        g.call("renderbufferStorage", &[g.k("RENDERBUFFER"), g.k(f), n(2.0), n(2.0)]);
        g.call("framebufferRenderbuffer", &[g.k("FRAMEBUFFER"), g.k("COLOR_ATTACHMENT0"), g.k("RENDERBUFFER"), r]);
    };
    rb_of(&mut g, "RGBA8");
    g.cmds();
    let pts = g.k("POINTS");
    g.call("drawArrays", &[pts, n(0.0), n(1.0)]);
    g.expect("INVALID_OPERATION");
    rb_of(&mut g, "RGBA8UI");
    g.call("drawArrays", &[pts, n(0.0), n(1.0)]);
    g.no_err();
    assert_eq!(draws(&g.cmds()).len(), 1);
    g.program(vs, fs_f);
    g.call("drawArrays", &[pts, n(0.0), n(1.0)]);
    g.expect("INVALID_OPERATION");
    let _ = pu;
}
