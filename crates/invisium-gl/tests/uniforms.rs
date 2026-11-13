//! P3: uniform values in `Draw::uniforms`, getUniform, errors, sharing.

mod common;
use common::*;
use invisium_gl::*;
use invisium_glsl::Place;
use std::rc::Rc;

const VS: &str = "#version 300 es\nvoid main() { gl_Position = vec4(0.0, 0.0, 0.0, 1.0); gl_PointSize = 1.0; }\n";
const FS: &str = "#version 300 es
precision highp float;
precision highp int;
uniform float uF; uniform vec2 uV2; uniform vec3 uV3; uniform vec4 uV4;
uniform int uI; uniform ivec2 uI2; uniform ivec3 uI3; uniform ivec4 uI4;
uniform uint uU; uniform uvec2 uU2; uniform uvec3 uU3; uniform uvec4 uU4;
uniform bool uB; uniform bvec2 uB2; uniform bvec3 uB3; uniform bvec4 uB4;
uniform mat2 uM2; uniform mat3 uM3; uniform mat4 uM4;
uniform float uArr[4];
uniform sampler2D uS;
out vec4 o;
void main() {
  float s = uF + uV2.x + uV3.x + uV4.x + float(uI + uI2.x + uI3.x + uI4.x) + float(uU + uU2.x + uU3.x + uU4.x);
  s += (uB ? 1.0 : 0.0) + (uB2.x ? 1.0 : 0.0) + (uB3.x ? 1.0 : 0.0) + (uB4.x ? 1.0 : 0.0);
  s += uM2[0][0] + uM3[0][0] + uM4[0][0] + uArr[0] + uArr[3] + texture(uS, vec2(0.5)).x;
  o = vec4(s);
}
";

struct T {
    g: Gl,
    r: invisium_glsl::Program,
    p: Arg<'static>,
}

impl T {
    fn new() -> T {
        let mut g = Gl::new(Version::WebGl2);
        let p = g.program(VS, FS);
        T { g, r: reflect(VS, FS, true), p }
    }
    fn set(&mut self, method: &str, name: &str, args: &[Arg]) {
        let l = self.g.loc(self.p, name);
        let mut v = vec![l];
        v.extend_from_slice(args);
        self.g.call(method, &v);
    }
    fn draw(&mut self) -> Rc<[u8]> {
        self.g.cmds();
        let pts = self.g.k("POINTS");
        self.g.call("drawArrays", &[pts, n(0.0), n(1.0)]);
        self.g.no_err();
        draws(&self.g.cmds()).pop().expect("a draw").uniforms
    }
    /// Values (4 bytes each) of element `elem`, column `col` of a uniform.
    fn read(&self, bytes: &[u8], name: &str, elem: u32, col: u32) -> Vec<[u8; 4]> {
        let u = self.r.uniforms.iter().find(|u| u.name == name || u.name == format!("{name}[0]")).unwrap();
        let Place::Block { offset, stride, rows, col_stride, .. } = u.place else { panic!() };
        let at = (offset + elem * stride + col * col_stride) as usize;
        (0..rows as usize).map(|r| bytes[at + r * 4..at + r * 4 + 4].try_into().unwrap()).collect()
    }
}

fn f(v: &[f32]) -> Vec<[u8; 4]> {
    v.iter().map(|x| x.to_le_bytes()).collect()
}
fn i(v: &[i32]) -> Vec<[u8; 4]> {
    v.iter().map(|x| x.to_le_bytes()).collect()
}

