//! P5: draws — the full command, the draw-time rules, rewritten topologies.

mod common;
use common::scene::*;
use common::*;
use invisium_gl::cmd::*;
use invisium_gl::*;

#[test]
fn full_draw_commands() {
    let mut s = scene(Attributes::default());
    let tri = s.g.k("TRIANGLES");
    let us = s.g.k("UNSIGNED_SHORT");
    let d = s.one("drawArrays", &[tri, n(1.0), n(3.0)]);
    assert_eq!(d.uniforms.len(), reflect(VS, FS, true).uniform_bytes as usize);
    assert!(d.uniforms.iter().all(|&b| b == 0));
    assert_eq!(d, Draw { first: 1, count: 3, ..s.expected(&d, 0) });
    let d = s.one("drawElements", &[tri, n(3.0), us, n(2.0)]);
    let idx = IndexSource { buffer: h(s.ebuf), offset: 2, wide: false };
    assert_eq!(d, Draw { index: Some(idx), count: 3, ..s.expected(&d, 0) });
    let d = s.one("drawRangeElements", &[tri, n(0.0), n(3.0), n(6.0), us, n(0.0)]);
    assert_eq!(d, Draw { index: Some(IndexSource { offset: 0, ..idx }), count: 6, ..s.expected(&d, 0) });
    s.g.call("vertexAttribDivisor", &[n(s.uv as f64), n(1.0)]);
    let d = s.one("drawArraysInstanced", &[tri, n(0.0), n(3.0), n(4.0)]);
    assert_eq!(d, Draw { count: 3, instances: 4, ..s.expected(&d, 1) });
    let d = s.one("drawElementsInstanced", &[tri, n(6.0), us, n(0.0), n(2.0)]);
    assert_eq!(d, Draw { index: Some(IndexSource { offset: 0, ..idx }), count: 6, instances: 2, ..s.expected(&d, 1) });
    // Too many instances for the per-instance array.
    s.draw("drawArraysInstanced", &[tri, n(0.0), n(3.0), n(5.0)]);
    s.g.expect("INVALID_OPERATION");
    // A disabled array is a constant with the current value.
    s.g.call("disableVertexAttribArray", &[n(s.uv as f64)]);
    s.g.call("vertexAttrib2f", &[n(s.uv as f64), n(0.5), n(0.25)]);
    let d = s.one("drawArrays", &[tri, n(0.0), n(3.0)]);
    assert_eq!(d.consts, vec![ConstAttr { location: s.uv, value: ConstValue::F([0.5, 0.25, 0.0, 1.0]) }]);
    assert_eq!(d.attrs, vec![s.attr(s.pos, 0)]);
    assert!(s.g.ctx.present(&mut s.g.dev));
}

fn fails(s: &mut Scene, name: &str, args: &[Arg], err: &str) {
    let cmds = s.draw(name, args);
    s.g.expect(err);
    assert!(draws(&cmds).is_empty(), "{name}: no command");
}

