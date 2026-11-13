//! A small drawing scene shared by the draw tests.

use super::*;
use invisium_gl::cmd::*;
use invisium_gl::*;

pub const VS: &str = "attribute vec3 aPos;\nattribute vec2 aUv;\nuniform mat4 uMvp;\nvarying vec2 vUv;\nvoid main() { vUv = aUv; gl_Position = uMvp * vec4(aPos, 1.0); }\n";
pub const FS: &str = "precision mediump float;\nuniform vec4 uColor;\nuniform sampler2D uTex;\nvarying vec2 vUv;\nvoid main() { gl_FragColor = texture2D(uTex, vUv) * uColor; }\n";

pub fn h(a: Arg) -> u32 {
    if let Arg::Object(_, h) = a {
        h
    } else {
        0
    }
}

pub struct Scene {
    pub g: Gl,
    pub prog: Arg<'static>,
    pub vbuf: Arg<'static>,
    pub ebuf: Arg<'static>,
    pub pos: u32,
    pub uv: u32,
}

/// A program, 4 interleaved vertices (stride 20) and 16-bit indices [0, 1, 2, 2, 1, 3].
pub fn scene(attrs: Attributes) -> Scene {
    let mut g = Gl::with(Version::WebGl2, attrs, 16, 16);
    let prog = g.program(VS, FS);
    let loc = |g: &mut Gl, n: &str| match g.call("getAttribLocation", &[prog, Arg::Str(n)]) {
        Ret::Num(v) => v as u32,
        _ => panic!(),
    };
    let (pos, uv) = (loc(&mut g, "aPos"), loc(&mut g, "aUv"));
    let vbuf = g.obj("createBuffer");
    g.call("bindBuffer", &[g.k("ARRAY_BUFFER"), vbuf]);
    g.call("bufferData", &[g.k("ARRAY_BUFFER"), Arg::Bytes(Elem::F32, &f32_bytes(&[0.0; 20])), g.k("STATIC_DRAW")]);
    let fl = g.k("FLOAT");
    g.call("vertexAttribPointer", &[n(pos as f64), n(3.0), fl, Arg::Bool(false), n(20.0), n(0.0)]);
    g.call("vertexAttribPointer", &[n(uv as f64), n(2.0), fl, Arg::Bool(false), n(20.0), n(12.0)]);
    g.call("enableVertexAttribArray", &[n(pos as f64)]);
    g.call("enableVertexAttribArray", &[n(uv as f64)]);
    let ebuf = g.obj("createBuffer");
    g.call("bindBuffer", &[g.k("ELEMENT_ARRAY_BUFFER"), ebuf]);
    g.call("bufferData", &[g.k("ELEMENT_ARRAY_BUFFER"), Arg::Bytes(Elem::U16, &u16_bytes(&[0, 1, 2, 2, 1, 3])), g.k("STATIC_DRAW")]);
    g.no_err();
    g.cmds();
    Scene { g, prog, vbuf, ebuf, pos, uv }
}

pub fn default_state() -> DrawState {
    DrawState {
        topology: Topology::Triangles,
        cull: None,
        front_ccw: false,
        blend: None,
        blend_color: [0.0; 4],
        color_mask: [true; 4],
        depth_test: None,
        depth_write: false,
        depth_bias: None,
        stencil: None,
        alpha_to_coverage: false,
        viewport: [0, 0, 16, 16],
        depth_range: [0.0, 1.0],
        scissor: None,
    }
}

pub fn default_sampling() -> Sampling {
    Sampling { mag: Filter::Linear, min: Filter::Nearest, mip: Some(Filter::Linear), wrap: [Wrap::Repeat; 3], compare: None, lod_min: -1000.0, lod_max: 1000.0, base_level: 0, max_level: 0 }
}

impl Scene {
    pub fn draw(&mut self, name: &str, args: &[Arg]) -> Vec<Cmd> {
        self.g.cmds();
        self.g.call(name, args);
        self.g.cmds()
    }
    pub fn one(&mut self, name: &str, args: &[Arg]) -> Draw {
        let cmds = self.draw(name, args);
        self.g.no_err();
        let mut d = draws(&cmds);
        assert_eq!(d.len(), 1, "{cmds:?}");
        d.pop().unwrap()
    }
    /// One `drawArrays(TRIANGLES, 0, 3)`.
    pub fn tri(&mut self) -> Draw {
        let t = self.g.k("TRIANGLES");
        self.one("drawArrays", &[t, n(0.0), n(3.0)])
    }
    pub fn attr(&self, loc: u32, divisor: u32) -> VertexAttr {
        let (size, offset) = if loc == self.pos { (3, 0) } else { (2, 12) };
        VertexAttr { location: loc, buffer: h(self.vbuf), offset, stride: 20, ty: AttrType::F32, size, normalized: false, integer: false, divisor }
    }
    pub fn expected(&self, d: &Draw, divisor_uv: u32) -> Draw {
        let r = reflect(VS, FS, true);
        let attrs = r.attributes.iter().map(|a| self.attr(a.location, if a.location == self.uv { divisor_uv } else { 0 })).collect();
        Draw {
            target: Target::Surface,
            state: default_state(),
            program: h(self.prog),
            attrs,
            consts: vec![],
            index: None,
            first: 0,
            count: 0,
            instances: 1,
            uniforms: d.uniforms.clone(),
            blocks: vec![],
            textures: vec![TexBind { texture: 0, sampling: default_sampling() }],
        }
    }
}
