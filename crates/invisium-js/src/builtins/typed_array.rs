//! Typed arrays: the nine element kinds over a shared %TypedArray%.prototype. Element access
//! itself is in `props.rs` / `interp.rs` (bounds-checked through `typed_get` / `typed_put`).

use super::arg;
use super::array::tick;
use crate::runtime::*;
use crate::value::*;

fn this_view(rt: &mut Runtime, this: Value) -> JsResult<(ObjId, ViewData)> {
    match (this, rt.typed_view(this)) {
        (Value::Object(o), Some(v)) => Ok((o, v)),
        _ => Err(rt.type_error("this is not a typed array")),
    }
}

impl Runtime {
    /// A typed array of `len` zeroed elements on a fresh buffer.
    pub(crate) fn typed_alloc(&mut self, kind: TypedKind, len: usize, proto: Option<ObjId>) -> JsResult<ObjId> {
        let Some(bytes) = len.checked_mul(kind.size()).filter(|b| *b <= self.limits.max_heap_bytes) else {
            return Err(self.range_error("Invalid typed array length"));
        };
        self.reserve_bytes(bytes)?;
        let bp = self.intr.array_buffer_proto;
        let buffer = self.new_buffer(vec![0; bytes], bp);
        Ok(self.alloc(Object::new(proto, ObjKind::TypedArray(ViewData { buffer, offset: 0, length: len, kind }))))
    }

    /// The elements of a typed array, iterable or array-like, rooted for the current native.
    fn list_from(&mut self, src: Value) -> JsResult<Vec<Value>> {
        let items: Vec<Value> = if let (Value::Object(o), Some(v)) = (src, self.typed_view(src)) {
            (0..v.length).map(|i| self.typed_value(o, i).unwrap_or(Value::Number(0.0))).collect()
        } else {
            let it = self.get_prop(src, PropKey::Sym(SYM_ITERATOR))?;
            if !it.is_nullish() {
                self.iterate_to_vec(src)?
            } else {
                let n = self.length_of(src)?;
                if n > (self.limits.max_heap_bytes / 16) as u64 {
                    return Err(self.range_error("Invalid typed array length"));
                }
                self.check_alloc(n as usize * 16).map_err(|t| self.thrown_to_exc(t))?;
                let mut v = Vec::with_capacity(n as usize);
                for i in 0..n {
                    tick(self, i)?;
                    v.push(self.get_idx(src, i)?);
                }
                v
            }
        };
        self.scratch.extend_from_slice(&items);
        Ok(items)
    }

    fn typed_from_list(&mut self, kind: TypedKind, items: &[Value], proto: Option<ObjId>) -> JsResult<Value> {
        let o = self.typed_alloc(kind, items.len(), proto)?;
        for (i, v) in items.iter().enumerate() {
            self.typed_store(o, i, *v)?;
        }
        Ok(Value::Object(o))
    }
}

fn typed_ctor(rt: &mut Runtime, a: &[Value], kind: TypedKind) -> JsResult<Value> {
    let nt = rt.native_new_target;
    if nt == Value::Undefined {
        return Err(rt.type_error(&format!("Constructor {} requires 'new'", kind.name())));
    }
    let default = rt.intr.typed_protos[kind as usize];
    let proto = rt.proto_from_ctor(nt, default)?;
    let first = arg(a, 0);
    let Value::Object(src) = first else {
        let len = rt.to_index(first, "typed array length")?;
        return Ok(Value::Object(rt.typed_alloc(kind, len, proto)?));
    };
    if rt.buffer_ref(src).is_none() {
        let items = rt.list_from(first)?;
        return rt.typed_from_list(kind, &items, proto);
    }
    // a view onto an existing buffer: (buffer, byteOffset, length)
    let sz = kind.size();
    let offset = rt.to_index(arg(a, 1), "offset")?;
    let given = match arg(a, 2) {
        Value::Undefined => None,
        v => Some(rt.to_index(v, "typed array length")?),
    };
    let buf_len = rt.buffer_ref(src).map(|b| b.len()).unwrap_or(0);
    if offset % sz != 0 {
        return Err(rt.range_error(&format!("start offset of {} should be a multiple of {}", kind.name(), sz)));
    }
    let length = match given {
        None if buf_len % sz != 0 => return Err(rt.range_error(&format!("byte length of {} should be a multiple of {}", kind.name(), sz))),
        None if offset > buf_len => return Err(rt.range_error("Start offset is outside the bounds of the buffer")),
        None => (buf_len - offset) / sz,
        Some(n) if n.checked_mul(sz).and_then(|b| b.checked_add(offset)).is_some_and(|end| end <= buf_len) => n,
        Some(_) => return Err(rt.range_error("Invalid typed array length")),
    };
    Ok(Value::Object(rt.alloc(Object::new(proto, ObjKind::TypedArray(ViewData { buffer: src, offset, length, kind })))))
}

