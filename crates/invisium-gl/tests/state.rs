//! L3: state setters, initial values, errors.

mod common;
use common::*;
use invisium_gl::*;

fn num(r: Ret) -> f64 {
    match r {
        Ret::Num(n) => n,
        r => panic!("not a number: {r:?}"),
    }
}

#[test]
fn initial_values() {
    let mut g = Gl::with(Version::WebGl2, Attributes::default(), 300, 150);
    assert_eq!(g.param("VIEWPORT"), Ret::I32s(vec![0, 0, 300, 150]));
    assert_eq!(g.param("SCISSOR_BOX"), Ret::I32s(vec![0, 0, 300, 150]));
    assert_eq!(g.param("COLOR_CLEAR_VALUE"), Ret::F32s(vec![0.0; 4]));
    assert_eq!(g.param("COLOR_WRITEMASK"), Ret::Bools(vec![true; 4]));
    assert_eq!(g.param("DEPTH_CLEAR_VALUE"), Ret::Num(1.0));
    assert_eq!(g.param("DEPTH_RANGE"), Ret::F32s(vec![0.0, 1.0]));
    assert_eq!(num(g.param("DEPTH_FUNC")), g.kv("LESS"));
    assert_eq!(num(g.param("BLEND_SRC_RGB")), 1.0);
    assert_eq!(num(g.param("BLEND_DST_ALPHA")), 0.0);
    assert_eq!(num(g.param("BLEND_EQUATION_RGB")), g.kv("FUNC_ADD"));
    assert_eq!(num(g.param("CULL_FACE_MODE")), g.kv("BACK"));
    assert_eq!(num(g.param("FRONT_FACE")), g.kv("CCW"));
    assert_eq!(num(g.param("STENCIL_WRITEMASK")), 4294967295.0);
    assert_eq!(num(g.param("STENCIL_FUNC")), g.kv("ALWAYS"));
    assert_eq!(num(g.param("UNPACK_ALIGNMENT")), 4.0);
    assert_eq!(num(g.param("ACTIVE_TEXTURE")), g.kv("TEXTURE0"));
    assert_eq!(num(g.param("UNPACK_COLORSPACE_CONVERSION_WEBGL")), g.kv("BROWSER_DEFAULT_WEBGL"));
    assert_eq!(g.param("UNPACK_FLIP_Y_WEBGL"), Ret::Bool(false));
    assert_eq!(g.param("ARRAY_BUFFER_BINDING"), Ret::Null);
    assert_eq!(g.param("FRAMEBUFFER_BINDING"), Ret::Null);
    for cap in ["BLEND", "CULL_FACE", "DEPTH_TEST", "SCISSOR_TEST", "STENCIL_TEST", "RASTERIZER_DISCARD"] {
        let k = g.k(cap);
        assert_eq!(g.call("isEnabled", &[k]), Ret::Bool(false), "{cap}");
    }
    let k = g.k("DITHER");
    assert_eq!(g.call("isEnabled", &[k]), Ret::Bool(true));
    // Fixed answers.
    assert_eq!(g.param("VENDOR"), Ret::Str("Mozilla".into()));
    assert_eq!(g.param("RENDERER"), Ret::Str("Mozilla".into()));
    assert_eq!(g.param("VERSION"), Ret::Str("WebGL 2.0".into()));
    assert_eq!(g.param("SHADING_LANGUAGE_VERSION"), Ret::Str("WebGL GLSL ES 3.00".into()));
    assert_eq!(g.param("MAX_TEXTURE_SIZE"), Ret::Num(8192.0));
    assert_eq!(g.param("MAX_VIEWPORT_DIMS"), Ret::I32s(vec![8192, 8192]));
    assert_eq!(g.param("MAX_SAMPLES"), Ret::Num(4.0));
    assert_eq!(g.param("ALIASED_LINE_WIDTH_RANGE"), Ret::F32s(vec![1.0, 1.0]));
    assert_eq!(g.param("DEPTH_BITS"), Ret::Num(24.0));
    assert_eq!(g.param("STENCIL_BITS"), Ret::Num(0.0));
    g.no_err();
    let mut g1 = Gl::new(Version::WebGl1);
    assert_eq!(g1.param("VERSION"), Ret::Str("WebGL 1.0".into()));
    assert_eq!(g1.param("SHADING_LANGUAGE_VERSION"), Ret::Str("WebGL GLSL ES 1.0".into()));
    g1.call("getParameter", &[n(0x8073 as f64)]);
    g1.expect("INVALID_ENUM");
    let (v, f) = (g.k("VERTEX_SHADER"), g.k("HIGH_FLOAT"));
    assert_eq!(g.call("getShaderPrecisionFormat", &[v, f]), Ret::Precision { range_min: 127, range_max: 127, precision: 23 });
}

