//! Time-value arithmetic and date-string parsing for `Date`. No time zone exists here: local
//! time is UTC by design (the engine never reveals the machine's zone).

pub(super) const MS_PER_DAY: f64 = 86_400_000.0;
pub(super) const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
pub(super) const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTH_NAMES: [&str; 12] = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];
const DAY_NAMES: [&str; 7] = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"];

/// Days since 1970-01-01 of a proleptic Gregorian date (month 1..=12).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// [year, month (0-based), day, hours, minutes, seconds, milliseconds, weekday] of a finite,
/// in-range time value.
pub(super) fn fields(t: f64) -> [i64; 8] {
    let day = (t / MS_PER_DAY).floor();
    let rest = (t - day * MS_PER_DAY) as i64;
    let day = day as i64;
    let (y, m, d) = civil_from_days(day);
    [y, m - 1, d, rest / 3_600_000, rest / 60_000 % 60, rest / 1000 % 60, rest % 1000, (day + 4).rem_euclid(7)]
}

fn integer(n: f64) -> f64 {
    n.trunc() + 0.0
}

pub(super) fn make_day(year: f64, month: f64, date: f64) -> f64 {
    if !(year.is_finite() && month.is_finite() && date.is_finite()) {
        return f64::NAN;
    }
    let (y, m) = (integer(year), integer(month));
    let ym = y + (m / 12.0).floor();
    if ym.abs() > 400_000.0 {
        return f64::NAN;
    }
    let mn = m.rem_euclid(12.0) as i64;
    days_from_civil(ym as i64, mn + 1, 1) as f64 + integer(date) - 1.0
}

pub(super) fn make_time(h: f64, m: f64, s: f64, ms: f64) -> f64 {
    if !(h.is_finite() && m.is_finite() && s.is_finite() && ms.is_finite()) {
        return f64::NAN;
    }
    integer(h) * 3_600_000.0 + integer(m) * 60_000.0 + integer(s) * 1000.0 + integer(ms)
}

pub(super) fn make_date(day: f64, time: f64) -> f64 {
    let t = day * MS_PER_DAY + time;
    if t.is_finite() { t } else { f64::NAN }
}

pub(super) fn time_clip(t: f64) -> f64 {
    if !t.is_finite() || t.abs() > 8.64e15 { f64::NAN } else { integer(t) }
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> u8 {
        self.b.get(self.i).copied().unwrap_or(0)
    }
    fn eat(&mut self, c: u8) -> bool {
        let hit = self.peek() == c;
        self.i += hit as usize;
        hit
    }
    /// Exactly `n` digits.
    fn fixed(&mut self, n: usize) -> Option<i64> {
        let d = self.b.get(self.i..self.i + n)?;
        if !d.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.i += n;
        Some(d.iter().fold(0, |a, c| a * 10 + (c - b'0') as i64))
    }
    /// A run of digits: (value, digit count). Long runs saturate (far outside any valid field)
    /// so later arithmetic cannot overflow.
    fn number(&mut self) -> (i64, usize) {
        let start = self.i;
        let mut v: i64 = 0;
        while self.peek().is_ascii_digit() {
            v = (v * 10 + (self.peek() - b'0') as i64).min(999_999_999_999);
            self.i += 1;
        }
        (v, self.i - start)
    }
    /// `.fff` (or `,fff`): milliseconds from the first three digits.
    fn fraction(&mut self) -> Option<i64> {
        if !(self.peek() == b'.' || self.peek() == b',') || !self.b.get(self.i + 1).is_some_and(u8::is_ascii_digit) {
            return Some(0);
        }
        self.i += 1;
        let start = self.i;
        let (_, k) = self.number();
        let d = &self.b[start..start + k.min(3)];
        Some(d.iter().fold(0, |a, c| a * 10 + (c - b'0') as i64) * [100, 10, 1][d.len() - 1])
    }
}

fn compose(y: i64, mon0: i64, d: i64, h: i64, mi: i64, s: i64, ms: i64, offset_min: i64) -> Option<f64> {
    let midnight24 = h == 24 && mi == 0 && s == 0 && ms == 0;
    if !(0..12).contains(&mon0) || !(1..=31).contains(&d) || !(h < 24 || midnight24) || mi > 59 || s > 59 {
        return None;
    }
    let t = make_date(make_day(y as f64, mon0 as f64, d as f64), make_time(h as f64, mi as f64, s as f64, ms as f64));
    let t = time_clip(t - offset_min as f64 * 60_000.0);
    if t.is_nan() { None } else { Some(t) }
}

/// The ECMAScript date-time string format: `YYYY[-MM[-DD]][THH:mm[:ss[.sss]][Z|±HH:mm]]`.
fn parse_iso(b: &[u8]) -> Option<f64> {
    let mut c = Cursor { b, i: 0 };
    let year = match c.peek() {
        b'+' | b'-' => {
            let neg = c.peek() == b'-';
            c.i += 1;
            let y = c.fixed(6)?;
            if neg && y == 0 {
                return None;
            }
            if neg { -y } else { y }
        }
        _ => c.fixed(4)?,
    };
    let (mut month, mut day) = (1, 1);
    if c.eat(b'-') {
        month = c.fixed(2)?;
        if c.eat(b'-') {
            day = c.fixed(2)?;
        }
    }
    let (mut h, mut mi, mut s, mut ms, mut off) = (0, 0, 0, 0, 0);
    if matches!(c.peek(), b'T' | b't' | b' ') {
        c.i += 1;
        h = c.fixed(2)?;
        if !c.eat(b':') {
            return None;
        }
        mi = c.fixed(2)?;
        if c.eat(b':') {
            s = c.fixed(2)?;
            ms = c.fraction()?;
        }
        if c.eat(b'Z') || c.eat(b'z') {
        } else if matches!(c.peek(), b'+' | b'-') {
            let sign = if c.peek() == b'-' { -1 } else { 1 };
            c.i += 1;
            let oh = c.fixed(2)?;
            c.eat(b':');
            off = sign * (oh * 60 + c.fixed(2)?);
        }
    }
    if c.i != b.len() {
        return None;
    }
    compose(year, month - 1, day, h, mi, s, ms, off)
}

