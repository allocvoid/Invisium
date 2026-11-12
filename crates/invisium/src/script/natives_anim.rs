//! Native primitives behind `requestAnimationFrame` and `Element.animate`.

use super::natives::{arg, def, node, Cx, NRes, Native};
use crate::anim::{Direction, Easing, Fill, Keyframe, Timing};
use crate::css::properties::{self, Declaration, PropId, Value as CssValue};
use crate::css::tokenizer::tokenize_value;
use invisium_js::{Runtime, Value};
use std::rc::Rc;

/// `wantFrame()`: a script has work for the next frame.
fn n_want_frame(_rt: &mut Runtime, cx: &Cx, _args: &[Value]) -> NRes {
    cx.st.borrow_mut().frame_wanted = true;
    Ok(Value::Undefined)
}

/// `observing(flag)`: size or visibility observers exist (called whenever a target is added
/// or the last one goes away).
fn n_observing(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let on = rt.to_boolean(arg(args, 0));
    let mut st = cx.st.borrow_mut();
    st.observing = on;
    st.observers_due = on;
    Ok(Value::Undefined)
}

fn num_or(rt: &mut Runtime, v: Value, default: f64) -> f64 {
    if matches!(v, Value::Undefined | Value::Null) {
        return default;
    }
    let n = rt.to_number(v);
    if n.is_nan() {
        default
    } else {
        n
    }
}

/// `animate(element, frames, duration, delay, endDelay, iterations, iterationStart, direction,
/// fill, easing)` → animation id (0 if it could not be created).
///
/// `frames` is flat: for each keyframe its offset (0..1), its timing function or null, the
/// number of properties, then that many (CSS property name, value text) pairs.
fn n_animate(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let target = node(rt, cx, arg(args, 0))?;
    let list = arg(args, 1);
    let len_v = rt.get(list, "length").unwrap_or(Value::Undefined);
    let len = (rt.to_number(len_v).max(0.0) as u32).min(200_000);
    let mut frames: Vec<Keyframe> = Vec::new();
    let mut decls: Vec<Declaration> = Vec::new();
    let no_urls = |_: &str| None;
    let mut i = 0u32;
    while i + 2 < len && frames.len() < 5000 {
        let item = |rt: &mut Runtime, k: u32| rt.get_index(list, k).unwrap_or(Value::Undefined);
        let off = item(rt, i);
        let offset = rt.to_number(off);
        let ease_v = item(rt, i + 1);
        let easing = if matches!(ease_v, Value::Undefined | Value::Null) {
            None
        } else {
            Easing::parse_str(&rt.to_string_lossy(ease_v))
        };
        let count_v = item(rt, i + 2);
        let count = rt.to_number(count_v).max(0.0) as u32;
        i += 3;
        let mut values: Vec<(PropId, CssValue)> = Vec::new();
        for _ in 0..count {
            if i + 1 >= len {
                break;
            }
            let name_v = item(rt, i);
            let text_v = item(rt, i + 1);
            let name = rt.to_string_lossy(name_v);
            let text = rt.to_string_lossy(text_v);
            i += 2;
            decls.clear();
            let toks = tokenize_value(&text);
            properties::parse_declaration(&name, &toks, &text, false, &no_urls, &mut decls);
            for d in decls.drain(..) {
                // custom properties and values that need var() are not animated
                if !matches!(d.value, CssValue::Custom(..) | CssValue::Pending(_)) {
                    values.push((d.prop, d.value));
                }
            }
        }
        if offset.is_finite() && !values.is_empty() {
            frames.push(Keyframe { offset: offset.clamp(0.0, 1.0) as f32, easing, values });
        }
    }
    let duration = num_or(rt, arg(args, 2), 0.0).max(0.0);
    let delay = num_or(rt, arg(args, 3), 0.0);
    let end_delay = num_or(rt, arg(args, 4), 0.0);
    let iterations = num_or(rt, arg(args, 5), 1.0).max(0.0);
    let iteration_start = num_or(rt, arg(args, 6), 0.0).max(0.0);
    let dir = rt.to_string_lossy(arg(args, 7));
    let fill = rt.to_string_lossy(arg(args, 8));
    let easing = rt.to_string_lossy(arg(args, 9));
    let finite = |v: f64| if v.is_finite() { v } else { 0.0 };
    let timing = Timing {
        delay: finite(delay),
        end_delay: finite(end_delay),
        duration: if duration.is_finite() { duration } else { 0.0 },
        iterations,
        iteration_start: if iteration_start.is_finite() { iteration_start } else { 0.0 },
        direction: match dir.as_str() {
            "reverse" => Direction::Reverse,
            "alternate" => Direction::Alternate,
            "alternate-reverse" => Direction::AlternateReverse,
            _ => Direction::Normal,
        },
        fill: match fill.as_str() {
            "forwards" => Fill::Forwards,
            "backwards" => Fill::Backwards,
            "both" => Fill::Both,
            _ => Fill::None,
        },
        easing: Easing::parse_str(&easing).unwrap_or(Easing::Linear),
    };
    let mut st = cx.st.borrow_mut();
    let id = st.page.script_animate(target, frames, timing);
    st.frame_wanted = true;
    Ok(Value::Number(id as f64))
}

/// `animation(id, what, arg)`: control or query a script animation; `undefined` if the id is
/// unknown (cancelled, or ended without keeping its values).
fn n_animation(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let id = rt.to_number(arg(args, 0)).max(0.0) as u32;
    let what = rt.to_string_lossy(arg(args, 1));
    let a = rt.to_number(arg(args, 2));
    let mut st = cx.st.borrow_mut();
    let r = st.page.script_animation(id, &what, a);
    if !matches!(what.as_str(), "time" | "start" | "rate" | "end" | "state") {
        st.frame_wanted = true;
    }
    Ok(match r {
        Some(v) if v.is_nan() => Value::Null,
        Some(v) => Value::Number(v),
        None => Value::Undefined,
    })
}

pub(crate) fn install(rt: &mut Runtime, n: Value, cx: &Rc<Cx>) {
    let table: &[(&str, u32, Native)] = &[("wantFrame", 0, n_want_frame), ("observing", 1, n_observing), ("animate", 10, n_animate), ("animation", 3, n_animation)];
    for &(name, arity, f) in table {
        def(rt, n, cx, name, arity, f);
    }
}
