//! L7: limits, overflow, random calls, context loss.

mod common;
use common::*;
use invisium_gl::cmd::{Cmd, TexFormat};
use invisium_gl::*;

#[test]
fn memory_budget() {
    let mut g = Gl::new(Version::WebGl2);
    g.tex("TEXTURE_2D");
    g.cmds();
    g.call("texStorage2D", &[g.k("TEXTURE_2D"), n(1.0), g.k("RGBA32F"), n(8192.0), n(8192.0)]);
    g.expect("OUT_OF_MEMORY");
    assert!(g.cmds().is_empty(), "nothing allocated");
    // Within the budget it works; two of them do not fit.
    g.call("texStorage2D", &[g.k("TEXTURE_2D"), n(1.0), g.k("RGBA8"), n(8192.0), n(8192.0)]);
    g.no_err();
    g.tex("TEXTURE_2D");
    let a = [g.k("TEXTURE_2D"), n(0.0), g.k("RGBA16F"), n(8192.0), n(8192.0), n(0.0), g.k("RGBA"), g.k("HALF_FLOAT"), Arg::Null];
    g.call("texImage2D", &a);
    g.expect("OUT_OF_MEMORY");
    let b = g.obj("createBuffer");
    g.call("bindBuffer", &[g.k("ARRAY_BUFFER"), b]);
    g.call("bufferData", &[g.k("ARRAY_BUFFER"), n(300.0 * 1048576.0), g.k("STATIC_DRAW")]);
    g.expect("OUT_OF_MEMORY");
    let r = g.obj("createRenderbuffer");
    g.call("bindRenderbuffer", &[g.k("RENDERBUFFER"), r]);
    g.call("renderbufferStorageMultisample", &[g.k("RENDERBUFFER"), n(4.0), g.k("RGBA8"), n(8192.0), n(8192.0)]);
    g.expect("OUT_OF_MEMORY");
}

#[test]
fn overflowing_sizes() {
    let mut g = Gl::new(Version::WebGl2);
    g.tex("TEXTURE_2D");
    let big = n(2147483647.0);
    let a = [g.k("TEXTURE_2D"), n(0.0), g.k("RGBA8"), big, big, n(0.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Null];
    g.call("texImage2D", &a);
    g.expect("INVALID_VALUE");
    let b = g.obj("createBuffer");
    g.call("bindBuffer", &[g.k("ARRAY_BUFFER"), b]);
    g.call("bufferData", &[g.k("ARRAY_BUFFER"), n(9.0e18), g.k("STATIC_DRAW")]);
    g.expect("OUT_OF_MEMORY");
    g.call("bufferData", &[g.k("ARRAY_BUFFER"), n(16.0), g.k("STATIC_DRAW")]);
    g.call("bufferSubData", &[g.k("ARRAY_BUFFER"), n(9.0e18), Arg::Bytes(Elem::U8, &[1])]);
    g.expect("INVALID_VALUE");
    g.call("copyBufferSubData", &[g.k("ARRAY_BUFFER"), g.k("ARRAY_BUFFER"), n(9.0e18), n(9.0e18), n(9.0e18)]);
    g.expect("INVALID_VALUE");
    g.call("pixelStorei", &[g.k("PACK_ROW_LENGTH"), big]);
    g.call("pixelStorei", &[g.k("PACK_SKIP_ROWS"), big]);
    g.call("readPixels", &[n(0.0), n(0.0), big, big, g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[0; 64])]);
    g.expect("INVALID_OPERATION");
    g.call("pixelStorei", &[g.k("UNPACK_ROW_LENGTH"), big]);
    g.call("pixelStorei", &[g.k("UNPACK_SKIP_IMAGES"), big]);
    g.call("pixelStorei", &[g.k("UNPACK_IMAGE_HEIGHT"), big]);
    g.tex("TEXTURE_3D");
    let a = [g.k("TEXTURE_3D"), n(0.0), g.k("RGBA32F"), n(2048.0), n(2048.0), n(2048.0), n(0.0), g.k("RGBA"), g.k("FLOAT"), Arg::Bytes(Elem::F32, &[0; 64])];
    g.call("texImage3D", &a);
    assert_ne!(g.err(), 0.0);
    let a = [g.k("TEXTURE_3D"), n(0.0), g.k("RGBA32F"), n(2048.0), n(2048.0), n(2048.0), n(0.0), g.k("RGBA"), g.k("FLOAT"), Arg::Null];
    g.call("texImage3D", &a);
    g.expect("OUT_OF_MEMORY");
    let px = Arg::Pixels { width: u32::MAX, height: u32::MAX, rgba: &[0; 16] };
    g.call("texImage2D", &[g.k("TEXTURE_2D"), n(0.0), g.k("RGBA"), g.k("RGBA"), g.k("UNSIGNED_BYTE"), px]);
    assert_ne!(g.err(), 0.0);
}

