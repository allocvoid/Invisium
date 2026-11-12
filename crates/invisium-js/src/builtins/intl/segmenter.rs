//! `Intl.Segmenter`: graphemes (a base character plus following combining marks, variation
//! selectors, emoji modifiers and joiner sequences), words (runs of letters and digits, with
//! apostrophes and points inside them) and sentences (split after `.`, `!`, `?`).
//! Positions are UTF-16 code-unit indices, as in every engine.

use super::*;

const GRANULARITIES: [&str; 3] = ["grapheme", "word", "sentence"];

fn ctor(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    if rt.native_new_target == Value::Undefined {
        return Err(rt.type_error("Constructor Intl.Segmenter requires 'new'"));
    }
    let o = opt_obj(rt, arg(a, 1))?;
    check_locales(rt, arg(a, 0), o)?;
    let g = opt_enum(rt, o, "granularity", &GRANULARITIES)?.unwrap_or(0);
    let proto = rt.intl_proto(P_SEGMENTER)?;
    Ok(Value::Object(rt.new_intl(proto, IntlData::Segmenter(g))))
}

fn is_extend(c: u32) -> bool {
    matches!(c, 0x300..=0x36F | 0x483..=0x489 | 0x591..=0x5BD | 0x610..=0x61A | 0x64B..=0x65F | 0x900..=0x903 | 0x93A..=0x94F
        | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200C | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F
        | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F | 0xE0100..=0xE01EF)
}

fn is_regional(c: u32) -> bool {
    (0x1F1E6..=0x1F1FF).contains(&c)
}

/// Code points with their UTF-16 offsets.
fn code_points(units: &[u16]) -> Vec<(u32, usize)> {
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = units[i] as u32;
        if (0xD800..0xDC00).contains(&u) && i + 1 < units.len() && (0xDC00..0xE000).contains(&(units[i + 1] as u32)) {
            out.push((0x10000 + ((u - 0xD800) << 10) + (units[i + 1] as u32 - 0xDC00), i));
            i += 2;
        } else {
            out.push((u, i));
            i += 1;
        }
    }
    out
}

