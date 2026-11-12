//! Array constructor and the mutating / structural Array.prototype methods.
//! Callback-driven methods, sort, join and flat live in `array_fn.rs`.

use super::arg;
use crate::runtime::*;
use crate::value::*;

pub(crate) fn tick(rt: &mut Runtime, i: u64) -> JsResult<()> {
    if i & 1023 == 1023 {
        rt.steps += 1024;
        if rt.steps > rt.limits.step_budget {
            return Err(Exc::Abort(ErrorKind::StepBudget));
        }
        rt.drain_pending_steps()?;
    }
    Ok(())
}

pub(crate) fn this_len(rt: &mut Runtime, this: Value) -> JsResult<(Value, u64)> {
    let o = rt.to_object(this)?;
    let v = Value::Object(o);
    let n = rt.length_of(v)?;
    Ok((v, n))
}

pub(crate) fn set_index(rt: &mut Runtime, o: Value, i: u64, v: Value) -> JsResult<()> {
    if let Value::Object(oid) = o {
        if i < u32::MAX as u64 && matches!(rt.obj(oid).kind, ObjKind::Array(_)) {
            if rt.array_set_index(oid, i as u32, v)? {
                return Ok(());
            }
            return Err(rt.type_error("Cannot assign to read only array element"));
        }
        if matches!(rt.obj(oid).kind, ObjKind::TypedArray(_)) {
            return rt.typed_store(oid, i as usize, v);
        }
    }
    let k = rt.index_key(i);
    if !rt.set_prop(o, k, v, o)? {
        return Err(rt.type_error("Cannot assign to read only property"));
    }
    Ok(())
}

pub(crate) fn has_index(rt: &mut Runtime, o: Value, i: u64) -> JsResult<bool> {
    let Value::Object(oid) = o else { return Ok(false) };
    if let ObjKind::Array(items) = &rt.obj(oid).kind {
        if let Some(Some(_)) = items.get(i as usize) {
            return Ok(true);
        }
    }
    if let Some(view) = rt.typed_view(o) {
        return Ok((i as usize) < view.length);
    }
    let k = rt.index_key(i);
    rt.has_property(oid, k)
}

pub(crate) fn delete_index(rt: &mut Runtime, o: Value, i: u64) -> JsResult<()> {
    if let Value::Object(oid) = o {
        let k = rt.index_key(i);
        if !rt.delete_prop(oid, k) {
            return Err(rt.type_error("Cannot delete property"));
        }
    }
    Ok(())
}

pub(crate) fn set_len(rt: &mut Runtime, o: Value, n: u64) -> JsResult<()> {
    if !rt.set_prop(o, PropKey::Str(A_LENGTH), Value::Number(n as f64), o)? {
        return Err(rt.type_error("Cannot assign to read only property 'length'"));
    }
    Ok(())
}

fn array_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let nt = rt.native_new_target;
    let proto = if nt == Value::Undefined { rt.intr.array_proto } else { let p = rt.intr.array_proto; rt.proto_from_ctor(nt, p)? };
    let items: Vec<Option<Value>> = if a.len() == 1 {
        match a[0] {
            Value::Number(n) => {
                if n < 0.0 || n.fract() != 0.0 || n > 4294967295.0 {
                    return Err(rt.range_error("Invalid array length"));
                }
                rt.check_alloc(n as usize * 16).map_err(|t| rt.thrown_to_exc(t))?;
                vec![None; n as usize]
            }
            v => vec![Some(v)],
        }
    } else {
        a.iter().map(|v| Some(*v)).collect()
    };
    Ok(Value::Object(rt.alloc(Object::new(proto, ObjKind::Array(items)))))
}

fn is_array(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(matches!(arg(a, 0), Value::Object(o) if rt.is_array_obj(o))))
}

/// The object `Array.from` / `Array.of` fill: `new this()` for a subclass, else a plain array.
fn new_for_ctor(rt: &mut Runtime, this: Value, len: Option<usize>) -> JsResult<Value> {
    if this.as_obj() != rt.intr.array_ctor && rt.is_constructor(this) {
        let args: Vec<Value> = len.map(|n| Value::Number(n as f64)).into_iter().collect();
        return rt.construct_value(this, &args, this);
    }
    Ok(Value::Object(rt.new_array(Vec::new())))
}

fn array_of(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    if this.as_obj() == rt.intr.array_ctor || !rt.is_constructor(this) {
        return Ok(rt.new_array_values(a));
    }
    let out = new_for_ctor(rt, this, Some(a.len()))?;
    for (i, v) in a.iter().enumerate() {
        set_index(rt, out, i as u64, *v)?;
    }
    set_len(rt, out, a.len() as u64)?;
    Ok(out)
}

