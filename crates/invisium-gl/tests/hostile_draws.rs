//! Milestone 2 fix: hostile draw sizes stay inside the budget; the index scan cache; a
//! failed relink of the program in use keeps its locations.

mod common;
use common::scene::*;
use common::*;
use invisium_gl::cmd::*;
use invisium_gl::*;

const BIG: f64 = 2147483647.0;
const OVER: f64 = 2147483648.0;

const VS0: &str = "#version 300 es\nvoid main() { gl_Position = vec4(0.0, 0.0, 0.0, 1.0); gl_PointSize = 1.0; }\n";
const FS0: &str = "#version 300 es\nprecision highp float;\nuniform vec4 uC;\nout vec4 o;\nvoid main() { o = uC; }\n";

/// Run a draw; return the error and how many draws and index bytes were queued.
fn attempt(g: &mut Gl, name: &str, args: &[Arg]) -> (f64, usize, usize) {
    g.cmds();
    g.call(name, args);
    let cmds = g.cmds();
    let bytes = cmds.iter().map(|c| if let Cmd::BufferCreate { bytes, .. } = c { bytes.len() } else { 0 }).sum();
    (g.err(), draws(&cmds).len(), bytes)
}

fn check(g: &mut Gl, name: &str, args: &[Arg], err: &str, drawn: usize) {
    let want = if err.is_empty() { 0.0 } else { g.kv(err) };
    let (e, d, bytes) = attempt(g, name, args);
    assert_eq!((e, d), (want, drawn), "{name} {args:?}");
    assert!(bytes < 1 << 20, "{name}: {bytes} bytes queued");
    assert!(g.ctx.memory_bytes() < 1 << 20, "{name}: memory {}", g.ctx.memory_bytes());
}

#[test]
fn huge_counts_with_attributes() {
    let mut s = scene(Attributes::default());
    let g = &mut s.g;
    let (tri, ll, fan, ub, us) = (g.k("TRIANGLES"), g.k("LINE_LOOP"), g.k("TRIANGLE_FAN"), g.k("UNSIGNED_BYTE"), g.k("UNSIGNED_SHORT"));
    for mode in [tri, ll, fan] {
        check(g, "drawArrays", &[mode, n(0.0), n(BIG)], "INVALID_OPERATION", 0);
        check(g, "drawArrays", &[mode, n(BIG), n(3.0)], "INVALID_OPERATION", 0);
        check(g, "drawArrays", &[mode, n(0.0), n(OVER)], "INVALID_VALUE", 0);
        check(g, "drawArrays", &[mode, n(OVER), n(3.0)], "INVALID_VALUE", 0);
        check(g, "drawArraysInstanced", &[mode, n(0.0), n(3.0), n(BIG)], "", 1);
        check(g, "drawArraysInstanced", &[mode, n(0.0), n(3.0), n(OVER)], "INVALID_VALUE", 0);
        check(g, "drawElements", &[mode, n(BIG), us, n(0.0)], "INVALID_OPERATION", 0);
        check(g, "drawElements", &[mode, n(OVER), us, n(0.0)], "INVALID_VALUE", 0);
        check(g, "drawElements", &[mode, n(3.0), us, n(BIG)], "INVALID_OPERATION", 0);
        check(g, "drawElementsInstanced", &[mode, n(6.0), us, n(0.0), n(BIG)], "", 1);
    }
    // 8-bit indices.
    let e8 = g.obj("createBuffer");
    g.call("bindBuffer", &[g.k("ELEMENT_ARRAY_BUFFER"), e8]);
    g.call("bufferData", &[g.k("ELEMENT_ARRAY_BUFFER"), Arg::Bytes(Elem::U8, &[0, 1, 2, 3]), g.k("STATIC_DRAW")]);
    for mode in [tri, ll, fan] {
        check(g, "drawElements", &[mode, n(BIG), ub, n(0.0)], "INVALID_OPERATION", 0);
        check(g, "drawElements", &[mode, n(OVER), ub, n(0.0)], "INVALID_VALUE", 0);
        check(g, "drawElements", &[mode, n(1.0), ub, n(BIG)], "INVALID_OPERATION", 0);
        check(g, "drawElementsInstanced", &[mode, n(4.0), ub, n(0.0), n(BIG)], "", 1);
    }
    // A per-instance array cannot cover 2^31 - 1 instances.
    g.call("vertexAttribDivisor", &[n(s.uv as f64), n(1.0)]);
    check(&mut s.g, "drawArraysInstanced", &[tri, n(0.0), n(3.0), n(BIG)], "INVALID_OPERATION", 0);
}

