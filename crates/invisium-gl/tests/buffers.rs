//! L4: buffer objects.

mod common;
use common::*;
use invisium_gl::cmd::Cmd;
use invisium_gl::*;

#[test]
fn data_sub_data_copy_and_readback() {
    let mut g = Gl::new(Version::WebGl2);
    let (a, b) = (g.obj("createBuffer"), g.obj("createBuffer"));
    let (Arg::Object(_, ha), Arg::Object(_, hb)) = (a, b) else { panic!() };
    let (ab, cw, sd) = (g.k("ARRAY_BUFFER"), g.k("COPY_WRITE_BUFFER"), g.k("STATIC_DRAW"));
    g.call("bindBuffer", &[ab, a]);
    let data: Vec<u8> = (0..16).collect();
    g.call("bufferData", &[ab, Arg::Bytes(Elem::U8, &data), sd]);
    g.call("bufferSubData", &[ab, n(4.0), Arg::Bytes(Elem::U8, &[100, 101])]);
    // WebGL 2 srcOffset / length (in elements).
    let words = [1u16, 2, 3, 4];
    let wb = u16_bytes(&words);
    g.call("bufferSubData", &[ab, n(8.0), Arg::Bytes(Elem::U16, &wb), n(1.0), n(2.0)]);
    g.call("bindBuffer", &[cw, b]);
    g.call("bufferData", &[cw, n(8.0), sd]);
    let cr = g.k("COPY_READ_BUFFER");
    g.call("bindBuffer", &[cr, a]);
    g.call("copyBufferSubData", &[cr, cw, n(2.0), n(1.0), n(4.0)]);
    g.no_err();
    let cmds = g.cmds();
    assert!(cmds.contains(&Cmd::BufferCreate { id: ha, index: false, bytes: data.clone() }));
    assert!(cmds.contains(&Cmd::BufferWrite { id: ha, offset: 4, bytes: vec![100, 101] }));
    assert!(cmds.contains(&Cmd::BufferWrite { id: ha, offset: 8, bytes: vec![2, 0, 3, 0] }));
    assert!(cmds.contains(&Cmd::BufferCreate { id: hb, index: false, bytes: vec![0; 8] }));
    assert!(cmds.contains(&Cmd::BufferCopy { src: ha, src_offset: 2, dst: hb, dst_offset: 1, size: 4 }));
    let mut out = [0u8; 16];
    let r = g.call("getBufferSubData", &[ab, n(0.0), Arg::Bytes(Elem::U8, &out)]);
    let Ret::Fill { arg: 2, offset: 0, bytes } = r else { panic!("{r:?}") };
    out.copy_from_slice(&bytes);
    assert_eq!(out, [0, 1, 2, 3, 100, 101, 6, 7, 2, 0, 3, 0, 12, 13, 14, 15]);
    let r = g.call("getBufferSubData", &[cw, n(0.0), Arg::Bytes(Elem::U8, &[0; 8]), n(2.0), n(4.0)]);
    assert_eq!(r, Ret::Fill { arg: 2, offset: 2, bytes: vec![0, 2, 3, 100] });
    let size = g.k("BUFFER_SIZE");
    assert_eq!(g.call("getBufferParameter", &[ab, size]), Ret::Num(16.0));
    let usage = g.k("BUFFER_USAGE");
    assert_eq!(g.call("getBufferParameter", &[ab, usage]), Ret::Num(g.kv("STATIC_DRAW")));
    g.no_err();
}

