//! What a draw needs besides geometry: `DrawState`, texture bindings, output checks.

use crate::args::*;
use crate::cmd::*;
use crate::consts1::*;
use crate::consts2::*;
use crate::formats::{class, sized, Class};
use crate::objects::{Att, TexParams, MAX_LEVELS};
use crate::textures::level_dims;
use crate::Context;

pub fn compare(f: u32) -> Compare {
    match f {
        NEVER => Compare::Never,
        LESS => Compare::Less,
        EQUAL => Compare::Equal,
        LEQUAL => Compare::LessEqual,
        GREATER => Compare::Greater,
        NOTEQUAL => Compare::NotEqual,
        GEQUAL => Compare::GreaterEqual,
        _ => Compare::Always,
    }
}

fn factor(f: u32) -> BlendFactor {
    match f {
        ZERO => BlendFactor::Zero,
        SRC_COLOR => BlendFactor::SrcColor,
        ONE_MINUS_SRC_COLOR => BlendFactor::OneMinusSrcColor,
        DST_COLOR => BlendFactor::DstColor,
        ONE_MINUS_DST_COLOR => BlendFactor::OneMinusDstColor,
        SRC_ALPHA => BlendFactor::SrcAlpha,
        ONE_MINUS_SRC_ALPHA => BlendFactor::OneMinusSrcAlpha,
        DST_ALPHA => BlendFactor::DstAlpha,
        ONE_MINUS_DST_ALPHA => BlendFactor::OneMinusDstAlpha,
        CONSTANT_COLOR => BlendFactor::ConstantColor,
        ONE_MINUS_CONSTANT_COLOR => BlendFactor::OneMinusConstantColor,
        CONSTANT_ALPHA => BlendFactor::ConstantAlpha,
        ONE_MINUS_CONSTANT_ALPHA => BlendFactor::OneMinusConstantAlpha,
        SRC_ALPHA_SATURATE => BlendFactor::SrcAlphaSaturate,
        _ => BlendFactor::One,
    }
}

fn blend_op(e: u32) -> BlendOp {
    match e {
        FUNC_SUBTRACT => BlendOp::Subtract,
        FUNC_REVERSE_SUBTRACT => BlendOp::ReverseSubtract,
        MIN => BlendOp::Min,
        MAX => BlendOp::Max,
        _ => BlendOp::Add,
    }
}

fn stencil_op(o: u32) -> StencilOp {
    match o {
        ZERO => StencilOp::Zero,
        REPLACE => StencilOp::Replace,
        INCR => StencilOp::Incr,
        DECR => StencilOp::Decr,
        INVERT => StencilOp::Invert,
        INCR_WRAP => StencilOp::IncrWrap,
        DECR_WRAP => StencilOp::DecrWrap,
        _ => StencilOp::Keep,
    }
}

fn wrap(w: u32) -> Wrap {
    match w {
        CLAMP_TO_EDGE => Wrap::Clamp,
        MIRRORED_REPEAT => Wrap::Mirror,
        _ => Wrap::Repeat,
    }
}

fn is_mip(min: u32) -> bool {
    !matches!(min, NEAREST | LINEAR)
}

