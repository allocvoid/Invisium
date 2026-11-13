//! P4: vertex state and vertex array objects.

mod common;
use common::*;
use invisium_gl::*;

fn attrib(g: &mut Gl, i: f64, pname: &str) -> Ret {
    let p = g.k(pname);
    g.call("getVertexAttrib", &[n(i), p])
}

#[test]
fn vertex_arrays_keep_separate_state() {
    let mut g = Gl::new(Version::WebGl2);
    let (b, e) = (g.obj("createBuffer"), g.obj("createBuffer"));
    let (ab, eb) = (g.k("ARRAY_BUFFER"), g.k("ELEMENT_ARRAY_BUFFER"));
    g.call("bindBuffer", &[ab, b]);
    g.call("bindBuffer", &[eb, e]);
    g.call("vertexAttribPointer", &[n(1.0), n(3.0), g.k("FLOAT"), Arg::Bool(false), n(12.0), n(4.0)]);
    g.call("enableVertexAttribArray", &[n(1.0)]);
    let v = g.obj("createVertexArray");
    assert_eq!(g.call("isVertexArray", &[v]), Ret::Bool(false));
    g.call("bindVertexArray", &[v]);
    assert_eq!(g.call("isVertexArray", &[v]), Ret::Bool(true));
    assert_eq!(g.param("VERTEX_ARRAY_BINDING"), Ret::Object(Kind::VertexArray, match v { Arg::Object(_, h) => h, _ => 0 }));
    assert_eq!(g.param("ELEMENT_ARRAY_BUFFER_BINDING"), Ret::Null);
    assert_eq!(attrib(&mut g, 1.0, "VERTEX_ATTRIB_ARRAY_ENABLED"), Ret::Bool(false));
    assert_eq!(attrib(&mut g, 1.0, "VERTEX_ATTRIB_ARRAY_BUFFER_BINDING"), Ret::Null);
    assert_eq!(g.param("ARRAY_BUFFER_BINDING"), Ret::Object(Kind::Buffer, match b { Arg::Object(_, h) => h, _ => 0 }), "not vertex array state");
    g.call("vertexAttribIPointer", &[n(2.0), n(2.0), g.k("UNSIGNED_SHORT"), n(0.0), n(8.0)]);
    g.call("vertexAttribDivisor", &[n(2.0), n(3.0)]);
    g.call("bindVertexArray", &[Arg::Null]);
    assert!(matches!(g.param("ELEMENT_ARRAY_BUFFER_BINDING"), Ret::Object(..)));
    assert_eq!(attrib(&mut g, 1.0, "VERTEX_ATTRIB_ARRAY_ENABLED"), Ret::Bool(true));
    assert_eq!(attrib(&mut g, 1.0, "VERTEX_ATTRIB_ARRAY_STRIDE"), Ret::Num(12.0));
    assert_eq!(attrib(&mut g, 1.0, "VERTEX_ATTRIB_ARRAY_SIZE"), Ret::Num(3.0));
    assert_eq!(g.call("getVertexAttribOffset", &[n(1.0), g.k("VERTEX_ATTRIB_ARRAY_POINTER")]), Ret::Num(4.0));
    assert_eq!(attrib(&mut g, 2.0, "VERTEX_ATTRIB_ARRAY_DIVISOR"), Ret::Num(0.0));
    g.call("bindVertexArray", &[v]);
    assert_eq!(attrib(&mut g, 2.0, "VERTEX_ATTRIB_ARRAY_DIVISOR"), Ret::Num(3.0));
    assert_eq!(attrib(&mut g, 2.0, "VERTEX_ATTRIB_ARRAY_INTEGER"), Ret::Bool(true));
    assert_eq!(attrib(&mut g, 2.0, "VERTEX_ATTRIB_ARRAY_TYPE"), Ret::Num(g.kv("UNSIGNED_SHORT")));
    // Deleting the bound vertex array binds the default one.
    g.call("deleteVertexArray", &[v]);
    assert_eq!(g.param("VERTEX_ARRAY_BINDING"), Ret::Null);
    assert!(matches!(g.param("ELEMENT_ARRAY_BUFFER_BINDING"), Ret::Object(..)));
    g.call("bindVertexArray", &[v]);
    g.expect("INVALID_OPERATION");
    g.no_err();
}

