//! B4: hostile input against the Milestone 2a features must produce errors, never crash or
//! exhaust the native stack. These tests run on the default 2 MiB test-thread stack.

use invisium_js::{ErrorKind, Limits, Runtime};

/// Evaluates `src` and drains the job queue; returns the error kind of whichever step failed.
fn kind_of(src: &str) -> ErrorKind {
    let mut rt = Runtime::new(Limits::default());
    match rt.eval(src, "hostile.js") {
        Ok(v) => match rt.run_jobs() {
            Ok(()) => panic!("expected an error, got {}", rt.to_string_lossy(v)),
            Err(e) => e.kind,
        },
        Err(e) => e.kind,
    }
}

#[test]
fn infinite_promise_chain_stops_on_the_step_budget_in_run_jobs() {
    let mut rt = Runtime::new(Limits::default());
    rt.eval("function f(){ Promise.resolve().then(f) } f()", "chain.js").expect("the chain only starts in eval");
    assert!(rt.has_pending_jobs());
    let e = rt.run_jobs().unwrap_err();
    assert_eq!(e.kind, ErrorKind::StepBudget);
    // the runtime stays usable afterwards
    let v = rt.eval("1 + 1", "after.js").unwrap();
    assert_eq!(rt.to_string_lossy(v), "2");
    rt.run_jobs().unwrap();
}

/// Like `kind_of`, with a smaller step budget so that budget tests stay fast.
fn kind_of_small(src: &str) -> ErrorKind {
    let mut rt = Runtime::new(Limits { step_budget: 2_000_000, ..Limits::default() });
    match rt.eval(src, "hostile.js") {
        Ok(v) => match rt.run_jobs() {
            Ok(()) => panic!("expected an error, got {}", rt.to_string_lossy(v)),
            Err(e) => e.kind,
        },
        Err(e) => e.kind,
    }
}