impl Context {
    /// The fixed-function state of a draw; `None`: the draw produces nothing.
    pub(crate) fn draw_state(&self, topology: Topology, target: &Target) -> R<Option<DrawState>> {
        let s = &self.st;
        let (has_d, has_s) = self.draw_planes();
        let mask: u32 = if has_s { 0xFF } else { 0 };
        let [f, b] = s.stencil;
        let r = |v: i32| (v.clamp(0, 255) as u32) & mask;
        if r(f.reference) != r(b.reference) || f.value_mask & mask != b.value_mask & mask || f.write_mask & mask != b.write_mask & mask {
            return Err(INVALID_OPERATION);
        }
        if s.caps.rasterizer_discard {
            return Ok(None);
        }
        let tri = matches!(topology, Topology::Triangles | Topology::TriangleStrip);
        let cull = match (s.caps.cull_face, s.cull_face_mode) {
            (false, _) => None,
            (true, FRONT) => Some(Face::Front),
            (true, BACK) => Some(Face::Back),
            (true, _) if tri => return Ok(None),
            _ => None,
        };
        let mut color_mask = s.color_mask;
        if *target == Target::Surface && !self.attrs.alpha {
            color_mask[3] = false;
        }
        let face = |x: &crate::state::StencilSide| StencilFace { func: compare(x.func), fail: stencil_op(x.fail), depth_fail: stencil_op(x.zfail), pass: stencil_op(x.zpass) };
        Ok(Some(DrawState {
            topology,
            cull,
            front_ccw: s.front_face == CW,
            blend: s.caps.blend.then(|| Blend {
                color_op: blend_op(s.blend_eq[0]),
                alpha_op: blend_op(s.blend_eq[1]),
                src_color: factor(s.blend_func[0]),
                dst_color: factor(s.blend_func[1]),
                src_alpha: factor(s.blend_func[2]),
                dst_alpha: factor(s.blend_func[3]),
            }),
            blend_color: s.blend_color,
            color_mask,
            depth_test: (s.caps.depth_test && has_d).then(|| compare(s.depth_func)),
            depth_write: s.caps.depth_test && has_d && s.depth_mask,
            depth_bias: s.caps.polygon_offset_fill.then_some((s.polygon_offset[0], s.polygon_offset[1])),
            stencil: (s.caps.stencil_test && has_s).then(|| Stencil {
                front: face(&f),
                back: face(&b),
                reference: r(f.reference),
                read_mask: f.value_mask & 0xFF,
                write_mask: f.write_mask & 0xFF,
            }),
            alpha_to_coverage: s.caps.sample_alpha_to_coverage,
            viewport: s.viewport,
            depth_range: s.depth_range,
            scissor: s.caps.scissor_test.then_some(s.scissor),
        }))
    }

    /// Fragment outputs must match the class of the attachment they are written to.
    pub(crate) fn check_outputs(&self, outputs: &[(u32, Scalar)]) -> R<()> {
        for &(loc, base) in outputs {
            let Some(f) = self.draw_buffer_format(loc as usize) else { continue };
            let ok = match base {
                Scalar::F32 => matches!(class(f), Class::Norm | Class::Float),
                Scalar::I32 => class(f) == Class::Int,
                Scalar::U32 => class(f) == Class::Uint,
            };
            if !ok {
                return Err(INVALID_OPERATION);
            }
        }
        Ok(())
    }

    /// One `TexBind` per program texture slot.
    pub(crate) fn draw_textures(&self, slots: &[TexSlot], units: &[u32]) -> R<Vec<TexBind>> {
        let attached: Vec<u32> = match self.objs.framebuffers.get(&self.st.draw_fb) {
            None => vec![],
            Some(f) => f.all().filter_map(|a| if let Att::Tex { id, .. } = a { Some(id) } else { None }).collect(),
        };
        let mut used: Vec<(u32, TexKind, SampleKind)> = Vec::new();
        let mut out = Vec::with_capacity(slots.len());
        for (i, slot) in slots.iter().enumerate() {
            let unit = units.get(i).copied().unwrap_or(0);
            if used.iter().any(|u| u.0 == unit && (u.1 != slot.kind || u.2 != slot.sample)) {
                return Err(INVALID_OPERATION);
            }
            used.push((unit, slot.kind, slot.sample));
            let k = match slot.kind {
                TexKind::D2 => 0,
                TexKind::Cube => 1,
                TexKind::D3 => 2,
                TexKind::Array => 3,
            };
            let tex = self.st.units.get(unit as usize).map(|u| u[k]).unwrap_or(0);
            if tex != 0 && attached.contains(&tex) {
                return Err(INVALID_OPERATION);
            }
            let sampler = self.st.sampler_units.get(unit as usize).copied().unwrap_or(0);
            out.push(self.tex_bind(tex, slot, sampler));
        }
        Ok(out)
    }

