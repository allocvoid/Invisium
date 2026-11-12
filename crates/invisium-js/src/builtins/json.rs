//! JSON.parse (iterative, no native recursion on nesting) and JSON.stringify.

use super::arg;
use crate::runtime::*;
use crate::value::*;

struct P<'a> {
    s: &'a [u16],
    i: usize,
}

impl<'a> P<'a> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], 0x20 | 0x09 | 0x0A | 0x0D) {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u16> {
        self.s.get(self.i).copied()
    }
}

fn syntax(rt: &mut Runtime, p: &P) -> Exc {
    let msg = match p.peek() {
        Some(c) => format!("Unexpected token '{}' in JSON at position {}", char::from_u32(c as u32).unwrap_or('?'), p.i),
        None => "Unexpected end of JSON input".to_string(),
    };
    Exc::Throw(rt.make_error("SyntaxError", &msg))
}

fn parse_string(rt: &mut Runtime, p: &mut P) -> JsResult<Vec<u16>> {
    p.i += 1;
    let mut out = Vec::new();
    loop {
        let Some(c) = p.peek() else { return Err(syntax(rt, p)) };
        p.i += 1;
        match c {
            0x22 => return Ok(out),
            0x5C => {
                let Some(e) = p.peek() else { return Err(syntax(rt, p)) };
                p.i += 1;
                match e {
                    0x22 | 0x5C | 0x2F => out.push(e),
                    0x62 => out.push(8),
                    0x66 => out.push(12),
                    0x6E => out.push(10),
                    0x72 => out.push(13),
                    0x74 => out.push(9),
                    0x75 => {
                        let mut v = 0u32;
                        for _ in 0..4 {
                            let d = p.peek().and_then(|d| char::from_u32(d as u32)).and_then(|d| d.to_digit(16));
                            match d {
                                Some(d) => v = v * 16 + d,
                                None => return Err(syntax(rt, p)),
                            }
                            p.i += 1;
                        }
                        out.push(v as u16);
                    }
                    _ => {
                        p.i -= 1;
                        return Err(syntax(rt, p));
                    }
                }
            }
            c if c < 0x20 => {
                p.i -= 1;
                return Err(syntax(rt, p));
            }
            c => out.push(c),
        }
    }
}

fn parse_number(rt: &mut Runtime, p: &mut P) -> JsResult<f64> {
    let start = p.i;
    let digit = |c: Option<u16>| matches!(c, Some(x) if (0x30..=0x39).contains(&x));
    if p.peek() == Some(0x2D) {
        p.i += 1;
    }
    if p.peek() == Some(0x30) {
        p.i += 1;
    } else if digit(p.peek()) {
        while digit(p.peek()) {
            p.i += 1;
        }
    } else {
        return Err(syntax(rt, p));
    }
    if p.peek() == Some(0x2E) {
        p.i += 1;
        if !digit(p.peek()) {
            return Err(syntax(rt, p));
        }
        while digit(p.peek()) {
            p.i += 1;
        }
    }
    if matches!(p.peek(), Some(0x65) | Some(0x45)) {
        p.i += 1;
        if matches!(p.peek(), Some(0x2B) | Some(0x2D)) {
            p.i += 1;
        }
        if !digit(p.peek()) {
            return Err(syntax(rt, p));
        }
        while digit(p.peek()) {
            p.i += 1;
        }
    }
    let text = String::from_utf16_lossy(&p.s[start..p.i]);
    Ok(text.parse::<f64>().unwrap_or(f64::NAN))
}

