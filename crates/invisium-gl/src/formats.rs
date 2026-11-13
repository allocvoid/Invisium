//! Which (internal format, format, type) combinations exist, and what they become on the device.

use crate::cmd::TexFormat;
use crate::cmd::TexFormat as T;
use crate::consts1::*;
use crate::consts2::*;

const HF: u32 = HALF_FLOAT;
const F: u32 = FLOAT;
const UB: u32 = UNSIGNED_BYTE;

/// How a format may be rendered to (WebGL 2 sized formats).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rend {
    No,
    Yes,
    /// With `EXT_color_buffer_float`.
    Float,
    /// With `EXT_color_buffer_float` or `EXT_color_buffer_half_float`.
    Half,
    /// With `EXT_color_buffer_half_float` only (`RGB16F`).
    HalfOnly,
}

pub struct Sized {
    pub internal: u32,
    pub format: u32,
    pub types: &'static [u32],
    pub fmt: TexFormat,
    pub rend: Rend,
    pub filterable: bool,
}

macro_rules! sized {
    ($($i:ident, $f:ident, [$($t:expr),*], $fmt:ident, $r:ident, $flt:expr;)*) => {
        pub static SIZED: &[Sized] = &[$(Sized { internal: $i, format: $f, types: &[$($t),*], fmt: T::$fmt, rend: Rend::$r, filterable: $flt }),*];
    };
}

sized! {
    R8, RED, [UB], R8, Yes, true;
    R8_SNORM, RED, [BYTE], R8Snorm, No, true;
    R16F, RED, [HF, F], R16F, Half, true;
    R32F, RED, [F], R32F, Float, false;
    R8UI, RED_INTEGER, [UB], R8U, Yes, false;
    R8I, RED_INTEGER, [BYTE], R8I, Yes, false;
    R16UI, RED_INTEGER, [UNSIGNED_SHORT], R16U, Yes, false;
    R16I, RED_INTEGER, [SHORT], R16I, Yes, false;
    R32UI, RED_INTEGER, [UNSIGNED_INT], R32U, Yes, false;
    R32I, RED_INTEGER, [INT], R32I, Yes, false;
    RG8, RG, [UB], Rg8, Yes, true;
    RG8_SNORM, RG, [BYTE], Rg8Snorm, No, true;
    RG16F, RG, [HF, F], Rg16F, Half, true;
    RG32F, RG, [F], Rg32F, Float, false;
    RG8UI, RG_INTEGER, [UB], Rg8U, Yes, false;
    RG8I, RG_INTEGER, [BYTE], Rg8I, Yes, false;
    RG16UI, RG_INTEGER, [UNSIGNED_SHORT], Rg16U, Yes, false;
    RG16I, RG_INTEGER, [SHORT], Rg16I, Yes, false;
    RG32UI, RG_INTEGER, [UNSIGNED_INT], Rg32U, Yes, false;
    RG32I, RG_INTEGER, [INT], Rg32I, Yes, false;
    RGB8, RGB, [UB], Rgba8, Yes, true;
    SRGB8, RGB, [UB], Srgb8A8, No, true;
    RGB565, RGB, [UB, UNSIGNED_SHORT_5_6_5], Rgba8, Yes, true;
    RGB8_SNORM, RGB, [BYTE], Rgba8Snorm, No, true;
    R11F_G11F_B10F, RGB, [UNSIGNED_INT_10F_11F_11F_REV, HF, F], R11G11B10F, Float, true;
    RGB9_E5, RGB, [UNSIGNED_INT_5_9_9_9_REV, HF, F], Rgba16F, No, true;
    RGB16F, RGB, [HF, F], Rgba16F, HalfOnly, true;
    RGB32F, RGB, [F], Rgba32F, No, false;
    RGB8UI, RGB_INTEGER, [UB], Rgba8U, No, false;
    RGB8I, RGB_INTEGER, [BYTE], Rgba8I, No, false;
    RGB16UI, RGB_INTEGER, [UNSIGNED_SHORT], Rgba16U, No, false;
    RGB16I, RGB_INTEGER, [SHORT], Rgba16I, No, false;
    RGB32UI, RGB_INTEGER, [UNSIGNED_INT], Rgba32U, No, false;
    RGB32I, RGB_INTEGER, [INT], Rgba32I, No, false;
    RGBA8, RGBA, [UB], Rgba8, Yes, true;
    SRGB8_ALPHA8, RGBA, [UB], Srgb8A8, Yes, true;
    RGBA8_SNORM, RGBA, [BYTE], Rgba8Snorm, No, true;
    RGB5_A1, RGBA, [UB, UNSIGNED_SHORT_5_5_5_1, UNSIGNED_INT_2_10_10_10_REV], Rgba8, Yes, true;
    RGBA4, RGBA, [UB, UNSIGNED_SHORT_4_4_4_4], Rgba8, Yes, true;
    RGB10_A2, RGBA, [UNSIGNED_INT_2_10_10_10_REV], Rgb10A2, Yes, true;
    RGBA16F, RGBA, [HF, F], Rgba16F, Half, true;
    RGBA32F, RGBA, [F], Rgba32F, Float, false;
    RGBA8UI, RGBA_INTEGER, [UB], Rgba8U, Yes, false;
    RGBA8I, RGBA_INTEGER, [BYTE], Rgba8I, Yes, false;
    RGB10_A2UI, RGBA_INTEGER, [UNSIGNED_INT_2_10_10_10_REV], Rgba16U, Yes, false;
    RGBA16UI, RGBA_INTEGER, [UNSIGNED_SHORT], Rgba16U, Yes, false;
    RGBA16I, RGBA_INTEGER, [SHORT], Rgba16I, Yes, false;
    RGBA32I, RGBA_INTEGER, [INT], Rgba32I, Yes, false;
    RGBA32UI, RGBA_INTEGER, [UNSIGNED_INT], Rgba32U, Yes, false;
    DEPTH_COMPONENT16, DEPTH_COMPONENT, [UNSIGNED_SHORT, UNSIGNED_INT], Depth16, Yes, false;
    DEPTH_COMPONENT24, DEPTH_COMPONENT, [UNSIGNED_INT], Depth24, Yes, false;
    DEPTH_COMPONENT32F, DEPTH_COMPONENT, [F], Depth32F, Yes, false;
    DEPTH24_STENCIL8, DEPTH_STENCIL, [UNSIGNED_INT_24_8], Depth24Stencil8, Yes, false;
    DEPTH32F_STENCIL8, DEPTH_STENCIL, [FLOAT_32_UNSIGNED_INT_24_8_REV], Depth32FStencil8, Yes, false;
}

