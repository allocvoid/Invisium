//! Milestone 3d: the conformance corpus of milestones 1 to 3c, run again with GC stress on
//! (`set_gc_stress`): a collection at every point where the engine may collect at all. Each
//! program runs the way its own suite runs it (clock, `gc()`, how the result is read).

use invisium_js::{Limits, Runtime, Value};

#[path = "conformance/cases1.rs"]
mod c1_1;
#[path = "conformance/cases2.rs"]
mod c1_2;
#[path = "conformance/cases3.rs"]
mod c1_3;
#[path = "conformance/cases4.rs"]
mod c1_4;
#[path = "conformance/cases5.rs"]
mod c1_5;
#[path = "conformance2/async_fn.rs"]
mod c2_async;
#[path = "conformance2/collections.rs"]
mod c2_coll;
#[path = "conformance2/extra.rs"]
mod c2_extra;
#[path = "conformance2/generator.rs"]
mod c2_gen;
#[path = "conformance2/misc.rs"]
mod c2_misc;
#[path = "conformance2/promise.rs"]
mod c2_promise;
#[path = "conformance2/proxy.rs"]
mod c2_proxy;
#[path = "conformance3/binary.rs"]
mod c3_binary;
#[path = "conformance3/clone.rs"]
mod c3_clone;
#[path = "conformance3/date.rs"]
mod c3_date;
#[path = "conformance3/regex.rs"]
mod c3_regex;
#[path = "conformance3/regex_replace.rs"]
mod c3_replace;
#[path = "conformance4/bigint.rs"]
mod c4_bigint;
#[path = "conformance4/bigint2.rs"]
mod c4_bigint2;
#[path = "conformance4/builtins.rs"]
mod c4_builtins;
#[path = "conformance4/builtins2.rs"]
mod c4_builtins2;
#[path = "conformance4/compat.rs"]
mod c4_compat;
#[path = "conformance4/errors.rs"]
mod c4_errors;
#[path = "conformance4/eval.rs"]
mod c4_eval;
#[path = "conformance4/syntax.rs"]
mod c4_syntax;
#[path = "conformance4/syntax2.rs"]
mod c4_syntax2;
#[path = "conformance5/intl_dt.rs"]
mod c5_dt;
#[path = "conformance5/intl_misc.rs"]
mod c5_misc;
#[path = "conformance5/intl_nf.rs"]
mod c5_nf;
#[path = "conformance5/intl_pc.rs"]
mod c5_pc;
#[path = "conformance5/intl_rl.rs"]
mod c5_rl;
#[path = "conformance6/ident.rs"]
mod c6_ident;
#[path = "conformance6/promise_cap.rs"]
mod c6_cap;

/// How a suite reads its result after running the program and draining the job queue.
#[derive(Clone, Copy)]
enum Out {
    /// The completion value (milestone 1: no job drain either).
    Completion,
    /// The global `out` when defined, else the completion value.
    OutOrCompletion,
    /// `String(typeof out === 'function' ? out() : out)`.
    OutCalled,
}

#[derive(Clone, Copy)]
struct Suite {
    clock: Option<f64>,
    gc_fn: bool,
    out: Out,
    drain: bool,
}

const M1: Suite = Suite { clock: None, gc_fn: false, out: Out::Completion, drain: false };
const M2A: Suite = Suite { clock: None, gc_fn: false, out: Out::OutOrCompletion, drain: true };
const M2B: Suite = Suite { clock: Some(1_710_505_845_678.0), gc_fn: false, out: Out::OutOrCompletion, drain: true };
const M3A: Suite = Suite { clock: None, gc_fn: true, out: Out::OutOrCompletion, drain: true };
const M3B: Suite = Suite { clock: Some(1709600823678.0), gc_fn: false, out: Out::Completion, drain: true };
const M3C: Suite = Suite { clock: None, gc_fn: false, out: Out::OutCalled, drain: true };

/// The stress interval used for every program.
const EVERY: u32 = 1;

fn run(s: Suite, src: &str, every: u32) -> (String, u64) {
    let mut rt = Runtime::new(Limits::default());
    if let Some(ms) = s.clock {
        rt.set_clock(Box::new(move || ms));
    }
    if s.gc_fn {
        let gc = rt.function("gc", 0, std::rc::Rc::new(|rt: &mut Runtime, _, _| {
            rt.gc();
            Ok(Value::Undefined)
        }));
        let global = rt.global();
        rt.set(global, "gc", gc).expect("install gc");
    }
    rt.set_gc_stress(every);
    let start = rt.gc_count();
    let fail = |e: invisium_js::JsError| format!("{}!", if e.name.is_empty() { "Thrown".to_string() } else { e.name });
    let r = (|| {
        let v = rt.eval(src, "case.js").map_err(fail)?;
        let root = rt.root(v);
        if s.drain {
            rt.run_jobs().map_err(fail)?;
        }
        let r = match s.out {
            Out::Completion => rt.to_string_lossy(v),
            Out::OutOrCompletion => {
                let out = rt.eval("typeof out === 'undefined' ? undefined : out", "out.js").map_err(fail)?;
                if out == Value::Undefined { rt.to_string_lossy(v) } else { rt.to_string_lossy(out) }
            }
            Out::OutCalled => {
                let o = rt.eval("String(typeof out === 'function' ? out() : out)", "read.js").map_err(fail)?;
                rt.to_string_lossy(o)
            }
        };
        rt.unroot(root);
        Ok::<String, String>(r)
    })();
    let n = rt.gc_count() - start;
    (r.unwrap_or_else(|e| e), n)
}

