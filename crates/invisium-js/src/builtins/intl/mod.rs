//! `Intl` with one fixed locale. For privacy every user gets the same answers: the locale is
//! always `en-US` and the time zone always `UTC`, whatever a script asks for
//! (`resolvedOptions()` says so). Nothing in here reads the machine's locale, zone or clock.

mod collator;
mod datetime;
mod datetime_fmt;
mod decimal;
mod display;
mod list;
mod locale;
mod number;
mod number_data;
mod number_fmt;
mod plural;
mod reltime;
mod segmenter;
mod tag;

use super::{arg, NFn};
use crate::ops::Hint;
use crate::runtime::*;
use crate::value::*;

pub(crate) use datetime::{date_to_locale, DtOpts};
pub(crate) use collator::locale_compare;
pub(crate) use number::to_locale as number_to_locale;
use collator::CollOpts;
use number_fmt::NfOpts;
use plural::PluralOpts;

pub(crate) const LOCALE: &str = "en-US";

/// Indices into `Intrinsics::intl_protos`.
pub(crate) const P_DATETIME: usize = 0;
pub(crate) const P_NUMBER: usize = 1;
pub(crate) const P_PLURAL: usize = 2;
pub(crate) const P_COLLATOR: usize = 3;
pub(crate) const P_RELTIME: usize = 4;
pub(crate) const P_LIST: usize = 5;
pub(crate) const P_LOCALE: usize = 6;
pub(crate) const P_DISPLAY: usize = 7;
pub(crate) const P_SEGMENTER: usize = 8;
pub(crate) const P_SEGMENTS: usize = 9;

/// The resolved options behind an `Intl` object: plain data, fixed at construction.
pub(crate) enum IntlData {
    DateTime(DtOpts),
    Number(Box<NfOpts>),
    Plural(Box<PluralOpts>),
    Collator(Box<CollOpts>),
    RelTime(reltime::RelOpts),
    List(list::ListOpts),
    /// The canonical tag of an `Intl.Locale`.
    Locale(String),
    Display(display::DisplayOpts),
    /// A segmenter's granularity: 0 grapheme, 1 word, 2 sentence.
    Segmenter(u8),
    /// The result of `segment()`: granularity and the string.
    Segments(u8, StrId),
}

pub(crate) struct IntlObj {
    pub data: IntlData,
    /// The bound `format` / `compare` function, created on first access.
    pub bound: Value,
}

impl IntlObj {
    pub fn trace(&self, f: &mut dyn FnMut(Value)) {
        f(self.bound);
        if let IntlData::Segments(_, s) = self.data {
            f(Value::String(s));
        }
    }
}

impl Runtime {
    /// Charges `n` steps for native work proportional to a script-controlled size, before
    /// the work is done.
    pub(crate) fn charge(&mut self, n: u64) -> JsResult<()> {
        self.steps = self.steps.saturating_add(n);
        if self.steps > self.limits.step_budget {
            return Err(Exc::Abort(ErrorKind::StepBudget));
        }
        Ok(())
    }

    pub(crate) fn new_intl(&mut self, proto: Option<ObjId>, data: IntlData) -> ObjId {
        self.alloc(Object::new(proto, ObjKind::Intl(Box::new(IntlObj { data, bound: Value::Undefined }))))
    }

    pub(crate) fn intl_ref(&self, v: Value) -> Option<&IntlObj> {
        match v {
            Value::Object(o) => match &self.obj(o).kind {
                ObjKind::Intl(d) => Some(d),
                _ => None,
            },
            _ => None,
        }
    }

    /// The prototype for a new `Intl` object: `new.target.prototype`, else the built-in one.
    fn intl_proto(&mut self, idx: usize) -> JsResult<Option<ObjId>> {
        let default = self.intr.intl_protos[idx];
        let nt = self.native_new_target;
        self.proto_from_ctor(nt, default)
    }

    /// The cached function bound to an `Intl` object (`format`, `compare`), made on first use.
    fn intl_bound(&mut self, this: Value, name: &str, arity: u32, f: InternFn) -> Value {
        if let Some(d) = self.intl_ref(this) {
            if d.bound != Value::Undefined {
                return d.bound;
            }
        }
        let func = Value::Object(self.new_intern(name, arity, f, vec![this]));
        if let Value::Object(o) = this {
            if let ObjKind::Intl(d) = &mut self.obj_mut(o).kind {
                d.bound = func;
            }
        }
        func
    }
}

