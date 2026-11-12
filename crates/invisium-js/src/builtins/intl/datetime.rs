//! `Intl.DateTimeFormat`: option resolution, construction and `resolvedOptions`. The time
//! zone is always UTC and the locale always en-US; a requested zone or locale is validated
//! and then ignored.

use super::datetime_fmt as fmt;
use super::*;
use crate::builtins::date_time::time_clip;

/// Resolved fields. Widths: 0 = absent; text fields 1 narrow, 2 short, 3 long; numeric fields
/// 1 numeric, 2 two-digit; `month` 1 numeric, 2 two-digit, 3 narrow, 4 short, 5 long.
#[derive(Clone, Default)]
pub(crate) struct DtOpts {
    pub weekday: u8,
    pub era: u8,
    pub year: u8,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// `fractionalSecondDigits`, 0..=3.
    pub frac: u8,
    /// 0 absent, then the index into `ZONE` plus one.
    pub tz_name: u8,
    /// Index into `CYCLES`.
    pub hour_cycle: u8,
    /// 0 absent, then the index into `STYLES` plus one.
    pub date_style: u8,
    pub time_style: u8,
}

const WIDTHS: [&str; 3] = ["narrow", "short", "long"];
const NUMERIC: [&str; 2] = ["numeric", "2-digit"];
const MONTH: [&str; 5] = ["numeric", "2-digit", "narrow", "short", "long"];
const ZONE: [&str; 6] = ["short", "long", "shortOffset", "longOffset", "shortGeneric", "longGeneric"];
const STYLES: [&str; 4] = ["full", "long", "medium", "short"];
const CYCLES: [&str; 4] = ["h11", "h12", "h23", "h24"];

/// A time-zone name is checked for shape only (there is no zone database, and every zone
/// formats as UTC): IANA-style names and numeric offsets pass, anything else is a RangeError.
fn zone_shape_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'+' | b'-' | b':'))
}

/// `required`: 0 any field, 1 a date field, 2 a time field. `defaults`: what is added when
/// no required field was asked for: 0 date, 1 time, 2 both.
pub(crate) fn resolve(rt: &mut Runtime, locales: Value, options: Value, required: u8, defaults: u8) -> JsResult<DtOpts> {
    let o = opt_obj(rt, options)?;
    check_locales(rt, locales, o)?;
    opt_string(rt, o, "calendar")?;
    opt_string(rt, o, "numberingSystem")?;
    let hour12 = opt_bool(rt, o, "hour12")?;
    let cycle = opt_enum(rt, o, "hourCycle", &CYCLES)?;
    if let Some(zone) = opt_string(rt, o, "timeZone")? {
        if !zone_shape_ok(&zone) {
            return Err(rt.range_error("Invalid time zone specified"));
        }
    }
    let width = |rt: &mut Runtime, name: &str, allowed: &[&str]| -> JsResult<u8> { Ok(opt_enum(rt, o, name, allowed)?.map(|i| i + 1).unwrap_or(0)) };
    let mut d = DtOpts { weekday: width(rt, "weekday", &WIDTHS)?, era: width(rt, "era", &WIDTHS)?, ..DtOpts::default() };
    d.year = width(rt, "year", &NUMERIC)?;
    d.month = width(rt, "month", &MONTH)?;
    d.day = width(rt, "day", &NUMERIC)?;
    // `dayPeriod` ("in the morning") is accepted; en-US output here always uses AM / PM
    let day_period = width(rt, "dayPeriod", &WIDTHS)?;
    d.hour = width(rt, "hour", &NUMERIC)?;
    d.minute = width(rt, "minute", &NUMERIC)?;
    d.second = width(rt, "second", &NUMERIC)?;
    d.frac = opt_int(rt, o, "fractionalSecondDigits", 1, 3)?.unwrap_or(0) as u8;
    d.tz_name = width(rt, "timeZoneName", &ZONE)?;
    opt_enum(rt, o, "formatMatcher", &["basic", "best fit"])?;
    d.date_style = width(rt, "dateStyle", &STYLES)?;
    d.time_style = width(rt, "timeStyle", &STYLES)?;
    let has_date = d.weekday | d.year | d.month | d.day != 0;
    let has_time = d.hour | d.minute | d.second | d.frac | day_period != 0;
    if d.date_style | d.time_style != 0 {
        if has_date || has_time || d.era | d.tz_name != 0 {
            return Err(rt.type_error("Can't set a date-time component option when dateStyle or timeStyle is used"));
        }
        if (required == 1 && d.time_style != 0) || (required == 2 && d.date_style != 0) {
            return Err(rt.type_error("Invalid option: dateStyle / timeStyle does not apply here"));
        }
        if d.date_style != 0 {
            (d.weekday, d.month, d.day, d.year) = match d.date_style {
                1 => (3, 5, 1, 1),
                2 => (0, 5, 1, 1),
                3 => (0, 4, 1, 1),
                _ => (0, 1, 1, 2),
            };
        }
        if d.time_style != 0 {
            (d.hour, d.minute) = (1, 2);
            d.second = if d.time_style == 4 { 0 } else { 2 };
            d.tz_name = match d.time_style {
                1 => 2,
                2 => 1,
                _ => 0,
            };
        }
    } else {
        let need = match required {
            1 => !has_date,
            2 => !has_time,
            _ => !has_date && !has_time,
        };
        if need && defaults != 1 {
            (d.year, d.month, d.day) = (1, 1, 1);
        }
        if need && defaults != 0 {
            (d.hour, d.minute, d.second) = (1, 1, 1);
        }
    }
    // en-US prefers the 12-hour clock; `hour12` overrides `hourCycle`
    d.hour_cycle = match (hour12, cycle) {
        (Some(true), _) => 1,
        (Some(false), _) => 2,
        (None, Some(c)) => c,
        (None, None) => 1,
    };
    Ok(d)
}

