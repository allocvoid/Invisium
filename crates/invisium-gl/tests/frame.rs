//! P9: one whole frame as a library would issue it; the exact command list.

mod common;
use common::scene::{FS, VS};
use common::*;
use invisium_gl::cmd::*;
use invisium_gl::*;
use invisium_glsl::Place;

fn h(a: Arg) -> u32 {
    if let Arg::Object(_, h) = a {
        h
    } else {
        panic!()
    }
}

#[test]
fn one_frame() {
    let attrs = Attributes { antialias: false, ..Attributes::default() };
    let mut g = Gl::with(Version::WebGl2, attrs, 4, 4);
    let k = |g: &Gl, s: &str| g.k(s);
    // Program.
    let prog = g.program(VS, FS);
    let r = reflect(VS, FS, true);
    let loc_of = |g: &mut Gl, n: &str| match g.call("getAttribLocation", &[prog, Arg::Str(n)]) {
        Ret::Num(v) => v as u32,
        _ => panic!(),
    };
    let (pos, uv) = (loc_of(&mut g, "aPos"), loc_of(&mut g, "aUv"));
    // Vertex array with an interleaved buffer and 16-bit indices.
    let vao = g.obj("createVertexArray");
    g.call("bindVertexArray", &[vao]);
    let vb = g.obj("createBuffer");
    g.call("bindBuffer", &[k(&g, "ARRAY_BUFFER"), vb]);
    let verts: Vec<f32> = vec![-1.0, -1.0, 0.0, 0.0, 0.0, 1.0, -1.0, 0.0, 1.0, 0.0, -1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0];
    let vbytes = f32_bytes(&verts);
    g.call("bufferData", &[k(&g, "ARRAY_BUFFER"), Arg::Bytes(Elem::F32, &vbytes), k(&g, "STATIC_DRAW")]);
    g.call("vertexAttribPointer", &[n(pos as f64), n(3.0), k(&g, "FLOAT"), Arg::Bool(false), n(20.0), n(0.0)]);
    g.call("vertexAttribPointer", &[n(uv as f64), n(2.0), k(&g, "FLOAT"), Arg::Bool(false), n(20.0), n(12.0)]);
    g.call("enableVertexAttribArray", &[n(pos as f64)]);
    g.call("enableVertexAttribArray", &[n(uv as f64)]);
    let eb = g.obj("createBuffer");
    g.call("bindBuffer", &[k(&g, "ELEMENT_ARRAY_BUFFER"), eb]);
    let ibytes = u16_bytes(&[0, 1, 2, 2, 1, 3]);
    g.call("bufferData", &[k(&g, "ELEMENT_ARRAY_BUFFER"), Arg::Bytes(Elem::U16, &ibytes), k(&g, "STATIC_DRAW")]);
    // Texture.
    let tex = g.tex("TEXTURE_2D");
    let a = [k(&g, "TEXTURE_2D"), n(0.0), k(&g, "RGBA"), n(1.0), n(1.0), n(0.0), k(&g, "RGBA"), k(&g, "UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[255, 0, 0, 255])];
    g.call("texImage2D", &a);
    g.call("texParameteri", &[k(&g, "TEXTURE_2D"), k(&g, "TEXTURE_MIN_FILTER"), k(&g, "LINEAR")]);
    // Clear, uniforms, draw, present.
    g.call("clearColor", &[n(0.0), n(0.0), n(0.0), n(1.0)]);
    let mask = g.kv("COLOR_BUFFER_BIT") + g.kv("DEPTH_BUFFER_BIT");
    g.call("clear", &[n(mask)]);
    let ident = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    let (lm, lc, lt) = (g.loc(prog, "uMvp"), g.loc(prog, "uColor"), g.loc(prog, "uTex"));
    g.call("uniformMatrix4fv", &[lm, Arg::Bool(false), Arg::Nums(&ident)]);
    g.call("uniform4f", &[lc, n(1.0), n(0.5), n(0.25), n(1.0)]);
    g.call("uniform1i", &[lt, n(0.0)]);
    g.call("drawElements", &[k(&g, "TRIANGLES"), n(6.0), k(&g, "UNSIGNED_SHORT"), n(0.0)]);
    g.no_err();
    assert!(g.ctx.present(&mut g.dev));
    let cmds = std::mem::take(&mut g.dev.cmds);

    // The expected list.
    let desc = ProgramDesc {
        vertex_wgsl: r.vertex_wgsl.clone(),
        fragment_wgsl: r.fragment_wgsl.clone(),
        uniform_bytes: r.uniform_bytes,
        blocks: vec![],
        textures: r.textures.clone(),
        attributes: r.attributes.iter().map(|a| AttrSlot { location: a.location, base: a.base }).collect(),
        outputs: r.outputs.iter().map(|o| OutSlot { location: o.location, base: o.base }).collect(),
        writes_depth: r.writes_depth,
    };
    let mut uniforms = vec![0u8; r.uniform_bytes as usize];
    for u in &r.uniforms {
        let Place::Block { offset, col_stride, .. } = u.place else { continue };
        let vals: &[f32] = if u.name == "uMvp" { &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0] } else { &[1.0, 0.5, 0.25, 1.0] };
        let rows = if u.name == "uMvp" { 4 } else { vals.len() };
        for (i, v) in vals.iter().enumerate() {
            let at = offset as usize + (i / rows) * col_stride as usize + (i % rows) * 4;
            uniforms[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    let va = |l: u32, size: u8, offset: u32| VertexAttr { location: l, buffer: h(vb), offset, stride: 20, ty: AttrType::F32, size, normalized: false, integer: false, divisor: 0 };
    let attrs_list = r.attributes.iter().map(|a| if a.location == pos { va(pos, 3, 0) } else { va(uv, 2, 12) }).collect();
    let sampling = Sampling { mag: Filter::Linear, min: Filter::Linear, mip: None, wrap: [Wrap::Repeat; 3], compare: None, lod_min: -1000.0, lod_max: 1000.0, base_level: 0, max_level: 0 };
    let state = DrawState { viewport: [0, 0, 4, 4], ..common::scene::default_state() };
    let draw = Draw {
        target: Target::Surface,
        state,
        program: h(prog),
        attrs: attrs_list,
        consts: vec![],
        index: Some(IndexSource { buffer: h(eb), offset: 0, wide: false }),
        first: 0,
        count: 6,
        instances: 1,
        uniforms: uniforms.into(),
        blocks: vec![],
        textures: vec![TexBind { texture: h(tex), sampling }],
    };
    let tdesc = TexDesc { kind: TexKind::D2, format: TexFormat::Rgba8, width: 1, height: 1, depth: 1, mips: 1, samples: 1 };
    let want = vec![
        Cmd::Configure { width: 4, height: 4, alpha: true, depth: true, stencil: false, samples: 1, premultiplied: true, preserve: false },
        Cmd::ProgramCreate { id: h(prog), desc: Box::new(desc) },
        Cmd::BufferCreate { id: h(vb), index: false, bytes: vbytes },
        Cmd::BufferCreate { id: h(eb), index: true, bytes: ibytes },
        Cmd::TextureCreate { id: h(tex), desc: tdesc },
        Cmd::TextureWrite { id: h(tex), level: 0, layer: 0, x: 0, y: 0, width: 1, height: 1, depth: 1, bytes: vec![255, 0, 0, 255] },
        Cmd::Clear { target: Target::Surface, colors: vec![(0, ClearColor::F([0.0, 0.0, 0.0, 1.0]))], depth: Some(1.0), stencil: None, color_mask: [true; 4], stencil_mask: 0xFFFF_FFFF, scissor: None },
        Cmd::Draw(Box::new(draw)),
        Cmd::Present,
    ];
    assert_eq!(cmds.len(), want.len(), "{cmds:#?}");
    for (got, want) in cmds.iter().zip(&want) {
        assert_eq!(got, want);
    }
}
