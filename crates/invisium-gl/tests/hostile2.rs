//! P8: random calls over every op with real programs, buffers and textures in the mix.

mod common;
use common::scene::*;
use common::*;
use invisium_gl::*;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

const NUMS: &[f64] = &[-1.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 12.0, 16.0, 20.0, 255.0, 65535.0, 1e9, 2147483648.0, f64::NAN, f64::INFINITY, 0.5];
const KINDS: &[Kind] = &[
    Kind::Buffer, Kind::Texture, Kind::Renderbuffer, Kind::Framebuffer, Kind::Shader, Kind::Program, Kind::UniformLocation,
    Kind::VertexArray, Kind::Sampler, Kind::Query, Kind::Sync, Kind::TransformFeedback,
];
const ELEMS: &[Elem] = &[Elem::U8, Elem::I8, Elem::U16, Elem::I16, Elem::U32, Elem::I32, Elem::F32, Elem::Raw];
const DRAWS: &[&str] = &["drawArrays", "drawElements", "drawArraysInstanced", "drawElementsInstanced", "drawRangeElements"];

/// `reset` runs every 200 calls so that well-formed draws keep reaching the device.
fn run(mut g: Gl, extra: &[(&str, Vec<Arg<'static>>)], reset: &[(&str, Vec<Arg<'static>>)], seed: u64) {
    g.dev.discard = true;
    for (name, args) in extra {
        g.call(name, args);
    }
    let v = g.v;
    let ops: Vec<u16> = methods(v).iter().map(|m| m.1).chain([901]).collect();
    let draw_ops: Vec<u16> = DRAWS.iter().filter_map(|d| method_op(v, d)).chain([175, 176]).collect();
    let consts: Vec<u32> = constants(v).iter().map(|c| c.1).collect();
    let bytes = vec![0x11u8; 512];
    let floats: Vec<f64> = (0..64).map(|i| (i % 7) as f64 * 0.5).collect();
    let mut rng = Rng(seed);
    for i in 0..50_000 {
        if i % 200 == 0 {
            for (name, args) in reset {
                g.call(name, args);
            }
        }
        if rng.below(4) == 0 {
            // A well-formed draw with random mode, range and instance count.
            let (mode, first, count, inst) = (rng.below(8) as f64, rng.below(4) as f64, rng.below(8) as f64, rng.below(3) as f64);
            let ty = g.kv("UNSIGNED_SHORT");
            let webgl1 = g.v == Version::WebGl1;
            let (name, args) = match rng.below(3) {
                0 => (if webgl1 { "drawArrays" } else { "drawArraysInstanced" }, vec![n(mode), n(first), n(count), n(inst)]),
                1 => (if webgl1 { "drawElements" } else { "drawElementsInstanced" }, vec![n(mode), n(count), n(ty), n(first * 2.0), n(inst)]),
                _ if webgl1 => ("drawArrays", vec![n(mode), n(first), n(count)]),
                _ => ("drawRangeElements", vec![n(mode), n(0.0), n(3.0), n(count), n(ty), n(first * 2.0)]),
            };
            g.call(name, &args);
            continue;
        }
        let op = if rng.below(4) == 0 { draw_ops[rng.below(draw_ops.len() as u64) as usize] } else { ops[rng.below(ops.len() as u64) as usize] };
        let argc = rng.below(12) as usize;
        let mut args = Vec::with_capacity(argc);
        for _ in 0..argc {
            args.push(match rng.below(14) {
                0 => Arg::Undefined,
                1 => Arg::Null,
                2 => Arg::Bool(rng.below(2) == 1),
                3..=5 => Arg::Num(NUMS[rng.below(NUMS.len() as u64) as usize]),
                6..=8 => Arg::Num(consts[rng.below(consts.len() as u64) as usize] as f64),
                9 => Arg::Str(["uColor", "uMvp", "aPos", "uTex", "x", "uW[2]"][rng.below(6) as usize]),
                10 => Arg::Bytes(ELEMS[rng.below(ELEMS.len() as u64) as usize], &bytes[..rng.below(300) as usize]),
                11 => Arg::Nums(&floats[..rng.below(40) as usize]),
                12 => Arg::Pixels { width: rng.below(4) as u32, height: rng.below(4) as u32, rgba: &bytes[..rng.below(64) as usize] },
                _ => Arg::Object(KINDS[rng.below(KINDS.len() as u64) as usize], 1 + rng.below(30) as u32),
            });
        }
        g.ctx.call(&mut g.dev, op, &args);
        if i % 1000 == 0 {
            g.ctx.present(&mut g.dev);
        }
    }
    g.ctx.flush(&mut g.dev);
    assert!(g.ctx.memory_bytes() < 600 << 20);
    assert!(g.dev.draw_count > 0, "some random draws went through");
}

