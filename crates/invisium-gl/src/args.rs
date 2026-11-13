//! Argument conversion with WebIDL semantics. A value of the wrong script type is
//! `INVALID_OPERATION` (never a panic).

use crate::consts1::INVALID_OPERATION;
use crate::{Arg, Elem, Kind};

/// `Err(code)` records a GL error and makes the call do nothing.
pub type R<T> = Result<T, u32>;

pub fn arg<'a>(a: &[Arg<'a>], i: usize) -> Arg<'a> {
    a.get(i).copied().unwrap_or(Arg::Undefined)
}

pub fn num(a: &[Arg], i: usize) -> R<f64> {
    match arg(a, i) {
        Arg::Num(n) => Ok(n),
        Arg::Bool(b) => Ok(b as u8 as f64),
        Arg::Null => Ok(0.0),
        Arg::Undefined => Ok(f64::NAN),
        _ => Err(INVALID_OPERATION),
    }
}

fn modulo32(n: f64) -> u32 {
    if !n.is_finite() {
        return 0;
    }
    n.trunc().rem_euclid(4294967296.0) as u32
}

/// `long` / `GLint` / `GLsizei` (ToInt32).
pub fn int(a: &[Arg], i: usize) -> R<i32> {
    Ok(modulo32(num(a, i)?) as i32)
}

/// `unsigned long` / `GLenum` / `GLuint` / `GLbitfield` (ToUint32).
pub fn uint(a: &[Arg], i: usize) -> R<u32> {
    Ok(modulo32(num(a, i)?))
}

/// `long long` (`GLintptr`, `GLsizeiptr`, `GLint64`).
pub fn long(a: &[Arg], i: usize) -> R<i64> {
    let n = num(a, i)?;
    Ok(if n.is_finite() { n.trunc() as i64 } else { 0 })
}

/// `unrestricted float` / `GLfloat` / `GLclampf`.
pub fn float(a: &[Arg], i: usize) -> R<f32> {
    Ok(num(a, i)? as f32)
}

/// `boolean` (ToBoolean).
pub fn boolean(a: &[Arg], i: usize) -> R<bool> {
    Ok(match arg(a, i) {
        Arg::Undefined | Arg::Null => false,
        Arg::Bool(b) => b,
        Arg::Num(n) => n != 0.0 && !n.is_nan(),
        Arg::Str(s) => !s.is_empty(),
        _ => true,
    })
}

/// A nullable WebGL object of `kind`.
pub fn obj(a: &[Arg], i: usize, kind: Kind) -> R<Option<u32>> {
    match arg(a, i) {
        Arg::Undefined | Arg::Null => Ok(None),
        Arg::Object(k, h) if k == kind => Ok(Some(h)),
        _ => Err(INVALID_OPERATION),
    }
}

/// A sequence of numbers: a typed array of element `want` (or any element type when `want`
/// is `None`) or a plain array.
pub fn nums(a: &[Arg], i: usize, want: Elem) -> R<Vec<f64>> {
    match arg(a, i) {
        Arg::Nums(v) => Ok(v.to_vec()),
        Arg::Bytes(e, b) if e == want => Ok(elems(e, b)),
        _ => Err(INVALID_OPERATION),
    }
}

/// Bytes per element of a typed array.
pub fn elem_size(e: Elem) -> usize {
    match e {
        Elem::I8 | Elem::U8 | Elem::U8Clamped | Elem::Raw => 1,
        Elem::I16 | Elem::U16 => 2,
        Elem::I32 | Elem::U32 | Elem::F32 => 4,
        Elem::F64 => 8,
    }
}

pub fn elems(e: Elem, b: &[u8]) -> Vec<f64> {
    let n = elem_size(e);
    b.chunks_exact(n)
        .map(|c| match e {
            Elem::I8 => c[0] as i8 as f64,
            Elem::U8 | Elem::U8Clamped | Elem::Raw => c[0] as f64,
            Elem::I16 => i16::from_le_bytes([c[0], c[1]]) as f64,
            Elem::U16 => u16::from_le_bytes([c[0], c[1]]) as f64,
            Elem::I32 => i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64,
            Elem::U32 => u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64,
            Elem::F32 => f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64,
            Elem::F64 => f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]),
        })
        .collect()
}
