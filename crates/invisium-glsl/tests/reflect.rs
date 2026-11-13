//! G5: reflection data (uniforms, texture slots, attributes, outputs).

mod common;
use common::*;
use invisium_draw::gl::{SampleKind, TexKind};
use invisium_glsl::{Base, Place, Program, Scalar};

const VS1: &str = "attribute vec2 a_pos; void main() { gl_Position = vec4(a_pos, 0.0, 1.0); }";
const VS3: &str = "#version 300 es\nin vec2 a_pos; void main() { gl_Position = vec4(a_pos, 0.0, 1.0); }";

fn uni<'a>(p: &'a Program, name: &str) -> &'a invisium_glsl::Uniform {
    p.uniforms.iter().find(|u| u.name == name).unwrap_or_else(|| panic!("no uniform {name} in {:?}", p.uniforms))
}

fn block(p: &Program, name: &str) -> (u32, u32, Base, u8, u8, u32) {
    match uni(p, name).place {
        Place::Block { offset, stride, base, rows, cols, col_stride } => (offset, stride, base, rows, cols, col_stride),
        other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn uniform_layout_and_types() {
    let fs = "precision highp float; struct S { vec3 a; float b; mat2 c; bool d[2]; };
        uniform float f; uniform vec3 v3; uniform mat3 m3; uniform bool b; uniform float arr[4]; uniform vec2 v2arr[2];
        uniform S s; uniform S sa[2]; uniform float notused; uniform mat4 m4; uniform bvec2 bv; uniform ivec3 iv;
        void main() { gl_FragColor = vec4(f + v3.x + m3[0][0] + (b ? 1.0 : 0.0) + arr[1] + v2arr[1].y + s.a.x + sa[1].b + m4[3][3] + float(bv.y) + float(iv.z)); }";
    let p = build(VS1, fs, false);
    validate_program(&p);
    assert!(p.uniforms.iter().all(|u| u.name != "notused"), "unreferenced uniform reported");
    let expect: [(&str, u32, u32, u32, Base, u8, u8, u32); 15] = [
        ("f", 0x1406, 1, 0, Base::F32, 1, 1, 16),
        ("v3", 0x8B51, 1, 16, Base::F32, 3, 1, 16),
        ("m3", 0x8B5B, 1, 32, Base::F32, 3, 3, 16),
        ("b", 0x8B56, 1, 80, Base::Bool, 1, 1, 16),
        ("arr[0]", 0x1406, 4, 96, Base::F32, 1, 1, 16),
        ("v2arr[0]", 0x8B50, 2, 160, Base::F32, 2, 1, 16),
        ("s.a", 0x8B51, 1, 192, Base::F32, 3, 1, 16),
        ("s.b", 0x1406, 1, 204, Base::F32, 1, 1, 16),
        ("s.c", 0x8B5A, 1, 208, Base::F32, 2, 2, 8),
        ("s.d[0]", 0x8B56, 2, 224, Base::Bool, 1, 1, 16),
        ("sa[0].a", 0x8B51, 1, 256, Base::F32, 3, 1, 16),
        ("sa[1].c", 0x8B5A, 1, 336, Base::F32, 2, 2, 8),
        ("m4", 0x8B5C, 1, 384, Base::F32, 4, 4, 16),
        ("bv", 0x8B57, 1, 448, Base::Bool, 2, 1, 16),
        ("iv", 0x8B54, 1, 464, Base::I32, 3, 1, 16),
    ];
    for (name, ty, size, off, base, rows, cols, cs) in expect {
        let u = uni(&p, name);
        assert_eq!((u.gl_type, u.size), (ty, size), "{name}");
        let (o, _, b, r, c, s) = block(&p, name);
        assert_eq!((o, b, r, c), (off, base, rows, cols), "{name}");
        if cols > 1 {
            assert_eq!(s, cs, "{name} column stride");
        }
    }
    assert_eq!(block(&p, "arr[0]").1, 16, "float array stride");
    assert_eq!(block(&p, "s.d[0]").1, 16, "bool array stride");
    assert!(p.uniforms.iter().any(|u| u.name == "sa[1].d[0]"));
    assert_eq!(p.uniform_bytes, 480);
    assert_eq!(p.uniform_bytes % 16, 0);
}

#[test]
fn no_uniforms_means_zero_bytes() {
    let p = build(VS1, "precision mediump float; uniform float unused; void main() { gl_FragColor = vec4(1.0); }", false);
    assert_eq!(p.uniform_bytes, 0);
    assert!(p.uniforms.is_empty());
    assert!(!p.vertex_wgsl.contains("var<uniform>") && !p.fragment_wgsl.contains("var<uniform>"));
}

#[test]
fn uniform_shared_between_stages() {
    let vs = "uniform vec4 shared; uniform float vonly; attribute vec2 a_pos; void main() { gl_Position = vec4(a_pos, vonly, 1.0) + shared; }";
    let fs = "precision mediump float; uniform vec4 shared; uniform float fonly; void main() { gl_FragColor = shared * fonly; }";
    let p = build(vs, fs, false);
    validate_program(&p);
    let names: Vec<&str> = p.uniforms.iter().map(|u| u.name.as_str()).collect();
    assert_eq!(names, ["shared", "vonly", "fonly"]);
    assert_eq!(block(&p, "fonly").0, 20);
    assert_eq!(p.uniform_bytes, 32);
}

#[test]
fn sampler_slots() {
    let vs = "uniform sampler2D both; attribute vec2 a_pos; void main() { gl_Position = texture2D(both, a_pos); }";
    let fs = "precision mediump float; uniform sampler2D a; uniform samplerCube c[2]; uniform sampler2D both; uniform sampler2D unused;
        void main() { gl_FragColor = texture2D(a, vec2(0.0)) + textureCube(c[1], vec3(1.0)) + textureCube(c[0], vec3(1.0)) + texture2D(both, vec2(0.5)); }";
    let p = build(vs, fs, false);
    validate_program(&p);
    let slot = |n: &str| match uni(&p, n).place {
        Place::Texture { slot } => slot,
        other => panic!("{other:?}"),
    };
    assert_eq!((slot("both"), slot("a"), slot("c[0]")), (0, 1, 2));
    assert_eq!(uni(&p, "c[0]").size, 2);
    assert_eq!(uni(&p, "c[0]").gl_type, 0x8B60);
    assert!(p.uniforms.iter().all(|u| u.name != "unused"));
    assert_eq!(p.textures.len(), 4);
    for (k, t) in p.textures.iter().enumerate() {
        assert_eq!((t.texture, t.sampler), (2 * k as u32, 2 * k as u32 + 1));
        assert_eq!(t.sample, SampleKind::Float);
        assert!(t.in_fragment);
        assert_eq!(t.in_vertex, k == 0);
    }
    assert_eq!((p.textures[0].kind, p.textures[1].kind, p.textures[2].kind, p.textures[3].kind), (TexKind::D2, TexKind::D2, TexKind::Cube, TexKind::Cube));
}

#[test]
fn every_sampler_type_gets_a_slot() {
    let types = [
        ("sampler2D", 0x8B5E, TexKind::D2, SampleKind::Float),
        ("sampler3D", 0x8B5F, TexKind::D3, SampleKind::Float),
        ("samplerCube", 0x8B60, TexKind::Cube, SampleKind::Float),
        ("sampler2DArray", 0x8DC1, TexKind::Array, SampleKind::Float),
        ("sampler2DShadow", 0x8B62, TexKind::D2, SampleKind::Shadow),
        ("samplerCubeShadow", 0x8DC5, TexKind::Cube, SampleKind::Shadow),
        ("sampler2DArrayShadow", 0x8DC4, TexKind::Array, SampleKind::Shadow),
        ("isampler2D", 0x8DCA, TexKind::D2, SampleKind::Int),
        ("isampler3D", 0x8DCB, TexKind::D3, SampleKind::Int),
        ("isamplerCube", 0x8DCC, TexKind::Cube, SampleKind::Int),
        ("isampler2DArray", 0x8DCF, TexKind::Array, SampleKind::Int),
        ("usampler2D", 0x8DD2, TexKind::D2, SampleKind::Uint),
        ("usampler3D", 0x8DD3, TexKind::D3, SampleKind::Uint),
        ("usamplerCube", 0x8DD4, TexKind::Cube, SampleKind::Uint),
        ("usampler2DArray", 0x8DD7, TexKind::Array, SampleKind::Uint),
    ];
    let mut fs = String::from("#version 300 es\nprecision highp float; out vec4 o;\n");
    let mut sum = String::from("int s = 0;");
    for (k, (t, ..)) in types.iter().enumerate() {
        fs.push_str(&format!("precision highp {t}; uniform highp {t} u{k};\n"));
        sum.push_str(&format!(" s += textureSize(u{k}, 0).x;"));
    }
    fs.push_str(&format!("void main() {{ {sum} o = vec4(float(s)); }}"));
    let p = build(VS3, &fs, true);
    validate_program(&p);
    for (k, (_, gl, kind, sample)) in types.iter().enumerate() {
        let u = uni(&p, &format!("u{k}"));
        assert_eq!(u.gl_type, *gl, "u{k}");
        let Place::Texture { slot } = u.place else { panic!() };
        let t = p.textures[slot as usize];
        assert_eq!((t.kind, t.sample), (*kind, *sample), "u{k}");
    }
}

#[test]
fn attribute_locations() {
    let vs = "#version 300 es\nlayout(location = 5) in vec4 a; in vec2 b; in mat4 m; in float c; in vec3 unused; in ivec2 iv;
        void main() { gl_Position = m * a + vec4(b, c, float(iv.x)); }";
    let p = build(vs, "#version 300 es\nprecision mediump float; out vec4 o; void main() { o = vec4(1.0); }", true);
    let p2 = try_build(vs, "#version 300 es\nprecision mediump float; out vec4 o; void main() { o = vec4(1.0); }", true, &[("b".into(), 7), ("a".into(), 9)]).unwrap();
    validate_program(&p2);
    let loc = |p: &Program, n: &str| p.attributes.iter().find(|a| a.name == n).map(|a| (a.location, a.columns, a.gl_type, a.base));
    assert_eq!(loc(&p, "unused"), None);
    assert_eq!(loc(&p2, "a"), Some((5, 1, 0x8B52, Scalar::F32)), "layout wins over binding");
    assert_eq!(loc(&p2, "b"), Some((7, 1, 0x8B50, Scalar::F32)), "binding");
    assert_eq!(loc(&p2, "m"), Some((0, 4, 0x8B5C, Scalar::F32)), "mat4 takes four");
    assert_eq!(loc(&p2, "c"), Some((4, 1, 0x1406, Scalar::F32)), "lowest free");
    assert_eq!(loc(&p2, "iv"), Some((6, 1, 0x8B53, Scalar::I32)));
    assert_eq!(loc(&p, "b"), Some((0, 1, 0x8B50, Scalar::F32)));
    assert_eq!(loc(&p, "m"), Some((1, 4, 0x8B5C, Scalar::F32)));
    let p1 = try_build("attribute vec4 x; attribute vec4 y; void main() { gl_Position = x + y; }", "void main() {}", false, &[("y".into(), 0)]).unwrap();
    assert_eq!((loc(&p1, "y").unwrap().0, loc(&p1, "x").unwrap().0), (0, 1));
}

#[test]
fn outputs() {
    let p = build(VS1, "precision mediump float; void main() { gl_FragColor = vec4(1.0); }", false);
    assert_eq!(p.outputs.len(), 1);
    assert_eq!((p.outputs[0].name.as_str(), p.outputs[0].location, p.outputs[0].base), ("gl_FragColor", 0, Scalar::F32));
    assert!(!p.writes_depth);
    let fs = "#version 300 es\nprecision mediump float; layout(location = 2) out ivec4 i; layout(location = 0) out vec4 c; layout(location = 3) out uvec2 u;
        void main() { c = vec4(1.0); i = ivec4(2); u = uvec2(3u); gl_FragDepth = 0.5; }";
    let p = build(VS3, fs, true);
    validate_program(&p);
    let mut o: Vec<(String, u32, Scalar)> = p.outputs.iter().map(|o| (o.name.clone(), o.location, o.base)).collect();
    o.sort_by_key(|x| x.1);
    assert_eq!(o, [("c".into(), 0, Scalar::F32), ("i".into(), 2, Scalar::I32), ("u".into(), 3, Scalar::U32)]);
    assert!(p.writes_depth);
    let single = build(VS3, "#version 300 es\nprecision mediump float; out vec4 only; void main() { only = vec4(1.0); }", true);
    assert_eq!(single.outputs[0].location, 0);
}
