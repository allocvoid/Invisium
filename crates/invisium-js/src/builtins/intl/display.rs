//! `Intl.DisplayNames`: English names for the most common language, region and script codes,
//! the currencies `NumberFormat` knows, calendars and date-time fields; any other well-formed
//! code comes back as itself (or `undefined` with `fallback: 'none'`).

use super::*;

#[derive(Clone)]
pub(crate) struct DisplayOpts {
    /// Index into `TYPES`.
    pub kind: u8,
    pub style: u8,
    pub fallback_none: bool,
    pub standard: bool,
}

const TYPES: [&str; 6] = ["language", "region", "script", "currency", "calendar", "dateTimeField"];
const STYLES: [&str; 3] = ["long", "short", "narrow"];

const LANGUAGES: &[(&str, &str)] = &[
    ("ar", "Arabic"), ("bn", "Bangla"), ("cs", "Czech"), ("da", "Danish"), ("de", "German"), ("el", "Greek"),
    ("en", "English"), ("es", "Spanish"), ("fa", "Persian"), ("fi", "Finnish"), ("fr", "French"), ("he", "Hebrew"),
    ("hi", "Hindi"), ("hu", "Hungarian"), ("id", "Indonesian"), ("it", "Italian"), ("ja", "Japanese"), ("ko", "Korean"),
    ("ms", "Malay"), ("nb", "Norwegian Bokmål"), ("nl", "Dutch"), ("no", "Norwegian"), ("pl", "Polish"), ("pt", "Portuguese"),
    ("ro", "Romanian"), ("ru", "Russian"), ("sv", "Swedish"), ("ta", "Tamil"), ("th", "Thai"), ("tr", "Turkish"),
    ("uk", "Ukrainian"), ("ur", "Urdu"), ("vi", "Vietnamese"), ("zh", "Chinese"),
];

/// Dialect names used with `languageDisplay: 'dialect'` (the default).
const DIALECTS: &[(&str, &str)] = &[
    ("en-AU", "Australian English"), ("en-CA", "Canadian English"), ("en-GB", "British English"), ("en-US", "American English"),
    ("es-419", "Latin American Spanish"), ("es-ES", "European Spanish"), ("es-MX", "Mexican Spanish"), ("fr-CA", "Canadian French"),
    ("fr-CH", "Swiss French"), ("pt-BR", "Brazilian Portuguese"), ("pt-PT", "European Portuguese"), ("zh-Hans", "Simplified Chinese"),
    ("zh-Hant", "Traditional Chinese"), ("de-AT", "Austrian German"), ("de-CH", "Swiss High German"),
];

const REGIONS: &[(&str, &str)] = &[
    ("001", "world"), ("419", "Latin America"), ("AR", "Argentina"), ("AT", "Austria"), ("AU", "Australia"), ("BE", "Belgium"),
    ("BR", "Brazil"), ("CA", "Canada"), ("CH", "Switzerland"), ("CN", "China"), ("DE", "Germany"), ("DK", "Denmark"),
    ("EG", "Egypt"), ("ES", "Spain"), ("FI", "Finland"), ("FR", "France"), ("GB", "United Kingdom"), ("GR", "Greece"),
    ("HK", "Hong Kong SAR China"), ("ID", "Indonesia"), ("IE", "Ireland"), ("IL", "Israel"), ("IN", "India"), ("IT", "Italy"),
    ("JP", "Japan"), ("KR", "South Korea"), ("MX", "Mexico"), ("NG", "Nigeria"), ("NL", "Netherlands"), ("NO", "Norway"),
    ("NZ", "New Zealand"), ("PL", "Poland"), ("PT", "Portugal"), ("RU", "Russia"), ("SA", "Saudi Arabia"), ("SE", "Sweden"),
    ("SG", "Singapore"), ("TR", "Türkiye"), ("TW", "Taiwan"), ("UA", "Ukraine"), ("US", "United States"), ("ZA", "South Africa"),
];

/// Short forms where English has one.
const SHORT_REGIONS: &[(&str, &str)] = &[("GB", "UK"), ("US", "US"), ("HK", "Hong Kong")];

const SCRIPTS: &[(&str, &str)] = &[
    ("Arab", "Arabic"), ("Cyrl", "Cyrillic"), ("Deva", "Devanagari"), ("Grek", "Greek"), ("Hans", "Simplified Han"),
    ("Hant", "Traditional Han"), ("Hebr", "Hebrew"), ("Jpan", "Japanese"), ("Kore", "Korean"), ("Latn", "Latin"), ("Thai", "Thai"),
];

const CALENDARS: &[(&str, &str)] = &[
    ("buddhist", "Buddhist Calendar"), ("chinese", "Chinese Calendar"), ("gregory", "Gregorian Calendar"),
    ("hebrew", "Hebrew Calendar"), ("islamic", "Hijri Calendar"), ("iso8601", "ISO-8601 Calendar"), ("japanese", "Japanese Calendar"),
];

const FIELDS: &[(&str, &str)] = &[
    ("era", "era"), ("year", "year"), ("quarter", "quarter"), ("month", "month"), ("weekOfYear", "week"),
    ("weekday", "day of the week"), ("day", "day"), ("dayPeriod", "AM/PM"), ("hour", "hour"), ("minute", "minute"),
    ("second", "second"), ("timeZoneName", "time zone"),
];

