//! A3: hostile input must produce errors, never crash or exhaust the native stack.
//! These tests run on the default 2 MiB test-thread stack.

use invisium_js::{ErrorKind, Limits, Runtime};

fn kind_of(src: &str) -> ErrorKind {
    let mut rt = Runtime::new(Limits::default());
    match rt.eval(src, "hostile.js") {
        Ok(v) => panic!("expected an error, got {}", rt.to_string_lossy(v)),
        Err(e) => e.kind,
    }
}

#[test]
fn deeply_nested_parentheses_is_syntax_error() {
    let src = format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000));
    assert_eq!(kind_of(&src), ErrorKind::Syntax);
}

#[test]
fn deeply_nested_literals_and_blocks_are_syntax_errors() {
    assert_eq!(kind_of(&format!("{}{}", "[".repeat(100_000), "]".repeat(100_000))), ErrorKind::Syntax);
    assert_eq!(kind_of(&format!("{}{}", "{".repeat(100_000), "}".repeat(100_000))), ErrorKind::Syntax);
    assert_eq!(kind_of(&format!("{}1", "!".repeat(100_000))), ErrorKind::Syntax);
    assert_eq!(kind_of(&format!("1{}", "+1".repeat(200_000))), ErrorKind::Syntax);
    assert_eq!(kind_of(&format!("x = {}1{}", "f(".repeat(100_000), ")".repeat(100_000))), ErrorKind::Syntax);
}

#[test]
fn moderately_deep_nesting_still_works() {
    let mut rt = Runtime::new(Limits::default());
    let src = format!("{}1{}", "(".repeat(500), ")".repeat(500));
    let v = rt.eval(&src, "ok.js").expect("500 nested parens should parse");
    assert_eq!(rt.to_string_lossy(v), "1");
    let chain = format!("0{}", "+1".repeat(5000));
    let v = rt.eval(&chain, "ok.js").expect("long binary chains should parse");
    assert_eq!(rt.to_string_lossy(v), "5000");
}

#[test]
fn deep_recursion_is_stack_overflow() {
    assert_eq!(kind_of("function f() { return f() + 1; } f()"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("const o = { get x() { return this.x; } }; o.x"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("function g() { return [1].map(g); } g()"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("const o = {}; o.toString = function() { return String(this); }; String(o)"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("let a = JSON.parse('['.repeat(100000) + ']'.repeat(100000)); JSON.stringify(a)"), ErrorKind::StackOverflow);
}

#[test]
fn stack_overflow_is_catchable_by_script() {
    let mut rt = Runtime::new(Limits::default());
    let v = rt
        .eval("function f() { f(); } let r; try { f(); } catch (e) { r = e instanceof RangeError; } r", "so.js")
        .expect("caught");
    assert_eq!(rt.to_string_lossy(v), "true");
}

#[test]
fn infinite_loops_hit_the_step_budget() {
    assert_eq!(kind_of("while (true) {}"), ErrorKind::StepBudget);
    assert_eq!(kind_of("for (;;) { try { } finally { } }"), ErrorKind::StepBudget);
    let mut rt = Runtime::new(Limits { step_budget: 10_000, ..Limits::default() });
    let e = rt.eval("let n = 0; try { while (true) n++; } catch (e) { 'caught' }", "b.js").unwrap_err();
    assert_eq!(e.kind, ErrorKind::StepBudget, "budget exhaustion must not be catchable by script");
    let v = rt.eval("1 + 1", "after.js").unwrap();
    assert_eq!(rt.to_string_lossy(v), "2");
}

#[test]
fn allocation_loops_hit_the_heap_cap() {
    assert_eq!(kind_of("let a = []; while (true) a.push(new Array(1000).fill(1));"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("let s = 'x'; while (true) s += s;"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("let o = {}; for (let i = 0; ; i++) o['k' + i] = i;"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("'x'.repeat(1 << 27)"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("new Array(4294967295).fill(0)"), ErrorKind::OutOfMemory);
}

#[test]
fn garbage_input_never_panics() {
    let inputs = [
        "}{][", "/*", "`${", "'\\u{110000}'", "a => => b", "class extends {}", "((((", "))))", "...a",
        "let let = 1", "function (", "x = {a b}", "`${`${`${", "/[/", "0x", "1e", "08.5", "\\u0041 = 1", "\u{2028}",
        "@", "#", "for (let [a] of", "switch (1) { case }", "try {}", "new.target", "super()", "return 5",
        "a?.b`t`", "({a = 1})", "[a, b] += 1", "async function f() {}", "function* g() {}", "1n", "'\\x4'",
        "label: label: 1", "continue", "break foo", "if (1) let x = 1", "obj = {get a(x) {}}",
    ];
    for src in inputs {
        let mut rt = Runtime::new(Limits::default());
        let _ = rt.eval(src, "garbage.js");
    }
}
