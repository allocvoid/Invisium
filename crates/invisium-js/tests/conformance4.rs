//! Milestone 3a conformance suite: (source, expected result).
//! The program is evaluated as `case.js`, the job queue is drained, then the result is the
//! global `out` when the program defined one, else the completion value; both through ToString.
//! An uncaught error renders as `<ErrorName>!`.

use invisium_js::{Limits, Runtime, Value};

#[path = "conformance4/bigint.rs"]
mod bigint;
#[path = "conformance4/bigint2.rs"]
mod bigint2;
#[path = "conformance4/builtins.rs"]
mod builtins;
#[path = "conformance4/builtins2.rs"]
mod builtins2;
#[path = "conformance4/compat.rs"]
mod compat;
#[path = "conformance4/errors.rs"]
mod errors;
#[path = "conformance4/eval.rs"]
mod eval;
#[path = "conformance4/syntax.rs"]
mod syntax;
#[path = "conformance4/syntax2.rs"]
mod syntax2;

/// The charter asks for at least 250 further conformance programs.
const MIN_CASES: usize = 250;

pub fn run(src: &str) -> String {
    let mut rt = Runtime::new(Limits::default());
    // `gc()` lets a program force a collection (WeakRef / FinalizationRegistry cases)
    let gc = rt.function("gc", 0, std::rc::Rc::new(|rt: &mut Runtime, _, _| {
        rt.gc();
        Ok(Value::Undefined)
    }));
    let global = rt.global();
    rt.set(global, "gc", gc).expect("install gc");
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
fn conformance_suite_3a() {
    let all: Vec<(&str, &[(&str, &str)])> = vec![
        ("errors", errors::CASES),
        ("eval", eval::CASES),
        ("builtins", builtins::CASES),
        ("builtins2", builtins2::CASES),
        ("syntax", syntax::CASES),
        ("syntax2", syntax2::CASES),
        ("compat", compat::CASES),
        ("bigint", bigint::CASES),
        ("bigint2", bigint2::CASES),
    ];
    let total: usize = all.iter().map(|c| c.1.len()).sum();
    let failed: usize = all.iter().map(|c| check(c.0, c.1)).sum();
    eprintln!("conformance 3a: {} cases, {} failed", total, failed);
    assert_eq!(failed, 0, "{} of {} conformance cases failed", failed, total);
    assert!(total >= MIN_CASES, "suite must have at least {} cases, has {}", MIN_CASES, total);
}
