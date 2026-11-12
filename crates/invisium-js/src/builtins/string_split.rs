//! String.prototype.split / replace / replaceAll with string patterns.

use super::arg;
use super::string::find_units;
use crate::runtime::*;
use crate::value::*;

fn this_units(rt: &mut Runtime, this: Value) -> JsResult<std::rc::Rc<[u16]>> {
    if this.is_nullish() {
        return Err(rt.type_error("String.prototype method called on null or undefined"));
    }
    rt.to_units(this)
}

pub(super) fn split(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let lim = match arg(a, 1) {
        Value::Undefined => u32::MAX,
        v => crate::ops::to_uint32(rt.to_num(v)?),
    } as usize;
    let out = Value::Object(rt.new_array(Vec::new()));
    let mut parts: Vec<Vec<u16>> = Vec::new();
    if lim == 0 {
        return Ok(out);
    }
    match arg(a, 0) {
        Value::Undefined => parts.push(s.to_vec()),
        sepv => {
            let sep = rt.to_units(sepv)?;
            if sep.is_empty() {
                for &u in s.iter().take(lim) {
                    parts.push(vec![u]);
                }
            } else {
                let mut start = 0;
                while let Some(i) = find_units(&s, &sep, start) {
                    parts.push(s[start..i].to_vec());
                    start = i + sep.len();
                    if parts.len() >= lim {
                        break;
                    }
                }
                if parts.len() < lim {
                    parts.push(s[start..].to_vec());
                }
            }
        }
    }
    for (i, p) in parts.into_iter().take(lim).enumerate() {
        let v = rt.new_str16(p);
        super::array::set_index(rt, out, i as u64, v)?;
    }
    Ok(out)
}

/// GetSubstitution for string patterns ($$, $&, $`, $').
fn substitute(matched: &[u16], s: &[u16], pos: usize, repl: &[u16], out: &mut Vec<u16>) {
    let mut i = 0;
    while i < repl.len() {
        let c = repl[i];
        if c == b'$' as u16 && i + 1 < repl.len() {
            let n = repl[i + 1];
            match n {
                x if x == b'$' as u16 => {
                    out.push(b'$' as u16);
                    i += 2;
                    continue;
                }
                x if x == b'&' as u16 => {
                    out.extend_from_slice(matched);
                    i += 2;
                    continue;
                }
                x if x == b'`' as u16 => {
                    out.extend_from_slice(&s[..pos]);
                    i += 2;
                    continue;
                }
                x if x == b'\'' as u16 => {
                    out.extend_from_slice(&s[(pos + matched.len()).min(s.len())..]);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
        i += 1;
    }
}

fn replace_impl(rt: &mut Runtime, this: Value, a: &[Value], all: bool) -> JsResult<Value> {
    let s = this_units(rt, this)?;
    let pat = rt.to_units(arg(a, 0))?;
    let rv = arg(a, 1);
    let func = rt.is_callable_val(rv);
    let repl = if func { None } else { Some(rt.to_units(rv)?) };
    let mut positions = Vec::new();
    let mut from = 0;
    while let Some(i) = find_units(&s, &pat, from) {
        positions.push(i);
        if !all {
            break;
        }
        from = i + pat.len().max(1);
        if from > s.len() {
            break;
        }
    }
    let mut out = Vec::with_capacity(s.len());
    let mut last = 0;
    for p in positions {
        out.extend_from_slice(&s[last..p]);
        let matched = &s[p..p + pat.len()];
        if func {
            let m = rt.new_str16(matched.to_vec());
            let whole = rt.new_str16(s.to_vec());
            let r = rt.call_value(rv, Value::Undefined, &[m, Value::Number(p as f64), whole])?;
            let ru = rt.to_units(r)?;
            out.extend_from_slice(&ru);
        } else if let Some(r) = &repl {
            substitute(matched, &s, p, r, &mut out);
        }
        last = p + pat.len();
        if out.len() > (1 << 28) {
            return Err(rt.range_error("Invalid string length"));
        }
    }
    out.extend_from_slice(&s[last.min(s.len())..]);
    Ok(rt.new_str16(out))
}

pub(super) fn replace(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> {
    replace_impl(rt, t, a, false)
}

pub(super) fn replace_all(rt: &mut Runtime, t: Value, a: &[Value]) -> JsResult<Value> {
    replace_impl(rt, t, a, true)
}
