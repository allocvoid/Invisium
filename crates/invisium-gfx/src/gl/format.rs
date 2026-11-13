//! Mappings from the command contract's enums to wgpu.

use crate::draw::gl::*;
use wgpu::TextureFormat as F;

/// Component class of a texture format as a shader sees it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Class {
    Float,
    Sint,
    Uint,
    Depth,
    Stencil,
}

/// The wgpu format storing `f`. `Depth32FStencil8` falls back to `Depth24PlusStencil8` when the
/// device lacks the feature.
pub(crate) fn tex_format(f: TexFormat, features: wgpu::Features) -> F {
    use TexFormat::*;
    match f {
        R8 => F::R8Unorm,
        Rg8 => F::Rg8Unorm,
        Rgba8 => F::Rgba8Unorm,
        Srgb8A8 => F::Rgba8UnormSrgb,
        R8Snorm => F::R8Snorm,
        Rg8Snorm => F::Rg8Snorm,
        Rgba8Snorm => F::Rgba8Snorm,
        R16F => F::R16Float,
        Rg16F => F::Rg16Float,
        Rgba16F => F::Rgba16Float,
        R32F => F::R32Float,
        Rg32F => F::Rg32Float,
        Rgba32F => F::Rgba32Float,
        R11G11B10F => F::Rg11b10Ufloat,
        Rgb10A2 => F::Rgb10a2Unorm,
        R8I => F::R8Sint,
        R8U => F::R8Uint,
        R16I => F::R16Sint,
        R16U => F::R16Uint,
        R32I => F::R32Sint,
        R32U => F::R32Uint,
        Rg8I => F::Rg8Sint,
        Rg8U => F::Rg8Uint,
        Rg16I => F::Rg16Sint,
        Rg16U => F::Rg16Uint,
        Rg32I => F::Rg32Sint,
        Rg32U => F::Rg32Uint,
        Rgba8I => F::Rgba8Sint,
        Rgba8U => F::Rgba8Uint,
        Rgba16I => F::Rgba16Sint,
        Rgba16U => F::Rgba16Uint,
        Rgba32I => F::Rgba32Sint,
        Rgba32U => F::Rgba32Uint,
        Depth16 => F::Depth16Unorm,
        Depth24 => F::Depth24Plus,
        Depth32F => F::Depth32Float,
        Depth24Stencil8 => F::Depth24PlusStencil8,
        Depth32FStencil8 => {
            if features.contains(wgpu::Features::DEPTH32FLOAT_STENCIL8) {
                F::Depth32FloatStencil8
            } else {
                F::Depth24PlusStencil8
            }
        }
        Stencil8 => F::Stencil8,
    }
}

pub(crate) fn class(f: F) -> Class {
    match f {
        F::Stencil8 => Class::Stencil,
        _ if f.is_depth_stencil_format() => Class::Depth,
        _ => match f.sample_type(None, None) {
            Some(wgpu::TextureSampleType::Sint) => Class::Sint,
            Some(wgpu::TextureSampleType::Uint) => Class::Uint,
            _ => Class::Float,
        },
    }
}

pub(crate) fn features(f: F, dev: wgpu::Features) -> wgpu::TextureFormatFeatures {
    f.guaranteed_format_features(dev)
}

pub(crate) fn renderable(f: F, dev: wgpu::Features) -> bool {
    features(f, dev).allowed_usages.contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
}

pub(crate) fn filterable(f: F, dev: wgpu::Features) -> bool {
    matches!(f.sample_type(None, Some(dev)), Some(wgpu::TextureSampleType::Float { filterable: true }))
}

pub(crate) fn blendable(f: F, dev: wgpu::Features) -> bool {
    features(f, dev).flags.contains(wgpu::TextureFormatFeatureFlags::BLENDABLE)
}

pub(crate) fn msaa4(f: F, dev: wgpu::Features) -> bool {
    features(f, dev).flags.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4)
}

pub(crate) fn compare(c: Compare) -> wgpu::CompareFunction {
    use wgpu::CompareFunction as C;
    match c {
        Compare::Never => C::Never,
        Compare::Less => C::Less,
        Compare::Equal => C::Equal,
        Compare::LessEqual => C::LessEqual,
        Compare::Greater => C::Greater,
        Compare::NotEqual => C::NotEqual,
        Compare::GreaterEqual => C::GreaterEqual,
        Compare::Always => C::Always,
    }
}

pub(crate) fn filter(f: Filter) -> wgpu::FilterMode {
    match f {
        Filter::Nearest => wgpu::FilterMode::Nearest,
        Filter::Linear => wgpu::FilterMode::Linear,
    }
}

