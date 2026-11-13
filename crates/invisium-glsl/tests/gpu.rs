//! G4: translated shaders executed on the GPU (4x4 Rgba32Float target, read back).
//! Every channel compared is one value check; the tests skip when there is no adapter.

mod common;
use common::*;
use invisium_glsl::Program;

const VS1: &str = "attribute vec2 a_pos; void main() { gl_Position = vec4(a_pos, 0.0, 1.0); }";
const VS3: &str = "#version 300 es\nin vec2 a_pos; void main() { gl_Position = vec4(a_pos, 0.0, 1.0); }";

fn check(got: [f32; 4], want: [f32; 4], what: &str) {
    for c in 0..4 {
        if want[c].is_nan() {
            continue;
        }
        assert!((got[c] - want[c]).abs() < 1e-4, "{what}: channel {c}: got {got:?}, want {want:?}");
    }
}

/// Build, validate, draw a full-screen triangle and return pixel (0, 0).
fn frag(vs: &str, fs: &str, v3: bool, setup: impl FnOnce(&Program, &mut Draw)) -> Option<[f32; 4]> {
    let p = build(vs, fs, v3);
    validate_program(&p);
    let g = gpu()?;
    let mut d = Draw::new(&p);
    setup(&p, &mut d);
    Some(g.render(&d)[0])
}

fn f1(body: &str) -> Option<[f32; 4]> {
    frag(VS1, &format!("precision highp float;\n{body}"), false, |_, _| {})
}

fn f3(body: &str) -> Option<[f32; 4]> {
    frag(VS3, &format!("#version 300 es\nprecision highp float;\nout vec4 o;\n{body}"), true, |_, _| {})
}

#[test]
fn floored_mod_negative() {
    let r = f1("void main() { gl_FragColor = vec4(mod(-1.0, 3.0), mod(5.5, -2.0), mod(-7.0, -3.0), mod(vec2(7.0), 3.0).y); }");
    if let Some(r) = r {
        check(r, [2.0, -0.5, -1.0, 1.0], "mod");
    }
}

#[test]
fn matrix_constructors_and_indexing() {
    let r = f1("void main() { mat3 m = mat3(1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0);
        gl_FragColor = vec4(m[0][1], m[1][0], m[2][2], m[1][2]); }");
    if let Some(r) = r {
        check(r, [2.0, 4.0, 9.0, 6.0], "mat3 m[col][row]");
    }
    let r = f1("void main() { mat4 m4 = mat4(1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0);
        mat3 m3 = mat3(m4); gl_FragColor = vec4(m3[0][0], m3[1][1], m3[2][0], m3[2][2]); }");
    if let Some(r) = r {
        check(r, [1.0, 6.0, 9.0, 11.0], "mat3(mat4)");
    }
    let r = f1("void main() { mat4 b = mat4(mat2(1.0, 2.0, 3.0, 4.0)); mat2 d = mat2(3.0);
        gl_FragColor = vec4(b[0][1], b[2][2] + b[3][2], d[1][1] + d[0][1], b[1][0]); }");
    if let Some(r) = r {
        check(r, [2.0, 1.0, 3.0, 3.0], "mat4(mat2) and diagonal");
    }
}

#[test]
fn matrix_vector_products() {
    let r = f1("void main() { mat2 m = mat2(1.0, 2.0, 3.0, 4.0); vec2 a = m * vec2(1.0, 1.0); vec2 b = vec2(1.0, 1.0) * m;
        gl_FragColor = vec4(a, b); }");
    if let Some(r) = r {
        check(r, [4.0, 6.0, 3.0, 7.0], "m*v and v*m");
    }
}

#[test]
fn swizzle_writes() {
    let r = f1("void main() { vec4 v = vec4(0.0); v.zx = vec2(1.0, 2.0); v.y += 5.0; v.wy -= vec2(1.0, 2.0);
        gl_FragColor = v; }");
    if let Some(r) = r {
        check(r, [2.0, 3.0, 1.0, -1.0], "swizzle writes");
    }
}

