//! Milestone 2a conformance suite: (source, expected result).
//! The program is evaluated, the job queue is drained with `run_jobs`, then the result is the
//! global `out` when the program defined one, else the completion value; both through ToString.
//! A thrown error renders as `<ErrorName>!`.

use invisium_js::{Limits, Runtime, Value};

#[path = "conformance2/async_fn.rs"]
mod async_fn;
#[path = "conformance2/collections.rs"]
mod collections;
#[path = "conformance2/extra.rs"]
mod extra;
#[path = "conformance2/generator.rs"]
mod generator;
#[path = "conformance2/misc.rs"]
mod misc;
#[path = "conformance2/promise.rs"]
mod promise;
#[path = "conformance2/proxy.rs"]
mod proxy;

pub fn run(src: &str) -> String {
    let mut rt = Runtime::new(Limits::default());
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
fn conformance_suite_2a() {
    let all: Vec<(&str, &[(&str, &str)])> = vec![("promise", promise::CASES), ("generator", generator::CASES), ("async", async_fn::CASES), ("collections", collections::CASES), ("proxy", proxy::CASES), ("misc", misc::CASES), ("extra", extra::CASES)];
    let total: usize = all.iter().map(|c| c.1.len()).sum();
    let failed: usize = all.iter().map(|c| check(c.0, c.1)).sum();
    eprintln!("conformance 2a: {} cases, {} failed", total, failed);
    assert!(total >= 150, "suite must have at least 150 cases, has {}", total);
    assert_eq!(failed, 0, "{} of {} conformance cases failed", failed, total);
}
