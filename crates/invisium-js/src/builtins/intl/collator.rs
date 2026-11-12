//! `Intl.Collator` and `String.prototype.localeCompare`: a three-level comparison in the
//! spirit of the Unicode collation algorithm's root order, for the scripts the engine can
//! normalise. Primary: whitespace < punctuation and symbols < digits < Latin < Greek <
//! Cyrillic < everything else; secondary: accents (combining marks after NFD); tertiary: case.

use super::*;
use crate::builtins::normalize::{normalize, Form};
use std::cmp::Ordering;

#[derive(Clone)]
pub(crate) struct CollOpts {
    pub search: bool,
    /// 0 base, 1 accent, 2 case, 3 variant.
    pub sensitivity: u8,
    pub ignore_punct: bool,
    pub numeric: bool,
    /// 0 false, 1 upper, 2 lower.
    pub case_first: u8,
}

const SENS: [&str; 4] = ["base", "accent", "case", "variant"];
const CASE_FIRST: [&str; 3] = ["false", "upper", "lower"];

/// One collation element.
struct Elem {
    p: u32,
    s: u64,
    t: u8,
}

fn is_mark(c: u32) -> bool {
    matches!(c, 0x300..=0x36F | 0x483..=0x489 | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

fn is_punct(c: char) -> bool {
    c.is_whitespace() || (!c.is_alphanumeric() && (c as u32) < 0x3000) || matches!(c as u32, 0x3000..=0x303F | 0xFF01..=0xFF0F)
}

/// Letters with no decomposition that sort as a base letter plus an accent-level difference.
fn base_letter(c: char) -> Option<(char, u64)> {
    Some(match c {
        'ø' => ('o', 0x50),
        'đ' | 'ð' => ('d', 0x51),
        'ł' => ('l', 0x52),
        'ı' => ('i', 0x53),
        'ħ' => ('h', 0x54),
        'ŧ' => ('t', 0x55),
        _ => return None,
    })
}

fn elements(text: &[u16], o: &CollOpts) -> Vec<Elem> {
    let nfd = normalize(text, Form::Nfd);
    let chars: Vec<char> = char::decode_utf16(nfd.iter().copied()).map(|r| r.unwrap_or('\u{fffd}')).collect();
    let mut out: Vec<Elem> = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let cp = c as u32;
        i += 1;
        if is_mark(cp) {
            if let Some(last) = out.last_mut() {
                last.s = last.s.saturating_mul(128).saturating_add((cp & 0x7f) as u64 + 1);
            }
            continue;
        }
        if cp < 0x20 && !c.is_whitespace() || matches!(cp, 0x7f..=0x9f | 0x200b..=0x200f | 0xfeff | 0xad) {
            continue;
        }
        let upper = c.is_uppercase();
        let t = if o.case_first == 1 { u8::from(!upper) } else { u8::from(upper) };
        if c.is_ascii_digit() {
            if o.numeric {
                let start = i - 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let run: String = chars[start..i].iter().collect();
                let digits = run.trim_start_matches('0');
                out.push(Elem { p: 0x1_0800 + digits.len().min(0x7ff) as u32, s: 0, t: 0 });
                for d in digits.bytes() {
                    out.push(Elem { p: 0x1_0000 + (d - b'0') as u32, s: 0, t: 0 });
                }
            } else {
                out.push(Elem { p: 0x1_0000 + cp - '0' as u32, s: 0, t: 0 });
            }
            continue;
        }
        if is_punct(c) {
            if !o.ignore_punct {
                out.push(Elem { p: if c.is_whitespace() { 0x100 } else { 0x200 + cp.min(0xfdff) }, s: 0, t: 0 });
            }
            continue;
        }
        let lower = c.to_lowercase().next().unwrap_or(c);
        let expand: Option<&str> = match lower {
            'ß' => Some("ss"),
            'æ' => Some("ae"),
            'œ' => Some("oe"),
            'ĳ' => Some("ij"),
            _ => None,
        };
        if let Some(e) = expand {
            for b in e.chars() {
                out.push(Elem { p: 0x2_0000 + (b as u32 - 'a' as u32) * 4, s: 0, t: t + 2 });
            }
            continue;
        }
        let (base, s) = base_letter(lower).unwrap_or((lower, 0));
        let p = match base as u32 {
            b @ 0x61..=0x7a => 0x2_0000 + (b - 0x61) * 4,
            0xfe => 0x2_0000 + 26 * 4,
            0x3c2 => 0x3_0000 + 0x3c3,
            b @ 0x370..=0x3ff => 0x3_0000 + b,
            b @ 0x400..=0x4ff => 0x4_0000 + b,
            b => 0x5_0000 + b,
        };
        let t = if lower == 'ς' { t + 2 } else { t };
        out.push(Elem { p, s, t });
    }
    out
}

pub(super) fn compare(o: &CollOpts, a: &[u16], b: &[u16]) -> Ordering {
    let (ea, eb) = (elements(a, o), elements(b, o));
    let primary = ea.iter().map(|e| e.p).cmp(eb.iter().map(|e| e.p));
    if primary != Ordering::Equal {
        return primary;
    }
    if o.sensitivity == 1 || o.sensitivity == 3 {
        let s = ea.iter().map(|e| e.s).cmp(eb.iter().map(|e| e.s));
        if s != Ordering::Equal {
            return s;
        }
    }
    if o.sensitivity >= 2 {
        let t = ea.iter().map(|e| e.t).cmp(eb.iter().map(|e| e.t));
        if t != Ordering::Equal {
            return t;
        }
    }
    Ordering::Equal
}

pub(super) fn resolve(rt: &mut Runtime, locales: Value, options: Value) -> JsResult<CollOpts> {
    let o = opt_obj(rt, options)?;
    let list = locale_list(rt, locales)?;
    let search = opt_enum(rt, o, "usage", &["sort", "search"])? == Some(1);
    opt_enum(rt, o, "localeMatcher", &["lookup", "best fit"])?;
    opt_string(rt, o, "collation")?;
    let numeric = opt_bool(rt, o, "numeric")?;
    let case_first = opt_enum(rt, o, "caseFirst", &["upper", "lower", "false"])?.map(|i| [1, 2, 0][i as usize]);
    let sensitivity = opt_enum(rt, o, "sensitivity", &SENS)?.unwrap_or(3);
    let ignore_punct = opt_bool(rt, o, "ignorePunctuation")?.unwrap_or(false);
    // the `-u-kn` / `-u-kf` keywords of the first requested tag apply when no option says otherwise
    let tag = list.first().and_then(|t| tag::parse(t));
    let kw = |k: &str| tag.as_ref().and_then(|t| t.keyword(k).map(|s| s.to_string()));
    let numeric = numeric.unwrap_or_else(|| matches!(kw("kn").as_deref(), Some("") | Some("true")));
    let case_first = case_first.unwrap_or_else(|| match kw("kf").as_deref() {
        Some("upper") => 1,
        Some("lower") => 2,
        _ => 0,
    });
    Ok(CollOpts { search, sensitivity, ignore_punct, numeric, case_first })
}

fn ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let c = resolve(rt, arg(a, 0), arg(a, 1))?;
    let proto = rt.intl_proto(P_COLLATOR)?;
    Ok(Value::Object(rt.new_intl(proto, IntlData::Collator(Box::new(c)))))
}