/// Programs whose expected result encodes *when* collections happen: they observe a
/// weakly-held target (WeakRef, FinalizationRegistry) dying only at their explicit `gc()`.
/// Under stress the target legitimately dies earlier, so they are not run here.
const SCHEDULE_DEPENDENT: &[&str] = &[
    "var r = new WeakRef({ a: 1 }); var before = typeof r.deref(); gc();",
    "var cyc = (function () { var a = {}, b = { a: a }; a.b = b; return new WeakRef(a) })();",
    "var tok = {}; (function () { reg.register({}, 'x', tok); reg.register({}, 'y') })(); var un = reg.unregister(tok);",
];

fn check(name: &str, s: Suite, cases: &[(&str, &str)]) -> (usize, usize, u64) {
    let mut failed = 0;
    let mut gcs = 0;
    let mut ran = 0;
    for (src, want) in cases {
        if SCHEDULE_DEPENDENT.iter().any(|p| src.contains(p)) {
            eprintln!("skipped (depends on the collection schedule): {}", src);
            continue;
        }
        ran += 1;
        let (got, n) = run(s, src, EVERY);
        gcs += n;
        if got != *want {
            failed += 1;
            eprintln!("STRESS FAIL [{}]: {}\n   want: {:?}\n    got: {:?}", name, src, want, got);
        }
    }
    (ran, failed, gcs)
}

fn suite(groups: &[(&str, Suite, &[(&str, &str)])]) {
    let t = std::time::Instant::now();
    let (mut total, mut failed, mut gcs) = (0, 0, 0);
    for (name, s, cases) in groups {
        let (n, f, g) = check(name, *s, cases);
        total += n;
        failed += f;
        gcs += g;
    }
    eprintln!("gc stress (every {}): {} programs, {} failed, {} collections, {:?}", EVERY, total, failed, gcs, t.elapsed());
    assert_eq!(failed, 0, "{} of {} programs failed under GC stress", failed, total);
}

#[test]
fn stress_corpus_m1() {
    suite(&[("m1/1", M1, c1_1::CASES), ("m1/2", M1, c1_2::CASES), ("m1/3", M1, c1_3::CASES), ("m1/4", M1, c1_4::CASES), ("m1/5", M1, c1_5::CASES)]);
}

#[test]
fn stress_corpus_m2a() {
    suite(&[
        ("promise", M2A, c2_promise::CASES),
        ("generator", M2A, c2_gen::CASES),
        ("async", M2A, c2_async::CASES),
        ("collections", M2A, c2_coll::CASES),
        ("proxy", M2A, c2_proxy::CASES),
        ("misc", M2A, c2_misc::CASES),
        ("extra", M2A, c2_extra::CASES),
    ]);
}

#[test]
fn stress_corpus_m2b() {
    suite(&[("regex", M2B, c3_regex::CASES), ("regex_replace", M2B, c3_replace::CASES), ("date", M2B, c3_date::CASES), ("binary", M2B, c3_binary::CASES), ("clone", M2B, c3_clone::CASES)]);
}

#[test]
fn stress_corpus_m3a() {
    suite(&[
        ("errors", M3A, c4_errors::CASES),
        ("eval", M3A, c4_eval::CASES),
        ("builtins", M3A, c4_builtins::CASES),
        ("builtins2", M3A, c4_builtins2::CASES),
        ("syntax", M3A, c4_syntax::CASES),
        ("syntax2", M3A, c4_syntax2::CASES),
        ("compat", M3A, c4_compat::CASES),
        ("bigint", M3A, c4_bigint::CASES),
        ("bigint2", M3A, c4_bigint2::CASES),
    ]);
}

#[test]
fn stress_corpus_m3b() {
    suite(&[("datetime", M3B, c5_dt::CASES), ("number", M3B, c5_nf::CASES), ("plural_collator", M3B, c5_pc::CASES), ("reltime_list", M3B, c5_rl::CASES), ("misc", M3B, c5_misc::CASES)]);
}

#[test]
fn stress_corpus_m3c() {
    suite(&[("ident", M3C, c6_ident::CASES), ("promise_cap", M3C, c6_cap::CASES)]);
}
