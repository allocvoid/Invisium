//! E3: hostile input against the Milestone 3a features — errors, never a panic, a hang or
//! memory beyond the heap cap. These tests run on the default 2 MiB test-thread stack.

use invisium_js::{ErrorKind, JsError, Limits, Runtime};

fn run_with(limits: Limits, src: &str) -> Result<String, JsError> {
    let mut rt = Runtime::new(limits);
    let v = rt.eval(src, "hostile.js")?;
    let r = rt.to_string_lossy(v);
    rt.run_jobs()?;
    Ok(r)
}

fn run(src: &str) -> Result<String, JsError> {
    run_with(Limits::default(), src)
}

fn kind_of(src: &str) -> ErrorKind {
    match run(src) {
        Ok(v) => panic!("expected an error from `{}`, got {}", src, v),
        Err(e) => e.kind,
    }
}

#[test]
fn eval_recursion_is_stack_overflow() {
    // a string that evals itself: directly, indirectly, and through the Function constructor
    assert_eq!(kind_of("var s = 'eval(s)'; eval(s)"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("var s = '(0, eval)(s)'; (0, eval)(s)"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("function f() { return eval('f()') } f()"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("var s = 'new Function(s)()'; new Function(s)()"), ErrorKind::StackOverflow);
    // ... and the overflow is an ordinary RangeError for the script
    assert_eq!(run("var s = 'eval(s)'; try { eval(s) } catch (e) { e.name + '|' + (e instanceof RangeError) }").unwrap(), "RangeError|true");
}

#[test]
fn eval_of_deeply_nested_source_is_a_syntax_error() {
    for src in [
        "try { eval('('.repeat(100000) + '1' + ')'.repeat(100000)) } catch (e) { e.name }",
        "function f() { var local = 1; try { return eval('['.repeat(100000) + ']'.repeat(100000)) } catch (e) { return e.name } } f()",
        "try { (0, eval)('{'.repeat(100000) + '}'.repeat(100000)) } catch (e) { e.name }",
        "try { eval('with({}) '.repeat(100000) + ';') } catch (e) { e.name }",
        "try { eval('-'.repeat(200000) + '1') } catch (e) { e.name }",
        "try { new Function('return ' + '!'.repeat(200000) + '1') } catch (e) { e.name }",
    ] {
        assert_eq!(run(src).unwrap(), "SyntaxError", "{}", src);
    }
    // the runtime is still fine afterwards
    assert_eq!(run("try { eval('(') } catch (e) {} eval('1 + 1')").unwrap(), "2");
}

#[test]
fn eval_loops_stop_on_the_step_budget_or_the_heap_cap() {
    let small = Limits { step_budget: 400_000, ..Limits::default() };
    // every compilation is charged to the budget, so a loop of them ends quickly
    let e = run_with(small, "for (;;) eval('1')").unwrap_err();
    assert_eq!(e.kind, ErrorKind::StepBudget);
    let e = run_with(small, "function f() { var n = 0; for (;;) eval('n++') } f()").unwrap_err();
    assert_eq!(e.kind, ErrorKind::StepBudget);
    let e = run_with(small, "for (;;) new Function('return 1')").unwrap_err();
    assert_eq!(e.kind, ErrorKind::StepBudget);
    // eval-declared variables are heap objects like any other
    let e = run_with(small, "function f() { for (var i = 0; ; i++) eval('var v' + i + ' = [i, i, i]') } f()").unwrap_err();
    assert!(matches!(e.kind, ErrorKind::StepBudget | ErrorKind::OutOfMemory), "{:?}", e.kind);
    // source text that doubles every round
    let r = run("var s = '1'; try { for (;;) s = eval('s + s') } catch (e) { e.name }");
    assert!(matches!(&r, Ok(n) if n == "RangeError") || matches!(&r, Err(e) if e.kind == ErrorKind::OutOfMemory), "{:?}", r);
}

#[test]
fn with_over_a_recursing_proxy_is_stack_overflow() {
    assert_eq!(kind_of("var p = new Proxy({}, { has(t, k) { with (p) { return x } } }); with (p) { x }"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("var p = new Proxy({}, { has(t, k) { return k in p } }); with (p) { anything }"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("var p = new Proxy({}, { has() { return true }, get(t, k) { with (p) { return y } } }); with (p) { y }"), ErrorKind::StackOverflow);
    assert_eq!(run("var p = new Proxy({}, { has(t, k) { with (p) { return x } } }); try { with (p) { x } } catch (e) { e.name }").unwrap(), "RangeError");
    // a deep chain of `with` scopes is only as deep as the script wrote it
    assert_eq!(run("var src = ''; for (var i = 0; i < 200; i++) src += 'with ({ v' + i + ': ' + i + ' }) '; eval(src + 'v0 + v199')").unwrap(), "199");
    assert_eq!(kind_of("with ({ get g() { with (this) { return g } } }) { g }"), ErrorKind::StackOverflow);
}

#[test]
fn weak_references_and_registries_stay_bounded() {
    let small = Limits { step_budget: 3_000_000, ..Limits::default() };
    for src in [
        "for (;;) new WeakRef({})",
        "var reg = new FinalizationRegistry(() => {}); for (;;) reg.register({}, 1)",
        "var reg = new FinalizationRegistry(function again() { reg.register({}, 1) }); reg.register({}, 1); var keep = []; for (;;) keep.push(new WeakRef({}))",
    ] {
        let e = run_with(small, src).unwrap_err();
        assert!(matches!(e.kind, ErrorKind::StepBudget | ErrorKind::OutOfMemory), "{}: {:?}", src, e.kind);
    }
    // a cleanup callback that throws or recurses does not take the runtime down
    let mut rt = Runtime::new(Limits::default());
    rt.eval("var reg = new FinalizationRegistry(function cb(h) { if (h === 'throw') throw new Error('in cleanup'); cb(h) }); (function () { reg.register({}, 'throw'); reg.register({}, 'recurse') })()", "reg.js").unwrap();
    rt.gc();
    assert!(rt.has_pending_jobs());
    rt.run_jobs().unwrap();
    let msgs = rt.take_unhandled_rejections();
    assert_eq!(msgs.len(), 2, "{:?}", msgs);
    let v = rt.eval("1 + 1", "after.js").unwrap();
    assert_eq!(rt.to_string_lossy(v), "2");
}

#[test]
fn class_and_argument_machinery_survives_abuse() {
    let small = Limits { step_budget: 3_000_000, ..Limits::default() };
    for src in [
        "for (;;) { class A { #x = 1; static #y; #m() {} } new A() }",
        "function f(a) { for (;;) { arguments[0] = [a]; } } f(1)",
        "function* g(a = g().next()) {} g()",
        "function f() { { function f() { return f() } } return f() } f()",
        "var it = { [Symbol.iterator]() { return this }, next() { return { done: false } }, return() { throw 1 } }; for (;;) { try { for (var v of it) throw 2 } catch (e) {} }",
        "var o = {}; o.__defineGetter__('g', function () { return this.g }); o.g",
        "var a = {}; for (;;) { var b = {}; b.__proto__ = a; a = b; a.x }",
    ] {
        let e = run_with(small, src).unwrap_err();
        assert!(matches!(e.kind, ErrorKind::StepBudget | ErrorKind::OutOfMemory | ErrorKind::StackOverflow), "{}: {:?}", src, e.kind);
    }
    // nothing below may panic, whatever it returns or throws
    let src = r#"
        var log = [];
        function attempt(f) { try { log.push(String(f())); } catch (e) { log.push(e.name); } }
        attempt(() => String.fromCodePoint.apply(null, new Array(70000).fill(0x10FFFF)).length);
        attempt(() => 'é'.repeat(100000).normalize('NFD').length);
        attempt(() => ('a' + '́'.repeat(50000)).normalize('NFC').length);
        attempt(() => '𐀀\udfff\ud800'.normalize('NFKD').length);
        attempt(() => unescape('%u%uD800%u12G4%').length);
        attempt(() => [1, 2, 3].copyWithin(-1e9, 1e9, -Infinity).join());
        attempt(() => [1, 2, 3].toSpliced(NaN, Infinity, ...new Array(1000)).length);
        attempt(() => Array.prototype.toSpliced.call({ length: 2 ** 40 }, 0, 0));
        attempt(() => Object.groupBy({ [Symbol.iterator]() { return { next() { throw new Error('x') } } } }, x => x));
        attempt(() => new WeakRef(new Proxy({}, {})).deref() instanceof Object);
        attempt(() => Error.captureStackTrace(Object.freeze({})));
        attempt(() => { Error.stackTraceLimit = 1e9; return new Error('x').stack.split('\n').length });
        attempt(() => { Error.stackTraceLimit = 'many'; return new Error('x').stack.split('\n').length });
        attempt(() => { Error.stackTraceLimit = -5; return new Error('x').stack });
        attempt(() => /a/.compile('('.repeat(5000)));
        attempt(() => eval('class A { ' + '#p; '.repeat(3000) + '}').name);
        log.join('|')
    "#;
    let out = run(src).unwrap();
    assert_eq!(out.split('|').count(), 16, "{}", out);
    assert!(out.starts_with("140000|200000|"), "{}", out);
}

#[test]
fn giant_bigint_operations_end_as_errors() {
    fn outcome(src: &str) -> String {
        match run(src) {
            Ok(v) => format!("ok:{}", v),
            Err(e) if e.kind == ErrorKind::Thrown => e.name,
            Err(e) => format!("{:?}", e.kind),
        }
    }
    // results past the size cap are RangeErrors, decided before any work is done
    for src in ["2n ** 10000000n", "var x = 1n; for (;;) x <<= 100000n", "(1n << 4000000n) * (1n << 4000000n)", "BigInt.asUintN(2 ** 53 - 1, -1n)", "1n << (2n ** 80n)", "(-3n) ** (2n ** 70n)", "new BigInt64Array(2 ** 40)"] {
        assert_eq!(outcome(src), "RangeError", "{}", src);
    }
    // quadratic work is charged to the step budget instead of running for minutes
    for src in [
        "var x = 3n; for (;;) x *= x",
        "(1n << 4000000n).toString().length",
        "((1n << 2000000n) - 1n) * ((1n << 2000000n) - 1n)",
        "(1n << 2000000n) / ((1n << 1000000n) + 1n)",
        "BigInt('9'.repeat(1200000))",
        "eval('1' + '0'.repeat(1200000) + 'n')",
        "var x = 7n ** 30000n; for (;;) x % (x - 1n)",
    ] {
        let o = outcome(src);
        assert!(o == "StepBudget" || o == "RangeError", "{}: {}", src, o);
    }
    // big values that are kept fill the heap like anything else ...
    let o = outcome("var big = 1n << 4000000n; var keep = []; for (;;) keep.push(big + BigInt(keep.length))");
    assert_eq!(o, "OutOfMemory");
    // ... and garbage ones are collected: 250 MiB of BigInts in total under the 64 MiB cap
    assert_eq!(outcome("var x = 1n << 1000000n; for (var i = 0; i < 2000; i++) x = x + 1n; (x - (1n << 1000000n)).toString()"), "ok:2000");
    // sizeable but legitimate arithmetic still works, and the runtime survives the aborts
    assert_eq!(outcome("(10n ** 20000n).toString().length"), "ok:20001");
    assert_eq!(outcome("var a = (1n << 20000n) - 1n; (a * a / a === a) + '|' + (a % (a - 1n))"), "ok:true|1");
    let mut rt = Runtime::new(Limits::default());
    assert_eq!(rt.eval("var x = 3n; for (;;) x *= x", "spin.js").unwrap_err().kind, ErrorKind::StepBudget);
    let v = rt.eval("(2n ** 64n).toString()", "after.js").unwrap();
    assert_eq!(rt.to_string_lossy(v), "18446744073709551616");
}

/// Linear BigInt operations (add, bitwise, compare, wrap, ...) are charged by operand size,
/// so a loop over values near the size cap runs into the step budget quickly.
#[test]
fn loops_over_huge_bigints_stop_on_the_step_budget() {
    let started = std::time::Instant::now();
    // two values of about 4 million bits, under the 2^22-bit cap
    let setup = "var x = (1n << 4000000n) - 1n, y = (1n << 3999999n) + 12345n, z = x + 0n;";
    for body in [
        "for (;;) x = x + y",
        "for (;;) x ^ y",
        "for (;;) x < y",
        "for (;;) x === z",
        "for (;;) -x",
        "for (;;) x >> 1n",
        "for (;;) 1n << 4000000n",
        "for (;;) BigInt.asUintN(4000000, x)",
        "for (;;) BigInt.asIntN(64, x)",
        "for (;;) BigInt(Object(x))",
        "var o = Object(x); for (;;) o.valueOf()",
        "var m = new Map(); for (;;) m.has(x)",
        "var list = [z]; for (;;) list.includes(x)",
    ] {
        let src = format!("{} {}", setup, body);
        assert_eq!(kind_of(&src), ErrorKind::StepBudget, "{}", body);
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "took {:?}", started.elapsed());
    // small values are not affected: a million cheap operations cost a million-odd steps
    assert_eq!(run("var n = 0n; for (var i = 0; i < 1000000; i++) { n = n + 1n; if (n < 0n) break } n").unwrap(), "1000000");
}

/// One native call that compares or keys a huge BigInt once per element must not run
/// unchecked: the per-element charge is settled inside the loop.
#[test]
fn native_loops_over_huge_bigints_stop_on_the_step_budget() {
    let started = std::time::Instant::now();
    // x and z are ~4-million-bit values that differ only in a top bit: comparing them reads
    // every limb and never matches (equal values would end the search at index 0)
    let setup = "var x = (1n << 4000000n) - 1n, z = x - (1n << 3999990n);";
    for body in [
        "var a = Array(1e6).fill(z); a.indexOf(x)",
        "var a = Array(1e6).fill(z); a.lastIndexOf(x)",
        "var a = Array(1e6).fill(z); a.includes(x)",
        "new Set(Array(1e6).fill(x))",
        "new Map(Array(1e5).fill([x, 1]))",
        "var s = new Set([z]); Array(1e6).fill(x).forEach(s.has, s)",
        "Map.groupBy(Array(1e6).fill(1), () => x)",
    ] {
        let src = format!("{} {}", setup, body);
        assert_eq!(kind_of(&src), ErrorKind::StepBudget, "{}", body);
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "took {:?}", started.elapsed());
}

#[test]
fn source_text_kept_alive_by_eval_counts_toward_the_heap_cap() {
    // each closure keeps its megabyte of source alive: the heap cap stops the hoarding
    let pad = "var pad = '/*' + 'x'.repeat(1 << 20) + '*/';";
    for body in ["keep.push(eval('(function () {' + pad + '})'))", "keep.push(new Function(pad + 'return 1'))", "keep.push((0, eval)('(function () {' + pad + '})'))"] {
        let src = format!("{} var keep = []; for (;;) {}", pad, body);
        assert_eq!(kind_of(&src), ErrorKind::OutOfMemory, "{}", body);
    }
    // source whose closures are dropped is reclaimed: 400 MiB of it in total under the 64 MiB cap
    let src = format!("{} var n = 0; for (var i = 0; i < 100; i++) n += eval('(function () {{' + pad + 'return 1 }})')(); n", pad);
    assert_eq!(run(&src).unwrap(), "100");
}
