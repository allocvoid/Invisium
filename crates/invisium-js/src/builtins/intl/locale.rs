//! `Intl.Locale`: parsing a language tag (with option overrides) and the basic getters.
//! A Locale only describes a tag; formatting stays en-US whatever it says.

use super::tag::{self, Tag};
use super::*;

/// Likely subtags for the languages `maximize` / `minimize` know: (language, script, region).
const LIKELY: &[(&str, &str, &str)] = &[
    ("en", "Latn", "US"), ("fr", "Latn", "FR"), ("de", "Latn", "DE"), ("es", "Latn", "ES"), ("it", "Latn", "IT"),
    ("pt", "Latn", "BR"), ("nl", "Latn", "NL"), ("sv", "Latn", "SE"), ("pl", "Latn", "PL"), ("tr", "Latn", "TR"),
    ("ru", "Cyrl", "RU"), ("uk", "Cyrl", "UA"), ("el", "Grek", "GR"), ("ja", "Jpan", "JP"), ("ko", "Kore", "KR"),
    ("zh", "Hans", "CN"), ("ar", "Arab", "EG"), ("he", "Hebr", "IL"), ("hi", "Deva", "IN"), ("th", "Thai", "TH"),
];

fn this_tag(rt: &mut Runtime, this: Value) -> JsResult<Tag> {
    let text = match rt.intl_ref(this).map(|d| &d.data) {
        Some(IntlData::Locale(s)) => s.clone(),
        _ => return Err(rt.type_error("Intl.Locale method called on incompatible receiver")),
    };
    tag::parse(&text).ok_or_else(|| rt.type_error("Invalid Intl.Locale"))
}

/// A string option that must match `ok`, else a RangeError.
fn checked(rt: &mut Runtime, o: Option<ObjId>, name: &str, ok: fn(&str) -> bool) -> JsResult<Option<String>> {
    match opt_string(rt, o, name)? {
        Some(s) if ok(&s) => Ok(Some(s)),
        Some(_) => Err(rt.range_error(&format!("Incorrect locale information provided for option \"{}\"", name))),
        None => Ok(None),
    }
}

fn type_ok(s: &str) -> bool {
    !s.is_empty() && s.split('-').all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

fn ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    if rt.native_new_target == Value::Undefined {
        return Err(rt.type_error("Constructor Intl.Locale requires 'new'"));
    }
    let t = arg(a, 0);
    if !matches!(t, Value::String(_) | Value::Object(_)) {
        return Err(rt.type_error("First argument to Intl.Locale constructor can't be empty or missing"));
    }
    let text = rt.to_rust_string(t)?;
    let Some(mut tag) = tag::parse(&text) else { return Err(rt.range_error("Incorrect locale information provided")) };
    let o = opt_obj(rt, arg(a, 1))?;
    if let Some(l) = checked(rt, o, "language", |s| matches!(s.len(), 2 | 3 | 5..=8) && s.bytes().all(|b| b.is_ascii_alphabetic()))? {
        tag.language = l.to_ascii_lowercase();
    }
    if let Some(s) = checked(rt, o, "script", |s| s.len() == 4 && s.bytes().all(|b| b.is_ascii_alphabetic()))? {
        let lower = s.to_ascii_lowercase();
        tag.script = Some(format!("{}{}", lower[..1].to_ascii_uppercase(), &lower[1..]));
    }
    if let Some(r) = checked(rt, o, "region", |s| (s.len() == 2 && s.bytes().all(|b| b.is_ascii_alphabetic())) || (s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit())))? {
        tag.region = Some(r.to_ascii_uppercase());
    }
    for (name, key) in [("calendar", "ca"), ("collation", "co")] {
        if let Some(v) = checked(rt, o, name, type_ok)? {
            tag.set_keyword(key, &v.to_ascii_lowercase());
        }
    }
    if let Some(i) = opt_enum(rt, o, "hourCycle", &["h11", "h12", "h23", "h24"])? {
        tag.set_keyword("hc", ["h11", "h12", "h23", "h24"][i as usize]);
    }
    if let Some(i) = opt_enum(rt, o, "caseFirst", &["upper", "lower", "false"])? {
        tag.set_keyword("kf", ["upper", "lower", "false"][i as usize]);
    }
    if let Some(n) = opt_bool(rt, o, "numeric")? {
        tag.set_keyword("kn", if n { "" } else { "false" });
    }
    if let Some(v) = checked(rt, o, "numberingSystem", type_ok)? {
        tag.set_keyword("nu", &v.to_ascii_lowercase());
    }
    let proto = rt.intl_proto(P_LOCALE)?;
    Ok(Value::Object(rt.new_intl(proto, IntlData::Locale(tag.canonical()))))
}

