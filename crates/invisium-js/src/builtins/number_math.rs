//! Number, Boolean, Math, parseInt / parseFloat.

use super::arg;
use crate::numfmt;
use crate::runtime::*;
use crate::value::*;

fn number_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    // Number(value) is ToNumeric: a BigInt converts explicitly here (and only here)
    let n = match a.first() {
        None => 0.0,
        Some(v) => match rt.to_numeric(*v)? {
            Value::BigInt(b) => rt.big(b).to_f64(),
            other => rt.to_num(other)?,
        },
    };
    let nt = rt.native_new_target;
    if nt == Value::Undefined {
        return Ok(Value::Number(n));
    }
    let p = rt.intr.number_proto;
    let proto = rt.proto_from_ctor(nt, p)?;
    Ok(Value::Object(rt.alloc(Object::new(proto, ObjKind::Number(n)))))
}

fn this_num(rt: &mut Runtime, this: Value) -> JsResult<f64> {
    match this {
        Value::Number(n) => Ok(n),
        Value::Object(o) => match rt.obj(o).kind {
            ObjKind::Number(n) => Ok(n),
            _ => Err(rt.type_error("Number.prototype method called on incompatible receiver")),
        },
        _ => Err(rt.type_error("Number.prototype method called on incompatible receiver")),
    }
}

fn num_to_string(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let n = this_num(rt, this)?;
    let radix = match arg(a, 0) {
        Value::Undefined => 10.0,
        v => crate::ops::to_integer(rt.to_num(v)?),
    };
    if !(2.0..=36.0).contains(&radix) {
        return Err(rt.range_error("toString() radix must be between 2 and 36"));
    }
    let s = numfmt::to_radix(n, radix as u32);
    Ok(rt.new_str(&s))
}

fn digits_arg(rt: &mut Runtime, v: Value, lo: f64, hi: f64) -> JsResult<Option<usize>> {
    if v == Value::Undefined {
        return Ok(None);
    }
    let d = crate::ops::to_integer(rt.to_num(v)?);
    if d < lo || d > hi {
        return Err(rt.range_error("toFixed() digits argument must be between 0 and 100"));
    }
    Ok(Some(d as usize))
}

fn to_fixed(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let n = this_num(rt, this)?;
    let d = digits_arg(rt, arg(a, 0), 0.0, 100.0)?.unwrap_or(0);
    let s = numfmt::to_fixed(n, d);
    Ok(rt.new_str(&s))
}

fn to_precision(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let n = this_num(rt, this)?;
    if arg(a, 0) == Value::Undefined {
        let s = numfmt::number_to_string(n);
        return Ok(rt.new_str(&s));
    }
    let p = if n.is_finite() { digits_arg(rt, arg(a, 0), 1.0, 100.0)?.unwrap_or(1) } else { 1 };
    let s = numfmt::to_precision(n, p);
    Ok(rt.new_str(&s))
}

fn to_exponential(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let n = this_num(rt, this)?;
    let d = if n.is_finite() { digits_arg(rt, arg(a, 0), 0.0, 100.0)? } else { None };
    let s = numfmt::to_exponential(n, d);
    Ok(rt.new_str(&s))
}

fn num_value_of(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(this_num(rt, this)?))
}

/// Formats through `Intl.NumberFormat` (always en-US), honouring `options`.
fn to_locale_string(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let n = this_num(rt, this)?;
    let s = super::intl::number_to_locale(rt, Value::Number(n), arg(a, 0), arg(a, 1))?;
    Ok(rt.new_str(&s))
}

fn n_is_nan(_: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(matches!(arg(a, 0), Value::Number(n) if n.is_nan())))
}
fn n_is_finite(_: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(matches!(arg(a, 0), Value::Number(n) if n.is_finite())))
}
fn n_is_integer(_: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(matches!(arg(a, 0), Value::Number(n) if n.is_finite() && n.trunc() == n)))
}
fn n_is_safe_integer(_: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(matches!(arg(a, 0), Value::Number(n) if n.is_finite() && n.trunc() == n && n.abs() <= 9007199254740991.0)))
}

