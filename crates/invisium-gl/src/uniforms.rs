//! `uniform*`, `uniformMatrix*` and `getUniform`.

use crate::args::*;
use crate::consts1::*;
use crate::consts2::*;
use crate::progobj::Loc;
use crate::{Arg, Context, Elem, Kind, Ret};
use invisium_glsl::{Base, Place};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Cat {
    F(u32),
    I(u32),
    U(u32),
    B(u32),
    Sampler,
    /// columns, rows.
    Mat(u32, u32),
}

#[allow(non_upper_case_globals)]
fn cat(t: u32) -> Option<Cat> {
    Some(match t {
        FLOAT => Cat::F(1),
        FLOAT_VEC2..=FLOAT_VEC4 => Cat::F(t - FLOAT_VEC2 + 2),
        INT => Cat::I(1),
        INT_VEC2..=INT_VEC4 => Cat::I(t - INT_VEC2 + 2),
        UNSIGNED_INT => Cat::U(1),
        UNSIGNED_INT_VEC2..=UNSIGNED_INT_VEC4 => Cat::U(t - UNSIGNED_INT_VEC2 + 2),
        BOOL => Cat::B(1),
        BOOL_VEC2..=BOOL_VEC4 => Cat::B(t - BOOL_VEC2 + 2),
        FLOAT_MAT2 => Cat::Mat(2, 2),
        FLOAT_MAT3 => Cat::Mat(3, 3),
        FLOAT_MAT4 => Cat::Mat(4, 4),
        FLOAT_MAT2x3 => Cat::Mat(2, 3),
        FLOAT_MAT2x4 => Cat::Mat(2, 4),
        FLOAT_MAT3x2 => Cat::Mat(3, 2),
        FLOAT_MAT3x4 => Cat::Mat(3, 4),
        FLOAT_MAT4x2 => Cat::Mat(4, 2),
        FLOAT_MAT4x3 => Cat::Mat(4, 3),
        SAMPLER_2D | SAMPLER_CUBE | SAMPLER_3D | SAMPLER_2D_SHADOW | SAMPLER_2D_ARRAY | SAMPLER_2D_ARRAY_SHADOW
        | SAMPLER_CUBE_SHADOW | INT_SAMPLER_2D | INT_SAMPLER_3D | INT_SAMPLER_CUBE | INT_SAMPLER_2D_ARRAY
        | UNSIGNED_INT_SAMPLER_2D | UNSIGNED_INT_SAMPLER_3D | UNSIGNED_INT_SAMPLER_CUBE | UNSIGNED_INT_SAMPLER_2D_ARRAY => Cat::Sampler,
        _ => return None,
    })
}

/// What a setter method sets: (kind letter, components) or a matrix.
fn setter(name: &str) -> Option<(Cat, bool)> {
    if let Some(rest) = name.strip_prefix("uniformMatrix") {
        let dims = rest.strip_suffix("fv")?;
        let (c, r) = match dims.as_bytes() {
            [c] => (c - b'0', c - b'0'),
            [c, b'x', r] => (c - b'0', r - b'0'),
            _ => return None,
        };
        return Some((Cat::Mat(c as u32, r as u32), true));
    }
    let rest = name.strip_prefix("uniform")?;
    let n = rest.as_bytes().first()?.wrapping_sub(b'0') as u32;
    let (kind, v) = match rest.get(1..)? {
        "f" => ('f', false),
        "fv" => ('f', true),
        "i" => ('i', false),
        "iv" => ('i', true),
        "ui" => ('u', false),
        "uiv" => ('u', true),
        _ => return None,
    };
    if !(1..=4).contains(&n) {
        return None;
    }
    Some((match kind {
        'f' => Cat::F(n),
        'i' => Cat::I(n),
        _ => Cat::U(n),
    }, v))
}

fn compatible(set: Cat, target: Cat) -> bool {
    match (set, target) {
        (Cat::F(a), Cat::F(b)) | (Cat::I(a), Cat::I(b)) | (Cat::U(a), Cat::U(b)) => a == b,
        (Cat::F(a) | Cat::I(a) | Cat::U(a), Cat::B(b)) => a == b,
        (Cat::I(1), Cat::Sampler) => true,
        (Cat::Mat(c, r), Cat::Mat(c2, r2)) => c == c2 && r == r2,
        _ => false,
    }
}

fn comps(c: Cat) -> u32 {
    match c {
        Cat::F(n) | Cat::I(n) | Cat::U(n) | Cat::B(n) => n,
        Cat::Sampler => 1,
        Cat::Mat(c, r) => c * r,
    }
}

pub fn is_uniform_setter(name: &str) -> bool {
    setter(name).is_some()
}