/// A small seeded generator (xorshift64*).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const NUMS: &[f64] = &[-1.0, 0.0, 1.0, 2.0, 3.0, 4.0, 7.0, 8.0, 16.0, 255.0, 256.0, 1e9, 2147483648.0, -2147483648.0, 9.0e15, f64::NAN, f64::INFINITY, 0.5];
const KINDS: &[Kind] = &[Kind::Buffer, Kind::Texture, Kind::Renderbuffer, Kind::Framebuffer, Kind::Program, Kind::Sampler];
const ELEMS: &[Elem] = &[Elem::U8, Elem::I8, Elem::U16, Elem::I16, Elem::U32, Elem::I32, Elem::F32, Elem::F64, Elem::Raw, Elem::U8Clamped];

#[test]
fn random_calls_never_panic() {
    let bytes = vec![0x5Au8; 4096];
    let numbers: Vec<f64> = (0..64).map(|i| i as f64 * 0.75).collect();
    for v in [Version::WebGl1, Version::WebGl2] {
        let mut g = Gl::new(v);
        g.dev.discard = true;
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ v as u64);
        let ops: Vec<u16> = methods(v).iter().map(|m| m.1).chain([175, 176, 178, 222, 225, 901, 9999]).collect();
        let consts: Vec<u32> = constants(v).iter().map(|c| c.1).collect();
        for e in tables_exts(v) {
            g.call("getExtension", &[Arg::Str(e)]);
        }
        for i in 0..50_000 {
            let op = ops[rng.below(ops.len() as u64) as usize];
            let argc = rng.below(12) as usize;
            let mut args = Vec::with_capacity(argc);
            for _ in 0..argc {
                let a = match rng.below(12) {
                    0 => Arg::Undefined,
                    1 => Arg::Null,
                    2 => Arg::Bool(rng.below(2) == 1),
                    3 | 4 => Arg::Num(NUMS[rng.below(NUMS.len() as u64) as usize]),
                    5..=7 => Arg::Num(consts[rng.below(consts.len() as u64) as usize] as f64),
                    8 => Arg::Str("x"),
                    9 => {
                        let len = rng.below(200) as usize;
                        Arg::Bytes(ELEMS[rng.below(ELEMS.len() as u64) as usize], &bytes[..len])
                    }
                    10 => Arg::Nums(&numbers[..rng.below(20) as usize]),
                    _ => match rng.below(3) {
                        0 => Arg::Pixels { width: rng.below(5) as u32, height: rng.below(5) as u32, rgba: &bytes[..rng.below(80) as usize] },
                        _ => Arg::Object(KINDS[rng.below(KINDS.len() as u64) as usize], 1 + rng.below(40) as u32),
                    },
                };
                args.push(a);
            }
            g.ctx.call(&mut g.dev, op, &args);
            if i % 997 == 0 {
                g.ctx.present(&mut g.dev);
            }
        }
        assert!(g.ctx.memory_bytes() < 600 << 20);
    }
}

fn tables_exts(v: Version) -> Vec<&'static str> {
    let names: &[&'static str] = match v {
        Version::WebGl1 => &["OES_texture_float", "OES_texture_half_float", "WEBGL_depth_texture", "WEBGL_draw_buffers", "ANGLE_instanced_arrays", "OES_vertex_array_object", "EXT_blend_minmax", "WEBGL_color_buffer_float"],
        Version::WebGl2 => &["EXT_color_buffer_float", "EXT_color_buffer_half_float"],
    };
    names.to_vec()
}

#[test]
fn lost_context() {
    let mut g = Gl::new(Version::WebGl2);
    let t = g.tex("TEXTURE_2D");
    g.call("getExtension", &[Arg::Str("WEBGL_lose_context")]);
    g.call("enable", &[n(1.0)]);
    g.call("loseContext", &[]);
    g.cmds();
    assert_eq!(g.call("isContextLost", &[]), Ret::Bool(true));
    assert_eq!(g.err(), g.kv("CONTEXT_LOST_WEBGL"));
    g.no_err();
    assert_eq!(g.param("VIEWPORT"), Ret::Null);
    assert_eq!(g.call("createTexture", &[]), Ret::Null);
    assert_eq!(g.call("isTexture", &[t]), Ret::Bool(false));
    assert_eq!(g.call("isEnabled", &[g.k("BLEND")]), Ret::Bool(false));
    assert_eq!(g.call("getContextAttributes", &[]), Ret::Null);
    assert_eq!(g.call("getSupportedExtensions", &[]), Ret::Null);
    assert_eq!(g.call("getExtension", &[Arg::Str("WEBGL_lose_context")]), Ret::Null);
    assert_eq!(g.call("checkFramebufferStatus", &[g.k("FRAMEBUFFER")]), Ret::Num(g.kv("FRAMEBUFFER_UNSUPPORTED")));
    g.call("clear", &[g.k("COLOR_BUFFER_BIT")]);
    let a = [g.k("TEXTURE_2D"), n(0.0), g.k("RGBA"), n(1.0), n(1.0), n(0.0), g.k("RGBA"), g.k("UNSIGNED_BYTE"), Arg::Bytes(Elem::U8, &[1, 2, 3, 4])];
    g.call("texImage2D", &a);
    g.call("restoreContext", &[]);
    g.no_err();
    assert!(g.cmds().is_empty(), "a lost context emits nothing");
    assert!(!g.ctx.present(&mut g.dev));
    let _ = TexFormat::Rgba8;
    let _ = Cmd::Present;
}
