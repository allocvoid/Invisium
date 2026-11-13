//! Mock device and helpers shared by the integration tests.
#![allow(dead_code)]

pub mod scene;

use invisium_gl::cmd::{Cmd, GlDevice, GlStats, Rect4, Target, TexFormat};
use invisium_gl::*;

/// Records every command; answers `read_pixels` from a prepared image.
#[derive(Default)]
pub struct Mock {
    pub cmds: Vec<Cmd>,
    pub reads: Vec<(Target, u32, Rect4)>,
    /// Format, width, height and tightly packed rows (row 0 first) of what reads return.
    pub image: Option<(TexFormat, u32, u32, Vec<u8>)>,
    /// Drop commands instead of recording them (long random runs).
    pub discard: bool,
    /// The device's own buffer contents, kept by running the buffer commands.
    pub buffers: std::collections::HashMap<u32, Vec<u8>>,
    pub buffer_reads: u32,
    /// `Cmd::Draw`s run.
    pub draw_count: u32,
}

impl Mock {
    fn apply(&mut self, c: &Cmd) {
        match c {
            Cmd::BufferCreate { id, bytes, .. } => {
                self.buffers.insert(*id, bytes.clone());
            }
            Cmd::BufferWrite { id, offset, bytes } => {
                if let Some(b) = self.buffers.get_mut(id) {
                    let o = *offset as usize;
                    b[o..o + bytes.len()].copy_from_slice(bytes);
                }
            }
            Cmd::BufferCopy { src, src_offset, dst, dst_offset, size } => {
                let (so, doff, n) = (*src_offset as usize, *dst_offset as usize, *size as usize);
                let chunk = self.buffers.get(src).expect("copy from a missing buffer")[so..so + n].to_vec();
                self.buffers.get_mut(dst).expect("copy to a missing buffer")[doff..doff + n].copy_from_slice(&chunk);
            }
            Cmd::BufferDelete { id } => {
                self.buffers.remove(id);
            }
            Cmd::Draw(_) => self.draw_count += 1,
            _ => {}
        }
    }
}

impl GlDevice for Mock {
    fn create(&mut self) -> Option<u32> {
        Some(1)
    }
    fn destroy(&mut self, _ctx: u32) {}
    fn image(&self, _ctx: u32) -> u32 {
        0
    }
    fn run(&mut self, _ctx: u32, cmds: &mut Vec<Cmd>) {
        for c in cmds.iter() {
            self.apply(c);
        }
        if self.discard {
            cmds.clear();
        } else {
            self.cmds.append(cmds);
        }
    }
    fn read_pixels(&mut self, _ctx: u32, target: &Target, color: u32, rect: Rect4) -> Option<(TexFormat, Vec<u8>)> {
        self.reads.push((target.clone(), color, rect));
        let (fmt, w, _h, data) = self.image.as_ref()?;
        let tb = fmt.bytes() as usize;
        let mut out = Vec::new();
        for y in rect[1]..rect[1] + rect[3] {
            let s = (y as usize * *w as usize + rect[0] as usize) * tb;
            out.extend_from_slice(&data[s..s + rect[2] as usize * tb]);
        }
        Some((*fmt, out))
    }
    fn read_buffer(&mut self, _ctx: u32, buffer: u32, offset: u32, len: u32) -> Option<Vec<u8>> {
        self.buffer_reads += 1;
        let b = self.buffers.get(&buffer)?;
        b.get(offset as usize..(offset + len) as usize).map(|s| s.to_vec())
    }
    fn stats(&self) -> GlStats {
        GlStats::default()
    }
}

pub struct Gl {
    pub ctx: Context,
    pub dev: Mock,
    pub v: Version,
}

pub fn n(v: f64) -> Arg<'static> {
    Arg::Num(v)
}

pub fn c(v: Version, name: &str) -> f64 {
    constants(v).iter().find(|x| x.0 == name).unwrap_or_else(|| panic!("no constant {name}")).1 as f64
}