#[test]
fn cpu_copies_only_for_element_arrays() {
    let mut g = Gl::new(Version::WebGl2);
    let (a, e1, e2) = (g.obj("createBuffer"), g.obj("createBuffer"), g.obj("createBuffer"));
    let (ab, eb, sd) = (g.k("ARRAY_BUFFER"), g.k("ELEMENT_ARRAY_BUFFER"), g.k("STATIC_DRAW"));
    g.call("bindBuffer", &[ab, a]);
    g.call("bufferData", &[ab, Arg::Bytes(Elem::U8, &[9; 64]), sd]);
    g.cmds();
    assert_eq!(g.ctx.memory_bytes(), 0, "no CPU copy of an array buffer");
    let data: Vec<u8> = (1..=8).collect();
    g.call("bindBuffer", &[eb, e1]);
    g.call("bufferData", &[eb, Arg::Bytes(Elem::U8, &data), sd]);
    g.call("bindBuffer", &[eb, e2]);
    g.call("bufferData", &[eb, n(8.0), sd]);
    g.cmds();
    assert_eq!(g.ctx.memory_bytes(), 16);
    // Element array to element array: the destination's copy follows.
    let (cr, cw) = (g.k("COPY_READ_BUFFER"), g.k("COPY_WRITE_BUFFER"));
    g.call("bindBuffer", &[cr, e1]);
    g.call("bindBuffer", &[cw, e2]);
    g.call("copyBufferSubData", &[cr, cw, n(2.0), n(4.0), n(4.0)]);
    g.no_err();
    let reads = g.dev.buffer_reads;
    let r = g.call("getBufferSubData", &[eb, n(0.0), Arg::Bytes(Elem::U8, &[0; 8])]);
    assert_eq!(r, Ret::Fill { arg: 2, offset: 0, bytes: vec![0, 0, 0, 0, 3, 4, 5, 6] });
    assert_eq!(g.dev.buffer_reads, reads, "element arrays answer from the CPU copy");
    // An array buffer is read back from the device, after pending commands ran.
    g.call("bufferSubData", &[ab, n(1.0), Arg::Bytes(Elem::U8, &[7, 7])]);
    let r = g.call("getBufferSubData", &[ab, n(0.0), Arg::Bytes(Elem::U8, &[0; 4])]);
    assert_eq!(r, Ret::Fill { arg: 2, offset: 0, bytes: vec![9, 7, 7, 9] });
    assert_eq!(g.dev.buffer_reads, reads + 1);
    // A pixel unpack buffer feeds texImage2D through the device read-back.
    let p = g.obj("createBuffer");
    let pu = g.k("PIXEL_UNPACK_BUFFER");
    g.call("bindBuffer", &[pu, p]);
    g.call("bufferData", &[pu, Arg::Bytes(Elem::U8, &[0, 0, 0, 0, 1, 2, 3, 4]), sd]);
    g.tex("TEXTURE_2D");
    let args = [g.k("TEXTURE_2D"), n(0.0), g.k("RGBA8"), n(1.0), n(1.0), n(0.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), n(4.0)];
    g.call("texImage2D", &args);
    g.no_err();
    assert_eq!(writes(&g.cmds())[0].8, vec![1, 2, 3, 4]);
    assert_eq!(g.ctx.memory_bytes(), 16);
}

#[test]
fn out_of_range_and_errors() {
    let mut g = Gl::new(Version::WebGl2);
    let a = g.obj("createBuffer");
    let (ab, sd) = (g.k("ARRAY_BUFFER"), g.k("STATIC_DRAW"));
    g.call("bufferData", &[ab, n(4.0), sd]);
    g.expect("INVALID_OPERATION");
    g.call("bindBuffer", &[ab, a]);
    g.call("bufferData", &[ab, n(-1.0), sd]);
    g.expect("INVALID_VALUE");
    g.call("bufferData", &[ab, n(4.0), n(0x1234 as f64)]);
    g.expect("INVALID_ENUM");
    g.call("bufferData", &[ab, n(8.0), sd]);
    g.no_err();
    g.cmds();
    g.call("bufferSubData", &[ab, n(6.0), Arg::Bytes(Elem::U8, &[1, 2, 3])]);
    g.expect("INVALID_VALUE");
    g.call("bufferSubData", &[ab, n(-1.0), Arg::Bytes(Elem::U8, &[1])]);
    g.expect("INVALID_VALUE");
    g.call("getBufferSubData", &[ab, n(4.0), Arg::Bytes(Elem::U8, &[0; 8])]);
    g.expect("INVALID_VALUE");
    let cr = g.k("COPY_READ_BUFFER");
    g.call("bindBuffer", &[cr, a]);
    g.call("copyBufferSubData", &[cr, ab, n(0.0), n(2.0), n(4.0)]);
    g.expect("INVALID_VALUE");
    g.call("copyBufferSubData", &[cr, ab, n(0.0), n(6.0), n(4.0)]);
    g.expect("INVALID_VALUE");
    assert!(g.cmds().is_empty(), "failed calls emit nothing");
    g.call("bindBuffer", &[n(0x1234 as f64), a]);
    g.expect("INVALID_ENUM");
}