fn this_opts(rt: &mut Runtime, this: Value) -> JsResult<CollOpts> {
    match rt.intl_ref(this).map(|d| &d.data) {
        Some(IntlData::Collator(c)) => Ok((**c).clone()),
        _ => Err(rt.type_error("Intl.Collator method called on incompatible receiver")),
    }
}

/// Compares two values as strings, charging steps for their length first.
pub(crate) fn compare_values(rt: &mut Runtime, c: &CollOpts, a: Value, b: Value) -> JsResult<Value> {
    let x = rt.to_units(a)?;
    let y = rt.to_units(b)?;
    rt.charge(1 + (x.len() + y.len()) as u64 / 4)?;
    Ok(Value::Number(match compare(c, &x, &y) {
        Ordering::Less => -1.0,
        Ordering::Equal => 0.0,
        Ordering::Greater => 1.0,
    }))
}

fn bound_compare(rt: &mut Runtime, slots: &[Value], _: Value, a: &[Value]) -> JsResult<Value> {
    let c = this_opts(rt, crate::promise::slot(slots, 0))?;
    compare_values(rt, &c, arg(a, 0), arg(a, 1))
}

fn get_compare(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    this_opts(rt, this)?;
    Ok(rt.intl_bound(this, "", 2, bound_compare))
}

fn resolved_options(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let c = this_opts(rt, this)?;
    let o = rt.plain_object();
    put_str(rt, o, "locale", LOCALE);
    put_str(rt, o, "usage", if c.search { "search" } else { "sort" });
    put_str(rt, o, "sensitivity", SENS[c.sensitivity as usize % 4]);
    put(rt, o, "ignorePunctuation", Value::Bool(c.ignore_punct));
    put_str(rt, o, "collation", "default");
    put(rt, o, "numeric", Value::Bool(c.numeric));
    put_str(rt, o, "caseFirst", CASE_FIRST[c.case_first as usize % 3]);
    Ok(Value::Object(o))
}

/// `String.prototype.localeCompare(that, locales, options)` for an already coerced `this`.
pub(crate) fn locale_compare(rt: &mut Runtime, this: &[u16], that: &[u16], locales: Value, options: Value) -> JsResult<Value> {
    let c = resolve(rt, locales, options)?;
    rt.charge(1 + (this.len() + that.len()) as u64 / 4)?;
    Ok(Value::Number(match compare(&c, this, that) {
        Ordering::Less => -1.0,
        Ordering::Equal => 0.0,
        Ordering::Greater => 1.0,
    }))
}

pub(super) fn setup(rt: &mut Runtime, intl: ObjId) {
    let (_, p) = intl_class(rt, intl, P_COLLATOR, "Collator", 0, ctor);
    rt.getter(p, "compare", get_compare);
    rt.method(p, "resolvedOptions", 0, resolved_options);
}
