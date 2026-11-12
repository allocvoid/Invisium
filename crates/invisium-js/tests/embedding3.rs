//! Milestone 2b embedding surface: `array_buffer` / `buffer_bytes`, buffers and compiled
//! patterns in the heap accounting, and `Date` driven by the injected clock.

use invisium_js::{Limits, Runtime, Value};
use std::cell::Cell;
use std::rc::Rc;

fn text(rt: &mut Runtime, src: &str) -> String {
    let v = rt.eval(src, "test.js").unwrap_or_else(|e| panic!("{}: {}", src, e));
    rt.to_string_lossy(v)
}

#[test]
fn array_buffer_round_trips_bytes() {
    let mut rt = Runtime::new(Limits::default());
    let buf = rt.array_buffer(&[1, 2, 3, 4, 250]);
    let root = rt.root(buf);
    let global = rt.global();
    rt.set(global, "buf", buf).unwrap();
    assert_eq!(text(&mut rt, "Object.prototype.toString.call(buf) + buf.byteLength + (buf instanceof ArrayBuffer)"), "[object ArrayBuffer]5true");
    assert_eq!(text(&mut rt, "var u = new Uint8Array(buf); u[0] = 9; u.length + ':' + u[4]"), "5:250");
    // script writes are visible to the host
    assert_eq!(rt.buffer_bytes(buf), Some(vec![9, 2, 3, 4, 250]));
    let empty = rt.array_buffer(&[]);
    assert_eq!(rt.buffer_bytes(empty), Some(Vec::new()));
    rt.unroot(root);
}

#[test]
fn buffer_bytes_accepts_any_view_and_nothing_else() {
    let mut rt = Runtime::new(Limits::default());
    let v = rt.eval("new Uint16Array([0x0102, 0x0304]).subarray(1)", "t.js").unwrap();
    assert_eq!(rt.buffer_bytes(v), Some(vec![4, 3]));
    let v = rt.eval("new DataView(new Uint8Array([1, 2, 3, 4]).buffer, 1, 2)", "t.js").unwrap();
    assert_eq!(rt.buffer_bytes(v), Some(vec![2, 3]));
    let v = rt.eval("new Float32Array([1])", "t.js").unwrap();
    assert_eq!(rt.buffer_bytes(v), Some(vec![0, 0, 128, 63]));
    let v = rt.eval("new Uint8Array(0)", "t.js").unwrap();
    assert_eq!(rt.buffer_bytes(v), Some(Vec::new()));
    for src in ["({ length: 2 })", "[1, 2]", "'bytes'", "42", "undefined", "new Date(0)"] {
        let v = rt.eval(src, "t.js").unwrap();
        assert_eq!(rt.buffer_bytes(v), None, "{}", src);
    }
    assert_eq!(rt.buffer_bytes(Value::Null), None);
}

#[test]
fn buffers_count_toward_the_heap_and_are_collected() {
    let mut rt = Runtime::new(Limits::default());
    rt.gc();
    let before = rt.heap_bytes();
    rt.eval("globalThis.big = new ArrayBuffer(8 << 20); globalThis.view = new Float64Array(1 << 19);", "t.js").unwrap();
    rt.gc();
    assert!(rt.heap_bytes() >= before + (12 << 20), "8 MiB + 4 MiB of buffers must be accounted");
    rt.eval("big = null; view = null;", "t.js").unwrap();
    rt.gc();
    assert!(rt.heap_bytes() < before + (1 << 20), "dropped buffers must be reclaimed");
    // a buffer made by the host is accounted the same way
    let buf = rt.array_buffer(&vec![7u8; 4 << 20]);
    let root = rt.root(buf);
    rt.gc();
    assert!(rt.heap_bytes() >= before + (4 << 20));
    rt.unroot(root);
    rt.gc();
    assert!(rt.heap_bytes() < before + (1 << 20));
}

#[test]
fn compiled_patterns_are_accounted_and_released() {
    let mut rt = Runtime::new(Limits::default());
    rt.gc();
    let before = rt.heap_bytes();
    // 20 000 distinct patterns: the cache must not keep them once the RegExp objects are gone
    assert_eq!(text(&mut rt, "var n = 0; for (var i = 0; i < 20000; i++) { if (new RegExp('a' + i + '[xyz]{2,5}(b|c)+').test('a' + i + 'xyb')) n++; } n"), "20000");
    rt.gc();
    assert!(rt.heap_bytes() < before + (2 << 20), "idle compiled patterns must be dropped: {} -> {}", before, rt.heap_bytes());
    // a live RegExp keeps its program, and the program shows up in the accounting
    rt.eval("globalThis.keep = new RegExp('(?:' + 'abcdefgh|'.repeat(2000) + 'z)');", "t.js").unwrap();
    rt.gc();
    let with_regex = rt.heap_bytes();
    assert!(with_regex > before + 16_000, "a large program must be accounted: {} -> {}", before, with_regex);
    assert_eq!(text(&mut rt, "keep.test('xxabcdefghxx') + ':' + keep.test('y')"), "true:false");
}

#[test]
fn date_uses_the_injected_clock() {
    let mut rt = Runtime::new(Limits::default());
    let calls = Rc::new(Cell::new(0u32));
    let seen = calls.clone();
    rt.set_clock(Box::new(move || {
        seen.set(seen.get() + 1);
        1_000_000_000_000.0 + seen.get() as f64
    }));
    assert_eq!(text(&mut rt, "Date.now()"), "1000000000001");
    assert_eq!(text(&mut rt, "new Date().toISOString()"), "2001-09-09T01:46:40.002Z");
    assert_eq!(text(&mut rt, "Date().slice(0, 24)"), "Sun Sep 09 2001 01:46:40");
    assert_eq!(calls.get(), 3, "one clock read per Date.now() / new Date() / Date()");
    // dates built from explicit values never consult the clock
    assert_eq!(text(&mut rt, "new Date(0).getFullYear() + Date.UTC(2000, 0) + Date.parse('2000-01-01')"), "1893369601970");
    assert_eq!(calls.get(), 3);
    // a broken clock yields an invalid date, not a panic
    rt.set_clock(Box::new(|| f64::NAN));
    assert_eq!(text(&mut rt, "String(new Date()) + '|' + Date.now()"), "Invalid Date|NaN");
    rt.set_clock(Box::new(|| f64::INFINITY));
    assert_eq!(text(&mut rt, "new Date().getTime()"), "NaN");
    rt.set_clock(Box::new(|| 1234.75));
    assert_eq!(text(&mut rt, "Date.now()"), "1234");
}

#[test]
fn default_clock_is_coarse_and_local_time_is_utc() {
    let mut rt = Runtime::new(Limits::default());
    assert_eq!(text(&mut rt, "Date.now() % 100"), "0");
    assert_eq!(text(&mut rt, "Date.now() > 1.6e12"), "true");
    assert_eq!(text(&mut rt, "var d = new Date(); d.getTimezoneOffset() + ':' + (d.getHours() === d.getUTCHours()) + ':' + /GMT\\+0000 \\(Coordinated Universal Time\\)$/.test(String(d))"), "0:true:true");
}
