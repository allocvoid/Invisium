//! B3: embedding additions of Milestone 2a (step 0 functions, promises from Rust).

use invisium_js::{ErrorKind, HostClass, Limits, Runtime, Thrown, Value};
use std::cell::Cell;
use std::rc::Rc;

fn s(rt: &mut Runtime, v: Value) -> String {
    rt.to_string_lossy(v)
}

struct Doc {
    title: String,
    reads: Cell<u32>,
}

fn node_get(rt: &mut Runtime, id: u64, key: &str) -> Result<Option<Value>, Thrown> {
    if key != "title" {
        return Ok(None);
    }
    let Some(data) = rt.host_data() else { return Err(rt.throw_error("Error", "no host data")) };
    let Ok(doc) = data.downcast::<Doc>() else { return Err(rt.throw_error("Error", "wrong host data")) };
    doc.reads.set(doc.reads.get() + 1);
    Ok(Some(rt.string(&format!("{}#{}", doc.title, id))))
}

fn node_set(_: &mut Runtime, _: u64, _: &str, _: Value) -> Result<bool, Thrown> {
    Ok(false)
}

#[test]
fn wrapper_with_proto_inherits_native_accessor() {
    let mut rt = Runtime::new(Limits::default());
    let class = rt.register_host_class(HostClass { name: "Node", get: node_get, set: node_set });
    let proto = rt.object();
    let getter = rt.function("get nodeId", 0, Rc::new(|rt: &mut Runtime, this: Value, _: &[Value]| {
        match rt.host_id(this) {
            Some((_, id)) => Ok(Value::Number(id as f64 * 10.0)),
            None => Err(rt.throw_error("TypeError", "Illegal invocation")),
        }
    }));
    let seen = Rc::new(Cell::new(0.0));
    let seen2 = seen.clone();
    let setter = rt.function("set nodeId", 1, Rc::new(move |rt: &mut Runtime, _: Value, a: &[Value]| {
        seen2.set(rt.to_number(a.first().copied().unwrap_or(Value::Undefined)));
        Ok(Value::Undefined)
    }));
    rt.define_accessor(proto, "nodeId", Some(getter), Some(setter), true).unwrap();
    rt.define_accessor(proto, "readOnly", Some(getter), None, false).unwrap();
    let el = rt.host_object_with_proto(class, 4, proto);
    let other = rt.object();
    assert_eq!(rt.host_object_with_proto(class, 4, other), el, "existing wrapper is returned unchanged");
    assert_eq!(rt.host_object(class, 4), el);
    let g = rt.global();
    rt.set(g, "el", el).unwrap();
    rt.set(g, "proto", proto).unwrap();
    let v = rt.eval("el.nodeId = 99; [el.nodeId, el.readOnly, Object.getPrototypeOf(el) === proto, Object.keys(proto).join('|')].join()", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), "40,40,true,nodeId");
    assert_eq!(seen.get(), 99.0);
    let v = rt.eval("'use strict'; try { el.readOnly = 1; 'no error' } catch (e) { e.name }", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), "TypeError");
    let v = rt.eval("const d = Object.getOwnPropertyDescriptor(proto, 'readOnly'); [typeof d.get, d.set, d.enumerable, d.configurable].join()", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), "function,,false,true");
    // replacing an accessor works because accessors are configurable
    rt.define_accessor(proto, "nodeId", None, None, false).unwrap();
    let v = rt.eval("String(el.nodeId)", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), "undefined");
    assert!(rt.define_accessor(Value::Number(1.0), "x", None, None, false).is_err());
    assert!(rt.define_accessor(proto, "bad", Some(Value::Number(1.0)), None, false).is_err());
}