// ---- reading options ----

/// The options argument: `undefined` means none, anything else is converted to an object.
fn opt_obj(rt: &mut Runtime, v: Value) -> JsResult<Option<ObjId>> {
    match v {
        Value::Undefined => Ok(None),
        _ => Ok(Some(rt.to_object(v)?)),
    }
}

fn opt_raw(rt: &mut Runtime, o: Option<ObjId>, name: &str) -> JsResult<Value> {
    match o {
        None => Ok(Value::Undefined),
        Some(o) => {
            let k = rt.key_str(name);
            rt.get_prop(Value::Object(o), k)
        }
    }
}

fn opt_string(rt: &mut Runtime, o: Option<ObjId>, name: &str) -> JsResult<Option<String>> {
    match opt_raw(rt, o, name)? {
        Value::Undefined => Ok(None),
        v => Ok(Some(rt.to_rust_string(v)?)),
    }
}

/// A string option restricted to `allowed`: its index there, `None` when absent.
fn opt_enum(rt: &mut Runtime, o: Option<ObjId>, name: &str, allowed: &[&str]) -> JsResult<Option<u8>> {
    let Some(s) = opt_string(rt, o, name)? else { return Ok(None) };
    match allowed.iter().position(|a| *a == s) {
        Some(i) => Ok(Some(i as u8)),
        None => Err(rt.range_error(&format!("Value out of range for Intl option \"{}\"", name))),
    }
}

fn opt_bool(rt: &mut Runtime, o: Option<ObjId>, name: &str) -> JsResult<Option<bool>> {
    match opt_raw(rt, o, name)? {
        Value::Undefined => Ok(None),
        v => Ok(Some(rt.truthy(v))),
    }
}

/// An integer option in `min..=max`; anything else (NaN, infinities, absurd sizes) is a
/// RangeError, so no digit count from a script ever reaches a formatting loop unchecked.
fn opt_int(rt: &mut Runtime, o: Option<ObjId>, name: &str, min: u32, max: u32) -> JsResult<Option<u32>> {
    let v = opt_raw(rt, o, name)?;
    int_in_range(rt, v, name, min, max)
}

fn int_in_range(rt: &mut Runtime, v: Value, name: &str, min: u32, max: u32) -> JsResult<Option<u32>> {
    if v == Value::Undefined {
        return Ok(None);
    }
    let n = rt.to_num(v)?;
    if n.is_nan() || n < min as f64 || n > max as f64 {
        return Err(rt.range_error(&format!("{} value is out of range", name)));
    }
    Ok(Some(n.floor() as u32))
}

fn put(rt: &mut Runtime, o: ObjId, name: &str, v: Value) {
    let k = rt.key_str(name);
    rt.define_value(o, k, v, DEFAULT_FLAGS);
}

fn put_str(rt: &mut Runtime, o: ObjId, name: &str, s: &str) {
    let v = rt.new_str(s);
    put(rt, o, name, v);
}