fn parse_value_text(rt: &mut Runtime, text: &[u16]) -> JsResult<Value> {
    let mut p = P { s: text, i: 0 };
    let mut stack: Vec<(ObjId, Option<PropKey>, u32)> = Vec::new();
    let mark = rt.scratch.len();
    let result = 'outer: loop {
        p.ws();
        let mut v = match p.peek() {
            Some(0x5B) => {
                p.i += 1;
                let a = rt.new_array(Vec::new());
                rt.scratch.push(Value::Object(a));
                p.ws();
                if p.peek() == Some(0x5D) {
                    p.i += 1;
                    Value::Object(a)
                } else {
                    stack.push((a, None, 0));
                    continue;
                }
            }
            Some(0x7B) => {
                p.i += 1;
                let o = rt.plain_object();
                rt.scratch.push(Value::Object(o));
                p.ws();
                if p.peek() == Some(0x7D) {
                    p.i += 1;
                    Value::Object(o)
                } else {
                    if p.peek() != Some(0x22) {
                        break 'outer Err(syntax(rt, &p));
                    }
                    let k = parse_string(rt, &mut p)?;
                    p.ws();
                    if p.peek() != Some(0x3A) {
                        break 'outer Err(syntax(rt, &p));
                    }
                    p.i += 1;
                    let key = PropKey::Str(rt.intern16(&k));
                    stack.push((o, Some(key), 0));
                    continue;
                }
            }
            Some(0x22) => {
                let s = parse_string(rt, &mut p)?;
                rt.new_str16(s)
            }
            Some(c) if c == 0x2D || (0x30..=0x39).contains(&c) => Value::Number(parse_number(rt, &mut p)?),
            Some(0x74) if p.s[p.i..].starts_with(&[0x74, 0x72, 0x75, 0x65]) => {
                p.i += 4;
                Value::Bool(true)
            }
            Some(0x66) if p.s[p.i..].starts_with(&[0x66, 0x61, 0x6C, 0x73, 0x65]) => {
                p.i += 5;
                Value::Bool(false)
            }
            Some(0x6E) if p.s[p.i..].starts_with(&[0x6E, 0x75, 0x6C, 0x6C]) => {
                p.i += 4;
                Value::Null
            }
            _ => break 'outer Err(syntax(rt, &p)),
        };
        loop {
            let Some(top) = stack.last_mut() else { break 'outer Ok(v) };
            let (cont, key) = (top.0, top.1);
            match key {
                None => {
                    if let ObjKind::Array(items) = &mut rt.obj_mut(cont).kind {
                        items.push(Some(v));
                    }
                    rt.grow(16);
                }
                Some(k) => rt.define_value(cont, k, v, DEFAULT_FLAGS),
            }
            p.ws();
            let close = if key.is_none() { 0x5D } else { 0x7D };
            match p.peek() {
                Some(0x2C) => {
                    p.i += 1;
                    if key.is_some() {
                        p.ws();
                        if p.peek() != Some(0x22) {
                            break 'outer Err(syntax(rt, &p));
                        }
                        let k = parse_string(rt, &mut p)?;
                        p.ws();
                        if p.peek() != Some(0x3A) {
                            break 'outer Err(syntax(rt, &p));
                        }
                        p.i += 1;
                        let nk = PropKey::Str(rt.intern16(&k));
                        if let Some(t) = stack.last_mut() {
                            t.1 = Some(nk);
                        }
                    }
                    continue 'outer;
                }
                Some(c) if c == close => {
                    p.i += 1;
                    v = Value::Object(cont);
                    stack.pop();
                }
                _ => break 'outer Err(syntax(rt, &p)),
            }
        }
    };
    let r = match result {
        Ok(v) => {
            p.ws();
            if p.i < p.s.len() { Err(syntax(rt, &p)) } else { Ok(v) }
        }
        Err(e) => Err(e),
    };
    if let Ok(v) = r {
        rt.scratch.truncate(mark);
        rt.scratch.push(v);
    }
    r
}

fn internalize(rt: &mut Runtime, holder: ObjId, key: PropKey, reviver: Value) -> JsResult<Value> {
    rt.check_native_stack()?;
    let val = rt.get_prop(Value::Object(holder), key)?;
    if let Value::Object(o) = val {
        let keys: Vec<PropKey> = if let ObjKind::Array(items) = &rt.obj(o).kind {
            let n = items.len();
            (0..n as u64).map(|i| rt.index_key(i)).collect()
        } else {
            rt.own_enum_keys_ex(o)?
        };
        for k in keys {
            let nv = internalize(rt, o, k, reviver)?;
            if nv == Value::Undefined {
                rt.delete_prop(o, k);
            } else {
                rt.define_value(o, k, nv, DEFAULT_FLAGS);
            }
        }
    }
    let kv = rt.key_to_value(key);
    rt.call_value(reviver, Value::Object(holder), &[kv, val])
}

fn json_parse(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let text = rt.to_units(arg(a, 0))?;
    let v = parse_value_text(rt, &text)?;
    let reviver = arg(a, 1);
    if rt.is_callable_val(reviver) {
        let root = rt.plain_object();
        rt.define_value(root, PropKey::Str(A_EMPTY), v, DEFAULT_FLAGS);
        return internalize(rt, root, PropKey::Str(A_EMPTY), reviver);
    }
    Ok(v)
}

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto;
    let j = rt.new_object(op);
    rt.set_global("JSON", Value::Object(j));
    rt.method(j, "parse", 2, json_parse);
    rt.method(j, "stringify", 3, super::json_stringify::stringify);
    let tag = rt.new_str("JSON");
    rt.define_value(j, PropKey::Sym(SYM_TO_STRING_TAG), tag, CONFIGURABLE);
}
