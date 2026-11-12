//! A4: embedding API — natives, calls from Rust, host classes, rooting and collection.

use invisium_js::{ErrorKind, HostClass, Limits, Runtime, Thrown, Value};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

fn s(rt: &mut Runtime, v: Value) -> String {
    rt.to_string_lossy(v)
}

#[test]
fn native_function_called_from_script() {
    let mut rt = Runtime::new(Limits::default());
    let add = rt.function("add", 2, Rc::new(|rt: &mut Runtime, _this: Value, args: &[Value]| {
        let a = rt.to_number(args.first().copied().unwrap_or(Value::Undefined));
        let b = rt.to_number(args.get(1).copied().unwrap_or(Value::Undefined));
        Ok(Value::Number(a + b))
    }));
    let g = rt.global();
    rt.set(g, "add", add).unwrap();
    let v = rt.eval("add(2, 3) + add.length", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), "7");

    let fail = rt.function("fail", 0, Rc::new(|rt: &mut Runtime, _: Value, _: &[Value]| -> Result<Value, Thrown> {
        Err(rt.throw_error("TypeError", "nope"))
    }));
    rt.set(g, "fail", fail).unwrap();
    let v = rt.eval("try { fail(); } catch (e) { e.name + ':' + e.message }", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), "TypeError:nope");
    let e = rt.eval("fail()", "t.js").unwrap_err();
    assert_eq!((e.kind, e.name.as_str(), e.message.as_str()), (ErrorKind::Thrown, "TypeError", "nope"));
}

#[test]
fn script_called_from_rust() {
    let mut rt = Runtime::new(Limits::default());
    rt.eval("function sq(x) { return x * x; } var obj = { k: 3, m(y) { return this.k + y; } };", "t.js").unwrap();
    let g = rt.global();
    let sq = rt.get(g, "sq").unwrap();
    assert!(rt.is_callable(sq));
    assert_eq!(rt.type_of(sq), "function");
    let r = rt.call(sq, Value::Undefined, &[Value::Number(7.0)]).unwrap();
    assert_eq!(r, Value::Number(49.0));
    let obj = rt.get(g, "obj").unwrap();
    let m = rt.get(obj, "m").unwrap();
    assert_eq!(rt.call(m, obj, &[Value::Number(4.0)]).unwrap(), Value::Number(7.0));
    let arr = rt.array(&[Value::Number(1.0), Value::Bool(true)]);
    assert_eq!(rt.get_index(arr, 1).unwrap(), Value::Bool(true));
    let st = rt.string("héllo");
    assert_eq!(s(&mut rt, st), "héllo");
    let thrower = rt.eval("(function() { throw new RangeError('r'); })", "t.js").unwrap();
    let e = rt.call(thrower, Value::Undefined, &[]).unwrap_err();
    assert_eq!(e.name, "RangeError");
    let bad = rt.eval("({ toString() { throw 1; } })", "t.js").unwrap();
    assert_eq!(rt.to_string_lossy(bad), "");
    assert!(rt.to_boolean(Value::Number(1.0)));
}

static LAST_SET: AtomicU64 = AtomicU64::new(0);

fn host_get(rt: &mut Runtime, id: u64, key: &str) -> Result<Option<Value>, Thrown> {
    match key {
        "tag" => Ok(Some(rt.string(&format!("node-{}", id)))),
        "double" => Ok(Some(rt.function("double", 1, Rc::new(|rt: &mut Runtime, _: Value, a: &[Value]| {
            let n = rt.to_number(a.first().copied().unwrap_or(Value::Undefined));
            Ok(Value::Number(n * 2.0))
        })))),
        "boom" => Err(rt.throw_error("Error", "host get failed")),
        _ => Ok(None),
    }
}

fn host_set(rt: &mut Runtime, _id: u64, key: &str, value: Value) -> Result<bool, Thrown> {
    if key == "value" {
        LAST_SET.store(rt.to_number(value) as u64, Ordering::SeqCst);
        return Ok(true);
    }
    Ok(false)
}

