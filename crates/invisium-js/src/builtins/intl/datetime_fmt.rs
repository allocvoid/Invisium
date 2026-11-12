//! en-US date and time patterns for `Intl.DateTimeFormat`, built from the resolved fields.
//! Every time value is rendered in UTC with the proleptic Gregorian calendar.

use super::DtOpts;
use crate::builtins::date_time::fields;

pub(super) type Part = (&'static str, String);

#[derive(Clone, Copy, PartialEq)]
enum Tok {
    Weekday,
    Era,
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Frac,
    Period,
    Zone,
    Lit(&'static str),
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
/// Separator of a range: an en dash between thin spaces, as CLDR's English data has it.
const RANGE_SEP: &str = "\u{2009}\u{2013}\u{2009}";

/// `Tue, Mar 5, 2024 AD`, `3/5/2024`, `March 2024`, `3/5` ... from whichever fields are present.
fn date_tokens(o: &DtOpts) -> Vec<Tok> {
    let mut v = Vec::new();
    let text = o.month >= 3;
    if o.month != 0 {
        if o.weekday != 0 {
            v.extend([Tok::Weekday, Tok::Lit(", ")]);
        }
        v.push(Tok::Month);
        if o.day != 0 {
            v.extend([Tok::Lit(if text { " " } else { "/" }), Tok::Day]);
        }
        if o.year != 0 {
            v.extend([Tok::Lit(if !text { "/" } else if o.day != 0 { ", " } else { " " }), Tok::Year]);
        }
        if o.era != 0 {
            v.extend([Tok::Lit(" "), Tok::Era]);
        }
    } else {
        for (width, tok) in [(o.day, Tok::Day), (o.weekday, Tok::Weekday), (o.year, Tok::Year), (o.era, Tok::Era)] {
            if width != 0 {
                if !v.is_empty() {
                    v.push(Tok::Lit(" "));
                }
                v.push(tok);
            }
        }
    }
    v
}

/// `12:07:03 AM UTC`, `00:07`, `07:03.678` ...
fn time_tokens(o: &DtOpts) -> Vec<Tok> {
    let mut v = Vec::new();
    for (width, tok, sep) in [(o.hour, Tok::Hour, ""), (o.minute, Tok::Minute, ":"), (o.second, Tok::Second, ":"), (o.frac, Tok::Frac, ".")] {
        if width != 0 {
            if !v.is_empty() {
                v.push(Tok::Lit(sep));
            }
            v.push(tok);
        }
    }
    if o.hour != 0 && o.hour_cycle < 2 {
        v.extend([Tok::Lit(" "), Tok::Period]);
    }
    if o.tz_name != 0 {
        if !v.is_empty() {
            v.push(Tok::Lit(" "));
        }
        v.push(Tok::Zone);
    }
    v
}

/// The literal between the date and the time: a long month reads "March 5, 2024 at 1:00 PM".
fn joiner(o: &DtOpts) -> Tok {
    Tok::Lit(if o.month == 5 && o.hour != 0 { " at " } else { ", " })
}

fn tokens(o: &DtOpts) -> Vec<Tok> {
    let (mut v, time) = (date_tokens(o), time_tokens(o));
    if !v.is_empty() && !time.is_empty() {
        v.push(joiner(o));
    }
    v.extend(time);
    v
}

fn pad(n: i64, two: bool) -> String {
    if two { format!("{:02}", n) } else { n.to_string() }
}

fn by_width(name: &str, width: u8) -> String {
    match width {
        1 => name.chars().take(1).collect(),
        2 => name.chars().take(3).collect(),
        _ => name.to_string(),
    }
}

fn render(tok: Tok, o: &DtOpts, f: &[i64; 8]) -> Part {
    match tok {
        Tok::Lit(s) => ("literal", s.to_string()),
        Tok::Weekday => ("weekday", by_width(DAYS[f[7].rem_euclid(7) as usize], o.weekday)),
        Tok::Era => {
            let names = if f[0] <= 0 { ["B", "BC", "Before Christ"] } else { ["A", "AD", "Anno Domini"] };
            ("era", names[(o.era.clamp(1, 3) - 1) as usize].to_string())
        }
        Tok::Year => {
            // year of the era: 1 BC is the year before 1 AD
            let y = if f[0] <= 0 { 1 - f[0] } else { f[0] };
            ("year", if o.year == 2 { format!("{:02}", y % 100) } else { y.to_string() })
        }
        Tok::Month => {
            let name = MONTHS[f[1].rem_euclid(12) as usize];
            ("month", match o.month {
                1 | 2 => pad(f[1] + 1, o.month == 2),
                w => by_width(name, w - 2),
            })
        }
        Tok::Day => ("day", pad(f[2], o.day == 2)),
        Tok::Hour => {
            let h = match o.hour_cycle {
                0 => f[3] % 12,
                1 => if f[3] % 12 == 0 { 12 } else { f[3] % 12 },
                2 => f[3],
                _ => if f[3] == 0 { 24 } else { f[3] },
            };
            ("hour", pad(h, o.hour == 2 || o.hour_cycle >= 2))
        }
        Tok::Minute => ("minute", pad(f[4], o.minute == 2 || o.hour != 0)),
        Tok::Second => ("second", pad(f[5], o.second == 2 || o.minute != 0)),
        Tok::Frac => ("fractionalSecond", format!("{:03}", f[6]).chars().take(o.frac as usize).collect()),
        Tok::Period => ("dayPeriod", if f[3] < 12 { "AM" } else { "PM" }.to_string()),
        Tok::Zone => ("timeZoneName", match o.tz_name {
            1 => "UTC",
            2 => "Coordinated Universal Time",
            _ => "GMT",
        }.to_string()),
    }
}

pub(super) fn parts(o: &DtOpts, t: f64) -> Vec<Part> {
    let f = fields(t);
    tokens(o).into_iter().map(|tok| render(tok, o, &f)).collect()
}

pub(super) fn format(o: &DtOpts, t: f64) -> String {
    parts(o, t).into_iter().map(|p| p.1).collect()
}

/// Start and end token lists of a date-only range, with what both ends share written once:
/// `Mar 5 – 7, 2024`, `Mar 5 – Apr 7, 2024`, `Mar – Apr 2024`; numeric dates stay whole.
fn date_range(o: &DtOpts, fa: &[i64; 8], fb: &[i64; 8]) -> (Vec<Tok>, Vec<Tok>) {
    let (mut ta, mut tb) = (date_tokens(o), date_tokens(o));
    let same_year = fa[0] == fb[0];
    if o.month >= 3 && (o.year == 0 || same_year) {
        if o.year != 0 {
            if let Some(p) = ta.iter().position(|t| *t == Tok::Year) {
                ta.truncate(p.saturating_sub(1));
            }
        }
        if same_year && fa[1] == fb[1] && o.weekday == 0 && o.day != 0 {
            if let Some(p) = tb.iter().position(|t| *t == Tok::Month) {
                tb.drain(p..(p + 2).min(tb.len()));
            }
        }
    }
    (ta, tb)
}

/// A time-only range: a day period or zone name both ends share is written once, at the end
/// (`10:00 – 11:30 AM`, `10:00 AM – 2:00 PM UTC`).
fn time_range(o: &DtOpts, fa: &[i64; 8], fb: &[i64; 8]) -> (Vec<Tok>, Vec<Tok>) {
    let (mut ta, tb) = (time_tokens(o), time_tokens(o));
    while let Some(last) = ta.last().copied() {
        let shared = match last {
            Tok::Period | Tok::Zone => render(last, o, fa) == render(last, o, fb),
            Tok::Lit(" ") => true,
            _ => false,
        };
        if !shared {
            break;
        }
        ta.pop();
    }
    (ta, tb)
}

/// The parts of `formatRange`, each tagged `shared`, `startRange` or `endRange`.
pub(super) fn range_parts(o: &DtOpts, a: f64, b: f64) -> Vec<(Part, &'static str)> {
    let (fa, fb) = (fields(a), fields(b));
    let all = tokens(o);
    let tag = |toks: &[Tok], f: &[i64; 8], source: &'static str| -> Vec<(Part, &'static str)> { toks.iter().map(|t| (render(*t, o, f), source)).collect() };
    let whole_a = tag(&all, &fa, "shared");
    if whole_a.iter().map(|p| &p.0).eq(tag(&all, &fb, "shared").iter().map(|p| &p.0)) {
        // both ends print the same: one date, not a range
        return whole_a;
    }
    let (date, time) = (date_tokens(o), time_tokens(o));
    let mut out = Vec::new();
    let (ta, tb) = if date.is_empty() {
        time_range(o, &fa, &fb)
    } else if time.is_empty() {
        date_range(o, &fa, &fb)
    } else if fa[..3] == fb[..3] {
        // same day: the date once, then the range of times
        out = tag(&date, &fa, "shared");
        out.push((render(joiner(o), o, &fa), "shared"));
        time_range(o, &fa, &fb)
    } else {
        (all.clone(), all)
    };
    out.extend(tag(&ta, &fa, "startRange"));
    out.push((("literal", RANGE_SEP.to_string()), "shared"));
    out.extend(tag(&tb, &fb, "endRange"));
    out
}
