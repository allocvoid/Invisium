//! `getParameter` and the other context-level queries.

use crate::args::*;
use crate::consts1::*;
use crate::consts2::*;
use crate::formats::{bits, class, Class};
use crate::limits as l;
use crate::tables;
use crate::{Arg, Context, Kind, Ret, Version};

fn o(kind: Kind, h: u32) -> Ret {
    if h == 0 {
        Ret::Null
    } else {
        Ret::Object(kind, h)
    }
}

fn n(v: impl Into<f64>) -> Ret {
    Ret::Num(v.into())
}

impl Context {
    /// r, g, b, a, depth, stencil bits and the sample count of the draw framebuffer.
    fn draw_bits(&self) -> ([u32; 6], u32) {
        match self.objs.framebuffers.get(&self.st.draw_fb) {
            None => {
                let a = self.attrs;
                ([8, 8, 8, if a.alpha { 8 } else { 0 }, if a.depth { 24 } else { 0 }, if a.stencil { 8 } else { 0 }], if a.antialias { 4 } else { 0 })
            }
            Some(f) => {
                let mut b = [0; 6];
                let mut samples = 0;
                if let Some(i) = f.color[0].and_then(|a| self.att_info(&a)) {
                    b[..4].copy_from_slice(&bits(i.internal, i.fmt)[..4]);
                    samples = i.samples;
                }
                if let Some(i) = self.fb_depth_stencil(f).and_then(|a| self.att_info(&a)) {
                    b[4..].copy_from_slice(&bits(i.internal, i.fmt)[4..]);
                    samples = samples.max(i.samples);
                }
                (b, samples)
            }
        }
    }

    pub(crate) fn query_call(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        match name {
            "getParameter" => self.get_parameter(uint(a, 0)?),
            "getContextAttributes" => Ok(Ret::Attributes(self.attrs)),
            "getSupportedExtensions" => Ok(Ret::Strs(tables::ext_list(self.version).iter().map(|s| s.to_string()).collect())),
            "getExtension" => {
                let Arg::Str(s) = arg(a, 0) else { return Ok(Ret::Null) };
                match tables::ext_canonical(self.version, s) {
                    Some(e) => {
                        if !self.exts.contains(&e) {
                            self.exts.push(e);
                        }
                        Ok(Ret::Extension(e))
                    }
                    None => Ok(Ret::Null),
                }
            }
            "getShaderPrecisionFormat" => {
                let (shader, prec) = (uint(a, 0)?, uint(a, 1)?);
                if shader != VERTEX_SHADER && shader != FRAGMENT_SHADER {
                    return Err(INVALID_ENUM);
                }
                match prec {
                    LOW_FLOAT | MEDIUM_FLOAT | HIGH_FLOAT => Ok(Ret::Precision { range_min: 127, range_max: 127, precision: 23 }),
                    LOW_INT | MEDIUM_INT | HIGH_INT => Ok(Ret::Precision { range_min: 31, range_max: 30, precision: 0 }),
                    _ => Err(INVALID_ENUM),
                }
            }
            "getIndexedParameter" => {
                let (target, index) = (uint(a, 0)?, uint(a, 1)? as usize);
                let (list, kind) = match target {
                    UNIFORM_BUFFER_BINDING | UNIFORM_BUFFER_START | UNIFORM_BUFFER_SIZE => (&self.st.uniform_bindings, target - UNIFORM_BUFFER_BINDING),
                    TRANSFORM_FEEDBACK_BUFFER_BINDING => (&self.st.tf_bindings, 0),
                    TRANSFORM_FEEDBACK_BUFFER_START => (&self.st.tf_bindings, 1),
                    TRANSFORM_FEEDBACK_BUFFER_SIZE => (&self.st.tf_bindings, 2),
                    _ => return Err(INVALID_ENUM),
                };
                let b = list.get(index).ok_or(INVALID_VALUE)?;
                Ok(match kind {
                    0 => o(Kind::Buffer, b.buffer),
                    1 => n(b.offset as f64),
                    _ => n(b.size as f64),
                })
            }
            _ => Ok(Ret::Undefined),
        }
    }