#[test]
fn host_class_get_and_set_are_hit() {
    let mut rt = Runtime::new(Limits::default());
    let class = rt.register_host_class(HostClass { name: "Node", get: host_get, set: host_set });
    let el = rt.host_object(class, 7);
    assert_eq!(rt.host_object(class, 7), el, "same (class, id) must give the same object");
    assert_eq!(rt.host_id(el), Some((class, 7)));
    assert_eq!(rt.host_id(Value::Null), None);
    let g = rt.global();
    rt.set(g, "el", el).unwrap();
    let v = rt.eval("el.value = 42; el.custom = 'c'; [el.tag, el.double(21), el.custom, 'value' in el, el.missing].join()", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), "node-7,42,c,false,");
    assert_eq!(LAST_SET.load(Ordering::SeqCst), 42);
    let v = rt.eval("try { el.boom } catch (e) { e.message }", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), "host get failed");
    rt.gc();
    let v = rt.eval("el.custom", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), "c", "host objects keep expando properties across GC");
}

#[test]
fn root_keeps_object_alive_across_gc() {
    let mut rt = Runtime::new(Limits::default());
    let o = rt.object();
    let val = rt.string("kept value");
    rt.set(o, "k", val).unwrap();
    let id = rt.root(o);
    for _ in 0..3 {
        rt.eval("let junk = []; for (let i = 0; i < 1000; i++) junk.push({i});", &format!("j{}.js", 0)).ok();
        rt.gc();
    }
    let v = rt.get(o, "k").unwrap();
    assert_eq!(s(&mut rt, v), "kept value");
    rt.unroot(id);
}

#[test]
fn unrooted_cycles_are_collected() {
    let mut rt = Runtime::new(Limits::default());
    rt.gc();
    let base = rt.heap_bytes();
    rt.eval("var keep = []; for (let i = 0; i < 20000; i++) { const a = {i}; const b = {a}; a.b = b; keep.push(a); }", "c.js").unwrap();
    rt.gc();
    let full = rt.heap_bytes();
    assert!(full > base + 1_000_000, "cycles should occupy memory: {} -> {}", base, full);
    rt.eval("keep = null;", "c.js").unwrap();
    rt.gc();
    let after = rt.heap_bytes();
    assert!(after < base + (full - base) / 10, "cycles must be collected: base {} full {} after {}", base, full, after);
}

#[test]
fn heap_cap_is_reported_to_host_and_runtime_recovers() {
    let mut rt = Runtime::new(Limits { max_heap_bytes: 16 << 20, ..Limits::default() });
    let e = rt.eval("(() => { const a = []; while (true) a.push({x: 'y'.repeat(100) + a.length}); })()", "oom.js").unwrap_err();
    assert_eq!(e.kind, ErrorKind::OutOfMemory);
    let v = rt.eval("[1, 2, 3].map(x => x * 2).join()", "after.js").unwrap();
    assert_eq!(s(&mut rt, v), "2,4,6");
}

#[test]
fn injected_clock_and_random() {
    let mut rt = Runtime::new(Limits::default());
    rt.set_random(Box::new(|| 0.25));
    rt.set_clock(Box::new(|| 1_000.0));
    let v = rt.eval("Math.random()", "r.js").unwrap();
    assert_eq!(v, Value::Number(0.25));
}

#[test]
fn step_budget_applies_to_calls_from_rust() {
    let mut rt = Runtime::new(Limits { step_budget: 100_000, ..Limits::default() });
    rt.eval("function spin() { while (true) {} }", "s.js").unwrap();
    let g = rt.global();
    let spin = rt.get(g, "spin").unwrap();
    let e = rt.call(spin, Value::Undefined, &[]).unwrap_err();
    assert_eq!(e.kind, ErrorKind::StepBudget);
}

#[test]
fn errors_carry_line_numbers() {
    let mut rt = Runtime::new(Limits::default());
    let e = rt.eval("let a = 1;\nlet b = 2;\nnull.prop;", "l.js").unwrap_err();
    assert_eq!((e.name.as_str(), e.line), ("TypeError", 3));
    let e = rt.eval("\n\n  let = ;", "l.js").unwrap_err();
    assert_eq!((e.kind, e.line), (ErrorKind::Syntax, 3));
    let e = rt.eval("function f() {\n  throw new Error('deep');\n}\nf();", "l.js").unwrap_err();
    assert_eq!(e.line, 2);
    assert!(e.stack.contains("Error: deep"), "stack: {}", e.stack);
}