fn array_from(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let src = arg(a, 0);
    let mapf = arg(a, 1);
    if mapf != Value::Undefined {
        rt.callable_arg(mapf, "Array.from")?;
    }
    if src.is_nullish() {
        return Err(rt.type_error("Array.from requires an array-like or iterable object"));
    }
    let out_v = new_for_ctor(rt, this, None)?;
    let iter_m = rt.get_prop(src, PropKey::Sym(SYM_ITERATOR))?;
    let items: Vec<Value> = if !iter_m.is_nullish() {
        rt.iterate_to_vec(src)?
    } else {
        let n = rt.length_of(src)?;
        let mut v = Vec::new();
        for i in 0..n {
            tick(rt, i)?;
            v.push(rt.get_idx(src, i)?);
        }
        v
    };
    for (i, item) in items.into_iter().enumerate() {
        let v = if mapf != Value::Undefined { rt.call_value(mapf, arg(a, 2), &[item, Value::Number(i as f64)])? } else { item };
        set_index(rt, out_v, i as u64, v)?;
    }
    Ok(out_v)
}

fn push(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    if let Value::Object(o) = this {
        let ob = rt.obj_mut(o);
        if ob.integrity == 0 && ob.extensible {
            if let ObjKind::Array(items) = &mut ob.kind {
                items.extend(a.iter().map(|v| Some(*v)));
                let n = items.len();
                rt.grow(a.len() * 16);
                return Ok(Value::Number(n as f64));
            }
        }
    }
    let (o, mut n) = this_len(rt, this)?;
    for v in a {
        set_index(rt, o, n, *v)?;
        n += 1;
    }
    set_len(rt, o, n)?;
    Ok(Value::Number(n as f64))
}

fn pop(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    if let Value::Object(o) = this {
        let ob = rt.obj_mut(o);
        if ob.integrity == 0 {
            if let ObjKind::Array(items) = &mut ob.kind {
                return Ok(items.pop().flatten().unwrap_or(Value::Undefined));
            }
        }
    }
    let (o, n) = this_len(rt, this)?;
    if n == 0 {
        set_len(rt, o, 0)?;
        return Ok(Value::Undefined);
    }
    let v = rt.get_idx(o, n - 1)?;
    delete_index(rt, o, n - 1)?;
    set_len(rt, o, n - 1)?;
    Ok(v)
}

fn shift(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    if let Value::Object(o) = this {
        let ob = rt.obj_mut(o);
        if ob.integrity == 0 {
            if let ObjKind::Array(items) = &mut ob.kind {
                if items.is_empty() {
                    return Ok(Value::Undefined);
                }
                return Ok(items.remove(0).unwrap_or(Value::Undefined));
            }
        }
    }
    let (o, n) = this_len(rt, this)?;
    if n == 0 {
        set_len(rt, o, 0)?;
        return Ok(Value::Undefined);
    }
    let first = rt.get_idx(o, 0)?;
    for k in 1..n {
        tick(rt, k)?;
        if has_index(rt, o, k)? {
            let v = rt.get_idx(o, k)?;
            set_index(rt, o, k - 1, v)?;
        } else {
            delete_index(rt, o, k - 1)?;
        }
    }
    delete_index(rt, o, n - 1)?;
    set_len(rt, o, n - 1)?;
    Ok(first)
}

fn unshift(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    if let Value::Object(o) = this {
        let ob = rt.obj_mut(o);
        if ob.integrity == 0 && ob.extensible {
            if let ObjKind::Array(items) = &mut ob.kind {
                let tail = std::mem::take(items);
                items.extend(a.iter().map(|v| Some(*v)));
                items.extend(tail);
                let n = items.len();
                rt.grow(a.len() * 16);
                return Ok(Value::Number(n as f64));
            }
        }
    }
    let (o, n) = this_len(rt, this)?;
    let c = a.len() as u64;
    let mut k = n;
    while k > 0 {
        tick(rt, k)?;
        if has_index(rt, o, k - 1)? {
            let v = rt.get_idx(o, k - 1)?;
            set_index(rt, o, k - 1 + c, v)?;
        } else {
            delete_index(rt, o, k - 1 + c)?;
        }
        k -= 1;
    }
    for (i, v) in a.iter().enumerate() {
        set_index(rt, o, i as u64, *v)?;
    }
    set_len(rt, o, n + c)?;
    Ok(Value::Number((n + c) as f64))
}

fn slice(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, n) = this_len(rt, this)?;
    let start = rt.rel_index(arg(a, 0), n as f64, 0.0)? as u64;
    let end = rt.rel_index(arg(a, 1), n as f64, n as f64)? as u64;
    let out = match rt.array_species(o, end.saturating_sub(start))? {
        Some(v) => v,
        None => Value::Object(rt.new_array(Vec::new())),
    };
    let mut j = 0;
    let mut k = start;
    while k < end {
        tick(rt, k)?;
        if has_index(rt, o, k)? {
            let v = rt.get_idx(o, k)?;
            set_index(rt, out, j, v)?;
        }
        k += 1;
        j += 1;
    }
    set_len(rt, out, j)?;
    Ok(out)
}

