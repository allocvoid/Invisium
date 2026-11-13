//! Fixed-function state setters.

use crate::args::*;
use crate::consts1::*;
use crate::consts2::*;
use crate::{Arg, Context, Ret};

fn is_compare(f: u32) -> bool {
    (NEVER..=ALWAYS).contains(&f)
}

fn is_stencil_op(o: u32) -> bool {
    matches!(o, ZERO | KEEP | REPLACE | INCR | DECR | INVERT | INCR_WRAP | DECR_WRAP)
}

fn faces(face: u32) -> R<&'static [usize]> {
    match face {
        FRONT => Ok(&[0]),
        BACK => Ok(&[1]),
        FRONT_AND_BACK => Ok(&[0, 1]),
        _ => Err(INVALID_ENUM),
    }
}

fn is_const_color(f: u32) -> bool {
    matches!(f, CONSTANT_COLOR | ONE_MINUS_CONSTANT_COLOR)
}

fn is_const_alpha(f: u32) -> bool {
    matches!(f, CONSTANT_ALPHA | ONE_MINUS_CONSTANT_ALPHA)
}

impl Context {
    pub(crate) fn cap(&mut self, cap: u32) -> R<&mut bool> {
        let c = &mut self.st.caps;
        Ok(match cap {
            BLEND => &mut c.blend,
            CULL_FACE => &mut c.cull_face,
            DEPTH_TEST => &mut c.depth_test,
            DITHER => &mut c.dither,
            POLYGON_OFFSET_FILL => &mut c.polygon_offset_fill,
            SAMPLE_ALPHA_TO_COVERAGE => &mut c.sample_alpha_to_coverage,
            SAMPLE_COVERAGE => &mut c.sample_coverage,
            SCISSOR_TEST => &mut c.scissor_test,
            STENCIL_TEST => &mut c.stencil_test,
            RASTERIZER_DISCARD if self.version == crate::Version::WebGl2 => &mut c.rasterizer_discard,
            _ => return Err(INVALID_ENUM),
        })
    }

    pub(crate) fn set_cap(&mut self, a: &[Arg], on: bool) -> R<Ret> {
        *self.cap(uint(a, 0)?)? = on;
        Ok(Ret::Undefined)
    }

    fn blend_mode_ok(&self, m: u32) -> bool {
        match m {
            FUNC_ADD | FUNC_SUBTRACT | FUNC_REVERSE_SUBTRACT => true,
            MIN | MAX => self.is2() || self.ext("EXT_blend_minmax"),
            _ => false,
        }
    }

    fn blend_factor_ok(&self, f: u32, dst: bool) -> bool {
        match f {
            ZERO | ONE | SRC_COLOR | ONE_MINUS_SRC_COLOR | DST_COLOR | ONE_MINUS_DST_COLOR | SRC_ALPHA
            | ONE_MINUS_SRC_ALPHA | DST_ALPHA | ONE_MINUS_DST_ALPHA | CONSTANT_COLOR | ONE_MINUS_CONSTANT_COLOR
            | CONSTANT_ALPHA | ONE_MINUS_CONSTANT_ALPHA => true,
            SRC_ALPHA_SATURATE => !dst || self.is2(),
            _ => false,
        }
    }

