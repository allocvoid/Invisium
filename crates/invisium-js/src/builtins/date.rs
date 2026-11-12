//! Date: constructor, statics, getters, setters and formatting. Local time is UTC and the
//! clock is the embedder's (`set_clock`), so nothing about the machine leaks to scripts;
//! `toLocale*String` always formats as en-US.

use super::arg;
use super::date_time::*;
use crate::ops::Hint;
use crate::runtime::*;
use crate::value::*;

impl Runtime {
    /// The injected clock as a time value.
    pub(crate) fn now_ms(&mut self) -> f64 {
        time_clip((self.clock)())
    }

    pub(crate) fn new_date(&mut self, t: f64, proto: Option<ObjId>) -> Value {
        Value::Object(self.alloc(Object::new(proto, ObjKind::Date(time_clip(t)))))
    }

    pub(crate) fn date_value(&self, v: Value) -> Option<f64> {
        match v {
            Value::Object(o) => match self.obj(o).kind {
                ObjKind::Date(t) => Some(t),
                _ => None,
            },
            _ => None,
        }
    }
}

fn this_date(rt: &mut Runtime, this: Value) -> JsResult<(ObjId, f64)> {
    match (this, rt.date_value(this)) {
        (Value::Object(o), Some(t)) => Ok((o, t)),
        _ => Err(rt.type_error("this is not a Date object")),
    }
}

fn store(rt: &mut Runtime, o: ObjId, t: f64) -> Value {
    let t = time_clip(t);
    if let ObjKind::Date(slot) = &mut rt.obj_mut(o).kind {
        *slot = t;
    }
    Value::Number(t)
}

/// Year, month, day, hours, minutes, seconds, milliseconds from up to seven arguments.
fn from_components(rt: &mut Runtime, a: &[Value]) -> JsResult<f64> {
    let mut f = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
    for (slot, v) in f.iter_mut().zip(a) {
        *slot = rt.to_num(*v)?;
    }
    if (0.0..=99.0).contains(&f[0].trunc()) {
        f[0] = 1900.0 + f[0].trunc();
    }
    Ok(make_date(make_day(f[0], f[1], f[2]), make_time(f[3], f[4], f[5], f[6])))
}

fn date_ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let nt = rt.native_new_target;
    if nt == Value::Undefined {
        let now = rt.now_ms();
        return Ok(rt.new_str(&format_time(now, 0)));
    }
    let t = match a {
        [] => rt.now_ms(),
        [one] => match rt.date_value(*one) {
            Some(t) => t,
            None => match rt.to_primitive(*one, Hint::Default)? {
                Value::String(s) => parse_date(&rt.rust_string(s)),
                other => rt.to_num(other)?,
            },
        },
        _ => from_components(rt, a)?,
    };
    let default = rt.intr.date_proto;
    let proto = rt.proto_from_ctor(nt, default)?;
    Ok(rt.new_date(t, proto))
}

fn date_now(rt: &mut Runtime, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(rt.now_ms()))
}

fn date_parse(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    let s = rt.to_rust_string(arg(a, 0))?;
    Ok(Value::Number(parse_date(&s)))
}

fn date_utc(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(time_clip(from_components(rt, a)?)))
}

fn value_of(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(this_date(rt, this)?.1))
}

/// Field `idx` of `fields()` (7 = weekday, 8 = legacy two-digit-era year), NaN when invalid.
fn get_field(rt: &mut Runtime, this: Value, idx: usize) -> JsResult<Value> {
    let (_, t) = this_date(rt, this)?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let f = fields(t);
    let v = if idx == 8 { f[0] - 1900 } else { f[idx] };
    Ok(Value::Number(v as f64))
}

fn timezone_offset(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, t) = this_date(rt, this)?;
    Ok(Value::Number(if t.is_nan() { f64::NAN } else { 0.0 }))
}

