//! Milestone 3d: the module scenarios of `tests/modules.rs` run again with GC stress on
//! (a collection at every collection point): static graphs, cycles, top-level `await`,
//! dynamic `import()` through `take_import_requests` / `finish_import`, namespaces and live
//! bindings. The host roots what it keeps across collecting calls, as the embedder rule asks.

use invisium_js::{ErrorKind, JsError, Limits, Runtime, Value};
use std::collections::HashMap;

fn resolver() -> Box<dyn FnMut(&str, &str) -> Option<String>> {
    Box::new(|spec: &str, _referrer: &str| if spec == "bad" { None } else { Some(format!("mem:/{}", spec.trim_start_matches("./"))) })
}

struct Host {
    rt: Runtime,
    files: HashMap<String, String>,
}

impl Host {
    fn new(files: &[(&str, &str)]) -> Host {
        let mut rt = Runtime::new(Limits::default());
        rt.set_module_resolver(resolver());
        rt.set_gc_stress(1);
        rt.eval("var log = [], out;", "setup.js").expect("setup");
        let files = files.iter().map(|(n, s)| (format!("mem:/{}", n), s.to_string())).collect();
        Host { rt, files }
    }

    fn source(&self, url: &str) -> Option<String> {
        self.files.get(url.split('?').next().unwrap_or(url)).cloned()
    }

    fn load(&mut self, url: &str) -> Result<(), JsError> {
        let mut todo = vec![url.to_string()];
        while let Some(u) = todo.pop() {
            if self.rt.has_module(&u) {
                continue;
            }
            let Some(src) = self.source(&u) else { continue };
            todo.extend(self.rt.add_module(&u, &src)?);
        }
        Ok(())
    }

    fn pump(&mut self) {
        for _ in 0..10_000 {
            let reqs = self.rt.take_import_requests();
            if reqs.is_empty() {
                break;
            }
            for (id, url) in reqs {
                let r = match self.source(&url) {
                    Some(_) => self.load(&url).map_err(|e| e.message),
                    None => Err(format!("404 {}", url)),
                };
                self.rt.finish_import(id, r);
            }
            self.rt.run_jobs().expect("jobs");
        }
    }

    /// Runs the graph; the returned promise is rooted (it is kept across `run_jobs`).
    fn run(&mut self, entry: &str) -> Result<Value, JsError> {
        let url = format!("mem:/{}", entry);
        self.load(&url)?;
        let p = self.rt.run_module(&url)?;
        self.rt.root(p);
        self.rt.run_jobs()?;
        self.pump();
        Ok(p)
    }

    fn eval(&mut self, src: &str) -> String {
        match self.rt.eval(src, "check.js") {
            Ok(v) => self.rt.to_string_lossy(v),
            Err(e) => format!("{}!", e.name),
        }
    }

    fn state(&mut self, p: Value) -> String {
        let g = self.rt.global();
        self.rt.set(g, "__p", p).expect("set");
        self.eval("var __s = 'pending'; __p.then(() => { __s = 'fulfilled' }, e => { __s = 'rejected:' + (e && e.name) });");
        self.rt.run_jobs().expect("jobs");
        self.eval("__s")
    }
}

fn out_of(files: &[(&str, &str)], entry: &str) -> String {
    let mut h = Host::new(files);
    h.run(entry).expect("module runs");
    h.eval("String(out)")
}

const A: &str = "export default 1; export const x = 2; export function f() { return 3 }";

#[test]
fn stress_namespace_and_reexports() {
    let main = "import d, { x, f as g } from './a.js'; import * as ns from './a.js';
        out = [d, x, g(), ns.x, ns.default, Object.keys(ns).join(' '), ns[Symbol.toStringTag], Object.isExtensible(ns), Object.getPrototypeOf(ns)].join();";
    assert_eq!(out_of(&[("a.js", A), ("main.js", main)], "main.js"), "1,2,3,2,1,default f x,Module,false,");
    let b = "export { x as y } from './a.js'; export * from './c.js'; export * as nsA from './a.js'; export { default } from './a.js';";
    let main = "import { y, z, nsA, default as d } from './b.js'; import * as all from './b.js'; out = [y, z, nsA.x, d, Object.keys(all).join(' ')].join();";
    let files = [("a.js", A), ("b.js", b), ("c.js", "export const z = 9; export default 'not re-exported by *';"), ("main.js", main)];
    assert_eq!(out_of(&files, "main.js"), "2,9,2,1,default nsA y z");
}

#[test]
fn stress_live_bindings() {
    let counter = "export let count = 0; export function inc() { count++ } export { count as alias };";
    let main = "import { count, inc, alias } from './counter.js'; import * as ns from './counter.js';
        const before = count; inc(); inc();
        let err; try { count = 5 } catch (e) { err = e.name }
        let err2; try { ns.count = 5 } catch (e) { err2 = e.name }
        out = [before, count, alias, ns.count, err, err2].join();";
    assert_eq!(out_of(&[("counter.js", counter), ("main.js", main)], "main.js"), "0,2,2,2,TypeError,TypeError");
}