pub fn sized(internal: u32) -> Option<&'static Sized> {
    SIZED.iter().find(|s| s.internal == internal)
}

pub fn is_unsized(internal: u32) -> bool {
    matches!(internal, RGBA | RGB | LUMINANCE_ALPHA | LUMINANCE | ALPHA | DEPTH_COMPONENT | DEPTH_STENCIL)
}

pub fn is_format_enum(f: u32) -> bool {
    matches!(
        f,
        RGBA | RGB | LUMINANCE_ALPHA | LUMINANCE | ALPHA | DEPTH_COMPONENT | DEPTH_STENCIL | RED | RG
            | RED_INTEGER | RG_INTEGER | RGB_INTEGER | RGBA_INTEGER
    )
}

pub fn is_type_enum(t: u32) -> bool {
    matches!(
        t,
        UNSIGNED_BYTE | BYTE | UNSIGNED_SHORT | SHORT | UNSIGNED_INT | INT | HALF_FLOAT | FLOAT
            | UNSIGNED_SHORT_5_6_5 | UNSIGNED_SHORT_4_4_4_4 | UNSIGNED_SHORT_5_5_5_1
            | UNSIGNED_INT_2_10_10_10_REV | UNSIGNED_INT_10F_11F_11F_REV | UNSIGNED_INT_5_9_9_9_REV
            | UNSIGNED_INT_24_8 | FLOAT_32_UNSIGNED_INT_24_8_REV | HALF_FLOAT_OES
    )
}

/// Context features that change which combinations exist.
#[derive(Clone, Copy)]
pub struct Feat {
    pub webgl2: bool,
    pub tex_float: bool,
    pub tex_half: bool,
    pub depth_tex: bool,
}

/// The device format of an unsized (WebGL 1 style) combination, if it exists.
pub fn unsized_fmt(feat: Feat, internal: u32, format: u32, ty: u32) -> Option<TexFormat> {
    if internal != format {
        return None;
    }
    let color = matches!(format, RGBA | RGB | LUMINANCE_ALPHA | LUMINANCE | ALPHA);
    match (format, ty) {
        (RGBA, UNSIGNED_SHORT_4_4_4_4 | UNSIGNED_SHORT_5_5_5_1) | (RGB, UNSIGNED_SHORT_5_6_5) => Some(T::Rgba8),
        (_, UNSIGNED_BYTE) if color => Some(T::Rgba8),
        (_, FLOAT) if color && feat.tex_float && !feat.webgl2 => Some(T::Rgba32F),
        (_, HALF_FLOAT_OES) if color && feat.tex_half && !feat.webgl2 => Some(T::Rgba16F),
        (DEPTH_COMPONENT, UNSIGNED_SHORT) if feat.webgl2 || feat.depth_tex => Some(T::Depth16),
        (DEPTH_COMPONENT, UNSIGNED_INT) if feat.webgl2 || feat.depth_tex => Some(T::Depth24),
        (DEPTH_STENCIL, UNSIGNED_INT_24_8) if feat.webgl2 || feat.depth_tex => Some(T::Depth24Stencil8),
        _ => None,
    }
}

