//! G6: invalid shaders give `Err` whose first line is `ERROR: 0:<n>:` with the right line.

mod common;
use common::try_build;
use invisium_glsl::{compile, Stage};

const VS1: &str = "attribute vec2 a_pos; void main() { gl_Position = vec4(a_pos, 0.0, 1.0); }";
const FS1: &str = "precision mediump float; void main() { gl_FragColor = vec4(1.0); }";

fn first_line(e: &str) -> &str {
    e.lines().next().unwrap_or("")
}

fn expect(stage: Stage, src: &str, line: u32, webgl2: bool, what: &str) {
    match compile(stage, src, webgl2) {
        Ok(_) => panic!("{what}: compiled but should not:\n{src}"),
        Err(e) => assert!(first_line(&e).starts_with(&format!("ERROR: 0:{line}:")), "{what}: want line {line}, got: {e}"),
    }
}

fn frag(src: &str, line: u32, what: &str) {
    expect(Stage::Fragment, &format!("precision mediump float;\n{src}"), line + 1, false, what);
}

fn frag3(src: &str, line: u32, what: &str) {
    expect(Stage::Fragment, &format!("#version 300 es\nprecision mediump float;\nout vec4 o;\n{src}"), line + 3, true, what);
}

fn link_err(vs: &str, fs: &str, binds: &[(String, u32)], line: u32, webgl2: bool, what: &str) {
    match try_build(vs, fs, webgl2, binds) {
        Ok(_) => panic!("{what}: linked but should not"),
        Err(e) => assert!(first_line(&e).starts_with(&format!("ERROR: 0:{line}:")), "{what}: want line {line}, got: {e}"),
    }
}

#[test]
fn type_errors() {
    frag("void main() {\n  float a = 1;\n}", 2, "int to float");
    frag("void main() {\n  vec3 a = vec3(1.0);\n  vec2 b = a + vec2(1.0);\n}", 3, "vec3 + vec2");
    frag("float f() {\n  return 1;\n}\nvoid main() {}", 2, "return type");
    frag("void main() {\n  if (1.0) {}\n}", 2, "non-bool condition");
    frag3("void main() {\n  uint u = 1;\n}", 2, "int to uint");
}

#[test]
fn name_errors() {
    frag("void main() {\n  gl_FragColor = vec4(x);\n}", 2, "undeclared identifier");
    frag("float a;\nfloat a;\nvoid main() {}", 2, "redefinition");
    frag("void main() {\n  float b;\n  float b;\n}", 3, "local redefinition");
    frag("void f(float x) {}\nvoid f(float y) {}\nvoid main() {}", 2, "function redefinition");
    frag("void main() {\n  vec4 c = texture(c, vec2(0.0));\n}", 2, "texture() in 1.00");
}

#[test]
fn call_errors() {
    frag("float f(float x) { return x; }\nvoid main() {\n  float y = f(1.0, 2.0);\n}", 3, "wrong argument count");
    frag("void main() {\n  float y = sin(1);\n}", 2, "no matching overload of built-in");
    frag("float f(vec2 x) { return x.x; }\nvoid main() {\n\n  float y = f(1.0);\n}", 4, "no matching overload of user function");
    frag("float f(float x);\nfloat g() { return f(1.0); }\nvoid main() {\n  float y = g();\n}", 2, "called but never defined");
    frag("float f(float x) { return g(x); }\nfloat g(float x);\nvoid main() {}", 1, "call before declaration");
}

#[test]
fn assignment_errors() {
    frag("const float k = 1.0;\nvoid main() {\n  k = 2.0;\n}", 3, "assign to const");
    frag("uniform float u;\nvoid main() {\n  u = 2.0;\n}", 3, "assign to uniform");
    frag("varying vec2 v;\nvoid main() {\n  v = vec2(1.0);\n}", 3, "assign to fragment input");
    frag("void main() {\n  gl_FragCoord = vec4(0.0);\n}", 2, "assign to read-only built-in");
    frag("void f(out float x) { x = 1.0; }\nvoid main() {\n  f(2.0);\n}", 3, "out argument not an l-value");
    frag("void main() {\n  vec2 v;\n  v.xx = vec2(1.0);\n}", 3, "duplicate swizzle write");
}

#[test]
fn swizzle_and_array_errors() {
    frag("void main() {\n  vec2 v = vec2(1.0);\n  float a = v.z;\n}", 3, "swizzle out of range");
    frag("void main() {\n  vec4 v = vec4(1.0);\n  vec2 a = v.xg;\n}", 3, "mixed swizzle sets");
    frag("void main() {\n  int n = 3;\n  float a[n];\n}", 3, "non-constant array size");
    frag("void main() {\n  float a[2];\n  a[2] = 1.0;\n}", 3, "constant index out of range");
}

#[test]
fn structural_errors() {
    frag("float f(float x) { return g(x); }\nfloat g(float x) { return f(x); }\nvoid main() {}", 1, "recursion (call before declaration)");
    frag("float g(float x);\nfloat f(float x) { return g(x); }\nfloat g(float x) {\n  return f(x);\n}\nvoid main() { f(1.0); }", 4, "recursion");
    expect(Stage::Fragment, "precision mediump float;\nvoid other() {}\n", 2, false, "missing main");
    frag("void main() {\n  break;\n}", 2, "break outside loop");
    expect(Stage::Vertex, "void main() {\n  discard;\n}", 2, false, "discard in vertex shader");
    frag("void main() {\n  float a = 1.0\n  float b = 2.0;\n}", 3, "missing semicolon");
    frag3("void main() {\n  int x = 1;\n  switch (x) {\n    case 1: o = vec4(1.0);\n    case 2: o = vec4(2.0); break;\n  }\n}", 3, "switch fall-through");
}

#[test]
fn preprocessor_errors() {
    frag("void main() {}\n#error stop here", 2, "#error");
    frag("#if 1\nvoid main() {}", 1, "unterminated #if");
    frag("#ifdef X\n#else\n#elif 1\n#endif\nvoid main() {}", 3, "#elif after #else");
}

#[test]
fn link_errors() {
    link_err(
        "attribute vec2 a_pos; varying vec2 v; void main() { v = a_pos; gl_Position = vec4(a_pos, 0.0, 1.0); }",
        "precision mediump float;\nvarying vec3 v;\nvoid main() { gl_FragColor = vec4(v, 1.0); }",
        &[],
        2,
        false,
        "varying type mismatch",
    );
    link_err(VS1, "precision mediump float;\n\nvarying vec2 w;\nvoid main() { gl_FragColor = vec4(w, 0.0, 1.0); }", &[], 3, false, "varying missing in vertex");
    link_err(
        "uniform vec4 u; attribute vec2 a_pos; void main() { gl_Position = u; }",
        "precision mediump float;\nuniform vec3 u;\nvoid main() { gl_FragColor = vec4(u, 1.0); }",
        &[],
        2,
        false,
        "uniform type mismatch",
    );
    link_err(
        "#version 300 es\nlayout(location = 1) in vec4 a;\nlayout(location = 1) in vec4 b;\nvoid main() { gl_Position = a + b; }",
        "#version 300 es\nprecision mediump float; out vec4 o; void main() { o = vec4(1.0); }",
        &[],
        3,
        true,
        "layout location clash",
    );
    link_err(
        "attribute vec4 a;\nattribute mat2 m;\nvoid main() { gl_Position = a + vec4(m[0], m[1]); }",
        FS1,
        &[("a".into(), 3), ("m".into(), 2)],
        2,
        false,
        "binding clash (mat2 spans 2-3)",
    );
}
