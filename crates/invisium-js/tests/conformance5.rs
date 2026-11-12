//! Milestone 3b `Intl` conformance suite: (source, expected result).
//! The program is evaluated as `case.js`, the job queue is drained, then the result is the
//! completion value through ToString. An uncaught error renders as `<ErrorName>!`.

use invisium_js::{Limits, Runtime};

#[path = "conformance5/intl_dt.rs"]
mod intl_dt;
#[path = "conformance5/intl_nf.rs"]
mod intl_nf;
#[path = "conformance5/intl_pc.rs"]
mod intl_pc;
#[path = "conformance5/intl_rl.rs"]
mod intl_rl;
#[path = "conformance5/intl_misc.rs"]
mod intl_misc;

/// The charter asks for at least 150 `Intl` conformance programs.
const MIN_CASES: usize = 150;

pub fn run(src: &str) -> String {
    let mut rt = Runtime::new(Limits::default());
    // a fixed clock, so `format()` with no argument is deterministic
    rt.set_clock(Box::new(|| 1709600823678.0));
    let fail = |e: invisium_js::JsError| format!("{}!", if e.name.is_empty() { "Thrown".to_string() } else { e.name });
    let v = match rt.eval(src, "case.js") {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let root = rt.root(v);
    if let Err(e) = rt.run_jobs() {
        return fail(e);
    }
    let r = rt.to_string_lossy(v);
    rt.unroot(root);
    r
}

fn check(name: &str, cases: &[(&str, &str)]) -> usize {
    let mut failed = 0;
    for (src, want) in cases {
        let got = run(src);
        if got != *want {
            failed += 1;
            eprintln!("FAIL [{}]: {}\n   want: {:?}\n    got: {:?}", name, src, want, got);
        }
    }
    failed
}

#[test]
fn conformance_suite_intl() {
    let all: Vec<(&str, &[(&str, &str)])> = vec![("datetime", intl_dt::CASES), ("number", intl_nf::CASES), ("plural_collator", intl_pc::CASES), ("reltime_list", intl_rl::CASES), ("misc", intl_misc::CASES)];
    let total: usize = all.iter().map(|c| c.1.len()).sum();
    let failed: usize = all.iter().map(|c| check(c.0, c.1)).sum();
    eprintln!("conformance 3b intl: {} cases, {} failed", total, failed);
    assert_eq!(failed, 0, "{} of {} conformance cases failed", failed, total);
    assert!(total >= MIN_CASES, "suite must have at least {} cases, has {}", MIN_CASES, total);
}