#[test]
fn every_setter_writes_the_reflected_place() {
    let mut t = T::new();
    t.set("uniform1f", "uF", &[n(1.5)]);
    t.set("uniform2f", "uV2", &[n(1.0), n(2.0)]);
    t.set("uniform3f", "uV3", &[n(1.0), n(2.0), n(3.0)]);
    t.set("uniform4fv", "uV4", &[Arg::Nums(&[9.0, 1.0, 2.0, 3.0, 4.0, 9.0]), n(1.0), n(4.0)]);
    t.set("uniform1i", "uI", &[n(-3.0)]);
    t.set("uniform2iv", "uI2", &[Arg::Bytes(Elem::I32, &[1, 0, 0, 0, 2, 0, 0, 0])]);
    t.set("uniform3i", "uI3", &[n(4.0), n(5.0), n(6.0)]);
    t.set("uniform4i", "uI4", &[n(7.0), n(8.0), n(9.0), n(10.0)]);
    t.set("uniform1ui", "uU", &[n(4000000000.0)]);
    t.set("uniform2ui", "uU2", &[n(1.0), n(2.0)]);
    t.set("uniform3uiv", "uU3", &[Arg::Nums(&[3.0, 4.0, 5.0])]);
    t.set("uniform4ui", "uU4", &[n(1.0), n(2.0), n(3.0), n(4.0)]);
    t.set("uniform1f", "uB", &[n(2.5)]);
    t.set("uniform2i", "uB2", &[n(0.0), n(5.0)]);
    t.set("uniform3ui", "uB3", &[n(1.0), n(0.0), n(1.0)]);
    t.set("uniform4fv", "uB4", &[Arg::Nums(&[0.0, 0.5, 0.0, -1.0])]);
    t.set("uniformMatrix2fv", "uM2", &[Arg::Bool(false), Arg::Nums(&[1.0, 2.0, 3.0, 4.0])]);
    t.set("uniformMatrix3fv", "uM3", &[Arg::Bool(true), Arg::Nums(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0])]);
    let m4: Vec<f64> = (0..18).map(|v| v as f64).collect();
    t.set("uniformMatrix4fv", "uM4", &[Arg::Bool(false), Arg::Nums(&m4), n(2.0), n(16.0)]);
    t.set("uniform1fv", "uArr", &[Arg::Nums(&[1.0, 2.0, 3.0, 4.0, 5.0])]);
    t.set("uniform1fv", "uArr[2]", &[Arg::Nums(&[7.0, 8.0])]);
    t.set("uniform1i", "uS", &[n(3.0)]);
    t.g.no_err();
    let b = t.draw();
    assert_eq!(t.read(&b, "uF", 0, 0), f(&[1.5]));
    assert_eq!(t.read(&b, "uV2", 0, 0), f(&[1.0, 2.0]));
    assert_eq!(t.read(&b, "uV3", 0, 0), f(&[1.0, 2.0, 3.0]));
    assert_eq!(t.read(&b, "uV4", 0, 0), f(&[1.0, 2.0, 3.0, 4.0]));
    assert_eq!(t.read(&b, "uI", 0, 0), i(&[-3]));
    assert_eq!(t.read(&b, "uI2", 0, 0), i(&[1, 2]));
    assert_eq!(t.read(&b, "uI3", 0, 0), i(&[4, 5, 6]));
    assert_eq!(t.read(&b, "uI4", 0, 0), i(&[7, 8, 9, 10]));
    assert_eq!(t.read(&b, "uU", 0, 0), vec![4000000000u32.to_le_bytes()]);
    assert_eq!(t.read(&b, "uU2", 0, 0), i(&[1, 2]));
    assert_eq!(t.read(&b, "uU3", 0, 0), i(&[3, 4, 5]));
    assert_eq!(t.read(&b, "uU4", 0, 0), i(&[1, 2, 3, 4]));
    assert_eq!(t.read(&b, "uB", 0, 0), i(&[1]));
    assert_eq!(t.read(&b, "uB2", 0, 0), i(&[0, 1]));
    assert_eq!(t.read(&b, "uB3", 0, 0), i(&[1, 0, 1]));
    assert_eq!(t.read(&b, "uB4", 0, 0), i(&[0, 1, 0, 1]));
    assert_eq!((t.read(&b, "uM2", 0, 0), t.read(&b, "uM2", 0, 1)), (f(&[1.0, 2.0]), f(&[3.0, 4.0])));
    assert_eq!(t.read(&b, "uM3", 0, 0), f(&[1.0, 4.0, 7.0]), "transposed");
    assert_eq!(t.read(&b, "uM3", 0, 2), f(&[3.0, 6.0, 9.0]));
    assert_eq!(t.read(&b, "uM4", 0, 0), f(&[2.0, 3.0, 4.0, 5.0]));
    assert_eq!(t.read(&b, "uM4", 0, 3), f(&[14.0, 15.0, 16.0, 17.0]));
    let arr: Vec<_> = (0..4).map(|e| t.read(&b, "uArr", e, 0)[0]).collect();
    assert_eq!(arr, f(&[1.0, 2.0, 7.0, 8.0]));
    // getUniform reads every kind back.
    let get = |t: &mut T, name: &str| {
        let l = t.g.loc(t.p, name);
        t.g.call("getUniform", &[t.p, l])
    };
    assert_eq!(get(&mut t, "uF"), Ret::Num(1.5));
    assert_eq!(get(&mut t, "uV3"), Ret::F32s(vec![1.0, 2.0, 3.0]));
    assert_eq!(get(&mut t, "uI"), Ret::Num(-3.0));
    assert_eq!(get(&mut t, "uI2"), Ret::I32s(vec![1, 2]));
    assert_eq!(get(&mut t, "uU"), Ret::Num(4000000000.0));
    assert_eq!(get(&mut t, "uU3"), Ret::U32s(vec![3, 4, 5]));
    assert_eq!(get(&mut t, "uB"), Ret::Bool(true));
    assert_eq!(get(&mut t, "uB2"), Ret::Bools(vec![false, true]));
    assert_eq!(get(&mut t, "uM2"), Ret::F32s(vec![1.0, 2.0, 3.0, 4.0]));
    assert_eq!(get(&mut t, "uArr[3]"), Ret::Num(8.0));
    assert_eq!(get(&mut t, "uS"), Ret::Num(3.0));
    t.g.no_err();
}

