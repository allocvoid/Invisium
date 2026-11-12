//! `RegExp.prototype[Symbol.match / matchAll / search / split]` and the matchAll iterator.
//! `Symbol.replace` and the `String.prototype` entry points are in `regexp_replace.rs`.

use super::arg;
use super::array::set_index;
use super::regexp::advance_index;
use crate::runtime::*;
use crate::value::*;
use invisium_regex::Flags;
use std::rc::Rc;

pub(super) fn this_regexp_object(rt: &mut Runtime, this: Value, what: &str) -> JsResult<()> {
    if matches!(this, Value::Object(_)) {
        return Ok(());
    }
    Err(rt.type_error(&format!("RegExp.prototype[{}] called on incompatible receiver", what)))
}

impl Runtime {
    /// ToString(Get(rx, "flags")).
    pub(crate) fn regexp_flags_text(&mut self, rx: Value) -> JsResult<String> {
        let f = self.get_prop(rx, PropKey::Str(A_FLAGS))?;
        self.to_rust_string(f)
    }

    /// The compiled program when `rx` is a RegExp whose `exec` is the built-in one.
    pub(crate) fn regexp_pristine(&mut self, rx: Value) -> JsResult<Option<Rc<RegexProg>>> {
        let Some(prog) = self.regex_prog(rx) else { return Ok(None) };
        let exec = self.get_prop(rx, PropKey::Str(A_EXEC))?;
        Ok(if exec.as_obj().is_some() && exec.as_obj() == self.intr.regexp_exec { Some(prog) } else { None })
    }

    /// SpeciesConstructor(rx, %RegExp%): the object's own constructor (a subclass makes
    /// instances of itself), the intrinsic when there is none.
    pub(crate) fn regexp_species(&mut self, rx: Value) -> JsResult<Value> {
        let default = self.intr.regexp_ctor.map(Value::Object).unwrap_or(Value::Undefined);
        let c = self.get_prop(rx, PropKey::Str(A_CONSTRUCTOR))?;
        match c {
            Value::Undefined => Ok(default),
            Value::Object(_) if self.is_constructor(c) => Ok(c),
            Value::Object(_) => Ok(default),
            _ => Err(self.type_error("The .constructor property is not an object")),
        }
    }

    /// Steps `lastIndex` past an empty match so a global loop makes progress.
    pub(crate) fn bump_last_index(&mut self, rx: Value, s: &[u16], unicode: bool) -> JsResult<()> {
        let li = self.last_index(rx)?;
        let next = if li < s.len() as f64 { advance_index(s, li as usize, unicode) as f64 } else { li + 1.0 };
        self.set_last_index(rx, next)
    }
}

fn sym_match(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    this_regexp_object(rt, this, "Symbol.match")?;
    let s = rt.to_str(arg(a, 0))?;
    let flags = rt.regexp_flags_text(this)?;
    if !flags.contains('g') {
        return rt.regexp_exec(this, s, true);
    }
    let unicode = flags.contains('u') || flags.contains('v');
    rt.set_last_index(this, 0.0)?;
    let units = rt.str_rc(s);
    let fast = rt.regexp_pristine(this)?;
    let out = Value::Object(rt.new_array(Vec::new()));
    let mark = rt.scratch.len();
    let mut n = 0u64;
    loop {
        let matched: Vec<u16> = match &fast {
            Some(prog) => match rt.regexp_match_raw(this, prog, &units)? {
                None => break,
                Some(c) => c.get(0).and_then(|(x, y)| units.get(x..y)).unwrap_or(&[]).to_vec(),
            },
            None => {
                let r = rt.regexp_exec(this, s, true)?;
                if r == Value::Null {
                    break;
                }
                let m0 = rt.get_idx(r, 0)?;
                rt.to_units(m0)?.to_vec()
            }
        };
        if matched.is_empty() {
            rt.bump_last_index(this, &units, unicode)?;
        }
        let v = rt.new_str16(matched);
        set_index(rt, out, n, v)?;
        n += 1;
        rt.scratch.truncate(mark);
        rt.safepoint()?;
    }
    Ok(if n == 0 { Value::Null } else { out })
}

