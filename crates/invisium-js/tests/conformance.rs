//! Table-driven conformance suite: (source, expected result).
//! A result is the completion value converted with ToString; a thrown error renders as
//! `<ErrorName>!` (e.g. `TypeError!`), a syntax error as `SyntaxError!`.

use invisium_js::{Limits, Runtime, Value};

#[path = "conformance/cases1.rs"]
mod cases1;
#[path = "conformance/cases2.rs"]
mod cases2;
#[path = "conformance/cases3.rs"]
mod cases3;
#[path = "conformance/cases4.rs"]
mod cases4;
#[path = "conformance/cases5.rs"]
mod cases5;

pub fn run(src: &str) -> String {
    let mut rt = Runtime::new(Limits::default());
    match rt.eval(src, "case.js") {
        Ok(v) => match v {
            Value::String(_) => rt.to_string_lossy(v),
            other => rt.to_string_lossy(other),
        },
        Err(e) => format!("{}!", if e.name.is_empty() { "Thrown".to_string() } else { e.name }),
    }
}

fn check(cases: &[(&str, &str)]) -> usize {
    let mut failed = 0;
    for (src, want) in cases {
        let got = run(src);
        if got != *want {
            failed += 1;
            eprintln!("FAIL: {}\n   want: {}\n    got: {}", src, want, got);
        }
    }
    failed
}

#[test]
fn conformance_suite() {
    let all: Vec<&[(&str, &str)]> = vec![cases1::CASES, cases2::CASES, cases3::CASES, cases4::CASES, cases5::CASES];
    let total: usize = all.iter().map(|c| c.len()).sum();
    let failed: usize = all.iter().map(|c| check(c)).sum();
    eprintln!("conformance: {} cases, {} failed", total, failed);
    assert!(total >= 250, "suite must have at least 250 cases, has {}", total);
    assert_eq!(failed, 0, "{} of {} conformance cases failed", failed, total);
}
