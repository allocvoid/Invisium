//! ArrayBuffer, the element codec shared by typed arrays and DataView, and the embedding
//! helpers `array_buffer` / `buffer_bytes`. Buffers count toward the heap cap in full.

use super::arg;
use crate::ops::{to_int32, to_integer};
use crate::runtime::*;
use crate::value::*;

/// ToUint8Clamp: clamp to 0..=255, ties to even.
fn clamp_u8(n: f64) -> u8 {
    if n.is_nan() {
        return 0;
    }
    let c = n.clamp(0.0, 255.0);
    let f = c.floor();
    let up = c - f > 0.5 || (c - f == 0.5 && f % 2.0 == 1.0);
    (f + up as u8 as f64) as u8
}

/// Writes `n` as one element in little-endian order; returns the element size.
pub(crate) fn encode(kind: TypedKind, n: f64, out: &mut [u8; 8]) -> usize {
    match kind {
        TypedKind::Int8 | TypedKind::Uint8 => out[0] = to_int32(n) as u8,
        TypedKind::Uint8Clamped => out[0] = clamp_u8(n),
        TypedKind::Int16 | TypedKind::Uint16 => out[..2].copy_from_slice(&(to_int32(n) as u16).to_le_bytes()),
        TypedKind::Int32 | TypedKind::Uint32 => out[..4].copy_from_slice(&to_int32(n).to_le_bytes()),
        TypedKind::Float32 => out[..4].copy_from_slice(&(n as f32).to_le_bytes()),
        TypedKind::Float64 => out.copy_from_slice(&n.to_le_bytes()),
        // (BigInt elements are normally written as exact bits: see `typed_store`)
        TypedKind::BigInt64 | TypedKind::BigUint64 => out.copy_from_slice(&(n as i64).to_le_bytes()),
    }
    kind.size()
}

/// Reads one little-endian element (missing bytes read as zero).
pub(crate) fn decode(kind: TypedKind, bytes: &[u8]) -> f64 {
    let mut b = [0u8; 8];
    let n = bytes.len().min(8);
    b[..n].copy_from_slice(&bytes[..n]);
    match kind {
        TypedKind::Int8 => b[0] as i8 as f64,
        TypedKind::Uint8 | TypedKind::Uint8Clamped => b[0] as f64,
        TypedKind::Int16 => i16::from_le_bytes([b[0], b[1]]) as f64,
        TypedKind::Uint16 => u16::from_le_bytes([b[0], b[1]]) as f64,
        TypedKind::Int32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
        TypedKind::Uint32 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
        TypedKind::Float32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
        TypedKind::Float64 => f64::from_le_bytes(b),
        TypedKind::BigInt64 => i64::from_le_bytes(b) as f64,
        TypedKind::BigUint64 => u64::from_le_bytes(b) as f64,
    }
}

impl Runtime {
    /// Makes sure `n` more buffer bytes fit under the heap cap. A request the cap can never
    /// hold is a RangeError; one that does not fit right now (after a collection) is the
    /// usual out-of-memory abort.
    pub(crate) fn reserve_bytes(&mut self, n: usize) -> JsResult<()> {
        if n > self.limits.max_heap_bytes {
            return Err(self.range_error("Array buffer allocation failed"));
        }
        if self.total_heap().saturating_add(n) > self.limits.max_heap_bytes {
            self.collect_garbage();
        }
        self.check_alloc(n).map_err(|t| self.thrown_to_exc(t))
    }

    pub(crate) fn new_buffer(&mut self, bytes: Vec<u8>, proto: Option<ObjId>) -> ObjId {
        self.alloc(Object::new(proto, ObjKind::ArrayBuffer(bytes)))
    }

    pub(crate) fn buffer_ref(&self, o: ObjId) -> Option<&Vec<u8>> {
        match &self.obj(o).kind {
            ObjKind::ArrayBuffer(b) => Some(b),
            _ => None,
        }
    }