/// Replaces up to `max` consecutive fields starting at `first` (0 = year ... 6 = ms).
fn set_fields(rt: &mut Runtime, this: Value, a: &[Value], first: usize, max: usize) -> JsResult<Value> {
    let (o, t) = this_date(rt, this)?;
    let mut given = Vec::with_capacity(max);
    for i in 0..a.len().clamp(1, max) {
        given.push(rt.to_num(arg(a, i))?);
    }
    // only setFullYear revives an invalid date (from time zero)
    let base = match (t.is_nan(), first) {
        (false, _) => t,
        (true, 0) => 0.0,
        (true, _) => return Ok(Value::Number(f64::NAN)),
    };
    let mut f = fields(base).map(|x| x as f64);
    f[first..first + given.len()].copy_from_slice(&given);
    Ok(store(rt, o, make_date(make_day(f[0], f[1], f[2]), make_time(f[3], f[4], f[5], f[6]))))
}

fn set_time(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (o, _) = this_date(rt, this)?;
    let t = rt.to_num(arg(a, 0))?;
    Ok(store(rt, o, t))
}

fn set_year(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let y = rt.to_num(arg(a, 0))?;
    let y = if (0.0..=99.0).contains(&y.trunc()) { 1900.0 + y.trunc() } else { y };
    set_fields(rt, this, &[Value::Number(y)], 0, 1)
}

fn year_text(y: i64) -> String {
    if y < 0 { format!("-{:04}", -y) } else { format!("{:04}", y) }
}

/// 0 toString, 1 toDateString, 2 toTimeString, 3 toISOString, 4 toUTCString,
/// 5 toLocaleString, 6 toLocaleDateString, 7 toLocaleTimeString (the locale forms are en-US).
fn format_time(t: f64, kind: u8) -> String {
    if t.is_nan() {
        return "Invalid Date".into();
    }
    let f = fields(t);
    let (wd, mon) = (DAYS[f[7] as usize % 7], MONTHS[f[1] as usize % 12]);
    let date = format!("{} {} {:02} {}", wd, mon, f[2], year_text(f[0]));
    let time = format!("{:02}:{:02}:{:02} GMT+0000 (Coordinated Universal Time)", f[3], f[4], f[5]);
    let us_date = format!("{}/{}/{}", f[1] + 1, f[2], f[0]);
    let us_time = format!("{}:{:02}:{:02} {}", if f[3] % 12 == 0 { 12 } else { f[3] % 12 }, f[4], f[5], if f[3] < 12 { "AM" } else { "PM" });
    match kind {
        0 => format!("{} {}", date, time),
        1 => date,
        2 => time,
        3 => {
            let y = if (0..=9999).contains(&f[0]) { format!("{:04}", f[0]) } else { format!("{}{:06}", if f[0] < 0 { '-' } else { '+' }, f[0].abs()) };
            format!("{}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", y, f[1] + 1, f[2], f[3], f[4], f[5], f[6])
        }
        4 => format!("{}, {:02} {} {} {:02}:{:02}:{:02} GMT", wd, f[2], mon, year_text(f[0]), f[3], f[4], f[5]),
        5 => format!("{}, {}", us_date, us_time),
        6 => us_date,
        _ => us_time,
    }
}

fn to_text(rt: &mut Runtime, this: Value, kind: u8, a: &[Value]) -> JsResult<Value> {
    let (_, t) = this_date(rt, this)?;
    if kind == 3 && t.is_nan() {
        return Err(rt.range_error("Invalid time value"));
    }
    if kind >= 5 && !t.is_nan() {
        // the locale forms go through Intl.DateTimeFormat, honouring `options`
        let s = super::intl::date_to_locale(rt, t, kind - 5, arg(a, 0), arg(a, 1))?;
        return Ok(rt.new_str(&s));
    }
    Ok(rt.new_str(&format_time(t, kind)))
}

fn to_json(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = Value::Object(rt.to_object(this)?);
    if let Value::Number(n) = rt.to_primitive(o, Hint::Number)? {
        if !n.is_finite() {
            return Ok(Value::Null);
        }
    }
    let k = rt.key_str("toISOString");
    let f = rt.get_prop(o, k)?;
    rt.call_value(f, o, &[])
}