#[test]
fn out_inout_through_swizzle_and_array() {
    let r = f1("void f(out float a, inout vec2 b) { a = 7.0; b = b * 2.0 + 1.0; }
        void main() { vec4 v = vec4(1.0, 1.0, 2.0, 1.0); float arr[3]; arr[0] = 0.0; arr[1] = 0.0; arr[2] = 0.0;
        f(arr[1], v.zx); gl_FragColor = vec4(v.x, arr[1], v.z, v.y); }");
    if let Some(r) = r {
        check(r, [3.0, 7.0, 5.0, 1.0], "out/inout args");
    }
}

#[test]
fn conditional_and_logic_side_effects() {
    let r = f1("void main() { float a = 0.0; float b = 0.0;
        float r = (a > -1.0) ? (a += 1.0) : (b += 1.0);
        bool t = (b > 0.0) && ((b += 10.0) > 0.0);
        bool u = (a > 0.0) || ((a += 10.0) > 0.0);
        gl_FragColor = vec4(a, b, r, float(t) + 2.0 * float(u)); }");
    if let Some(r) = r {
        check(r, [1.0, 0.0, 1.0, 2.0], "?: && || side effects");
    }
}

#[test]
fn increment_inside_expressions() {
    let r = f1("void main() { int i = 1; int j = i++ + ++i; float x = 2.0; float y = x-- * 3.0;
        gl_FragColor = vec4(float(i), float(j), x, y); }");
    if let Some(r) = r {
        check(r, [3.0, 4.0, 1.0, 6.0], "++ in expressions");
    }
}

#[test]
fn loops_do_while_continue() {
    let r = f1("void main() { int i = 0; int s = 0;
        do { i++; if (i == 2) continue; s += i; } while (i < 5);
        int k = 0; for (int j = 0; j < 10; j++) { if (j == 4) break; k += j; }
        int w = 0; while (w < 3) { w++; }
        gl_FragColor = vec4(float(i), float(s), float(k), float(w)); }");
    if let Some(r) = r {
        check(r, [5.0, 13.0, 6.0, 3.0], "do-while/for/while");
    }
}

#[test]
fn uniforms_at_reflected_offsets() {
    let fs = "precision highp float; struct L { vec3 color; float power; };
        uniform L light; uniform float w[3]; uniform bool flag; uniform vec2 v2; uniform mat3 m;
        void main() { gl_FragColor = vec4(light.color.y + light.power, w[1] + w[2], flag ? 1.0 : 0.0, v2.y + m[2][1]); }";
    let r = frag(VS1, fs, false, |p, d| {
        let b = &mut d.uniforms;
        set(p, b, "light.color", 0, &[1.0, 2.0, 3.0]);
        set(p, b, "light.power", 0, &[0.5]);
        for (i, v) in [1.0, 10.0, 100.0].iter().enumerate() {
            set(p, b, "w[0]", i as u32, &[*v]);
        }
        set(p, b, "flag", 0, &[1.0]);
        set(p, b, "v2", 0, &[0.0, 9.0]);
        set(p, b, "m", 0, &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 20.0, 0.0]);
    });
    if let Some(r) = r {
        check(r, [2.5, 110.0, 1.0, 29.0], "struct/array/bool/mat3 uniforms");
    }
}

#[test]
fn array_of_structures_uniform() {
    let fs = "#version 300 es\nprecision highp float; out vec4 o; struct P { float a; vec2 b; bool on; };
        uniform P ps[2]; uniform int idx;
        void main() { o = vec4(ps[0].a, ps[1].b.x, ps[idx].a, ps[0].b.y + (ps[1].on ? 100.0 : 0.0)); }";
    let r = frag(VS3, fs, true, |p, d| {
        let b = &mut d.uniforms;
        set(p, b, "ps[0].a", 0, &[1.0]);
        set(p, b, "ps[0].b", 0, &[2.0, 3.0]);
        set(p, b, "ps[1].a", 0, &[4.0]);
        set(p, b, "ps[1].b", 0, &[5.0, 6.0]);
        set(p, b, "ps[1].on", 0, &[1.0]);
        set(p, b, "idx", 0, &[1.0]);
    });
    if let Some(r) = r {
        check(r, [1.0, 5.0, 4.0, 103.0], "array of structures");
    }
}

fn tex2x2() -> (u32, u32, Vec<u8>) {
    (2, 2, vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255])
}