fn get_buffer(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Object(this_view(rt, this)?.1.buffer))
}
fn get_byte_length(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let v = this_view(rt, this)?.1;
    Ok(Value::Number((v.length * v.kind.size()) as f64))
}
fn get_byte_offset(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(this_view(rt, this)?.1.offset as f64))
}
fn get_length(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(this_view(rt, this)?.1.length as f64))
}
fn get_tag(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(match rt.typed_view(this) {
        Some(v) => rt.new_str(v.kind.name()),
        None => Value::Undefined,
    })
}

fn set(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, view) = this_view(rt, this)?;
    let offset = crate::ops::to_integer(rt.to_num(arg(a, 1))?);
    if offset < 0.0 {
        return Err(rt.range_error("offset is out of bounds"));
    }
    let src = Value::Object(rt.to_object(arg(a, 0))?);
    let n = rt.length_of(src)?;
    if offset + n as f64 > view.length as f64 {
        return Err(rt.range_error("offset is out of bounds"));
    }
    // read everything first: source and target may share one buffer
    let mut items = Vec::with_capacity(n as usize);
    for i in 0..n {
        tick(rt, i)?;
        items.push(rt.get_idx(src, i)?);
    }
    rt.scratch.extend_from_slice(&items);
    for (i, v) in items.into_iter().enumerate() {
        rt.typed_store(o, offset as usize + i, v)?;
    }
    Ok(Value::Undefined)
}

/// Resolves (begin, end) arguments against the array's length.
fn range(rt: &mut Runtime, a: &[Value], len: usize) -> JsResult<(usize, usize)> {
    let from = rt.rel_index(arg(a, 0), len as f64, 0.0)? as usize;
    let to = rt.rel_index(arg(a, 1), len as f64, len as f64)? as usize;
    Ok((from.min(len), to.clamp(from.min(len), len)))
}

fn subarray(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, v) = this_view(rt, this)?;
    let (from, to) = range(rt, a, v.length)?;
    let begin = v.offset + from * v.kind.size();
    let args = [Value::Object(v.buffer), Value::Number(begin as f64), Value::Number((to - from) as f64)];
    if let Some(s) = rt.typed_species(this, v.kind, &args, 0)? {
        return Ok(Value::Object(s));
    }
    let proto = rt.obj(o).proto;
    let view = ViewData { buffer: v.buffer, offset: begin, length: to - from, kind: v.kind };
    Ok(Value::Object(rt.alloc(Object::new(proto, ObjKind::TypedArray(view)))))
}

