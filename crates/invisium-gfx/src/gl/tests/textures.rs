//! H5: textures and sampling.

use super::*;

pub(super) const VS_UV: &str = "
struct O { @builtin(position) p: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs_main(@location(0) p: vec2<f32>) -> O {
    var o: O; o.p = vec4<f32>(p.x, -p.y, 0.0, 1.0); o.uv = p * 0.5 + 0.5; return o;
}";

/// `textureSampleLevel` of a texture of type `ty` at the coordinate expression `coord`.
pub(super) fn fs_tex(ty: &str, coord: &str) -> String {
    format!(
        "struct U {{ k: vec4<f32> }};
@group(0) @binding(0) var<uniform> u: U;
@group(1) @binding(0) var t: {ty};
@group(1) @binding(1) var s: sampler;
@fragment fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {{
    let c = uv * u.k.x + vec2<f32>(u.k.y);
    return textureSampleLevel(t, s, {coord}, u.k.z);
}}"
    )
}

pub(super) fn tex_program(id: u32, fs: &str, kind: TexKind, sample: SampleKind) -> Cmd {
    let mut d = desc(VS_UV, fs, 16, &[(0, Scalar::F32)]);
    d.textures = vec![TexSlot { texture: 0, sampler: 1, kind, sample, in_vertex: false, in_fragment: true }];
    program(id, d)
}

/// Full-screen quad (buffer 1) with program `prog` sampling texture `tex`.
pub(super) fn tex_draw(prog: u32, tex: u32, s: Sampling, k: [f32; 4], w: u32, h: u32) -> Cmd {
    let mut d = draw(Target::Surface, prog, st(w, h), vec![attr(0, 1, AttrType::F32, 2, 8, 0)], 6, uni(k));
    d.textures = vec![TexBind { texture: tex, sampling: s }];
    Cmd::Draw(Box::new(d))
}

#[allow(clippy::too_many_arguments)]
fn write(id: u32, level: u32, layer: u32, x: u32, y: u32, w: u32, h: u32, depth: u32, bytes: Vec<u8>) -> Cmd {
    Cmd::TextureWrite { id, level, layer, x, y, width: w, height: h, depth, bytes }
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn setup(h: &H) {
    h.run(vec![cfg(4, 4), buffer(1, quad()), tex_program(10, &fs_tex("texture_2d<f32>", "c"), TexKind::D2, SampleKind::Float)]);
}

#[test]
fn filters_and_wrap_modes() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    h.run(vec![Cmd::TextureCreate { id: 1, desc: tex2d(TexFormat::Rgba8, 2, 1, 1) }, write(1, 0, 0, 0, 0, 2, 1, 1, [RED, GREEN].concat())]);
    h.run(vec![tex_draw(10, 1, sampling(Filter::Nearest, Wrap::Clamp), [1.0, 0.0, 0.0, 0.0], 4, 4)]);
    assert_near(h.surf(1, 0), RED, 0);
    assert_near(h.surf(2, 0), GREEN, 0);
    h.run(vec![tex_draw(10, 1, sampling(Filter::Linear, Wrap::Clamp), [1.0, 0.0, 0.0, 0.0], 4, 4)]);
    assert_near(h.surf(1, 0), [191, 64, 0, 255], 3);
    for (wrap, c2, c3) in [(Wrap::Repeat, RED, GREEN), (Wrap::Clamp, GREEN, GREEN), (Wrap::Mirror, GREEN, RED)] {
        h.run(vec![tex_draw(10, 1, sampling(Filter::Nearest, wrap), [2.0, 0.0, 0.0, 0.0], 4, 4)]);
        assert_near(h.surf(2, 1), c2, 0);
        assert_near(h.surf(3, 1), c3, 0);
    }
}

#[test]
fn sub_rectangle_write_and_texture_zero() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    h.run(vec![
        Cmd::TextureCreate { id: 1, desc: tex2d(TexFormat::Rgba8, 4, 4, 1) },
        write(1, 0, 0, 2, 2, 2, 2, 1, BLUE.repeat(4)),
        tex_draw(10, 1, sampling(Filter::Nearest, Wrap::Clamp), [1.0, 0.0, 0.0, 0.0], 4, 4),
    ]);
    assert_near(h.surf(2, 2), BLUE, 0);
    assert_near(h.surf(3, 3), BLUE, 0);
    assert_near(h.surf(1, 2), [0, 0, 0, 0], 0);
    h.run(vec![clear(Target::Surface, [1.0; 4]), tex_draw(10, 0, sampling(Filter::Linear, Wrap::Clamp), [1.0, 0.0, 0.0, 0.0], 4, 4)]);
    assert_near(h.surf(1, 1), [0, 0, 0, 0], 0);
}