#[test]
fn setters_reflected() {
    let mut g = Gl::new(Version::WebGl2);
    let caps = ["BLEND", "CULL_FACE", "DEPTH_TEST", "DITHER", "POLYGON_OFFSET_FILL", "SAMPLE_ALPHA_TO_COVERAGE", "SAMPLE_COVERAGE", "SCISSOR_TEST", "STENCIL_TEST", "RASTERIZER_DISCARD"];
    for cap in caps {
        let k = g.k(cap);
        g.call("enable", &[k]);
        assert_eq!(g.call("isEnabled", &[k]), Ret::Bool(true), "{cap}");
        assert_eq!(g.param(cap), Ret::Bool(true), "{cap}");
        g.call("disable", &[k]);
        assert_eq!(g.call("isEnabled", &[k]), Ret::Bool(false), "{cap}");
    }
    g.call("blendColor", &[n(0.1), n(0.2), n(0.3), n(0.4)]);
    assert_eq!(g.param("BLEND_COLOR"), Ret::F32s(vec![0.1, 0.2, 0.3, 0.4]));
    let (s, a) = (g.k("FUNC_SUBTRACT"), g.k("MAX"));
    g.call("blendEquationSeparate", &[s, a]);
    assert_eq!(num(g.param("BLEND_EQUATION_ALPHA")), g.kv("MAX"));
    let f: Vec<_> = ["SRC_ALPHA", "ONE_MINUS_SRC_ALPHA", "ONE", "DST_ALPHA"].iter().map(|x| g.k(x)).collect();
    g.call("blendFuncSeparate", &f);
    assert_eq!(num(g.param("BLEND_DST_RGB")), g.kv("ONE_MINUS_SRC_ALPHA"));
    assert_eq!(num(g.param("BLEND_DST_ALPHA")), g.kv("DST_ALPHA"));
    g.call("clearColor", &[n(0.5), n(0.25), n(1.0), n(0.0)]);
    assert_eq!(g.param("COLOR_CLEAR_VALUE"), Ret::F32s(vec![0.5, 0.25, 1.0, 0.0]));
    g.call("clearDepth", &[n(0.5)]);
    assert_eq!(g.param("DEPTH_CLEAR_VALUE"), Ret::Num(0.5));
    g.call("clearStencil", &[n(7.0)]);
    assert_eq!(g.param("STENCIL_CLEAR_VALUE"), Ret::Num(7.0));
    g.call("colorMask", &[Arg::Bool(true), Arg::Bool(false), Arg::Bool(true), Arg::Bool(false)]);
    assert_eq!(g.param("COLOR_WRITEMASK"), Ret::Bools(vec![true, false, true, false]));
    let ff = g.k("FRONT");
    g.call("cullFace", &[ff]);
    assert_eq!(num(g.param("CULL_FACE_MODE")), g.kv("FRONT"));
    let gq = g.k("GEQUAL");
    g.call("depthFunc", &[gq]);
    assert_eq!(num(g.param("DEPTH_FUNC")), g.kv("GEQUAL"));
    g.call("depthMask", &[Arg::Bool(false)]);
    assert_eq!(g.param("DEPTH_WRITEMASK"), Ret::Bool(false));
    g.call("depthRange", &[n(0.25), n(0.75)]);
    assert_eq!(g.param("DEPTH_RANGE"), Ret::F32s(vec![0.25, 0.75]));
    let cw = g.k("CW");
    g.call("frontFace", &[cw]);
    assert_eq!(num(g.param("FRONT_FACE")), g.kv("CW"));
    let (h, ni) = (g.k("GENERATE_MIPMAP_HINT"), g.k("NICEST"));
    g.call("hint", &[h, ni]);
    assert_eq!(num(g.param("GENERATE_MIPMAP_HINT")), g.kv("NICEST"));
    g.call("lineWidth", &[n(3.0)]);
    assert_eq!(g.param("LINE_WIDTH"), Ret::Num(3.0));
    let (pa, one) = (g.k("UNPACK_ALIGNMENT"), n(1.0));
    g.call("pixelStorei", &[pa, one]);
    assert_eq!(g.param("UNPACK_ALIGNMENT"), Ret::Num(1.0));
    let rl = g.k("UNPACK_ROW_LENGTH");
    g.call("pixelStorei", &[rl, n(12.0)]);
    assert_eq!(g.param("UNPACK_ROW_LENGTH"), Ret::Num(12.0));
    g.call("polygonOffset", &[n(2.0), n(3.0)]);
    assert_eq!(g.param("POLYGON_OFFSET_FACTOR"), Ret::Num(2.0));
    assert_eq!(g.param("POLYGON_OFFSET_UNITS"), Ret::Num(3.0));
    g.call("sampleCoverage", &[n(0.5), Arg::Bool(true)]);
    assert_eq!(g.param("SAMPLE_COVERAGE_VALUE"), Ret::Num(0.5));
    assert_eq!(g.param("SAMPLE_COVERAGE_INVERT"), Ret::Bool(true));
    g.call("scissor", &[n(1.0), n(2.0), n(3.0), n(4.0)]);
    assert_eq!(g.param("SCISSOR_BOX"), Ret::I32s(vec![1, 2, 3, 4]));
    g.call("viewport", &[n(-1.0), n(2.0), n(30.0), n(40.0)]);
    assert_eq!(g.param("VIEWPORT"), Ret::I32s(vec![-1, 2, 30, 40]));
    let (back, eq) = (g.k("BACK"), g.k("EQUAL"));
    g.call("stencilFuncSeparate", &[back, eq, n(3.0), n(255.0)]);
    assert_eq!(num(g.param("STENCIL_BACK_FUNC")), g.kv("EQUAL"));
    assert_eq!(num(g.param("STENCIL_BACK_REF")), 3.0);
    assert_eq!(num(g.param("STENCIL_FUNC")), g.kv("ALWAYS"));
    g.call("stencilMask", &[n(15.0)]);
    assert_eq!(num(g.param("STENCIL_BACK_WRITEMASK")), 15.0);
    g.call("stencilMaskSeparate", &[back, n(7.0)]);
    assert_eq!(num(g.param("STENCIL_BACK_WRITEMASK")), 7.0);
    assert_eq!(num(g.param("STENCIL_WRITEMASK")), 15.0);
    let ops: Vec<_> = ["INCR", "DECR_WRAP", "INVERT"].iter().map(|x| g.k(x)).collect();
    g.call("stencilOp", &ops);
    assert_eq!(num(g.param("STENCIL_PASS_DEPTH_PASS")), g.kv("INVERT"));
    assert_eq!(num(g.param("STENCIL_BACK_FAIL")), g.kv("INCR"));
    g.call("stencilOpSeparate", &[back, g.k("ZERO"), g.k("KEEP"), g.k("REPLACE")]);
    assert_eq!(num(g.param("STENCIL_BACK_FAIL")), 0.0);
    g.call("stencilFunc", &[g.k("NEVER"), n(1.0), n(2.0)]);
    assert_eq!(num(g.param("STENCIL_VALUE_MASK")), 2.0);
    let t5 = Arg::Num(g.kv("TEXTURE0") + 5.0);
    g.call("activeTexture", &[t5]);
    assert_eq!(num(g.param("ACTIVE_TEXTURE")), g.kv("TEXTURE0") + 5.0);
    g.no_err();
}

