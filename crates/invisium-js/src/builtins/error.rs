//! Error and its subclasses.

use super::arg;
use crate::runtime::*;
use crate::value::*;

impl Runtime {
    fn error_proto_for(&self, class: &str) -> Option<ObjId> {
        match class {
            "TypeError" => self.intr.type_error_proto,
            "RangeError" => self.intr.range_error_proto,
            "SyntaxError" => self.intr.syntax_error_proto,
            "ReferenceError" => self.intr.reference_error_proto,
            "EvalError" => self.intr.eval_error_proto,
            "URIError" => self.intr.uri_error_proto,
            _ => self.intr.error_proto,
        }
    }

    /// `Error.stackTraceLimit`, capped at `MAX_STACK_FRAMES`.
    fn stack_trace_limit(&self) -> usize {
        let limit = self.intr.error_ctor.and_then(|c| self.obj(c).props.get(PropKey::Str(A_STACK_TRACE_LIMIT)).map(|p| p.value));
        match limit {
            Some(Value::Number(n)) if n.is_nan() || n <= 0.0 => 0,
            Some(Value::Number(n)) => (n.min(MAX_STACK_FRAMES as f64)) as usize,
            _ => MAX_STACK_FRAMES,
        }
    }

    /// The active frames, innermost first, one `\n    at name (script:line:column)` each.
    /// With `above`, frames up to and including the innermost call of that function are left out.
    pub(crate) fn stack_frames(&self, above: Option<ObjId>) -> String {
        let limit = self.stack_trace_limit();
        let mut out = String::new();
        let mut n = 0;
        let mut skipping = above.is_some();
        for f in self.frames.iter().rev() {
            if skipping {
                skipping = f.callee != above;
                continue;
            }
            if n >= limit {
                break;
            }
            n += 1;
            let p = f.code.pos_at(f.pc.saturating_sub(1));
            let name = if f.code.name.is_empty() { "<anonymous>".to_string() } else { String::from_utf16_lossy(&f.code.name) };
            out.push_str(&format!("\n    at {} ({}:{}:{})", name, f.code.script, p.line, p.col));
        }
        out
    }

    /// Creates an error object of a built-in class (used for engine-raised errors).
    pub(crate) fn make_error(&mut self, class: &str, msg: &str) -> Value {
        let proto = self.error_proto_for(class);
        let o = self.alloc(Object::new(proto, ObjKind::Error));
        let m = self.new_str(msg);
        self.define_value(o, PropKey::Str(A_MESSAGE), m, HIDDEN);
        let stack = format!("{}: {}{}", class, msg, self.stack_frames(None));
        let s = self.new_str(&stack);
        self.define_value(o, PropKey::Str(A_STACK), s, HIDDEN);
        self.throw_pos = self.current_pos();
        Value::Object(o)
    }

    pub fn throw_error(&mut self, class: &str, message: &str) -> crate::Thrown {
        crate::Thrown(self.make_error(class, message))
    }
}

/// `name: message` of an error-like object, as `Error.prototype.toString` renders it.
fn error_head(rt: &mut Runtime, o: Value) -> JsResult<String> {
    let n = rt.get_prop(o, PropKey::Str(A_NAME))?;
    let n = if n == Value::Undefined { "Error".to_string() } else { rt.to_rust_string(n)? };
    let m = rt.get_prop(o, PropKey::Str(A_MESSAGE))?;
    let m = if m == Value::Undefined { String::new() } else { rt.to_rust_string(m)? };
    Ok(if n.is_empty() {
        m
    } else if m.is_empty() {
        n
    } else {
        format!("{}: {}", n, m)
    })
}

/// `Error.captureStackTrace(obj[, above])`: sets `obj.stack` to the current stack.
fn capture_stack_trace(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let Value::Object(o) = arg(a, 0) else { return Err(rt.type_error("Invalid argument: Error.captureStackTrace needs an object")) };
    let head = error_head(rt, arg(a, 0))?;
    let above = match arg(a, 1) {
        Value::Object(f) if rt.is_callable_obj(f) => Some(f),
        _ => None,
    };
    let stack = format!("{}{}", head, rt.stack_frames(above));
    let s = rt.new_str(&stack);
    rt.define_value(o, PropKey::Str(A_STACK), s, HIDDEN);
    Ok(Value::Undefined)
}