fn graphemes(cps: &[(u32, usize)]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut ri_run = 0;
    for (k, &(c, at)) in cps.iter().enumerate() {
        let prev = if k > 0 { Some(cps[k - 1].0) } else { None };
        let join = match prev {
            None => false,
            Some(p) => is_extend(c) || p == 0x200D || (p == 0x0D && c == 0x0A) || (is_regional(p) && is_regional(c) && ri_run % 2 == 1) || c == 0x200D,
        };
        ri_run = if is_regional(c) { ri_run + 1 } else { 0 };
        if !join {
            starts.push(at);
        }
    }
    starts
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Word boundaries, and whether each word is word-like.
fn words(cps: &[(u32, usize)]) -> Vec<(usize, bool)> {
    let ch = |k: usize| char::from_u32(cps[k].0).unwrap_or('\u{fffd}');
    let mut out = Vec::new();
    let mut k = 0;
    while k < cps.len() {
        let start = cps[k].1;
        let c = ch(k);
        k += 1;
        if word_char(c) {
            loop {
                while k < cps.len() && (word_char(ch(k)) || is_extend(cps[k].0)) {
                    k += 1;
                }
                // an apostrophe or point between two letters, or a point or comma between two
                // digits, keeps the word together ("don't", "e.g", "3.14", "1,000")
                if k + 1 >= cps.len() {
                    break;
                }
                let (prev, sep, next) = (ch(k - 1), ch(k), ch(k + 1));
                let letters = matches!(sep, '\'' | '’' | '.' | ':' | '·') && prev.is_alphabetic() && next.is_alphabetic();
                let digits = matches!(sep, '.' | ',') && prev.is_numeric() && next.is_numeric();
                if letters || digits {
                    k += 2;
                } else {
                    break;
                }
            }
            out.push((start, true));
        } else if c.is_whitespace() && c != '\n' && c != '\r' {
            while k < cps.len() && ch(k).is_whitespace() && ch(k) != '\n' && ch(k) != '\r' {
                k += 1;
            }
            out.push((start, false));
        } else {
            while k < cps.len() && is_extend(cps[k].0) {
                k += 1;
            }
            if c == '\r' && k < cps.len() && ch(k) == '\n' {
                k += 1;
            }
            out.push((start, false));
        }
    }
    out
}

fn sentences(cps: &[(u32, usize)]) -> Vec<usize> {
    let ch = |k: usize| char::from_u32(cps[k].0).unwrap_or('\u{fffd}');
    let mut starts = Vec::new();
    let mut k = 0;
    while k < cps.len() {
        starts.push(cps[k].1);
        while k < cps.len() {
            let c = ch(k);
            k += 1;
            if matches!(c, '.' | '!' | '?') {
                while k < cps.len() && matches!(ch(k), '.' | '!' | '?' | '"' | '\'' | ')' | ']' | '”' | '’') {
                    k += 1;
                }
                if k == cps.len() || ch(k).is_whitespace() {
                    while k < cps.len() && ch(k).is_whitespace() {
                        k += 1;
                    }
                    break;
                }
            } else if c == '\n' || c == '\u{2029}' {
                break;
            }
        }
    }
    starts
}

/// (start, end, word-like) of every segment, in UTF-16 units.
fn segments(rt: &mut Runtime, g: u8, units: &[u16]) -> JsResult<Vec<(usize, usize, Option<bool>)>> {
    rt.charge(1 + units.len() as u64 / 2)?;
    let cps = code_points(units);
    let starts: Vec<(usize, Option<bool>)> = match g {
        0 => graphemes(&cps).into_iter().map(|s| (s, None)).collect(),
        1 => words(&cps).into_iter().map(|(s, w)| (s, Some(w))).collect(),
        _ => sentences(&cps).into_iter().map(|s| (s, None)).collect(),
    };
    let mut out = Vec::with_capacity(starts.len());
    for (i, (s, w)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map(|n| n.0).unwrap_or(units.len());
        out.push((*s, end, *w));
    }
    Ok(out)
}

fn segment(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let g = match rt.intl_ref(this).map(|d| &d.data) {
        Some(IntlData::Segmenter(g)) => *g,
        _ => return Err(rt.type_error("Intl.Segmenter method called on incompatible receiver")),
    };
    let s = rt.to_str(arg(a, 0))?;
    let proto = rt.intr.intl_protos[P_SEGMENTS];
    Ok(Value::Object(rt.new_intl(proto, IntlData::Segments(g, s))))
}

fn this_segments(rt: &mut Runtime, this: Value) -> JsResult<(u8, StrId)> {
    match rt.intl_ref(this).map(|d| &d.data) {
        Some(IntlData::Segments(g, s)) => Ok((*g, *s)),
        _ => Err(rt.type_error("Segments method called on incompatible receiver")),
    }
}

fn segment_object(rt: &mut Runtime, s: StrId, units: &[u16], seg: (usize, usize, Option<bool>)) -> Value {
    let o = rt.plain_object();
    let text = rt.new_str16(units[seg.0..seg.1].to_vec());
    put(rt, o, "segment", text);
    put(rt, o, "index", Value::Number(seg.0 as f64));
    put(rt, o, "input", Value::String(s));
    if let Some(w) = seg.2 {
        put(rt, o, "isWordLike", Value::Bool(w));
    }
    Value::Object(o)
}

fn containing(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let (g, s) = this_segments(rt, this)?;
    let n = crate::ops::to_integer(rt.to_num(arg(a, 0))?);
    let units = rt.str_rc(s);
    if n < 0.0 || n >= units.len() as f64 {
        return Ok(Value::Undefined);
    }
    let segs = segments(rt, g, &units)?;
    let n = n as usize;
    match segs.into_iter().find(|seg| seg.0 <= n && n < seg.1) {
        Some(seg) => Ok(segment_object(rt, s, &units, seg)),
        None => Ok(Value::Undefined),
    }
}

fn iterator(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let (g, s) = this_segments(rt, this)?;
    let units = rt.str_rc(s);
    let segs = segments(rt, g, &units)?;
    rt.charge(segs.len() as u64)?;
    let mut items = Vec::with_capacity(segs.len());
    let mark = rt.scratch.len();
    for seg in segs {
        let v = segment_object(rt, s, &units, seg);
        rt.scratch.push(v);
        items.push(v);
    }
    let arr = rt.new_array_values(&items);
    rt.scratch.truncate(mark);
    Ok(rt.new_array_iter(arr, IterKind::Values))
}

fn resolved_options(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    let g = match rt.intl_ref(this).map(|d| &d.data) {
        Some(IntlData::Segmenter(g)) => *g,
        _ => return Err(rt.type_error("Intl.Segmenter method called on incompatible receiver")),
    };
    let o = rt.plain_object();
    put_str(rt, o, "locale", LOCALE);
    put_str(rt, o, "granularity", GRANULARITIES[g as usize % 3]);
    Ok(Value::Object(o))
}

pub(super) fn setup(rt: &mut Runtime, intl: ObjId) {
    let (_, p) = intl_class(rt, intl, P_SEGMENTER, "Segmenter", 0, ctor);
    rt.method(p, "segment", 1, segment);
    rt.method(p, "resolvedOptions", 0, resolved_options);
    let op = rt.intr.object_proto;
    let sp = rt.new_object(op);
    rt.intr.intl_protos[P_SEGMENTS] = Some(sp);
    rt.method(sp, "containing", 1, containing);
    rt.method_sym(sp, SYM_ITERATOR, "[Symbol.iterator]", 0, iterator);
}