    fn get_parameter(&self, p: u32) -> R<Ret> {
        let s = &self.st;
        let w2 = self.version == Version::WebGl2;
        let unit = s.units[s.active_unit as usize];
        let (fb_bits, samples) = self.draw_bits();
        let f32s = |v: &[f32]| Ret::F32s(v.to_vec());
        let r = match p {
            ACTIVE_TEXTURE => n(TEXTURE0 + s.active_unit),
            ALIASED_LINE_WIDTH_RANGE | ALIASED_POINT_SIZE_RANGE => f32s(&[1.0, 1.0]),
            RED_BITS | GREEN_BITS | BLUE_BITS | ALPHA_BITS | DEPTH_BITS | STENCIL_BITS => n(fb_bits[match p {
                RED_BITS => 0,
                GREEN_BITS => 1,
                BLUE_BITS => 2,
                ALPHA_BITS => 3,
                DEPTH_BITS => 4,
                _ => 5,
            }]),
            ARRAY_BUFFER_BINDING => o(Kind::Buffer, s.array_buffer),
            ELEMENT_ARRAY_BUFFER_BINDING => o(Kind::Buffer, s.element_buffer),
            BLEND | CULL_FACE | DEPTH_TEST | DITHER | POLYGON_OFFSET_FILL | SAMPLE_ALPHA_TO_COVERAGE | SAMPLE_COVERAGE
            | SCISSOR_TEST | STENCIL_TEST => Ret::Bool(self.cap_value(p)),
            RASTERIZER_DISCARD if w2 => Ret::Bool(s.caps.rasterizer_discard),
            BLEND_COLOR => f32s(&s.blend_color),
            BLEND_EQUATION_RGB => n(s.blend_eq[0]),
            BLEND_EQUATION_ALPHA => n(s.blend_eq[1]),
            BLEND_SRC_RGB => n(s.blend_func[0]),
            BLEND_DST_RGB => n(s.blend_func[1]),
            BLEND_SRC_ALPHA => n(s.blend_func[2]),
            BLEND_DST_ALPHA => n(s.blend_func[3]),
            COLOR_CLEAR_VALUE => f32s(&s.clear_color),
            COLOR_WRITEMASK => Ret::Bools(s.color_mask.to_vec()),
            COMPRESSED_TEXTURE_FORMATS => Ret::U32s(vec![]),
            CULL_FACE_MODE => n(s.cull_face_mode),
            CURRENT_PROGRAM => o(Kind::Program, s.program),
            DEPTH_CLEAR_VALUE => n(s.clear_depth),
            DEPTH_FUNC => n(s.depth_func),
            DEPTH_RANGE => f32s(&s.depth_range),
            DEPTH_WRITEMASK => Ret::Bool(s.depth_mask),
            FRAMEBUFFER_BINDING => o(Kind::Framebuffer, s.draw_fb),
            READ_FRAMEBUFFER_BINDING if w2 => o(Kind::Framebuffer, s.read_fb),
            FRONT_FACE => n(s.front_face),
            GENERATE_MIPMAP_HINT => n(s.hint_mipmap),
            FRAGMENT_SHADER_DERIVATIVE_HINT if w2 || self.ext("OES_standard_derivatives") => n(s.hint_derivative),
            IMPLEMENTATION_COLOR_READ_FORMAT | IMPLEMENTATION_COLOR_READ_TYPE => {
                let src = self.read_src().map_err(|_| INVALID_OPERATION)?;
                let (f, t) = match class(src.fmt) {
                    Class::Float => (RGBA, FLOAT),
                    Class::Int => (RGBA_INTEGER, INT),
                    Class::Uint => (RGBA_INTEGER, UNSIGNED_INT),
                    _ => (RGBA, UNSIGNED_BYTE),
                };
                n(if p == IMPLEMENTATION_COLOR_READ_FORMAT { f } else { t })
            }
            LINE_WIDTH => n(s.line_width),
            MAX_COMBINED_TEXTURE_IMAGE_UNITS => n(l::MAX_COMBINED_TEXTURE_IMAGE_UNITS),
            MAX_CUBE_MAP_TEXTURE_SIZE => n(l::MAX_CUBE_MAP_TEXTURE_SIZE),
            MAX_FRAGMENT_UNIFORM_VECTORS => n(l::MAX_FRAGMENT_UNIFORM_VECTORS),
            MAX_RENDERBUFFER_SIZE => n(l::MAX_RENDERBUFFER_SIZE),
            MAX_TEXTURE_IMAGE_UNITS => n(l::MAX_TEXTURE_IMAGE_UNITS),
            MAX_TEXTURE_SIZE => n(l::MAX_TEXTURE_SIZE),
            MAX_VARYING_VECTORS => n(l::MAX_VARYING_VECTORS),
            MAX_VERTEX_ATTRIBS => n(l::MAX_VERTEX_ATTRIBS),
            MAX_VERTEX_TEXTURE_IMAGE_UNITS => n(l::MAX_VERTEX_TEXTURE_IMAGE_UNITS),
            MAX_VERTEX_UNIFORM_VECTORS => n(l::MAX_VERTEX_UNIFORM_VECTORS),
            MAX_VIEWPORT_DIMS => Ret::I32s(vec![l::MAX_VIEWPORT_DIM, l::MAX_VIEWPORT_DIM]),
            PACK_ALIGNMENT => n(s.pack.alignment),
            UNPACK_ALIGNMENT => n(s.unpack.alignment),
            POLYGON_OFFSET_FACTOR => n(s.polygon_offset[0]),
            POLYGON_OFFSET_UNITS => n(s.polygon_offset[1]),
            RENDERBUFFER_BINDING => o(Kind::Renderbuffer, s.renderbuffer),
            RENDERER | VENDOR => Ret::Str("Mozilla".into()),
            VERSION => Ret::Str(if w2 { "WebGL 2.0" } else { "WebGL 1.0" }.into()),
            SHADING_LANGUAGE_VERSION => Ret::Str(if w2 { "WebGL GLSL ES 3.00" } else { "WebGL GLSL ES 1.0" }.into()),
            SAMPLE_BUFFERS => n((samples > 0) as u32),
            SAMPLES => n(samples),
            SAMPLE_COVERAGE_VALUE => n(s.sample_coverage_value),
            SAMPLE_COVERAGE_INVERT => Ret::Bool(s.sample_coverage_invert),
            SCISSOR_BOX => Ret::I32s(s.scissor.to_vec()),
            VIEWPORT => Ret::I32s(s.viewport.to_vec()),
            STENCIL_CLEAR_VALUE => n(s.clear_stencil),
            STENCIL_FUNC | STENCIL_BACK_FUNC => n(s.stencil[(p == STENCIL_BACK_FUNC) as usize].func),
            STENCIL_FAIL | STENCIL_BACK_FAIL => n(s.stencil[(p == STENCIL_BACK_FAIL) as usize].fail),
            STENCIL_PASS_DEPTH_FAIL | STENCIL_BACK_PASS_DEPTH_FAIL => n(s.stencil[(p == STENCIL_BACK_PASS_DEPTH_FAIL) as usize].zfail),
            STENCIL_PASS_DEPTH_PASS | STENCIL_BACK_PASS_DEPTH_PASS => n(s.stencil[(p == STENCIL_BACK_PASS_DEPTH_PASS) as usize].zpass),
            STENCIL_REF | STENCIL_BACK_REF => n(s.stencil[(p == STENCIL_BACK_REF) as usize].reference),
            STENCIL_VALUE_MASK | STENCIL_BACK_VALUE_MASK => n(s.stencil[(p == STENCIL_BACK_VALUE_MASK) as usize].value_mask),
            STENCIL_WRITEMASK | STENCIL_BACK_WRITEMASK => n(s.stencil[(p == STENCIL_BACK_WRITEMASK) as usize].write_mask),
            SUBPIXEL_BITS => n(l::SUBPIXEL_BITS),
            TEXTURE_BINDING_2D => o(Kind::Texture, unit[0]),
            TEXTURE_BINDING_CUBE_MAP => o(Kind::Texture, unit[1]),
            UNPACK_FLIP_Y_WEBGL => Ret::Bool(s.unpack.flip_y),
            UNPACK_PREMULTIPLY_ALPHA_WEBGL => Ret::Bool(s.unpack.premultiply),
            UNPACK_COLORSPACE_CONVERSION_WEBGL => n(s.unpack.colorspace),
            DRAW_BUFFER0..=DRAW_BUFFER7 if w2 || self.ext("WEBGL_draw_buffers") => {
                let i = (p - DRAW_BUFFER0) as usize;
                n(match self.objs.framebuffers.get(&s.draw_fb) {
                    None => if i == 0 { s.surface_draw } else { NONE },
                    Some(f) => f.draw_buffers[i],
                })
            }
            MAX_DRAW_BUFFERS | MAX_COLOR_ATTACHMENTS if w2 || self.ext("WEBGL_draw_buffers") => n(l::MAX_DRAW_BUFFERS),
            VERTEX_ARRAY_BINDING if w2 || self.ext("OES_vertex_array_object") => o(Kind::VertexArray, s.vao),
            _ if w2 => return self.get_parameter2(p),
            _ => return Err(INVALID_ENUM),
        };
        Ok(r)
    }