#[test]
fn invalid_calls_change_nothing() {
    let mut g = Gl::new(Version::WebGl2);
    g.call("enable", &[n(0x1234 as f64)]);
    g.expect("INVALID_ENUM");
    g.call("blendFunc", &[n(0x1234 as f64), n(1.0)]);
    g.expect("INVALID_ENUM");
    assert_eq!(num(g.param("BLEND_SRC_RGB")), 1.0);
    let (cc, ca) = (g.k("CONSTANT_COLOR"), g.k("CONSTANT_ALPHA"));
    g.call("blendFunc", &[cc, ca]);
    g.expect("INVALID_OPERATION");
    assert_eq!(num(g.param("BLEND_DST_RGB")), 0.0);
    g.call("depthFunc", &[n(0.0)]);
    g.expect("INVALID_ENUM");
    assert_eq!(num(g.param("DEPTH_FUNC")), g.kv("LESS"));
    g.call("lineWidth", &[n(0.0)]);
    g.expect("INVALID_VALUE");
    g.call("lineWidth", &[n(f64::NAN)]);
    g.expect("INVALID_VALUE");
    assert_eq!(g.param("LINE_WIDTH"), Ret::Num(1.0));
    g.call("viewport", &[n(0.0), n(0.0), n(-1.0), n(5.0)]);
    g.expect("INVALID_VALUE");
    assert_eq!(g.param("VIEWPORT"), Ret::I32s(vec![0, 0, 16, 16]));
    g.call("depthRange", &[n(0.8), n(0.2)]);
    g.expect("INVALID_OPERATION");
    let pa = g.k("UNPACK_ALIGNMENT");
    g.call("pixelStorei", &[pa, n(3.0)]);
    g.expect("INVALID_VALUE");
    assert_eq!(g.param("UNPACK_ALIGNMENT"), Ret::Num(4.0));
    g.call("activeTexture", &[n(g.kv("TEXTURE0") + 32.0)]);
    g.expect("INVALID_ENUM");
    g.call("stencilOp", &[n(1.0), n(2.0), n(3.0)]);
    g.expect("INVALID_ENUM");
    g.call("hint", &[n(1.0), g.k("NICEST")]);
    g.expect("INVALID_ENUM");
    g.call("enable", &[Arg::Str("BLEND")]);
    g.expect("INVALID_OPERATION");
    // The first error wins until it is read; then NO_ERROR.
    g.call("enable", &[n(1.0)]);
    g.call("lineWidth", &[n(-1.0)]);
    g.expect("INVALID_ENUM");
    g.no_err();
    // WebGL 1 rejects WebGL 2 state.
    let mut g1 = Gl::new(Version::WebGl1);
    g1.call("blendEquation", &[n(0x8008 as f64)]);
    g1.expect("INVALID_ENUM");
    g1.call("enable", &[n(0x8C89 as f64)]);
    g1.expect("INVALID_ENUM");
    g1.call("pixelStorei", &[n(0x0CF2 as f64), n(4.0)]);
    g1.expect("INVALID_ENUM");
    g1.call("getExtension", &[Arg::Str("ext_BLEND_minmax")]);
    g1.call("blendEquation", &[n(0x8008 as f64)]);
    g1.no_err();
}

#[test]
fn context_queries() {
    let mut g = Gl::new(Version::WebGl1);
    assert_eq!(g.call("getContextAttributes", &[]), Ret::Attributes(Attributes::default()));
    assert_eq!(g.call("isContextLost", &[]), Ret::Bool(false));
    let Ret::Strs(list) = g.call("getSupportedExtensions", &[]) else { panic!() };
    assert_eq!(list.len(), 13);
    assert_eq!(list[0], "ANGLE_instanced_arrays");
    assert_eq!(g.call("getExtension", &[Arg::Str("oes_texture_float")]), Ret::Extension("OES_texture_float"));
    assert_eq!(g.call("getExtension", &[Arg::Str("WEBGL_debug_renderer_info")]), Ret::Null);
    let mut g2 = Gl::new(Version::WebGl2);
    let Ret::Strs(list) = g2.call("getSupportedExtensions", &[]) else { panic!() };
    assert_eq!(list, vec!["EXT_color_buffer_float", "EXT_color_buffer_half_float", "WEBGL_lose_context"]);
}