    pub(crate) fn set_state(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        let webgl1 = !self.is2();
        match name {
            "blendColor" | "clearColor" => {
                let mut v = [float(a, 0)?, float(a, 1)?, float(a, 2)?, float(a, 3)?];
                if name == "blendColor" || webgl1 {
                    for x in &mut v {
                        *x = x.clamp(0.0, 1.0);
                    }
                }
                if name == "blendColor" {
                    self.st.blend_color = v;
                } else {
                    self.st.clear_color = v;
                }
            }
            "blendEquation" | "blendEquationSeparate" => {
                let rgb = uint(a, 0)?;
                let alpha = if name == "blendEquation" { rgb } else { uint(a, 1)? };
                if !self.blend_mode_ok(rgb) || !self.blend_mode_ok(alpha) {
                    return Err(INVALID_ENUM);
                }
                self.st.blend_eq = [rgb, alpha];
            }
            "blendFunc" | "blendFuncSeparate" => {
                let f = if name == "blendFunc" {
                    let (s, d) = (uint(a, 0)?, uint(a, 1)?);
                    [s, d, s, d]
                } else {
                    [uint(a, 0)?, uint(a, 1)?, uint(a, 2)?, uint(a, 3)?]
                };
                if !self.blend_factor_ok(f[0], false)
                    || !self.blend_factor_ok(f[1], true)
                    || !self.blend_factor_ok(f[2], false)
                    || !self.blend_factor_ok(f[3], true)
                {
                    return Err(INVALID_ENUM);
                }
                if (is_const_color(f[0]) && is_const_alpha(f[1])) || (is_const_alpha(f[0]) && is_const_color(f[1])) {
                    return Err(INVALID_OPERATION);
                }
                self.st.blend_func = f;
            }
            "clearDepth" => self.st.clear_depth = float(a, 0)?.clamp(0.0, 1.0),
            "clearStencil" => self.st.clear_stencil = int(a, 0)?,
            "colorMask" => self.st.color_mask = [boolean(a, 0)?, boolean(a, 1)?, boolean(a, 2)?, boolean(a, 3)?],
            "cullFace" => {
                let m = uint(a, 0)?;
                faces(m)?;
                self.st.cull_face_mode = m;
            }
            "depthFunc" => {
                let f = uint(a, 0)?;
                if !is_compare(f) {
                    return Err(INVALID_ENUM);
                }
                self.st.depth_func = f;
            }
            "depthMask" => self.st.depth_mask = boolean(a, 0)?,
            "depthRange" => {
                let (n, f) = (float(a, 0)?.clamp(0.0, 1.0), float(a, 1)?.clamp(0.0, 1.0));
                if n > f {
                    return Err(INVALID_OPERATION);
                }
                self.st.depth_range = [n, f];
            }
            "frontFace" => {
                let m = uint(a, 0)?;
                if m != CW && m != CCW {
                    return Err(INVALID_ENUM);
                }
                self.st.front_face = m;
            }
            "hint" => {
                let (t, m) = (uint(a, 0)?, uint(a, 1)?);
                let deriv = t == FRAGMENT_SHADER_DERIVATIVE_HINT && (self.is2() || self.ext("OES_standard_derivatives"));
                if t != GENERATE_MIPMAP_HINT && !deriv {
                    return Err(INVALID_ENUM);
                }
                if !matches!(m, FASTEST | NICEST | DONT_CARE) {
                    return Err(INVALID_ENUM);
                }
                if deriv {
                    self.st.hint_derivative = m;
                } else {
                    self.st.hint_mipmap = m;
                }
            }
            "lineWidth" => {
                let w = float(a, 0)?;
                if w.is_nan() || w <= 0.0 {
                    return Err(INVALID_VALUE);
                }
                self.st.line_width = w;
            }
            "polygonOffset" => self.st.polygon_offset = [float(a, 0)?, float(a, 1)?],
            "sampleCoverage" => {
                self.st.sample_coverage_value = float(a, 0)?.clamp(0.0, 1.0);
                self.st.sample_coverage_invert = boolean(a, 1)?;
            }
            "scissor" | "viewport" => {
                let (x, y, w, h) = (int(a, 0)?, int(a, 1)?, int(a, 2)?, int(a, 3)?);
                if w < 0 || h < 0 {
                    return Err(INVALID_VALUE);
                }
                if name == "scissor" {
                    self.st.scissor = [x, y, w, h];
                } else {
                    let m = crate::limits::MAX_VIEWPORT_DIM;
                    self.st.viewport = [x, y, w.min(m), h.min(m)];
                }
            }
            "stencilFunc" | "stencilFuncSeparate" => {
                let (fs, i) = if name == "stencilFunc" { (faces(FRONT_AND_BACK)?, 0) } else { (faces(uint(a, 0)?)?, 1) };
                let (func, r, mask) = (uint(a, i)?, int(a, i + 1)?, uint(a, i + 2)?);
                if !is_compare(func) {
                    return Err(INVALID_ENUM);
                }
                for &f in fs {
                    let s = &mut self.st.stencil[f];
                    s.func = func;
                    s.reference = r;
                    s.value_mask = mask;
                }
            }
            "stencilMask" | "stencilMaskSeparate" => {
                let (fs, i) = if name == "stencilMask" { (faces(FRONT_AND_BACK)?, 0) } else { (faces(uint(a, 0)?)?, 1) };
                let mask = uint(a, i)?;
                for &f in fs {
                    self.st.stencil[f].write_mask = mask;
                }
            }
            "stencilOp" | "stencilOpSeparate" => {
                let (fs, i) = if name == "stencilOp" { (faces(FRONT_AND_BACK)?, 0) } else { (faces(uint(a, 0)?)?, 1) };
                let ops = [uint(a, i)?, uint(a, i + 1)?, uint(a, i + 2)?];
                if !ops.iter().all(|&o| is_stencil_op(o)) {
                    return Err(INVALID_ENUM);
                }
                for &f in fs {
                    let s = &mut self.st.stencil[f];
                    s.fail = ops[0];
                    s.zfail = ops[1];
                    s.zpass = ops[2];
                }
            }
            "activeTexture" => {
                let t = uint(a, 0)?;
                let unit = t.wrapping_sub(TEXTURE0);
                if unit >= crate::limits::MAX_COMBINED_TEXTURE_IMAGE_UNITS {
                    return Err(INVALID_ENUM);
                }
                self.st.active_unit = unit;
            }
            "pixelStorei" => self.pixel_store(uint(a, 0)?, int(a, 1)?)?,
            _ => {}
        }
        Ok(Ret::Undefined)
    }

