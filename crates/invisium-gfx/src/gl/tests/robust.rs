//! H9 (robustness) and H10 (efficiency).

use super::surface::composite;
use super::textures::{fs_tex, tex_program};
use super::*;

fn setup(h: &H) {
    h.run(vec![cfg(4, 4), basic_program(), buffer(1, quad())]);
}

fn red_ok(h: &H) {
    h.run(vec![clear(Target::Surface, [0.0; 4]), quad_draw(Target::Surface, st(4, 4), [1.0, 0.0, 0.0, 1.0])]);
    assert_near(h.surf(1, 1), [255, 0, 0, 255], 0);
}

#[test]
fn invalid_programs_and_unknown_ids() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    h.run(vec![program(7, desc("this is not wgsl", FS_UNI, 16, &[(0, Scalar::F32)]))]);
    let mut d = draw(Target::Surface, 7, st(4, 4), vec![attr(0, 1, AttrType::F32, 2, 8, 0)], 6, uni([0.0, 1.0, 0.0, 1.0]));
    h.run(vec![clear(Target::Surface, [0.0; 4]), Cmd::Draw(Box::new(d.clone()))]);
    assert_near(h.surf(1, 1), [0, 0, 0, 0], 0);
    red_ok(&h);
    // Unknown program, buffer, texture, targets.
    d.program = 99;
    let mut bad_buf = draw(Target::Surface, 1, st(4, 4), vec![attr(0, 42, AttrType::F32, 2, 8, 0)], 6, uni([0.0; 4]));
    bad_buf.index = Some(IndexSource { buffer: 77, offset: 0, wide: false });
    h.run(vec![
        Cmd::Draw(Box::new(d)),
        Cmd::Draw(Box::new(bad_buf)),
        Cmd::BufferWrite { id: 55, offset: 0, bytes: vec![1, 2, 3] },
        Cmd::BufferCopy { src: 55, src_offset: 0, dst: 1, dst_offset: 0, size: 4 },
        Cmd::TextureWrite { id: 55, level: 0, layer: 0, x: 0, y: 0, width: 1, height: 1, depth: 1, bytes: vec![0; 4] },
        Cmd::GenerateMips { id: 55 },
        Cmd::TextureGrow { id: 55, mips: 3 },
        Cmd::Blit { src: fbo(55), src_color: 0, src_rect: [0, 0, 1, 1], dst: Target::Surface, dst_rect: [0, 0, 1, 1], color: true, depth: true, stencil: true, linear: true },
        quad_draw(fbo(56), st(4, 4), [1.0; 4]),
        Cmd::ProgramDelete { id: 1234 },
        Cmd::TextureDelete { id: 1234 },
        Cmd::BufferDelete { id: 1234 },
    ]);
    assert!(h.read(&fbo(55), 0, [0, 0, 1, 1]).is_none());
    assert!(h.read(&Target::Surface, 3, [0, 0, 1, 1]).is_none());
    assert!(h.dev.borrow_mut().read_pixels(999, &Target::Surface, 0, [0, 0, 1, 1]).is_none());
    h.dev.borrow_mut().run(999, &mut vec![Cmd::Present]);
    red_ok(&h);
}

#[test]
fn mismatched_lists_and_unconfigured_surfaces() {
    let Some(h) = h(4, 4) else { return };
    // Before Configure.
    h.run(vec![basic_program(), buffer(1, quad()), quad_draw(Target::Surface, st(4, 4), [1.0; 4]), Cmd::Present]);
    assert!(h.read(&Target::Surface, 0, [0, 0, 1, 1]).is_none());
    // Zero-size Configure.
    h.run(vec![cfg(0, 0), clear(Target::Surface, [1.0; 4]), quad_draw(Target::Surface, st(4, 4), [1.0; 4]), Cmd::Present]);
    assert!(h.read(&Target::Surface, 0, [0, 0, 1, 1]).is_none());
    h.run(vec![cfg(4, 4)]);
    // No attribute for location 0 (a constant (0, 0, 0, 1) stands in), and extra attributes.
    let none = draw(Target::Surface, 1, st(4, 4), vec![], 6, uni([1.0; 4]));
    let mut extra = draw(Target::Surface, 1, st(4, 4), vec![attr(0, 1, AttrType::F32, 2, 8, 0), attr(5, 1, AttrType::F32, 2, 8, 0)], 6, uni([0.0, 1.0, 0.0, 1.0]));
    extra.textures = vec![TexBind { texture: 3, sampling: sampling(Filter::Linear, Wrap::Clamp) }];
    extra.uniforms = Rc::from(vec![0u8; 4]); // too short: padded with zeros
    h.run(vec![Cmd::Draw(Box::new(none)), Cmd::Draw(Box::new(extra))]);
    assert_near(h.surf(1, 1), [0, 0, 0, 0], 0);
    // A texture program drawn with no texture list samples transparent black.
    let mut d = draw(Target::Surface, 10, st(4, 4), vec![attr(0, 1, AttrType::F32, 2, 8, 0)], 6, uni([1.0, 0.0, 0.0, 0.0]));
    d.textures.clear();
    h.run(vec![clear(Target::Surface, [1.0; 4]), tex_program(10, &fs_tex("texture_2d<f32>", "c"), TexKind::D2, SampleKind::Float), Cmd::Draw(Box::new(d))]);
    assert_near(h.surf(1, 1), [0, 0, 0, 0], 0);
}

