//! Function constructor and Function.prototype.

use super::arg;
use crate::runtime::*;
use crate::value::*;

fn function_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let mut params = Vec::new();
    for v in a.iter().take(a.len().saturating_sub(1)) {
        params.push(rt.to_rust_string(*v)?);
    }
    let body = match a.last() {
        Some(v) => rt.to_rust_string(*v)?,
        None => String::new(),
    };
    let src = format!("(function anonymous({}\n) {{\n{}\n}})", params.join(","), body);
    rt.eval_internal(&src, "<anonymous>")
}

fn call(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let f = rt.callable_arg(this, "Function.prototype.call")?;
    let rest = if a.len() > 1 { &a[1..] } else { &[] };
    rt.call_value(f, arg(a, 0), rest)
}

pub(crate) fn list_from_array_like(rt: &mut Runtime, v: Value) -> JsResult<Vec<Value>> {
    if v.is_nullish() {
        return Ok(Vec::new());
    }
    if !matches!(v, Value::Object(_)) {
        return Err(rt.type_error("CreateListFromArrayLike called on non-object"));
    }
    let n = rt.length_of(v)?;
    if n > 10_000_000 {
        return Err(rt.range_error("Too many arguments in function call"));
    }
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        out.push(rt.get_idx(v, i)?);
    }
    Ok(out)
}

fn apply(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let f = rt.callable_arg(this, "Function.prototype.apply")?;
    let args = list_from_array_like(rt, arg(a, 1))?;
    rt.call_value(f, arg(a, 0), &args)
}

fn bind(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let f = rt.callable_arg(this, "Function.prototype.bind")?;
    let Value::Object(target) = f else { return Err(rt.type_error("Bind must be called on a function")) };
    let bargs: Vec<Value> = a.iter().skip(1).copied().collect();
    let tl = rt.get_prop(f, PropKey::Str(A_LENGTH))?;
    let len = match tl {
        Value::Number(n) => (n - bargs.len() as f64).max(0.0),
        _ => 0.0,
    };
    let tn = rt.get_prop(f, PropKey::Str(A_NAME))?;
    let tn = match tn {
        Value::String(s) => rt.rust_string(s),
        _ => String::new(),
    };
    let proto = rt.obj(target).proto;
    let o = rt.alloc(Object::new(proto, ObjKind::Function(Box::new(FuncData::Bound { target, this: arg(a, 0), args: bargs }))));
    rt.define_value(o, PropKey::Str(A_LENGTH), Value::Number(len), CONFIGURABLE);
    let n = rt.new_str(&format!("bound {}", tn));
    rt.define_value(o, PropKey::Str(A_NAME), n, CONFIGURABLE);
    Ok(Value::Object(o))
}

fn to_string(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let Value::Object(o) = this else { return Err(rt.type_error("Function.prototype.toString requires that 'this' be a Function")) };
    let text = match &rt.obj(o).kind {
        ObjKind::Function(f) => match &**f {
            FuncData::Closure(c) => {
                let (s, e) = c.code.source_range;
                if e > s && e <= c.code.source.len() {
                    Some(c.code.source[s..e].iter().collect::<String>())
                } else {
                    None
                }
            }
            _ => None,
        },
        _ => return Err(rt.type_error("Function.prototype.toString requires that 'this' be a Function")),
    };
    let text = match text {
        Some(t) => t,
        None => {
            let n = rt.get_prop(this, PropKey::Str(A_NAME))?;
            let n = match n {
                Value::String(s) => rt.rust_string(s),
                _ => String::new(),
            };
            format!("function {}() {{ [native code] }}", n)
        }
    };
    Ok(rt.new_str(&text))
}

fn has_instance(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let Value::Object(t) = this else { return Ok(Value::Bool(false)) };
    if !rt.is_callable_obj(t) {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(rt.ordinary_has_instance(t, arg(a, 0))?))
}

pub(super) fn setup(rt: &mut Runtime) {
    let fp = rt.intr.function_proto.unwrap_or(ObjId(0));
    rt.define_value(fp, PropKey::Str(A_LENGTH), Value::Number(0.0), CONFIGURABLE);
    rt.define_value(fp, PropKey::Str(A_NAME), Value::String(StrId::from_atom(A_EMPTY)), CONFIGURABLE);
    rt.constructor("Function", 1, function_ctor, fp);
    rt.method(fp, "call", 1, call);
    rt.method(fp, "apply", 2, apply);
    rt.method(fp, "bind", 1, bind);
    rt.method(fp, "toString", 0, to_string);
    let hi = rt.native_fn("[Symbol.hasInstance]", 1, has_instance);
    rt.define_value(fp, PropKey::Sym(SYM_HAS_INSTANCE), Value::Object(hi), 0);
    rt.fn_has_instance = Some(hi);
}
