//! Milestone 3d: the stress switch itself, the reported page pattern (S4 item 3), and one
//! small test per rooting bug the stress runs found. Each bug test fails without its fix.

use invisium_js::{Limits, Runtime, Value};

fn stressed() -> Runtime {
    let mut rt = Runtime::new(Limits::default());
    rt.set_gc_stress(1);
    rt
}

fn eval_str(rt: &mut Runtime, src: &str) -> String {
    match rt.eval(src, "t.js") {
        Ok(v) => rt.to_string_lossy(v),
        Err(e) => format!("{}!", e.name),
    }
}

const SMALL: &str = "var a = []; for (var i = 0; i < 20; i++) a.push({ i: i }); a.map(o => o.i).join().length";

#[test]
fn stress_makes_collections_happen_where_none_would_otherwise() {
    let mut rt = Runtime::new(Limits::default());
    let before = rt.gc_count();
    assert_eq!(eval_str(&mut rt, SMALL), "49");
    assert_eq!(rt.gc_count(), before, "a small program on a fresh runtime does not collect");

    let mut rt = stressed();
    let before = rt.gc_count();
    assert_eq!(eval_str(&mut rt, SMALL), "49");
    let every1 = rt.gc_count() - before;
    assert!(every1 >= 40, "every = 1 collects at each loop iteration and call, got {}", every1);

    let mut rt = Runtime::new(Limits::default());
    rt.set_gc_stress(7);
    let before = rt.gc_count();
    assert_eq!(eval_str(&mut rt, SMALL), "49");
    let every7 = rt.gc_count() - before;
    assert!(every7 > 0 && every7 < every1, "every = 7 collects less often: {} vs {}", every7, every1);

    rt.set_gc_stress(0);
    let before = rt.gc_count();
    assert_eq!(eval_str(&mut rt, SMALL), "49");
    assert_eq!(rt.gc_count(), before, "0 turns stress off");
    rt.gc();
    assert_eq!(rt.gc_count(), before + 1, "gc() counts");
}

/// Item 3: `F.prototype = { ... }; new F().text()` for 2,000 rounds, with allocation pressure
/// from script and from the embedder, timer-style re-entry through `Runtime::call`, promise
/// jobs between the rounds, and a collection at every collection point.
#[test]
fn reported_pattern_prototype_literal_under_stress() {
    let mut rt = stressed();
    rt.eval(
        "function F(n) { this.n = n } \
         F.prototype = { text() { return 't' + this.n }, attr(k) { return k + '=' + this.n } }; \
         var junk = [], last = ''; \
         function tick(i, label, extra) { \
           var f = new F(i); var r = f.text() + ',' + f.attr(label) + ',' + extra.tag; \
           for (var j = 0; j < 8; j++) junk.push({ j: j, s: 'x' + j }); \
           if (junk.length > 400) junk = []; \
           Promise.resolve(new F(i + 1)).then(g => { last = g.text() + g.attr('p') }); \
           return r }",
        "setup.js",
    )
    .expect("setup");
    let global = rt.global();
    let tick = rt.get(global, "tick").unwrap();
    let tick_root = rt.root(tick);
    let start = rt.gc_count();
    for i in 0..2000u32 {
        // the embedder's own allocation pressure; values passed straight into `call`
        let _pressure = rt.array(&[Value::Number(i as f64)]);
        let extra = rt.object();
        let tag = rt.string(&format!("tag{}", i));
        rt.set(extra, "tag", tag).unwrap();
        let label = rt.string(&format!("k{}", i));
        let r = rt.call(tick, Value::Undefined, &[Value::Number(i as f64), label, extra]).expect("tick");
        assert_eq!(rt.to_string_lossy(r), format!("t{},k{}={},tag{}", i, i, i, i));
        rt.run_jobs().expect("jobs");
        let last = rt.get(global, "last").unwrap();
        assert_eq!(rt.to_string_lossy(last), format!("t{}p={}", i + 1, i + 1));
    }
    rt.unroot(tick_root);
    assert!(rt.gc_count() - start > 20_000, "stress collected {} times", rt.gc_count() - start);
}

// ---- one test per bug ----

