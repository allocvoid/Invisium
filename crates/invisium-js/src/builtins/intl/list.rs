//! `Intl.ListFormat` with the English patterns: "A, B, and C", "A, B, or C", "A, B, C".

use super::*;

#[derive(Clone)]
pub(crate) struct ListOpts {
    /// 0 conjunction, 1 disjunction, 2 unit.
    pub kind: u8,
    /// 0 long, 1 short, 2 narrow.
    pub style: u8,
}

const TYPES: [&str; 3] = ["conjunction", "disjunction", "unit"];
const STYLES: [&str; 3] = ["long", "short", "narrow"];

/// (separator between two items, separator before the last of three or more, other separators).
fn patterns(o: &ListOpts) -> (&'static str, &'static str, &'static str) {
    match (o.kind, o.style) {
        (0, 0) => (" and ", ", and ", ", "),
        (0, 1) => (" & ", ", & ", ", "),
        (0, _) => (", ", ", ", ", "),
        (1, _) => (" or ", ", or ", ", "),
        (_, 2) => (" ", " ", " "),
        _ => (", ", ", ", ", "),
    }
}

fn ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    if rt.native_new_target == Value::Undefined {
        return Err(rt.type_error("Constructor Intl.ListFormat requires 'new'"));
    }
    let o = opt_obj(rt, arg(a, 1))?;
    check_locales(rt, arg(a, 0), o)?;
    let kind = opt_enum(rt, o, "type", &TYPES)?.unwrap_or(0);
    let style = opt_enum(rt, o, "style", &STYLES)?.unwrap_or(0);
    let proto = rt.intl_proto(P_LIST)?;
    Ok(Value::Object(rt.new_intl(proto, IntlData::List(ListOpts { kind, style }))))
}

fn this_opts(rt: &mut Runtime, this: Value) -> JsResult<ListOpts> {
    match rt.intl_ref(this).map(|d| &d.data) {
        Some(IntlData::List(l)) => Ok(l.clone()),
        _ => Err(rt.type_error("Intl.ListFormat method called on incompatible receiver")),
    }
}

/// StringListFromIterable: every element must be a string.
fn string_list(rt: &mut Runtime, v: Value) -> JsResult<Vec<String>> {
    if v == Value::Undefined {
        return Ok(Vec::new());
    }
    let items = rt.iterate_to_vec(v)?;
    rt.charge(items.len() as u64)?;
    let mut out = Vec::with_capacity(items.len());
    for x in items {
        match x {
            Value::String(s) => {
                let text = rt.rust_string(s);
                rt.charge(text.len() as u64 / 16)?;
                out.push(text);
            }
            _ => return Err(rt.type_error("Iterable yielded a value which is not a string")),
        }
    }
    Ok(out)
}

fn list_parts(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Vec<(&'static str, String)>> {
    let o = this_opts(rt, this)?;
    let items = string_list(rt, arg(a, 0))?;
    let (two, last, middle) = patterns(&o);
    let n = items.len();
    let mut out = Vec::with_capacity(n * 2);
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            let sep = if n == 2 { two } else if i == n - 1 { last } else { middle };
            out.push(("literal", sep.to_string()));
        }
        out.push(("element", item));
    }
    Ok(out)
}

fn format(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let parts = list_parts(rt, this, a)?;
    let s: String = parts.into_iter().map(|p| p.1).collect();
    Ok(rt.new_str(&s))
}

fn format_to_parts(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let parts = list_parts(rt, this, a)?;
    parts_array(rt, &parts)
}

fn resolved_options(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let l = this_opts(rt, this)?;
    let o = rt.plain_object();
    put_str(rt, o, "locale", LOCALE);
    put_str(rt, o, "type", TYPES[l.kind as usize % 3]);
    put_str(rt, o, "style", STYLES[l.style as usize % 3]);
    Ok(Value::Object(o))
}

pub(super) fn setup(rt: &mut Runtime, intl: ObjId) {
    let (_, p) = intl_class(rt, intl, P_LIST, "ListFormat", 0, ctor);
    rt.method(p, "format", 1, format);
    rt.method(p, "formatToParts", 1, format_to_parts);
    rt.method(p, "resolvedOptions", 0, resolved_options);
}
