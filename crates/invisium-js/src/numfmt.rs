//! Number <-> string conversions following ECMAScript formatting rules.

/// Shortest round-trip digits and decimal exponent `n` such that value = 0.DIGITS * 10^n.
fn shortest(x: f64) -> (String, i32) {
    let s = format!("{:e}", x);
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = digits.trim_end_matches('0').to_string();
    let digits = if digits.is_empty() { "0".to_string() } else { digits };
    (digits, exp.parse::<i32>().unwrap_or(0) + 1)
}

pub fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if x < 0.0 {
        return format!("-{}", number_to_string(-x));
    }
    if x.fract() == 0.0 && x < 9007199254740992.0 {
        return format!("{}", x as u128);
    }
    let (d, n) = shortest(x);
    let k = d.len() as i32;
    if k <= n && n <= 21 {
        let mut s = d;
        s.extend(std::iter::repeat('0').take((n - k) as usize));
        s
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &d[..n as usize], &d[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{}", "0".repeat((-n) as usize), d)
    } else {
        let e = n - 1;
        let sign = if e >= 0 { '+' } else { '-' };
        if k == 1 {
            format!("{}e{}{}", d, sign, e.abs())
        } else {
            format!("{}.{}e{}{}", &d[..1], &d[1..], sign, e.abs())
        }
    }
}

/// Rounds a digit string (half up) to `keep` digits; returns digits and whether it overflowed
/// to an extra leading digit.
fn round_digits(digits: &str, keep: usize) -> (String, bool) {
    let bytes: Vec<u8> = digits.bytes().collect();
    if bytes.len() <= keep {
        let mut s = digits.to_string();
        s.extend(std::iter::repeat('0').take(keep - bytes.len()));
        return (s, false);
    }
    let mut out: Vec<u8> = bytes[..keep].to_vec();
    if bytes[keep] >= b'5' {
        let mut i = keep;
        loop {
            if i == 0 {
                out.insert(0, b'1');
                return (String::from_utf8(out).unwrap_or_default(), true);
            }
            i -= 1;
            if out[i] == b'9' {
                out[i] = b'0';
            } else {
                out[i] += 1;
                break;
            }
        }
    }
    (String::from_utf8(out).unwrap_or_default(), false)
}

/// Exact decimal expansion: (digits, point position) for |x|.
fn exact(x: f64) -> (String, i32) {
    let s = format!("{:.1100e}", x.abs());
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    (digits, exp.parse::<i32>().unwrap_or(0) + 1)
}

pub fn to_fixed(x: f64, f: usize) -> String {
    if !x.is_finite() || x.abs() >= 1e21 {
        return number_to_string(x);
    }
    let neg = x < 0.0 && x != 0.0;
    let (digits, point) = exact(x);
    // digits represent 0.DIGITS * 10^point; we need `point + f` digits
    let keep = point + f as i32;
    let body = if keep < 0 {
        format_point(&"0".repeat(f + 1), 1, f)
    } else {
        let (mut r, carry) = round_digits(&digits, keep as usize);
        let point = if carry { point + 1 } else { point };
        let int_len = point.max(0) as usize;
        if point <= 0 {
            r = format!("{}{}", "0".repeat((-point) as usize + 1), r);
            let il = 1;
            format_point(&r, il, f)
        } else {
            format_point(&r, int_len, f)
        }
    };
    let body = if body.starts_with('.') { format!("0{}", body) } else { body };
    let is_zero = body.chars().all(|c| c == '0' || c == '.');
    if neg && !is_zero { format!("-{}", body) } else { body }
}

fn format_point(digits: &str, int_len: usize, f: usize) -> String {
    let int_part = &digits[..int_len.min(digits.len())];
    let frac = &digits[int_len.min(digits.len())..];
    let int_part = if int_part.is_empty() { "0" } else { int_part };
    if f == 0 {
        int_part.to_string()
    } else {
        format!("{}.{}", int_part, frac)
    }
}

pub fn to_exponential(x: f64, f: Option<usize>) -> String {
    if !x.is_finite() {
        return number_to_string(x);
    }
    let neg = x < 0.0;
    let (d, e) = if x == 0.0 {
        ("0".repeat(f.unwrap_or(0) + 1), 0)
    } else {
        match f {
            None => {
                let (d, n) = shortest(x.abs());
                (d, n - 1)
            }
            Some(f) => {
                let (digits, point) = exact(x);
                let (r, carry) = round_digits(&digits, f + 1);
                let r = if carry { r[..f + 1].to_string() } else { r };
                (r, if carry { point } else { point - 1 })
            }
        }
    };
    let mant = if d.len() > 1 { format!("{}.{}", &d[..1], &d[1..]) } else { d };
    let sign = if e >= 0 { '+' } else { '-' };
    format!("{}{}e{}{}", if neg { "-" } else { "" }, mant, sign, e.abs())
}