#[test]
fn stress_cycles_and_shared_modules() {
    let a = "import { b } from './b.js'; export function a() { return 'a' } log.push('a:' + b());";
    let b = "import { c } from './c.js'; export function b() { return 'b' } log.push('b:' + c());";
    let c = "import { a } from './a.js'; export function c() { return 'c' } log.push('c:' + a());";
    let mut h = Host::new(&[("a.js", a), ("b.js", b), ("c.js", c)]);
    let p = h.run("a.js").expect("cycle links");
    assert_eq!(h.eval("log.join()"), "c:a,b:c,a:b");
    assert_eq!(h.state(p), "fulfilled");
    let x = "import { y } from './y.js'; export let early = 1;";
    let y = "import { early } from './x.js'; export const y = 2; log.push(typeof early);";
    let mut h = Host::new(&[("x.js", x), ("y.js", y)]);
    let p = h.run("x.js").expect("links");
    assert_eq!(h.state(p), "rejected:ReferenceError");
    let shared = "globalThis.count = (globalThis.count || 0) + 1; export const v = 1;";
    let main = "import './x.js'; import './y.js'; import { v } from './shared.js'; out = v;";
    let files = [("shared.js", shared), ("x.js", "import { v } from './shared.js';"), ("y.js", "import './shared.js';"), ("main.js", main)];
    let mut h = Host::new(&files);
    h.run("main.js").expect("runs");
    h.rt.run_module("mem:/main.js").expect("second run is a no-op");
    h.eval("import('./shared.js').then(ns => { out = ns.v + ':' + count })");
    h.rt.run_jobs().unwrap();
    h.pump();
    assert_eq!(h.eval("count + '|' + out"), "1|1:1");
}

#[test]
fn stress_import_meta() {
    let main = "out = [import.meta.url, typeof this, (function () { return this })(), Object.getPrototypeOf(import.meta), import.meta === import.meta].join();";
    assert_eq!(out_of(&[("main.js", main)], "main.js"), "mem:/main.js,undefined,,,true");
}

#[test]
fn stress_top_level_await() {
    let dep = "export let v = 1; await null; v = 2; await Promise.resolve(); log.push('dep');";
    let main = "import { v } from './dep.js'; log.push('main:' + v);";
    let mut h = Host::new(&[("dep.js", dep), ("main.js", main)]);
    let p = h.run("main.js").expect("runs");
    assert_eq!(h.eval("log.join()"), "dep,main:2");
    assert_eq!(h.state(p), "fulfilled");
    let failing = "await null; throw new RangeError('late');";
    let mut h = Host::new(&[("f.js", failing), ("main.js", "import './f.js'; log.push('never');")]);
    let p = h.run("main.js").expect("links");
    assert_eq!(h.state(p), "rejected:RangeError");
    assert_eq!(h.eval("log.length"), "0");
}

#[test]
fn stress_dynamic_import_resolved_and_rejected() {
    let main = "import('./a.js').then(ns => log.push('ns:' + ns.x + ns.default));
        import('bad').catch(e => log.push('bad:' + e.name));
        import('./missing.js').catch(e => log.push('missing:' + e.name + ':' + e.message));
        import('./broken.js').catch(e => log.push('broken:' + e.name));";
    let files = [("a.js", A), ("broken.js", "import { nope } from './a.js';"), ("main.js", main)];
    let mut h = Host::new(&files);
    h.run("main.js").expect("runs");
    assert_eq!(h.eval("log.sort().join('|')"), "bad:TypeError|broken:SyntaxError|missing:TypeError:404 mem:/missing.js|ns:21");
    h.eval("import('./a.js').then(ns => { out = ns.f() })");
    h.rt.run_jobs().unwrap();
    h.pump();
    assert_eq!(h.eval("out"), "3");
}

/// The engine core's sequence: `run_module`, `take_import_requests`, `add_module` when the
/// lazy source arrives, `finish_import` (Ok, then Err), `run_jobs`.
/// Bug: `finish_import` took the `import()` promise out of the pending-import table (its
/// only root) and then entered the runtime, whose entry collection freed it; neither the
/// fulfilment nor the rejection callback ever ran.
#[test]
fn bug_import_promise_unrooted_in_finish_import() {
    let mut h = Host::new(&[]);
    let main = "import('./lazy.js').then(m => log.push('lazy:' + m.v)); import('./missing.js').catch(e => log.push('missing:' + e.name));";
    h.rt.add_module("mem:/main.js", main).expect("add");
    let p = h.rt.run_module("mem:/main.js").expect("runs");
    h.rt.root(p);
    let reqs = h.rt.take_import_requests();
    assert_eq!(reqs.len(), 2);
    for (id, url) in reqs {
        if url == "mem:/lazy.js" {
            h.rt.add_module(&url, "export const v = 'L' + 'Z';").expect("lazy");
            h.rt.finish_import(id, Ok(()));
        } else {
            h.rt.finish_import(id, Err(format!("404 {}", url)));
        }
    }
    h.rt.run_jobs().expect("jobs");
    assert_eq!(h.eval("log.sort().join('|')"), "lazy:LZ|missing:TypeError");
}

#[test]
fn stress_module_errors() {
    let mut h = Host::new(&[]);
    let e = h.rt.add_module("mem:/bad.js", "export const a = 1;\nlet x = ;\n").expect_err("syntax error");
    assert_eq!((e.kind, e.line), (ErrorKind::Syntax, 2));
    let mut h = Host::new(&[("a.js", A), ("main.js", "// importing\nimport { nope } from './a.js';\n")]);
    let e = h.run("main.js").expect_err("missing export");
    assert_eq!((e.name.as_str(), e.line), ("SyntaxError", 2));
}
