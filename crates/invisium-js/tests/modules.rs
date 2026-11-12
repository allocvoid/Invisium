//! F2: ES modules through the embedding API, with an in-test resolver and source table.

use invisium_js::{ErrorKind, JsError, Limits, Runtime, Value};
use std::collections::HashMap;

/// Specifiers resolve to `mem:/<name>`; the specifier `bad` is refused.
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
        rt.eval("var log = [], out;", "setup.js").expect("setup");
        let files = files.iter().map(|(n, s)| (format!("mem:/{}", n), s.to_string())).collect();
        Host { rt, files }
    }

    /// The source of an address (a query string is ignored).
    fn source(&self, url: &str) -> Option<String> {
        self.files.get(url.split('?').next().unwrap_or(url)).cloned()
    }

    /// Adds the static graph under `url`; addresses without a source are left out.
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

    /// Serves dynamic imports until none are left.
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

    fn run(&mut self, entry: &str) -> Result<Value, JsError> {
        let url = format!("mem:/{}", entry);
        self.load(&url)?;
        let p = self.rt.run_module(&url)?;
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

    /// The state of a promise: "fulfilled", "rejected:<name>" or "pending".
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
fn default_named_and_namespace_imports() {
    let main = "import d, { x, f as g } from './a.js'; import * as ns from './a.js';
        out = [d, x, g(), ns.x, ns.default, Object.keys(ns).join(' '), ns[Symbol.toStringTag], Object.isExtensible(ns), Object.getPrototypeOf(ns)].join();";
    assert_eq!(out_of(&[("a.js", A), ("main.js", main)], "main.js"), "1,2,3,2,1,default f x,Module,false,");
}

#[test]
fn re_exports() {
    let b = "export { x as y } from './a.js'; export * from './c.js'; export * as nsA from './a.js'; export { default } from './a.js';";
    let main = "import { y, z, nsA, default as d } from './b.js'; import * as all from './b.js'; out = [y, z, nsA.x, d, Object.keys(all).join(' ')].join();";
    let files = [("a.js", A), ("b.js", b), ("c.js", "export const z = 9; export default 'not re-exported by *';"), ("main.js", main)];
    assert_eq!(out_of(&files, "main.js"), "2,9,2,1,default nsA y z");
}

#[test]
fn live_bindings_are_read_only_views() {
    let counter = "export let count = 0; export function inc() { count++ } export { count as alias };";
    let main = "import { count, inc, alias } from './counter.js'; import * as ns from './counter.js';
        const before = count; inc(); inc();
        let err; try { count = 5 } catch (e) { err = e.name }
        let err2; try { ns.count = 5 } catch (e) { err2 = e.name }
        out = [before, count, alias, ns.count, err, err2].join();";
    assert_eq!(out_of(&[("counter.js", counter), ("main.js", main)], "main.js"), "0,2,2,2,TypeError,TypeError");
}

#[test]
fn three_module_cycle_runs_in_post_order_with_hoisted_functions() {
    let a = "import { b } from './b.js'; export function a() { return 'a' } log.push('a:' + b());";
    let b = "import { c } from './c.js'; export function b() { return 'b' } log.push('b:' + c());";
    let c = "import { a } from './a.js'; export function c() { return 'c' } log.push('c:' + a());";
    let mut h = Host::new(&[("a.js", a), ("b.js", b), ("c.js", c)]);
    let p = h.run("a.js").expect("cycle links");
    assert_eq!(h.eval("log.join()"), "c:a,b:c,a:b");
    assert_eq!(h.state(p), "fulfilled");
    // a `let` read across the cycle before it is initialised is a ReferenceError
    let x = "import { y } from './y.js'; export let early = 1;";
    let y = "import { early } from './x.js'; export const y = 2; log.push(typeof early);";
    let mut h = Host::new(&[("x.js", x), ("y.js", y)]);
    let p = h.run("x.js").expect("links");
    assert_eq!(h.state(p), "rejected:ReferenceError");
}

#[test]
fn a_shared_module_is_evaluated_once() {
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
fn import_meta_url_and_module_strictness() {
    let main = "out = [import.meta.url, typeof this, (function () { return this })(), Object.getPrototypeOf(import.meta), import.meta === import.meta].join();";
    assert_eq!(out_of(&[("main.js", main)], "main.js"), "mem:/main.js,undefined,,,true");
    let mut h = Host::new(&[]);
    for (src, what) in [("with (x) {}", "with"), ("let x = 010;", "octal")] {
        let e = h.rt.add_module(&format!("mem:/{}.js", what), src).expect_err(what);
        assert_eq!(e.kind, ErrorKind::Syntax, "{}", what);
    }
    // classic scripts keep import / export declarations and import.meta as SyntaxErrors
    for src in ["import x from 'y';", "export const a = 1;", "import.meta"] {
        assert_eq!(h.eval(src), "SyntaxError!", "{}", src);
    }
}

#[test]
fn top_level_await_delays_importers() {
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
fn dynamic_import_resolved_and_rejected() {
    let main = "import('./a.js').then(ns => log.push('ns:' + ns.x + ns.default));
        import('bad').catch(e => log.push('bad:' + e.name));
        import('./missing.js').catch(e => log.push('missing:' + e.name + ':' + e.message));
        import('./broken.js').catch(e => log.push('broken:' + e.name));";
    let files = [("a.js", A), ("broken.js", "import { nope } from './a.js';"), ("main.js", main)];
    let mut h = Host::new(&files);
    h.run("main.js").expect("runs");
    let log = h.eval("log.sort().join('|')");
    assert_eq!(log, "bad:TypeError|broken:SyntaxError|missing:TypeError:404 mem:/missing.js|ns:21");
    // from a classic script, too (the referrer is the script's file name)
    h.eval("import('./a.js').then(ns => { out = ns.f() })");
    h.rt.run_jobs().unwrap();
    h.pump();
    assert_eq!(h.eval("out"), "3");
}

#[test]
fn errors_name_the_module_and_line() {
    let mut h = Host::new(&[]);
    let e = h.rt.add_module("mem:/bad.js", "export const a = 1;\nlet x = ;\n").expect_err("syntax error");
    assert_eq!((e.kind, e.name.as_str(), e.line), (ErrorKind::Syntax, "SyntaxError", 2));
    assert!(e.stack.contains("mem:/bad.js:2"), "{}", e.stack);
    let main = "// importing\nimport { nope } from './a.js';\n";
    let mut h = Host::new(&[("a.js", A), ("main.js", main)]);
    let e = h.run("main.js").expect_err("missing export");
    assert_eq!((e.name.as_str(), e.line), ("SyntaxError", 2));
    assert!(e.message.contains("nope") && e.stack.contains("mem:/main.js:2"), "{} / {}", e.message, e.stack);
    let mut h = Host::new(&[("main.js", "import './ghost.js';")]);
    let e = h.run("main.js").expect_err("missing dependency");
    assert_eq!(e.name, "TypeError");
    assert!(e.message.contains("mem:/ghost.js"), "{}", e.message);
    assert!(h.rt.run_module("mem:/never-added.js").is_err());
    let dup = h.rt.add_module("mem:/dup.js", "export const a = 1; export { a };").expect_err("duplicate export");
    assert_eq!(dup.kind, ErrorKind::Syntax);
}
