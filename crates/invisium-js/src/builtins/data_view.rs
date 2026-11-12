//! DataView: explicit-endianness reads and writes at byte offsets of an ArrayBuffer.

use super::arg;
use super::buffer::{decode, encode};
use crate::runtime::*;
use crate::value::*;

fn this_view(rt: &mut Runtime, this: Value) -> JsResult<ViewData> {
    if let Value::Object(o) = this {
        if let ObjKind::DataView(v) = rt.obj(o).kind {
            return Ok(v);
        }
    }
    Err(rt.type_error("this is not a DataView"))
}

fn view_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let nt = rt.native_new_target;
    if nt == Value::Undefined {
        return Err(rt.type_error("Constructor DataView requires 'new'"));
    }
    let buffer = match arg(a, 0) {
        Value::Object(o) if rt.buffer_ref(o).is_some() => o,
        _ => return Err(rt.type_error("First argument to DataView constructor must be an ArrayBuffer")),
    };
    let offset = rt.to_index(arg(a, 1), "offset")?;
    let given = match arg(a, 2) {
        Value::Undefined => None,
        v => Some(rt.to_index(v, "DataView length")?),
    };
    let buf_len = rt.buffer_ref(buffer).map(|b| b.len()).unwrap_or(0);
    if offset > buf_len {
        return Err(rt.range_error("Start offset is outside the bounds of the buffer"));
    }
    let length = match given {
        None => buf_len - offset,
        Some(n) if n <= buf_len - offset => n,
        Some(_) => return Err(rt.range_error("Invalid DataView length")),
    };
    let default = rt.intr.data_view_proto;
    let proto = rt.proto_from_ctor(nt, default)?;
    Ok(Value::Object(rt.alloc(Object::new(proto, ObjKind::DataView(ViewData { buffer, offset, length, kind: TypedKind::Uint8 })))))
}

fn get_buffer(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Object(this_view(rt, this)?.buffer))
}
fn get_byte_length(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(this_view(rt, this)?.length as f64))
}
fn get_byte_offset(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(this_view(rt, this)?.offset as f64))
}

/// The absolute byte position of an element access, or a RangeError when it leaves the view.
fn locate(rt: &mut Runtime, view: &ViewData, index: usize, size: usize) -> JsResult<usize> {
    let inside = index.checked_add(size).is_some_and(|end| end <= view.length);
    let fits = rt.buffer_ref(view.buffer).is_some_and(|b| view.offset.saturating_add(view.length) <= b.len());
    if !(inside && fits) {
        return Err(rt.range_error("Offset is outside the bounds of the DataView"));
    }
    Ok(view.offset + index)
}

fn read(rt: &mut Runtime, this: Value, a: &[Value], kind: TypedKind) -> JsResult<Value> {
    let view = this_view(rt, this)?;
    let index = rt.to_index(arg(a, 0), "offset")?;
    let little = rt.truthy(arg(a, 1));
    let size = kind.size();
    let at = locate(rt, &view, index, size)?;
    let mut tmp = [0u8; 8];
    if let Some(src) = rt.buffer_ref(view.buffer).and_then(|b| b.get(at..at + size)) {
        tmp[..size].copy_from_slice(src);
    }
    if !little {
        tmp[..size].reverse();
    }
    if kind.is_big() {
        return Ok(rt.big_from_bits(u64::from_le_bytes(tmp), kind == TypedKind::BigInt64));
    }
    Ok(Value::Number(decode(kind, &tmp[..size])))
}

fn write(rt: &mut Runtime, this: Value, a: &[Value], kind: TypedKind) -> JsResult<Value> {
    let view = this_view(rt, this)?;
    let index = rt.to_index(arg(a, 0), "offset")?;
    let mut tmp = [0u8; 8];
    let size = if kind.is_big() {
        tmp = rt.to_bigint(arg(a, 1))?.to_u64_wrapping().to_le_bytes();
        8
    } else {
        let n = rt.to_num(arg(a, 1))?;
        encode(kind, n, &mut tmp)
    };
    let little = rt.truthy(arg(a, 2));
    let at = locate(rt, &view, index, size)?;
    if !little {
        tmp[..size].reverse();
    }
    if let ObjKind::ArrayBuffer(b) = &mut rt.obj_mut(view.buffer).kind {
        if let Some(dst) = b.get_mut(at..at + size) {
            dst.copy_from_slice(&tmp[..size]);
        }
    }
    Ok(Value::Undefined)
}

macro_rules! accessors {
    ($($name:literal => $k:ident),*) => {
        [$((concat!("get", $name), 1, (|rt, t, a| read(rt, t, a, TypedKind::$k)) as super::NFn),
           (concat!("set", $name), 2, (|rt, t, a| write(rt, t, a, TypedKind::$k)) as super::NFn)),*]
    };
}

pub(super) fn setup(rt: &mut Runtime) {
    let dp = rt.new_object(rt.intr.object_proto);
    rt.intr.data_view_proto = Some(dp);
    rt.constructor("DataView", 1, view_ctor, dp);
    rt.getter(dp, "buffer", get_buffer);
    rt.getter(dp, "byteLength", get_byte_length);
    rt.getter(dp, "byteOffset", get_byte_offset);
    let methods: [(&str, u32, super::NFn); 20] = accessors!(
        "Int8" => Int8, "Uint8" => Uint8, "Int16" => Int16, "Uint16" => Uint16,
        "Int32" => Int32, "Uint32" => Uint32, "Float32" => Float32, "Float64" => Float64,
        "BigInt64" => BigInt64, "BigUint64" => BigUint64
    );
    for (name, arity, f) in methods {
        rt.method(dp, name, arity, f);
    }
    let tag = rt.new_str("DataView");
    rt.define_value(dp, PropKey::Sym(SYM_TO_STRING_TAG), tag, CONFIGURABLE);
}