#[test]
fn sampler2d_texture_and_texel_fetch() {
    let fs = "#version 300 es\nprecision highp float; out vec4 o; uniform sampler2D t;
        void main() { vec4 f = texelFetch(t, ivec2(1, 0), 0); vec4 s = texture(t, vec2(0.25, 0.75));
        o = vec4(f.g, s.b, float(textureSize(t, 0).x), s.r); }";
    let r = frag(VS3, fs, true, |_, d| d.textures.push(tex2x2()));
    if let Some(r) = r {
        check(r, [1.0, 1.0, 2.0, 0.0], "texture/texelFetch/textureSize");
    }
    let fs1 = "precision mediump float; uniform sampler2D t; void main() { gl_FragColor = texture2D(t, vec2(0.75, 0.25)); }";
    let r = frag(VS1, fs1, false, |_, d| d.textures.push(tex2x2()));
    if let Some(r) = r {
        check(r, [0.0, 1.0, 0.0, 1.0], "texture2D");
    }
}

#[test]
fn attribute_through_varying() {
    let vs = "attribute vec2 a_pos; attribute float a_val; attribute vec3 a_col; varying float v_val; varying vec3 v_col;
        void main() { v_val = a_val; v_col = a_col; gl_Position = vec4(a_pos, 0.0, 1.0); }";
    let fs = "precision mediump float; varying float v_val; varying vec3 v_col; void main() { gl_FragColor = vec4(v_val, v_col); }";
    let r = frag(vs, fs, false, |_, d| {
        d.attribs.push(("a_val", Data::F(vec![0.25; 3])));
        d.attribs.push(("a_col", Data::F([0.5, 0.75, 1.0].repeat(3))));
    });
    if let Some(r) = r {
        check(r, [0.25, 0.5, 0.75, 1.0], "attribute -> varying");
    }
}

#[test]
fn flat_integer_varying() {
    let vs = "#version 300 es\nin vec2 a_pos; in ivec2 a_i; flat out int vi; flat out uvec2 vu;
        void main() { vi = a_i.x * 2 + 1; vu = uvec2(a_i) + 1u; gl_Position = vec4(a_pos, 0.0, 1.0); }";
    let fs = "#version 300 es\nprecision highp float; flat in int vi; flat in uvec2 vu; out vec4 o;
        void main() { o = vec4(float(vi), float(vu.x), float(vu.y), 0.0); }";
    let r = frag(vs, fs, true, |_, d| d.attribs.push(("a_i", Data::I([3, 5].repeat(3)))));
    if let Some(r) = r {
        check(r, [7.0, 4.0, 6.0, 0.0], "flat int varying");
    }
}

#[test]
fn lower_left_quarter_lands_in_rows_0_1() {
    let p = build(VS1, "precision mediump float; void main() { gl_FragColor = vec4(1.0); }", false);
    validate_program(&p);
    let Some(g) = gpu() else { return };
    let mut d = Draw::new(&p);
    d.attribs = vec![("a_pos", Data::F(vec![-1.0, -1.0, 0.0, -1.0, -1.0, 0.0]))];
    let px = g.render(&d);
    check(px[0], [1.0; 4], "row 0, column 0 drawn");
    for y in 0..4 {
        for x in 0..4 {
            if y >= 2 || x >= 2 {
                check(px[y * 4 + x], [-1.0; 4], &format!("pixel row {y} column {x} untouched"));
            }
        }
    }
}

#[test]
fn depth_maps_minus_w_to_0_and_w_to_1() {
    for (z, want) in [(-2.0f32, 0.0f32), (2.0, 1.0)] {
        let vs = format!("attribute vec2 a_pos; void main() {{ gl_Position = vec4(a_pos * 2.0, {z:?}, 2.0); }}");
        let r = frag(&vs, "precision highp float; void main() { gl_FragColor = vec4(gl_FragCoord.z); }", false, |_, _| {});
        if let Some(r) = r {
            check(r, [want; 4], "gl_FragCoord.z");
        }
    }
}

