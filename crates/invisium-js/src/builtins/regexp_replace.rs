//! `RegExp.prototype[Symbol.replace]`, GetSubstitution (`$1 $<name> $& $\` $' $$`), and the
//! `String.prototype` methods that hand over to a pattern's `Symbol.*` method.

use super::arg;
use super::regexp_sym::this_regexp_object;
use crate::runtime::*;
use crate::value::*;
use invisium_regex::Captures;
use std::rc::Rc;

/// One match, in the shape `Symbol.replace` consumes.
struct Found {
    position: usize,
    matched: Vec<u16>,
    caps: Vec<Option<Vec<u16>>>,
    /// The named-groups object, or undefined.
    groups: Value,
}

impl Found {
    fn cost(&self) -> usize {
        64 + self.matched.len() * 2 + self.caps.iter().map(|c| 24 + c.as_ref().map(|u| u.len() * 2).unwrap_or(0)).sum::<usize>()
    }
}

fn digit(u: Option<&u16>) -> Option<usize> {
    u.filter(|c| (0x30..=0x39).contains(*c)).map(|c| (*c - 0x30) as usize)
}

/// GetSubstitution: expands the replacement template for one match.
fn substitute(rt: &mut Runtime, m: &Found, s: &[u16], tpl: &[u16], out: &mut Vec<u16>) -> JsResult<()> {
    let ncap = m.caps.len();
    let position = m.position.min(s.len());
    let tail = (position + m.matched.len()).min(s.len());
    let mut i = 0;
    while i < tpl.len() {
        let c = tpl[i];
        if c != b'$' as u16 || i + 1 >= tpl.len() {
            out.push(c);
            i += 1;
            continue;
        }
        let n = tpl[i + 1];
        if n == b'$' as u16 {
            out.push(c);
            i += 2;
        } else if n == b'&' as u16 {
            out.extend_from_slice(&m.matched);
            i += 2;
        } else if n == b'`' as u16 {
            out.extend_from_slice(&s[..position]);
            i += 2;
        } else if n == b'\'' as u16 {
            out.extend_from_slice(&s[tail..]);
            i += 2;
        } else if let Some(d1) = digit(Some(&n)) {
            // two digits when they name an existing group, else one
            let two = digit(tpl.get(i + 2)).map(|d2| d1 * 10 + d2).filter(|v| (1..=ncap).contains(v));
            let (idx, used) = match two {
                Some(v) => (v, 3),
                None => (d1, 2),
            };
            if (1..=ncap).contains(&idx) {
                if let Some(Some(cap)) = m.caps.get(idx - 1) {
                    out.extend_from_slice(cap);
                }
                i += used;
            } else {
                out.push(c);
                i += 1;
            }
        } else if n == b'<' as u16 && m.groups != Value::Undefined {
            match tpl[i + 2..].iter().position(|u| *u == b'>' as u16) {
                None => {
                    out.push(c);
                    i += 1;
                }
                Some(rel) => {
                    let name = rt.intern16(&tpl[i + 2..i + 2 + rel]);
                    let v = rt.get_prop(m.groups, PropKey::Str(name))?;
                    if v != Value::Undefined {
                        out.extend_from_slice(&rt.to_units(v)?);
                    }
                    i += rel + 3;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    Ok(())
}

fn found_from_captures(rt: &mut Runtime, prog: &RegexProg, units: &[u16], c: &Captures) -> Found {
    let span = |i: usize| c.get(i).and_then(|(x, y)| units.get(x..y));
    let caps: Vec<Option<Vec<u16>>> = (1..c.len()).map(|i| span(i).map(|u| u.to_vec())).collect();
    let names = prog.regex.group_names();
    let groups = if names.is_empty() {
        Value::Undefined
    } else {
        let mut vals = vec![Value::Undefined];
        for cap in &caps {
            vals.push(match cap {
                Some(u) => rt.new_str16(u.clone()),
                None => Value::Undefined,
            });
        }
        rt.named_groups(names, &vals)
    };
    Found { position: c.get(0).map(|x| x.0).unwrap_or(0), matched: span(0).unwrap_or(&[]).to_vec(), caps, groups }
}

fn found_from_result(rt: &mut Runtime, r: Value, len: usize) -> JsResult<Found> {
    // (a result object from a user `exec` may claim any length: bound the captures read)
    let ncap = rt.length_of(r)?.saturating_sub(1).min(1 << 16);
    let m0 = rt.get_idx(r, 0)?;
    let matched = rt.to_units(m0)?.to_vec();
    let pv = rt.get_prop(r, PropKey::Str(A_INDEX))?;
    let position = crate::ops::to_integer(rt.to_num(pv)?).max(0.0).min(len as f64) as usize;
    let mut caps = Vec::with_capacity(ncap as usize);
    for i in 1..=ncap {
        caps.push(match rt.get_idx(r, i)? {
            Value::Undefined => None,
            v => Some(rt.to_units(v)?.to_vec()),
        });
    }
    let mut groups = rt.get_prop(r, PropKey::Str(A_GROUPS))?;
    if groups != Value::Undefined {
        groups = Value::Object(rt.to_object(groups)?);
        rt.scratch.push(groups);
    }
    Ok(Found { position, matched, caps, groups })
}

/// Target string, replacer function (or undefined) and replacement template of one call.
struct Job<'a> {
    s: StrId,
    units: &'a [u16],
    func: Value,
    tpl: Option<&'a [u16]>,
}

/// Appends the text before the match and its replacement.
fn emit(rt: &mut Runtime, m: &Found, job: &Job, next: &mut usize, out: &mut Vec<u16>) -> JsResult<()> {
    let units = job.units;
    let position = m.position.min(units.len());
    let mut piece: Vec<u16> = Vec::new();
    match job.tpl {
        Some(t) => substitute(rt, m, units, t, &mut piece)?,
        None => {
            let mut args = Vec::with_capacity(m.caps.len() + 4);
            args.push(rt.new_str16(m.matched.clone()));
            for c in &m.caps {
                args.push(match c {
                    Some(u) => rt.new_str16(u.clone()),
                    None => Value::Undefined,
                });
            }
            args.push(Value::Number(position as f64));
            args.push(Value::String(job.s));
            if m.groups != Value::Undefined {
                args.push(m.groups);
            }
            let r = rt.call_value(job.func, Value::Undefined, &args)?;
            piece = rt.to_units(r)?.to_vec();
        }
    }
    if position >= *next {
        out.extend_from_slice(&units[*next..position]);
        out.extend_from_slice(&piece);
        *next = (position + m.matched.len()).min(units.len());
    }
    if out.len() > (1 << 28) {
        return Err(rt.range_error("Invalid string length"));
    }
    // the result under construction is not a heap string yet: hold it to the cap all the same
    rt.check_alloc(out.len() * 2).map_err(|t| rt.thrown_to_exc(t))
}

fn sym_replace(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    this_regexp_object(rt, this, "Symbol.replace")?;
    let s = rt.to_str(arg(a, 0))?;
    let units = rt.str_rc(s);
    let rv = arg(a, 1);
    let functional = rt.is_callable_val(rv);
    let tpl = if functional { None } else { Some(rt.to_units(rv)?) };
    let job = Job { s, units: &units, func: rv, tpl: tpl.as_deref() };
    let flags = rt.regexp_flags_text(this)?;
    let (global, unicode) = (flags.contains('g'), flags.contains('u') || flags.contains('v'));
    if global {
        rt.set_last_index(this, 0.0)?;
    }
    let fast = rt.regexp_pristine(this)?;
    let mut out: Vec<u16> = Vec::new();
    let mut next = 0usize;
    // replacer functions run only after all matching is done, as the specification orders it
    let mut pending: Vec<Found> = Vec::new();
    let mut held = 0usize;
    let mark = rt.scratch.len();
    loop {
        let found = match &fast {
            Some(prog) => match rt.regexp_match_raw(this, prog, &units)? {
                None => break,
                Some(c) => found_from_captures(rt, prog, &units, &c),
            },
            None => {
                let r = rt.regexp_exec(this, s, true)?;
                if r == Value::Null {
                    break;
                }
                found_from_result(rt, r, units.len())?
            }
        };
        if global && found.matched.is_empty() {
            rt.bump_last_index(this, &units, unicode)?;
        }
        if functional {
            held += found.cost();
            pending.push(found);
            if pending.len() % 256 == 0 {
                rt.check_alloc(held).map_err(|t| rt.thrown_to_exc(t))?;
            }
        } else {
            emit(rt, &found, &job, &mut next, &mut out)?;
            rt.scratch.truncate(mark);
        }
        if !global {
            break;
        }
        rt.safepoint()?;
    }
    for m in &pending {
        emit(rt, m, &job, &mut next, &mut out)?;
    }
    out.extend_from_slice(&units[next.min(units.len())..]);
    if out.len() > (1 << 28) {
        return Err(rt.range_error("Invalid string length"));
    }
    Ok(rt.new_str16(out))
}

// ---- String.prototype side ----

fn require_this(rt: &mut Runtime, this: Value, name: &str) -> JsResult<()> {
    if this.is_nullish() {
        return Err(rt.type_error(&format!("String.prototype.{} called on null or undefined", name)));
    }
    Ok(())
}

/// GetMethod(v, sym) for an object pattern; primitives never carry these methods.
fn pattern_method(rt: &mut Runtime, v: Value, sym: SymId) -> JsResult<Option<Value>> {
    if !matches!(v, Value::Object(_)) {
        return Ok(None);
    }
    let m = rt.get_prop(v, PropKey::Sym(sym))?;
    if m.is_nullish() {
        return Ok(None);
    }
    Ok(Some(rt.callable_arg(m, "Symbol method of the pattern")?))
}

/// RegExpCreate(pattern, flags) followed by Invoke(rx, sym, [s]).
fn create_and_invoke(rt: &mut Runtime, pattern: Value, flags: &str, sym: SymId, s: StrId) -> JsResult<Value> {
    let source: Rc<[u16]> = if pattern == Value::Undefined { Rc::from(Vec::new()) } else { rt.to_units(pattern)? };
    let f: Vec<u16> = flags.encode_utf16().collect();
    let proto = rt.intr.regexp_proto;
    let rx = rt.regexp_new(source, &f, proto)?;
    let m = rt.get_prop(rx, PropKey::Sym(sym))?;
    rt.call_value(m, rx, &[Value::String(s)])
}

/// A regex handed to matchAll / replaceAll must be global.
fn require_global(rt: &mut Runtime, re: Value, name: &str) -> JsResult<()> {
    if rt.is_regexp(re)? {
        let f = rt.get_prop(re, PropKey::Str(A_FLAGS))?;
        if f.is_nullish() || !rt.to_rust_string(f)?.contains('g') {
            return Err(rt.type_error(&format!("String.prototype.{} called with a non-global RegExp argument", name)));
        }
    }
    Ok(())
}

fn str_match(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    require_this(rt, this, "match")?;
    let re = arg(a, 0);
    if let Some(m) = pattern_method(rt, re, SYM_MATCH)? {
        return rt.call_value(m, re, &[this]);
    }
    let s = rt.to_str(this)?;
    create_and_invoke(rt, re, "", SYM_MATCH, s)
}

fn str_match_all(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    require_this(rt, this, "matchAll")?;
    let re = arg(a, 0);
    require_global(rt, re, "matchAll")?;
    if let Some(m) = pattern_method(rt, re, SYM_MATCH_ALL)? {
        return rt.call_value(m, re, &[this]);
    }
    let s = rt.to_str(this)?;
    create_and_invoke(rt, re, "g", SYM_MATCH_ALL, s)
}

fn str_search(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    require_this(rt, this, "search")?;
    let re = arg(a, 0);
    if let Some(m) = pattern_method(rt, re, SYM_SEARCH)? {
        return rt.call_value(m, re, &[this]);
    }
    let s = rt.to_str(this)?;
    create_and_invoke(rt, re, "", SYM_SEARCH, s)
}

fn str_replace(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    require_this(rt, this, "replace")?;
    if let Some(m) = pattern_method(rt, arg(a, 0), SYM_REPLACE)? {
        return rt.call_value(m, arg(a, 0), &[this, arg(a, 1)]);
    }
    super::string_split::replace(rt, this, a)
}

fn str_replace_all(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    require_this(rt, this, "replaceAll")?;
    require_global(rt, arg(a, 0), "replaceAll")?;
    if let Some(m) = pattern_method(rt, arg(a, 0), SYM_REPLACE)? {
        return rt.call_value(m, arg(a, 0), &[this, arg(a, 1)]);
    }
    super::string_split::replace_all(rt, this, a)
}

fn str_split(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    require_this(rt, this, "split")?;
    if let Some(m) = pattern_method(rt, arg(a, 0), SYM_SPLIT)? {
        return rt.call_value(m, arg(a, 0), &[this, arg(a, 1)]);
    }
    super::string_split::split(rt, this, a)
}

pub(super) fn setup(rt: &mut Runtime, rp: ObjId) {
    rt.method_sym(rp, SYM_REPLACE, "[Symbol.replace]", 2, sym_replace);
    if let Some(sp) = rt.intr.string_proto {
        let methods: [(&str, u32, super::NFn); 6] =
            [("match", 1, str_match), ("matchAll", 1, str_match_all), ("search", 1, str_search), ("replace", 2, str_replace), ("replaceAll", 2, str_replace_all), ("split", 2, str_split)];
        for (n, ar, f) in methods {
            rt.method(sp, n, ar, f);
        }
    }
}
