//! E4: embedding additions of Milestone 3a — `Runtime::compile` / `Script` / `Runtime::run`,
//! and the stack carried by `JsError`.

use invisium_js::{ErrorKind, Limits, Runtime, Script, Value};

#[test]
fn compile_once_run_in_two_runtimes_with_independent_globals() {
    let script: Script = Runtime::compile("var counter = (typeof counter === 'number' ? counter : 0) + 1; var tag = 'rt' + seed; counter", "boot.js").unwrap();
    let mut a = Runtime::new(Limits::default());
    let mut b = Runtime::new(Limits::default());
    let (ga, gb) = (a.global(), b.global());
    a.set(ga, "seed", Value::Number(1.0)).unwrap();
    b.set(gb, "seed", Value::Number(2.0)).unwrap();
    assert_eq!(a.run(&script).unwrap(), Value::Number(1.0));
    assert_eq!(a.run(&script).unwrap(), Value::Number(2.0));
    // the second runtime never saw the first one's globals
    assert_eq!(b.run(&script).unwrap(), Value::Number(1.0));
    let (ta, tb) = (a.get(ga, "tag").unwrap(), b.get(gb, "tag").unwrap());
    assert_eq!((a.to_string_lossy(ta), b.to_string_lossy(tb)), ("rt1".to_string(), "rt2".to_string()));
    // functions created by one run belong to that runtime only
    let lib = Runtime::compile("function twice(x) { return x * 2 } var made = { n: twice(21) }; made.n", "lib.js").unwrap();
    assert_eq!(a.run(&lib).unwrap(), Value::Number(42.0));
    let f = b.eval("typeof twice", "probe.js").unwrap();
    assert_eq!(b.to_string_lossy(f), "undefined");
}

#[test]
fn a_script_is_shared_cheaply_and_across_threads() {
    let script = Runtime::compile("var s = 0; for (var i = 1; i <= 100; i++) s += i; s", "sum.js").unwrap();
    let copy = script.clone();
    let handle = std::thread::spawn(move || {
        let mut rt = Runtime::new(Limits::default());
        rt.run(&copy).unwrap()
    });
    let mut rt = Runtime::new(Limits::default());
    assert_eq!(rt.run(&script).unwrap(), Value::Number(5050.0));
    assert_eq!(handle.join().unwrap(), Value::Number(5050.0));
}

#[test]
fn compile_reports_syntax_errors_and_run_reports_runtime_errors() {
    let e = match Runtime::compile("var x = ;", "bad.js") {
        Ok(_) => panic!("expected a syntax error"),
        Err(e) => e,
    };
    assert_eq!((e.kind, e.name.as_str(), e.line), (ErrorKind::Syntax, "SyntaxError", 1));
    assert!(e.stack.contains("bad.js:1:"), "stack: {}", e.stack);
    let script = Runtime::compile("function boom() { missing.call() }\nboom()", "run.js").unwrap();
    let mut rt = Runtime::new(Limits::default());
    let e = rt.run(&script).unwrap_err();
    assert_eq!((e.kind, e.name.as_str(), e.message.as_str(), e.line), (ErrorKind::Thrown, "ReferenceError", "missing is not defined", 1));
    assert!(e.stack.contains("at boom (run.js:1:") && e.stack.contains("at <anonymous> (run.js:2:"), "stack: {}", e.stack);
    // the runtime stays usable, and `eval` keeps working next to `run`
    assert_eq!(rt.eval("1 + 1", "after.js").unwrap(), Value::Number(2.0));
}

#[test]
fn js_error_stack_lists_every_frame_of_an_uncaught_error() {
    let mut rt = Runtime::new(Limits::default());
    let src = "function one() { throw new Error('three deep') }\nfunction two() { one() }\nfunction three() { two() }\nthree()";
    let e = rt.eval(src, "a.js").unwrap_err();
    assert_eq!((e.kind, e.name.as_str(), e.message.as_str()), (ErrorKind::Thrown, "Error", "three deep"));
    let lines: Vec<&str> = e.stack.lines().collect();
    assert_eq!(lines[0], "Error: three deep");
    assert!(lines[1].starts_with("    at one (a.js:1:"), "stack: {}", e.stack);
    assert!(lines[2].starts_with("    at two (a.js:2:"), "stack: {}", e.stack);
    assert!(lines[3].starts_with("    at three (a.js:3:"), "stack: {}", e.stack);
    assert!(lines[4].starts_with("    at <anonymous> (a.js:4:"), "stack: {}", e.stack);
    assert_eq!(lines.len(), 5);
    // an engine-raised error carries the same kind of stack, naming the culprit
    let e = rt.eval("var cfg = {};\nfunction init() { cfg.setup() }\ninit()", "b.js").unwrap_err();
    assert_eq!(e.message, "cfg.setup is not a function");
    assert!(e.stack.starts_with("TypeError: cfg.setup is not a function\n    at init (b.js:2:"), "stack: {}", e.stack);
    assert!(e.stack.contains("\n    at <anonymous> (b.js:3:"), "stack: {}", e.stack);
}