#[test]
fn errors_and_sharing() {
    let mut t = T::new();
    for (m, name, args) in [
        ("uniform1i", "uF", vec![n(1.0)]),
        ("uniform2f", "uF", vec![n(1.0), n(2.0)]),
        ("uniform1fv", "uF", vec![Arg::Nums(&[1.0, 2.0])]),
        ("uniform1f", "uI", vec![n(1.0)]),
        ("uniform1i", "uU", vec![n(1.0)]),
        ("uniformMatrix2x3fv", "uM2", vec![Arg::Bool(false), Arg::Nums(&[0.0; 6])]),
        ("uniform1f", "uS", vec![n(1.0)]),
        ("uniform3fv", "uV3", vec![Arg::Bytes(Elem::I32, &[0; 12])]),
    ] {
        t.set(m, name, &args);
        t.g.expect("INVALID_OPERATION");
    }
    for (m, name, args) in [
        ("uniform3fv", "uV3", vec![Arg::Nums(&[1.0, 2.0])]),
        ("uniform1i", "uS", vec![n(32.0)]),
        ("uniform4fv", "uV4", vec![Arg::Nums(&[0.0; 4]), n(2.0)]),
        ("uniform4fv", "uV4", vec![Arg::Nums(&[0.0; 4]), n(0.0), n(5.0)]),
    ] {
        t.set(m, name, &args);
        t.g.expect("INVALID_VALUE");
    }
    // Draws without a change share one allocation; a change gives a new one.
    let a = t.draw();
    let b = t.draw();
    assert!(Rc::ptr_eq(&a, &b));
    t.set("uniform1f", "uF", &[n(2.0)]);
    let c = t.draw();
    assert!(!Rc::ptr_eq(&b, &c));
    assert_eq!(t.read(&a, "uF", 0, 0), f(&[0.0]), "a handed-out buffer never changes");
    assert_eq!(t.read(&c, "uF", 0, 0), f(&[2.0]));
    // WebGL 1: no transpose.
    let mut g = Gl::new(Version::WebGl1);
    let vs = "attribute vec4 a; uniform mat2 m; void main() { gl_Position = a + vec4(m[0], m[1]); }";
    let p = g.program(vs, "void main() { gl_FragColor = vec4(1.0); }");
    let l = g.loc(p, "m");
    g.call("uniformMatrix2fv", &[l, Arg::Bool(true), Arg::Nums(&[0.0; 4])]);
    g.expect("INVALID_VALUE");
}