fn slice(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, v) = this_view(rt, this)?;
    let (from, to) = range(rt, a, v.length)?;
    let sz = v.kind.size();
    if let Some(s) = rt.typed_species(this, v.kind, &[Value::Number((to - from) as f64)], to - from)? {
        // another constructor's array: copy element by element (the kinds may differ)
        for i in from..to {
            if let Some(x) = rt.typed_value(o, i) {
                rt.typed_store(s, i - from, x)?;
            }
        }
        return Ok(Value::Object(s));
    }
    let proto = rt.obj(o).proto;
    let out = rt.typed_alloc(v.kind, to - from, proto)?;
    let bytes = rt.view_bytes(&v, v.length * sz).get(from * sz..to * sz).unwrap_or(&[]).to_vec();
    if let Some(ov) = rt.typed_view(Value::Object(out)) {
        if let ObjKind::ArrayBuffer(b) = &mut rt.obj_mut(ov.buffer).kind {
            if let Some(dst) = b.get_mut(..bytes.len()) {
                dst.copy_from_slice(&bytes);
            }
        }
    }
    Ok(Value::Object(out))
}

fn copy_within(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (_, v) = this_view(rt, this)?;
    let len = v.length as f64;
    let target = rt.rel_index(arg(a, 0), len, 0.0)? as usize;
    let (from, to) = range(rt, &a[a.len().min(1)..], v.length)?;
    let count = (to - from).min(v.length.saturating_sub(target));
    let sz = v.kind.size();
    if let ObjKind::ArrayBuffer(b) = &mut rt.obj_mut(v.buffer).kind {
        let (src, dst) = (v.offset + from * sz, v.offset + target * sz);
        if src + count * sz <= b.len() && dst + count * sz <= b.len() {
            b.copy_within(src..src + count * sz, dst);
        }
    }
    Ok(this)
}

fn map(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, v) = this_view(rt, this)?;
    let cb = rt.callable_arg(arg(a, 0), "callback")?;
    let out = match rt.typed_species(this, v.kind, &[Value::Number(v.length as f64)], v.length)? {
        Some(s) => s,
        None => {
            let proto = rt.obj(o).proto;
            rt.typed_alloc(v.kind, v.length, proto)?
        }
    };
    for i in 0..v.length {
        let x = rt.typed_value(o, i).unwrap_or(Value::Number(0.0));
        let r = rt.call_value(cb, arg(a, 1), &[x, Value::Number(i as f64), this])?;
        rt.typed_store(out, i, r)?;
    }
    Ok(Value::Object(out))
}

fn filter(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, v) = this_view(rt, this)?;
    let cb = rt.callable_arg(arg(a, 0), "callback")?;
    let mut kept = Vec::new();
    for i in 0..v.length {
        let x = rt.typed_value(o, i).unwrap_or(Value::Number(0.0));
        let r = rt.call_value(cb, arg(a, 1), &[x, Value::Number(i as f64), this])?;
        if rt.truthy(r) {
            kept.push(x);
        }
    }
    if let Some(s) = rt.typed_species(this, v.kind, &[Value::Number(kept.len() as f64)], kept.len())? {
        for (i, x) in kept.iter().enumerate() {
            rt.typed_store(s, i, *x)?;
        }
        return Ok(Value::Object(s));
    }
    let proto = rt.obj(o).proto;
    rt.typed_from_list(v.kind, &kept, proto)
}

fn sort(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, v) = this_view(rt, this)?;
    if arg(a, 0) != Value::Undefined {
        return super::array_fn::sort(rt, this, a);
    }
    if v.kind.is_big() {
        // exact 64-bit integers: sort the raw values, not their Number approximations
        let signed = v.kind == TypedKind::BigInt64;
        rt.steps = rt.steps.saturating_add(v.length as u64 / 4);
        rt.safepoint_counted()?;
        let bytes = rt.view_bytes(&v, v.length * 8).to_vec();
        let mut vals: Vec<i128> = bytes
            .chunks_exact(8)
            .map(|c| {
                let mut b = [0u8; 8];
                b.copy_from_slice(c);
                if signed { i64::from_le_bytes(b) as i128 } else { u64::from_le_bytes(b) as i128 }
            })
            .collect();
        vals.sort_unstable();
        for (i, x) in vals.into_iter().enumerate() {
            rt.typed_put_bits(o, i, x as u64);
        }
        return Ok(this);
    }
    // default order is numeric: -0 before +0, NaN last
    rt.steps = rt.steps.saturating_add(v.length as u64 / 4);
    rt.safepoint_counted()?;
    let mut nums: Vec<f64> = (0..v.length).map(|i| rt.typed_get(o, i).map(|x| if x.is_nan() { f64::NAN } else { x }).unwrap_or(0.0)).collect();
    nums.sort_by(|x, y| x.total_cmp(y));
    for (i, n) in nums.into_iter().enumerate() {
        rt.typed_put(o, i, n);
    }
    Ok(this)
}

