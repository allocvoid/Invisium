//! Milestone 3b, Step 0: the settable native-stack budget.

use invisium_js::{ErrorKind, Limits, Runtime, Value};
use std::cell::Cell;
use std::rc::Rc;

/// Installs `dive`, a native that re-enters the runtime by calling the script function `step`
/// (which calls `dive` again), and returns the greatest nesting depth reached before the engine
/// refused to go deeper, plus the error the outermost call ended with.
fn dive_depth(budget: Option<usize>) -> (u32, bool, ErrorKind) {
    let mut rt = Runtime::new(Limits { max_call_depth: 10_000_000, ..Limits::default() });
    if let Some(b) = budget {
        rt.set_native_stack_budget(b);
    }
    let depth = Rc::new(Cell::new(0u32));
    let deepest = Rc::new(Cell::new(0u32));
    let (d, m) = (depth.clone(), deepest.clone());
    let dive = rt.function(
        "dive",
        0,
        Rc::new(move |rt: &mut Runtime, _, _: &[Value]| {
            d.set(d.get() + 1);
            m.set(m.get().max(d.get()));
            let g = rt.global();
            let step = rt.get(g, "step").map_err(|_| rt.throw_error("Error", "no step"))?;
            let r = rt.call(step, Value::Undefined, &[]);
            d.set(d.get() - 1);
            match r {
                Ok(v) => Ok(v),
                // hand the script-level error on (same class) so the script can catch it
                Err(e) => Err(rt.throw_error(&e.name, &e.message)),
            }
        }),
    );
    let g = rt.global();
    rt.set(g, "dive", dive).unwrap();
    rt.eval("function step() { return dive() }", "step.js").unwrap();
    // the overflow is an ordinary catchable RangeError inside script...
    let caught = rt.eval("var caught = false; try { dive() } catch (e) { caught = e instanceof RangeError } caught", "catch.js").unwrap();
    // ...and a StackOverflow error for the embedder when nothing catches it
    let err = rt.eval("(function again() { return [0].map(again) })()", "native-recursion.js").unwrap_err();
    // the runtime is still usable afterwards
    assert_eq!(rt.eval("1 + 1", "after.js").unwrap(), Value::Number(2.0));
    (deepest.get(), caught == Value::Bool(true), err.kind)
}

#[test]
fn a_larger_native_stack_budget_allows_clearly_deeper_native_nesting() {
    let handle = std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(|| {
            let (shallow, caught_default, kind_default) = dive_depth(None);
            let (deep, caught_big, kind_big) = dive_depth(Some(8 << 20));
            (shallow, caught_default, kind_default, deep, caught_big, kind_big)
        })
        .unwrap();
    let (shallow, caught_default, kind_default, deep, caught_big, kind_big) = handle.join().expect("no crash");
    assert!(shallow >= 8, "default budget reached depth {}", shallow);
    assert!(deep > shallow * 4, "8 MiB budget reached depth {}, default {}", deep, shallow);
    assert!(caught_default && caught_big, "the overflow must be catchable in script");
    assert_eq!((kind_default, kind_big), (ErrorKind::StackOverflow, ErrorKind::StackOverflow));
}

#[test]
fn the_default_budget_is_unchanged_and_a_tiny_budget_still_fails_cleanly() {
    // default: fits the 2 MiB test thread, exactly as before this milestone
    let mut rt = Runtime::new(Limits::default());
    let e = rt.eval("(function again() { return [0].map(again) })()", "d.js").unwrap_err();
    assert_eq!(e.kind, ErrorKind::StackOverflow);
    // a budget of zero refuses every native call but does not crash or poison the runtime
    rt.set_native_stack_budget(0);
    let e = rt.eval("[1].map(function (x) { return x })", "z.js").unwrap_err();
    assert_eq!((e.kind, e.name.as_str()), (ErrorKind::StackOverflow, "RangeError"));
    rt.set_native_stack_budget(640 * 1024);
    assert_eq!(rt.eval("[1, 2].map(function (x) { return x * 2 }).join()", "ok.js").map(|v| rt.to_string_lossy(v)).unwrap(), "2,4");
}