fn sym_match_all(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    this_regexp_object(rt, this, "Symbol.matchAll")?;
    let s = rt.to_str(arg(a, 0))?;
    let c = rt.regexp_species(this)?;
    let flags = rt.regexp_flags_text(this)?;
    let fv = rt.new_str(&flags);
    let matcher = rt.construct_value(c, &[this, fv], c)?;
    rt.scratch.push(matcher);
    let li = rt.last_index(this)?;
    rt.set_last_index(matcher, li)?;
    let (global, unicode) = (flags.contains('g'), flags.contains('u') || flags.contains('v'));
    let proto = rt.intr.regexp_iter_proto;
    Ok(Value::Object(rt.alloc(Object::new(proto, ObjKind::RegExpStringIter { rx: matcher, s, global, unicode, done: false }))))
}

fn iter_next(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let state = match this {
        Value::Object(o) => match rt.obj(o).kind {
            ObjKind::RegExpStringIter { rx, s, global, unicode, done } => Some((o, rx, s, global, unicode, done)),
            _ => None,
        },
        _ => None,
    };
    let Some((o, rx, s, global, unicode, done)) = state else {
        return Err(rt.type_error("next method called on incompatible receiver"));
    };
    let finish = |rt: &mut Runtime| {
        if let ObjKind::RegExpStringIter { done, .. } = &mut rt.obj_mut(o).kind {
            *done = true;
        }
    };
    if done {
        return Ok(rt.iter_result(Value::Undefined, true));
    }
    let m = rt.regexp_exec(rx, s, true)?;
    if m == Value::Null {
        finish(rt);
        return Ok(rt.iter_result(Value::Undefined, true));
    }
    if !global {
        finish(rt);
        return Ok(rt.iter_result(m, false));
    }
    let m0 = rt.get_idx(m, 0)?;
    if rt.to_units(m0)?.is_empty() {
        let units = rt.str_rc(s);
        rt.bump_last_index(rx, &units, unicode)?;
    }
    Ok(rt.iter_result(m, false))
}

fn sym_search(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    this_regexp_object(rt, this, "Symbol.search")?;
    let s = rt.to_str(arg(a, 0))?;
    let previous = rt.get_prop(this, PropKey::Str(A_LAST_INDEX))?;
    if !rt.same_value(previous, Value::Number(0.0)) {
        rt.set_last_index(this, 0.0)?;
    }
    let r = rt.regexp_exec(this, s, true)?;
    let current = rt.get_prop(this, PropKey::Str(A_LAST_INDEX))?;
    if !rt.same_value(current, previous) && !rt.set_prop(this, PropKey::Str(A_LAST_INDEX), previous, this)? {
        return Err(rt.type_error("Cannot assign to read only property 'lastIndex' of object"));
    }
    if r == Value::Null {
        return Ok(Value::Number(-1.0));
    }
    rt.get_prop(r, PropKey::Str(A_INDEX))
}

/// Appends to the split result; true once the limit is reached.
fn push_part(rt: &mut Runtime, out: Value, n: &mut u64, v: Value, lim: u32) -> JsResult<bool> {
    set_index(rt, out, *n, v)?;
    *n += 1;
    Ok(*n >= lim as u64)
}

