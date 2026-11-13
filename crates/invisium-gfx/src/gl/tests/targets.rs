//! H7 (render targets, blits, copies, clears) and H8 (read_pixels).

use super::textures::{fs_tex, tex_draw, tex_program};
use super::*;

fn att(texture: u32, level: u32, layer: u32) -> Option<Attachment> {
    Some(Attachment { texture, level, layer })
}

fn setup(h: &H) {
    h.run(vec![cfg(4, 4), basic_program(), buffer(1, quad())]);
}

fn rgba(h: &H, t: &Target, color: u32, x: i32, y: i32) -> [u8; 4] {
    let (_, v) = h.read(t, color, [x, y, 1, 1]).expect("read_pixels");
    [v[0], v[1], v[2], v[3]]
}

#[test]
fn render_to_texture_then_sample_in_same_run() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    h.run(vec![
        tex_program(10, &fs_tex("texture_2d<f32>", "c"), TexKind::D2, SampleKind::Float),
        Cmd::TextureCreate { id: 5, desc: tex2d(TexFormat::Rgba8, 4, 4, 1) },
        quad_draw(fbo(5), st(4, 4), [1.0, 0.0, 1.0, 1.0]),
        tex_draw(10, 5, sampling(Filter::Nearest, Wrap::Clamp), [1.0, 0.0, 0.0, 0.0], 4, 4),
    ]);
    assert_near(h.surf(2, 1), [255, 0, 255, 255], 0);
}

#[test]
fn multiple_attachments_and_images() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    let fs = "
struct O { @location(0) a: vec4<f32>, @location(1) b: vec4<f32> };
@fragment fn fs_main() -> O { var o: O; o.a = vec4<f32>(1.0, 0.0, 0.0, 1.0); o.b = vec4<f32>(0.0, 0.0, 1.0, 1.0); return o; }";
    let mut pd = desc(VS, fs, 0, &[(0, Scalar::F32)]);
    pd.outputs.push(OutSlot { location: 1, base: Scalar::F32 });
    let two = Target::Fbo { color: vec![att(1, 0, 0), att(2, 0, 0)], depth_stencil: att(3, 0, 0) };
    let mut s = st(4, 4);
    s.depth_test = Some(Compare::Less);
    s.depth_write = true;
    h.run(vec![
        program(2, pd),
        Cmd::TextureCreate { id: 1, desc: tex2d(TexFormat::Rgba8, 4, 4, 1) },
        Cmd::TextureCreate { id: 2, desc: tex2d(TexFormat::Rgba8, 4, 4, 1) },
        Cmd::TextureCreate { id: 3, desc: tex2d(TexFormat::Depth24Stencil8, 4, 4, 1) },
        Cmd::Clear { target: two.clone(), colors: vec![], depth: Some(1.0), stencil: Some(0), color_mask: [true; 4], stencil_mask: 0xff, scissor: None },
        Cmd::Draw(Box::new(draw(two.clone(), 2, s, vec![attr(0, 1, AttrType::F32, 2, 8, 0)], 6, Rc::from(vec![])))),
    ]);
    assert_near(rgba(&h, &two, 0, 1, 1), [255, 0, 0, 255], 0);
    assert_near(rgba(&h, &two, 1, 1, 1), [0, 0, 255, 255], 0);

    // A mip level, a cube face and an array layer as attachments.
    let cube = TexDesc { kind: TexKind::Cube, format: TexFormat::Rgba8, width: 4, height: 4, depth: 1, mips: 1, samples: 1 };
    let arr = TexDesc { kind: TexKind::Array, format: TexFormat::Rgba8, width: 4, height: 4, depth: 3, mips: 1, samples: 1 };
    h.run(vec![
        Cmd::TextureCreate { id: 4, desc: tex2d(TexFormat::Rgba8, 8, 8, 2) },
        Cmd::TextureCreate { id: 5, desc: cube },
        Cmd::TextureCreate { id: 6, desc: arr },
    ]);
    for (tex, level, layer) in [(4, 1, 0), (5, 0, 2), (6, 0, 1)] {
        let t = Target::Fbo { color: vec![att(tex, level, layer)], depth_stencil: None };
        h.run(vec![quad_draw(t.clone(), st(4, 4), [0.0, 1.0, 0.0, 1.0])]);
        assert_near(rgba(&h, &t, 0, 3, 3), [0, 255, 0, 255], 0);
        // Level 0 / face 0 / layer 0 is untouched.
        let other = Target::Fbo { color: vec![att(tex, 0, 0)], depth_stencil: None };
        assert_near(rgba(&h, &other, 0, 3, 3), [0, 0, 0, 0], 0);
    }
}

