//! P6: stand-ins and queries; P7: WebGL 1 extension objects.

mod common;
use common::scene::*;
use common::*;
use invisium_gl::cmd::*;
use invisium_gl::*;

#[test]
fn queries_sync_and_transform_feedback() {
    let mut g = Gl::new(Version::WebGl2);
    let q = g.obj("createQuery");
    let asp = g.k("ANY_SAMPLES_PASSED");
    assert_eq!(g.call("isQuery", &[q]), Ret::Bool(false));
    g.call("beginQuery", &[asp, q]);
    g.no_err();
    assert_eq!(g.call("isQuery", &[q]), Ret::Bool(true));
    assert_eq!(g.call("getQuery", &[asp, g.k("CURRENT_QUERY")]), Ret::Object(Kind::Query, h(q)));
    g.call("beginQuery", &[g.k("ANY_SAMPLES_PASSED_CONSERVATIVE"), q]);
    g.expect("INVALID_OPERATION");
    g.call("getQueryParameter", &[q, g.k("QUERY_RESULT")]);
    g.expect("INVALID_OPERATION");
    g.call("endQuery", &[asp]);
    assert_eq!(g.call("getQueryParameter", &[q, g.k("QUERY_RESULT_AVAILABLE")]), Ret::Bool(true));
    assert_eq!(g.call("getQueryParameter", &[q, g.k("QUERY_RESULT")]), Ret::Bool(false));
    g.call("endQuery", &[asp]);
    g.expect("INVALID_OPERATION");
    let s = match g.call("fenceSync", &[g.k("SYNC_GPU_COMMANDS_COMPLETE"), n(0.0)]) {
        Ret::Object(k, h) => Arg::Object(k, h),
        r => panic!("{r:?}"),
    };
    assert_eq!(g.call("clientWaitSync", &[s, n(0.0), n(0.0)]), Ret::Num(g.kv("ALREADY_SIGNALED")));
    assert_eq!(g.call("getSyncParameter", &[s, g.k("SYNC_STATUS")]), Ret::Num(g.kv("SIGNALED")));
    g.call("waitSync", &[s, n(0.0), n(-1.0)]);
    g.no_err();
    g.call("deleteSync", &[s]);
    assert_eq!(g.call("isSync", &[s]), Ret::Bool(false));
    // Transform feedback: state only.
    let tf = g.obj("createTransformFeedback");
    g.call("bindTransformFeedback", &[g.k("TRANSFORM_FEEDBACK"), tf]);
    assert_eq!(g.param("TRANSFORM_FEEDBACK_BINDING"), Ret::Object(Kind::TransformFeedback, h(tf)));
    g.call("beginTransformFeedback", &[g.k("POINTS")]);
    g.expect("INVALID_OPERATION");
    let p = g.program("#version 300 es\nvoid main() { gl_Position = vec4(0.0); }\n", "#version 300 es\nprecision highp float;\nout vec4 o;\nvoid main() { o = vec4(1.0); }\n");
    g.call("beginTransformFeedback", &[g.k("POINTS")]);
    assert_eq!(g.param("TRANSFORM_FEEDBACK_ACTIVE"), Ret::Bool(true));
    g.call("useProgram", &[p]);
    g.expect("INVALID_OPERATION");
    g.call("pauseTransformFeedback", &[]);
    assert_eq!(g.param("TRANSFORM_FEEDBACK_PAUSED"), Ret::Bool(true));
    g.call("resumeTransformFeedback", &[]);
    g.call("endTransformFeedback", &[]);
    g.no_err();
    assert_eq!(g.call("getProgramParameter", &[p, g.k("TRANSFORM_FEEDBACK_VARYINGS")]), Ret::Num(0.0));
    // getIndexedParameter and getInternalformatParameter.
    let b = g.obj("createBuffer");
    g.call("bindBufferRange", &[g.k("UNIFORM_BUFFER"), n(2.0), b, n(256.0), n(64.0)]);
    assert_eq!(g.call("getIndexedParameter", &[g.k("UNIFORM_BUFFER_START"), n(2.0)]), Ret::Num(256.0));
    assert_eq!(g.call("getIndexedParameter", &[g.k("UNIFORM_BUFFER_SIZE"), n(2.0)]), Ret::Num(64.0));
    assert_eq!(g.call("getIndexedParameter", &[g.k("UNIFORM_BUFFER_BINDING"), n(2.0)]), Ret::Object(Kind::Buffer, h(b)));
    let rb = g.k("RENDERBUFFER");
    let s = g.k("SAMPLES");
    assert_eq!(g.call("getInternalformatParameter", &[rb, g.k("RGBA8"), s]), Ret::I32s(vec![4]));
    assert_eq!(g.call("getInternalformatParameter", &[rb, g.k("RGBA8UI"), s]), Ret::I32s(vec![]));
    g.call("getInternalformatParameter", &[rb, g.k("RGBA32F"), s]);
    g.expect("INVALID_ENUM");
    g.call("getExtension", &[Arg::Str("EXT_color_buffer_float")]);
    assert_eq!(g.call("getInternalformatParameter", &[rb, g.k("RGBA32F"), s]), Ret::I32s(vec![4]));
    // Uniform-block API on a program without blocks.
    assert_eq!(g.call("getUniformBlockIndex", &[p, Arg::Str("Nope")]), Ret::Num(g.kv("INVALID_INDEX")));
    assert_eq!(g.call("getProgramParameter", &[p, g.k("ACTIVE_UNIFORM_BLOCKS")]), Ret::Num(0.0));
    g.call("uniformBlockBinding", &[p, n(0.0), n(1.0)]);
    g.expect("INVALID_VALUE");
    g.call("getActiveUniformBlockParameter", &[p, n(0.0), g.k("UNIFORM_BLOCK_BINDING")]);
    g.expect("INVALID_VALUE");
    g.no_err();
}

