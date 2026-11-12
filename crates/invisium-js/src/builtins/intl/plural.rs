//! `Intl.PluralRules`: the English cardinal (`one`, `other`) and ordinal (`one`, `two`,
//! `few`, `other`) rules, applied to the number as the digit options would show it.

use super::number::{digit_options, put_digits};
use super::number_fmt::{digits_of, NfOpts};
use super::*;

#[derive(Clone)]
pub(crate) struct PluralOpts {
    pub ordinal: bool,
    pub digits: NfOpts,
}

/// The English plural category of `x`.
pub(super) fn category(p: &PluralOpts, x: f64) -> &'static str {
    if !x.is_finite() {
        return "other";
    }
    let (int, frac) = digits_of(&p.digits, x.abs());
    if !p.ordinal {
        return if int == "1" && frac.is_empty() { "one" } else { "other" };
    }
    if !frac.is_empty() {
        return "other";
    }
    let tail: u32 = int[int.len().saturating_sub(2)..].parse().unwrap_or(0);
    match (tail % 10, tail) {
        (1, t) if t != 11 => "one",
        (2, t) if t != 12 => "two",
        (3, t) if t != 13 => "few",
        _ => "other",
    }
}

pub(super) fn resolve(rt: &mut Runtime, locales: Value, options: Value) -> JsResult<PluralOpts> {
    let o = opt_obj(rt, options)?;
    check_locales(rt, locales, o)?;
    let ordinal = opt_enum(rt, o, "type", &["cardinal", "ordinal"])? == Some(1);
    let mut digits = NfOpts::default();
    digit_options(rt, o, &mut digits, 0, 3, false)?;
    Ok(PluralOpts { ordinal, digits })
}

fn ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    if rt.native_new_target == Value::Undefined {
        return Err(rt.type_error("Constructor Intl.PluralRules requires 'new'"));
    }
    let p = resolve(rt, arg(a, 0), arg(a, 1))?;
    let proto = rt.intl_proto(P_PLURAL)?;
    Ok(Value::Object(rt.new_intl(proto, IntlData::Plural(Box::new(p)))))
}

fn this_opts(rt: &mut Runtime, this: Value) -> JsResult<PluralOpts> {
    match rt.intl_ref(this).map(|d| &d.data) {
        Some(IntlData::Plural(p)) => Ok((**p).clone()),
        _ => Err(rt.type_error("Intl.PluralRules method called on incompatible receiver")),
    }
}

fn select(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let p = this_opts(rt, this)?;
    let x = rt.to_num(arg(a, 0))?;
    Ok(rt.new_str(category(&p, x)))
}

fn select_range(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let p = this_opts(rt, this)?;
    if arg(a, 0) == Value::Undefined || arg(a, 1) == Value::Undefined {
        return Err(rt.type_error("start and end are required"));
    }
    let x = rt.to_num(arg(a, 0))?;
    let y = rt.to_num(arg(a, 1))?;
    if x.is_nan() || y.is_nan() {
        return Err(rt.range_error("Invalid number range"));
    }
    // English range rules: one+other -> other, other+one -> one, other+other -> other
    Ok(rt.new_str(category(&p, y)))
}

fn resolved_options(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let p = this_opts(rt, this)?;
    let o = rt.plain_object();
    put_str(rt, o, "locale", LOCALE);
    put_str(rt, o, "type", if p.ordinal { "ordinal" } else { "cardinal" });
    put_digits(rt, o, &p.digits);
    let cats: Vec<String> = if p.ordinal { vec!["one".into(), "two".into(), "few".into(), "other".into()] } else { vec!["one".into(), "other".into()] };
    let arr = strings_array(rt, &cats);
    put(rt, o, "pluralCategories", arr);
    put_str(rt, o, "roundingMode", p.digits.rounding_mode.name());
    Ok(Value::Object(o))
}

pub(super) fn setup(rt: &mut Runtime, intl: ObjId) {
    let (_, p) = intl_class(rt, intl, P_PLURAL, "PluralRules", 0, ctor);
    rt.method(p, "select", 1, select);
    rt.method(p, "selectRange", 2, select_range);
    rt.method(p, "resolvedOptions", 0, resolved_options);
}
