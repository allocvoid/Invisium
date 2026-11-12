//! RegExp objects on top of `invisium_regex`: the compile cache, the constructor, `exec` /
//! `test` and the accessors. The `Symbol.*` methods live in `regexp_sym.rs`.

use super::arg;
use crate::runtime::*;
use crate::value::*;
use invisium_regex::{Captures, ExecError, Flags, Regex};
use std::rc::Rc;

fn flag_bits(f: Flags) -> u8 {
    [f.has_indices, f.global, f.ignore_case, f.multiline, f.dot_all, f.unicode, f.unicode_sets, f.sticky]
        .iter()
        .enumerate()
        .fold(0, |acc, (i, on)| acc | ((*on as u8) << i))
}

/// ToLength of an already numeric value.
pub(crate) fn to_length(n: f64) -> f64 {
    let n = crate::ops::to_integer(n);
    if n <= 0.0 { 0.0 } else { n.min(9007199254740991.0) }
}

/// AdvanceStringIndex: one code unit, or a whole code point in unicode mode.
pub(crate) fn advance_index(s: &[u16], i: usize, unicode: bool) -> usize {
    if unicode && i + 1 < s.len() && (0xD800..0xDC00).contains(&s[i]) && (0xDC00..0xE000).contains(&s[i + 1]) {
        return i + 2;
    }
    i + 1
}

/// EscapeRegExpPattern: the text `source` reports (a `/` or line break never ends the literal).
fn escape_source(src: &[u16]) -> Vec<u16> {
    if src.is_empty() {
        return "(?:)".encode_utf16().collect();
    }
    let mut out = Vec::with_capacity(src.len());
    let mut in_class = false;
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        let esc: Option<&str> = match c {
            0x0A => Some("\\n"),
            0x0D => Some("\\r"),
            0x2028 => Some("\\u2028"),
            0x2029 => Some("\\u2029"),
            0x2F if !in_class => Some("\\/"),
            _ => None,
        };
        if let Some(e) = esc {
            out.extend(e.encode_utf16());
        } else if c == b'\\' as u16 && i + 1 < src.len() && !matches!(src[i + 1], 0x0A | 0x0D | 0x2028 | 0x2029) {
            out.extend_from_slice(&src[i..i + 2]);
            i += 1;
        } else {
            in_class = match c {
                0x5B => true,
                0x5D => false,
                _ => in_class,
            };
            out.push(c);
        }
        i += 1;
    }
    out
}

impl Runtime {
    /// Compiles a pattern, or fetches it from the per-runtime cache.
    pub(crate) fn regex_compile(&mut self, source: Rc<[u16]>, flags: Flags) -> JsResult<Rc<RegexProg>> {
        let key = (source, flag_bits(flags));
        if let Some(p) = self.regex_cache.get(&key) {
            return Ok(p.clone());
        }
        let source = key.0.clone();
        self.check_alloc(source.len() * 2 + 256).map_err(|t| self.thrown_to_exc(t))?;
        match Regex::new(&source, flags) {
            Ok(regex) => {
                let bytes = regex.memory_bytes() + source.len() * 2 + 96;
                let p = Rc::new(RegexProg { regex, source, bytes });
                self.grow(bytes);
                self.regex_cache.insert(key, p.clone());
                Ok(p)
            }
            Err(e) => {
                let shown: String = String::from_utf16_lossy(&source).chars().take(120).collect();
                let msg = format!("Invalid regular expression: /{}/{}: {}", shown, flags.to_string(), e.message);
                Err(Exc::Throw(self.make_error("SyntaxError", &msg)))
            }
        }
    }

    pub(crate) fn regexp_alloc(&mut self, prog: Rc<RegexProg>, proto: Option<ObjId>) -> Value {
        let o = self.alloc(Object::new(proto, ObjKind::RegExp(prog)));
        self.define_value(o, PropKey::Str(A_LAST_INDEX), Value::Number(0.0), WRITABLE);
        Value::Object(o)
    }

    pub(crate) fn parse_flags(&mut self, flags: &[u16]) -> JsResult<Flags> {
        let text = String::from_utf16_lossy(flags);
        match Flags::parse(&text) {
            Some(f) => Ok(f),
            None => {
                let shown: String = text.chars().take(40).collect();
                Err(Exc::Throw(self.make_error("SyntaxError", &format!("Invalid flags supplied to RegExp constructor '{}'", shown))))
            }
        }
    }