#[test]
fn microtask_floods_hit_a_limit() {
    assert_eq!(kind_of_small("function f(){ queueMicrotask(f) } f()"), ErrorKind::StepBudget);
    assert_eq!(kind_of_small("const t = { then(r) { r(t); } }; Promise.resolve(t)"), ErrorKind::StepBudget);
    assert_eq!(kind_of_small("(async function f() { for (;;) await null; })()"), ErrorKind::StepBudget);
    assert_eq!(kind_of_small("async function* g() { for (;;) yield 1; } (async () => { for await (const x of g()) {} })()"), ErrorKind::StepBudget);
    assert_eq!(kind_of("for (;;) queueMicrotask(() => {})"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("const p = new Promise(() => {}); for (;;) p.then(() => {})"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("const keep = []; for (;;) keep.push((async () => { await new Promise(() => {}); })())"), ErrorKind::OutOfMemory);
}

#[test]
fn generator_recursion_is_stack_overflow() {
    // a generator that recurses into itself, through yield*, next() and spread
    assert_eq!(kind_of("function* g() { yield* g(); } g().next()"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("function* g() { yield g().next().value; } g().next()"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("function* g() { yield [...g()]; } [...g()]"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("function* g() { for (const x of g()) yield x; } for (const x of g()) {}"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("const o = { *[Symbol.iterator]() { yield* o; } }; Array.from(o)"), ErrorKind::StackOverflow);
    // the overflow is an ordinary RangeError inside script
    let mut rt = Runtime::new(Limits::default());
    let v = rt.eval("function* g() { yield* g(); } let r; try { g().next(); } catch (e) { r = e instanceof RangeError; } r", "so.js").unwrap();
    assert_eq!(rt.to_string_lossy(v), "true");
}

#[test]
fn proxy_trap_recursion_is_stack_overflow() {
    // a trap that re-enters itself through the receiver / the proxy
    assert_eq!(kind_of("const p = new Proxy({}, { get(t, k, r) { return r[k]; } }); p.x"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("const p = new Proxy({}, { set(t, k, v, r) { r[k] = v; return true; } }); p.x = 1"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("const p = new Proxy({}, { has(t, k) { return k in p; } }); 'x' in p"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("const p = new Proxy({}, { ownKeys() { return Object.keys(p); } }); Object.keys(p)"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("const p = new Proxy({}, { deleteProperty(t, k) { return delete p[k]; } }); delete p.x"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("const p = new Proxy({}, { getOwnPropertyDescriptor(t, k) { return Object.getOwnPropertyDescriptor(p, k); } }); Object.getOwnPropertyDescriptor(p, 'x')"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("const p = new Proxy({}, { defineProperty(t, k, d) { return Reflect.defineProperty(p, k, d); } }); Object.defineProperty(p, 'x', { value: 1 })"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("const p = new Proxy(function () {}, { apply() { return p(); } }); p()"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("const p = new Proxy(function () {}, { construct() { return new p(); } }); new p()"), ErrorKind::StackOverflow);
    // the handler itself is a proxy that forwards to itself
    assert_eq!(kind_of("const h = new Proxy({}, { get(t, k, r) { return r[k]; } }); new Proxy({}, h).x"), ErrorKind::StackOverflow);
    // a very long chain of trap-less proxies recurses natively inside the engine
    let chain = "let p = {}; for (let i = 0; i < 200000; i++) p = new Proxy(p, {});";
    for tail in ["p.x", "p.x = 1", "'x' in p", "delete p.x", "Object.keys(p)", "Object.getOwnPropertyDescriptor(p, 'x')", "Object.defineProperty(p, 'x', { value: 1 })", "JSON.stringify(p)", "for (const k in p) {}"] {
        assert_eq!(kind_of(&format!("{} {}", chain, tail)), ErrorKind::StackOverflow, "chain then `{}`", tail);
    }
    assert_eq!(kind_of("let f = function () {}; for (let i = 0; i < 200000; i++) f = new Proxy(f, {}); f()"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("let f = function () {}; for (let i = 0; i < 200000; i++) f = new Proxy(f, {}); new f()"), ErrorKind::StackOverflow);
    // iterative walks over the same chain terminate
    let mut rt = Runtime::new(Limits::default());
    let v = rt.eval(&format!("{} [Array.isArray(p), Object.getPrototypeOf(p) === Object.prototype, typeof p].join()", chain), "walk.js").unwrap();
    assert_eq!(rt.to_string_lossy(v), "false,true,object");
    // the overflow is catchable and the runtime keeps working
    let v = rt.eval("const q = new Proxy({}, { get(t, k, r) { return r[k]; } }); let r; try { q.x; } catch (e) { r = e instanceof RangeError; } r", "so.js").unwrap();
    assert_eq!(rt.to_string_lossy(v), "true");
}

#[test]
fn proxy_and_collection_loops_respect_limits() {
    assert_eq!(kind_of_small("const p = new Proxy({}, { get: () => 1 }); for (;;) p.x;"), ErrorKind::StepBudget);
    // iterating a collection that keeps moving its only entry never ends on its own
    assert_eq!(kind_of_small("const m = new Map([[0, 0]]); for (const [k] of m) { m.delete(k); m.set(k + 1, 0); }"), ErrorKind::StepBudget);
    assert_eq!(kind_of_small("const s = new Set([0]); s.forEach(v => { s.delete(v); s.add(v + 1); });"), ErrorKind::StepBudget);
    // ... and one that keeps growing while iterated runs into the heap cap
    assert_eq!(kind_of("const m = new Map([[0, 0]]); for (const [k] of m) m.set(k + 1, 0);"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("const m = new Map(); for (let i = 0; ; i++) m.set(i, 'v' + i);"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("const s = new Set(); for (let i = 0; ; i++) s.add({ i });"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("const wm = new WeakMap(); const keep = []; for (;;) { const k = {}; keep.push(k); wm.set(k, [1, 2, 3]); }"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("Reflect.ownKeys(new Proxy({}, { ownKeys: () => ({ length: 1e9 }) }))"), ErrorKind::Thrown);
}

#[test]
fn garbage_input_with_new_syntax_never_panics() {
    let inputs = [
        "async", "async function", "async function*", "async () =>", "async x =>", "async (", "async (a, b", "async\nfunction f(){}",
        "function*", "function* g() { yield", "function* g() { yield* }", "function* g() { yield yield yield }", "yield 1", "await 1",
        "async function f() { await }", "async function f() { for await (x in y) {} }", "for await (const x of y) {}", "async function f() { for await (;;) {} }",
        "({ async })", "({ async, get, set, static })", "({ async *[x]() {} })", "({ *() {} })", "({ async get x() {} })", "class A { async }",
        "class A { *constructor() {} }", "class A { async *#x() {} }", "class A { static async *[Symbol.iterator]() { yield* this; } }",
        "async () => await", "async function f(await) {}", "function* g(yield) {}", "function* g() { var yield; }", "async(1)(2)`t`",
        "async function f() { (await 1)++ }", "function* g() { yield = 1 }", "function* g() { (yield) = 1 }", "x = async", "async => async",
        "new Proxy", "new Proxy()", "Reflect.get()", "new Map([[]])", "new Promise", "Promise.all()", "function* g() { try { yield } finally { yield } }",
    ];
    for src in inputs {
        let mut rt = Runtime::new(Limits { step_budget: 100_000, ..Limits::default() });
        let _ = rt.eval(src, "garbage.js");
        let _ = rt.run_jobs();
    }
    // deep nesting through the new constructs is still a syntax error, not a native overflow
    let mut rt = Runtime::new(Limits::default());
    let deep_await = format!("async function f() {{ {}1 }}", "await ".repeat(100_000));
    assert_eq!(rt.eval(&deep_await, "deep.js").unwrap_err().kind, ErrorKind::Syntax);
    let deep_yield = format!("function* g() {{ {}1 }}", "yield ".repeat(100_000));
    assert_eq!(rt.eval(&deep_yield, "deep.js").unwrap_err().kind, ErrorKind::Syntax);
    let deep_arrow = format!("{}1", "async () => ".repeat(100_000));
    assert_eq!(rt.eval(&deep_arrow, "deep.js").unwrap_err().kind, ErrorKind::Syntax);
}

#[test]
fn generator_loops_and_suspended_frames_respect_limits() {
    assert_eq!(kind_of_small("function* g() { for (;;) yield 1; } for (const x of g()) {}"), ErrorKind::StepBudget);
    assert_eq!(kind_of_small("function* g() { for (;;) yield 1; } const it = g(); for (;;) it.next();"), ErrorKind::StepBudget);
    assert_eq!(kind_of_small("function* g() { try { for (;;) yield 1; } finally { for (;;) {} } } const it = g(); it.next(); it.return()"), ErrorKind::StepBudget);
    assert_eq!(kind_of("function* g() { const big = new Array(1000).fill(0); yield big; } const keep = []; for (;;) { const it = g(); it.next(); keep.push(it); }"), ErrorKind::OutOfMemory);
    // deep async recursion ends as a rejection, not a crash, and the runtime stays usable
    let mut rt = Runtime::new(Limits::default());
    rt.eval("var out; async function r() { await r(); } r().catch(e => { out = e instanceof RangeError; });", "deep.js").unwrap();
    rt.run_jobs().unwrap();
    let v = rt.eval("out", "deep.js").unwrap();
    assert_eq!(rt.to_string_lossy(v), "true");
    // a suspended generator survives garbage collection with its frame intact
    rt.eval("function* g() { let local = { n: 1 }; for (;;) { local.n += yield local.n; } } var it = g(); it.next();", "gc.js").unwrap();
    rt.gc();
    rt.eval("var junk = []; for (let i = 0; i < 20000; i++) junk.push({ i }); junk = null;", "gc.js").unwrap();
    rt.gc();
    let v = rt.eval("it.next(5).value + it.next(10).value", "gc.js").unwrap();
    assert_eq!(rt.to_string_lossy(v), "22");
}