    fn cap_value(&self, cap: u32) -> bool {
        let c = &self.st.caps;
        match cap {
            BLEND => c.blend,
            CULL_FACE => c.cull_face,
            DEPTH_TEST => c.depth_test,
            DITHER => c.dither,
            POLYGON_OFFSET_FILL => c.polygon_offset_fill,
            SAMPLE_ALPHA_TO_COVERAGE => c.sample_alpha_to_coverage,
            SAMPLE_COVERAGE => c.sample_coverage,
            SCISSOR_TEST => c.scissor_test,
            STENCIL_TEST => c.stencil_test,
            _ => false,
        }
    }

    /// Parameters only WebGL 2 has.
    fn get_parameter2(&self, p: u32) -> R<Ret> {
        let s = &self.st;
        let unit = s.units[s.active_unit as usize];
        Ok(match p {
            COPY_READ_BUFFER_BINDING => o(Kind::Buffer, s.copy_read),
            COPY_WRITE_BUFFER_BINDING => o(Kind::Buffer, s.copy_write),
            PIXEL_PACK_BUFFER_BINDING => o(Kind::Buffer, s.pixel_pack),
            PIXEL_UNPACK_BUFFER_BINDING => o(Kind::Buffer, s.pixel_unpack),
            UNIFORM_BUFFER_BINDING => o(Kind::Buffer, s.uniform_buffer),
            TRANSFORM_FEEDBACK_BUFFER_BINDING => o(Kind::Buffer, s.tf_buffer),
            TEXTURE_BINDING_3D => o(Kind::Texture, unit[2]),
            TEXTURE_BINDING_2D_ARRAY => o(Kind::Texture, unit[3]),
            SAMPLER_BINDING => o(Kind::Sampler, s.sampler_units[s.active_unit as usize]),
            TRANSFORM_FEEDBACK_BINDING => o(Kind::TransformFeedback, s.tf),
            TRANSFORM_FEEDBACK_ACTIVE => Ret::Bool(s.tf_active),
            TRANSFORM_FEEDBACK_PAUSED => Ret::Bool(s.tf_paused),
            READ_BUFFER => n(match self.objs.framebuffers.get(&s.read_fb) {
                None => s.surface_read,
                Some(f) => f.read_buffer,
            }),
            PACK_ROW_LENGTH => n(s.pack.row_length),
            PACK_SKIP_PIXELS => n(s.pack.skip_pixels),
            PACK_SKIP_ROWS => n(s.pack.skip_rows),
            UNPACK_ROW_LENGTH => n(s.unpack.row_length),
            UNPACK_IMAGE_HEIGHT => n(s.unpack.image_height),
            UNPACK_SKIP_PIXELS => n(s.unpack.skip_pixels),
            UNPACK_SKIP_ROWS => n(s.unpack.skip_rows),
            UNPACK_SKIP_IMAGES => n(s.unpack.skip_images),
            MAX_3D_TEXTURE_SIZE => n(l::MAX_3D_TEXTURE_SIZE),
            MAX_ARRAY_TEXTURE_LAYERS => n(l::MAX_ARRAY_TEXTURE_LAYERS),
            MAX_CLIENT_WAIT_TIMEOUT_WEBGL | MAX_SERVER_WAIT_TIMEOUT => n(0),
            MAX_COMBINED_UNIFORM_BLOCKS => n(l::MAX_COMBINED_UNIFORM_BLOCKS),
            MAX_VERTEX_UNIFORM_BLOCKS => n(l::MAX_VERTEX_UNIFORM_BLOCKS),
            MAX_FRAGMENT_UNIFORM_BLOCKS => n(l::MAX_FRAGMENT_UNIFORM_BLOCKS),
            MAX_VERTEX_UNIFORM_COMPONENTS => n(l::MAX_VERTEX_UNIFORM_VECTORS * 4),
            MAX_FRAGMENT_UNIFORM_COMPONENTS => n(l::MAX_FRAGMENT_UNIFORM_VECTORS * 4),
            MAX_COMBINED_VERTEX_UNIFORM_COMPONENTS => n(l::MAX_VERTEX_UNIFORM_BLOCKS * l::MAX_UNIFORM_BLOCK_SIZE / 4 + l::MAX_VERTEX_UNIFORM_VECTORS * 4),
            MAX_COMBINED_FRAGMENT_UNIFORM_COMPONENTS => n(l::MAX_FRAGMENT_UNIFORM_BLOCKS * l::MAX_UNIFORM_BLOCK_SIZE / 4 + l::MAX_FRAGMENT_UNIFORM_VECTORS * 4),
            MAX_ELEMENT_INDEX => n(u32::MAX),
            MAX_ELEMENTS_INDICES | MAX_ELEMENTS_VERTICES => n(1_048_576),
            MAX_VARYING_COMPONENTS | MAX_VERTEX_OUTPUT_COMPONENTS | MAX_FRAGMENT_INPUT_COMPONENTS => n(l::MAX_VARYING_VECTORS * 4),
            MAX_PROGRAM_TEXEL_OFFSET => n(7),
            MIN_PROGRAM_TEXEL_OFFSET => n(-8),
            MAX_SAMPLES => n(l::MAX_SAMPLES),
            MAX_TEXTURE_LOD_BIAS => n(l::MAX_TEXTURE_LOD_BIAS),
            MAX_TRANSFORM_FEEDBACK_INTERLEAVED_COMPONENTS => n(64),
            MAX_TRANSFORM_FEEDBACK_SEPARATE_ATTRIBS | MAX_TRANSFORM_FEEDBACK_SEPARATE_COMPONENTS => n(l::MAX_TRANSFORM_FEEDBACK_SEPARATE_ATTRIBS),
            MAX_UNIFORM_BLOCK_SIZE => n(l::MAX_UNIFORM_BLOCK_SIZE),
            MAX_UNIFORM_BUFFER_BINDINGS => n(l::MAX_UNIFORM_BUFFER_BINDINGS),
            UNIFORM_BUFFER_OFFSET_ALIGNMENT => n(l::UNIFORM_BUFFER_OFFSET_ALIGNMENT),
            _ => return Err(INVALID_ENUM),
        })
    }
}
