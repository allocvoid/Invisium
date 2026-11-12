//! en-US number patterns for `Intl.NumberFormat`: rounding by the resolved digit options,
//! notation, sign display and the style's affixes, as `formatToParts` parts.

use super::decimal::{groups, Dec, RMode};
use super::number_data as data;

pub(super) type Part = (&'static str, String);

/// Resolved `Intl.NumberFormat` options (also the digit options of `PluralRules`).
#[derive(Clone)]
pub(crate) struct NfOpts {
    /// 0 decimal, 1 percent, 2 currency, 3 unit.
    pub style: u8,
    pub currency: String,
    /// 0 symbol, 1 narrowSymbol, 2 code, 3 name.
    pub currency_display: u8,
    pub accounting: bool,
    pub unit: String,
    /// 0 short, 1 narrow, 2 long.
    pub unit_display: u8,
    /// 0 standard, 1 scientific, 2 engineering, 3 compact.
    pub notation: u8,
    pub compact_long: bool,
    /// 0 off, 1 auto, 2 always, 3 min2.
    pub grouping: u8,
    pub min_int: u32,
    pub min_frac: u32,
    pub max_frac: u32,
    pub min_sig: u32,
    pub max_sig: u32,
    /// 0 fractionDigits, 1 significantDigits, 2 morePrecision, 3 lessPrecision.
    pub rounding_type: u8,
    /// What `resolvedOptions().roundingPriority` reports: 0 auto, 1 morePrecision, 2 lessPrecision.
    pub rounding_priority: u8,
    pub rounding_mode: RMode,
    pub rounding_increment: u32,
    pub strip_if_integer: bool,
    /// 0 auto, 1 never, 2 always, 3 exceptZero, 4 negative.
    pub sign: u8,
}

impl Default for NfOpts {
    fn default() -> Self {
        NfOpts {
            style: 0,
            currency: String::new(),
            currency_display: 0,
            accounting: false,
            unit: String::new(),
            unit_display: 0,
            notation: 0,
            compact_long: false,
            grouping: 1,
            min_int: 1,
            min_frac: 0,
            max_frac: 3,
            min_sig: 1,
            max_sig: 21,
            rounding_type: 0,
            rounding_priority: 0,
            rounding_mode: RMode::HalfExpand,
            rounding_increment: 1,
            strip_if_integer: false,
            sign: 0,
        }
    }
}

/// A value to format: a double, or a BigInt as (negative, magnitude).
pub(crate) enum NumIn {
    F(f64),
    Big(bool, Dec),
}

fn to_f64(d: &Dec) -> f64 {
    if d.is_zero() {
        return 0.0;
    }
    let digits: String = d.digits.iter().map(|x| char::from(b'0' + x)).collect();
    format!("0.{}e{}", digits, d.point).parse().unwrap_or(0.0)
}

/// Rounds by the digit options; returns the rounded magnitude and the fraction digits to show
/// at least.
fn round(o: &NfOpts, d: &Dec, neg: bool) -> (Dec, usize) {
    let mode = o.rounding_mode;
    let by_frac = |d: &Dec| -> (Dec, usize) {
        let mut r = d.round_at(-(o.max_frac as i32), neg, mode);
        if o.rounding_increment > 1 {
            // round to a multiple of `increment` units in the last fraction place
            let mut scaled = d.clone();
            scaled.shift(o.max_frac as i32);
            let q = to_f64(&scaled) / o.rounding_increment as f64;
            let k = to_f64(&Dec::from_f64(q).round_at(0, neg, mode));
            r = Dec::from_f64(k * o.rounding_increment as f64);
            r.shift(-(o.max_frac as i32));
        }
        (r, o.min_frac as usize)
    };
    let by_sig = |d: &Dec| -> (Dec, usize) {
        let r = d.round_sig(o.max_sig, neg, mode);
        let f = r.frac_for_sig(o.min_sig);
        (r, f)
    };
    let (r, mut min_frac) = match o.rounding_type {
        0 => by_frac(d),
        1 => by_sig(d),
        t => {
            let sig_mag = d.point as i64 - o.max_sig as i64;
            let frac_mag = -(o.max_frac as i64);
            let sig_more_precise = sig_mag <= frac_mag;
            if sig_more_precise == (t == 2) { by_sig(d) } else { by_frac(d) }
        }
    };
    if o.strip_if_integer && r.point >= r.digits.len() as i32 {
        min_frac = 0;
    }
    (r, min_frac)
}

/// The rounded integer and fraction digits of `x` (finite), as the format would show them.
pub(super) fn digits_of(o: &NfOpts, x: f64) -> (String, String) {
    let d = Dec::from_f64(x);
    let (r, min_frac) = round(o, &d, x < 0.0);
    r.split(o.min_int as usize, min_frac)
}

const COMPACT: [(&str, &str); 4] = [("K", "thousand"), ("M", "million"), ("B", "billion"), ("T", "trillion")];

/// Scales for the notation and rounds: (rounded magnitude, fraction digits to show, exponent).
fn scale(o: &NfOpts, d: &Dec, neg: bool) -> (Dec, usize, i32) {
    let mag = d.magnitude();
    let mut e = match o.notation {
        1 => mag,
        2 => mag.div_euclid(3) * 3,
        3 if mag >= 3 => (mag.div_euclid(3) * 3).min(12),
        _ => 0,
    };
    for _ in 0..2 {
        let mut s = d.clone();
        s.shift(-e);
        let (r, min_frac) = round(o, &s, neg);
        let rm = r.magnitude();
        let bump = match o.notation {
            1 if !r.is_zero() && rm >= 1 => 1,
            2 if !r.is_zero() && rm >= 3 => 3,
            3 if !r.is_zero() && rm >= 3 && e < 12 => 3,
            _ => 0,
        };
        if bump == 0 {
            return (r, min_frac, e);
        }
        e += bump;
    }
    let mut s = d.clone();
    s.shift(-e);
    let (r, min_frac) = round(o, &s, neg);
    (r, min_frac, e)
}

fn number_parts(o: &NfOpts, r: &Dec, min_frac: usize, out: &mut Vec<Part>) -> (String, String) {
    let (int, frac) = r.split(o.min_int as usize, min_frac);
    let group = match o.grouping {
        0 => false,
        3 => int.len() >= 5,
        _ => int.len() > 3,
    };
    if group {
        for (i, g) in groups(&int).into_iter().enumerate() {
            if i > 0 {
                out.push(("group", ",".into()));
            }
            out.push(("integer", g.to_string()));
        }
    } else {
        out.push(("integer", int.clone()));
    }
    if !frac.is_empty() {
        out.push(("decimal", ".".into()));
        out.push(("fraction", frac.clone()));
    }
    (int, frac)
}

/// The parts of a formatted number.
pub(super) fn parts(o: &NfOpts, x: NumIn) -> Vec<Part> {
    let nan = matches!(x, NumIn::F(f) if f.is_nan());
    let (neg, dec) = match x {
        NumIn::F(f) if f.is_nan() => (false, None),
        NumIn::F(f) => (f.is_sign_negative(), if f.is_finite() { Some(Dec::from_f64(f)) } else { None }),
        NumIn::Big(neg, d) => (neg, Some(d)),
    };
    let mut num = Vec::new();
    let mut plural = true;
    let mut zero = false;
    match dec {
        None => num.push(if nan { ("nan", "NaN".to_string()) } else { ("infinity", "∞".to_string()) }),
        Some(mut d) => {
            if o.style == 1 {
                d.shift(2);
            }
            let (r, min_frac, e) = scale(o, &d, neg);
            zero = r.is_zero();
            let (int, frac) = number_parts(o, &r, min_frac, &mut num);
            plural = !(int == "1" && frac.is_empty());
            if o.notation == 1 || o.notation == 2 {
                num.push(("exponentSeparator", "E".into()));
                if e < 0 {
                    num.push(("exponentMinusSign", "-".into()));
                }
                num.push(("exponentInteger", e.unsigned_abs().to_string()));
            } else if o.notation == 3 && e >= 3 {
                let (short, long) = COMPACT[(e / 3 - 1).clamp(0, 3) as usize];
                if o.compact_long {
                    num.push(("literal", " ".into()));
                    num.push(("compact", long.into()));
                } else {
                    num.push(("compact", short.into()));
                }
            }
        }
    }
    let sign = if nan {
        None
    } else {
        match o.sign {
            0 if neg => Some("-"),
            2 => Some(if neg { "-" } else { "+" }),
            3 if !zero => Some(if neg { "-" } else { "+" }),
            4 if neg && !zero => Some("-"),
            _ => None,
        }
    };
    let sign_part = |s: &str| -> Part { (if s == "-" { "minusSign" } else { "plusSign" }, s.to_string()) };
    let mut out = Vec::new();
    match o.style {
        2 if o.currency_display != 3 => {
            let sym = data::currency_text(&o.currency, o.currency_display, plural);
            let paren = o.accounting && sign == Some("-");
            if paren {
                out.push(("literal", "(".into()));
            } else if let Some(s) = sign {
                out.push(sign_part(s));
            }
            let spaced = sym.chars().last().is_some_and(|c| c.is_alphabetic());
            out.push(("currency", sym));
            if spaced {
                out.push(("literal", "\u{a0}".into()));
            }
            out.extend(num);
            if paren {
                out.push(("literal", ")".into()));
            }
        }
        _ => {
            if let Some(s) = sign {
                out.push(sign_part(s));
            }
            out.extend(num);
            match o.style {
                1 => out.push(("percentSign", "%".into())),
                2 => {
                    out.push(("literal", " ".into()));
                    out.push(("currency", data::currency_text(&o.currency, 3, plural)));
                }
                3 => {
                    let (text, attach) = data::unit_text(&o.unit, o.unit_display, plural);
                    if !attach {
                        out.push(("literal", " ".into()));
                    }
                    out.push(("unit", text));
                }
                _ => {}
            }
        }
    }
    out
}
