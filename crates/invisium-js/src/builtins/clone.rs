//! `structuredClone`: a deep copy of plain data, Map, Set, Date, RegExp, Error, primitive
//! wrappers, ArrayBuffer and its views. Cycles and shared references are preserved; anything
//! else (functions, symbols, host objects, proxies, promises ...) is a `DataCloneError`.

use super::arg;
use crate::runtime::*;
use crate::value::*;
use std::collections::HashMap;
use std::rc::Rc;

/// What has to be copied out of a source object before any script can run again.
enum Shape {
    Wrapper(ObjKind),
    Date(f64),
    RegExp(Rc<RegexProg>),
    Buffer(usize),
    View(ViewData, bool),
    Collection(Vec<(Value, Value)>, bool),
    Array(Vec<Option<Value>>),
    Error,
    Plain,
    Refused(&'static str),
}

struct Cloner {
    /// Source object -> its copy: keeps identity (and terminates on cycles).
    seen: HashMap<u32, ObjId>,
    nodes: u64,
}

fn clone_error(rt: &mut Runtime, what: &str) -> Exc {
    let e = rt.make_error("Error", &format!("{} could not be cloned.", what));
    if let Value::Object(o) = e {
        let name = rt.new_str("DataCloneError");
        rt.define_value(o, PropKey::Str(A_NAME), name, HIDDEN);
    }
    Exc::Throw(e)
}

fn shape_of(rt: &Runtime, o: ObjId) -> Shape {
    match &rt.obj(o).kind {
        ObjKind::Ordinary | ObjKind::Arguments(_) => Shape::Plain,
        ObjKind::Array(items) => Shape::Array(items.clone()),
        ObjKind::Boolean(b) => Shape::Wrapper(ObjKind::Boolean(*b)),
        ObjKind::Number(n) => Shape::Wrapper(ObjKind::Number(*n)),
        ObjKind::String(s) => Shape::Wrapper(ObjKind::String(*s)),
        ObjKind::Date(t) => Shape::Date(*t),
        ObjKind::RegExp(p) => Shape::RegExp(p.clone()),
        ObjKind::ArrayBuffer(b) => Shape::Buffer(b.len()),
        ObjKind::TypedArray(v) => Shape::View(*v, false),
        ObjKind::DataView(v) => Shape::View(*v, true),
        ObjKind::Map(d) => Shape::Collection(d.entries.iter().flatten().copied().collect(), false),
        ObjKind::Set(d) => Shape::Collection(d.entries.iter().flatten().copied().collect(), true),
        ObjKind::Error => Shape::Error,
        ObjKind::Function(_) => Shape::Refused("A function"),
        ObjKind::Proxy(_) => Shape::Refused("A Proxy"),
        ObjKind::Host(..) => Shape::Refused("A host object"),
        ObjKind::Promise(_) => Shape::Refused("A Promise"),
        ObjKind::WeakMap(_) | ObjKind::WeakSet(_) => Shape::Refused("A weak collection"),
        _ => Shape::Refused("This object"),
    }
}

const ERROR_NAMES: [&str; 7] = ["Error", "EvalError", "RangeError", "ReferenceError", "SyntaxError", "TypeError", "URIError"];

fn clone_value(rt: &mut Runtime, c: &mut Cloner, v: Value) -> JsResult<Value> {
    let o = match v {
        Value::Symbol(_) => return Err(clone_error(rt, "A Symbol")),
        Value::Object(o) => o,
        primitive => return Ok(primitive),
    };
    if let Some(done) = c.seen.get(&o.0) {
        return Ok(Value::Object(*done));
    }
    // depth is bounded by the native stack budget, work by the step budget, size by the cap
    rt.check_native_stack()?;
    c.nodes += 1;
    if c.nodes % 64 == 0 {
        rt.steps = rt.steps.saturating_add(63);
        rt.safepoint_counted()?;
        rt.check_alloc(c.seen.len() * 48).map_err(|t| rt.thrown_to_exc(t))?;
    }
    let source = Value::Object(o);
    let shape = shape_of(rt, o);
    // a getter run later may unlink these from the source: keep the snapshot alive
    match &shape {
        Shape::Array(items) => rt.scratch.extend(items.iter().flatten()),
        Shape::Collection(entries, _) => rt.scratch.extend(entries.iter().flat_map(|(k, v)| [*k, *v])),
        _ => {}
    }
    let out = match shape {
        Shape::Refused(what) => return Err(clone_error(rt, what)),
        Shape::Wrapper(kind) => {
            let proto = match kind {
                ObjKind::Boolean(_) => rt.intr.boolean_proto,
                ObjKind::Number(_) => rt.intr.number_proto,
                _ => rt.intr.string_proto,
            };
            rt.alloc(Object::new(proto, kind))
        }
        Shape::Date(t) => {
            let proto = rt.intr.date_proto;
            rt.alloc(Object::new(proto, ObjKind::Date(t)))
        }
        Shape::RegExp(prog) => {
            let proto = rt.intr.regexp_proto;
            match rt.regexp_alloc(prog, proto) {
                Value::Object(r) => r,
                _ => return Err(clone_error(rt, "This object")),
            }
        }
        Shape::Buffer(len) => {
            rt.reserve_bytes(len)?;
            let bytes = rt.buffer_ref(o).cloned().unwrap_or_default();
            let proto = rt.intr.array_buffer_proto;
            rt.new_buffer(bytes, proto)
        }
        Shape::View(view, is_data_view) => {
            // the buffer goes through the map, so views of one buffer still share one copy
            let Value::Object(buffer) = clone_value(rt, c, Value::Object(view.buffer))? else {
                return Err(clone_error(rt, "This object"));
            };
            let copy = ViewData { buffer, ..view };
            let (proto, kind) = if is_data_view { (rt.intr.data_view_proto, ObjKind::DataView(copy)) } else { (rt.intr.typed_protos[view.kind as usize], ObjKind::TypedArray(copy)) };
            rt.alloc(Object::new(proto, kind))
        }
        Shape::Collection(entries, is_set) => {
            let (proto, kind) = if is_set { (rt.intr.set_proto, ObjKind::Set(Box::default())) } else { (rt.intr.map_proto, ObjKind::Map(Box::default())) };
            let out = rt.alloc(Object::new(proto, kind));
            c.seen.insert(o.0, out);
            for (k, val) in entries {
                let ck = clone_value(rt, c, k)?;
                let cv = if is_set { ck } else { clone_value(rt, c, val)? };
                rt.map_insert(out, ck, cv);
                rt.drain_pending_steps()?;
            }
            out
        }
        Shape::Array(items) => {
            rt.check_alloc(items.len() * 16).map_err(|t| rt.thrown_to_exc(t))?;
            let out = rt.new_array(vec![None; items.len()]);
            c.seen.insert(o.0, out);
            for (i, item) in items.into_iter().enumerate() {
                if let Some(x) = item {
                    let cx = clone_value(rt, c, x)?;
                    if let ObjKind::Array(dst) = &mut rt.obj_mut(out).kind {
                        if let Some(slot) = dst.get_mut(i) {
                            *slot = Some(cx);
                        }
                    }
                }
            }
            out
        }
        Shape::Error => {
            let name = rt.get_prop(source, PropKey::Str(A_NAME))?;
            let name = rt.to_rust_string(name)?;
            let class = ERROR_NAMES.iter().find(|n| **n == name).copied().unwrap_or("Error");
            let message = match rt.get_own_prop(o, PropKey::Str(A_MESSAGE)) {
                Some(p) if !p.is_accessor() => rt.to_rust_string(p.value)?,
                _ => String::new(),
            };
            let Value::Object(out) = rt.make_error(class, &message) else { return Err(clone_error(rt, "This object")) };
            c.seen.insert(o.0, out);
            for key in [A_STACK, A_CAUSE] {
                if let Some(p) = rt.get_own_prop(o, PropKey::Str(key)).filter(|p| !p.is_accessor()) {
                    let cv = clone_value(rt, c, p.value)?;
                    rt.define_value(out, PropKey::Str(key), cv, HIDDEN);
                }
            }
            out
        }
        Shape::Plain => rt.plain_object(),
    };
    c.seen.insert(o.0, out);
    // own enumerable string-keyed properties of plain objects (getters run), and the named
    // extras of an array
    if matches!(rt.obj(out).kind, ObjKind::Ordinary | ObjKind::Array(_)) {
        let keys: Vec<PropKey> = if matches!(rt.obj(o).kind, ObjKind::Array(_)) {
            rt.obj(o).props.iter().filter(|(k, p)| matches!(k, PropKey::Str(_)) && p.flags & ENUMERABLE != 0).map(|(k, _)| *k).collect()
        } else {
            rt.own_enum_keys_ex(o)?
        };
        for k in keys {
            let x = rt.get_prop(source, k)?;
            let cx = clone_value(rt, c, x)?;
            rt.define_value(out, k, cx, DEFAULT_FLAGS);
        }
    }
    Ok(Value::Object(out))
}

fn structured_clone(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    if a.is_empty() {
        return Err(rt.type_error("structuredClone requires 1 argument"));
    }
    let mut c = Cloner { seen: HashMap::new(), nodes: 0 };
    clone_value(rt, &mut c, arg(a, 0))
}

pub(super) fn setup(rt: &mut Runtime) {
    let g = rt.global;
    rt.method(g, "structuredClone", 1, structured_clone);
}