impl Gl {
    pub fn new(v: Version) -> Gl {
        Gl::with(v, Attributes::default(), 16, 16)
    }
    pub fn with(v: Version, attrs: Attributes, w: u32, h: u32) -> Gl {
        Gl { ctx: Context::new(1, v, attrs, w, h), dev: Mock::default(), v }
    }
    /// The value of a context constant, as a number argument.
    pub fn k(&self, name: &str) -> Arg<'static> {
        Arg::Num(c(self.v, name))
    }
    pub fn kv(&self, name: &str) -> f64 {
        c(self.v, name)
    }
    pub fn call(&mut self, name: &str, args: &[Arg]) -> Ret {
        let op = method_op(self.v, name).unwrap_or_else(|| {
            let mut found = None;
            for e in ["WEBGL_lose_context", "ANGLE_instanced_arrays", "OES_vertex_array_object", "WEBGL_draw_buffers"] {
                if let Some(info) = extension(self.v, e) {
                    if let Some(m) = info.methods.iter().find(|m| m.0 == name) {
                        found = Some(m.1);
                    }
                }
            }
            found.unwrap_or_else(|| panic!("no method {name}"))
        });
        self.ctx.call(&mut self.dev, op, args)
    }
    pub fn err(&mut self) -> f64 {
        match self.call("getError", &[]) {
            Ret::Num(e) => e,
            r => panic!("getError returned {r:?}"),
        }
    }
    pub fn no_err(&mut self) {
        assert_eq!(self.err(), 0.0, "unexpected GL error");
    }
    pub fn expect(&mut self, name: &str) {
        let want = self.kv(name);
        assert_eq!(self.err(), want, "expected {name}");
    }
    /// Flush and take every command sent so far.
    pub fn cmds(&mut self) -> Vec<Cmd> {
        self.ctx.flush(&mut self.dev);
        std::mem::take(&mut self.dev.cmds)
    }
    pub fn param(&mut self, name: &str) -> Ret {
        let p = self.k(name);
        self.call("getParameter", &[p])
    }
    pub fn obj(&mut self, create: &str) -> Arg<'static> {
        match self.call(create, &[]) {
            Ret::Object(k, h) => Arg::Object(k, h),
            r => panic!("{create} returned {r:?}"),
        }
    }
    /// Create a texture and bind it to `target`.
    pub fn tex(&mut self, target: &str) -> Arg<'static> {
        let t = self.obj("createTexture");
        let tg = self.k(target);
        self.call("bindTexture", &[tg, t]);
        t
    }
}

impl Gl {
    pub fn shader(&mut self, ty: &str, src: &str) -> Arg<'static> {
        let t = self.k(ty);
        let s = match self.call("createShader", &[t]) {
            Ret::Object(k, h) => Arg::Object(k, h),
            r => panic!("createShader returned {r:?}"),
        };
        self.call("shaderSource", &[s, Arg::Str(src)]);
        self.call("compileShader", &[s]);
        s
    }
    /// Compile, attach and link; panics with the logs if the link fails. Leaves it in use.
    pub fn program(&mut self, vs: &str, fs: &str) -> Arg<'static> {
        let (v, f) = (self.shader("VERTEX_SHADER", vs), self.shader("FRAGMENT_SHADER", fs));
        let p = self.obj("createProgram");
        self.call("attachShader", &[p, v]);
        self.call("attachShader", &[p, f]);
        self.call("linkProgram", &[p]);
        let ls = self.k("LINK_STATUS");
        if self.call("getProgramParameter", &[p, ls]) != Ret::Bool(true) {
            let logs = (self.call("getShaderInfoLog", &[v]), self.call("getShaderInfoLog", &[f]), self.call("getProgramInfoLog", &[p]));
            panic!("link failed: {logs:?}");
        }
        self.call("useProgram", &[p]);
        p
    }
    pub fn loc(&mut self, p: Arg<'static>, name: &str) -> Arg<'static> {
        match self.call("getUniformLocation", &[p, Arg::Str(name)]) {
            Ret::Object(k, h) => Arg::Object(k, h),
            r => panic!("no location for {name}: {r:?}"),
        }
    }
}

/// What the translator makes of a pair of shaders (for comparing reflection).
pub fn reflect(vs: &str, fs: &str, webgl2: bool) -> invisium_glsl::Program {
    let v = invisium_glsl::compile(invisium_glsl::Stage::Vertex, vs, webgl2).unwrap();
    let f = invisium_glsl::compile(invisium_glsl::Stage::Fragment, fs, webgl2).unwrap();
    invisium_glsl::link(&v, &f, &[]).unwrap()
}

/// Every `Cmd::Draw` among the commands.
pub fn draws(cmds: &[Cmd]) -> Vec<invisium_gl::cmd::Draw> {
    cmds.iter().filter_map(|c| if let Cmd::Draw(d) = c { Some((**d).clone()) } else { None }).collect()
}

/// Every `TextureWrite` among the commands: (id, level, layer, x, y, w, h, d, bytes).
pub type Write = (u32, u32, u32, u32, u32, u32, u32, u32, Vec<u8>);

pub fn writes(cmds: &[Cmd]) -> Vec<Write> {
    cmds.iter()
        .filter_map(|c| match c {
            Cmd::TextureWrite { id, level, layer, x, y, width, height, depth, bytes } => {
                Some((*id, *level, *layer, *x, *y, *width, *height, *depth, bytes.clone()))
            }
            _ => None,
        })
        .collect()
}

pub fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn u16_bytes(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}