fn ext_op(v: Version, ext: &str, name: &str) -> u16 {
    extension(v, ext).unwrap().methods.iter().find(|m| m.0 == name).unwrap().1
}

#[test]
fn webgl1_extension_objects() {
    let mut s = scene(Attributes::default());
    let mut g = Gl::new(Version::WebGl1);
    let p = g.program(VS, FS);
    let loc = |g: &mut Gl, n: &str| match g.call("getAttribLocation", &[p, Arg::Str(n)]) {
        Ret::Num(v) => v,
        _ => panic!(),
    };
    let (pos, uv) = (loc(&mut g, "aPos"), loc(&mut g, "aUv"));
    let v = Version::WebGl1;
    // Without getExtension the ops are not reachable.
    let create_vao = ext_op(v, "OES_vertex_array_object", "createVertexArrayOES");
    assert_eq!(g.ctx.call(&mut g.dev, create_vao, &[]), Ret::Undefined);
    for e in ["OES_vertex_array_object", "ANGLE_instanced_arrays", "WEBGL_draw_buffers"] {
        g.call("getExtension", &[Arg::Str(e)]);
    }
    let vao = match g.ctx.call(&mut g.dev, create_vao, &[]) {
        Ret::Object(Kind::VertexArray, h) => Arg::Object(Kind::VertexArray, h),
        r => panic!("{r:?}"),
    };
    g.ctx.call(&mut g.dev, ext_op(v, "OES_vertex_array_object", "bindVertexArrayOES"), &[vao]);
    assert_eq!(g.ctx.call(&mut g.dev, ext_op(v, "OES_vertex_array_object", "isVertexArrayOES"), &[vao]), Ret::Bool(true));
    let vab = n(0x85B5 as f64);
    assert_eq!(g.call("getParameter", &[vab]), Ret::Object(Kind::VertexArray, h(vao)));
    let b = g.obj("createBuffer");
    g.call("bindBuffer", &[g.k("ARRAY_BUFFER"), b]);
    g.call("bufferData", &[g.k("ARRAY_BUFFER"), n(80.0), g.k("STATIC_DRAW")]);
    for (l, size, off) in [(pos, 3.0, 0.0), (uv, 2.0, 12.0)] {
        g.call("vertexAttribPointer", &[n(l), n(size), g.k("FLOAT"), Arg::Bool(false), n(20.0), n(off)]);
        g.call("enableVertexAttribArray", &[n(l)]);
    }
    let divisor = ext_op(v, "ANGLE_instanced_arrays", "vertexAttribDivisorANGLE");
    g.ctx.call(&mut g.dev, divisor, &[n(uv), n(1.0)]);
    g.cmds();
    let tri = g.k("TRIANGLES");
    g.ctx.call(&mut g.dev, ext_op(v, "ANGLE_instanced_arrays", "drawArraysInstancedANGLE"), &[tri, n(0.0), n(3.0), n(4.0)]);
    g.no_err();
    let d1 = draws(&g.cmds());
    // The same draw through WebGL 2's op gives the same command.
    s.g.call("vertexAttribDivisor", &[n(s.uv as f64), n(1.0)]);
    let d2 = s.one("drawArraysInstanced", &[tri, n(0.0), n(3.0), n(4.0)]);
    assert_eq!((d1[0].instances, d1[0].count, d1[0].attrs.len()), (d2.instances, d2.count, d2.attrs.len()));
    // All arrays instanced: ANGLE refuses.
    g.ctx.call(&mut g.dev, divisor, &[n(pos), n(1.0)]);
    g.ctx.call(&mut g.dev, ext_op(v, "ANGLE_instanced_arrays", "drawArraysInstancedANGLE"), &[tri, n(0.0), n(3.0), n(4.0)]);
    g.expect("INVALID_OPERATION");
    // drawBuffersWEBGL on the drawing buffer.
    let back = g.k("BACK");
    let none = [g.kv("NONE")];
    g.ctx.call(&mut g.dev, ext_op(v, "WEBGL_draw_buffers", "drawBuffersWEBGL"), &[Arg::Nums(&none)]);
    g.no_err();
    assert_eq!(g.call("getParameter", &[n(0x8825 as f64)]), Ret::Num(0.0));
    g.ctx.call(&mut g.dev, ext_op(v, "WEBGL_draw_buffers", "drawBuffersWEBGL"), &[Arg::Nums(&[0x8CE0 as f64])]);
    g.expect("INVALID_OPERATION");
    let _ = (back, Topology::Points);
    g.ctx.call(&mut g.dev, ext_op(v, "OES_vertex_array_object", "deleteVertexArrayOES"), &[vao]);
    assert_eq!(g.call("getParameter", &[vab]), Ret::Null);
}
