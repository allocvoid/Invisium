//! Hostile input against the Milestone 2b features: errors, never a panic, a hang or memory
//! beyond the heap cap. These tests run on the default 2 MiB test-thread stack.

use invisium_js::{ErrorKind, JsError, Limits, Runtime};

fn run(src: &str) -> Result<String, JsError> {
    let mut rt = Runtime::new(Limits::default());
    let v = rt.eval(src, "hostile.js")?;
    Ok(rt.to_string_lossy(v))
}

fn kind_of(src: &str) -> ErrorKind {
    match run(src) {
        Ok(v) => panic!("expected an error from `{}`, got {}", src, v),
        Err(e) => e.kind,
    }
}

/// A catchable error of the given class.
fn throws(src: &str, name: &str) {
    match run(src) {
        Ok(v) => panic!("expected {} from `{}`, got {}", name, src, v),
        Err(e) => assert_eq!((e.kind, e.name.as_str()), (ErrorKind::Thrown, name), "{}", src),
    }
}

#[test]
fn buffer_allocations_respect_the_heap_cap() {
    // a request the cap can never hold is a RangeError and allocates nothing
    throws("new ArrayBuffer(1e12)", "RangeError");
    throws("new Uint8Array(2 ** 52)", "RangeError");
    throws("new Float64Array(1e15)", "RangeError");
    throws("new Uint8Array(2 ** 53)", "RangeError");
    throws("new ArrayBuffer(-1)", "RangeError");
    throws("Uint8Array.from({ length: 1e15 })", "RangeError");
    // buffers that are kept alive fill the heap like anything else
    assert_eq!(kind_of("var keep = []; for (;;) keep.push(new Uint8Array(1 << 20))"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("var keep = []; for (;;) keep.push(new ArrayBuffer(1 << 20).slice(0))"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("var b = new Uint8Array(24 << 20); var keep = []; for (;;) keep.push(structuredClone(b))"), ErrorKind::OutOfMemory);
    // ... and garbage buffers are collected: 600 MiB allocated in total under a 64 MiB cap
    assert_eq!(run("for (var i = 0; i < 600; i++) new Uint8Array(1 << 20); 'ok'").unwrap(), "ok");
    // listing the keys of a huge typed array would intern one name per element
    assert_eq!(kind_of("Object.keys(new Uint8Array(40 << 20))"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("for (var k in new Uint8Array(40 << 20)) {}"), ErrorKind::OutOfMemory);
    assert_eq!(kind_of("JSON.stringify(new Uint8Array(40 << 20))"), ErrorKind::OutOfMemory);
}

#[test]
fn odd_indices_offsets_and_lengths_never_panic() {
    let src = r#"
        var log = [];
        function attempt(f) { try { log.push(String(f())); } catch (e) { log.push(e.name); } }
        var buf = new ArrayBuffer(8), ta = new Uint8Array(buf), dv = new DataView(buf), empty = new Uint8Array(0);
        var odd = [NaN, -1, -0, 1.5, Infinity, -Infinity, 2 ** 32, 2 ** 53, 1e308, '7', 'x', null, undefined, {}, [], true];
        for (var k of odd) {
            attempt(() => { ta[k] = 1; return ta[k]; });
            attempt(() => empty[k]);
            attempt(() => new Uint16Array(buf, k));
            attempt(() => new Uint16Array(buf, 0, k).length);
            attempt(() => new Uint8Array(k).length);
            attempt(() => new DataView(buf, k).byteLength);
            attempt(() => new DataView(buf, 0, k).byteLength);
            attempt(() => dv.getFloat64(k));
            attempt(() => dv.setInt16(k, k));
            attempt(() => dv.getUint8(0, k));
            attempt(() => ta.subarray(k, k).length);
            attempt(() => ta.slice(k).length);
            attempt(() => ta.set([1], k));
            attempt(() => ta.set(k));
            attempt(() => ta.fill(k, k, k).length);
            attempt(() => ta.copyWithin(k, k, k).length);
            attempt(() => ta.at(k));
            attempt(() => ta.indexOf(k, k));
            attempt(() => buf.slice(k, k).byteLength);
            attempt(() => new Float32Array([k])[0]);
            attempt(() => Object.defineProperty(ta, k, { value: 1 })[0]);
            attempt(() => delete ta[k]);
            attempt(() => k in ta);
        }
        attempt(() => new Uint8Array({ length: -5 }).length);
        attempt(() => new Uint8Array({ get length() { throw new EvalError('x'); } }));
        attempt(() => ta.set({ length: 2, get 0() { throw new EvalError('x'); } }));
        attempt(() => ta.fill({ valueOf() { throw new EvalError('x'); } }));
        attempt(() => ta.map(() => { throw new EvalError('x'); }));
        attempt(() => ta.sort(() => { throw new EvalError('x'); }));
        attempt(() => Uint8Array.prototype.set.call({}, [1]));
        attempt(() => Object.getPrototypeOf(Uint8Array.prototype).length);
        attempt(() => DataView.prototype.getInt8.call(ta, 0));
        attempt(() => ArrayBuffer.prototype.slice.call(ta, 0));
        attempt(() => new Uint8Array(new ArrayBuffer(0)).subarray(0).slice(0).reverse().sort().fill(1).length);
        attempt(() => Reflect.ownKeys(new Float64Array(2)).join());
        attempt(() => Object.freeze(new Uint8Array(1))[0]);
        log.length + ':' + ta.length
    "#;
    assert_eq!(run(src).unwrap(), "381:8");
}

#[test]
fn hostile_dates_are_invalid_not_fatal() {
    let src = r#"
        var odd = [NaN, Infinity, -Infinity, 1e308, -1e308, 8.64e15, 8.64e15 + 1, -8.64e15 - 1, 2 ** 53, -0, 1.5, '12', 'x', null, undefined, {}, [], true];
        var out = 0;
        for (var a of odd) {
            for (var b of odd) {
                var d = new Date(a, b);
                d.setMonth(b); d.setDate(a); d.setHours(a, b); d.setFullYear(b, a, b); d.setMilliseconds(a); d.setTime(b); d.setYear(a);
                var u = Date.UTC(a, b, a, b, a, b, a);
                var s = [String(d), d.toUTCString(), d.toLocaleString(), d.toDateString(), d.toJSON(), d.getDay(), d.getTimezoneOffset(), u].join();
                try { d.toISOString(); } catch (e) { if (!(e instanceof RangeError)) throw e; }
                out += s.length > 0 ? 1 : 0;
            }
            new Date(a).toString(); new Date(String(a)).getTime(); Date.parse(a);
        }
        var junk = ['9'.repeat(5000), '2024-' + '1'.repeat(400), '99999999999999999999999 Mar 99999999999999', 'Mar 15 2024 99:99:99', 'Mar 15 2024 12:00 +99999999999',
            '2024-03-15T12:00:00.' + '9'.repeat(300) + 'Z', '((((((', 'Mar (' .repeat(100), '\u0000', '+275760-09-13T00:00:00.001Z', '-271821-04-19T23:59:59.999Z', '1/1/1 1:1:1 pm am', '12/31/99999999999'];
        for (var j of junk) out += isNaN(Date.parse(j)) ? 1 : 2;
        out
    "#;
    let v = run(src).unwrap();
    assert!(v.parse::<u32>().unwrap() >= 324 + 13, "{}", v);
    throws("new Date(Symbol())", "TypeError");
    throws("new Date(0).setHours({ valueOf() { throw new EvalError('v'); } })", "EvalError");
    throws("Date.prototype.toISOString.call(new Date(NaN))", "RangeError");
    throws("Date.prototype.getTime.call(1)", "TypeError");
}

#[test]
fn hostile_patterns_and_regexp_state() {
    // patterns the compiler must refuse (or accept) without recursing or exploding
    for src in [
        "new RegExp('('.repeat(100000))",
        "new RegExp('(a'.repeat(5000) + ')'.repeat(5000))",
        "new RegExp('[')",
        "new RegExp('a{2,1}')",
        "new RegExp('\\\\')",
        "new RegExp('(?<n>a)(?<n>b)')",
        "new RegExp('a', 'gg')",
        "new RegExp('a', 'z'.repeat(1000))",
    ] {
        throws(src, "SyntaxError");
    }
    let src = r#"
        var log = [];
        function attempt(f) { try { log.push(String(f())); } catch (e) { log.push(e.name); } }
        attempt(() => new RegExp('(?:a|b|c)'.repeat(20000)).test('abc'));
        attempt(() => new RegExp('a{99999}').test('a'));
        attempt(() => new RegExp('x'.repeat(200000)).test('x'));
        for (var li of [NaN, -1, 1e308, Infinity, -Infinity, 2 ** 53, 1.5, '2', null, undefined, {}]) {
            var re = /a/g; re.lastIndex = li; attempt(() => re.exec('aaa'));
            var st = /a/y; st.lastIndex = li; attempt(() => st.test('aaa'));
            var un = /\udc00/gu; un.lastIndex = li; attempt(() => un.exec('𐀀\udc00'));
        }
        var bad = /a/g; bad.lastIndex = { valueOf() { throw new EvalError('li'); } };
        attempt(() => bad.test('a'));
        attempt(() => 'a'.replace(bad, 'b'));
        attempt(() => RegExp.prototype[Symbol.replace].call({ exec() { return { index: -5, length: 1e9, 0: 'x' }; }, flags: '' }, 'abc', '$1$2$<q>'));
        attempt(() => RegExp.prototype[Symbol.replace].call({ exec() { return { index: 1e308, 0: { toString() { throw new EvalError('m'); } } }; } }, 'abc', 'z'));
        attempt(() => RegExp.prototype[Symbol.split].call({ constructor: undefined, flags: 'y', exec() { return null; } }, 'abc'));
        attempt(() => RegExp.prototype[Symbol.match].call({ flags: 'g', exec() { return null; }, lastIndex: 0 }, 'abc'));
        attempt(() => RegExp.prototype[Symbol.matchAll].call(1, 'abc'));
        attempt(() => RegExp.prototype.test.call({ exec: 5 }, 'abc'));
        attempt(() => 'abc'.replace(/(?<a>b)/, '$<' + 'a'.repeat(10000)));
        attempt(() => 'abc'.split(/b/, -1).length + ':' + 'abc'.split(/b/, NaN).length + ':' + 'abc'.split(/b/, 2 ** 32 + 1).length);
        attempt(() => 'x'.match({ [Symbol.match]: 5 }));
        attempt(() => [...'aaa'.matchAll({ [Symbol.matchAll]() { return [1, 2]; } })].join());
        attempt(() => '\ud800'.replace(/\ud800/u, '$&$&').length);
        log.length
    "#;
    assert_eq!(run(src).unwrap(), "49");
    // a replacer that re-enters replace without end
    assert_eq!(kind_of("function f(s) { return s.replace(/a/, f); } f('a')"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("var re = /a/g; re.exec = function (s) { return 'a'.replace(re, 'b'); }; re.test('a')"), ErrorKind::StackOverflow);
    // output that outgrows the heap while it is still being built
    let e = kind_of("'x'.repeat(1 << 20).replace(/x/g, 'y'.repeat(300))");
    assert!(matches!(e, ErrorKind::OutOfMemory | ErrorKind::Thrown), "{:?}", e);
    assert_eq!(kind_of("var re = /(?:)/g; var s = 'x'.repeat(1 << 16); var keep = []; for (;;) keep.push(s.split(re))"), ErrorKind::OutOfMemory);
    // the matcher's own backtrack-stack limit is an ordinary, catchable RangeError
    let deep = run("try { /(?:a|b)*c/.test('a'.repeat(4 << 20)) } catch (e) { e.name }").unwrap();
    assert!(deep == "RangeError" || deep == "false", "{}", deep);
    // a loop of cheap matches is still bounded by the step budget
    let mut rt = Runtime::new(Limits { step_budget: 2_000_000, ..Limits::default() });
    assert_eq!(rt.eval("for (;;) /a/.test('a')", "loop.js").unwrap_err().kind, ErrorKind::StepBudget);
    assert_eq!(rt.eval("var s = 'ab'.repeat(1 << 21); s.replace(/a/g, '').length", "big.js").unwrap_err().kind, ErrorKind::StepBudget);
}

#[test]
fn structured_clone_depth_size_and_cycles() {
    assert_eq!(kind_of("var a = []; for (var i = 0; i < 200000; i++) a = [a]; structuredClone(a)"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("var o = {}; for (var i = 0; i < 200000; i++) o = { o: o }; structuredClone(o)"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("var m = new Map(); for (var i = 0; i < 50000; i++) m = new Map([[i, m]]); structuredClone(m)"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("var s = new Set(); for (var i = 0; i < 50000; i++) s = new Set([s]); structuredClone({ deep: [s] })"), ErrorKind::StackOverflow);
    assert_eq!(kind_of("var a = new Array(100000).fill(0).map((_, i) => ({ i })); var keep = []; for (;;) keep.push(structuredClone(a))"), ErrorKind::OutOfMemory);
    assert_eq!(run("var a = []; a.push(a, a, [a]); var s = new Set(); s.add(s); var m = new Map(); m.set(m, m); var c = structuredClone({ a, s, m }); (c.a[2][0] === c.a) + ':' + c.s.has(c.s) + ':' + (c.m.get(c.m) === c.m)").unwrap(), "true:true:true");
    throws("structuredClone({ get x() { throw new EvalError('getter'); } })", "EvalError");
    let mut rt = Runtime::new(Limits { step_budget: 200_000, ..Limits::default() });
    rt.eval("var big = []; for (var i = 0; i < 20000; i++) big.push([i]);", "setup.js").unwrap();
    assert_eq!(rt.eval("for (;;) structuredClone(big)", "clone.js").unwrap_err().kind, ErrorKind::StepBudget);
}