#[test]
fn plain_host_class_uses_ordinary_lookup() {
    let mut rt = Runtime::new(Limits::default());
    let hooked = rt.register_host_class(HostClass { name: "Node", get: node_get, set: node_set });
    let plain = rt.register_plain_host_class("Element");
    assert_ne!(hooked, plain);
    let proto = rt.object();
    let getter = rt.function("get tagName", 0, Rc::new(|rt: &mut Runtime, this: Value, _: &[Value]| {
        match rt.host_id(this) {
            Some((_, id)) => Ok(rt.string(&format!("EL{}", id))),
            None => Err(rt.throw_error("TypeError", "Illegal invocation")),
        }
    }));
    rt.define_accessor(proto, "tagName", Some(getter), None, true).unwrap();
    let el = rt.host_object_with_proto(plain, 12, proto);
    assert_eq!(rt.host_object(plain, 12), el, "same (class, id) gives the same wrapper");
    assert_eq!(rt.host_id(el), Some((plain, 12)));
    let g = rt.global();
    rt.set(g, "el", el).unwrap();
    // `title` would hit the hooked class's get hook (and fail without host data); a plain class never calls it
    let v = rt.eval("el.expando = 5; el.title = 't'; [el.tagName, el.expando, el.title, 'tagName' in el, 'expando' in el, 'nope' in el, el.nope, Object.keys(el).join('|')].join()", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), "EL12,5,t,true,true,false,,expando|title");
    rt.gc();
    assert_eq!(rt.get(el, "expando").unwrap(), Value::Number(5.0));
    let v = rt.eval("delete el.expando; [el.expando, el.tagName].join()", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), ",EL12");
    // a default-proto wrapper of a plain class behaves like an ordinary object
    let bare = rt.host_object(plain, 13);
    assert_eq!(rt.host_id(bare), Some((plain, 13)));
    assert_eq!(rt.get(bare, "tagName").unwrap(), Value::Undefined);
}

#[test]
fn promise_resolved_from_rust_runs_then_on_run_jobs() {
    let mut rt = Runtime::new(Limits::default());
    let (p, resolve, reject) = rt.new_promise();
    let roots = [rt.root(p), rt.root(resolve), rt.root(reject)];
    assert!(!rt.has_pending_jobs());
    let g = rt.global();
    rt.set(g, "p", p).unwrap();
    rt.eval("var log = []; p.then(v => log.push('got ' + v)); log.push(p instanceof Promise);", "t.js").unwrap();
    assert!(!rt.has_pending_jobs(), "a pending promise queues nothing");
    rt.gc();
    let v = rt.string("from rust");
    rt.call(resolve, Value::Undefined, &[v]).unwrap();
    assert!(rt.has_pending_jobs());
    let log = rt.eval("log.join()", "t.js").unwrap();
    assert_eq!(s(&mut rt, log), "true", "then callbacks only run inside run_jobs");
    rt.run_jobs().unwrap();
    assert!(!rt.has_pending_jobs());
    let log = rt.eval("log.join()", "t.js").unwrap();
    assert_eq!(s(&mut rt, log), "true,got from rust");
    // settling twice is ignored
    rt.call(reject, Value::Undefined, &[Value::Number(1.0)]).unwrap();
    rt.run_jobs().unwrap();
    assert!(rt.take_unhandled_rejections().is_empty());
    for r in roots {
        rt.unroot(r);
    }

    // rejection from Rust reaches a catch handler; without one it is reported, not an error
    let (p2, _res2, rej2) = rt.new_promise();
    let keep = rt.root(rej2);
    rt.set(g, "p2", p2).unwrap();
    rt.eval("p2.catch(e => log.push('caught ' + e.message));", "t.js").unwrap();
    let err_ctor = rt.get(g, "Error").unwrap();
    let msg = rt.string("rust says no");
    let err = rt.construct(err_ctor, &[msg]).unwrap();
    rt.call(rej2, Value::Undefined, &[err]).unwrap();
    rt.run_jobs().unwrap();
    let log = rt.eval("log[log.length - 1]", "t.js").unwrap();
    assert_eq!(s(&mut rt, log), "caught rust says no");
    rt.unroot(keep);

    rt.eval("Promise.reject(new TypeError('nobody listens')); Promise.reject('handled later').catch(() => {});", "t.js").unwrap();
    rt.run_jobs().expect("an unhandled rejection is not an error from run_jobs");
    assert_eq!(rt.take_unhandled_rejections(), vec!["TypeError: nobody listens".to_string()]);
    assert!(rt.take_unhandled_rejections().is_empty(), "the list is drained by taking it");
}