fn lookup(table: &[(&str, &'static str)], key: &str) -> Option<&'static str> {
    table.iter().find(|e| e.0 == key).map(|e| e.1)
}

fn ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    if rt.native_new_target == Value::Undefined {
        return Err(rt.type_error("Constructor Intl.DisplayNames requires 'new'"));
    }
    let o = match arg(a, 1) {
        Value::Object(o) => Some(o),
        Value::Undefined => None,
        _ => return Err(rt.type_error("Options must be an object")),
    };
    check_locales(rt, arg(a, 0), o)?;
    let style = opt_enum(rt, o, "style", &STYLES)?.unwrap_or(0);
    let Some(kind) = opt_enum(rt, o, "type", &TYPES)? else {
        return Err(rt.type_error("Required option \"type\" is missing"));
    };
    let fallback_none = opt_enum(rt, o, "fallback", &["code", "none"])? == Some(1);
    let standard = opt_enum(rt, o, "languageDisplay", &["dialect", "standard"])? == Some(1);
    let proto = rt.intl_proto(P_DISPLAY)?;
    Ok(Value::Object(rt.new_intl(proto, IntlData::Display(DisplayOpts { kind, style, fallback_none, standard }))))
}

fn this_opts(rt: &mut Runtime, this: Value) -> JsResult<DisplayOpts> {
    match rt.intl_ref(this).map(|d| &d.data) {
        Some(IntlData::Display(d)) => Ok(d.clone()),
        _ => Err(rt.type_error("Intl.DisplayNames method called on incompatible receiver")),
    }
}

fn title_words(s: &str) -> String {
    s.split(' ').map(|w| {
        let mut c = w.chars();
        match c.next() {
            Some(f) => f.to_uppercase().chain(c).collect::<String>(),
            None => String::new(),
        }
    }).collect::<Vec<_>>().join(" ")
}

/// The canonical code and its name (if known), or `None` when the code is malformed.
fn name_of(d: &DisplayOpts, code: &str) -> Option<(String, Option<String>)> {
    let alpha = |s: &str| s.bytes().all(|b| b.is_ascii_alphabetic());
    Some(match d.kind {
        0 => {
            let t = tag::parse(code)?;
            if !t.extensions.is_empty() || !t.private.is_empty() || !t.keywords.is_empty() || !t.attributes.is_empty() {
                return None;
            }
            let canon = t.base_name();
            let lang = lookup(LANGUAGES, &t.language);
            let name = match (lookup(DIALECTS, &canon), lang) {
                (Some(dialect), _) if !d.standard => Some(dialect.to_string()),
                (_, Some(l)) => {
                    let extra: Vec<String> = t.script.iter().map(|s| lookup(SCRIPTS, s).unwrap_or(s).to_string())
                        .chain(t.region.iter().map(|r| lookup(REGIONS, r).unwrap_or(r).to_string())).collect();
                    Some(if extra.is_empty() { l.to_string() } else { format!("{} ({})", l, extra.join(", ")) })
                }
                _ => None,
            };
            (canon, name)
        }
        1 => {
            let ok = (code.len() == 2 && alpha(code)) || (code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()));
            if !ok {
                return None;
            }
            let c = code.to_ascii_uppercase();
            let short = if d.style != 0 { lookup(SHORT_REGIONS, &c) } else { None };
            let name = short.or_else(|| lookup(REGIONS, &c)).map(|s| s.to_string());
            (c, name)
        }
        2 => {
            if code.len() != 4 || !alpha(code) {
                return None;
            }
            let lower = code.to_ascii_lowercase();
            let c = format!("{}{}", lower[..1].to_ascii_uppercase(), &lower[1..]);
            let name = lookup(SCRIPTS, &c).map(|s| s.to_string());
            (c, name)
        }
        3 => {
            if code.len() != 3 || !alpha(code) {
                return None;
            }
            let c = code.to_ascii_uppercase();
            let name = super::number_data::currency_name(&c).map(title_words);
            (c, name)
        }
        4 => {
            let ok = !code.is_empty() && code.split('-').all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()));
            if !ok {
                return None;
            }
            let c = code.to_ascii_lowercase();
            let name = lookup(CALENDARS, if c == "gregorian" { "gregory" } else { &c }).map(|s| s.to_string());
            (c, name)
        }
        _ => {
            let name = lookup(FIELDS, code)?;
            (code.to_string(), Some(name.to_string()))
        }
    })
}

fn of(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let d = this_opts(rt, this)?;
    let code = rt.to_rust_string(arg(a, 0))?;
    rt.charge(1 + code.len() as u64 / 16)?;
    let Some((canon, name)) = name_of(&d, &code) else {
        return Err(rt.range_error(&format!("Invalid {} code: {}", TYPES[d.kind as usize % 6], if code.len() > 64 { "(too long)" } else { &code })));
    };
    Ok(match name {
        Some(n) => rt.new_str(&n),
        None if d.fallback_none => Value::Undefined,
        None => rt.new_str(&canon),
    })
}

fn resolved_options(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_opts(rt, this)?;
    let o = rt.plain_object();
    put_str(rt, o, "locale", LOCALE);
    put_str(rt, o, "style", STYLES[d.style as usize % 3]);
    put_str(rt, o, "type", TYPES[d.kind as usize % 6]);
    put_str(rt, o, "fallback", if d.fallback_none { "none" } else { "code" });
    if d.kind == 0 {
        put_str(rt, o, "languageDisplay", if d.standard { "standard" } else { "dialect" });
    }
    Ok(Value::Object(o))
}

pub(super) fn setup(rt: &mut Runtime, intl: ObjId) {
    let (_, p) = intl_class(rt, intl, P_DISPLAY, "DisplayNames", 2, ctor);
    rt.method(p, "of", 1, of);
    rt.method(p, "resolvedOptions", 0, resolved_options);
}
