//! G2: preprocessor. Expansion results are checked with `#if ... #error` and by compiling code.

use invisium_glsl::{compile, Stage};

const MAIN: &str = "\nprecision mediump float;\nvoid main() { gl_FragColor = vec4(1.0); }\n";

fn ok(src: &str) {
    if let Err(e) = compile(Stage::Fragment, &format!("{src}{MAIN}"), true) {
        panic!("{e}\n{src}");
    }
}

fn err_at(src: &str, line: u32, webgl2: bool) -> String {
    match compile(Stage::Fragment, &format!("{src}{MAIN}"), webgl2) {
        Ok(_) => panic!("expected an error:\n{src}"),
        Err(e) => {
            assert!(e.starts_with(&format!("ERROR: 0:{line}:")), "wrong line (want {line}): {e}");
            e
        }
    }
}

#[test]
fn object_and_function_like_macros() {
    ok("#define X 5\n#if X != 5\n#error object macro\n#endif");
    ok("#define SQ(x) ((x) * (x))\n#define QUAD(x) SQ(SQ(x))\n#if QUAD(2) != 16 || SQ(1 + 2) != 9\n#error nested\n#endif");
    ok("#define NONE() 7\n#define PAIR(a, b) a - b\n#if NONE() != 7 || PAIR(PAIR(9, 3), 1) != 5\n#error args\n#endif");
    ok("#define F(x) x\n#define G F\n#if G(3) != 3\n#error indirect\n#endif");
    // A function-like macro name without '(' is an ordinary identifier.
    ok("#define f(x) x\nfloat f = 1.0;");
    let e = err_at("#define P(a, b) a\n#if P(1) == 1\n#endif", 2, true);
    assert!(e.contains("arguments"), "{e}");
}

#[test]
fn macro_used_in_its_own_expansion() {
    ok("float foo = 1.0;\n#define foo foo + 1.0\nfloat bar() { return foo; }");
    ok("#define A B\n#define B A\nfloat A = 2.0;\nfloat get() { return A * 2.0; }");
    let e = err_at("#define A B\n#define B A\nfloat A = 2.0;\nfloat get() { return B; }", 4, true);
    assert!(e.contains("'B'"), "B expands to A then stops at B: {e}");
    ok("float R(float y) { return y; }\n#define R(x) R(x) * 2.0\nfloat use() { return R(1.0); }");
}

#[test]
fn if_arithmetic_and_defined() {
    ok("#if (3 + 4 * 2) % 5 != 1 || (1 << 3) != 8 || !(-1 < 0) || !0 != 1 || ~0 != -1 || 10 / 3 != 3 || (6 & 3) != 2 || (6 | 1) != 7 || (6 ^ 3) != 5\n#error arithmetic\n#endif");
    ok("#if 2 > 1 && 1 >= 1 && 1 <= 1 && 0 == 0 && (1 || 0) && !(0 && 1) && 64 >> 2 == 16\n#else\n#error comparisons\n#endif");
    ok("#define X\n#if defined X && defined(X) && !defined(Y) && !defined Y\n#else\n#error defined\n#endif");
    err_at("#if 1 / 0\n#endif", 1, true);
    err_at("\n#if UNDEFINED_NAME\n#endif", 2, true);
}

#[test]
fn conditional_directives() {
    ok("#ifdef NOPE\n#error a\n#elif 1\n#ifndef NOPE\n#define OK 1\n#else\n#error b\n#endif\n#else\n#error c\n#endif\n#if !OK\n#error d\n#endif");
    ok("#if 0\n#if 1\n#error skipped\n#endif\n#elif 0\n#error e\n#else\n#define Z 1\n#endif\n#ifndef Z\n#error f\n#endif");
    ok("#define U 1\n#undef U\n#ifdef U\n#error undef\n#endif\n#undef NEVER_DEFINED");
    err_at("\n\n#if 1\nfloat x;", 3, true);
    err_at("#endif", 1, true);
    err_at("#if 1\n#else\n#else\n#endif", 3, true);
}

#[test]
fn error_pragma_extension() {
    let e = err_at("\n\n#error custom message here", 3, true);
    assert!(e.contains("custom message"), "{e}");
    ok("#pragma optimize(off)\n#pragma STDGL invariant(all)\n#pragma anything at all");
    ok("#extension GL_OES_standard_derivatives : enable\n#extension GL_EXT_shader_texture_lod : require\n#extension GL_EXT_frag_depth : warn\n#extension GL_EXT_draw_buffers : disable\n#extension all : warn\n#extension all : disable");
    ok("#extension GL_SOME_unknown_ext : enable");
    err_at("\n#extension GL_OES_EGL_image_external : enable", 2, true);
    err_at("#extension all : enable", 1, true);
    err_at("#extension GL_SOME_unknown_ext : require", 1, true);
    err_at("#extension GL_OES_standard_derivatives enable", 1, true);
}

#[test]
fn version_rules() {
    let src = "#version 300 es\nprecision mediump float; out vec4 o; void main() { o = vec4(1.0); }";
    assert!(compile(Stage::Fragment, src, true).is_ok());
    let e = compile(Stage::Fragment, src, false).err().expect("300 es must fail in WebGL 1");
    assert!(e.starts_with("ERROR: 0:1:"), "{e}");
    let late = "precision mediump float;\n#version 300 es\nvoid main() {}";
    let e = compile(Stage::Fragment, late, true).err().expect("late #version must fail");
    assert!(e.starts_with("ERROR: 0:2:"), "{e}");
    assert!(compile(Stage::Fragment, "/* comment */ // line\n#version 100\nprecision mediump float; void main() { gl_FragColor = vec4(1.0); }", false).is_ok());
    assert!(compile(Stage::Fragment, "#version 200\nvoid main() {}", true).is_err());
    let v1 = compile(Stage::Vertex, "#version 100\nvoid main() { gl_Position = vec4(0.0); }", false).unwrap();
    assert_eq!(v1.version(), 100);
}

#[test]
fn predefined_macros_and_line() {
    ok("#if __LINE__ != 1\n#error line one\n#endif\n#if __FILE__ != 0 || GL_ES != 1 || __VERSION__ != 100 || GL_FRAGMENT_PRECISION_HIGH != 1\n#error predefined\n#endif");
    let v3 = "#version 300 es\n#if __VERSION__ != 300\n#error v\n#endif\nprecision mediump float; out vec4 o; void main() { o = vec4(1.0); }";
    assert!(compile(Stage::Fragment, v3, true).is_ok());
    ok("#line 50\n#if __LINE__ != 50\n#error after line\n#endif");
    err_at("#line 70\n#error here", 70, true);
    err_at("#define GL_MINE 1", 1, true);
    err_at("#undef __LINE__", 1, true);
    ok("#define LN __LINE__\n\n#if LN != 3\n#error macro line\n#endif");
}

#[test]
fn line_continuation_and_comments() {
    ok("#define A 1 \\\n  + 2\n#if __LINE__ != 3 || A != 3\n#error continuation\n#endif");
    ok("/* a comment\n   over two lines */\n#if __LINE__ != 3\n#error block comment\n#endif");
    ok("// one \\\n continued comment\n#if __LINE__ != 3\n#error line comment continuation\n#endif");
    err_at("float x = 1.0;\n/* never\nclosed", 2, true);
}

#[test]
fn redefinition() {
    ok("#define A 1\n#define A 1");
    err_at("#define A 1\n#define A 2", 2, true);
    err_at("#define F(x, x) x", 1, true);
}