#[test]
fn cube_face_3d_slice_array_layer() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    let cube = TexDesc { kind: TexKind::Cube, format: TexFormat::Rgba8, width: 1, height: 1, depth: 1, mips: 1, samples: 1 };
    let d3 = TexDesc { kind: TexKind::D3, format: TexFormat::Rgba8, width: 1, height: 1, depth: 4, mips: 1, samples: 1 };
    let arr = TexDesc { kind: TexKind::Array, format: TexFormat::Rgba8, width: 1, height: 1, depth: 3, mips: 1, samples: 1 };
    h.run(vec![
        Cmd::TextureCreate { id: 1, desc: cube },
        write(1, 0, 0, 0, 0, 1, 1, 1, RED.to_vec()),
        write(1, 0, 3, 0, 0, 1, 1, 1, GREEN.to_vec()),
        Cmd::TextureCreate { id: 2, desc: d3 },
        write(2, 0, 2, 0, 0, 1, 1, 1, BLUE.to_vec()),
        Cmd::TextureCreate { id: 3, desc: arr },
        write(3, 0, 1, 0, 0, 1, 1, 1, GREEN.to_vec()),
        tex_program(11, &fs_tex("texture_cube<f32>", "vec3<f32>(0.0, -1.0, 0.0)"), TexKind::Cube, SampleKind::Float),
        tex_program(12, &fs_tex("texture_3d<f32>", "vec3<f32>(0.5, 0.5, 0.625)"), TexKind::D3, SampleKind::Float),
        tex_program(13, &fs_tex("texture_2d_array<f32>", "c, 1"), TexKind::Array, SampleKind::Float),
    ]);
    let s = sampling(Filter::Nearest, Wrap::Clamp);
    for (prog, tex, want) in [(11, 1, GREEN), (12, 2, BLUE), (13, 3, GREEN)] {
        h.run(vec![clear(Target::Surface, [0.0; 4]), tex_draw(prog, tex, s, [1.0, 0.0, 0.0, 0.0], 4, 4)]);
        assert_near(h.surf(1, 1), want, 0);
    }
}

#[test]
fn mips_grow_and_base_level() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    let checker: Vec<u8> = (0..16).flat_map(|i| if (i % 4 + i / 4) % 2 == 0 { RED } else { BLUE }).collect();
    let mut s = sampling(Filter::Nearest, Wrap::Clamp);
    s.mip = Some(Filter::Nearest);
    h.run(vec![
        Cmd::TextureCreate { id: 1, desc: tex2d(TexFormat::Rgba8, 4, 4, 3) },
        write(1, 0, 0, 0, 0, 4, 4, 1, checker),
        Cmd::GenerateMips { id: 1 },
        tex_draw(10, 1, s, [1.0, 0.0, 2.0, 0.0], 4, 4),
    ]);
    assert_near(h.surf(1, 1), [128, 0, 128, 255], 3);
    // Grow keeps level 0; the new level can then be written and sampled.
    h.run(vec![
        Cmd::TextureCreate { id: 2, desc: tex2d(TexFormat::Rgba8, 2, 2, 1) },
        write(2, 0, 0, 0, 0, 2, 2, 1, GREEN.repeat(4)),
        Cmd::TextureGrow { id: 2, mips: 2 },
        write(2, 1, 0, 0, 0, 1, 1, 1, RED.to_vec()),
        tex_draw(10, 2, s, [1.0, 0.0, 0.0, 0.0], 4, 4),
    ]);
    assert_near(h.surf(1, 1), GREEN, 0);
    h.run(vec![tex_draw(10, 2, s, [1.0, 0.0, 1.0, 0.0], 4, 4)]);
    assert_near(h.surf(1, 1), RED, 0);
    // base_level 1: level 1 is what the shader sees as level 0.
    let mut b = s;
    b.base_level = 1;
    b.max_level = 1;
    h.run(vec![tex_draw(10, 2, b, [1.0, 0.0, 0.0, 0.0], 4, 4)]);
    assert_near(h.surf(1, 1), RED, 0);
}

