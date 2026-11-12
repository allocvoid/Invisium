//! `Intl.NumberFormat`: option resolution (ECMA-402 SetNumberFormatDigitOptions and friends),
//! construction, `format` / `formatToParts` / `resolvedOptions`, and the `toLocaleString`
//! methods of Number and BigInt.

use super::decimal::{Dec, RMode, RMODES};
use super::number_data as data;
use super::number_fmt::{self as fmt, NfOpts, NumIn};
use super::*;

const STYLES: [&str; 4] = ["decimal", "percent", "currency", "unit"];
const CUR_DISPLAY: [&str; 4] = ["symbol", "narrowSymbol", "code", "name"];
const UNIT_DISPLAY: [&str; 3] = ["short", "narrow", "long"];
const NOTATIONS: [&str; 4] = ["standard", "scientific", "engineering", "compact"];
const SIGNS: [&str; 5] = ["auto", "never", "always", "exceptZero", "negative"];
const PRIORITIES: [&str; 3] = ["auto", "morePrecision", "lessPrecision"];
const GROUPINGS: [&str; 4] = ["false", "auto", "always", "min2"];
const INCREMENTS: [u32; 15] = [1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 2500, 5000];

/// SetNumberFormatDigitOptions with the style's fraction defaults.
pub(super) fn digit_options(rt: &mut Runtime, o: Option<ObjId>, d: &mut NfOpts, mnfd_default: u32, mut mxfd_default: u32, compact: bool) -> JsResult<()> {
    d.min_int = opt_int(rt, o, "minimumIntegerDigits", 1, 21)?.unwrap_or(1);
    let mnfd = opt_raw(rt, o, "minimumFractionDigits")?;
    let mxfd = opt_raw(rt, o, "maximumFractionDigits")?;
    let mnsd = opt_raw(rt, o, "minimumSignificantDigits")?;
    let mxsd = opt_raw(rt, o, "maximumSignificantDigits")?;
    let inc = opt_int(rt, o, "roundingIncrement", 1, 5000)?.unwrap_or(1);
    if !INCREMENTS.contains(&inc) {
        return Err(rt.range_error("roundingIncrement value is out of range"));
    }
    d.rounding_increment = inc;
    d.rounding_mode = RMode::from_index(opt_enum(rt, o, "roundingMode", &RMODES)?.unwrap_or(6));
    d.rounding_priority = opt_enum(rt, o, "roundingPriority", &PRIORITIES)?.unwrap_or(0);
    d.strip_if_integer = opt_enum(rt, o, "trailingZeroDisplay", &["auto", "stripIfInteger"])? == Some(1);
    if inc != 1 {
        mxfd_default = mnfd_default;
    }
    let has_sd = mnsd != Value::Undefined || mxsd != Value::Undefined;
    let has_fd = mnfd != Value::Undefined || mxfd != Value::Undefined;
    let (mut need_sd, mut need_fd) = (true, true);
    if d.rounding_priority == 0 {
        need_sd = has_sd;
        if need_sd || (!has_fd && compact) {
            need_fd = false;
        }
    }
    if need_sd {
        let lo = int_in_range(rt, mnsd, "minimumSignificantDigits", 1, 21)?.unwrap_or(1);
        let hi = int_in_range(rt, mxsd, "maximumSignificantDigits", lo, 21)?.unwrap_or(21);
        (d.min_sig, d.max_sig) = (lo, hi);
    }
    if need_fd {
        if has_fd {
            let lo = int_in_range(rt, mnfd, "minimumFractionDigits", 0, 100)?;
            let hi = int_in_range(rt, mxfd, "maximumFractionDigits", 0, 100)?;
            (d.min_frac, d.max_frac) = match (lo, hi) {
                (None, Some(h)) => (mnfd_default.min(h), h),
                (Some(l), None) => (l, mxfd_default.max(l)),
                (Some(l), Some(h)) if l > h => return Err(rt.range_error("maximumFractionDigits value is out of range")),
                (l, h) => (l.unwrap_or(mnfd_default), h.unwrap_or(mxfd_default)),
            };
        } else {
            (d.min_frac, d.max_frac) = (mnfd_default, mxfd_default);
        }
    }
    d.rounding_type = if !need_sd && !need_fd {
        (d.min_frac, d.max_frac, d.min_sig, d.max_sig) = (0, 0, 1, 2);
        d.rounding_priority = 1;
        2
    } else {
        match d.rounding_priority {
            0 => u8::from(has_sd),
            p => p + 1,
        }
    };
    if inc != 1 {
        if d.rounding_type != 0 {
            return Err(rt.type_error("roundingIncrement requires fraction-digit rounding"));
        }
        if d.max_frac != d.min_frac {
            return Err(rt.range_error("roundingIncrement requires equal minimum and maximum fraction digits"));
        }
    }
    Ok(())
}