#[test]
fn jobs_run_under_the_step_budget_and_survive_gc() {
    let mut rt = Runtime::new(Limits { step_budget: 200_000, ..Limits::default() });
    rt.eval("var n = 0; function spin() { n++; Promise.resolve().then(spin); } spin();", "t.js").unwrap();
    let e = rt.run_jobs().unwrap_err();
    assert_eq!(e.kind, ErrorKind::StepBudget);
    assert!(!rt.has_pending_jobs(), "an aborted drain drops the remaining jobs");
    rt.eval("var seen = []; for (let i = 0; i < 2000; i++) Promise.resolve({ i }).then(o => { if (o.i % 500 === 0) seen.push(o.i); });", "t.js").unwrap();
    rt.gc();
    rt.run_jobs().unwrap();
    let v = rt.eval("seen.join()", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), "0,500,1000,1500");
}

#[test]
fn weak_collections_do_not_keep_keys_alive_across_gc() {
    let mut rt = Runtime::new(Limits::default());
    rt.gc();
    let base = rt.heap_bytes();
    // values reference their own keys (ephemeron cycles) and each other through the map
    rt.eval(
        "var wm = new WeakMap(), ws = new WeakSet(), kept = [];
         for (let i = 0; i < 20000; i++) {
             const k = { i, pad: 'x'.repeat(40) + i };
             wm.set(k, { self: k, payload: [i, i, i, i] });
             ws.add(k);
             if (i % 1000 === 0) kept.push(k);
         }",
        "w.js",
    )
    .unwrap();
    let full = rt.heap_bytes();
    assert!(full > base + 2_000_000, "the entries should occupy memory: {} -> {}", base, full);
    rt.gc();
    let after = rt.heap_bytes();
    assert!(after < base + (full - base) / 5, "dead keys must be collected with their values: base {} full {} after {}", base, full, after);
    // entries whose key is still reachable survive, values included
    let v = rt.eval("kept.map(k => wm.get(k).payload[0] + ':' + ws.has(k) + ':' + (wm.get(k).self === k)).slice(0, 3).join()", "w.js").unwrap();
    assert_eq!(s(&mut rt, v), "0:true:true,1000:true:true,2000:true:true");
    // a value reachable only through a live key keeps what it references alive
    rt.eval("var key = {}; wm.set(key, { deep: { deeper: ['still here'] } }); kept = null;", "w.js").unwrap();
    rt.gc();
    rt.eval("var junk = []; for (let i = 0; i < 5000; i++) junk.push({ i }); junk = null;", "w.js").unwrap();
    rt.gc();
    let v = rt.eval("wm.get(key).deep.deeper[0]", "w.js").unwrap();
    assert_eq!(s(&mut rt, v), "still here");
    // dropping the weak map itself releases everything
    rt.eval("wm = null; ws = null; key = null;", "w.js").unwrap();
    rt.gc();
    assert!(rt.heap_bytes() <= after, "heap after dropping the maps: {} (was {})", rt.heap_bytes(), after);
}