    /// The window of a typed array; its length reads 0 should it ever not fit its buffer.
    pub(crate) fn typed_view(&self, v: Value) -> Option<ViewData> {
        let Value::Object(o) = v else { return None };
        let ObjKind::TypedArray(mut vd) = self.obj(o).kind else { return None };
        let end = vd.offset.saturating_add(vd.length.saturating_mul(vd.kind.size()));
        if !self.buffer_ref(vd.buffer).is_some_and(|b| end <= b.len()) {
            vd.length = 0;
        }
        Some(vd)
    }

    /// The bytes a typed array or DataView covers.
    pub(crate) fn view_bytes(&self, vd: &ViewData, byte_len: usize) -> &[u8] {
        self.buffer_ref(vd.buffer).and_then(|b| b.get(vd.offset..vd.offset.saturating_add(byte_len))).unwrap_or(&[])
    }

    pub(crate) fn typed_get(&self, o: ObjId, i: usize) -> Option<f64> {
        let vd = self.typed_view(Value::Object(o))?;
        if i >= vd.length {
            return None;
        }
        let sz = vd.kind.size();
        let at = vd.offset + i * sz;
        Some(decode(vd.kind, self.buffer_ref(vd.buffer)?.get(at..at + sz)?))
    }

    /// The element as a value: a BigInt for the 64-bit integer kinds, else a Number.
    pub(crate) fn typed_value(&mut self, o: ObjId, i: usize) -> Option<Value> {
        let vd = self.typed_view(Value::Object(o))?;
        if !vd.kind.is_big() {
            return self.typed_get(o, i).map(Value::Number);
        }
        if i >= vd.length {
            return None;
        }
        let at = vd.offset + i * 8;
        let mut b = [0u8; 8];
        b.copy_from_slice(self.buffer_ref(vd.buffer)?.get(at..at + 8)?);
        Some(self.big_from_bits(u64::from_le_bytes(b), vd.kind == TypedKind::BigInt64))
    }

    /// A BigInt from 64 raw bits, read as signed or unsigned.
    pub(crate) fn big_from_bits(&mut self, bits: u64, signed: bool) -> Value {
        use crate::bigint::BigInt;
        self.alloc_big(if signed { BigInt::from_i64(bits as i64) } else { BigInt::from_u64(bits) })
    }

    /// Stores an element; false when `i` is outside the array (the write is dropped).
    pub(crate) fn typed_put(&mut self, o: ObjId, i: usize, n: f64) -> bool {
        let Some(vd) = self.typed_view(Value::Object(o)) else { return false };
        let mut tmp = [0u8; 8];
        let sz = encode(vd.kind, n, &mut tmp);
        self.typed_write(vd, i, &tmp[..sz])
    }

    /// Stores the low 64 bits of a BigInt element.
    pub(crate) fn typed_put_bits(&mut self, o: ObjId, i: usize, bits: u64) -> bool {
        let Some(vd) = self.typed_view(Value::Object(o)) else { return false };
        self.typed_write(vd, i, &bits.to_le_bytes())
    }

