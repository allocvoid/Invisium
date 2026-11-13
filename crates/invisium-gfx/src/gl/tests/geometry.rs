//! H3 (geometry) and H4 (uniforms).

use super::*;

/// Position at location 0, colour (vec4) at location 1, optional offset at location 2.
const VS_COLOR: &str = "
struct O { @builtin(position) p: vec4<f32>, @location(0) c: vec4<f32> };
@vertex fn vs_main(@location(0) p: vec2<f32>, @location(1) c: vec4<f32>, @location(2) off: vec2<f32>) -> O {
    var o: O;
    o.p = vec4<f32>(p.x + off.x, -(p.y + off.y), 0.0, 1.0);
    o.c = c;
    return o;
}";

const FS_VARY: &str = "
@fragment fn fs_main(@location(0) c: vec4<f32>) -> @location(0) vec4<f32> { return c; }";

fn color_program() -> Cmd {
    program(2, desc(VS_COLOR, FS_VARY, 0, &[(0, Scalar::F32), (1, Scalar::F32), (2, Scalar::F32)]))
}

fn red_quad_ok(h: &H) {
    for (x, y) in [(0, 0), (3, 3), (1, 2)] {
        assert_near(h.surf(x, y), [255, 0, 0, 255], 1);
    }
}

#[test]
fn triangle_and_indices_with_offset() {
    let Some(h) = h(4, 4) else { return };
    h.run(vec![cfg(4, 4), basic_program(), buffer(1, quad())]);
    h.run(vec![quad_draw(Target::Surface, st(4, 4), [1.0, 0.0, 0.0, 1.0])]);
    red_quad_ok(&h);
    let corners = f32s(&[-1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0]);
    for wide in [false, true] {
        let idx: Vec<u32> = vec![99, 99, 0, 1, 2, 0, 2, 3];
        let bytes: Vec<u8> = if wide {
            idx.iter().flat_map(|i| i.to_le_bytes()).collect()
        } else {
            idx.iter().flat_map(|&i| (i as u16).to_le_bytes()).collect()
        };
        let mut d = draw(Target::Surface, 1, st(4, 4), vec![attr(0, 2, AttrType::F32, 2, 8, 0)], 6, uni([1.0, 0.0, 0.0, 1.0]));
        d.index = Some(IndexSource { buffer: 3, offset: if wide { 8 } else { 4 }, wide });
        h.run(vec![
            clear(Target::Surface, [0.0; 4]),
            buffer(2, corners.clone()),
            Cmd::BufferCreate { id: 3, index: true, bytes },
            Cmd::Draw(Box::new(d)),
        ]);
        red_quad_ok(&h);
    }
}