fn from(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let mapf = arg(a, 1);
    if mapf != Value::Undefined {
        rt.callable_arg(mapf, "TypedArray.from")?;
    }
    let mut items = rt.list_from(arg(a, 0))?;
    if mapf != Value::Undefined {
        for (i, v) in items.iter_mut().enumerate() {
            *v = rt.call_value(mapf, arg(a, 2), &[*v, Value::Number(i as f64)])?;
            rt.scratch.push(*v);
        }
    }
    let list = rt.new_array_values(&items);
    rt.construct_value(this, &[list], this)
}

fn of(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let list = rt.new_array_values(a);
    rt.construct_value(this, &[list], this)
}

macro_rules! ctor {
    ($k:ident) => {
        |rt, _, a| typed_ctor(rt, a, TypedKind::$k)
    };
}

pub(super) fn setup(rt: &mut Runtime) {
    let tap = rt.new_object(rt.intr.object_proto);
    rt.getter(tap, "buffer", get_buffer);
    rt.getter(tap, "byteLength", get_byte_length);
    rt.getter(tap, "byteOffset", get_byte_offset);
    rt.getter(tap, "length", get_length);
    let tag = rt.native_fn("get [Symbol.toStringTag]", 0, get_tag);
    rt.define_own(tap, PropKey::Sym(SYM_TO_STRING_TAG), Prop { value: Value::Object(tag), setter: Value::Undefined, flags: ACCESSOR | CONFIGURABLE });
    let own: [(&str, u32, super::NFn); 7] =
        [("set", 1, set), ("subarray", 2, subarray), ("slice", 2, slice), ("copyWithin", 2, copy_within), ("map", 1, map), ("filter", 1, filter), ("sort", 1, sort)];
    for (n, ar, f) in own {
        rt.method(tap, n, ar, f);
    }
    // the remaining methods are Array.prototype's generic ones: they only need `length` and
    // indexed access, both of which take the typed fast paths
    if let Some(ap) = rt.intr.array_proto {
        for name in [
            "forEach", "some", "every", "find", "findIndex", "findLast", "findLastIndex", "indexOf", "lastIndexOf", "includes", "join",
            "reduce", "reduceRight", "reverse", "fill", "keys", "values", "entries", "at", "toString",
        ] {
            let k = rt.key_str(name);
            if let Some(p) = rt.get_own_prop(ap, k) {
                rt.define_value(tap, k, p.value, HIDDEN);
                if name == "values" {
                    rt.define_value(tap, PropKey::Sym(SYM_ITERATOR), p.value, HIDDEN);
                }
            }
        }
    }
    let ctors: [super::NFn; 11] = [
        ctor!(Int8), ctor!(Uint8), ctor!(Uint8Clamped), ctor!(Int16), ctor!(Uint16), ctor!(Int32), ctor!(Uint32), ctor!(Float32), ctor!(Float64),
        ctor!(BigInt64), ctor!(BigUint64),
    ];
    let bpe = rt.key_str("BYTES_PER_ELEMENT");
    for (kind, f) in TypedKind::ALL.into_iter().zip(ctors) {
        let proto = rt.new_object(Some(tap));
        rt.intr.typed_protos[kind as usize] = Some(proto);
        let c = rt.constructor(kind.name(), 3, f, proto);
        for o in [c, proto] {
            rt.define_value(o, bpe, Value::Number(kind.size() as f64), 0);
        }
        rt.method(c, "from", 1, from);
        rt.method(c, "of", 0, of);
    }
}