#[test]
fn huge_counts_and_rectangles() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    let mut a = draw(Target::Surface, 1, st(4, 4), vec![attr(0, 1, AttrType::F32, 2, 8, 0)], u32::MAX, uni([1.0; 4]));
    let mut b = a.clone();
    b.first = u32::MAX - 1;
    b.count = 6;
    let mut c = a.clone();
    c.count = 6;
    c.instances = u32::MAX;
    a.state.viewport = [i32::MIN, i32::MIN, i32::MAX, i32::MAX];
    let mut v = c.clone();
    v.instances = 1;
    v.state.viewport = [-1_000_000_000, -5, i32::MAX, i32::MAX];
    v.state.scissor = Some([i32::MIN, i32::MIN, i32::MAX, i32::MAX]);
    v.state.depth_range = [f32::NAN, 7.0];
    h.run(vec![
        Cmd::Draw(Box::new(a)),
        Cmd::Draw(Box::new(b)),
        Cmd::Draw(Box::new(c)),
        Cmd::Draw(Box::new(v)),
        Cmd::Clear { target: Target::Surface, colors: vec![(0, ClearColor::F([f32::NAN; 4]))], depth: Some(f32::INFINITY), stencil: Some(u32::MAX), color_mask: [true; 4], stencil_mask: u32::MAX, scissor: Some([i32::MAX, i32::MAX, i32::MAX, i32::MAX]) },
        Cmd::Blit { src: Target::Surface, src_color: 0, src_rect: [i32::MIN, 0, i32::MAX, 1], dst: Target::Surface, dst_rect: [0, i32::MAX, i32::MAX, i32::MIN], color: true, depth: true, stencil: true, linear: true },
        Cmd::TextureCreate { id: 9, desc: tex2d(TexFormat::Rgba32F, 1 << 20, 1 << 20, 1) },
        Cmd::TextureCreate { id: 10, desc: TexDesc { kind: TexKind::Array, format: TexFormat::Rgba8, width: 4, height: 4, depth: u32::MAX, mips: 99, samples: 3 } },
        Cmd::TextureCreate { id: 11, desc: tex2d(TexFormat::Rgba8, 4, 4, 1) },
        Cmd::TextureWrite { id: 11, level: 0, layer: 0, x: u32::MAX, y: 0, width: 2, height: 2, depth: 1, bytes: vec![0; 16] },
        Cmd::TextureWrite { id: 11, level: 5, layer: 9, x: 0, y: 0, width: u32::MAX, height: u32::MAX, depth: u32::MAX, bytes: vec![] },
        Cmd::BufferWrite { id: 1, offset: u32::MAX, bytes: vec![1; 8] },
        Cmd::BufferCopy { src: 1, src_offset: u32::MAX, dst: 1, dst_offset: 0, size: u32::MAX },
        Cmd::CopyToTexture { src: Target::Surface, src_color: 0, src_x: u32::MAX, src_y: 0, dst: Attachment { texture: 11, level: 0, layer: 0 }, dst_x: 0, dst_y: 0, width: u32::MAX, height: 1 },
        Cmd::Configure { width: u32::MAX, height: 4, alpha: true, depth: true, stencil: true, samples: 4, premultiplied: true, preserve: false },
    ]);
    assert!(h.read(&Target::Surface, 0, [i32::MAX, 0, i32::MAX, 1]).is_none());
    h.run(vec![cfg(4, 4)]);
    red_ok(&h);
}

