//! WGSL type names and the uniform-buffer layout of GLSL types.
//!
//! Layout rules (a function of the GLSL type alone, so both stages agree): scalars 4/4, `vec2`
//! 8/8, `vec3` 12/16, `vec4` 16/16, `bool` stored as `u32`; a matrix is its columns at a
//! stride of 8 (two rows) or 16; arrays and structures start on 16 bytes, structure sizes are
//! rounded up to 16, and array elements of scalar or vector type are padded to 16 bytes
//! (wrapped in a one-member structure in WGSL).

use crate::ir::*;

pub fn round_up(a: u32, x: u32) -> u32 {
    x.div_ceil(a).saturating_mul(a)
}

/// WGSL type of a value.
pub fn wty(m: &Module, t: &Ty) -> String {
    match t {
        Ty::Void => "void".into(),
        Ty::S(k) => k.wgsl().into(),
        Ty::V(k, n) => format!("vec{n}<{}>", k.wgsl()),
        Ty::M(c, r) => format!("mat{c}x{r}<f32>"),
        Ty::Struct(s) => format!("S{s}_{}", m.structs[*s as usize].name),
        Ty::Array(e, n) => format!("array<{}, {n}>", wty(m, e)),
        Ty::Sampler(s) => s.wgsl().0,
    }
}

/// A short name for a type, used in helper function names.
pub fn mangle(t: &Ty) -> String {
    match t {
        Ty::Void => "v".into(),
        Ty::S(k) => ["b", "i", "u", "f"][*k as usize].into(),
        Ty::V(k, n) => format!("v{n}{}", ["b", "i", "u", "f"][*k as usize]),
        Ty::M(c, r) => format!("m{c}{r}"),
        Ty::Struct(s) => format!("s{s}"),
        Ty::Array(e, n) => format!("a{n}{}", mangle(e)),
        Ty::Sampler(_) => "t".into(),
    }
}

/// Array elements that are wrapped to reach a 16-byte stride.
pub fn wrapped(e: &Ty) -> bool {
    e.is_sv()
}

/// Size and alignment in the uniform buffer.
pub fn slayout(m: &Module, t: &Ty) -> (u32, u32) {
    match t {
        Ty::S(_) => (4, 4),
        Ty::V(_, 2) => (8, 8),
        Ty::V(_, 3) => (12, 16),
        Ty::V(..) => (16, 16),
        Ty::M(c, r) => {
            let cs = if *r == 2 { 8 } else { 16 };
            (*c as u32 * cs, cs)
        }
        Ty::Struct(s) => (struct_offsets(m, *s).1, 16),
        Ty::Array(e, n) => (stride(m, e).saturating_mul(*n), 16),
        _ => (0, 4),
    }
}

/// Byte distance between array elements.
pub fn stride(m: &Module, e: &Ty) -> u32 {
    if wrapped(e) {
        16
    } else {
        round_up(16, slayout(m, e).0)
    }
}

/// Column stride of a matrix.
pub fn col_stride(t: &Ty) -> u32 {
    match t {
        Ty::M(_, 2) => 8,
        Ty::M(..) => 16,
        _ => 16,
    }
}

/// Field offsets and the (16-byte rounded) size of a structure, as stored at its definition.
pub fn struct_offsets(m: &Module, s: u32) -> (&[u32], u32) {
    let sd = &m.structs[s as usize];
    (&sd.offs, sd.size)
}

/// Lay out a structure's fields (nested structures already have their layout stored).
pub fn compute_struct_layout(m: &Module, fields: &[(String, Ty)]) -> (Vec<u32>, u32) {
    let mut offs = Vec::new();
    let mut cur = 0u32;
    for (_, ft) in fields {
        let (size, align) = slayout(m, ft);
        let align = if matches!(ft, Ty::Struct(_) | Ty::Array(..)) { 16 } else { align };
        let off = round_up(align, cur);
        offs.push(off);
        cur = off.saturating_add(size);
    }
    (offs, round_up(16, cur.max(16)))
}

/// WGSL type of the stored form of a value.
pub fn stype(m: &Module, t: &Ty) -> String {
    match t {
        Ty::S(K::Bool) => "u32".into(),
        Ty::V(K::Bool, n) => format!("vec{n}<u32>"),
        Ty::Struct(s) => format!("US{s}"),
        Ty::Array(e, n) if wrapped(e) => format!("array<W_{}, {n}>", mangle(e)),
        Ty::Array(e, n) => format!("array<{}, {n}>", stype(m, e)),
        _ => wty(m, t),
    }
}

/// WGSL declarations of the stored forms needed by `t` (wrappers and structures), appended to
/// `out` once each.
pub fn storage_decls(m: &Module, t: &Ty, done: &mut Vec<String>, out: &mut String) {
    match t {
        Ty::Array(e, _) => {
            if wrapped(e) {
                let name = format!("W_{}", mangle(e));
                if !done.contains(&name) {
                    out.push_str(&format!("struct {name} {{ @size(16) v: {} }}\n", stype(m, e)));
                    done.push(name);
                }
            } else {
                storage_decls(m, e, done, out);
            }
        }
        Ty::Struct(s) => {
            let name = format!("US{s}");
            if done.contains(&name) {
                return;
            }
            let fields = &m.structs[*s as usize].fields;
            for (_, ft) in fields {
                storage_decls(m, ft, done, out);
            }
            let (offs, size) = struct_offsets(m, *s);
            let offs = offs.to_vec();
            out.push_str(&format!("struct {name} {{\n"));
            for (k, (fname, ft)) in fields.iter().enumerate() {
                let end = offs.get(k + 1).copied().unwrap_or(size);
                let align = if matches!(ft, Ty::Struct(_) | Ty::Array(..)) || k == 0 { "@align(16) " } else { "" };
                out.push_str(&format!("  {align}@size({}) m_{fname}: {},\n", end - offs[k], stype(m, ft)));
            }
            out.push_str("}\n");
            done.push(name);
        }
        _ => {}
    }
}

/// GL type enum of a basic type.
pub fn gl_type(t: &Ty) -> u32 {
    match t {
        Ty::S(K::Float) => 0x1406,
        Ty::S(K::Int) => 0x1404,
        Ty::S(K::Uint) => 0x1405,
        Ty::S(K::Bool) => 0x8B56,
        Ty::V(K::Float, n) => 0x8B50 + *n as u32 - 2,
        Ty::V(K::Int, n) => 0x8B53 + *n as u32 - 2,
        Ty::V(K::Bool, n) => 0x8B57 + *n as u32 - 2,
        Ty::V(K::Uint, n) => 0x8DC6 + *n as u32 - 2,
        Ty::M(c, _) => 0x8B5A + *c as u32 - 2,
        Ty::Sampler(s) => s.info().1,
        _ => 0,
    }
}
