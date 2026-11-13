//! G3: shader pairs covering the language compile, link, and give WGSL that naga accepts.

mod common;
use common::*;

const VS1: &str = "attribute vec2 a_pos; void main() { gl_Position = vec4(a_pos, 0.0, 1.0); }";
const VS3: &str = "#version 300 es\nin vec2 a_pos; void main() { gl_Position = vec4(a_pos, 0.0, 1.0); }";
const FS1: &str = "precision mediump float; void main() { gl_FragColor = vec4(1.0); }";
const FS3: &str = "#version 300 es\nprecision mediump float; out vec4 o; void main() { o = vec4(1.0); }";

fn f1(body: &str) -> (String, String, bool) {
    (VS1.into(), format!("precision highp float;\n{body}"), false)
}
fn f3(body: &str) -> (String, String, bool) {
    (VS3.into(), format!("#version 300 es\nprecision highp float;\nout vec4 o;\n{body}"), true)
}
fn v1(body: &str) -> (String, String, bool) {
    (body.into(), FS1.into(), false)
}
fn v3(body: &str) -> (String, String, bool) {
    (format!("#version 300 es\n{body}"), FS3.into(), true)
}

fn pairs() -> Vec<(String, String, bool)> {
    let samplers = [
        ("sampler2D", "ivec2"), ("sampler3D", "ivec3"), ("samplerCube", "ivec2"), ("sampler2DArray", "ivec3"),
        ("sampler2DShadow", "ivec2"), ("samplerCubeShadow", "ivec2"), ("sampler2DArrayShadow", "ivec3"),
        ("isampler2D", "ivec2"), ("isampler3D", "ivec3"), ("isamplerCube", "ivec2"), ("isampler2DArray", "ivec3"),
        ("usampler2D", "ivec2"), ("usampler3D", "ivec3"), ("usamplerCube", "ivec2"), ("usampler2DArray", "ivec3"),
    ];
    let mut all_samplers = String::new();
    let mut sum = String::from("ivec3 s = ivec3(0);\n");
    for (k, (t, r)) in samplers.iter().enumerate() {
        all_samplers.push_str(&format!("precision highp {t}; uniform highp {t} u{k};\n{r} sz{k}(highp {t} s) {{ return textureSize(s, 0); }}\n"));
        let pad = if *r == "ivec2" { ", 0" } else { "" };
        sum.push_str(&format!("s += ivec3(sz{k}(u{k}){pad});\n"));
    }
    vec![
        // 1-4: minimal and preprocessor
        (VS1.into(), FS1.into(), false),
        (VS3.into(), FS3.into(), true),
        f1("#define A 2\n#define TWICE(x) ((x) * A)\n#define ADD(a, b) a + b\n#if TWICE(3) == 6 && defined(A) && !defined B\nconst float k = float(ADD(TWICE(1), 1));\n#elif 1\nconst float k = 0.0;\n#else\n#error no\n#endif\n#ifdef GL_ES\n#pragma optimize(on)\n#endif\n#undef A\n#ifndef A\nvoid main() { gl_FragColor = vec4(k, float(__LINE__), float(__VERSION__), 1.0); }\n#endif"),
        f3("#extension all : warn\n#define LONG_MACRO(a, \\\n  b) (a * \\\n  b)\n#line 100\nvoid main() { o = vec4(LONG_MACRO(2.0, 3.0), float(__LINE__), 0.0, 1.0); }"),
        // 5-6: every sampler type in precision statements, uniforms and parameters
        (VS3.into(), format!("#version 300 es\nprecision highp float;\nout vec4 o;\n{all_samplers}void main() {{ {sum} o = vec4(s, 1.0); }}"), true),
        (format!("#version 300 es\n{all_samplers}in vec2 a_pos; void main() {{ {sum} gl_Position = vec4(a_pos, float(s.x), 1.0); }}"), FS3.into(), true),
        // 7-10: qualifiers
        ("invariant varying vec2 v; attribute vec2 a_pos; invariant gl_Position; void main() { v = a_pos; gl_Position = vec4(a_pos, 0.0, 1.0); gl_PointSize = 1.0; }".into(),
         "precision lowp float; invariant varying vec2 v; void main() { gl_FragColor = vec4(v, 0.0, 1.0); }".into(), false),
        ("#version 300 es\nlayout(location = 3) in highp vec4 a_pos; centroid out vec2 c; flat out ivec2 f; smooth out float s; void main() { c = a_pos.xy; f = ivec2(1); s = 1.0; gl_Position = a_pos; }".into(),
         "#version 300 es\nprecision mediump float; centroid in vec2 c; flat in ivec2 f; smooth in float s; layout(location = 0) out vec4 o; void main() { o = vec4(c, float(f.x), s); }".into(), true),
        f1("const int N = 3; const float K[1] = float[1](1.0); void main() { const vec2 c = vec2(1.0, 2.0); highp float a[N]; mediump int i = N - 1; a[i] = c.y; gl_FragColor = vec4(a[2]); }".replace("const float K[1] = float[1](1.0); ", "").as_str()),
        f3("const float K[2] = float[2](1.0, 2.0); uniform lowp float u; void main() { o = vec4(K[1] + u); }"),
        // 11-15: statements
        f1("uniform float u; void main() { float r = 0.0; if (u > 1.0) r = 1.0; else if (u > 0.5) { r = 2.0; } else { r = 3.0; }
            for (int i = 0, j = 2; i < 4; ++i) { if (i == j) continue; r += float(i); }
            int k = 0; while (k < 3) { k++; if (k == 2) break; }
            do { r -= 1.0; } while (r > 10.0);
            if (r < -100.0) discard; gl_FragColor = vec4(r); }"),
        f3("uniform int u; float pick(int x) { switch (x) { case 0: case 1: return 1.0; case 2: { return 2.0; } default: break; } return 3.0; }
            void main() { float r = 0.0; switch (u) { case 5: r = 1.0; break; case 6: case 7: r = 2.0; break; } o = vec4(pick(u) + r); }"),
        f3("uniform uint u; void main() { float r = 0.0; switch (u) { default: r = 4.0; } for (;;) { r += 1.0; if (r > 9.0) break; } o = vec4(r); }"),
        f1("float f(float x) { if (x > 0.0) { return x; } return -x; } void main() { gl_FragColor = vec4(f(-2.0)); }"),
        f1("void main() { {} ; { float a = 1.0; { float a = 2.0; gl_FragColor = vec4(a); } } }"),
        // 16-19: operators
        f1("uniform float u; void main() { float a = u; a += 1.0; a -= 2.0; a *= 3.0; a /= 4.0; vec3 v = vec3(a); v *= 2.0; v += v; v = -v;
            mat2 m = mat2(1.0); m *= m; m = m * 2.0 + 1.0 - m / 2.0; m++; --m; v.xy++; bool b = (a < 1.0) ^^ (a >= 2.0) || !(a == 3.0) && (a != 4.0);
            float c = (a > 0.0 ? a : u, 1.0); float d = a = c = 5.0; gl_FragColor = vec4(v.x, m[0][0], b ? c : d, -m[1][1] / 1.0); }"),
        f3("uniform int i; uniform uint u; void main() { int a = i; a %= 3; a <<= 2; a >>= 1; a &= 7; a |= 8; a ^= 1;
            uint b = u; b %= 5u; b <<= 1u; b >>= 1; ivec3 v = ivec3(i) << 1; v = v >> ivec3(1); v &= 3; uvec2 w = ~uvec2(u) | 1u ^ uvec2(2u);
            o = vec4(float(a), float(b), float(v.x + (i / 2) * (-i) % 4), float(w.y & 15u)); }"),
        f3("uniform vec4 u; void main() { vec4 v = u; float s = v.x--; s += ++v.y; v.zw += v.xy * vec2(2.0); ivec2 iv = ivec2(1); iv++; o = vec4(s, v.z, float(iv.x--), float(--iv.y)); }"),
        f1("uniform mat3 m; uniform vec3 v; void main() { vec3 a = m * v; vec3 b = v * m; mat3 c = m * m; bool e = m == c && v != a && c != mat3(2.0); gl_FragColor = vec4(a + b + c[1], e ? 1.0 : 0.0); }"),
        // 20-22: constructors
        f1("uniform vec4 u; void main() { vec2 a = vec2(u); vec3 b = vec3(a, 1.0); vec4 c = vec4(1.0, b.xy, u.w); ivec3 i = ivec3(u); bvec2 bb = bvec2(u.x, 0.0);
            vec4 d = vec4(bb, i.xy); float f = float(i.z) + float(bb.y) + float(true); int g = int(u.y); bool h = bool(g);
            vec3 e = vec3(mat2(u)); vec4 k = vec4(vec2(1.0), vec3(2.0)); gl_FragColor = c + d + vec4(f, float(h), e.z, k.w); }"),
        f1("uniform vec4 u; void main() { mat2 a = mat2(u); mat2 b = mat2(u.xy, u.zw); mat3 c = mat3(u, u, u.x); mat4 d = mat4(c); mat3 e = mat3(d); mat2 f = mat2(e);
            mat4 g = mat4(2.0); mat3 h = mat3(vec2(1.0), 1.0, u.xyz, vec3(3.0)); gl_FragColor = vec4(a[1][1] + b[0][0], d[3][3] + e[2][2], f[1][0] + g[2][2], h[1][2]); }"),
        f3("struct S { int a; vec2 b; }; uniform uvec2 u; void main() { S s = S(1, vec2(2.0)); S t[2] = S[2](s, S(3, vec2(4.0)));
            float arr[] = float[](1.0, 2.0, 3.0); uvec3 x = uvec3(u, 7u); ivec2 y = ivec2(u); vec2 z = vec2(y); uint w = uint(-1);
            o = vec4(float(t[1].a) + arr[2], z.y, float(x.z + w), float(arr.length() + t.length())); }"),
        // 23-24: swizzles
        f1("uniform vec4 u; void main() { vec4 c = u.rgba + u.stpq + u.xyzw.wzyx; c.rg = c.gr; c.b = c.r; c.xzw = vec3(1.0, 2.0, 3.0); c.ts.y = 4.0; c.wz *= vec2(2.0); gl_FragColor = c.yyyy + c.wzyx; }"),
        f3("uniform ivec4 u; void main() { ivec4 c = u; c.yx.y = 9; c.zw++; ++c.x; c.zw.yx -= ivec2(1); o = vec4(c); }"),
        // 25-27: structures and arrays
        f1("struct In { vec2 p; float w[2]; }; struct Out { In i[2]; mat2 m; bool b; }; uniform Out u;
            void main() { Out o = u; o.i[1].w[0] = 1.0; o.i[0] = o.i[1]; bool same = o == u && o.i[0] != o.i[1];
            gl_FragColor = vec4(o.i[0].p, o.i[1].w[1], same ? o.m[1][1] : 0.0); }"),
        f3("struct L { vec3 c; float k; }; uniform L ls[4]; uniform int n; L pick(int i) { return ls[i]; }
            void main() { vec3 s = vec3(0.0); for (int i = 0; i < 4; i++) { if (i >= n) break; s += ls[i].c * ls[i].k; }
            float a[4] = float[4](1.0, 2.0, 3.0, 4.0); float b[4] = a; b[n] = 0.0; o = vec4(s + pick(n).c, (a == b) ? 1.0 : 0.0); }"),
        f1("uniform float w[4]; uniform vec3 v[2]; uniform mat4 m[2]; uniform bvec3 b[2]; float sum(float x[4]) { float s = 0.0; for (int i = 0; i < 4; i++) s += x[i]; return s; }
            void main() { gl_FragColor = vec4(sum(w), v[1].y, m[1][3][2], b[1].z ? 1.0 : 0.0); }"),
        // 28-29: functions
        f1("float g(float x); vec2 g(vec2 x) { return x * 2.0; } float g(float x) { return x + 1.0; } int g(int x) { return x; }
            void h(out vec2 a, inout float b, in float c) { a = vec2(b, c); b += c; c = 0.0; }
            struct S { vec2 p; float q[2]; };
            void main() { S s; s.q[1] = 1.0; vec4 v = vec4(0.0); h(s.p, s.q[1], 2.0); h(v.zw, v.x, g(1.0)); float k[3]; h(v.xy, k[2], float(g(1)));
            gl_FragColor = vec4(g(s.p), s.q[1], v.z + k[2]); }"),
        f3("vec3 f(vec3 a, int n) { for (int i = 0; i < n; i++) a = a.yzx; return a; } vec3 g(vec3 a) { return f(a, 2) + f(a, 1); } vec3 h(vec3 a) { return g(g(a)); }
            void main() { o = vec4(h(vec3(1.0, 2.0, 3.0)), 1.0); }"),
        // 30-33: built-in variables and constants
        ("#version 300 es\nin vec2 a_pos; flat out int id; void main() { id = gl_VertexID + gl_InstanceID; gl_PointSize = 2.0; gl_Position = vec4(a_pos, 0.0, 1.0); }".into(),
         "#version 300 es\nprecision mediump float; flat in int id; out vec4 o; void main() { gl_FragDepth = gl_FragCoord.z; o = vec4(float(id), gl_FrontFacing ? 1.0 : 0.0, gl_PointCoord); }".into(), true),
        f1("#extension GL_EXT_draw_buffers : require\nvoid main() { float a[gl_MaxDrawBuffers]; a[0] = float(gl_MaxVertexAttribs + gl_MaxTextureImageUnits); gl_FragData[0] = vec4(a[0]); gl_FragData[1] = gl_FragCoord; }"),
        f1("#extension GL_EXT_frag_depth : enable\nvoid main() { gl_FragDepthEXT = 0.5; gl_FragColor = vec4(gl_PointCoord, gl_FrontFacing ? 1.0 : 0.0, float(gl_MaxVaryingVectors)); }"),
        (VS3.into(), "#version 300 es\nprecision highp float; layout(location = 0) out vec4 o; layout(location = 1) out vec4 o2;
            void main() { o = vec4(float(gl_MaxDrawBuffers)); o2 = vec4(float(gl_MaxProgramTexelOffset - gl_MinProgramTexelOffset)); }".into(), true),
        // 34-38: built-in functions
        f1("#extension GL_OES_standard_derivatives : enable\nuniform vec3 x; uniform float s; void main() { vec3 a = radians(x) + degrees(x) + sin(x) + cos(x) + tan(x) + asin(x) + acos(x) + atan(x) + atan(x, x);
            a += pow(x, x) + exp(x) + log(x) + exp2(x) + log2(x) + sqrt(x) + inversesqrt(x) + abs(x) + sign(x) + floor(x) + ceil(x) + fract(x);
            a += mod(x, x) + mod(x, s) + min(x, x) + min(x, s) + max(x, s) + clamp(x, 0.0, 1.0) + clamp(x, x, x) + mix(x, x, s) + mix(x, x, x) + step(s, x) + step(x, x) + smoothstep(0.0, 1.0, x) + smoothstep(x, x, x);
            float f = length(x) + distance(x, x) + dot(x, x) + length(s) + dot(s, s) + normalize(s) + reflect(s, s) + refract(s, s, 0.5) + faceforward(s, s, s);
            a += cross(x, x) + normalize(x) + faceforward(x, x, x) + reflect(x, x) + refract(x, x, 0.5) + dFdx(x) + dFdy(x) + fwidth(x);
            gl_FragColor = vec4(a, f + mod(s, 2.0) + atan(s, 1.0)); }"),
        f3("uniform vec3 x; uniform ivec3 i; uniform uvec3 u; void main() { vec3 a = sinh(x) + cosh(x) + tanh(x) + asinh(x) + acosh(x) + atanh(x) + trunc(x) + round(x) + roundEven(x);
            ivec3 b = abs(i) + sign(i) + min(i, i) + max(i, 2) + clamp(i, 0, 3) + floatBitsToInt(x); uvec3 c = min(u, u) + max(u, 1u) + clamp(u, u, u) + floatBitsToUint(x);
            vec3 ip; vec3 fr = modf(x, ip); bvec3 n = isnan(x); bvec3 inf = isinf(x); a += intBitsToFloat(b) + uintBitsToFloat(c) + fr + ip + mix(x, ip, n) + mix(x, fr, inf);
            uint p = packSnorm2x16(x.xy) + packUnorm2x16(x.xy) + packHalf2x16(x.xy); vec2 q = unpackSnorm2x16(p) + unpackUnorm2x16(p) + unpackHalf2x16(p);
            o = vec4(a, q.x + float(isnan(x.x)) + float(isinf(x.y))); }"),
        f3("uniform mat2 m2; uniform mat3 m3; uniform mat4 m4; uniform vec4 v; void main() { mat2 a = matrixCompMult(m2, m2) + transpose(m2) + inverse(m2) + outerProduct(v.xy, v.zw);
            mat3 b = matrixCompMult(m3, m3) + transpose(m3) + inverse(m3) + outerProduct(v.xyz, v.xyz); mat4 c = inverse(m4) + transpose(m4) + outerProduct(v, v);
            o = vec4(a[0][0] + determinant(m2), b[1][1] + determinant(m3), c[2][2] + determinant(m4), 1.0); }"),
        f1("uniform vec4 a; uniform ivec2 b; uniform bvec3 c; void main() { bvec4 x = lessThan(a, a); bvec4 y = lessThanEqual(a, a); bvec4 z = greaterThan(a, a); bvec4 w = greaterThanEqual(a, a);
            bvec2 e = equal(b, b); bvec2 n = notEqual(b, b); bvec3 nc = not(c); bool r = any(x) || all(y) || any(z) && all(w) || any(e) || all(n) || any(nc) || any(equal(c, nc));
            gl_FragColor = vec4(r ? 1.0 : 0.0); }"),
        f1("uniform mat2 m; void main() { mat2 a = matrixCompMult(m, m); gl_FragColor = vec4(a[0], a[1]); }"),
        // 39-41: texture functions
        f3("uniform sampler2D t2; uniform samplerCube tc; uniform highp sampler3D t3; uniform highp sampler2DArray ta; uniform vec4 c;
            void main() { vec4 r = texture(t2, c.xy) + texture(t2, c.xy, 1.0) + textureLod(t2, c.xy, 0.0) + textureProj(t2, c.xyz) + textureProj(t2, c, 0.5) + texelFetch(t2, ivec2(0), 0) + textureProjLod(t2, c, 0.0);
            r += texture(tc, c.xyz) + texture(tc, c.xyz, 1.0) + textureLod(tc, c.xyz, 1.0);
            r += texture(t3, c.xyz) + textureLod(t3, c.xyz, 0.0) + textureProj(t3, c) + texelFetch(t3, ivec3(0), 0);
            r += texture(ta, c.xyz) + texture(ta, c.xyz, 0.5) + textureLod(ta, c.xyz, 0.0) + texelFetch(ta, ivec3(1), 0);
            r += vec4(vec2(textureSize(t2, 0) + textureSize(tc, 0)), vec2(textureSize(t3, 0).z + textureSize(ta, 0).z)); o = r; }"),
        f1("#extension GL_EXT_shader_texture_lod : enable\nuniform sampler2D t; uniform samplerCube c; varying vec2 v;
            void main() { gl_FragColor = texture2D(t, v) + texture2D(t, v, 1.0) + texture2DProj(t, vec3(v, 1.0)) + texture2DProj(t, vec4(v, 0.0, 1.0), 0.5)
              + textureCube(c, vec3(v, 1.0)) + textureCube(c, vec3(v, 1.0), 0.5) + texture2DLodEXT(t, v, 0.0) + texture2DProjLodEXT(t, vec3(v, 1.0), 0.0) + textureCubeLodEXT(c, vec3(v, 1.0), 1.0); }")
            .pipe(|(_, f, w)| ("attribute vec2 a_pos; varying vec2 v; void main() { v = a_pos; gl_Position = vec4(a_pos, 0.0, 1.0); }".into(), f, w)),
        v1("attribute vec2 a_pos; uniform sampler2D t; uniform samplerCube c; varying vec4 col;
            void main() { col = texture2DLod(t, a_pos, 0.0) + texture2D(t, a_pos) + texture2DProjLod(t, vec3(a_pos, 1.0), 0.0) + textureCubeLod(c, vec3(a_pos, 1.0), 0.0) + textureCube(c, vec3(1.0)); gl_Position = col; }"),
        // 42-44: samplers in functions and arrays, attributes, outputs
        f3("uniform sampler2D ts[3]; vec4 look(sampler2D s, vec2 c) { return texture(s, c); } void main() { o = look(ts[2], vec2(0.5)) + texture(ts[0], vec2(0.5)) + look(ts[1], vec2(0.0)); }"),
        v3("in vec4 a_pos; in mat4 a_m; in mat2 a_m2; in ivec3 a_i; in uint a_u; in float a_f; void main() { gl_Position = a_m * a_pos + vec4(a_m2[1], float(a_i.y + int(a_u)), a_f); }"),
        ("#version 300 es\nin vec2 a_pos; out float v[2]; out mat2 vm; void main() { v[0] = 1.0; v[1] = 2.0; vm = mat2(3.0); gl_Position = vec4(a_pos, 0.0, 1.0); }".into(),
         "#version 300 es\nprecision mediump float; in float v[2]; in mat2 vm; layout(location = 0) out vec4 c0; layout(location = 1) out ivec4 c1; layout(location = 2) out float c2[2];
            void main() { c0 = vec4(v[1], vm[0][0], 0.0, 1.0); c1 = ivec4(1); c2[0] = 1.0; c2[1] = v[0]; }".into(), true),
        // 45: global initialisers and constants
        f1("uniform float u; float g = 2.0; vec2 h = vec2(1.0, 3.0); const mat2 M = mat2(1.0, 2.0, 3.0, 4.0); float twice(float x) { return x * g; }
            void main() { g += u; gl_FragColor = vec4(twice(h.y), M[1][0], g, 1.0); }"),
    ]
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl<T> Pipe for T {}

#[test]
fn shader_pairs_compile_link_and_validate() {
    let ps = pairs();
    assert!(ps.len() >= 40, "only {} pairs", ps.len());
    for (k, (vs, fs, w)) in ps.iter().enumerate() {
        let p = try_build(vs, fs, *w, &[]).unwrap_or_else(|e| panic!("pair {}: {e}\n--- vs ---\n{vs}\n--- fs ---\n{fs}", k + 1));
        validate_program(&p);
    }
}