pub(crate) fn resolve(rt: &mut Runtime, locales: Value, options: Value) -> JsResult<NfOpts> {
    let o = opt_obj(rt, options)?;
    check_locales(rt, locales, o)?;
    opt_string(rt, o, "numberingSystem")?;
    let mut d = NfOpts { style: opt_enum(rt, o, "style", &STYLES)?.unwrap_or(0), ..NfOpts::default() };
    if let Some(c) = opt_string(rt, o, "currency")? {
        if c.len() != 3 || !c.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(rt.range_error(&format!("Invalid currency code : {}", c)));
        }
        d.currency = c.to_ascii_uppercase();
    } else if d.style == 2 {
        return Err(rt.type_error("Currency code is required with currency style."));
    }
    d.currency_display = opt_enum(rt, o, "currencyDisplay", &CUR_DISPLAY)?.unwrap_or(0);
    d.accounting = opt_enum(rt, o, "currencySign", &["standard", "accounting"])? == Some(1);
    if let Some(u) = opt_string(rt, o, "unit")? {
        if !data::unit_ok(&u) {
            return Err(rt.range_error(&format!("Invalid unit argument for Intl.NumberFormat() '{}'", u)));
        }
        d.unit = u;
    } else if d.style == 3 {
        return Err(rt.type_error("Unit is required with unit style."));
    }
    d.unit_display = opt_enum(rt, o, "unitDisplay", &UNIT_DISPLAY)?.unwrap_or(0);
    d.notation = opt_enum(rt, o, "notation", &NOTATIONS)?.unwrap_or(0);
    let (mnfd, mxfd) = match (d.style, d.notation) {
        (2, 0) => {
            let c = data::currency_digits(&d.currency);
            (c, c)
        }
        (1, _) => (0, 0),
        _ => (0, 3),
    };
    let compact = d.notation == 3;
    digit_options(rt, o, &mut d, mnfd, mxfd, compact)?;
    d.compact_long = opt_enum(rt, o, "compactDisplay", &["short", "long"])? == Some(1);
    let default_grouping = if d.notation == 3 { 3 } else { 1 };
    d.grouping = match opt_raw(rt, o, "useGrouping")? {
        Value::Undefined => default_grouping,
        Value::Bool(true) => 2,
        v if !rt.truthy(v) => 0,
        v => {
            let s = rt.to_rust_string(v)?;
            match s.as_str() {
                "true" | "false" => default_grouping,
                _ => match GROUPINGS.iter().position(|g| *g == s) {
                    Some(i) if i > 0 => i as u8,
                    _ => return Err(rt.range_error("Value out of range for Intl option \"useGrouping\"")),
                },
            }
        }
    };
    d.sign = opt_enum(rt, o, "signDisplay", &SIGNS)?.unwrap_or(0);
    Ok(d)
}

/// ToIntlMathematicalValue: BigInts exactly, everything else through ToNumber.
fn num_arg(rt: &mut Runtime, v: Value) -> JsResult<NumIn> {
    let p = rt.to_primitive(v, Hint::Number)?;
    if let Value::BigInt(b) = p {
        let b = rt.big(b);
        let text = rt.big_to_string(&b, 10)?;
        rt.charge(text.len() as u64 / 8)?;
        return Ok(NumIn::Big(b.neg, Dec::from_int_digits(&text)));
    }
    Ok(NumIn::F(rt.to_num(p)?))
}

fn format_parts(rt: &mut Runtime, d: &NfOpts, v: Value) -> JsResult<Vec<fmt::Part>> {
    let x = num_arg(rt, v)?;
    let parts = fmt::parts(d, x);
    rt.charge(parts.iter().map(|p| p.1.len() as u64 / 16 + 1).sum())?;
    Ok(parts)
}

pub(crate) fn format_value(rt: &mut Runtime, d: &NfOpts, v: Value) -> JsResult<String> {
    Ok(format_parts(rt, d, v)?.into_iter().map(|p| p.1).collect())
}