#[test]
fn pointer_validation_and_current_values() {
    let mut g = Gl::new(Version::WebGl2);
    let fl = g.k("FLOAT");
    g.call("vertexAttribPointer", &[n(0.0), n(4.0), fl, Arg::Bool(false), n(0.0), n(4.0)]);
    g.expect("INVALID_OPERATION");
    let b = g.obj("createBuffer");
    g.call("bindBuffer", &[g.k("ARRAY_BUFFER"), b]);
    for (args, err) in [
        (vec![n(16.0), n(4.0), fl, Arg::Bool(false), n(0.0), n(0.0)], "INVALID_VALUE"),
        (vec![n(0.0), n(5.0), fl, Arg::Bool(false), n(0.0), n(0.0)], "INVALID_VALUE"),
        (vec![n(0.0), n(4.0), fl, Arg::Bool(false), n(256.0), n(0.0)], "INVALID_VALUE"),
        (vec![n(0.0), n(4.0), fl, Arg::Bool(false), n(0.0), n(-4.0)], "INVALID_VALUE"),
        (vec![n(0.0), n(4.0), fl, Arg::Bool(false), n(6.0), n(0.0)], "INVALID_OPERATION"),
        (vec![n(0.0), n(4.0), fl, Arg::Bool(false), n(0.0), n(2.0)], "INVALID_OPERATION"),
        (vec![n(0.0), n(3.0), g.k("INT_2_10_10_10_REV"), Arg::Bool(true), n(0.0), n(0.0)], "INVALID_OPERATION"),
        (vec![n(0.0), n(4.0), n(0x1234 as f64), Arg::Bool(false), n(0.0), n(0.0)], "INVALID_ENUM"),
    ] {
        g.call("vertexAttribPointer", &args);
        g.expect(err);
    }
    g.call("vertexAttribIPointer", &[n(0.0), n(4.0), fl, n(0.0), n(0.0)]);
    g.expect("INVALID_ENUM");
    g.call("enableVertexAttribArray", &[n(16.0)]);
    g.expect("INVALID_VALUE");
    // Current values.
    assert_eq!(attrib(&mut g, 3.0, "CURRENT_VERTEX_ATTRIB"), Ret::F32s(vec![0.0, 0.0, 0.0, 1.0]));
    g.call("vertexAttrib2f", &[n(3.0), n(5.0), n(6.0)]);
    assert_eq!(attrib(&mut g, 3.0, "CURRENT_VERTEX_ATTRIB"), Ret::F32s(vec![5.0, 6.0, 0.0, 1.0]));
    g.call("vertexAttrib3fv", &[n(3.0), Arg::Nums(&[1.0, 2.0, 3.0])]);
    assert_eq!(attrib(&mut g, 3.0, "CURRENT_VERTEX_ATTRIB"), Ret::F32s(vec![1.0, 2.0, 3.0, 1.0]));
    g.call("vertexAttribI4i", &[n(4.0), n(-1.0), n(2.0), n(3.0), n(4.0)]);
    assert_eq!(attrib(&mut g, 4.0, "CURRENT_VERTEX_ATTRIB"), Ret::I32s(vec![-1, 2, 3, 4]));
    g.call("vertexAttribI4uiv", &[n(5.0), Arg::Nums(&[1.0, 2.0, 3.0, 4.0])]);
    assert_eq!(attrib(&mut g, 5.0, "CURRENT_VERTEX_ATTRIB"), Ret::U32s(vec![1, 2, 3, 4]));
    g.call("vertexAttrib4fv", &[n(3.0), Arg::Nums(&[1.0])]);
    g.expect("INVALID_VALUE");
    g.call("vertexAttrib1f", &[n(16.0), n(1.0)]);
    g.expect("INVALID_VALUE");
    g.call("getVertexAttrib", &[n(0.0), n(0x1234 as f64)]);
    g.expect("INVALID_ENUM");
    // Deleting a buffer unbinds it from the vertex array.
    g.call("vertexAttribPointer", &[n(0.0), n(4.0), fl, Arg::Bool(false), n(0.0), n(0.0)]);
    g.call("deleteBuffer", &[b]);
    assert_eq!(attrib(&mut g, 0.0, "VERTEX_ATTRIB_ARRAY_BUFFER_BINDING"), Ret::Null);
    g.no_err();
}
