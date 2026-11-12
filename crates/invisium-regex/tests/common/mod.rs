//! Shared helpers for the integration tests.
#![allow(dead_code)]

use invisium_regex::{Captures, ExecError, Flags, Regex, SyntaxError};

/// `(pattern, flags, subject, start, expected)`. `expected` is [`NO`], [`ERR`], or the capture
/// spans in code units: `"0,3 1,2 -"` is group 0 = [0,3), group 1 = [1,2), group 2 unset.
pub type Case = (&'static str, &'static str, &'static str, usize, &'static str);

pub const NO: &str = "no match";
pub const ERR: &str = "syntax error";

/// Step budget for ordinary cases.
pub const BUDGET: u64 = 10_000_000;

/// UTF-16 code units of `s`. `¦` followed by four hex digits is one raw code unit, so tests
/// can write lone surrogates (`¦D83D`).
pub fn u16s(s: &str) -> Vec<u16> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '¦' && i + 4 < chars.len() {
            let hex: String = chars[i + 1..i + 5].iter().collect();
            if let Ok(unit) = u16::from_str_radix(&hex, 16) {
                out.push(unit);
                i += 5;
                continue;
            }
        }
        let mut buf = [0u16; 2];
        out.extend_from_slice(chars[i].encode_utf16(&mut buf));
        i += 1;
    }
    out
}

pub fn compile(pattern: &str, flags: &str) -> Result<Regex, SyntaxError> {
    let flags = Flags::parse(flags).unwrap_or_else(|| panic!("bad flags {flags:?} in a test"));
    Regex::new(&u16s(pattern), flags)
}

pub fn regex(pattern: &str, flags: &str) -> Regex {
    compile(pattern, flags).unwrap_or_else(|e| panic!("/{pattern}/{flags} did not compile: {e}"))
}

pub fn spans(caps: &Captures) -> String {
    let parts: Vec<String> = (0..caps.len())
        .map(|g| match caps.get(g) {
            Some((s, e)) => format!("{s},{e}"),
            None => "-".to_string(),
        })
        .collect();
    parts.join(" ")
}

/// Run one case and describe the outcome in the `expected` notation.
pub fn outcome(pattern: &str, flags: &str, subject: &str, start: usize) -> String {
    let re = match compile(pattern, flags) {
        Ok(re) => re,
        Err(_) => return ERR.to_string(),
    };
    let mut steps = BUDGET;
    match re.exec(&u16s(subject), start, &mut steps) {
        Ok(Some(caps)) => spans(&caps),
        Ok(None) => NO.to_string(),
        Err(ExecError::StepLimit) => "step limit".to_string(),
        Err(ExecError::StackLimit) => "stack limit".to_string(),
    }
}

/// Check a whole table, reporting every failing row at once.
pub fn check(name: &str, table: &[Case]) {
    let mut failures = Vec::new();
    for (i, &(pattern, flags, subject, start, expected)) in table.iter().enumerate() {
        let got = outcome(pattern, flags, subject, start);
        if got != expected {
            failures.push(format!("  {name}[{i}] /{pattern}/{flags} on {subject:?} from {start}: expected {expected:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} rows failed:\n{}", failures.len(), table.len(), failures.join("\n"));
}

/// The matched text of each group (`None` = unset), or `None` if there is no match.
pub fn groups(re: &Regex, subject: &str, start: usize) -> Option<Vec<Option<String>>> {
    let text = u16s(subject);
    let mut steps = BUDGET;
    let caps = re.exec(&text, start, &mut steps).expect("exec failed")?;
    Some((0..caps.len()).map(|g| caps.get(g).map(|(s, e)| String::from_utf16_lossy(&text[s..e]))).collect())
}

/// Every non-overlapping match of group 0, the way a `g` loop would find them.
pub fn find_all(re: &Regex, subject: &str) -> Vec<String> {
    let text = u16s(subject);
    let mut out = Vec::new();
    let mut at = 0;
    let mut steps = u64::MAX;
    while let Some(caps) = re.exec(&text, at, &mut steps).expect("exec failed") {
        let (s, e) = caps.get(0).expect("group 0 is always set");
        out.push(String::from_utf16_lossy(&text[s..e]));
        at = if e == s { e + 1 } else { e };
    }
    out
}