impl Context {
    pub(crate) fn uniform_call(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        let (set, v) = setter(name).ok_or(INVALID_OPERATION)?;
        let is_mat = matches!(set, Cat::Mat(..));
        let Some(lh) = obj(a, 0, Kind::UniformLocation)? else { return Ok(Ret::Undefined) };
        let n = comps(set) as usize;
        let elem = match set {
            Cat::F(_) | Cat::Mat(..) => Elem::F32,
            Cat::I(_) => Elem::I32,
            _ => Elem::U32,
        };
        let transpose = if is_mat { boolean(a, 1)? } else { false };
        let vals: Vec<f64> = if v {
            let di = if is_mat { 2 } else { 1 };
            let all = nums(a, di, elem)?;
            let off = uint(a, di + 1)? as usize;
            let len = uint(a, di + 2)? as usize;
            if off > all.len() {
                return Err(INVALID_VALUE);
            }
            let len = if len == 0 { all.len() - off } else { len };
            if off + len > all.len() {
                return Err(INVALID_VALUE);
            }
            all[off..off + len].to_vec()
        } else {
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                out.push(match set {
                    Cat::F(_) => float(a, 1 + i)? as f64,
                    Cat::I(_) => int(a, 1 + i)? as f64,
                    _ => uint(a, 1 + i)? as f64,
                });
            }
            out
        };
        let loc = *self.objs.locations.get(&lh).ok_or(INVALID_OPERATION)?;
        if self.st.program == 0 || loc.program != self.st.program {
            return Err(INVALID_OPERATION);
        }
        if transpose && !self.is2() {
            return Err(INVALID_VALUE);
        }
        if vals.is_empty() || !vals.len().is_multiple_of(n) {
            return Err(INVALID_VALUE);
        }
        let exe = self.objs.programs.get_mut(&loc.program).and_then(|p| p.exe.as_mut()).ok_or(INVALID_OPERATION)?;
        let u = exe.prog.uniforms.get(loc.uniform).ok_or(INVALID_OPERATION)?.clone();
        let target = cat(u.gl_type).ok_or(INVALID_OPERATION)?;
        if !compatible(set, target) {
            return Err(INVALID_OPERATION);
        }
        let count = vals.len() / n;
        if u.size == 1 && count > 1 {
            return Err(INVALID_OPERATION);
        }
        let count = count.min((u.size.saturating_sub(loc.elem)) as usize);
        match u.place {
            Place::Texture { slot } => {
                if vals[..count].iter().any(|&x| x < 0.0 || x >= crate::limits::MAX_COMBINED_TEXTURE_IMAGE_UNITS as f64) {
                    return Err(INVALID_VALUE);
                }
                for (e, &x) in vals[..count].iter().enumerate() {
                    if let Some(s) = exe.units.get_mut(slot as usize + loc.elem as usize + e) {
                        *s = x as u32;
                    }
                }
            }
            Place::Block { offset, stride, base, rows, cols, col_stride } => {
                let (rows, cols) = (rows as usize, cols.max(1) as usize);
                for e in 0..count {
                    let at = offset as usize + (loc.elem as usize + e) * stride as usize;
                    for c in 0..cols {
                        for r in 0..rows {
                            let i = if transpose { r * cols + c } else { c * rows + r };
                            let Some(&x) = vals.get(e * n + i) else { continue };
                            let bytes = match base {
                                Base::F32 => (x as f32).to_le_bytes(),
                                Base::I32 => (x as i32).to_le_bytes(),
                                Base::U32 => (x as u32).to_le_bytes(),
                                Base::Bool => ((x != 0.0) as u32).to_le_bytes(),
                            };
                            let p = at + c * col_stride as usize + r * 4;
                            if let Some(dst) = exe.data.get_mut(p..p + 4) {
                                dst.copy_from_slice(&bytes);
                            }
                        }
                    }
                }
                exe.shared = None;
            }
            Place::Named { .. } => return Err(INVALID_OPERATION),
        }
        Ok(Ret::Undefined)
    }

    pub(crate) fn get_uniform(&self, a: &[Arg]) -> R<Ret> {
        let p = self.program_arg(a, 0)?;
        let lh = obj(a, 1, Kind::UniformLocation)?.ok_or(INVALID_OPERATION)?;
        let exe = self.linked(p)?;
        let loc: Loc = *self.objs.locations.get(&lh).ok_or(INVALID_OPERATION)?;
        if loc.program != p {
            return Err(INVALID_OPERATION);
        }
        let u = exe.prog.uniforms.get(loc.uniform).ok_or(INVALID_OPERATION)?;
        let c = cat(u.gl_type).ok_or(INVALID_OPERATION)?;
        let (offset, stride, base, rows, cols, col_stride) = match u.place {
            Place::Texture { slot } => {
                let unit = exe.units.get((slot + loc.elem) as usize).copied().unwrap_or(0);
                return Ok(Ret::Num(unit as f64));
            }
            Place::Block { offset, stride, base, rows, cols, col_stride } => (offset, stride, base, rows as usize, cols.max(1) as usize, col_stride),
            Place::Named { .. } => return Err(INVALID_OPERATION),
        };
        let at = (offset + loc.elem * stride) as usize;
        let mut vals = Vec::with_capacity(rows * cols);
        for col in 0..cols {
            for r in 0..rows {
                let p = at + col * col_stride as usize + r * 4;
                let b = exe.data.get(p..p + 4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0; 4]);
                vals.push(match base {
                    Base::F32 => f32::from_le_bytes(b) as f64,
                    Base::I32 => i32::from_le_bytes(b) as f64,
                    Base::U32 | Base::Bool => u32::from_le_bytes(b) as f64,
                });
            }
        }
        Ok(match c {
            Cat::F(1) => Ret::Num(vals[0]),
            Cat::I(1) => Ret::Num(vals[0]),
            Cat::U(1) => Ret::Num(vals[0]),
            Cat::B(1) => Ret::Bool(vals[0] != 0.0),
            Cat::F(_) | Cat::Mat(..) => Ret::F32s(vals.iter().map(|&x| x as f32).collect()),
            Cat::I(_) => Ret::I32s(vals.iter().map(|&x| x as i32).collect()),
            Cat::U(_) => Ret::U32s(vals.iter().map(|&x| x as u32).collect()),
            Cat::B(_) => Ret::Bools(vals.iter().map(|&x| x != 0.0).collect()),
            Cat::Sampler => Ret::Num(vals[0]),
        })
    }
}