#[test]
fn multisampled_fbo_resolved_by_blit() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    let ms = TexDesc { samples: 4, ..tex2d(TexFormat::Rgba8, 8, 8, 1) };
    let src = fbo(1);
    let dst = fbo(2);
    h.run(vec![
        Cmd::TextureCreate { id: 1, desc: ms },
        Cmd::TextureCreate { id: 2, desc: tex2d(TexFormat::Rgba8, 8, 8, 1) },
        buffer(3, f32s(&[-1.0, -1.0, 1.0, -1.0, -1.0, 1.0])),
        clear(src.clone(), [0.0, 0.0, 0.0, 1.0]),
        Cmd::Draw(Box::new(draw(src.clone(), 1, st(8, 8), vec![attr(0, 3, AttrType::F32, 2, 8, 0)], 3, uni([1.0, 0.0, 0.0, 1.0])))),
        Cmd::Blit { src: src.clone(), src_color: 0, src_rect: [0, 0, 8, 8], dst: dst.clone(), dst_rect: [0, 0, 8, 8], color: true, depth: false, stencil: false, linear: false },
    ]);
    let (_, px) = h.read(&dst, 0, [0, 0, 8, 8]).expect("read");
    let red: Vec<u8> = px.chunks(4).map(|p| p[0]).collect();
    assert_eq!((red[0], red[63]), (255, 0));
    assert!((0..8).any(|i| (20..235).contains(&red[i * 8 + 7 - i])), "edge {red:?}");
    // Reading the multisampled FBO directly resolves too.
    assert_eq!(rgba(&h, &src, 0, 0, 0), [255, 0, 0, 255]);
}

#[test]
fn blit_scaled_flipped_and_clipped() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    let px = [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]].concat();
    let (src, dst) = (fbo(1), fbo(2));
    let blit = |sr: Rect4, dr: Rect4| Cmd::Blit { src: src.clone(), src_color: 0, src_rect: sr, dst: dst.clone(), dst_rect: dr, color: true, depth: false, stencil: false, linear: false };
    h.run(vec![
        Cmd::TextureCreate { id: 1, desc: tex2d(TexFormat::Rgba8, 2, 2, 1) },
        Cmd::TextureWrite { id: 1, level: 0, layer: 0, x: 0, y: 0, width: 2, height: 2, depth: 1, bytes: px },
        Cmd::TextureCreate { id: 2, desc: tex2d(TexFormat::Rgba8, 4, 4, 1) },
        blit([0, 0, 2, 2], [0, 0, 4, 4]),
    ]);
    assert_eq!(rgba(&h, &dst, 0, 1, 1), [255, 0, 0, 255]);
    assert_eq!(rgba(&h, &dst, 0, 2, 1), [0, 255, 0, 255]);
    assert_eq!(rgba(&h, &dst, 0, 1, 3), [0, 0, 255, 255]);
    h.run(vec![blit([0, 0, 2, 2], [0, 4, 4, -4])]);
    assert_eq!(rgba(&h, &dst, 0, 0, 0), [0, 0, 255, 255]);
    assert_eq!(rgba(&h, &dst, 0, 3, 3), [0, 255, 0, 255]);
    h.run(vec![clear(dst.clone(), [0.0; 4]), blit([-2, 0, 4, 2], [0, 0, 4, 2])]);
    assert_eq!(rgba(&h, &dst, 0, 1, 0), [0, 0, 0, 0]);
    assert_eq!(rgba(&h, &dst, 0, 2, 0), [255, 0, 0, 255]);
    assert_eq!(rgba(&h, &dst, 0, 3, 1), [255, 255, 255, 255]);
}