    fn tex_bind(&self, h: u32, slot: &TexSlot, sampler: u32) -> TexBind {
        let tp = self.objs.textures.get(&h);
        let p: TexParams = match self.objs.samplers.get(&sampler) {
            Some(s) => s.params,
            None => tp.map(|t| t.params).unwrap_or_default(),
        };
        let mip = is_mip(p.min_filter);
        let (lmin, lmip) = match p.min_filter {
            NEAREST | NEAREST_MIPMAP_NEAREST => (Filter::Nearest, Some(Filter::Nearest)),
            NEAREST_MIPMAP_LINEAR => (Filter::Nearest, Some(Filter::Linear)),
            LINEAR_MIPMAP_NEAREST => (Filter::Linear, Some(Filter::Nearest)),
            _ => (Filter::Linear, Some(Filter::Linear)),
        };
        let mut sampling = Sampling {
            mag: if p.mag_filter == NEAREST { Filter::Nearest } else { Filter::Linear },
            min: lmin,
            mip: if mip { lmip } else { None },
            wrap: p.wrap.map(wrap),
            compare: (p.compare_mode == COMPARE_REF_TO_TEXTURE).then(|| compare(p.compare_func)),
            lod_min: p.min_lod,
            lod_max: p.max_lod,
            base_level: 0,
            max_level: 0,
        };
        let none = |sampling| TexBind { texture: 0, sampling };
        let Some(t) = tp else { return none(sampling) };
        if t.kind != Some(slot.kind) || t.dev.is_none() {
            return none(sampling);
        }
        let tparams = t.params;
        let mut base = tparams.base_level.max(0) as usize;
        if t.immutable {
            base = base.min(t.imm_levels as usize - 1);
        }
        let faces = t.faces();
        let Some(img) = t.image(base, 0).copied() else { return none(sampling) };
        let c = class(img.fmt);
        let kind_ok = match slot.sample {
            SampleKind::Float => matches!(c, Class::Norm | Class::Float | Class::Depth),
            SampleKind::Shadow => c == Class::Depth,
            SampleKind::Int => c == Class::Int,
            SampleKind::Uint => c == Class::Uint,
        };
        let linear = p.mag_filter == LINEAR || !matches!(p.min_filter, NEAREST | NEAREST_MIPMAP_NEAREST);
        let filterable = match c {
            Class::Norm => true,
            Class::Float => sized(img.internal).map(|s| s.filterable).unwrap_or(false),
            Class::Depth => p.compare_mode == COMPARE_REF_TO_TEXTURE,
            _ => false,
        };
        let pot = img.w.is_power_of_two() && img.h.is_power_of_two();
        let npot_bad = !self.is2() && !pot && (mip || p.wrap[0] != CLAMP_TO_EDGE || p.wrap[1] != CLAMP_TO_EDGE);
        if !kind_ok || (linear && !filterable) || npot_bad || img.w == 0 || img.h == 0 {
            return none(sampling);
        }
        let mut last = base;
        if mip {
            let chain = crate::objects::full_levels(img.w, img.h, if slot.kind == TexKind::D3 { img.d } else { 1 }) as usize;
            last = (base + chain - 1).min(MAX_LEVELS - 1).min(tparams.max_level.max(0) as usize).max(base);
            if t.immutable {
                last = last.min(t.imm_levels as usize - 1);
            }
        }
        for level in base..=last {
            let (w, hh, d) = level_dims(slot.kind, img.w, img.h, img.d, (level - base) as u32);
            for f in 0..faces {
                match t.image(level, f) {
                    Some(i) if i.on_dev && i.fmt == img.fmt && (i.w, i.h, i.d) == (w, hh, d) && (slot.kind != TexKind::Cube || i.w == i.h) => {}
                    _ => return none(sampling),
                }
            }
        }
        sampling.base_level = base as u32;
        sampling.max_level = last as u32;
        TexBind { texture: h, sampling }
    }
}