    fn pixel_store(&mut self, pname: u32, v: i32) -> R<()> {
        let u = &mut self.st.unpack;
        let p = &mut self.st.pack;
        let webgl2 = self.version == crate::Version::WebGl2;
        match pname {
            PACK_ALIGNMENT | UNPACK_ALIGNMENT => {
                if !matches!(v, 1 | 2 | 4 | 8) {
                    return Err(INVALID_VALUE);
                }
                if pname == PACK_ALIGNMENT {
                    p.alignment = v;
                } else {
                    u.alignment = v;
                }
            }
            UNPACK_FLIP_Y_WEBGL => u.flip_y = v != 0,
            UNPACK_PREMULTIPLY_ALPHA_WEBGL => u.premultiply = v != 0,
            UNPACK_COLORSPACE_CONVERSION_WEBGL => {
                let v = v as u32;
                if v != NONE && v != BROWSER_DEFAULT_WEBGL {
                    return Err(INVALID_ENUM);
                }
                u.colorspace = v;
            }
            PACK_ROW_LENGTH | PACK_SKIP_PIXELS | PACK_SKIP_ROWS | UNPACK_ROW_LENGTH | UNPACK_IMAGE_HEIGHT
            | UNPACK_SKIP_PIXELS | UNPACK_SKIP_ROWS | UNPACK_SKIP_IMAGES
                if webgl2 =>
            {
                if v < 0 {
                    return Err(INVALID_VALUE);
                }
                *match pname {
                    PACK_ROW_LENGTH => &mut p.row_length,
                    PACK_SKIP_PIXELS => &mut p.skip_pixels,
                    PACK_SKIP_ROWS => &mut p.skip_rows,
                    UNPACK_ROW_LENGTH => &mut u.row_length,
                    UNPACK_IMAGE_HEIGHT => &mut u.image_height,
                    UNPACK_SKIP_PIXELS => &mut u.skip_pixels,
                    UNPACK_SKIP_ROWS => &mut u.skip_rows,
                    _ => &mut u.skip_images,
                } = v;
            }
            _ => return Err(INVALID_ENUM),
        }
        Ok(())
    }
}