fn construct_error(rt: &mut Runtime, args: &[Value], default: Option<ObjId>) -> JsResult<Value> {
    let nt = rt.native_new_target;
    let proto = rt.proto_from_ctor(nt, default)?;
    let o = rt.alloc(Object::new(proto, ObjKind::Error));
    let msg = arg(args, 0);
    if msg != Value::Undefined {
        let s = rt.to_str(msg)?;
        rt.define_value(o, PropKey::Str(A_MESSAGE), Value::String(s), HIDDEN);
    }
    if let Value::Object(opts) = arg(args, 1) {
        if rt.has_property(opts, PropKey::Str(A_CAUSE))? {
            let c = rt.get_prop(Value::Object(opts), PropKey::Str(A_CAUSE))?;
            rt.define_value(o, PropKey::Str(A_CAUSE), c, HIDDEN);
        }
    }
    let head = error_head(rt, Value::Object(o))?;
    let stack = format!("{}{}", head, rt.stack_frames(None));
    let s = rt.new_str(&stack);
    rt.define_value(o, PropKey::Str(A_STACK), s, HIDDEN);
    Ok(Value::Object(o))
}

fn error_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let p = rt.intr.error_proto;
    construct_error(rt, a, p)
}
fn type_error_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let p = rt.intr.type_error_proto;
    construct_error(rt, a, p)
}
fn range_error_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let p = rt.intr.range_error_proto;
    construct_error(rt, a, p)
}
fn syntax_error_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let p = rt.intr.syntax_error_proto;
    construct_error(rt, a, p)
}
fn reference_error_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let p = rt.intr.reference_error_proto;
    construct_error(rt, a, p)
}
fn eval_error_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let p = rt.intr.eval_error_proto;
    construct_error(rt, a, p)
}
fn uri_error_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let p = rt.intr.uri_error_proto;
    construct_error(rt, a, p)
}

fn error_to_string(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    if !matches!(this, Value::Object(_)) {
        return Err(rt.type_error("Error.prototype.toString called on non-object"));
    }
    let s = error_head(rt, this)?;
    Ok(rt.new_str(&s))
}

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto;
    let ep = rt.new_object(op);
    rt.intr.error_proto = Some(ep);
    let ctor = rt.constructor("Error", 1, error_ctor, ep);
    rt.intr.error_ctor = Some(ctor);
    rt.method(ctor, "captureStackTrace", 2, capture_stack_trace);
    rt.define_value(ctor, PropKey::Str(A_STACK_TRACE_LIMIT), Value::Number(MAX_STACK_FRAMES as f64), DEFAULT_FLAGS);
    let n = rt.new_str("Error");
    rt.define_value(ep, PropKey::Str(A_NAME), n, HIDDEN);
    let empty = rt.atom_value(A_EMPTY);
    rt.define_value(ep, PropKey::Str(A_MESSAGE), empty, HIDDEN);
    rt.method(ep, "toString", 0, error_to_string);
    let subs: [(&str, super::NFn); 6] = [
        ("TypeError", type_error_ctor),
        ("RangeError", range_error_ctor),
        ("SyntaxError", syntax_error_ctor),
        ("ReferenceError", reference_error_ctor),
        ("EvalError", eval_error_ctor),
        ("URIError", uri_error_ctor),
    ];
    for (name, f) in subs {
        let p = rt.new_object(Some(ep));
        let c = rt.constructor(name, 1, f, p);
        rt.obj_mut(c).proto = Some(ctor);
        let nv = rt.new_str(name);
        rt.define_value(p, PropKey::Str(A_NAME), nv, HIDDEN);
        rt.define_value(p, PropKey::Str(A_MESSAGE), empty, HIDDEN);
        match name {
            "TypeError" => rt.intr.type_error_proto = Some(p),
            "RangeError" => rt.intr.range_error_proto = Some(p),
            "SyntaxError" => rt.intr.syntax_error_proto = Some(p),
            "ReferenceError" => rt.intr.reference_error_proto = Some(p),
            "EvalError" => rt.intr.eval_error_proto = Some(p),
            _ => rt.intr.uri_error_proto = Some(p),
        }
    }
}