/// Resolve a texture image specification. `Err` is the GL error: an unknown enum is
/// `INVALID_ENUM`, a known but invalid combination `INVALID_OPERATION`.
pub fn tex_format(feat: Feat, internal: u32, format: u32, ty: u32) -> Result<TexFormat, u32> {
    if !is_format_enum(format) || !is_type_enum(ty) {
        return Err(INVALID_ENUM);
    }
    if is_unsized(internal) {
        return unsized_fmt(feat, internal, format, ty).ok_or(INVALID_OPERATION);
    }
    if !feat.webgl2 {
        return Err(INVALID_ENUM);
    }
    let s = sized(internal).ok_or(INVALID_ENUM)?;
    if s.format == format && s.types.contains(&ty) {
        Ok(s.fmt)
    } else {
        Err(INVALID_OPERATION)
    }
}

pub use dev::{bits, class, Class};

mod dev {
use crate::cmd::TexFormat::{self, *};
use crate::consts1::*;
use crate::consts2::{RGB10_A2UI, RGB16F, RGB16I, RGB16UI, RGB32F, RGB32I, RGB32UI, RGB8, RGB8I, RGB8UI, RGB8_SNORM, RGB9_E5, SRGB8};

/// Components of the data class of a device format.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Norm,
    Float,
    Int,
    Uint,
    Depth,
}

pub fn class(f: TexFormat) -> Class {
    match f {
        R8 | Rg8 | Rgba8 | Srgb8A8 | R8Snorm | Rg8Snorm | Rgba8Snorm | Rgb10A2 => Class::Norm,
        R16F | Rg16F | Rgba16F | R32F | Rg32F | Rgba32F | R11G11B10F => Class::Float,
        R8I | R16I | R32I | Rg8I | Rg16I | Rg32I | Rgba8I | Rgba16I | Rgba32I => Class::Int,
        R8U | R16U | R32U | Rg8U | Rg16U | Rg32U | Rgba8U | Rgba16U | Rgba32U => Class::Uint,
        _ => Class::Depth,
    }
}

/// Bits of r, g, b, a, depth, stencil of an image, from its internal format where that is
/// narrower than the device format.
pub fn bits(internal: u32, f: TexFormat) -> [u32; 6] {
    match internal {
        RGBA4 => return [4, 4, 4, 4, 0, 0],
        RGB5_A1 => return [5, 5, 5, 1, 0, 0],
        RGB565 => return [5, 6, 5, 0, 0, 0],
        RGB10_A2UI => return [10, 10, 10, 2, 0, 0],
        _ => {}
    }
    let no_alpha = matches!(internal, RGB | LUMINANCE | RGB8 | SRGB8 | RGB8_SNORM | RGB9_E5 | RGB16F | RGB32F)
        || matches!(internal, RGB8UI | RGB8I | RGB16UI | RGB16I | RGB32UI | RGB32I);
    let mut b = match f {
        R8 | R8Snorm | R8I | R8U => [8, 0, 0, 0, 0, 0],
        Rg8 | Rg8Snorm | Rg8I | Rg8U => [8, 8, 0, 0, 0, 0],
        Rgba8 | Srgb8A8 | Rgba8Snorm | Rgba8I | Rgba8U => [8, 8, 8, 8, 0, 0],
        R16F | R16I | R16U => [16, 0, 0, 0, 0, 0],
        Rg16F | Rg16I | Rg16U => [16, 16, 0, 0, 0, 0],
        Rgba16F | Rgba16I | Rgba16U => [16, 16, 16, 16, 0, 0],
        R32F | R32I | R32U => [32, 0, 0, 0, 0, 0],
        Rg32F | Rg32I | Rg32U => [32, 32, 0, 0, 0, 0],
        Rgba32F | Rgba32I | Rgba32U => [32, 32, 32, 32, 0, 0],
        R11G11B10F => [11, 11, 10, 0, 0, 0],
        Rgb10A2 => [10, 10, 10, 2, 0, 0],
        Depth16 => [0, 0, 0, 0, 16, 0],
        Depth24 => [0, 0, 0, 0, 24, 0],
        Depth32F => [0, 0, 0, 0, 32, 0],
        Depth24Stencil8 => [0, 0, 0, 0, 24, 8],
        Depth32FStencil8 => [0, 0, 0, 0, 32, 8],
        Stencil8 => [0, 0, 0, 0, 0, 8],
    };
    if no_alpha {
        b[3] = 0;
    }
    if internal == ALPHA {
        b = [0, 0, 0, 8, 0, 0];
    }
    b
}
}

/// Channels a texture of this internal format has (r, g, b, a), for copy compatibility.
pub fn channels(internal: u32) -> [bool; 4] {
    match internal {
        ALPHA => [false, false, false, true],
        LUMINANCE => [true, false, false, false],
        LUMINANCE_ALPHA => [true, false, false, true],
        _ => match sized(internal).map(|s| s.format).unwrap_or(internal) {
            RED | RED_INTEGER => [true, false, false, false],
            RG | RG_INTEGER => [true, true, false, false],
            RGB | RGB_INTEGER => [true, true, true, false],
            _ => [true; 4],
        },
    }
}