/// Allocation-heavy workloads under a small heap, so that collections run in the middle of
/// generator resumes, awaits, promise jobs, proxy traps and collection updates.
#[test]
fn frequent_gc_keeps_suspended_state_intact() {
    let mut rt = Runtime::new(Limits { max_heap_bytes: 12 << 20, ..Limits::default() });
    let v = rt
        .eval(
            "function* gen(n) { for (let i = 0; i < n; i++) { const tmp = { i, pad: 'p'.repeat(50) + i }; const got = yield tmp; if (got !== tmp.i * 2) throw new Error('bad resume ' + got); } return 'done'; }
             let total = 0;
             for (let round = 0; round < 1000; round++) {
                 const it = gen(200); let r = it.next();
                 while (!r.done) { total += r.value.pad.length > 50 ? 1 : 0; r = it.next(r.value.i * 2); }
                 if (r.value !== 'done') throw new Error('bad return');
             }
             function* walk(d) { if (d > 0) { yield [d, 'x'.repeat(30)]; yield* walk(d - 1); } }
             let deep = 0; for (let i = 0; i < 3000; i++) for (const [d] of walk(40)) deep += d;
             total + ':' + deep",
            "gen.js",
        )
        .unwrap();
    assert_eq!(s(&mut rt, v), "200000:2460000");

    rt.eval(
        "var done = 0; const results = new Map(); const seen = new WeakSet();
         async function work(k) { const acc = []; for (let i = 0; i < 50; i++) acc.push(await { v: k + i, pad: 'q'.repeat(40) }); return acc.reduce((t, o) => t + o.v, 0); }
         for (let k = 0; k < 300; k++) work(k).then(v => { const key = { k }; results.set(k, { v, key }); seen.add(key); done++; });
         var all; Promise.all(Array.from({ length: 3000 }, (_, i) => new Promise(r => queueMicrotask(() => r({ i, pad: 'w'.repeat(20) }))))).then(a => { all = a.reduce((t, o) => t + o.i, 0); });
         var agen = 0; async function* numbers(n) { for (let i = 0; i < n; i++) yield { i, pad: 'y'.repeat(25) }; }
         (async () => { for (let r = 0; r < 20; r++) for await (const o of numbers(200)) agen += o.i; })();",
        "async.js",
    )
    .unwrap();
    rt.run_jobs().unwrap();
    assert!(rt.take_unhandled_rejections().is_empty());
    let v = rt.eval("let sum = 0, ok = 0; for (const [k, r] of results) { sum += r.v; if (seen.has(r.key) && r.key.k === k) ok++; } [done, sum, ok, all, agen].join()", "async.js").unwrap();
    assert_eq!(s(&mut rt, v), "300,2610000,300,4498500,398000");

    let v = rt
        .eval(
            "const store = new Proxy({}, { get: (t, k, r) => Reflect.get(t, k, r), set(t, k, v, r) { return Reflect.set(t, k, { boxed: v, pad: 'z'.repeat(30) }, r); }, ownKeys: t => Reflect.ownKeys(t).map(k => k) });
             for (let i = 0; i < 60000; i++) store['k' + (i % 100)] = i;
             let t = 0; for (const k of Object.keys(store)) t += store[k].boxed;
             const m = new Map(); for (let i = 0; i < 60000; i++) { m.set('key' + (i % 500), { i, pad: 'm'.repeat(20) }); if (i % 3 === 0) m.delete('key' + ((i + 7) % 500)); }
             let ms = 0; for (const [, o] of m) ms += o.i % 7;
             t + ':' + (m.size > 300) + ':' + (ms > 0)",
            "proxy.js",
        )
        .unwrap();
    assert_eq!(s(&mut rt, v), "5994950:true:true");
}

#[test]
fn map_tombstones_are_reclaimed_by_gc() {
    let mut rt = Runtime::new(Limits { max_heap_bytes: 24 << 20, ..Limits::default() });
    // heavy insert/delete churn on one Set must not grow without bound
    let v = rt.eval("const s = new Set(); for (let i = 0; i < 600000; i++) { s.add(i); s.delete(i - 3); } s.size", "churn.js").unwrap();
    assert_eq!(v, Value::Number(3.0));
    // an iterator created before a collection keeps its place across it
    let v = rt
        .eval(
            "const m = new Map(); for (let i = 0; i < 100; i++) m.set(i, i); for (let i = 0; i < 50; i++) m.delete(i);
             const it = m.keys(); it.next(); let junk = []; for (let i = 0; i < 60000; i++) junk.push({ i }); junk = null;
             [...it].length",
            "iter.js",
        )
        .unwrap();
    assert_eq!(v, Value::Number(49.0));
}

