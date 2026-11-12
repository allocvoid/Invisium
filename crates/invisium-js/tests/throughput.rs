//! A5: throughput sanity checks.

use invisium_js::{Limits, Runtime, Value};
use std::time::{Duration, Instant};

fn timed(src: &str) -> (Value, Duration) {
    let mut rt = Runtime::new(Limits::default());
    let t = Instant::now();
    let v = rt.eval(src, "perf.js").expect("script runs");
    (v, t.elapsed())
}

#[test]
fn sum_loop_three_million() {
    let (v, d) = timed("let s = 0; for (let i = 1; i <= 3000000; i++) { s += i; } s");
    assert_eq!(v, Value::Number(4500001500000.0));
    assert!(d < Duration::from_secs(2), "sum loop took {:?}", d);
}

#[test]
fn recursive_fib_25() {
    let (v, d) = timed("function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); } fib(25)");
    assert_eq!(v, Value::Number(75025.0));
    assert!(d < Duration::from_secs(2), "fib(25) took {:?}", d);
}