fn sym_split(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    this_regexp_object(rt, this, "Symbol.split")?;
    let s = rt.to_str(arg(a, 0))?;
    let units = rt.str_rc(s);
    let size = units.len();
    let c = rt.regexp_species(this)?;
    let flags = rt.regexp_flags_text(this)?;
    let unicode = flags.contains('u') || flags.contains('v');
    let default_ctor = c.as_obj().is_some() && c.as_obj() == rt.intr.regexp_ctor;
    let fast = if default_ctor { rt.regexp_pristine(this)? } else { None };
    // the spec probes every index with a sticky copy; an unmodified regex searches instead
    let (splitter, twin) = match fast {
        Some(prog) => {
            let f = prog.regex.flags();
            let twin = if f.sticky { rt.regex_compile(prog.source.clone(), Flags { sticky: false, ..f })? } else { prog };
            (Value::Undefined, Some(twin))
        }
        None => {
            let nf = rt.new_str(&if flags.contains('y') { flags.clone() } else { format!("{}y", flags) });
            let sp = rt.construct_value(c, &[this, nf], c)?;
            rt.scratch.push(sp);
            (sp, None)
        }
    };
    let out = Value::Object(rt.new_array(Vec::new()));
    let lim = match arg(a, 1) {
        Value::Undefined => u32::MAX,
        v => crate::ops::to_uint32(rt.to_num(v)?),
    };
    if lim == 0 {
        return Ok(out);
    }
    let mut n = 0u64;
    if size == 0 {
        let hit = match &twin {
            Some(t) => rt.regex_run(&t.regex, &units, 0)?.is_some(),
            None => rt.regexp_exec(splitter, s, true)? != Value::Null,
        };
        if !hit {
            push_part(rt, out, &mut n, Value::String(s), lim)?;
        }
        return Ok(out);
    }
    let mark = rt.scratch.len();
    let (mut p, mut q) = (0usize, 0usize);
    while q < size {
        rt.scratch.truncate(mark);
        rt.safepoint()?;
        // (start of the match at or after q, its end, captures 1..)
        let (at, e, caps): (usize, usize, Vec<Value>) = match &twin {
            Some(t) => match rt.regex_run(&t.regex, &units, q)? {
                None => break,
                Some(c) => {
                    let (ms, me) = c.get(0).unwrap_or((size, size));
                    if ms >= size {
                        break;
                    }
                    let caps = (1..c.len())
                        .map(|i| match c.get(i).and_then(|(x, y)| units.get(x..y)) {
                            Some(u) => rt.new_str16(u.to_vec()),
                            None => Value::Undefined,
                        })
                        .collect();
                    (ms, me.min(size), caps)
                }
            },
            None => {
                rt.set_last_index(splitter, q as f64)?;
                let z = rt.regexp_exec(splitter, s, true)?;
                if z == Value::Null {
                    q = advance_index(&units, q, unicode);
                    continue;
                }
                let e = rt.last_index(splitter)?.min(size as f64) as usize;
                let ncap = rt.length_of(z)?.saturating_sub(1).min(1 << 16);
                let mut caps = Vec::with_capacity(ncap as usize);
                for i in 1..=ncap {
                    caps.push(rt.get_idx(z, i)?);
                }
                (q, e, caps)
            }
        };
        if e == p {
            q = advance_index(&units, at, unicode);
            continue;
        }
        let head = rt.new_str16(units.get(p..at).unwrap_or(&[]).to_vec());
        if push_part(rt, out, &mut n, head, lim)? {
            return Ok(out);
        }
        p = e;
        for cv in caps {
            if push_part(rt, out, &mut n, cv, lim)? {
                return Ok(out);
            }
        }
        q = p;
    }
    let tail = rt.new_str16(units.get(p..).unwrap_or(&[]).to_vec());
    push_part(rt, out, &mut n, tail, lim)?;
    Ok(out)
}

pub(super) fn setup(rt: &mut Runtime, rp: ObjId) {
    rt.method_sym(rp, SYM_MATCH, "[Symbol.match]", 1, sym_match);
    rt.method_sym(rp, SYM_MATCH_ALL, "[Symbol.matchAll]", 1, sym_match_all);
    rt.method_sym(rp, SYM_SEARCH, "[Symbol.search]", 1, sym_search);
    rt.method_sym(rp, SYM_SPLIT, "[Symbol.split]", 2, sym_split);
    let ip = rt.new_object(rt.intr.iterator_proto);
    rt.intr.regexp_iter_proto = Some(ip);
    rt.method(ip, "next", 0, iter_next);
    let tag = rt.new_str("RegExp String Iterator");
    rt.define_value(ip, PropKey::Sym(SYM_TO_STRING_TAG), tag, CONFIGURABLE);
    super::regexp_replace::setup(rt, rp);
}