fn ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let d = resolve(rt, arg(a, 0), arg(a, 1), 0, 0)?;
    // callable without `new`, like the legacy constructor it is
    let proto = rt.intl_proto(P_DATETIME)?;
    Ok(Value::Object(rt.new_intl(proto, IntlData::DateTime(d))))
}

fn this_opts(rt: &mut Runtime, this: Value) -> JsResult<DtOpts> {
    match rt.intl_ref(this).map(|d| &d.data) {
        Some(IntlData::DateTime(d)) => Ok(d.clone()),
        _ => Err(rt.type_error("Intl.DateTimeFormat method called on incompatible receiver")),
    }
}

/// The time value to format: the injected clock for `undefined`, else ToNumber; an invalid
/// date is a RangeError.
fn time_arg(rt: &mut Runtime, v: Value) -> JsResult<f64> {
    let t = match v {
        Value::Undefined => rt.now_ms(),
        _ => time_clip(rt.to_num(v)?),
    };
    if t.is_nan() {
        return Err(rt.range_error("Invalid time value"));
    }
    Ok(t)
}

fn bound_format(rt: &mut Runtime, slots: &[Value], _: Value, a: &[Value]) -> JsResult<Value> {
    let d = this_opts(rt, crate::promise::slot(slots, 0))?;
    let t = time_arg(rt, arg(a, 0))?;
    Ok(rt.new_str(&fmt::format(&d, t)))
}

fn get_format(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    this_opts(rt, this)?;
    Ok(rt.intl_bound(this, "", 1, bound_format))
}

fn format_to_parts(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let d = this_opts(rt, this)?;
    let t = time_arg(rt, arg(a, 0))?;
    parts_array(rt, &fmt::parts(&d, t))
}

fn range_args(rt: &mut Runtime, a: &[Value]) -> JsResult<(f64, f64)> {
    if arg(a, 0) == Value::Undefined || arg(a, 1) == Value::Undefined {
        return Err(rt.type_error("startDate and endDate are required"));
    }
    Ok((time_arg(rt, arg(a, 0))?, time_arg(rt, arg(a, 1))?))
}