fn two_digit_year(n: i64, digits: usize) -> i64 {
    if digits > 2 { n } else if n < 50 { 2000 + n } else { 1900 + n }
}

fn zone_minutes(word: &str) -> Option<i64> {
    Some(match word {
        "gmt" | "utc" | "ut" | "z" => 0,
        "est" | "cdt" => -300,
        "edt" => -240,
        "cst" | "mdt" => -360,
        "mst" | "pdt" => -420,
        "pst" => -480,
        _ => return None,
    })
}

/// The `toString` / `toUTCString` (RFC 2822) shapes and other common spellings: month names,
/// `M/D/YYYY`, `hh:mm[:ss]`, AM/PM, zone names and numeric offsets, in any order.
fn parse_legacy(b: &[u8]) -> Option<f64> {
    let mut c = Cursor { b, i: 0 };
    let (mut year, mut month, mut day): (Option<i64>, Option<i64>, Option<i64>) = (None, None, None);
    let (mut h, mut mi, mut s, mut ms) = (0, 0, 0, 0);
    let (mut have_time, mut pm, mut zone): (bool, Option<bool>, Option<i64>) = (false, None, None);
    let mut loose: Vec<(i64, usize)> = Vec::new();
    while c.i < b.len() {
        let ch = c.peek();
        if ch.is_ascii_whitespace() || ch == b',' {
            c.i += 1;
        } else if ch == b'(' {
            // a comment such as "(Coordinated Universal Time)"
            let mut depth = 0;
            while c.i < b.len() {
                depth += match c.peek() {
                    b'(' => 1,
                    b')' => -1,
                    _ => 0,
                };
                c.i += 1;
                if depth == 0 {
                    break;
                }
            }
        } else if ch.is_ascii_alphabetic() {
            let start = c.i;
            while c.peek().is_ascii_alphabetic() {
                c.i += 1;
            }
            let word = String::from_utf8_lossy(&b[start..c.i]).to_ascii_lowercase();
            c.eat(b'.');
            if let Some(m) = MONTH_NAMES.iter().position(|n| word.len() >= 3 && n.starts_with(&word)) {
                month = Some(m as i64);
            } else if word == "am" || word == "pm" {
                pm = Some(word == "pm");
            } else if let Some(z) = zone_minutes(&word) {
                zone = Some(z);
            } else if !(DAY_NAMES.iter().any(|n| word.len() >= 3 && n.starts_with(&word)) || word == "t") {
                return None;
            }
        } else if ch.is_ascii_digit() {
            let (n, k) = c.number();
            if c.peek() == b':' && !have_time {
                c.i += 1;
                h = n;
                mi = c.number().0;
                if c.eat(b':') {
                    s = c.number().0;
                    ms = c.fraction()?;
                }
                have_time = true;
            } else if c.peek() == b'/' && month.is_none() {
                c.i += 1;
                let (n2, _) = c.number();
                let third = if c.eat(b'/') { Some(c.number()) } else { None };
                match (k >= 3, third) {
                    (true, Some((d, _))) => (year, month, day) = (Some(n), Some(n2 - 1), Some(d)),
                    (false, Some((y, yk))) => (year, month, day) = (Some(two_digit_year(y, yk)), Some(n - 1), Some(n2)),
                    _ => return None,
                }
            } else {
                loose.push((n, k));
            }
        } else if (ch == b'+' || ch == b'-') && (have_time || zone.is_some()) && b.get(c.i + 1).is_some_and(u8::is_ascii_digit) {
            let sign = if ch == b'-' { -1 } else { 1 };
            c.i += 1;
            let (n, k) = c.number();
            let minutes = if c.eat(b':') { n * 60 + c.number().0 } else if k > 2 { n / 100 * 60 + n % 100 } else { n * 60 };
            zone = Some(sign * minutes);
        } else if ch == b'-' || ch == b'.' {
            c.i += 1;
        } else {
            return None;
        }
    }
    for (n, k) in loose {
        if (k >= 3 || n > 31) && year.is_none() {
            year = Some(n);
        } else if day.is_none() && k < 3 {
            day = Some(n);
        } else if year.is_none() {
            year = Some(two_digit_year(n, k));
        } else {
            return None;
        }
    }
    match pm {
        Some(_) if !(1..=12).contains(&h) => return None,
        Some(true) if h < 12 => h += 12,
        Some(false) if h == 12 => h = 0,
        _ => {}
    }
    compose(year?, month?, day.unwrap_or(1), h, mi, s, ms, zone.unwrap_or(0))
}

/// `Date.parse`: the time value of a date string, NaN when it is not understood.
pub(super) fn parse_date(text: &str) -> f64 {
    let t = text.trim();
    if t.is_empty() || t.len() > 256 || !t.is_ascii() {
        return f64::NAN;
    }
    parse_iso(t.as_bytes()).or_else(|| parse_legacy(t.as_bytes())).unwrap_or(f64::NAN)
}