/// Date.prototype[Symbol.toPrimitive]: a date is a string unless a number is asked for.
fn to_primitive(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    if !matches!(this, Value::Object(_)) {
        return Err(rt.type_error("Date.prototype[Symbol.toPrimitive] called on a non-object"));
    }
    let hint = match arg(a, 0) {
        Value::String(s) => rt.rust_string(s),
        _ => String::new(),
    };
    let order = match hint.as_str() {
        "string" | "default" => [A_TO_STRING, A_VALUE_OF],
        "number" => [A_VALUE_OF, A_TO_STRING],
        _ => return Err(rt.type_error("Invalid hint")),
    };
    for name in order {
        let f = rt.get_prop(this, PropKey::Str(name))?;
        if rt.is_callable_val(f) {
            let r = rt.call_value(f, this, &[])?;
            if !matches!(r, Value::Object(_)) {
                return Ok(r);
            }
        }
    }
    Err(rt.type_error("Cannot convert object to primitive value"))
}

macro_rules! getter {
    ($idx:expr) => {
        |rt, this, _| get_field(rt, this, $idx)
    };
}
macro_rules! setter {
    ($first:expr, $max:expr) => {
        |rt, this, a| set_fields(rt, this, a, $first, $max)
    };
}
macro_rules! text {
    ($kind:expr) => {
        |rt, this, a| to_text(rt, this, $kind, a)
    };
}

pub(super) fn setup(rt: &mut Runtime) {
    let dp = rt.new_object(rt.intr.object_proto);
    rt.intr.date_proto = Some(dp);
    let c = rt.constructor("Date", 7, date_ctor, dp);
    rt.method(c, "now", 0, date_now);
    rt.method(c, "parse", 1, date_parse);
    rt.method(c, "UTC", 7, date_utc);
    // local time is UTC: every getX / setX has an identical getUTCX / setUTCX twin
    let getters: [(&str, super::NFn); 8] = [
        ("FullYear", getter!(0)),
        ("Month", getter!(1)),
        ("Date", getter!(2)),
        ("Hours", getter!(3)),
        ("Minutes", getter!(4)),
        ("Seconds", getter!(5)),
        ("Milliseconds", getter!(6)),
        ("Day", getter!(7)),
    ];
    for (name, f) in getters {
        rt.method(dp, &format!("get{}", name), 0, f);
        rt.method(dp, &format!("getUTC{}", name), 0, f);
    }
    let setters: [(&str, u32, super::NFn); 7] = [
        ("FullYear", 3, setter!(0, 3)),
        ("Month", 2, setter!(1, 2)),
        ("Date", 1, setter!(2, 1)),
        ("Hours", 4, setter!(3, 4)),
        ("Minutes", 3, setter!(4, 3)),
        ("Seconds", 2, setter!(5, 2)),
        ("Milliseconds", 1, setter!(6, 1)),
    ];
    for (name, arity, f) in setters {
        rt.method(dp, &format!("set{}", name), arity, f);
        rt.method(dp, &format!("setUTC{}", name), arity, f);
    }
    let texts: [(&str, super::NFn); 9] = [
        ("toString", text!(0)),
        ("toDateString", text!(1)),
        ("toTimeString", text!(2)),
        ("toISOString", text!(3)),
        ("toUTCString", text!(4)),
        ("toGMTString", text!(4)),
        ("toLocaleString", text!(5)),
        ("toLocaleDateString", text!(6)),
        ("toLocaleTimeString", text!(7)),
    ];
    for (name, f) in texts {
        rt.method(dp, name, 0, f);
    }
    rt.method(dp, "getTime", 0, value_of);
    rt.method(dp, "valueOf", 0, value_of);
    rt.method(dp, "getTimezoneOffset", 0, timezone_offset);
    rt.method(dp, "getYear", 0, getter!(8));
    rt.method(dp, "setTime", 1, set_time);
    rt.method(dp, "setYear", 1, set_year);
    rt.method(dp, "toJSON", 1, to_json);
    let f = rt.native_fn("[Symbol.toPrimitive]", 1, to_primitive);
    rt.define_value(dp, PropKey::Sym(SYM_TO_PRIMITIVE), Value::Object(f), CONFIGURABLE);
}