    /// Evaluates a regex literal: a new object every time, compiled once.
    pub(crate) fn regexp_literal(&mut self, source: Atom, flags: Atom) -> JsResult<Value> {
        let f = self.atom_units(flags).to_vec();
        let f = self.parse_flags(&f)?;
        let src = self.atoms.get(source.0 as usize).cloned().unwrap_or_else(|| Rc::from(Vec::new()));
        let prog = self.regex_compile(src, f)?;
        let proto = self.intr.regexp_proto;
        Ok(self.regexp_alloc(prog, proto))
    }

    /// RegExpCreate from already converted pattern and flag text.
    pub(crate) fn regexp_new(&mut self, source: Rc<[u16]>, flags: &[u16], proto: Option<ObjId>) -> JsResult<Value> {
        let f = self.parse_flags(flags)?;
        let prog = self.regex_compile(source, f)?;
        Ok(self.regexp_alloc(prog, proto))
    }

    pub(crate) fn regex_prog(&self, v: Value) -> Option<Rc<RegexProg>> {
        match v {
            Value::Object(o) => match &self.obj(o).kind {
                ObjKind::RegExp(p) => Some(p.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// IsRegExp: `Symbol.match` decides, a real RegExp is the default.
    pub(crate) fn is_regexp(&mut self, v: Value) -> JsResult<bool> {
        if !matches!(v, Value::Object(_)) {
            return Ok(false);
        }
        let m = self.get_prop(v, PropKey::Sym(SYM_MATCH))?;
        if m != Value::Undefined {
            return Ok(self.truthy(m));
        }
        Ok(self.regex_prog(v).is_some())
    }

    /// Runs the matcher, charging its work to the script's step budget.
    pub(crate) fn regex_run(&mut self, re: &Regex, text: &[u16], start: usize) -> JsResult<Option<Captures>> {
        let budget = self.limits.step_budget;
        self.steps += 1;
        if self.steps > budget {
            return Err(Exc::Abort(ErrorKind::StepBudget));
        }
        let mut left = budget - self.steps;
        let r = re.exec(text, start, &mut left);
        self.steps = budget - left.min(budget);
        match r {
            Ok(m) => Ok(m),
            Err(ExecError::StepLimit) => {
                self.steps = budget.saturating_add(1);
                Err(Exc::Abort(ErrorKind::StepBudget))
            }
            Err(ExecError::StackLimit) => Err(self.range_error("Regular expression is too complex: backtrack stack limit exceeded")),
        }
    }

    pub(crate) fn set_last_index(&mut self, r: Value, i: f64) -> JsResult<()> {
        if !self.set_prop(r, PropKey::Str(A_LAST_INDEX), Value::Number(i), r)? {
            return Err(self.type_error("Cannot assign to read only property 'lastIndex' of object"));
        }
        Ok(())
    }

    pub(crate) fn last_index(&mut self, r: Value) -> JsResult<f64> {
        let v = self.get_prop(r, PropKey::Str(A_LAST_INDEX))?;
        Ok(to_length(self.to_num(v)?))
    }

    /// RegExpBuiltinExec up to (not including) the result array; maintains `lastIndex`.
    pub(crate) fn regexp_match_raw(&mut self, r: Value, prog: &RegexProg, s: &[u16]) -> JsResult<Option<Captures>> {
        let f = prog.regex.flags();
        let tracks = f.global || f.sticky;
        let li = self.last_index(r)?;
        let start = if tracks { li } else { 0.0 };
        if start > s.len() as f64 {
            if tracks {
                self.set_last_index(r, 0.0)?;
            }
            return Ok(None);
        }
        let m = self.regex_run(&prog.regex, s, start as usize)?;
        if tracks {
            let end = m.as_ref().and_then(|c| c.get(0)).map(|x| x.1).unwrap_or(0);
            self.set_last_index(r, end as f64)?;
        }
        Ok(m)
    }

    /// The match array: captures, `index`, `input`, `groups` (and `indices` with the d flag).
    pub(crate) fn regexp_result(&mut self, prog: &RegexProg, s: StrId, units: &[u16], c: &Captures) -> Value {
        let mut items = Vec::with_capacity(c.len());
        let mut spans = Vec::with_capacity(c.len());
        for i in 0..c.len() {
            let span = c.get(i).filter(|(a, b)| a <= b && *b <= units.len());
            spans.push(span);
            items.push(match span {
                Some((a, b)) => self.new_str16(units[a..b].to_vec()),
                None => Value::Undefined,
            });
        }
        let arr = self.new_array(items.iter().map(|v| Some(*v)).collect());
        let index = spans.first().copied().flatten().map(|x| x.0).unwrap_or(0);
        self.define_value(arr, PropKey::Str(A_INDEX), Value::Number(index as f64), DEFAULT_FLAGS);
        self.define_value(arr, PropKey::Str(A_INPUT), Value::String(s), DEFAULT_FLAGS);
        let names = prog.regex.group_names();
        let groups = self.named_groups(names, &items);
        self.define_value(arr, PropKey::Str(A_GROUPS), groups, DEFAULT_FLAGS);
        if prog.regex.flags().has_indices {
            let pairs: Vec<Value> = spans
                .iter()
                .map(|sp| match sp {
                    Some((a, b)) => self.new_array_values(&[Value::Number(*a as f64), Value::Number(*b as f64)]),
                    None => Value::Undefined,
                })
                .collect();
            let ind = self.new_array(pairs.iter().map(|v| Some(*v)).collect());
            let g = self.named_groups(names, &pairs);
            self.define_value(ind, PropKey::Str(A_GROUPS), g, DEFAULT_FLAGS);
            self.define_value(arr, PropKey::Str(A_INDICES), Value::Object(ind), DEFAULT_FLAGS);
        }
        Value::Object(arr)
    }

    /// The `groups` object (null prototype), or undefined when the pattern has no named group.
    pub(crate) fn named_groups(&mut self, names: &[(String, usize)], values: &[Value]) -> Value {
        if names.is_empty() {
            return Value::Undefined;
        }
        let g = self.new_object(None);
        for (name, idx) in names {
            let k = self.key_str(name);
            let v = values.get(*idx).copied().unwrap_or(Value::Undefined);
            // duplicate names (in different alternatives): the one that took part wins
            if v != Value::Undefined || self.get_own_prop(g, k).is_none() {
                self.define_value(g, k, v, DEFAULT_FLAGS);
            }
        }
        Value::Object(g)
    }

    /// RegExpExec. With `build` false an unmodified regex skips the result array and a match
    /// is reported as `true`.
    pub(crate) fn regexp_exec(&mut self, r: Value, s: StrId, build: bool) -> JsResult<Value> {
        let exec = self.get_prop(r, PropKey::Str(A_EXEC))?;
        let builtin = exec.as_obj().is_some() && exec.as_obj() == self.intr.regexp_exec;
        if !builtin && self.is_callable_val(exec) {
            let res = self.call_value(exec, r, &[Value::String(s)])?;
            if !matches!(res, Value::Object(_) | Value::Null) {
                return Err(self.type_error("object null or function expected as the result of exec"));
            }
            self.scratch.push(res);
            return Ok(res);
        }
        let Some(prog) = self.regex_prog(r) else {
            return Err(self.type_error("RegExp.prototype.exec called on incompatible receiver"));
        };
        let units = self.str_rc(s);
        Ok(match self.regexp_match_raw(r, &prog, &units)? {
            None => Value::Null,
            Some(_) if !build => Value::Bool(true),
            Some(c) => self.regexp_result(&prog, s, &units, &c),
        })
    }
}

fn regexp_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let (pattern, flags) = (arg(a, 0), arg(a, 1));
    let pattern_is_regexp = rt.is_regexp(pattern)?;
    let nt = rt.native_new_target;
    if nt == Value::Undefined && pattern_is_regexp && flags == Value::Undefined {
        let c = rt.get_prop(pattern, PropKey::Str(A_CONSTRUCTOR))?;
        if c.as_obj().is_some() && c.as_obj() == rt.intr.regexp_ctor {
            return Ok(pattern);
        }
    }
    let (p, f) = match rt.regex_prog(pattern) {
        Some(prog) => {
            let f = if flags == Value::Undefined { rt.new_str(&prog.regex.flags().to_string()) } else { flags };
            (Value::String(rt.alloc_str_rc(prog.source.clone())), f)
        }
        None if pattern_is_regexp => {
            let p = rt.get_prop(pattern, PropKey::Str(A_SOURCE))?;
            let f = if flags == Value::Undefined { rt.get_prop(pattern, PropKey::Str(A_FLAGS))? } else { flags };
            (p, f)
        }
        None => (pattern, flags),
    };
    let source: Rc<[u16]> = if p == Value::Undefined { Rc::from(Vec::new()) } else { rt.to_units(p)? };
    let flag_text: Rc<[u16]> = if f == Value::Undefined { Rc::from(Vec::new()) } else { rt.to_units(f)? };
    let default = rt.intr.regexp_proto;
    let proto = if nt == Value::Undefined { default } else { rt.proto_from_ctor(nt, default)? };
    rt.regexp_new(source, &flag_text, proto)
}

fn exec(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let Some(prog) = rt.regex_prog(this) else {
        return Err(rt.type_error("RegExp.prototype.exec called on incompatible receiver"));
    };
    let s = rt.to_str(arg(a, 0))?;
    let units = rt.str_rc(s);
    Ok(match rt.regexp_match_raw(this, &prog, &units)? {
        Some(c) => rt.regexp_result(&prog, s, &units, &c),
        None => Value::Null,
    })
}

fn test(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    if !matches!(this, Value::Object(_)) {
        return Err(rt.type_error("RegExp.prototype.test called on incompatible receiver"));
    }
    let s = rt.to_str(arg(a, 0))?;
    Ok(Value::Bool(rt.regexp_exec(this, s, false)? != Value::Null))
}

fn source(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    match rt.regex_prog(this) {
        Some(p) => Ok(rt.new_str16(escape_source(&p.source))),
        None if this.as_obj().is_some() && this.as_obj() == rt.intr.regexp_proto => Ok(rt.new_str("(?:)")),
        None => Err(rt.type_error("RegExp.prototype.source getter called on incompatible receiver")),
    }
}

const FLAG_PROPS: [(&str, char); 8] =
    [("hasIndices", 'd'), ("global", 'g'), ("ignoreCase", 'i'), ("multiline", 'm'), ("dotAll", 's'), ("unicode", 'u'), ("unicodeSets", 'v'), ("sticky", 'y')];

fn flags(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    if let Some(p) = rt.regex_prog(this) {
        return Ok(rt.new_str(&p.regex.flags().to_string()));
    }
    if !matches!(this, Value::Object(_)) {
        return Err(rt.type_error("RegExp.prototype.flags getter called on incompatible receiver"));
    }
    let mut out = String::new();
    for (name, letter) in FLAG_PROPS {
        let k = rt.key_str(name);
        let v = rt.get_prop(this, k)?;
        if rt.truthy(v) {
            out.push(letter);
        }
    }
    Ok(rt.new_str(&out))
}

fn flag_getter(rt: &mut Runtime, this: Value, pick: fn(Flags) -> bool) -> JsResult<Value> {
    match rt.regex_prog(this) {
        Some(p) => Ok(Value::Bool(pick(p.regex.flags()))),
        None if this.as_obj().is_some() && this.as_obj() == rt.intr.regexp_proto => Ok(Value::Undefined),
        None => Err(rt.type_error("RegExp flag getter called on incompatible receiver")),
    }
}

fn to_string(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    if !matches!(this, Value::Object(_)) {
        return Err(rt.type_error("RegExp.prototype.toString called on incompatible receiver"));
    }
    let src = rt.get_prop(this, PropKey::Str(A_SOURCE))?;
    let mut out = vec![b'/' as u16];
    out.extend_from_slice(&rt.to_units(src)?);
    out.push(b'/' as u16);
    let fl = rt.get_prop(this, PropKey::Str(A_FLAGS))?;
    out.extend_from_slice(&rt.to_units(fl)?);
    Ok(rt.new_str16(out))
}

pub(super) fn setup(rt: &mut Runtime) {
    let rp = rt.new_object(rt.intr.object_proto);
    rt.intr.regexp_proto = Some(rp);
    let c = rt.constructor("RegExp", 2, regexp_ctor, rp);
    rt.intr.regexp_ctor = Some(c);
    let e = rt.method(rp, "exec", 1, exec);
    rt.intr.regexp_exec = Some(e);
    rt.method(rp, "test", 1, test);
    rt.method(rp, "toString", 0, to_string);
    rt.getter(rp, "source", source);
    rt.getter(rp, "flags", flags);
    rt.getter(rp, "hasIndices", |rt, t, _| flag_getter(rt, t, |f| f.has_indices));
    rt.getter(rp, "global", |rt, t, _| flag_getter(rt, t, |f| f.global));
    rt.getter(rp, "ignoreCase", |rt, t, _| flag_getter(rt, t, |f| f.ignore_case));
    rt.getter(rp, "multiline", |rt, t, _| flag_getter(rt, t, |f| f.multiline));
    rt.getter(rp, "dotAll", |rt, t, _| flag_getter(rt, t, |f| f.dot_all));
    rt.getter(rp, "unicode", |rt, t, _| flag_getter(rt, t, |f| f.unicode));
    rt.getter(rp, "unicodeSets", |rt, t, _| flag_getter(rt, t, |f| f.unicode_sets));
    rt.getter(rp, "sticky", |rt, t, _| flag_getter(rt, t, |f| f.sticky));
    super::regexp_sym::setup(rt, rp);
}
