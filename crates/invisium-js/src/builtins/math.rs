//! The Math object.

use super::arg;
use crate::runtime::*;
use crate::value::*;

macro_rules! math1 {
    ($($name:ident => $f:expr),* $(,)?) => {
        $(fn $name(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
            let x = rt.to_num(arg(a, 0))?;
            let f: fn(f64) -> f64 = $f;
            Ok(Value::Number(f(x)))
        })*
    };
}

fn js_round(x: f64) -> f64 {
    if !x.is_finite() || x.fract() == 0.0 {
        return x;
    }
    let f = x.floor();
    let r = if x - f >= 0.5 { f + 1.0 } else { f };
    if r == 0.0 && x < 0.0 { -0.0 } else { r }
}

fn sign(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 { x } else if x > 0.0 { 1.0 } else { -1.0 }
}

math1! {
    m_abs => f64::abs, m_acos => f64::acos, m_acosh => f64::acosh, m_asin => f64::asin,
    m_asinh => f64::asinh, m_atan => f64::atan, m_atanh => f64::atanh, m_cbrt => f64::cbrt,
    m_ceil => f64::ceil, m_cos => f64::cos, m_cosh => f64::cosh, m_exp => f64::exp,
    m_expm1 => f64::exp_m1, m_floor => f64::floor, m_log => f64::ln, m_log1p => f64::ln_1p,
    m_log10 => f64::log10, m_log2 => f64::log2, m_round => js_round, m_sign => sign,
    m_sin => f64::sin, m_sinh => f64::sinh, m_sqrt => f64::sqrt, m_tan => f64::tan,
    m_tanh => f64::tanh, m_trunc => f64::trunc, m_fround => |x| x as f32 as f64,
    m_clz32 => |x| crate::ops::to_uint32(x).leading_zeros() as f64,
}

fn m_atan2(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let y = rt.to_num(arg(a, 0))?;
    let x = rt.to_num(arg(a, 1))?;
    Ok(Value::Number(y.atan2(x)))
}

fn m_pow(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let x = rt.to_num(arg(a, 0))?;
    let y = rt.to_num(arg(a, 1))?;
    let r = if y.is_nan() || ((x == 1.0 || x == -1.0) && y.is_infinite()) { f64::NAN } else { x.powf(y) };
    Ok(Value::Number(r))
}

fn m_imul(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let x = crate::ops::to_int32(rt.to_num(arg(a, 0))?);
    let y = crate::ops::to_int32(rt.to_num(arg(a, 1))?);
    Ok(Value::Number(x.wrapping_mul(y) as f64))
}

fn m_hypot(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let mut nums = Vec::new();
    for v in a {
        nums.push(rt.to_num(*v)?);
    }
    if nums.iter().any(|n| n.is_infinite()) {
        return Ok(Value::Number(f64::INFINITY));
    }
    Ok(Value::Number(nums.iter().map(|n| n * n).sum::<f64>().sqrt()))
}

fn minmax(rt: &mut Runtime, a: &[Value], max: bool) -> JsResult<Value> {
    let mut r = if max { f64::NEG_INFINITY } else { f64::INFINITY };
    let mut nan = false;
    for v in a {
        let n = rt.to_num(*v)?;
        if n.is_nan() {
            nan = true;
        } else if max {
            if n > r || (n == 0.0 && r == 0.0 && !n.is_sign_negative()) {
                r = n;
            }
        } else if n < r || (n == 0.0 && r == 0.0 && n.is_sign_negative()) {
            r = n;
        }
    }
    Ok(Value::Number(if nan { f64::NAN } else { r }))
}

fn m_max(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    minmax(rt, a, true)
}
fn m_min(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    minmax(rt, a, false)
}

fn m_random(rt: &mut Runtime, _: Value, _: &[Value]) -> JsResult<Value> {
    let r = (rt.random)();
    Ok(Value::Number(if (0.0..1.0).contains(&r) { r } else { 0.0 }))
}

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto;
    let m = rt.new_object(op);
    rt.set_global("Math", Value::Object(m));
    for (n, v) in [
        ("E", std::f64::consts::E),
        ("LN10", std::f64::consts::LN_10),
        ("LN2", std::f64::consts::LN_2),
        ("LOG10E", std::f64::consts::LOG10_E),
        ("LOG2E", std::f64::consts::LOG2_E),
        ("PI", std::f64::consts::PI),
        ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
        ("SQRT2", std::f64::consts::SQRT_2),
    ] {
        let k = rt.key_str(n);
        rt.define_value(m, k, Value::Number(v), 0);
    }
    let fns: [(&str, u32, super::NFn); 35] = [
        ("abs", 1, m_abs), ("acos", 1, m_acos), ("acosh", 1, m_acosh), ("asin", 1, m_asin),
        ("asinh", 1, m_asinh), ("atan", 1, m_atan), ("atanh", 1, m_atanh), ("atan2", 2, m_atan2),
        ("cbrt", 1, m_cbrt), ("ceil", 1, m_ceil), ("clz32", 1, m_clz32), ("cos", 1, m_cos),
        ("cosh", 1, m_cosh), ("exp", 1, m_exp), ("expm1", 1, m_expm1), ("floor", 1, m_floor),
        ("fround", 1, m_fround), ("hypot", 2, m_hypot), ("imul", 2, m_imul), ("log", 1, m_log),
        ("log1p", 1, m_log1p), ("log10", 1, m_log10), ("log2", 1, m_log2), ("max", 2, m_max),
        ("min", 2, m_min), ("pow", 2, m_pow), ("random", 0, m_random), ("round", 1, m_round),
        ("sign", 1, m_sign), ("sin", 1, m_sin), ("sinh", 1, m_sinh), ("sqrt", 1, m_sqrt),
        ("tan", 1, m_tan), ("tanh", 1, m_tanh), ("trunc", 1, m_trunc),
    ];
    for (n, ar, f) in fns {
        rt.method(m, n, ar, f);
    }
    let tag = rt.new_str("Math");
    rt.define_value(m, PropKey::Sym(SYM_TO_STRING_TAG), tag, CONFIGURABLE);
}