pub(crate) fn wrap(w: Wrap) -> wgpu::AddressMode {
    match w {
        Wrap::Repeat => wgpu::AddressMode::Repeat,
        Wrap::Clamp => wgpu::AddressMode::ClampToEdge,
        Wrap::Mirror => wgpu::AddressMode::MirrorRepeat,
    }
}

pub(crate) fn blend_factor(f: BlendFactor) -> wgpu::BlendFactor {
    use wgpu::BlendFactor as B;
    match f {
        BlendFactor::Zero => B::Zero,
        BlendFactor::One => B::One,
        BlendFactor::SrcColor => B::Src,
        BlendFactor::OneMinusSrcColor => B::OneMinusSrc,
        BlendFactor::DstColor => B::Dst,
        BlendFactor::OneMinusDstColor => B::OneMinusDst,
        BlendFactor::SrcAlpha => B::SrcAlpha,
        BlendFactor::OneMinusSrcAlpha => B::OneMinusSrcAlpha,
        BlendFactor::DstAlpha => B::DstAlpha,
        BlendFactor::OneMinusDstAlpha => B::OneMinusDstAlpha,
        BlendFactor::ConstantColor | BlendFactor::ConstantAlpha => B::Constant,
        BlendFactor::OneMinusConstantColor | BlendFactor::OneMinusConstantAlpha => B::OneMinusConstant,
        BlendFactor::SrcAlphaSaturate => B::SrcAlphaSaturated,
    }
}

fn blend_op(o: BlendOp) -> wgpu::BlendOperation {
    match o {
        BlendOp::Add => wgpu::BlendOperation::Add,
        BlendOp::Subtract => wgpu::BlendOperation::Subtract,
        BlendOp::ReverseSubtract => wgpu::BlendOperation::ReverseSubtract,
        BlendOp::Min => wgpu::BlendOperation::Min,
        BlendOp::Max => wgpu::BlendOperation::Max,
    }
}

fn component(op: BlendOp, src: BlendFactor, dst: BlendFactor) -> wgpu::BlendComponent {
    let operation = blend_op(op);
    // WebGPU requires factors of One for Min/Max (GL ignores them there).
    let (src_factor, dst_factor) = if matches!(op, BlendOp::Min | BlendOp::Max) {
        (wgpu::BlendFactor::One, wgpu::BlendFactor::One)
    } else {
        (blend_factor(src), blend_factor(dst))
    };
    wgpu::BlendComponent { src_factor, dst_factor, operation }
}

pub(crate) fn blend(b: &Blend) -> wgpu::BlendState {
    wgpu::BlendState {
        color: component(b.color_op, b.src_color, b.dst_color),
        alpha: component(b.alpha_op, b.src_alpha, b.dst_alpha),
    }
}

/// The blend constant: `ConstantAlpha` factors read alpha in every channel, which wgpu cannot
/// express separately, so the colour channels take the alpha when only alpha factors are used.
pub(crate) fn blend_constant(b: &Option<Blend>, c: [f32; 4]) -> wgpu::Color {
    let uses_alpha = b.is_some_and(|b| {
        [b.src_color, b.dst_color]
            .iter()
            .any(|f| matches!(f, BlendFactor::ConstantAlpha | BlendFactor::OneMinusConstantAlpha))
    });
    let f = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) as f64 } else { 0.0 };
    if uses_alpha {
        let a = f(c[3]);
        wgpu::Color { r: a, g: a, b: a, a }
    } else {
        wgpu::Color { r: f(c[0]), g: f(c[1]), b: f(c[2]), a: f(c[3]) }
    }
}

pub(crate) fn stencil_op(o: StencilOp) -> wgpu::StencilOperation {
    use wgpu::StencilOperation as S;
    match o {
        StencilOp::Keep => S::Keep,
        StencilOp::Zero => S::Zero,
        StencilOp::Replace => S::Replace,
        StencilOp::Incr => S::IncrementClamp,
        StencilOp::Decr => S::DecrementClamp,
        StencilOp::Invert => S::Invert,
        StencilOp::IncrWrap => S::IncrementWrap,
        StencilOp::DecrWrap => S::DecrementWrap,
    }
}

pub(crate) fn stencil_face(f: &StencilFace) -> wgpu::StencilFaceState {
    wgpu::StencilFaceState {
        compare: compare(f.func),
        fail_op: stencil_op(f.fail),
        depth_fail_op: stencil_op(f.depth_fail),
        pass_op: stencil_op(f.pass),
    }
}