#[test]
fn draw_time_rules() {
    let mut s = scene(Attributes::default());
    let (tri, us, ub) = (s.g.k("TRIANGLES"), s.g.k("UNSIGNED_SHORT"), s.g.k("UNSIGNED_BYTE"));
    fails(&mut s, "drawArrays", &[tri, n(0.0), n(5.0)], "INVALID_OPERATION");
    fails(&mut s, "drawArrays", &[tri, n(-1.0), n(3.0)], "INVALID_VALUE");
    fails(&mut s, "drawArrays", &[n(7.0), n(0.0), n(3.0)], "INVALID_ENUM");
    fails(&mut s, "drawElements", &[tri, n(7.0), us, n(0.0)], "INVALID_OPERATION");
    fails(&mut s, "drawElements", &[tri, n(3.0), us, n(1.0)], "INVALID_OPERATION");
    fails(&mut s, "drawElements", &[tri, n(3.0), n(0x1234 as f64), n(0.0)], "INVALID_ENUM");
    // An index outside the vertex data.
    s.g.call("bufferSubData", &[s.g.k("ELEMENT_ARRAY_BUFFER"), n(10.0), Arg::Bytes(Elem::U16, &u16_bytes(&[9]))]);
    fails(&mut s, "drawElements", &[tri, n(6.0), us, n(0.0)], "INVALID_OPERATION");
    // The primitive restart index (WebGL 2) is not a vertex.
    s.g.call("bufferSubData", &[s.g.k("ELEMENT_ARRAY_BUFFER"), n(10.0), Arg::Bytes(Elem::U16, &u16_bytes(&[0xFFFF]))]);
    assert_eq!(draws(&s.draw("drawElements", &[tri, n(6.0), us, n(0.0)])).len(), 1);
    // Count 0 queues nothing and is not an error.
    assert!(s.draw("drawArrays", &[tri, n(0.0), n(0.0)]).is_empty());
    assert!(s.draw("drawArraysInstanced", &[tri, n(0.0), n(3.0), n(0.0)]).is_empty());
    s.g.no_err();
    // An integer array for a float attribute; a constant of the wrong type.
    s.g.call("vertexAttribIPointer", &[n(s.uv as f64), n(2.0), s.g.k("INT"), n(20.0), n(12.0)]);
    fails(&mut s, "drawArrays", &[tri, n(0.0), n(3.0)], "INVALID_OPERATION");
    s.g.call("disableVertexAttribArray", &[n(s.uv as f64)]);
    s.g.call("vertexAttribI4i", &[n(s.uv as f64), n(1.0), n(1.0), n(1.0), n(1.0)]);
    fails(&mut s, "drawArrays", &[tri, n(0.0), n(3.0)], "INVALID_OPERATION");
    s.g.call("vertexAttrib4f", &[n(s.uv as f64), n(0.0), n(0.0), n(0.0), n(1.0)]);
    // An enabled array without a buffer.
    s.g.call("bindBuffer", &[s.g.k("ARRAY_BUFFER"), Arg::Null]);
    s.g.call("vertexAttribPointer", &[n(s.uv as f64), n(2.0), s.g.k("FLOAT"), Arg::Bool(false), n(0.0), n(0.0)]);
    s.g.call("enableVertexAttribArray", &[n(s.uv as f64)]);
    fails(&mut s, "drawArrays", &[tri, n(0.0), n(3.0)], "INVALID_OPERATION");
    s.g.call("disableVertexAttribArray", &[n(s.uv as f64)]);
    // No element buffer; no program.
    s.g.call("bindBuffer", &[s.g.k("ELEMENT_ARRAY_BUFFER"), Arg::Null]);
    fails(&mut s, "drawElements", &[tri, n(3.0), us, n(0.0)], "INVALID_OPERATION");
    s.g.call("useProgram", &[Arg::Null]);
    fails(&mut s, "drawArrays", &[tri, n(0.0), n(3.0)], "INVALID_OPERATION");
    s.g.call("useProgram", &[s.prog]);
    // Stencil reference differing between faces (with a stencil buffer).
    let mut st = scene(Attributes { stencil: true, ..Attributes::default() });
    let (back, al) = (st.g.k("BACK"), st.g.k("ALWAYS"));
    st.g.call("stencilFuncSeparate", &[back, al, n(1.0), n(255.0)]);
    fails(&mut st, "drawArrays", &[tri, n(0.0), n(3.0)], "INVALID_OPERATION");
    // Without a stencil buffer it does not matter.
    s.g.call("stencilFuncSeparate", &[back, al, n(1.0), n(255.0)]);
    assert_eq!(draws(&s.draw("drawArrays", &[tri, n(0.0), n(3.0)])).len(), 1);
    // 8-bit indices are widened into the context's own index buffer.
    let e8 = s.g.obj("createBuffer");
    s.g.call("bindBuffer", &[s.g.k("ELEMENT_ARRAY_BUFFER"), e8]);
    s.g.call("bufferData", &[s.g.k("ELEMENT_ARRAY_BUFFER"), Arg::Bytes(Elem::U8, &[3, 2, 1]), s.g.k("STATIC_DRAW")]);
    let cmds = s.draw("drawElements", &[tri, n(3.0), ub, n(0.0)]);
    s.g.no_err();
    let Cmd::BufferCreate { id, index: true, bytes } = &cmds[0] else { panic!("{cmds:?}") };
    assert_eq!(bytes, &u16_bytes(&[3, 2, 1]));
    let d = &draws(&cmds)[0];
    assert_eq!((d.index, d.first, d.count), (Some(IndexSource { buffer: *id, offset: 0, wide: false }), 0, 3));
}

#[test]
fn rewritten_topologies() {
    let mut s = scene(Attributes::default());
    let cmds = s.draw("drawArrays", &[s.g.k("LINE_LOOP"), n(1.0), n(3.0)]);
    let Cmd::BufferCreate { id, index: true, bytes } = &cmds[0] else { panic!() };
    assert_eq!(bytes, &u16_bytes(&[1, 2, 3, 1]));
    let d = &draws(&cmds)[0];
    assert_eq!((d.state.topology, d.index, d.first, d.count), (Topology::LineStrip, Some(IndexSource { buffer: *id, offset: 0, wide: false }), 0, 4));
    let cmds = s.draw("drawArrays", &[s.g.k("TRIANGLE_FAN"), n(0.0), n(4.0)]);
    let Cmd::BufferCreate { bytes, .. } = &cmds[0] else { panic!() };
    assert_eq!(bytes, &u16_bytes(&[0, 1, 2, 0, 2, 3]));
    assert_eq!((draws(&cmds)[0].state.topology, draws(&cmds)[0].count), (Topology::Triangles, 6));
    let cmds = s.draw("drawElements", &[s.g.k("TRIANGLE_FAN"), n(4.0), s.g.k("UNSIGNED_SHORT"), n(4.0)]);
    let Cmd::BufferCreate { bytes, .. } = &cmds[0] else { panic!() };
    assert_eq!(bytes, &u16_bytes(&[2, 2, 1, 2, 1, 3]));
    assert!(s.draw("drawArrays", &[s.g.k("TRIANGLE_FAN"), n(0.0), n(2.0)]).is_empty());
    s.g.no_err();
}
