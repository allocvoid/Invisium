//! Milestone 2b conformance suite: (source, expected result).
//! The program is evaluated with a fixed injected clock (2024-03-15T12:30:45.678Z), the job
//! queue is drained, then the result is the global `out` when the program defined one, else the
//! completion value; both through ToString. A thrown error renders as `<ErrorName>!`.

use invisium_js::{ErrorKind, Limits, Runtime, Value};

#[path = "conformance3/binary.rs"]
mod binary;
#[path = "conformance3/clone.rs"]
mod clone;
#[path = "conformance3/date.rs"]
mod date;
#[path = "conformance3/regex.rs"]
mod regex;
#[path = "conformance3/regex_replace.rs"]
mod regex_replace;

pub const CLOCK_MS: f64 = 1_710_505_845_678.0;

pub fn run(src: &str) -> String {
    let mut rt = Runtime::new(Limits::default());
    rt.set_clock(Box::new(|| CLOCK_MS));
    let fail = |e: invisium_js::JsError| format!("{}!", if e.name.is_empty() { "Thrown".to_string() } else { e.name });
    let v = match rt.eval(src, "case.js") {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let root = rt.root(v);
    if let Err(e) = rt.run_jobs() {
        return fail(e);
    }
    let out = match rt.eval("typeof out === 'undefined' ? undefined : out", "out.js") {
        Ok(o) => o,
        Err(e) => return fail(e),
    };
    let r = if out == Value::Undefined { rt.to_string_lossy(v) } else { rt.to_string_lossy(out) };
    rt.unroot(root);
    r
}

fn check(name: &str, cases: &[(&str, &str)]) -> usize {
    let mut failed = 0;
    for (src, want) in cases {
        let got = run(src);
        if got != *want {
            failed += 1;
            eprintln!("FAIL [{}]: {}\n   want: {}\n    got: {}", name, src, want, got);
        }
    }
    failed
}

#[test]
fn conformance_suite_2b() {
    let all: Vec<(&str, &[(&str, &str)])> = vec![("regex", regex::CASES), ("regex_replace", regex_replace::CASES), ("date", date::CASES), ("binary", binary::CASES), ("clone", clone::CASES)];
    let total: usize = all.iter().map(|c| c.1.len()).sum();
    let failed: usize = all.iter().map(|c| check(c.0, c.1)).sum();
    eprintln!("conformance 2b: {} cases, {} failed", total, failed);
    assert!(total >= 150, "suite must have at least 150 cases, has {}", total);
    assert_eq!(failed, 0, "{} of {} conformance cases failed", failed, total);
}

/// A catastrophic pattern ends on the step budget like any runaway loop, and the runtime
/// stays usable afterwards.
#[test]
fn catastrophic_pattern_stops_on_the_step_budget() {
    let mut rt = Runtime::new(Limits { step_budget: 3_000_000, ..Limits::default() });
    for src in [
        "/(a+)+$/.test('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa!')",
        "'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa!'.replace(/(a|aa)+$/, 'x')",
        "'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa!'.split(/(a*)*b/)",
        "try { /(x+x+)+y/.exec('x'.repeat(60)) } catch (e) { 'caught' }",
    ] {
        let e = rt.eval(src, "catastrophic.js").unwrap_err();
        assert_eq!(e.kind, ErrorKind::StepBudget, "{}", src);
    }
    let v = rt.eval("/(a+)+$/.test('aaa')", "after.js").unwrap();
    assert_eq!(rt.to_string_lossy(v), "true");
}