pub(crate) fn topology(t: Topology) -> wgpu::PrimitiveTopology {
    match t {
        Topology::Points => wgpu::PrimitiveTopology::PointList,
        Topology::Lines => wgpu::PrimitiveTopology::LineList,
        Topology::LineStrip => wgpu::PrimitiveTopology::LineStrip,
        Topology::Triangles => wgpu::PrimitiveTopology::TriangleList,
        Topology::TriangleStrip => wgpu::PrimitiveTopology::TriangleStrip,
    }
}

pub(crate) fn scalar_of(c: Class) -> Option<Scalar> {
    match c {
        Class::Float => Some(Scalar::F32),
        Class::Sint => Some(Scalar::I32),
        Class::Uint => Some(Scalar::U32),
        _ => None,
    }
}

pub(crate) fn attr_type_bytes(t: AttrType) -> u32 {
    match t {
        AttrType::I8 | AttrType::U8 => 1,
        AttrType::I16 | AttrType::U16 | AttrType::F16 => 2,
        _ => 4,
    }
}

/// Bytes one element of `a` occupies in its buffer.
pub(crate) fn attr_bytes(a: &VertexAttr) -> u32 {
    match a.ty {
        AttrType::I1010102 | AttrType::U1010102 => 4,
        t => attr_type_bytes(t) * a.size.clamp(1, 4) as u32,
    }
}

/// A vertex format that reads `a` exactly as GL would for a shader input of class `base`, or
/// `None` when the data has to be converted on the CPU first.
pub(crate) fn direct_vertex_format(a: &VertexAttr, base: Scalar) -> Option<wgpu::VertexFormat> {
    use wgpu::VertexFormat as V;
    use AttrType::*;
    let n = a.size;
    if !(1..=4).contains(&n) {
        return None;
    }
    let f = match (base, a.ty, a.normalized, a.integer) {
        (Scalar::F32, F32, _, false) => [V::Float32, V::Float32x2, V::Float32x3, V::Float32x4][(n - 1) as usize],
        (Scalar::F32, F16, _, false) if n == 2 || n == 4 => [V::Float16x2, V::Float16x4][(n / 2 - 1) as usize],
        (Scalar::F32, U8, true, false) if n != 3 => [V::Unorm8, V::Unorm8x2, V::Unorm8x4][(n / 2) as usize],
        (Scalar::F32, I8, true, false) if n != 3 => [V::Snorm8, V::Snorm8x2, V::Snorm8x4][(n / 2) as usize],
        (Scalar::F32, U16, true, false) if n != 3 => [V::Unorm16, V::Unorm16x2, V::Unorm16x4][(n / 2) as usize],
        (Scalar::F32, I16, true, false) if n != 3 => [V::Snorm16, V::Snorm16x2, V::Snorm16x4][(n / 2) as usize],
        (Scalar::F32, U1010102, true, false) if n == 4 => V::Unorm10_10_10_2,
        (Scalar::I32, I8, _, true) if n != 3 => [V::Sint8, V::Sint8x2, V::Sint8x4][(n / 2) as usize],
        (Scalar::I32, I16, _, true) if n != 3 => [V::Sint16, V::Sint16x2, V::Sint16x4][(n / 2) as usize],
        (Scalar::I32, I32, _, true) => [V::Sint32, V::Sint32x2, V::Sint32x3, V::Sint32x4][(n - 1) as usize],
        (Scalar::U32, U8, _, true) if n != 3 => [V::Uint8, V::Uint8x2, V::Uint8x4][(n / 2) as usize],
        (Scalar::U32, U16, _, true) if n != 3 => [V::Uint16, V::Uint16x2, V::Uint16x4][(n / 2) as usize],
        (Scalar::U32, U32, _, true) => [V::Uint32, V::Uint32x2, V::Uint32x3, V::Uint32x4][(n - 1) as usize],
        _ => return None,
    };
    Some(f)
}

/// The 32-bit format with `n` components of class `base` (converted data and constants).
pub(crate) fn wide_format(base: Scalar, n: u8) -> wgpu::VertexFormat {
    use wgpu::VertexFormat as V;
    let i = (n.clamp(1, 4) - 1) as usize;
    match base {
        Scalar::F32 => [V::Float32, V::Float32x2, V::Float32x3, V::Float32x4][i],
        Scalar::I32 => [V::Sint32, V::Sint32x2, V::Sint32x3, V::Sint32x4][i],
        Scalar::U32 => [V::Uint32, V::Uint32x2, V::Uint32x3, V::Uint32x4][i],
    }
}