fn str_or_undef(rt: &mut Runtime, s: Option<&str>) -> Value {
    match s {
        Some(s) => rt.new_str(s),
        None => Value::Undefined,
    }
}

fn to_string(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let t = this_tag(rt, this)?;
    Ok(rt.new_str(&t.canonical()))
}

fn base_name(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let t = this_tag(rt, this)?;
    Ok(rt.new_str(&t.base_name()))
}

fn language(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let t = this_tag(rt, this)?;
    Ok(rt.new_str(&t.language))
}

fn script(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let t = this_tag(rt, this)?;
    Ok(str_or_undef(rt, t.script.as_deref()))
}

fn region(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let t = this_tag(rt, this)?;
    Ok(str_or_undef(rt, t.region.as_deref()))
}

fn keyword_getter(rt: &mut Runtime, this: Value, key: &str) -> JsResult<Value> {
    let t = this_tag(rt, this)?;
    Ok(str_or_undef(rt, t.keyword(key)))
}

fn numeric(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let t = this_tag(rt, this)?;
    Ok(Value::Bool(matches!(t.keyword("kn"), Some("") | Some("true"))))
}

/// `maximize` (`grow`) adds the likely script and region; `minimize` removes them again.
fn likely(rt: &mut Runtime, this: Value, grow: bool) -> JsResult<Value> {
    let mut t = this_tag(rt, this)?;
    if let Some((_, s, r)) = LIKELY.iter().find(|l| l.0 == t.language) {
        if grow {
            t.script.get_or_insert_with(|| s.to_string());
            t.region.get_or_insert_with(|| r.to_string());
        } else {
            if t.script.as_deref() == Some(*s) {
                t.script = None;
            }
            if t.region.as_deref() == Some(*r) {
                t.region = None;
            }
        }
    }
    let proto = rt.intr.intl_protos[P_LOCALE];
    Ok(Value::Object(rt.new_intl(proto, IntlData::Locale(t.canonical()))))
}

pub(super) fn setup(rt: &mut Runtime, intl: ObjId) {
    let (_, p) = intl_class(rt, intl, P_LOCALE, "Locale", 1, ctor);
    rt.method(p, "toString", 0, to_string);
    rt.method(p, "maximize", 0, |rt, this, _| likely(rt, this, true));
    rt.method(p, "minimize", 0, |rt, this, _| likely(rt, this, false));
    rt.getter(p, "baseName", base_name);
    rt.getter(p, "language", language);
    rt.getter(p, "script", script);
    rt.getter(p, "region", region);
    rt.getter(p, "calendar", |rt, this, _| keyword_getter(rt, this, "ca"));
    rt.getter(p, "collation", |rt, this, _| keyword_getter(rt, this, "co"));
    rt.getter(p, "hourCycle", |rt, this, _| keyword_getter(rt, this, "hc"));
    rt.getter(p, "caseFirst", |rt, this, _| keyword_getter(rt, this, "kf"));
    rt.getter(p, "numberingSystem", |rt, this, _| keyword_getter(rt, this, "nu"));
    rt.getter(p, "numeric", numeric);
}