fn ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let d = resolve(rt, arg(a, 0), arg(a, 1))?;
    let proto = rt.intl_proto(P_NUMBER)?;
    Ok(Value::Object(rt.new_intl(proto, IntlData::Number(Box::new(d)))))
}

fn this_opts(rt: &mut Runtime, this: Value) -> JsResult<NfOpts> {
    match rt.intl_ref(this).map(|d| &d.data) {
        Some(IntlData::Number(d)) => Ok((**d).clone()),
        _ => Err(rt.type_error("Intl.NumberFormat method called on incompatible receiver")),
    }
}

fn bound_format(rt: &mut Runtime, slots: &[Value], _: Value, a: &[Value]) -> JsResult<Value> {
    let d = this_opts(rt, crate::promise::slot(slots, 0))?;
    let s = format_value(rt, &d, arg(a, 0))?;
    Ok(rt.new_str(&s))
}

fn get_format(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    this_opts(rt, this)?;
    Ok(rt.intl_bound(this, "", 1, bound_format))
}

fn format_to_parts(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let d = this_opts(rt, this)?;
    let parts = format_parts(rt, &d, arg(a, 0))?;
    parts_array(rt, &parts)
}

/// The digit and rounding entries of `resolvedOptions` (shared with `PluralRules`).
pub(super) fn put_digits(rt: &mut Runtime, o: ObjId, d: &NfOpts) {
    put(rt, o, "minimumIntegerDigits", Value::Number(d.min_int as f64));
    if d.rounding_type != 1 {
        put(rt, o, "minimumFractionDigits", Value::Number(d.min_frac as f64));
        put(rt, o, "maximumFractionDigits", Value::Number(d.max_frac as f64));
    }
    if d.rounding_type != 0 {
        put(rt, o, "minimumSignificantDigits", Value::Number(d.min_sig as f64));
        put(rt, o, "maximumSignificantDigits", Value::Number(d.max_sig as f64));
    }
}

fn put_rounding(rt: &mut Runtime, o: ObjId, d: &NfOpts) {
    put(rt, o, "roundingIncrement", Value::Number(d.rounding_increment as f64));
    put_str(rt, o, "roundingMode", d.rounding_mode.name());
    put_str(rt, o, "roundingPriority", PRIORITIES[d.rounding_priority as usize % 3]);
    put_str(rt, o, "trailingZeroDisplay", if d.strip_if_integer { "stripIfInteger" } else { "auto" });
}

fn resolved_options(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_opts(rt, this)?;
    let o = rt.plain_object();
    put_str(rt, o, "locale", LOCALE);
    put_str(rt, o, "numberingSystem", "latn");
    put_str(rt, o, "style", STYLES[d.style as usize % 4]);
    if d.style == 2 {
        put_str(rt, o, "currency", &d.currency);
        put_str(rt, o, "currencyDisplay", CUR_DISPLAY[d.currency_display as usize % 4]);
        put_str(rt, o, "currencySign", if d.accounting { "accounting" } else { "standard" });
    }
    if d.style == 3 {
        put_str(rt, o, "unit", &d.unit);
        put_str(rt, o, "unitDisplay", UNIT_DISPLAY[d.unit_display as usize % 3]);
    }
    put_digits(rt, o, &d);
    match d.grouping {
        0 => put(rt, o, "useGrouping", Value::Bool(false)),
        g => put_str(rt, o, "useGrouping", GROUPINGS[g as usize % 4]),
    }
    put_str(rt, o, "notation", NOTATIONS[d.notation as usize % 4]);
    if d.notation == 3 {
        put_str(rt, o, "compactDisplay", if d.compact_long { "long" } else { "short" });
    }
    put_str(rt, o, "signDisplay", SIGNS[d.sign as usize % 5]);
    put_rounding(rt, o, &d);
    Ok(Value::Object(o))
}

/// `Number.prototype.toLocaleString` / `BigInt.prototype.toLocaleString` for a primitive.
pub(crate) fn to_locale(rt: &mut Runtime, x: Value, locales: Value, options: Value) -> JsResult<String> {
    let d = resolve(rt, locales, options)?;
    format_value(rt, &d, x)
}

pub(super) fn setup(rt: &mut Runtime, intl: ObjId) {
    let (_, p) = intl_class(rt, intl, P_NUMBER, "NumberFormat", 0, ctor);
    rt.getter(p, "format", get_format);
    rt.method(p, "formatToParts", 1, format_to_parts);
    rt.method(p, "resolvedOptions", 0, resolved_options);
}