pub(crate) fn parse_int(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let s = rt.to_rust_string(arg(a, 0))?;
    let mut t = s.trim_start_matches(numfmt::is_js_whitespace);
    let mut neg = false;
    if let Some(r) = t.strip_prefix('-') {
        neg = true;
        t = r;
    } else if let Some(r) = t.strip_prefix('+') {
        t = r;
    }
    let mut radix = crate::ops::to_int32(rt.to_num(arg(a, 1))?);
    let mut strip = true;
    if radix != 0 {
        if !(2..=36).contains(&radix) {
            return Ok(Value::Number(f64::NAN));
        }
        if radix != 16 {
            strip = false;
        }
    } else {
        radix = 10;
    }
    if strip && (t.starts_with("0x") || t.starts_with("0X")) {
        t = &t[2..];
        radix = 16;
    }
    let digits: String = t.chars().take_while(|c| c.is_digit(radix as u32)).collect();
    if digits.is_empty() {
        return Ok(Value::Number(f64::NAN));
    }
    let v = if radix == 10 {
        digits.parse::<f64>().unwrap_or(f64::NAN)
    } else {
        digits.chars().fold(0.0, |acc, c| acc * radix as f64 + c.to_digit(radix as u32).unwrap_or(0) as f64)
    };
    Ok(Value::Number(if neg { -v } else { v }))
}

pub(crate) fn parse_float(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let s = rt.to_rust_string(arg(a, 0))?;
    Ok(Value::Number(numfmt::parse_float(&s)))
}

fn boolean_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let b = rt.truthy(arg(a, 0));
    let nt = rt.native_new_target;
    if nt == Value::Undefined {
        return Ok(Value::Bool(b));
    }
    let p = rt.intr.boolean_proto;
    let proto = rt.proto_from_ctor(nt, p)?;
    Ok(Value::Object(rt.alloc(Object::new(proto, ObjKind::Boolean(b)))))
}

fn this_bool(rt: &mut Runtime, this: Value) -> JsResult<bool> {
    match this {
        Value::Bool(b) => Ok(b),
        Value::Object(o) => match rt.obj(o).kind {
            ObjKind::Boolean(b) => Ok(b),
            _ => Err(rt.type_error("Boolean.prototype method called on incompatible receiver")),
        },
        _ => Err(rt.type_error("Boolean.prototype method called on incompatible receiver")),
    }
}

fn bool_to_string(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let b = this_bool(rt, this)?;
    Ok(rt.atom_value(if b { A_TRUE } else { A_FALSE }))
}

fn bool_value_of(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(this_bool(rt, this)?))
}

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto;
    let np = rt.alloc(Object::new(op, ObjKind::Number(0.0)));
    rt.intr.number_proto = Some(np);
    let c = rt.constructor("Number", 1, number_ctor, np);
    for (n, v) in [
        ("MAX_SAFE_INTEGER", 9007199254740991.0),
        ("MIN_SAFE_INTEGER", -9007199254740991.0),
        ("EPSILON", f64::EPSILON),
        ("MAX_VALUE", f64::MAX),
        ("MIN_VALUE", 5e-324),
        ("NaN", f64::NAN),
        ("POSITIVE_INFINITY", f64::INFINITY),
        ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
    ] {
        let k = rt.key_str(n);
        rt.define_value(c, k, Value::Number(v), 0);
    }
    rt.method(c, "isNaN", 1, n_is_nan);
    rt.method(c, "isFinite", 1, n_is_finite);
    rt.method(c, "isInteger", 1, n_is_integer);
    rt.method(c, "isSafeInteger", 1, n_is_safe_integer);
    let pi = rt.method(c, "parseInt", 2, parse_int);
    let pf = rt.method(c, "parseFloat", 1, parse_float);
    rt.set_global("parseInt", Value::Object(pi));
    rt.set_global("parseFloat", Value::Object(pf));
    rt.method(np, "toString", 1, num_to_string);
    rt.method(np, "toFixed", 1, to_fixed);
    rt.method(np, "toPrecision", 1, to_precision);
    rt.method(np, "toExponential", 1, to_exponential);
    rt.method(np, "valueOf", 0, num_value_of);
    rt.method(np, "toLocaleString", 0, to_locale_string);

    let bp = rt.alloc(Object::new(op, ObjKind::Boolean(false)));
    rt.intr.boolean_proto = Some(bp);
    rt.constructor("Boolean", 1, boolean_ctor, bp);
    rt.method(bp, "toString", 0, bool_to_string);
    rt.method(bp, "valueOf", 0, bool_value_of);

    super::math::setup(rt);
}