/// A small deterministic generator (xorshift64*).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn n(&mut self, max: u64) -> u32 {
        (self.next() % max.max(1)) as u32
    }
    fn wild(&mut self) -> u32 {
        match self.n(6) {
            0 => u32::MAX,
            1 => 0,
            2 => self.n(1 << 20),
            _ => self.n(9),
        }
    }
    fn i(&mut self) -> i32 {
        self.wild() as i32 - if self.n(4) == 0 { 4 } else { 0 }
    }
    fn rect(&mut self) -> Rect4 {
        [self.i(), self.i(), self.i(), self.i()]
    }
    fn bytes(&mut self) -> Vec<u8> {
        (0..self.n(80)).map(|_| self.n(256) as u8).collect()
    }
    fn target(&mut self) -> Target {
        if self.n(3) == 0 {
            return Target::Surface;
        }
        let a = |r: &mut Rng| (r.n(3) > 0).then(|| Attachment { texture: r.n(5), level: r.n(3), layer: r.n(3) });
        Target::Fbo { color: (0..self.n(4)).map(|_| a(self)).collect(), depth_stencil: a(self) }
    }
}

fn random_cmd(r: &mut Rng) -> Cmd {
    const FORMATS: [TexFormat; 8] = [TexFormat::Rgba8, TexFormat::R8, TexFormat::Rgba16F, TexFormat::Rgba32F, TexFormat::Rgba8U, TexFormat::Depth24Stencil8, TexFormat::Depth32F, TexFormat::R11G11B10F];
    const KINDS: [TexKind; 4] = [TexKind::D2, TexKind::Cube, TexKind::D3, TexKind::Array];
    match r.n(14) {
        0 => Cmd::Configure { width: r.n(9), height: r.n(9), alpha: r.n(2) == 0, depth: r.n(2) == 0, stencil: r.n(2) == 0, samples: [1, 4, 2][r.n(3) as usize], premultiplied: r.n(2) == 0, preserve: r.n(2) == 0 },
        1 => Cmd::Present,
        2 => Cmd::BufferCreate { id: r.n(5), index: r.n(2) == 0, bytes: r.bytes() },
        3 => Cmd::BufferWrite { id: r.n(5), offset: r.wild(), bytes: r.bytes() },
        4 => Cmd::BufferCopy { src: r.n(5), src_offset: r.wild(), dst: r.n(5), dst_offset: r.wild(), size: r.wild() },
        5 => {
            let desc = TexDesc { kind: KINDS[r.n(4) as usize], format: FORMATS[r.n(8) as usize], width: r.wild().min(64), height: r.wild().min(64), depth: r.n(8), mips: r.n(5), samples: [1, 4, 0][r.n(3) as usize] };
            Cmd::TextureCreate { id: r.n(5), desc }
        }
        6 => Cmd::TextureWrite { id: r.n(5), level: r.n(3), layer: r.n(3), x: r.wild(), y: r.wild(), width: r.n(5), height: r.n(5), depth: r.n(3), bytes: r.bytes() },
        7 => [Cmd::GenerateMips { id: r.n(5) }, Cmd::TextureGrow { id: r.n(5), mips: r.n(6) }, Cmd::TextureDelete { id: r.n(5) }][r.n(3) as usize].clone(),
        8 => Cmd::CopyToTexture { src: r.target(), src_color: r.n(3), src_x: r.wild(), src_y: r.n(4), dst: Attachment { texture: r.n(5), level: r.n(2), layer: r.n(2) }, dst_x: r.n(4), dst_y: r.wild(), width: r.n(5), height: r.n(5) },
        9 => {
            let fs = [FS_UNI, "garbage", "@fragment fn fs_main() -> @location(0) vec4<u32> { return vec4<u32>(1u); }"][r.n(3) as usize];
            let mut d = desc(VS, fs, [16, 0, 32][r.n(3) as usize], &[(0, Scalar::F32)]);
            if r.n(3) == 0 {
                d.textures = vec![TexSlot { texture: 0, sampler: 1, kind: KINDS[r.n(4) as usize], sample: SampleKind::Float, in_vertex: false, in_fragment: true }];
            }
            program(r.n(4), d)
        }
        10 | 11 => {
            let mut s = st(r.n(9), r.n(9));
            s.viewport = r.rect();
            s.scissor = (r.n(2) == 0).then(|| r.rect());
            let attrs = (0..r.n(3)).map(|_| VertexAttr { location: r.n(3), buffer: r.n(5), offset: r.wild(), stride: r.wild(), ty: AttrType::U16, size: r.n(6) as u8, normalized: r.n(2) == 0, integer: r.n(2) == 0, divisor: r.n(3) }).collect();
            let mut d = draw(r.target(), r.n(4), s, attrs, r.wild(), Rc::from(r.bytes()));
            d.first = r.wild();
            d.instances = r.wild().min(1000);
            d.index = (r.n(3) == 0).then(|| IndexSource { buffer: r.n(5), offset: r.wild(), wide: r.n(2) == 0 });
            d.textures = (0..r.n(2)).map(|_| TexBind { texture: r.n(5), sampling: sampling(Filter::Linear, Wrap::Repeat) }).collect();
            d.blocks = (0..r.n(2)).map(|_| BlockBind { binding: r.n(3), buffer: r.n(5), offset: r.wild(), size: r.wild() }).collect();
            Cmd::Draw(Box::new(d))
        }
        12 => Cmd::Clear { target: r.target(), colors: vec![(r.n(3), [ClearColor::F([0.5; 4]), ClearColor::I([-1; 4]), ClearColor::U([7; 4])][r.n(3) as usize])], depth: Some(0.5), stencil: Some(r.wild()), color_mask: [r.n(2) == 0; 4], stencil_mask: r.wild(), scissor: (r.n(2) == 0).then(|| r.rect()) },
        _ => Cmd::Blit { src: r.target(), src_color: r.n(3), src_rect: r.rect(), dst: r.target(), dst_rect: r.rect(), color: r.n(2) == 0, depth: r.n(2) == 0, stencil: r.n(2) == 0, linear: r.n(2) == 0 },
    }
}