/// Bug: `new F()` allocated `this`, then `push_frame` ran its safepoint before `this` was in
/// any frame; a collection there freed it and `new F()` returned a dead handle.
#[test]
fn bug_new_this_collected_before_the_frame_exists() {
    let mut rt = stressed();
    assert_eq!(eval_str(&mut rt, "function F() {} F.prototype = { text() { return 'ok' } }; new F().text()"), "ok");
    assert_eq!(eval_str(&mut rt, "class A { hi() { return 'hi' } } new A().hi()"), "hi");
}

/// Bug: the embedding API collected at entry (`host_enter`) before its value arguments were
/// rooted, so a value made with `string` / `object` and passed straight in was freed.
#[test]
fn bug_embedder_arguments_collected_at_entry() {
    let mut rt = stressed();
    let f = rt.eval("(function (s, o) { return s + o.k })", "f.js").unwrap();
    let froot = rt.root(f);
    let o = rt.object();
    let k = rt.string("def");
    rt.set(o, "k", k).unwrap();
    let s = rt.string("abc");
    let r = rt.call(f, Value::Undefined, &[s, o]).unwrap();
    assert_eq!(rt.to_string_lossy(r), "abcdef");
    let v = rt.eval("'x'.repeat(3) + 'y'", "v.js").unwrap();
    assert_eq!(rt.to_string_lossy(v), "xxxy");
    rt.unroot(froot);
}

/// Bug: prototypes the engine keeps only in `Intrinsics` (the `Intl` segments prototype, and
/// `BigInt.prototype` once `BigInt` is deleted) were not in the root set.
#[test]
fn bug_intrinsic_prototypes_missing_from_the_root_set() {
    let mut rt = stressed();
    assert_eq!(eval_str(&mut rt, "[...new Intl.Segmenter().segment('ab')].map(s => s.segment).join()"), "a,b");
    let mut rt = stressed();
    assert_eq!(eval_str(&mut rt, "delete globalThis.BigInt; [1, 2].map(x => ({ x })); (5n).toString() + (6n).toString(16)"), "56");
}

/// Bug: `Array.from(iterable, mapFn)` kept the iterated items only in a Rust vector while it
/// called `mapFn`.
#[test]
fn bug_iterated_items_unrooted_while_native_calls_script() {
    let mut rt = stressed();
    assert_eq!(eval_str(&mut rt, "Array.from('a=1&b=2'.matchAll(/(\\w)=(\\d)/g), m => m[1] + m[2]).join()"), "a1,b2");
    assert_eq!(eval_str(&mut rt, "Array.from((function* () { yield { a: 1 }; yield { a: 2 } })(), o => [o.a]).join()"), "1,2");
}

/// Bug: a value that script handed back to a native (the result of `call_value` /
/// `construct_value`: a species-constructed array, a getter's result) lived only in the
/// native's Rust locals while it called script again.
#[test]
fn bug_script_results_unrooted_inside_natives() {
    let mut rt = stressed();
    assert_eq!(eval_str(&mut rt, "class MyArr extends Array {} var a = new MyArr(1, 2, 3); [a.map(x => x) instanceof MyArr, a.filter(x => x).length].join()"), "true,3");
    assert_eq!(eval_str(&mut rt, "class MyU8 extends Uint8Array {} var u = new MyU8([1, 2, 3]); u.map(x => x * 2).join()"), "2,4,6");
}

/// Bug: `a + b` (and `<`, and BigInt arithmetic) kept the first operand's primitive only in a
/// Rust local while converting the second ran script.
#[test]
fn bug_first_operand_primitive_collected_during_second_conversion() {
    let cases = [
        ("var o1 = { toString() { return 'x' + 'y' } }, o2 = { toString() { return 'z' + 'w' } }; o1 + o2", "xyzw"),
        ("var a = { valueOf() { return 'a' + 'a' } }, b = { valueOf() { return 'b' + 'b' } }; a < b", "true"),
        ("var a = { valueOf() { return 2n ** 70n } }, b = { valueOf() { return BigInt(1) } }; String(a - b)", "1180591620717411303423"),
    ];
    let mut failed = Vec::new();
    for (src, want) in cases {
        let mut rt = stressed();
        let got = eval_str(&mut rt, src);
        if got != want {
            failed.push(format!("{} -> {}", src, got));
        }
    }
    assert!(failed.is_empty(), "{:#?}", failed);
}