#[test]
fn aborts_carry_the_stack_they_happened_in() {
    let mut rt = Runtime::new(Limits { step_budget: 200_000, ..Limits::default() });
    let e = rt.eval("function spin() { for (;;) {} }\nfunction go() { spin() }\ngo()", "loop.js").unwrap_err();
    assert_eq!(e.kind, ErrorKind::StepBudget);
    assert!(e.stack.contains("at spin (loop.js:1:") && e.stack.contains("at go (loop.js:2:"), "stack: {}", e.stack);
    // the stack of a native calling back into script includes the script frames on both sides
    let e = rt.eval("function cb() { null.f }\nfunction run() { [1].forEach(cb) }\nrun()", "cb.js").unwrap_err();
    assert!(e.stack.contains("at cb (cb.js:1:") && e.stack.contains("at run (cb.js:2:"), "stack: {}", e.stack);
}

#[test]
fn stack_trace_building_stays_bounded_on_a_deep_stack() {
    let mut rt = Runtime::new(Limits::default());
    let v = rt.eval("function r(n) { return n ? r(n - 1) : new Error('deep').stack } r(390)", "deep.js").unwrap();
    let s = rt.to_string_lossy(v);
    assert_eq!(s.lines().count(), 11, "ten frames at most: {}", s);
    // 300 errors created at depth 390 cost 300 * 10 frame lines, not 300 * 390
    let v = rt.eval("function r(n) { if (n) return r(n - 1); var t = 0; for (var i = 0; i < 300; i++) t += new Error('e').stack.length; return t } r(390)", "deep2.js").unwrap();
    let Value::Number(total) = v else { panic!("number expected") };
    assert!(total < 300.0 * 400.0, "total stack text {}", total);
    // overflowing the stack still ends as StackOverflow with a bounded stack string
    let e = rt.eval("function inf() { return inf() + 1 } inf()", "inf.js").unwrap_err();
    assert_eq!(e.kind, ErrorKind::StackOverflow);
    assert!(e.stack.lines().count() <= 11, "stack: {}", e.stack);
}

#[test]
fn bigint_values_cross_the_embedding_boundary() {
    let mut rt = Runtime::new(Limits::default());
    let v = rt.eval("2n ** 64n", "big.js").unwrap();
    assert!(matches!(v, Value::BigInt(_)));
    assert_eq!(rt.type_of(v), "bigint");
    assert_eq!(rt.to_string_lossy(v), "18446744073709551616");
    assert!(rt.to_boolean(v));
    assert!(!rt.is_callable(v));
    // a rooted BigInt survives collections and can be handed back to script
    let root = rt.root(v);
    rt.gc();
    rt.eval("var junk = []; for (var i = 0; i < 1000; i++) junk.push(BigInt(i) << 200n); junk = null", "junk.js").unwrap();
    rt.gc();
    let g = rt.global();
    rt.set(g, "fromHost", v).unwrap();
    assert_eq!(rt.eval("fromHost + 1n === 2n ** 64n + 1n && typeof fromHost === 'bigint'", "t.js").unwrap(), Value::Bool(true));
    rt.unroot(root);
    // a native function receives BigInt arguments and may return them
    let echo = rt.function("echo", 1, std::rc::Rc::new(|rt: &mut Runtime, _, args: &[Value]| {
        let a = args.first().copied().unwrap_or(Value::Undefined);
        assert_eq!(rt.type_of(a), "bigint");
        Ok(a)
    }));
    rt.set(g, "echo", echo).unwrap();
    let r = rt.eval("echo(123456789012345678901234567890n) * 2n", "n.js").unwrap();
    assert_eq!(rt.to_string_lossy(r), "246913578024691357802469135780");
    // zero is falsy; ToNumber of a BigInt is an error in script, NaN through the lossy helper
    let zero = rt.eval("0n", "z.js").unwrap();
    assert!(!rt.to_boolean(zero));
    assert!(rt.to_number(v).is_nan());
}