#[test]
fn ten_thousand_malformed_commands() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    let mut r = Rng(0x1234_5678_9abc_def1);
    for _ in 0..100 {
        let cmds: Vec<Cmd> = (0..100).map(|_| random_cmd(&mut r)).collect();
        h.run(cmds);
        let t = r.target();
        let _ = h.read(&t, r.n(2), r.rect());
    }
    setup(&h);
    red_ok(&h);
}

#[test]
fn destroy_frees_everything() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    let before = h.stats();
    let Some(c2) = h.dev.borrow_mut().create() else { return };
    let mut cmds = vec![
        cfg(8, 8),
        basic_program(),
        buffer(1, quad()),
        Cmd::TextureCreate { id: 1, desc: tex2d(TexFormat::Rgba8, 16, 16, 5) },
        quad_draw(Target::Surface, st(8, 8), [1.0; 4]),
        Cmd::Present,
    ];
    h.dev.borrow_mut().run(c2, &mut cmds);
    let during = h.stats();
    assert_eq!(during.contexts, before.contexts + 1);
    assert!(during.texture_bytes > before.texture_bytes && during.buffer_bytes > before.buffer_bytes);
    h.dev.borrow_mut().destroy(c2);
    let after = h.stats();
    assert_eq!((after.contexts, after.buffer_bytes, after.texture_bytes, after.programs, after.pipelines), (before.contexts, before.buffer_bytes, before.texture_bytes, before.programs, before.pipelines));
    assert!(h.dev.borrow().image(c2) == 0);
}

#[test]
fn batching_and_caching() {
    let Some(mut h) = h(4, 4) else { return };
    setup(&h);
    let frame = || -> Vec<Cmd> {
        let mut v = vec![clear(Target::Surface, [0.0; 4])];
        v.extend((0..1000).map(|i| quad_draw(Target::Surface, st(4, 4), [i as f32 / 1000.0, 0.0, 0.0, 1.0])));
        v
    };
    let p0 = h.passes();
    h.run(frame());
    assert_eq!(h.passes() - p0, 1, "1,000 draws in one render pass");
    let pipelines = h.stats().pipelines;
    h.run(frame());
    h.run(frame());
    assert_eq!(h.stats().pipelines, pipelines);
    // Compositing a presented context does no GPU work for it.
    h.run(vec![Cmd::Present]);
    let p1 = h.passes();
    let _ = composite(&mut h, 4, 4);
    let _ = composite(&mut h, 4, 4);
    assert_eq!(h.passes(), p1);
}