#[test]
fn random_calls_with_real_objects_never_panic() {
    // WebGL 2: the drawing scene plus a texture with data and a second program.
    let mut s = scene(Attributes { stencil: true, ..Attributes::default() });
    s.g.tex("TEXTURE_2D");
    let a = [s.g.k("TEXTURE_2D"), n(0.0), s.g.k("RGBA"), n(2.0), n(2.0), n(0.0), s.g.k("RGBA"), s.g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[7; 16])];
    s.g.call("texImage2D", &a);
    let fs = "precision mediump float;\nuniform float uW[4];\nvoid main() { gl_FragColor = vec4(uW[0], uW[1], uW[2], uW[3]); }\n";
    s.g.program("attribute vec4 aPos;\nvoid main() { gl_Position = aPos; }\n", fs);
    s.g.call("useProgram", &[s.prog]);
    let reset = reset_list(&s.g, s.prog, s.vbuf, s.ebuf, s.pos, s.uv, true);
    run(s.g, &[], &reset, 0x1234_5678);

    // WebGL 1 with every extension enabled.
    let mut s = scene(Attributes::default());
    let mut g = Gl::new(Version::WebGl1);
    let prog = g.program(VS, FS);
    let (vbuf, ebuf) = (g.obj("createBuffer"), g.obj("createBuffer"));
    g.call("bindBuffer", &[g.k("ARRAY_BUFFER"), vbuf]);
    g.call("bufferData", &[g.k("ARRAY_BUFFER"), n(80.0), g.k("STATIC_DRAW")]);
    g.call("bindBuffer", &[g.k("ELEMENT_ARRAY_BUFFER"), ebuf]);
    g.call("bufferData", &[g.k("ELEMENT_ARRAY_BUFFER"), Arg::Bytes(Elem::U16, &u16_bytes(&[0, 1, 2, 2, 1, 3])), g.k("STATIC_DRAW")]);
    s.g.cmds();
    let exts: Vec<(&str, Vec<Arg>)> = ["OES_texture_float", "WEBGL_depth_texture", "WEBGL_draw_buffers", "ANGLE_instanced_arrays", "OES_vertex_array_object", "OES_element_index_uint", "WEBGL_color_buffer_float"]
        .iter()
        .map(|e| ("getExtension", vec![Arg::Str(e)]))
        .collect();
    let reset = reset_list(&g, prog, vbuf, ebuf, s.pos, s.uv, false);
    run(g, &exts, &reset, 0x9876_5432);
}

/// Calls that bring the context back to a drawable state.
fn reset_list(g: &Gl, prog: Arg<'static>, vbuf: Arg<'static>, ebuf: Arg<'static>, pos: u32, uv: u32, webgl2: bool) -> Vec<(&'static str, Vec<Arg<'static>>)> {
    let mut v = vec![
        ("useProgram", vec![prog]),
        ("bindFramebuffer", vec![g.k("FRAMEBUFFER"), Arg::Null]),
        ("disable", vec![g.k("CULL_FACE")]),
        ("disable", vec![g.k("STENCIL_TEST")]),
        ("bindBuffer", vec![g.k("ARRAY_BUFFER"), vbuf]),
        ("bindBuffer", vec![g.k("ELEMENT_ARRAY_BUFFER"), ebuf]),
    ];
    if webgl2 {
        v.insert(1, ("bindVertexArray", vec![Arg::Null]));
        v.push(("disable", vec![g.k("RASTERIZER_DISCARD")]));
    }
    for (l, size, off) in [(pos, 3.0, 0.0), (uv, 2.0, 12.0)] {
        v.push(("vertexAttribPointer", vec![n(l as f64), n(size), g.k("FLOAT"), Arg::Bool(false), n(20.0), n(off)]));
        v.push(("enableVertexAttribArray", vec![n(l as f64)]));
    }
    v
}