#[test]
fn functions_structs_arrays_switch() {
    let r = f3("struct S { float a; vec2 b[2]; };
        float g(float x) { return x * 2.0; } float g(vec2 x) { return x.x + x.y; }
        S make(float k) { S s; s.a = k; s.b[0] = vec2(k); s.b[1] = vec2(1.0, 2.0); return s; }
        void main() { S s = make(3.0); float arr[3] = float[3](1.0, 2.0, 3.0);
        int n = 2; float sw = 0.0;
        switch (n) { case 1: case 2: sw = 5.0; break; default: sw = 9.0; }
        o = vec4(g(s.a), g(s.b[1]), float(arr.length()) + arr[n], sw); }");
    if let Some(r) = r {
        check(r, [6.0, 3.0, 6.0, 5.0], "functions/structs/arrays/switch");
    }
}

#[test]
fn integer_ops_and_builtins() {
    let r = f3("void main() { int a = 7; uint b = 6u; ivec2 v = ivec2(12, -5);
        o = vec4(float((a << 2) | 1), float((b >> 1u) ^ 1u), float(v.x / 5 + v.y % 3 * 0), float(~a & 15)); }");
    if let Some(r) = r {
        check(r, [29.0, 2.0, 2.0, 8.0], "integer ops");
    }
    let r = f3("void main() { mat2 m = mat2(2.0, 0.0, 1.0, 4.0); mat2 i = inverse(m);
        o = vec4(clamp(5.0, 0.0, 1.0) + step(0.5, 0.7), determinant(m), (i * m)[1][1], degrees(atan(1.0, 1.0))); }");
    if let Some(r) = r {
        check(r, [2.0, 8.0, 1.0, 45.0], "math built-ins");
    }
}

/// Fragment shader with a uniform selector `s` and an `if / else if` chain built by `link(k)`.
fn chain_fs(n: usize, link: impl Fn(usize) -> String, tail: &str, out: &str) -> String {
    let mut b = String::from("precision highp float; uniform float sel;\nvoid main() { int s = int(sel); int i = 0; float r = 0.0;\n");
    for k in 0..n {
        b.push_str(&format!("{}if ({}) {{ r = {}.0; }}\n", if k == 0 { "" } else { "else " }, link(k), 100 + k));
    }
    b.push_str(&format!("{tail}\n{out} }}"));
    b
}

fn run_sel(fs: &str, sel: f32) -> Option<[f32; 4]> {
    frag(VS1, fs, false, |p, d| set(p, &mut d.uniforms, "sel", 0, &[sel]))
}

#[test]
fn long_else_if_chains_select_the_right_link() {
    for n in [20usize, 40] {
        let fs = chain_fs(n, |k| format!("s == {k}"), "else { r = -1.0; }", "gl_FragColor = vec4(r, 0.0, 0.0, 1.0);");
        assert!(build(VS1, &fs, false).fragment_wgsl.contains("= false;"), "a {n}-link chain should use the flat form");
        for (sel, want) in [(0, 100.0), (n / 2, 100.0 + (n / 2) as f32), (n - 1, 99.0 + n as f32), (n + 5, -1.0)] {
            if let Some(r) = run_sel(&fs, sel as f32) {
                check(r, [want, 0.0, 0.0, 1.0], &format!("{n}-link chain, selector {sel}"));
            }
        }
    }
}

#[test]
fn long_else_if_chain_conditions_run_once_in_order() {
    // Condition k increments i and is first true at k == s, so after the chain i == s + 1;
    // without a taken link every condition runs once and i == n.
    let n = 24;
    let fs = chain_fs(n, |_| "(i += 1) > s".into(), "else { r = -1.0; }", "gl_FragColor = vec4(r, float(i), 0.0, 1.0);");
    for (sel, want_r, want_i) in [(0, 100.0, 1.0), (11, 111.0, 12.0), (23, 123.0, 24.0), (30, -1.0, 24.0)] {
        if let Some(r) = run_sel(&fs, sel as f32) {
            check(r, [want_r, want_i, 0.0, 1.0], &format!("side-effect chain, selector {sel}"));
        }
    }
}

#[test]
fn long_else_if_chain_with_continue_and_break_in_a_loop() {
    let mut b = String::from("precision highp float;\nvoid main() { float acc = 0.0; int n = 0; int j;\nfor (j = 0; j < 30; j++) {\n n++;\n");
    for k in 0..20 {
        let body = match k {
            0 => "acc += 1.0; continue;",
            5 => "continue;",
            17 => "break;",
            _ => "acc += 2.0;",
        };
        b.push_str(&format!("{}if (j == {k}) {{ {body} }}\n", if k == 0 { "" } else { "else " }));
    }
    b.push_str("else { acc += 1000.0; }\n acc += 0.5;\n}\ngl_FragColor = vec4(acc, float(n), float(j), 1.0); }");
    // j = 0: +1 (continue); j = 1..16 except 5: 15 x 2.5; j = 5: continue; j = 17: break.
    if let Some(r) = frag(VS1, &b, false, |_, _| {}) {
        check(r, [38.5, 18.0, 17.0, 1.0], "chain with continue/break in a loop");
    }
}