#[test]
fn huge_counts_without_attributes() {
    let mut g = Gl::new(Version::WebGl2);
    g.program(VS0, FS0);
    let (tri, ll, fan, ub) = (g.k("TRIANGLES"), g.k("LINE_LOOP"), g.k("TRIANGLE_FAN"), g.k("UNSIGNED_BYTE"));
    check(&mut g, "drawArrays", &[tri, n(0.0), n(BIG)], "", 1);
    check(&mut g, "drawArrays", &[tri, n(BIG), n(BIG)], "", 1);
    check(&mut g, "drawArraysInstanced", &[tri, n(0.0), n(BIG), n(BIG)], "", 1);
    for mode in [ll, fan] {
        check(&mut g, "drawArrays", &[mode, n(0.0), n(BIG)], "OUT_OF_MEMORY", 0);
        check(&mut g, "drawArrays", &[mode, n(BIG), n(BIG)], "OUT_OF_MEMORY", 0);
        check(&mut g, "drawArraysInstanced", &[mode, n(0.0), n(BIG), n(BIG)], "OUT_OF_MEMORY", 0);
        check(&mut g, "drawArrays", &[mode, n(BIG), n(3.0)], "", 1);
        check(&mut g, "drawArraysInstanced", &[mode, n(0.0), n(3.0), n(BIG)], "", 1);
    }
    for mode in [tri, ll, fan] {
        check(&mut g, "drawArrays", &[mode, n(0.0), n(OVER)], "INVALID_VALUE", 0);
        check(&mut g, "drawArrays", &[mode, n(OVER), n(1.0)], "INVALID_VALUE", 0);
        check(&mut g, "drawArraysInstanced", &[mode, n(0.0), n(3.0), n(OVER)], "INVALID_VALUE", 0);
    }
    // A rewrite that would take a vertex index past 0xFFFF uses 32-bit indices.
    let cmds = {
        g.cmds();
        g.call("drawArrays", &[ll, n(BIG), n(2.0)]);
        g.cmds()
    };
    let Cmd::BufferCreate { bytes, .. } = &cmds[0] else { panic!() };
    assert_eq!(bytes, &[2147483647u32, 2147483648, 2147483647].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
    // 8-bit indices.
    let e8 = g.obj("createBuffer");
    g.call("bindBuffer", &[g.k("ELEMENT_ARRAY_BUFFER"), e8]);
    g.call("bufferData", &[g.k("ELEMENT_ARRAY_BUFFER"), Arg::Bytes(Elem::U8, &[0, 1, 2, 0xFF]), g.k("STATIC_DRAW")]);
    for mode in [tri, ll, fan] {
        check(&mut g, "drawElements", &[mode, n(BIG), ub, n(0.0)], "INVALID_OPERATION", 0);
        check(&mut g, "drawElements", &[mode, n(OVER), ub, n(0.0)], "INVALID_VALUE", 0);
        check(&mut g, "drawElementsInstanced", &[mode, n(4.0), ub, n(0.0), n(BIG)], "", 1);
    }
}

#[test]
fn index_scan_cache_follows_buffer_changes() {
    let mut s = scene(Attributes::default());
    let (tri, us) = (s.g.k("TRIANGLES"), s.g.k("UNSIGNED_SHORT"));
    for _ in 0..2 {
        s.one("drawElements", &[tri, n(6.0), us, n(0.0)]);
    }
    s.g.call("bufferSubData", &[s.g.k("ELEMENT_ARRAY_BUFFER"), n(0.0), Arg::Bytes(Elem::U16, &u16_bytes(&[4]))]);
    s.draw("drawElements", &[tri, n(6.0), us, n(0.0)]);
    s.g.expect("INVALID_OPERATION");
    s.g.call("bufferSubData", &[s.g.k("ELEMENT_ARRAY_BUFFER"), n(0.0), Arg::Bytes(Elem::U16, &u16_bytes(&[3]))]);
    s.one("drawElements", &[tri, n(6.0), us, n(0.0)]);
    // copyBufferSubData into the element buffer also invalidates.
    let eb = s.g.k("ELEMENT_ARRAY_BUFFER");
    let src = s.g.obj("createBuffer");
    s.g.call("bindBuffer", &[eb, src]);
    s.g.call("bufferData", &[eb, Arg::Bytes(Elem::U16, &u16_bytes(&[9])), s.g.k("STATIC_DRAW")]);
    s.g.call("bindBuffer", &[eb, s.ebuf]);
    s.g.call("bindBuffer", &[s.g.k("COPY_READ_BUFFER"), src]);
    s.g.call("bindBuffer", &[s.g.k("COPY_WRITE_BUFFER"), s.ebuf]);
    s.g.call("copyBufferSubData", &[s.g.k("COPY_READ_BUFFER"), s.g.k("COPY_WRITE_BUFFER"), n(0.0), n(0.0), n(2.0)]);
    s.g.no_err();
    s.draw("drawElements", &[tri, n(6.0), us, n(0.0)]);
    s.g.expect("INVALID_OPERATION");
}

#[test]
fn failed_relink_keeps_locations_of_the_executable_in_use() {
    let mut g = Gl::new(Version::WebGl2);
    let p = g.program(VS0, FS0);
    let l = g.loc(p, "uC");
    // Relink with a fragment shader that does not compile: the link fails.
    let Ret::Objects(_, list) = g.call("getAttachedShaders", &[p]) else { panic!() };
    let fs = Arg::Object(Kind::Shader, list[1]);
    g.call("shaderSource", &[fs, Arg::Str("#version 300 es\nnope\n")]);
    g.call("compileShader", &[fs]);
    g.call("linkProgram", &[p]);
    assert_eq!(g.call("getProgramParameter", &[p, g.k("LINK_STATUS")]), Ret::Bool(false));
    g.call("uniform4f", &[l, n(1.0), n(2.0), n(3.0), n(4.0)]);
    g.no_err();
    g.cmds();
    g.call("drawArrays", &[g.k("POINTS"), n(0.0), n(1.0)]);
    g.no_err();
    let d = draws(&g.cmds()).pop().expect("the old executable still draws");
    assert_eq!(&d.uniforms[..16], &f32_bytes(&[1.0, 2.0, 3.0, 4.0])[..]);
    // Once another program is used, the old executable and its locations are gone.
    let q = g.program(VS0, FS0);
    g.call("useProgram", &[p]);
    g.expect("INVALID_OPERATION");
    g.call("uniform4f", &[l, n(0.0), n(0.0), n(0.0), n(0.0)]);
    g.expect("INVALID_OPERATION");
    let _ = q;
}
