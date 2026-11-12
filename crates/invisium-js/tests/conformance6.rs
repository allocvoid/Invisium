//! Milestone 3c conformance suite: Unicode identifiers and promise capabilities with foreign
//! constructors. Each case is (source, expected). The program is evaluated, the job queue is
//! drained, then `out` is read (called first when it is a function) and converted to a string.
//! An uncaught error renders as `<ErrorName>!`.

use invisium_js::{Limits, Runtime};

#[path = "conformance6/ident.rs"]
mod ident;
#[path = "conformance6/promise_cap.rs"]
mod promise_cap;

fn run(src: &str) -> String {
    let mut rt = Runtime::new(Limits::default());
    let fail = |e: invisium_js::JsError| format!("{}!", if e.name.is_empty() { "Thrown".to_string() } else { e.name });
    if let Err(e) = rt.eval(src, "case.js") {
        return fail(e);
    }
    if let Err(e) = rt.run_jobs() {
        return fail(e);
    }
    match rt.eval("String(typeof out === 'function' ? out() : out)", "read.js") {
        Ok(v) => rt.to_string_lossy(v),
        Err(e) => fail(e),
    }
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
fn conformance_unicode_identifiers() {
    let failed = check("ident", ident::CASES);
    eprintln!("conformance 3c identifiers: {} cases, {} failed", ident::CASES.len(), failed);
    assert_eq!(failed, 0);
    assert!(ident::CASES.len() >= 40, "at least 40 identifier cases");
}

#[test]
fn conformance_promise_capabilities() {
    let failed = check("promise_cap", promise_cap::CASES);
    eprintln!("conformance 3c promise capabilities: {} cases, {} failed", promise_cap::CASES.len(), failed);
    assert_eq!(failed, 0);
    assert!(promise_cap::CASES.len() >= 30, "at least 30 promise capability cases");
}

/// The chess.com module: an object literal with a quoted Myanmar key and an unquoted one made
/// of U+1000 and the combining mark U+103A.
#[test]
fn chess_com_module_with_myanmar_keys() {
    let mut rt = Runtime::new(Limits::default());
    rt.set_module_resolver(Box::new(|spec: &str, _: &str| Some(spec.to_string())));
    rt.eval("var out;", "setup.js").expect("setup");
    let src = "const m = { \"\u{1031}\u{102b}\": \"aw\", \u{1000}\u{103a}: \"et\" }; out = m[\"\u{1031}\u{102b}\"] + m.\u{1000}\u{103a} + Object.keys(m).length;";
    rt.add_module("mem:/chess.js", src).expect("the module parses");
    rt.run_module("mem:/chess.js").expect("the module runs");
    rt.run_jobs().expect("jobs");
    let v = rt.eval("out", "read.js").unwrap();
    assert_eq!(rt.to_string_lossy(v), "awet2");
}