/// `[{ type, value }, ...]` for the `formatToParts` family.
fn parts_array(rt: &mut Runtime, parts: &[(&'static str, String)]) -> JsResult<Value> {
    rt.charge(parts.len() as u64)?;
    let mut items = Vec::with_capacity(parts.len());
    for (ty, value) in parts {
        let o = rt.plain_object();
        put_str(rt, o, "type", ty);
        put_str(rt, o, "value", value);
        items.push(Value::Object(o));
    }
    Ok(rt.new_array_values(&items))
}

// ---- locales: validated, canonicalised, then ignored ----

/// CanonicalizeLocaleList: the canonical form of every requested tag, duplicates dropped.
/// A malformed tag is a RangeError and a non-string, non-object element a TypeError, as in
/// every engine; which tags were asked for never changes a formatting result.
fn locale_list(rt: &mut Runtime, locales: Value) -> JsResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let one = |rt: &mut Runtime, v: Value, out: &mut Vec<String>| -> JsResult<()> {
        if !matches!(v, Value::String(_) | Value::Object(_)) {
            return Err(rt.type_error("Locale must be a string or an object"));
        }
        // an `Intl.Locale` object converts to its tag through its own `toString`
        let text = rt.to_rust_string(v)?;
        rt.charge(1 + text.len() as u64 / 16)?;
        match tag::parse(&text) {
            Some(t) => {
                let c = t.canonical();
                if !out.contains(&c) {
                    out.push(c);
                }
                Ok(())
            }
            None => Err(rt.range_error("Incorrect locale information provided")),
        }
    };
    let is_locale = matches!(rt.intl_ref(locales).map(|d| &d.data), Some(IntlData::Locale(_)));
    match locales {
        Value::Undefined => {}
        Value::String(_) => one(rt, locales, &mut out)?,
        _ if is_locale => one(rt, locales, &mut out)?,
        _ => {
            let o = rt.to_object(locales)?;
            let n = rt.length_of(Value::Object(o))?;
            for i in 0..n {
                // every element costs a step: `{ length: 2 ** 32 }` ends on the step budget
                rt.charge(1)?;
                let k = rt.index_key(i);
                if rt.has_property(o, k)? {
                    let v = rt.get_prop(Value::Object(o), k)?;
                    one(rt, v, &mut out)?;
                }
                // more distinct tags than any page needs: stop collecting, keep validating
                if out.len() > 1024 {
                    out.truncate(1024);
                }
            }
        }
    }
    Ok(out)
}

/// Only `en` and `en-US` are supported (with any extensions).
fn is_supported(canonical: &str) -> bool {
    match tag::parse(canonical) {
        Some(t) => t.language == "en" && t.region.as_deref().map(|r| r == "US").unwrap_or(true) && t.script.as_deref().map(|s| s == "Latn").unwrap_or(true),
        None => false,
    }
}

fn strings_array(rt: &mut Runtime, items: &[String]) -> Value {
    let vals: Vec<Value> = items.iter().map(|s| rt.new_str(s)).collect();
    rt.new_array_values(&vals)
}

fn get_canonical_locales(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let list = locale_list(rt, arg(a, 0))?;
    Ok(strings_array(rt, &list))
}

fn supported_locales_of(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let list = locale_list(rt, arg(a, 0))?;
    let o = opt_obj(rt, arg(a, 1))?;
    opt_enum(rt, o, "localeMatcher", &["lookup", "best fit"])?;
    let kept: Vec<String> = list.into_iter().filter(|t| is_supported(t)).collect();
    Ok(strings_array(rt, &kept))
}

/// Validates the `locales` argument and the `localeMatcher` option every constructor takes.
fn check_locales(rt: &mut Runtime, locales: Value, options: Option<ObjId>) -> JsResult<()> {
    locale_list(rt, locales)?;
    opt_enum(rt, options, "localeMatcher", &["lookup", "best fit"])?;
    Ok(())
}

/// Creates `Intl.<name>` with its prototype (remembered in `intl_protos[idx]`).
fn intl_class(rt: &mut Runtime, intl: ObjId, idx: usize, name: &str, arity: u32, f: NFn) -> (ObjId, ObjId) {
    let op = rt.intr.object_proto;
    let proto = rt.new_object(op);
    rt.intr.intl_protos[idx] = Some(proto);
    let c = rt.new_native(name, arity, Runtime::wrap(f), true);
    rt.define_value(c, PropKey::Str(A_PROTOTYPE), Value::Object(proto), 0);
    rt.define_value(proto, PropKey::Str(A_CONSTRUCTOR), Value::Object(c), HIDDEN);
    let k = rt.key_str(name);
    rt.define_value(intl, k, Value::Object(c), HIDDEN);
    let t = rt.new_str(&format!("Intl.{}", name));
    rt.define_value(proto, PropKey::Sym(SYM_TO_STRING_TAG), t, CONFIGURABLE);
    rt.method(c, "supportedLocalesOf", 1, supported_locales_of);
    (c, proto)
}

pub(super) fn setup(rt: &mut Runtime) {
    let intl = rt.plain_object();
    rt.set_global("Intl", Value::Object(intl));
    let t = rt.new_str("Intl");
    rt.define_value(intl, PropKey::Sym(SYM_TO_STRING_TAG), t, CONFIGURABLE);
    rt.method(intl, "getCanonicalLocales", 1, get_canonical_locales);
    datetime::setup(rt, intl);
    number::setup(rt, intl);
    plural::setup(rt, intl);
    collator::setup(rt, intl);
    reltime::setup(rt, intl);
    list::setup(rt, intl);
    locale::setup(rt, intl);
    display::setup(rt, intl);
    segmenter::setup(rt, intl);
}