fn format_range(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let d = this_opts(rt, this)?;
    let (x, y) = range_args(rt, a)?;
    let text: String = fmt::range_parts(&d, x, y).into_iter().map(|(p, _)| p.1).collect();
    Ok(rt.new_str(&text))
}

fn format_range_to_parts(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let d = this_opts(rt, this)?;
    let (x, y) = range_args(rt, a)?;
    let mut items = Vec::new();
    for ((ty, value), source) in fmt::range_parts(&d, x, y) {
        let o = rt.plain_object();
        put_str(rt, o, "type", ty);
        put_str(rt, o, "value", &value);
        put_str(rt, o, "source", source);
        items.push(Value::Object(o));
    }
    Ok(rt.new_array_values(&items))
}

fn resolved_options(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_opts(rt, this)?;
    let o = rt.plain_object();
    put_str(rt, o, "locale", LOCALE);
    put_str(rt, o, "calendar", "gregory");
    put_str(rt, o, "numberingSystem", "latn");
    put_str(rt, o, "timeZone", "UTC");
    if d.hour != 0 {
        put_str(rt, o, "hourCycle", CYCLES[d.hour_cycle as usize % 4]);
        put(rt, o, "hour12", Value::Bool(d.hour_cycle < 2));
    }
    if d.date_style | d.time_style == 0 {
        // what the en-US pattern really renders: minutes and seconds are padded after a
        // larger field, and the 24-hour clock pads its hour
        let hour = if d.hour != 0 && d.hour_cycle >= 2 { 2 } else { d.hour };
        let minute = if d.minute != 0 && d.hour != 0 { 2 } else { d.minute };
        let second = if d.second != 0 && d.minute != 0 { 2 } else { d.second };
        let text: [(&str, u8, &[&str]); 9] = [
            ("weekday", d.weekday, &WIDTHS),
            ("era", d.era, &WIDTHS),
            ("year", d.year, &NUMERIC),
            ("month", d.month, &MONTH),
            ("day", d.day, &NUMERIC),
            ("hour", hour, &NUMERIC),
            ("minute", minute, &NUMERIC),
            ("second", second, &NUMERIC),
            ("timeZoneName", d.tz_name, &ZONE),
        ];
        for (name, value, names) in text {
            if name == "timeZoneName" && d.frac != 0 {
                put(rt, o, "fractionalSecondDigits", Value::Number(d.frac as f64));
            }
            if value != 0 {
                put_str(rt, o, name, names[(value as usize - 1) % names.len()]);
            }
        }
    }
    for (name, value) in [("dateStyle", d.date_style), ("timeStyle", d.time_style)] {
        if value != 0 {
            put_str(rt, o, name, STYLES[(value as usize - 1) % 4]);
        }
    }
    Ok(Value::Object(o))
}

/// `Date.prototype.toLocaleString` (kind 0), `toLocaleDateString` (1), `toLocaleTimeString` (2)
/// for a valid time value, honouring the caller's `options`.
pub(crate) fn date_to_locale(rt: &mut Runtime, t: f64, kind: u8, locales: Value, options: Value) -> JsResult<String> {
    let (required, defaults) = match kind {
        0 => (0, 2),
        1 => (1, 0),
        _ => (2, 1),
    };
    let d = resolve(rt, locales, options, required, defaults)?;
    Ok(fmt::format(&d, t))
}

pub(super) fn setup(rt: &mut Runtime, intl: ObjId) {
    let (_, p) = intl_class(rt, intl, P_DATETIME, "DateTimeFormat", 0, ctor);
    rt.getter(p, "format", get_format);
    rt.method(p, "formatToParts", 1, format_to_parts);
    rt.method(p, "formatRange", 2, format_range);
    rt.method(p, "formatRangeToParts", 2, format_range_to_parts);
    rt.method(p, "resolvedOptions", 0, resolved_options);
}