fn splice(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, n) = this_len(rt, this)?;
    let start = rt.rel_index(arg(a, 0), n as f64, 0.0)? as u64;
    let del = if a.is_empty() {
        0
    } else if a.len() == 1 {
        n - start
    } else {
        let d = crate::ops::to_integer(rt.to_num(a[1])?);
        d.max(0.0).min((n - start) as f64) as u64
    };
    let items: Vec<Value> = a.iter().skip(2).copied().collect();
    let removed = match rt.array_species(o, del)? {
        Some(v) => v,
        None => Value::Object(rt.new_array(Vec::new())),
    };
    for i in 0..del {
        if has_index(rt, o, start + i)? {
            let v = rt.get_idx(o, start + i)?;
            set_index(rt, removed, i, v)?;
        }
    }
    set_len(rt, removed, del)?;
    let ic = items.len() as u64;
    if ic < del {
        for k in start..n - del {
            tick(rt, k)?;
            let (from, to) = (k + del, k + ic);
            if has_index(rt, o, from)? {
                let v = rt.get_idx(o, from)?;
                set_index(rt, o, to, v)?;
            } else {
                delete_index(rt, o, to)?;
            }
        }
        let mut k = n;
        while k > n - del + ic {
            delete_index(rt, o, k - 1)?;
            k -= 1;
        }
    } else if ic > del {
        let mut k = n - del;
        while k > start {
            tick(rt, k)?;
            let (from, to) = (k + del - 1, k + ic - 1);
            if has_index(rt, o, from)? {
                let v = rt.get_idx(o, from)?;
                set_index(rt, o, to, v)?;
            } else {
                delete_index(rt, o, to)?;
            }
            k -= 1;
        }
    }
    for (i, v) in items.into_iter().enumerate() {
        set_index(rt, o, start + i as u64, v)?;
    }
    set_len(rt, o, n - del + ic)?;
    Ok(removed)
}

fn concat(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let o = Value::Object(rt.to_object(this)?);
    let out = match rt.array_species(o, 0)? {
        Some(v) => v,
        None => Value::Object(rt.new_array(Vec::new())),
    };
    let mut n = 0u64;
    let mut all = vec![o];
    all.extend_from_slice(a);
    for item in all {
        let spread = matches!(item, Value::Object(io) if rt.is_array_obj(io));
        if spread {
            let len = rt.length_of(item)?;
            rt.check_alloc(len as usize * 16).map_err(|t| rt.thrown_to_exc(t))?;
            for i in 0..len {
                tick(rt, i)?;
                if has_index(rt, item, i)? {
                    let v = rt.get_idx(item, i)?;
                    set_index(rt, out, n, v)?;
                }
                n += 1;
            }
        } else {
            set_index(rt, out, n, item)?;
            n += 1;
        }
    }
    set_len(rt, out, n)?;
    Ok(out)
}

fn reverse(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let (o, n) = this_len(rt, this)?;
    let mut lo = 0u64;
    while n > 0 && lo < n / 2 {
        tick(rt, lo)?;
        let hi = n - 1 - lo;
        let (le, he) = (has_index(rt, o, lo)?, has_index(rt, o, hi)?);
        let lv = rt.get_idx(o, lo)?;
        let hv = rt.get_idx(o, hi)?;
        if he { set_index(rt, o, lo, hv)?; } else { delete_index(rt, o, lo)?; }
        if le { set_index(rt, o, hi, lv)?; } else { delete_index(rt, o, hi)?; }
        lo += 1;
    }
    Ok(o)
}

fn fill(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, n) = this_len(rt, this)?;
    let start = rt.rel_index(arg(a, 1), n as f64, 0.0)? as u64;
    let end = rt.rel_index(arg(a, 2), n as f64, n as f64)? as u64;
    for k in start..end {
        tick(rt, k)?;
        set_index(rt, o, k, arg(a, 0))?;
    }
    Ok(o)
}

fn iter_of(rt: &mut Runtime, this: Value, kind: IterKind) -> JsResult<Value> {
    let o = rt.to_object(this)?;
    Ok(rt.new_array_iter(Value::Object(o), kind))
}
fn keys(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    iter_of(rt, this, IterKind::Keys)
}
fn values(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    iter_of(rt, this, IterKind::Values)
}
fn entries(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    iter_of(rt, this, IterKind::Entries)
}

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto;
    let ap = rt.alloc(Object::new(op, ObjKind::Array(Vec::new())));
    rt.intr.array_proto = Some(ap);
    let c = rt.constructor("Array", 1, array_ctor, ap);
    rt.method(c, "isArray", 1, is_array);
    rt.method(c, "of", 0, array_of);
    rt.method(c, "from", 1, array_from);
    let methods: [(&str, u32, super::NFn); 12] = [
        ("push", 1, push),
        ("pop", 0, pop),
        ("shift", 0, shift),
        ("unshift", 1, unshift),
        ("slice", 2, slice),
        ("splice", 2, splice),
        ("concat", 1, concat),
        ("reverse", 0, reverse),
        ("fill", 1, fill),
        ("keys", 0, keys),
        ("entries", 0, entries),
        ("values", 0, values),
    ];
    for (n, ar, f) in methods {
        rt.method(ap, n, ar, f);
    }
    let vk = rt.key_str("values");
    if let Some(p) = rt.get_own_prop(ap, vk) {
        rt.define_value(ap, PropKey::Sym(SYM_ITERATOR), p.value, HIDDEN);
        rt.intr.array_values = p.value.as_obj();
    }
    super::array_fn::setup(rt, ap);
}