#[test]
fn binding_kind_rule_and_delete_unbinds() {
    for v in [Version::WebGl1, Version::WebGl2] {
        let mut g = Gl::new(v);
        let (a, e) = (g.obj("createBuffer"), g.obj("createBuffer"));
        assert_eq!(g.call("isBuffer", &[a]), Ret::Bool(false));
        let (ab, eb) = (g.k("ARRAY_BUFFER"), g.k("ELEMENT_ARRAY_BUFFER"));
        g.call("bindBuffer", &[ab, a]);
        g.call("bindBuffer", &[eb, e]);
        g.no_err();
        assert_eq!(g.call("isBuffer", &[a]), Ret::Bool(true));
        g.call("bindBuffer", &[eb, a]);
        g.expect("INVALID_OPERATION");
        g.call("bindBuffer", &[ab, e]);
        g.expect("INVALID_OPERATION");
        let Arg::Object(_, he) = e else { panic!() };
        g.call("bufferData", &[eb, Arg::Bytes(Elem::U16, &[1, 0, 2, 0]), g.k("STATIC_DRAW")]);
        assert!(g.cmds().contains(&Cmd::BufferCreate { id: he, index: true, bytes: vec![1, 0, 2, 0] }));
        if v == Version::WebGl2 {
            // Copy targets accept both kinds, but copies between kinds are refused.
            let cr = g.k("COPY_READ_BUFFER");
            g.call("bindBuffer", &[cr, e]);
            g.no_err();
            g.call("bindBuffer", &[g.k("UNIFORM_BUFFER"), e]);
            g.expect("INVALID_OPERATION");
            g.call("bindBufferBase", &[g.k("UNIFORM_BUFFER"), n(0.0), a]);
            g.no_err();
            assert_eq!(g.call("getIndexedParameter", &[g.k("UNIFORM_BUFFER_BINDING"), n(0.0)]), Ret::Object(Kind::Buffer, match a { Arg::Object(_, h) => h, _ => 0 }));
            g.call("bindBufferRange", &[g.k("UNIFORM_BUFFER"), n(1.0), a, n(3.0), n(16.0)]);
            g.expect("INVALID_VALUE");
            g.call("bindBufferBase", &[g.k("UNIFORM_BUFFER"), n(24.0), a]);
            g.expect("INVALID_VALUE");
        }
        g.call("deleteBuffer", &[a]);
        assert_eq!(g.param("ARRAY_BUFFER_BINDING"), Ret::Null);
        assert_eq!(g.call("isBuffer", &[a]), Ret::Bool(false));
        if v == Version::WebGl2 {
            assert_eq!(g.param("UNIFORM_BUFFER_BINDING"), Ret::Null);
        }
        g.call("bindBuffer", &[ab, a]);
        g.expect("INVALID_OPERATION");
        g.call("deleteBuffer", &[e]);
        assert_eq!(g.param("ELEMENT_ARRAY_BUFFER_BINDING"), Ret::Null);
        assert!(g.cmds().contains(&Cmd::BufferDelete { id: he }));
    }
}