#[test]
fn instancing_divisors_and_constants() {
    let Some(h) = h(4, 4) else { return };
    let small = f32s(&[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0]);
    let offsets = f32s(&[-1.0, -1.0, 0.0, -1.0, -1.0, 0.0, 0.0, 0.0]);
    let colors = f32s(&[1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
    let mut off = attr(2, 2, AttrType::F32, 2, 8, 0);
    off.divisor = 1;
    let mut col = attr(1, 3, AttrType::F32, 4, 16, 0);
    col.divisor = 2;
    let mut d = draw(Target::Surface, 2, st(4, 4), vec![attr(0, 1, AttrType::F32, 2, 8, 0), off, col], 6, Rc::from(vec![]));
    d.instances = 4;
    h.run(vec![cfg(4, 4), color_program(), buffer(1, small), buffer(2, offsets), buffer(3, colors), Cmd::Draw(Box::new(d))]);
    assert_near(h.surf(0, 0), [255, 0, 0, 255], 1); // instance 0
    assert_near(h.surf(3, 1), [255, 0, 0, 255], 1); // instance 1
    assert_near(h.surf(0, 3), [0, 255, 0, 255], 1); // instance 2
    assert_near(h.surf(3, 2), [0, 255, 0, 255], 1); // instance 3
    // Constant colour and offset.
    let mut d = draw(Target::Surface, 2, st(4, 4), vec![attr(0, 4, AttrType::F32, 2, 8, 0)], 6, Rc::from(vec![]));
    d.consts = vec![
        ConstAttr { location: 1, value: ConstValue::F([0.0, 0.0, 1.0, 1.0]) },
        ConstAttr { location: 2, value: ConstValue::F([0.0, 0.0, 0.0, 0.0]) },
    ];
    h.run(vec![buffer(4, quad()), Cmd::Draw(Box::new(d))]);
    assert_near(h.surf(1, 1), [0, 0, 255, 255], 1);
}

#[test]
fn interleaved_and_packed_formats() {
    let Some(h) = h(4, 4) else { return };
    // Interleaved x, y, r, g, b, a with stride 24.
    let corners = [[-1.0f32, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
    let mut inter = Vec::new();
    for c in corners {
        inter.extend(f32s(&[c[0], c[1], 0.0, 1.0, 1.0, 1.0]));
    }
    let d = draw(
        Target::Surface,
        2,
        st(4, 4),
        vec![attr(0, 1, AttrType::F32, 2, 24, 0), attr(1, 1, AttrType::F32, 4, 24, 8)],
        6,
        Rc::from(vec![]),
    );
    h.run(vec![cfg(4, 4), color_program(), buffer(1, inter), Cmd::Draw(Box::new(d))]);
    assert_near(h.surf(2, 2), [0, 255, 255, 255], 1);

    // Normalised I16 x2 positions, normalised U8 x4 colour, then U8 x3 (converted, alpha 1).
    let pos: Vec<u8> = corners.iter().flat_map(|c| c.iter().flat_map(|v| ((*v * 32767.0) as i16).to_le_bytes())).collect();
    let mut p = attr(0, 2, AttrType::I16, 2, 4, 0);
    p.normalized = true;
    for (size, bytes, want) in [(4u8, [255u8, 0, 128, 255], [255, 0, 128, 255]), (3, [0, 255, 0, 0], [0, 255, 0, 255])] {
        let mut c = attr(1, 3, AttrType::U8, size, 4, 0);
        c.normalized = true;
        let d = draw(Target::Surface, 2, st(4, 4), vec![p, c], 6, Rc::from(vec![]));
        h.run(vec![clear(Target::Surface, [0.0; 4]), buffer(2, pos.clone()), buffer(3, bytes.repeat(6)), Cmd::Draw(Box::new(d))]);
        assert_near(h.surf(1, 2), want, 1);
    }

    // F32 x3 positions with z, read by a vec3 input.
    let p3: Vec<f32> = corners.iter().flat_map(|c| [c[0], c[1], 0.5]).collect();
    let d = draw(Target::Surface, 3, st(4, 4), vec![attr(0, 4, AttrType::F32, 3, 12, 0)], 6, uni([0.0, 0.0, 1.0, 1.0]));
    h.run(vec![
        program(3, desc(VS3, FS_UNI, 16, &[(0, Scalar::F32)])),
        buffer(4, f32s(&p3)),
        Cmd::Draw(Box::new(d)),
    ]);
    assert_near(h.surf(3, 0), [0, 0, 255, 255], 1);

    // Non-normalised I8 positions read as floats (converted on the CPU).
    let i8pos: Vec<u8> = corners.iter().flat_map(|c| [c[0] as i8 as u8, c[1] as i8 as u8]).collect();
    let d = draw(Target::Surface, 1, st(4, 4), vec![attr(0, 5, AttrType::I8, 2, 2, 0)], 6, uni([1.0, 1.0, 0.0, 1.0]));
    h.run(vec![basic_program(), buffer(5, i8pos), Cmd::Draw(Box::new(d))]);
    assert_near(h.surf(0, 3), [255, 255, 0, 255], 1);
}

#[test]
fn integer_attribute() {
    let Some(h) = h(4, 4) else { return };
    let vs = "
struct O { @builtin(position) p: vec4<f32>, @location(0) @interpolate(flat) k: vec4<u32> };
@vertex fn vs_main(@location(0) p: vec2<f32>, @location(1) k: vec4<u32>) -> O {
    var o: O; o.p = vec4<f32>(p.x, -p.y, 0.0, 1.0); o.k = k; return o;
}";
    let fs = "
@fragment fn fs_main(@location(0) @interpolate(flat) k: vec4<u32>) -> @location(0) vec4<f32> {
    return vec4<f32>(k) / 1000.0;
}";
    let mut k = attr(1, 2, AttrType::U16, 4, 8, 0);
    k.integer = true;
    let vals: Vec<u8> = [1000u16, 500, 0, 1000].repeat(6).iter().flat_map(|v| v.to_le_bytes()).collect();
    let d = draw(Target::Surface, 4, st(4, 4), vec![attr(0, 1, AttrType::F32, 2, 8, 0), k], 6, Rc::from(vec![]));
    h.run(vec![
        cfg(4, 4),
        program(4, desc(vs, fs, 0, &[(0, Scalar::F32), (1, Scalar::U32)])),
        buffer(1, quad()),
        buffer(2, vals),
        Cmd::Draw(Box::new(d)),
    ]);
    assert_near(h.surf(1, 1), [255, 128, 0, 255], 1);
}

#[test]
fn uniforms_per_draw_and_named_blocks() {
    let Some(h) = h(4, 4) else { return };
    let mut left = st(4, 4);
    left.scissor = Some([0, 0, 2, 4]);
    let mut right = st(4, 4);
    right.scissor = Some([2, 0, 2, 4]);
    h.run(vec![cfg(4, 4), basic_program(), buffer(1, quad())]);
    let before = h.passes();
    h.run(vec![
        quad_draw(Target::Surface, left, [1.0, 0.0, 0.0, 1.0]),
        quad_draw(Target::Surface, right, [0.0, 1.0, 0.0, 1.0]),
    ]);
    assert_eq!(h.passes() - before, 1, "both draws share one pass");
    assert_near(h.surf(0, 1), [255, 0, 0, 255], 1);
    assert_near(h.surf(3, 1), [0, 255, 0, 255], 1);

    let fs = "
struct B { c: vec4<f32> };
@group(0) @binding(1) var<uniform> b: B;
@fragment fn fs_main() -> @location(0) vec4<f32> { return b.c; }";
    let mut pd = desc(VS, fs, 0, &[(0, Scalar::F32)]);
    pd.blocks = vec![BlockSlot { binding: 1, bytes: 16 }];
    let mut data = vec![0u8; 512];
    data[256..272].copy_from_slice(&f32s(&[0.0, 0.0, 1.0, 1.0]));
    data[16..32].copy_from_slice(&f32s(&[1.0, 0.0, 1.0, 1.0]));
    h.run(vec![program(5, pd), buffer(6, data)]);
    for (offset, want) in [(256, [0, 0, 255, 255]), (16, [255, 0, 255, 255])] {
        let mut d = draw(Target::Surface, 5, st(4, 4), vec![attr(0, 1, AttrType::F32, 2, 8, 0)], 6, Rc::from(vec![]));
        d.blocks = vec![BlockBind { binding: 1, buffer: 6, offset, size: 16 }];
        h.run(vec![Cmd::Draw(Box::new(d))]);
        assert_near(h.surf(2, 2), want, 1);
    }
}