pub fn to_precision(x: f64, p: usize) -> String {
    if !x.is_finite() {
        return number_to_string(x);
    }
    let neg = x < 0.0;
    if x == 0.0 {
        return if p > 1 { format!("0.{}", "0".repeat(p - 1)) } else { "0".into() };
    }
    let (digits, point) = exact(x);
    let (r, carry) = round_digits(&digits, p);
    let (r, e) = if carry { (r[..p].to_string(), point) } else { (r, point - 1) };
    let s = if e < -6 || e >= p as i32 {
        let mant = if p > 1 { format!("{}.{}", &r[..1], &r[1..]) } else { r };
        format!("{}e{}{}", mant, if e >= 0 { '+' } else { '-' }, e.abs())
    } else if e >= 0 {
        let il = e as usize + 1;
        if il >= p { r } else { format!("{}.{}", &r[..il], &r[il..]) }
    } else {
        format!("0.{}{}", "0".repeat((-e - 1) as usize), r)
    };
    if neg { format!("-{}", s) } else { s }
}

pub fn to_radix(x: f64, radix: u32) -> String {
    if radix == 10 || !x.is_finite() {
        return number_to_string(x);
    }
    if x == 0.0 {
        return "0".into();
    }
    let neg = x < 0.0;
    let x = x.abs();
    let mut int = x.trunc();
    let mut frac = x - int;
    let mut ds = Vec::new();
    if int == 0.0 {
        ds.push('0');
    }
    while int >= 1.0 {
        let d = (int % radix as f64) as u32;
        ds.push(std::char::from_digit(d, radix).unwrap_or('0'));
        int = (int / radix as f64).trunc();
    }
    ds.reverse();
    let mut s: String = ds.into_iter().collect();
    if frac > 0.0 {
        s.push('.');
        let mut n = 0;
        while frac > 0.0 && n < 52 {
            frac *= radix as f64;
            let d = frac.trunc() as u32;
            s.push(std::char::from_digit(d, radix).unwrap_or('0'));
            frac -= d as f64;
            n += 1;
        }
    }
    if neg { format!("-{}", s) } else { s }
}

pub fn is_js_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// ToNumber applied to a string.
pub fn string_to_num(s: &str) -> f64 {
    let t = s.trim_matches(is_js_whitespace);
    if t.is_empty() {
        return 0.0;
    }
    let lower = t.to_ascii_lowercase();
    for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            if rest.is_empty() || !rest.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return rest.chars().fold(0.0, |a, d| a * radix as f64 + d.to_digit(radix).unwrap_or(0) as f64);
        }
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    if decimal_prefix_len(t) != t.len() {
        return f64::NAN;
    }
    parse_decimal(t)
}

fn parse_decimal(t: &str) -> f64 {
    let fixed = if t.starts_with('.') || t.starts_with("+.") || t.starts_with("-.") {
        t.replacen('.', "0.", 1)
    } else {
        t.to_string()
    };
    let fixed = if fixed.ends_with('.') { format!("{}0", fixed) } else { fixed };
    fixed.parse::<f64>().unwrap_or(f64::NAN)
}

/// Length of the longest prefix of `t` that is a StrDecimalLiteral (without Infinity).
pub fn decimal_prefix_len(t: &str) -> usize {
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - int_start;
    if i < b.len() && b[i] == b'.' {
        let save = i;
        i += 1;
        let fs = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        digits += i - fs;
        if digits == 0 {
            i = save;
        }
    }
    if digits == 0 {
        return 0;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let save = i;
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let es = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == es {
            i = save;
        }
    }
    i
}

pub fn parse_float(s: &str) -> f64 {
    let t = s.trim_start_matches(is_js_whitespace);
    for (p, v) in [("Infinity", f64::INFINITY), ("+Infinity", f64::INFINITY), ("-Infinity", f64::NEG_INFINITY)] {
        if t.starts_with(p) {
            return v;
        }
    }
    let n = decimal_prefix_len(t);
    if n == 0 {
        return f64::NAN;
    }
    parse_decimal(&t[..n])
}