    fn typed_write(&mut self, vd: ViewData, i: usize, bytes: &[u8]) -> bool {
        if i >= vd.length {
            return false;
        }
        let (sz, tmp) = (bytes.len(), bytes);
        let at = vd.offset + i * sz;
        match &mut self.obj_mut(vd.buffer).kind {
            ObjKind::ArrayBuffer(b) => match b.get_mut(at..at + sz) {
                Some(dst) => {
                    dst.copy_from_slice(&tmp[..sz]);
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// `ta[i] = v`: ToNumber (or ToBigInt for the 64-bit integer kinds) first — it may run
    /// script — then the bounds-checked store.
    pub(crate) fn typed_store(&mut self, o: ObjId, i: usize, v: Value) -> JsResult<()> {
        let n = match v {
            Value::Number(n) if !matches!(self.obj(o).kind, ObjKind::TypedArray(vd) if vd.kind.is_big()) => n,
            other => {
                if matches!(self.obj(o).kind, ObjKind::TypedArray(vd) if vd.kind.is_big()) {
                    let bits = self.to_bigint(other)?.to_u64_wrapping();
                    self.typed_put_bits(o, i, bits);
                    return Ok(());
                }
                self.to_num(other)?
            }
        };
        self.typed_put(o, i, n);
        Ok(())
    }

    /// ToIndex: a non-negative integer, RangeError otherwise.
    pub(crate) fn to_index(&mut self, v: Value, what: &str) -> JsResult<usize> {
        if v == Value::Undefined {
            return Ok(0);
        }
        let n = to_integer(self.to_num(v)?);
        if !(0.0..=9007199254740991.0).contains(&n) {
            return Err(self.range_error(&format!("Invalid {}", what)));
        }
        Ok(n as usize)
    }

    /// A new ArrayBuffer holding a copy of `bytes` (embedding API).
    pub fn array_buffer(&mut self, bytes: &[u8]) -> Value {
        let p = self.intr.array_buffer_proto;
        Value::Object(self.new_buffer(bytes.to_vec(), p))
    }

    /// The bytes of an ArrayBuffer, or of the window a typed array / DataView covers; `None`
    /// for anything else (embedding API).
    pub fn buffer_bytes(&self, v: Value) -> Option<Vec<u8>> {
        let Value::Object(o) = v else { return None };
        match &self.obj(o).kind {
            ObjKind::ArrayBuffer(b) => Some(b.clone()),
            ObjKind::TypedArray(vd) => Some(self.view_bytes(vd, vd.length.saturating_mul(vd.kind.size())).to_vec()),
            ObjKind::DataView(vd) => Some(self.view_bytes(vd, vd.length).to_vec()),
            _ => None,
        }
    }
}

fn this_buffer(rt: &mut Runtime, this: Value) -> JsResult<ObjId> {
    match this {
        Value::Object(o) if rt.buffer_ref(o).is_some() => Ok(o),
        _ => Err(rt.type_error("this is not an ArrayBuffer")),
    }
}

fn buffer_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let nt = rt.native_new_target;
    if nt == Value::Undefined {
        return Err(rt.type_error("Constructor ArrayBuffer requires 'new'"));
    }
    let n = rt.to_index(arg(a, 0), "array buffer length")?;
    rt.reserve_bytes(n)?;
    let default = rt.intr.array_buffer_proto;
    let proto = rt.proto_from_ctor(nt, default)?;
    Ok(Value::Object(rt.new_buffer(vec![0; n], proto)))
}

fn byte_length(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = this_buffer(rt, this)?;
    Ok(Value::Number(rt.buffer_ref(o).map(|b| b.len()).unwrap_or(0) as f64))
}

fn slice(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let o = this_buffer(rt, this)?;
    let len = rt.buffer_ref(o).map(|b| b.len()).unwrap_or(0) as f64;
    let from = rt.rel_index(arg(a, 0), len, 0.0)? as usize;
    let to = rt.rel_index(arg(a, 1), len, len)? as usize;
    rt.reserve_bytes(to.saturating_sub(from))?;
    let part = rt.buffer_ref(o).and_then(|b| b.get(from..to.max(from))).unwrap_or(&[]).to_vec();
    let proto = rt.intr.array_buffer_proto;
    Ok(Value::Object(rt.new_buffer(part, proto)))
}

fn is_view(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(matches!(arg(a, 0), Value::Object(o) if matches!(rt.obj(o).kind, ObjKind::TypedArray(_) | ObjKind::DataView(_)))))
}

pub(super) fn setup(rt: &mut Runtime) {
    let bp = rt.new_object(rt.intr.object_proto);
    rt.intr.array_buffer_proto = Some(bp);
    let c = rt.constructor("ArrayBuffer", 1, buffer_ctor, bp);
    rt.method(c, "isView", 1, is_view);
    rt.getter(bp, "byteLength", byte_length);
    rt.method(bp, "slice", 2, slice);
    let tag = rt.new_str("ArrayBuffer");
    rt.define_value(bp, PropKey::Sym(SYM_TO_STRING_TAG), tag, CONFIGURABLE);
}