#[test]
fn float_shadow_and_integer_textures() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    // Rgba32F in a Float slot, even with linear filtering requested.
    h.run(vec![
        Cmd::TextureCreate { id: 1, desc: tex2d(TexFormat::Rgba32F, 2, 2, 1) },
        write(1, 0, 0, 0, 0, 2, 2, 1, f32s(&[0.25, 0.5, 1.0, 1.0]).repeat(4)),
        tex_draw(10, 1, sampling(Filter::Linear, Wrap::Clamp), [1.0, 0.0, 0.0, 0.0], 4, 4),
    ]);
    assert_near(h.surf(1, 1), [64, 128, 255, 255], 1);

    // A depth texture cleared to 0.5, compared in a Shadow slot.
    let shadow_fs = "
struct U { k: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;
@group(1) @binding(0) var t: texture_depth_2d;
@group(1) @binding(1) var s: sampler_comparison;
@fragment fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let r = textureSampleCompareLevel(t, s, uv, u.k.x);
    return vec4<f32>(r, r, r, 1.0);
}";
    let depth_target = Target::Fbo { color: vec![], depth_stencil: Some(Attachment { texture: 2, level: 0, layer: 0 }) };
    h.run(vec![
        Cmd::TextureCreate { id: 2, desc: tex2d(TexFormat::Depth32F, 2, 2, 1) },
        Cmd::Clear { target: depth_target, colors: vec![], depth: Some(0.5), stencil: None, color_mask: [true; 4], stencil_mask: 0, scissor: None },
        tex_program(14, shadow_fs, TexKind::D2, SampleKind::Shadow),
    ]);
    let mut s = sampling(Filter::Nearest, Wrap::Clamp);
    s.compare = Some(Compare::Less);
    h.run(vec![tex_draw(14, 2, s, [0.25, 0.0, 0.0, 0.0], 4, 4)]);
    assert_near(h.surf(1, 1), [255, 255, 255, 255], 0);
    h.run(vec![tex_draw(14, 2, s, [0.75, 0.0, 0.0, 0.0], 4, 4)]);
    assert_near(h.surf(1, 1), [0, 0, 0, 255], 0);

    // Rgba8U read with textureLoad.
    let uint_fs = "
@group(1) @binding(0) var t: texture_2d<u32>;
@fragment fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(textureLoad(t, vec2<i32>(0, 0), 0)) / 255.0;
}";
    h.run(vec![
        Cmd::TextureCreate { id: 3, desc: tex2d(TexFormat::Rgba8U, 1, 1, 1) },
        write(3, 0, 0, 0, 0, 1, 1, 1, vec![10, 20, 30, 255]),
        tex_program(15, uint_fs, TexKind::D2, SampleKind::Uint),
        tex_draw(15, 3, sampling(Filter::Nearest, Wrap::Clamp), [0.0; 4], 4, 4),
    ]);
    assert_near(h.surf(2, 2), [10, 20, 30, 255], 0);
}
