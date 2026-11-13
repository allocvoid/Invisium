//! Milestone 4 follow-up: error routing, lazy CPU copies of buffers, the per-context cap.

use super::*;
use std::sync::atomic::Ordering::Relaxed;

/// A validation error outside any error scope (a zero-sized texture).
fn provoke(r: &crate::Renderer) {
    let _ = r.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("deliberately invalid"),
        size: wgpu::Extent3d { width: 0, height: 0, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
}

#[test]
fn uncaptured_errors_never_panic_and_are_routed() {
    // A renderer that never created its WebGL device: its own error is reported, not a panic.
    {
        let _lock = GPU.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(r) = pollster::block_on(crate::Renderer::new_headless(4, 4, crate::GpuOptions::default())) else { return };
        provoke(&r);
        let _ = r.device.poll(wgpu::Maintain::Wait);
        assert_eq!(r.errors.own_errors.load(Relaxed), 1);
        assert_eq!(r.errors.gl_errors.load(Relaxed), 0);
    }
    // With the WebGL device: an error while GL commands run counts as a page error.
    let Some(h) = h(4, 4) else { return };
    let e = h.r.errors.clone();
    e.gl_running.store(true, Relaxed);
    provoke(&h.r);
    e.gl_running.store(false, Relaxed);
    assert_eq!((e.own_errors.load(Relaxed), e.gl_errors.load(Relaxed)), (0, 1));
    provoke(&h.r);
    assert_eq!((e.own_errors.load(Relaxed), e.gl_errors.load(Relaxed)), (1, 1));
    // Running commands leaves the flag down.
    h.run(vec![cfg(4, 4)]);
    assert!(!e.gl_running.load(Relaxed));
}

#[test]
fn buffers_get_cpu_copies_only_when_needed() {
    let Some(h) = h(4, 4) else { return };
    h.run(vec![cfg(4, 4), basic_program(), buffer(1, quad())]);
    let quad_gpu = (48 + 16) as usize; // padded GPU size of the quad buffer
    h.run(vec![quad_draw(Target::Surface, st(4, 4), [1.0, 0.0, 0.0, 1.0])]);
    assert_eq!(h.stats().buffer_bytes, quad_gpu, "created and drawn from: no copy");

    // A uniform block in buffer 2: alpha 1, red 0, then red set to 1.0 by an unaligned write.
    let fs = "
struct B { c: vec4<f32> };
@group(0) @binding(1) var<uniform> b: B;
@fragment fn fs_main() -> @location(0) vec4<f32> { return b.c; }";
    let mut pd = desc(VS, fs, 0, &[(0, Scalar::F32)]);
    pd.blocks = vec![BlockSlot { binding: 1, bytes: 16 }];
    let block_gpu = 16 + 16;
    h.run(vec![
        program(5, pd),
        buffer(2, f32s(&[0.0, 0.0, 0.0, 0.0])),
        Cmd::BufferWrite { id: 2, offset: 12, bytes: f32s(&[1.0]) },
    ]);
    assert_eq!(h.stats().buffer_bytes, quad_gpu + block_gpu, "aligned write: no copy");
    let mut d = draw(Target::Surface, 5, st(4, 4), vec![attr(0, 1, AttrType::F32, 2, 8, 0)], 6, Rc::from(vec![]));
    d.blocks = vec![BlockBind { binding: 1, buffer: 2, offset: 0, size: 16 }];
    h.run(vec![
        Cmd::BufferWrite { id: 2, offset: 2, bytes: vec![0x80, 0x3F, 0x00] },
        clear(Target::Surface, [0.0; 4]),
        Cmd::Draw(Box::new(d)),
    ]);
    assert_near(h.surf(1, 1), [255, 0, 0, 255], 0);
    assert_eq!(h.stats().buffer_bytes, quad_gpu + block_gpu + 16, "the unaligned write's copy is counted");

    // An unaligned copy reads the source's copy; the destination gets one too.
    h.run(vec![
        buffer(3, vec![0; 16]),
        Cmd::BufferCopy { src: 2, src_offset: 0, dst: 3, dst_offset: 0, size: 15 },
        Cmd::BufferWrite { id: 3, offset: 15, bytes: vec![0x3F] },
    ]);
    let mut d = draw(Target::Surface, 5, st(4, 4), vec![attr(0, 1, AttrType::F32, 2, 8, 0)], 6, Rc::from(vec![]));
    d.blocks = vec![BlockBind { binding: 1, buffer: 3, offset: 0, size: 16 }];
    h.run(vec![clear(Target::Surface, [0.0; 4]), Cmd::Draw(Box::new(d))]);
    assert_near(h.surf(1, 1), [255, 0, 0, 255], 0);
}

#[test]
fn read_buffer_ranges() {
    let Some(h) = h(4, 4) else { return };
    let initial: Vec<u8> = (0..20).collect();
    h.run(vec![buffer(1, initial), Cmd::BufferWrite { id: 1, offset: 8, bytes: vec![100, 101, 102, 103] }]);
    let read = |off, len| h.dev.borrow_mut().read_buffer(h.ctx, 1, off, len);
    // From the GPU (no copy exists, none is kept), with an unaligned start and length.
    assert_eq!(read(5, 9), Some(vec![5, 6, 7, 100, 101, 102, 103, 12, 13]));
    assert_eq!(h.stats().buffer_bytes, 20 + 16, "no copy kept");
    assert_eq!(read(18, 2), Some(vec![18, 19]));
    assert_eq!(read(18, 3), None);
    assert_eq!(read(u32::MAX, 2), None);
    assert_eq!(h.dev.borrow_mut().read_buffer(h.ctx, 9, 0, 1), None);
    assert_eq!(h.dev.borrow_mut().read_buffer(999, 1, 0, 1), None);
    // From the CPU copy an unaligned write made.
    h.run(vec![Cmd::BufferWrite { id: 1, offset: 1, bytes: vec![200] }]);
    assert_eq!(read(0, 4), Some(vec![0, 200, 2, 3]));
}

#[test]
fn enormous_draws_are_refused_quickly() {
    let Some(h) = h(4, 4) else { return };
    h.run(vec![cfg(4, 4), basic_program(), buffer(1, quad())]);
    // An attribute-less program, as for `drawArrays` without enabled arrays.
    h.run(vec![program(2, desc(VS, FS_UNI, 16, &[]))]);
    let mut left = st(4, 4);
    left.scissor = Some([0, 0, 2, 4]);
    let mut right = st(4, 4);
    right.scissor = Some([2, 0, 2, 4]);
    let mut huge = draw(Target::Surface, 2, st(4, 4), vec![], (1 << 31) - 1, uni([0.0, 0.0, 1.0, 1.0]));
    let mut many = draw(Target::Surface, 1, st(4, 4), vec![attr(0, 1, AttrType::F32, 2, 8, 0)], 3, uni([0.0, 0.0, 1.0, 1.0]));
    many.instances = (1 << 31) - 1;
    huge.first = 1 << 31; // first + count still fits in u32
    let start = std::time::Instant::now();
    h.run(vec![
        clear(Target::Surface, [0.0; 4]),
        quad_draw(Target::Surface, left, [1.0, 0.0, 0.0, 1.0]),
        Cmd::Draw(Box::new(huge)),
        Cmd::Draw(Box::new(many)),
        quad_draw(Target::Surface, right, [0.0, 1.0, 0.0, 1.0]),
    ]);
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert_near(h.surf(1, 1), [255, 0, 0, 255], 0);
    assert_near(h.surf(3, 2), [0, 255, 0, 255], 0);
}

#[test]
fn context_memory_cap() {
    let Some(h) = h(4, 4) else { return };
    h.r.gl_inner().borrow_mut().gpu.ctx_cap = 3000;
    h.run(vec![buffer(1, vec![0; 2000]), buffer(2, vec![0; 2000])]);
    assert_eq!(h.stats().buffer_bytes, 2016, "second buffer refused");
    h.run(vec![Cmd::TextureCreate { id: 1, desc: tex2d(TexFormat::Rgba8, 16, 16, 1) }]);
    assert_eq!(h.stats().texture_bytes, 0, "texture over the cap refused");
    h.run(vec![Cmd::TextureCreate { id: 2, desc: tex2d(TexFormat::Rgba8, 8, 8, 1) }]);
    assert_eq!(h.stats().texture_bytes, 256);
    // Replacing a buffer frees its old bytes first.
    h.run(vec![buffer(1, vec![0; 2400])]);
    assert_eq!(h.stats().buffer_bytes, 2416);
}