#[test]
fn define_property_honours_attributes() {
    let mut rt = Runtime::new(Limits::default());
    let o = rt.object();
    rt.define_property(o, "fixed", Value::Number(1.0), false, false, false).unwrap();
    rt.define_property(o, "open", Value::Number(2.0), true, true, true).unwrap();
    let g = rt.global();
    rt.set(g, "o", o).unwrap();
    let v = rt.eval("o.fixed = 5; o.open = 6; delete o.fixed; [o.fixed, o.open, Object.keys(o).join('|'), JSON.stringify(Object.getOwnPropertyDescriptor(o, 'fixed'))].join(' ')", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), r#"1 6 open {"value":1,"writable":false,"enumerable":false,"configurable":false}"#);
    let e = rt.define_property(o, "fixed", Value::Number(9.0), true, true, true).unwrap_err();
    assert_eq!((e.kind, e.name.as_str()), (ErrorKind::Thrown, "TypeError"));
    rt.define_property(o, "open", Value::Number(7.0), false, true, true).unwrap();
    let v = rt.eval("o.open = 8; o.open", "t.js").unwrap();
    assert_eq!(v, Value::Number(7.0));
    assert!(rt.define_property(Value::Undefined, "x", Value::Null, true, true, true).is_err());
}

#[test]
fn construct_calls_constructors_from_rust() {
    let mut rt = Runtime::new(Limits::default());
    rt.eval("class P { constructor(x, y) { this.sum = x + y; } get twice() { return this.sum * 2; } } function F(a) { this.a = a; }", "t.js").unwrap();
    let p = rt.eval("P", "t.js").unwrap();
    let inst = rt.construct(p, &[Value::Number(2.0), Value::Number(3.0)]).unwrap();
    assert_eq!(rt.get(inst, "twice").unwrap(), Value::Number(10.0));
    let g = rt.global();
    let f = rt.get(g, "F").unwrap();
    let inst = rt.construct(f, &[Value::Bool(true)]).unwrap();
    assert_eq!(rt.get(inst, "a").unwrap(), Value::Bool(true));
    rt.set(g, "inst", inst).unwrap();
    let v = rt.eval("inst instanceof F", "t.js").unwrap();
    assert_eq!(v, Value::Bool(true));
    let err_ctor = rt.get(g, "RangeError").unwrap();
    let msg = rt.string("too far");
    let e = rt.construct(err_ctor, &[msg]).unwrap();
    assert_eq!(s(&mut rt, e), "RangeError: too far");
    let arrow = rt.eval("(() => 1)", "t.js").unwrap();
    let e = rt.construct(arrow, &[]).unwrap_err();
    assert_eq!(e.name, "TypeError");
    assert!(rt.construct(Value::Number(3.0), &[]).is_err());
    let thrower = rt.eval("(function T() { throw new SyntaxError('ctor failed'); })", "t.js").unwrap();
    let e = rt.construct(thrower, &[]).unwrap_err();
    assert_eq!((e.name.as_str(), e.message.as_str()), ("SyntaxError", "ctor failed"));
}

#[test]
fn host_class_hook_reads_state_through_host_data() {
    let mut rt = Runtime::new(Limits::default());
    assert!(rt.host_data().is_none());
    let doc = Rc::new(Doc { title: "page".to_string(), reads: Cell::new(0) });
    rt.set_host_data(doc.clone());
    let class = rt.register_host_class(HostClass { name: "Node", get: node_get, set: node_set });
    let el = rt.host_object(class, 3);
    let g = rt.global();
    rt.set(g, "el", el).unwrap();
    let v = rt.eval("el.title + '/' + el.title", "t.js").unwrap();
    assert_eq!(s(&mut rt, v), "page#3/page#3");
    assert_eq!(doc.reads.get(), 2);
    // natives see the same slot
    let peek = rt.function("peek", 0, Rc::new(|rt: &mut Runtime, _: Value, _: &[Value]| {
        let n = rt.host_data().and_then(|d| d.downcast::<Doc>().ok()).map(|d| d.reads.get()).unwrap_or(0);
        Ok(Value::Number(n as f64))
    }));
    rt.set(g, "peek", peek).unwrap();
    assert_eq!(rt.eval("peek()", "t.js").unwrap(), Value::Number(2.0));
    rt.set_host_data(Rc::new(5u32));
    let e = rt.eval("el.title", "t.js").unwrap_err();
    assert_eq!(e.message, "wrong host data");
}