#[test]
fn copy_to_texture_and_fbo_clears() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    let mut green = clear(Target::Surface, [0.0, 1.0, 0.0, 1.0]);
    if let Cmd::Clear { scissor, .. } = &mut green {
        *scissor = Some([2, 2, 2, 2]);
    }
    h.run(vec![
        clear(Target::Surface, [1.0, 0.0, 0.0, 1.0]),
        green,
        Cmd::TextureCreate { id: 1, desc: tex2d(TexFormat::Rgba8, 4, 4, 1) },
        Cmd::CopyToTexture { src: Target::Surface, src_color: 0, src_x: 1, src_y: 1, dst: Attachment { texture: 1, level: 0, layer: 0 }, dst_x: 0, dst_y: 0, width: 3, height: 3 },
    ]);
    // H8: a sub-rectangle of the surface.
    let (fmt, sub) = h.read(&Target::Surface, 0, [1, 1, 2, 2]).expect("sub-rectangle");
    assert_eq!(fmt, TexFormat::Rgba8);
    assert_eq!(sub, [[255, 0, 0, 255], [255, 0, 0, 255], [255, 0, 0, 255], [0, 255, 0, 255]].concat());
    let t = fbo(1);
    assert_eq!(rgba(&h, &t, 0, 0, 0), [255, 0, 0, 255]);
    assert_eq!(rgba(&h, &t, 0, 2, 2), [0, 255, 0, 255]);
    assert_eq!(rgba(&h, &t, 0, 3, 3), [0, 0, 0, 0]);

    // Masked and scissored clear on an FBO.
    h.run(vec![
        clear(t.clone(), [0.0, 0.0, 0.0, 1.0]),
        Cmd::Clear { target: t.clone(), colors: vec![(0, ClearColor::F([1.0; 4]))], depth: None, stencil: None, color_mask: [true, false, false, false], stencil_mask: 0, scissor: Some([0, 0, 2, 2]) },
    ]);
    assert_eq!(rgba(&h, &t, 0, 1, 1), [255, 0, 0, 255]);
    assert_eq!(rgba(&h, &t, 0, 2, 2), [0, 0, 0, 255]);

    // Integer and half-float clears, read back in their formats.
    let (u, f) = (fbo(2), fbo(3));
    h.run(vec![
        Cmd::TextureCreate { id: 2, desc: tex2d(TexFormat::Rgba8U, 2, 2, 1) },
        Cmd::TextureCreate { id: 3, desc: tex2d(TexFormat::Rgba16F, 2, 2, 1) },
        Cmd::Clear { target: u.clone(), colors: vec![(0, ClearColor::U([1, 2, 3, 200]))], depth: None, stencil: None, color_mask: [true; 4], stencil_mask: 0, scissor: None },
        Cmd::Clear { target: u.clone(), colors: vec![(0, ClearColor::U([9, 9, 9, 9]))], depth: None, stencil: None, color_mask: [true; 4], stencil_mask: 0, scissor: Some([1, 1, 1, 1]) },
        Cmd::Clear { target: f.clone(), colors: vec![(0, ClearColor::F([0.5, 1.0, 2.0, 1.0]))], depth: None, stencil: None, color_mask: [true; 4], stencil_mask: 0, scissor: None },
    ]);
    let (fmt, v) = h.read(&u, 0, [0, 0, 2, 2]).expect("Rgba8U");
    assert_eq!(fmt, TexFormat::Rgba8U);
    assert_eq!(v, [[1, 2, 3, 200], [1, 2, 3, 200], [1, 2, 3, 200], [9, 9, 9, 9]].concat());
    let (fmt, v) = h.read(&f, 0, [1, 0, 1, 1]).expect("Rgba16F");
    assert_eq!(fmt, TexFormat::Rgba16F);
    let halves: Vec<u16> = v.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    assert_eq!(halves, [0x3800, 0x3C00, 0x4000, 0x3C00]);
}
