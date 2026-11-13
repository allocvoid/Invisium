//! H6: draw state.

use super::*;

fn blend(src: BlendFactor, dst: BlendFactor, op: BlendOp) -> Option<Blend> {
    Some(Blend { color_op: op, alpha_op: op, src_color: src, dst_color: dst, src_alpha: src, dst_alpha: dst })
}

fn setup(h: &H) {
    h.run(vec![cfg(4, 4), basic_program(), buffer(1, quad())]);
}

#[test]
fn blending() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    let cases = [
        ([0.0, 0.0, 1.0, 1.0], [1.0, 0.0, 0.0, 0.5], blend(BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha, BlendOp::Add), [128, 0, 127]),
        ([0.25, 0.0, 0.0, 1.0], [0.5, 0.0, 0.0, 0.0], blend(BlendFactor::One, BlendFactor::One, BlendOp::Add), [191, 0, 0]),
        ([0.5, 0.5, 0.5, 1.0], [0.25, 0.75, 0.5, 1.0], blend(BlendFactor::One, BlendFactor::One, BlendOp::Min), [64, 128, 128]),
        ([0.5, 0.5, 0.5, 1.0], [0.25, 0.75, 0.5, 1.0], blend(BlendFactor::One, BlendFactor::One, BlendOp::Max), [128, 191, 128]),
    ];
    for (bg, c, b, want) in cases {
        let mut s = st(4, 4);
        s.blend = b;
        h.run(vec![clear(Target::Surface, bg), quad_draw(Target::Surface, s, c)]);
        let p = h.surf(1, 1);
        assert_near([p[0], p[1], p[2], 0], [want[0], want[1], want[2], 0], 2);
    }
    // Blend colour.
    let mut s = st(4, 4);
    s.blend = blend(BlendFactor::ConstantColor, BlendFactor::Zero, BlendOp::Add);
    s.blend_color = [0.5, 0.25, 1.0, 1.0];
    h.run(vec![clear(Target::Surface, [0.0; 4]), quad_draw(Target::Surface, s, [1.0; 4])]);
    assert_near(h.surf(2, 2), [128, 64, 255, 255], 1);
    // Colour mask.
    let mut s = st(4, 4);
    s.color_mask = [false, true, false, true];
    h.run(vec![clear(Target::Surface, [0.0; 4]), quad_draw(Target::Surface, s, [1.0; 4])]);
    assert_near(h.surf(2, 2), [0, 255, 0, 255], 0);
}

/// A full-screen quad at depth `z` (program 3): its vertex buffer, then the draw.
fn z_draw(z: f32, c: [f32; 4], s: DrawState) -> [Cmd; 2] {
    let corners = [[-1.0f32, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
    let v: Vec<f32> = corners.iter().flat_map(|p| [p[0], p[1], z]).collect();
    let id = 100 + (z * 100.0) as u32;
    let d = Cmd::Draw(Box::new(draw(Target::Surface, 3, s, vec![attr(0, id, AttrType::F32, 3, 12, 0)], 6, uni(c))));
    [buffer(id, f32s(&v)), d]
}

#[test]
fn depth_test_write_and_range() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    h.run(vec![program(3, desc(VS3, FS_UNI, 16, &[(0, Scalar::F32)]))]);
    let mut less = st(4, 4);
    less.depth_test = Some(Compare::Less);
    less.depth_write = true;
    let mut greater = less;
    greater.depth_test = Some(Compare::Greater);
    greater.depth_write = false;
    let mut cmds = vec![clear(Target::Surface, [0.0; 4])];
    cmds.extend(z_draw(0.5, [1.0, 0.0, 0.0, 1.0], less));
    cmds.extend(z_draw(0.75, [0.0, 1.0, 0.0, 1.0], less));
    h.run(cmds);
    assert_near(h.surf(1, 1), [255, 0, 0, 255], 0);
    h.run(z_draw(0.75, [0.0, 0.0, 1.0, 1.0], greater).into());
    assert_near(h.surf(1, 1), [0, 0, 255, 255], 0);
    // Depth range [0.5, 1] maps z = 0 to depth 0.5.
    let mut ranged = less;
    ranged.depth_range = [0.5, 1.0];
    let mut cmds = vec![clear(Target::Surface, [0.0; 4])];
    cmds.extend(z_draw(0.0, [1.0, 0.0, 0.0, 1.0], ranged));
    h.run(cmds);
    h.run(z_draw(0.6, [0.0, 1.0, 0.0, 1.0], less).into());
    assert_near(h.surf(1, 1), [255, 0, 0, 255], 0);
    h.run(z_draw(0.4, [0.0, 0.0, 1.0, 1.0], less).into());
    assert_near(h.surf(1, 1), [0, 0, 255, 255], 0);
}

#[test]
fn stencil_replace_then_equal() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    let face = |func, pass| StencilFace { func, fail: StencilOp::Keep, depth_fail: StencilOp::Keep, pass };
    let mut write = st(4, 4);
    write.stencil = Some(Stencil {
        front: face(Compare::Always, StencilOp::Replace),
        back: face(Compare::Always, StencilOp::Replace),
        reference: 1,
        read_mask: 0xff,
        write_mask: 0xff,
    });
    write.scissor = Some([0, 0, 2, 4]);
    write.color_mask = [false; 4];
    let mut test = st(4, 4);
    test.stencil = Some(Stencil {
        front: face(Compare::Equal, StencilOp::Keep),
        back: face(Compare::Equal, StencilOp::Keep),
        reference: 1,
        read_mask: 0xff,
        write_mask: 0,
    });
    h.run(vec![
        clear(Target::Surface, [0.0; 4]),
        quad_draw(Target::Surface, write, [1.0; 4]),
        quad_draw(Target::Surface, test, [0.0, 1.0, 0.0, 1.0]),
    ]);
    assert_near(h.surf(1, 2), [0, 255, 0, 255], 0);
    assert_near(h.surf(2, 2), [0, 0, 0, 0], 0);
}

#[test]
fn culling_scissor_viewport() {
    let Some(h) = h(4, 4) else { return };
    setup(&h);
    // The quad's triangles are clockwise in device terms (y is mirrored).
    for (front_ccw, drawn) in [(false, true), (true, false)] {
        let mut s = st(4, 4);
        s.cull = Some(Face::Back);
        s.front_ccw = front_ccw;
        h.run(vec![clear(Target::Surface, [0.0; 4]), quad_draw(Target::Surface, s, [1.0, 0.0, 0.0, 1.0])]);
        let want = if drawn { [255, 0, 0, 255] } else { [0, 0, 0, 0] };
        assert_near(h.surf(1, 1), want, 0);
    }
    let mut s = st(4, 4);
    s.scissor = Some([1, 1, 2, 2]);
    h.run(vec![clear(Target::Surface, [0.0; 4]), quad_draw(Target::Surface, s, [1.0; 4])]);
    assert_near(h.surf(0, 0), [0, 0, 0, 0], 0);
    assert_near(h.surf(1, 1), [255; 4], 0);
    assert_near(h.surf(3, 3), [0, 0, 0, 0], 0);
    let mut s = st(4, 4);
    s.viewport = [0, 0, 2, 2];
    h.run(vec![clear(Target::Surface, [0.0; 4]), quad_draw(Target::Surface, s, [1.0; 4])]);
    assert_near(h.surf(1, 1), [255; 4], 0);
    assert_near(h.surf(2, 1), [0, 0, 0, 0], 0);
    assert_near(h.surf(1, 2), [0, 0, 0, 0], 0);
}
