//! The context's fixed-function state and its initial values.

use crate::consts1::*;

#[derive(Clone, Copy, Default)]
pub struct Caps {
    pub blend: bool,
    pub cull_face: bool,
    pub depth_test: bool,
    pub dither: bool,
    pub polygon_offset_fill: bool,
    pub sample_alpha_to_coverage: bool,
    pub sample_coverage: bool,
    pub scissor_test: bool,
    pub stencil_test: bool,
    pub rasterizer_discard: bool,
}

#[derive(Clone, Copy)]
pub struct StencilSide {
    pub func: u32,
    pub reference: i32,
    pub value_mask: u32,
    pub write_mask: u32,
    pub fail: u32,
    pub zfail: u32,
    pub zpass: u32,
}

#[derive(Clone, Copy)]
pub struct Pack {
    pub alignment: i32,
    pub row_length: i32,
    pub skip_pixels: i32,
    pub skip_rows: i32,
}

#[derive(Clone, Copy)]
pub struct Unpack {
    pub alignment: i32,
    pub row_length: i32,
    pub image_height: i32,
    pub skip_pixels: i32,
    pub skip_rows: i32,
    pub skip_images: i32,
    pub flip_y: bool,
    pub premultiply: bool,
    pub colorspace: u32,
}

#[derive(Clone, Copy, Default)]
pub struct IndexedBuffer {
    pub buffer: u32,
    pub offset: i64,
    pub size: i64,
}

pub struct State {
    pub caps: Caps,
    pub blend_color: [f32; 4],
    /// rgb, alpha.
    pub blend_eq: [u32; 2],
    /// src rgb, dst rgb, src alpha, dst alpha.
    pub blend_func: [u32; 4],
    pub clear_color: [f32; 4],
    pub clear_depth: f32,
    pub clear_stencil: i32,
    pub color_mask: [bool; 4],
    pub cull_face_mode: u32,
    pub depth_func: u32,
    pub depth_mask: bool,
    pub depth_range: [f32; 2],
    pub front_face: u32,
    pub hint_mipmap: u32,
    pub hint_derivative: u32,
    pub line_width: f32,
    /// factor, units.
    pub polygon_offset: [f32; 2],
    pub sample_coverage_value: f32,
    pub sample_coverage_invert: bool,
    pub scissor: [i32; 4],
    pub viewport: [i32; 4],
    /// front, back.
    pub stencil: [StencilSide; 2],
    pub pack: Pack,
    pub unpack: Unpack,
    pub active_unit: u32,
    pub array_buffer: u32,
    pub element_buffer: u32,
    pub copy_read: u32,
    pub copy_write: u32,
    pub pixel_pack: u32,
    pub pixel_unpack: u32,
    pub uniform_buffer: u32,
    pub tf_buffer: u32,
    pub uniform_bindings: Vec<IndexedBuffer>,
    pub tf_bindings: Vec<IndexedBuffer>,
    /// Per unit: 2D, cube, 3D, 2D array.
    pub units: Vec<[u32; 4]>,
    pub renderbuffer: u32,
    pub draw_fb: u32,
    pub read_fb: u32,
    /// `drawBuffers` / `readBuffer` of the drawing buffer (`BACK` or `NONE`).
    pub surface_draw: u32,
    pub surface_read: u32,
    /// Milestone 2: current program, bound vertex array (0 = default), current attribute
    /// values, sampler per unit, active queries (target, query), transform feedback.
    pub program: u32,
    pub vao: u32,
    pub attrib_values: Vec<crate::cmd::ConstValue>,
    pub sampler_units: Vec<u32>,
    pub queries: Vec<(u32, u32)>,
    pub tf: u32,
    pub tf_active: bool,
    pub tf_paused: bool,
}

impl State {
    pub fn new(width: u32, height: u32) -> State {
        let side = StencilSide { func: ALWAYS, reference: 0, value_mask: 0xFFFF_FFFF, write_mask: 0xFFFF_FFFF, fail: KEEP, zfail: KEEP, zpass: KEEP };
        let (w, h) = (width.min(i32::MAX as u32) as i32, height.min(i32::MAX as u32) as i32);
        State {
            caps: Caps { dither: true, ..Caps::default() },
            blend_color: [0.0; 4],
            blend_eq: [FUNC_ADD; 2],
            blend_func: [ONE, ZERO, ONE, ZERO],
            clear_color: [0.0; 4],
            clear_depth: 1.0,
            clear_stencil: 0,
            color_mask: [true; 4],
            cull_face_mode: BACK,
            depth_func: LESS,
            depth_mask: true,
            depth_range: [0.0, 1.0],
            front_face: CCW,
            hint_mipmap: DONT_CARE,
            hint_derivative: DONT_CARE,
            line_width: 1.0,
            polygon_offset: [0.0; 2],
            sample_coverage_value: 1.0,
            sample_coverage_invert: false,
            scissor: [0, 0, w, h],
            viewport: [0, 0, w.min(8192), h.min(8192)],
            stencil: [side; 2],
            pack: Pack { alignment: 4, row_length: 0, skip_pixels: 0, skip_rows: 0 },
            unpack: Unpack {
                alignment: 4,
                row_length: 0,
                image_height: 0,
                skip_pixels: 0,
                skip_rows: 0,
                skip_images: 0,
                flip_y: false,
                premultiply: false,
                colorspace: BROWSER_DEFAULT_WEBGL,
            },
            active_unit: 0,
            array_buffer: 0,
            element_buffer: 0,
            copy_read: 0,
            copy_write: 0,
            pixel_pack: 0,
            pixel_unpack: 0,
            uniform_buffer: 0,
            tf_buffer: 0,
            uniform_bindings: vec![IndexedBuffer::default(); crate::limits::MAX_UNIFORM_BUFFER_BINDINGS as usize],
            tf_bindings: vec![IndexedBuffer::default(); 4],
            units: vec![[0; 4]; crate::limits::MAX_COMBINED_TEXTURE_IMAGE_UNITS as usize],
            renderbuffer: 0,
            draw_fb: 0,
            read_fb: 0,
            surface_draw: BACK,
            surface_read: BACK,
            program: 0,
            vao: 0,
            attrib_values: vec![crate::progobj::default_attrib_value(); crate::limits::MAX_VERTEX_ATTRIBS as usize],
            sampler_units: vec![0; crate::limits::MAX_COMBINED_TEXTURE_IMAGE_UNITS as usize],
            queries: Vec::new(),
            tf: 0,
            tf_active: false,
            tf_paused: false,
        }
    }
}
