//! `Intl.RelativeTimeFormat` with the English patterns ("in 3 days", "3 days ago",
//! "yesterday" with `numeric: 'auto'`).

use super::number_fmt::{self as fmt, NfOpts, NumIn};
use super::*;

#[derive(Clone)]
pub(crate) struct RelOpts {
    /// 0 long, 1 short, 2 narrow.
    pub style: u8,
    pub auto: bool,
}

const STYLES: [&str; 3] = ["long", "short", "narrow"];
/// (unit, long singular, long plural, short singular, short plural, auto -1, auto 0, auto +1).
type UnitNames = (&'static str, &'static str, &'static str, &'static str, &'static str, &'static str, &'static str, &'static str);
const UNITS: [UnitNames; 8] = [
    ("second", "second", "seconds", "sec.", "sec.", "", "now", ""),
    ("minute", "minute", "minutes", "min.", "min.", "", "this minute", ""),
    ("hour", "hour", "hours", "hr.", "hr.", "", "this hour", ""),
    ("day", "day", "days", "day", "days", "yesterday", "today", "tomorrow"),
    ("week", "week", "weeks", "wk.", "wk.", "last week", "this week", "next week"),
    ("month", "month", "months", "mo.", "mo.", "last month", "this month", "next month"),
    ("quarter", "quarter", "quarters", "qtr.", "qtrs.", "last quarter", "this quarter", "next quarter"),
    ("year", "year", "years", "yr.", "yr.", "last year", "this year", "next year"),
];

fn ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    if rt.native_new_target == Value::Undefined {
        return Err(rt.type_error("Constructor Intl.RelativeTimeFormat requires 'new'"));
    }
    let o = opt_obj(rt, arg(a, 1))?;
    check_locales(rt, arg(a, 0), o)?;
    opt_string(rt, o, "numberingSystem")?;
    let style = opt_enum(rt, o, "style", &STYLES)?.unwrap_or(0);
    let auto = opt_enum(rt, o, "numeric", &["always", "auto"])? == Some(1);
    let proto = rt.intl_proto(P_RELTIME)?;
    Ok(Value::Object(rt.new_intl(proto, IntlData::RelTime(RelOpts { style, auto }))))
}

fn this_opts(rt: &mut Runtime, this: Value) -> JsResult<RelOpts> {
    match rt.intl_ref(this).map(|d| &d.data) {
        Some(IntlData::RelTime(r)) => Ok(r.clone()),
        _ => Err(rt.type_error("Intl.RelativeTimeFormat method called on incompatible receiver")),
    }
}

/// The parts of `format(value, unit)`; a number part carries its unit name.
fn rel_parts(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<(Vec<(&'static str, String)>, &'static str)> {
    let r = this_opts(rt, this)?;
    let x = rt.to_num(arg(a, 0))?;
    let unit_text = rt.to_rust_string(arg(a, 1))?;
    if !x.is_finite() {
        return Err(rt.range_error("Invalid number value"));
    }
    let singular = unit_text.strip_suffix('s').unwrap_or(&unit_text);
    let Some(u) = UNITS.iter().find(|u| u.0 == singular) else {
        return Err(rt.range_error(&format!("Invalid unit argument for format() '{}'", unit_text)));
    };
    if r.auto && (x == -1.0 || x == 0.0 || x == 1.0) {
        let phrase = [u.5, u.6, u.7][(x + 1.0) as usize];
        if !phrase.is_empty() {
            let abbreviate = r.style != 0 && matches!(u.0, "week" | "month" | "quarter" | "year");
            let phrase = if abbreviate { phrase.replace(u.1, u.3) } else { phrase.to_string() };
            return Ok((vec![("literal", phrase)], u.0));
        }
    }
    let past = x < 0.0 || (x == 0.0 && x.is_sign_negative());
    let num = fmt::parts(&NfOpts::default(), NumIn::F(x.abs()));
    let shown: String = num.iter().map(|p| p.1.as_str()).collect();
    let plural = shown != "1";
    let name = match (r.style, plural) {
        (0, false) => u.1,
        (0, true) => u.2,
        (_, false) => u.3,
        (_, true) => u.4,
    };
    let mut out = Vec::new();
    if !past {
        out.push(("literal", "in ".to_string()));
    }
    out.extend(num);
    out.push(("literal", if past { format!(" {} ago", name) } else { format!(" {}", name) }));
    Ok((out, u.0))
}

fn format(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (parts, _) = rel_parts(rt, this, a)?;
    let s: String = parts.into_iter().map(|p| p.1).collect();
    Ok(rt.new_str(&s))
}

fn format_to_parts(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (parts, unit) = rel_parts(rt, this, a)?;
    let mut items = Vec::new();
    for (ty, value) in parts {
        let o = rt.plain_object();
        put_str(rt, o, "type", ty);
        put_str(rt, o, "value", &value);
        if ty != "literal" {
            put_str(rt, o, "unit", unit);
        }
        items.push(Value::Object(o));
    }
    Ok(rt.new_array_values(&items))
}

fn resolved_options(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = this_opts(rt, this)?;
    let o = rt.plain_object();
    put_str(rt, o, "locale", LOCALE);
    put_str(rt, o, "style", STYLES[r.style as usize % 3]);
    put_str(rt, o, "numeric", if r.auto { "auto" } else { "always" });
    put_str(rt, o, "numberingSystem", "latn");
    Ok(Value::Object(o))
}

pub(super) fn setup(rt: &mut Runtime, intl: ObjId) {
    let (_, p) = intl_class(rt, intl, P_RELTIME, "RelativeTimeFormat", 0, ctor);
    rt.method(p, "format", 2, format);
    rt.method(p, "formatToParts", 2, format_to_parts);
    rt.method(p, "resolvedOptions", 0, resolved_options);
}
