//! F5 (module part): huge cycles, self-imports, deep re-export chains and deeply nested
//! dynamic imports end in results or errors, never a panic, a native stack overflow or an
//! unbounded loop. Runs on the default 2 MiB test-thread stack.

use invisium_js::{ErrorKind, Limits, Runtime};

fn runtime() -> Runtime {
    let mut rt = Runtime::new(Limits::default());
    rt.set_module_resolver(Box::new(|spec: &str, _: &str| Some(format!("mem:/{}", spec.trim_start_matches("./")))));
    rt.eval("var out;", "setup.js").expect("setup");
    rt
}

fn eval(rt: &mut Runtime, src: &str) -> String {
    match rt.eval(src, "check.js") {
        Ok(v) => rt.to_string_lossy(v),
        Err(e) => format!("{}!", e.name),
    }
}

#[test]
fn an_import_cycle_of_ten_thousand_modules() {
    const N: usize = 10_000;
    let mut rt = runtime();
    for i in 0..N {
        let src = format!("import {{ v as w }} from './m{}.js'; export const v = {}; export function next() {{ return w }}", (i + 1) % N, i);
        rt.add_module(&format!("mem:/m{}.js", i), &src).expect("module compiles");
    }
    rt.run_module("mem:/m0.js").expect("the cycle links and runs");
    rt.add_module("mem:/main.js", "import { next } from './m0.js'; out = next();").unwrap();
    rt.run_module("mem:/main.js").unwrap();
    rt.run_jobs().unwrap();
    assert_eq!(eval(&mut rt, "out"), "1");
}

#[test]
fn a_ten_thousand_deep_re_export_chain_resolves_iteratively() {
    const N: usize = 10_000;
    let mut rt = runtime();
    for i in 0..N {
        let src = if i + 1 == N { "export const leaf = 1;".to_string() } else { format!("export * from './c{}.js'; export {{ leaf as l{} }} from './c{}.js';", i + 1, i, i + 1) };
        rt.add_module(&format!("mem:/c{}.js", i), &src).unwrap();
    }
    rt.add_module("mem:/main.js", "import { leaf, l0 } from './c0.js'; import * as ns from './c9990.js'; out = [leaf, l0, Object.keys(ns).join(' ')].join();").unwrap();
    rt.run_module("mem:/main.js").expect("links");
    rt.run_jobs().unwrap();
    assert_eq!(eval(&mut rt, "out"), "1,1,l9990 l9991 l9992 l9993 l9994 l9995 l9996 l9997 l9998 leaf");
    // a missing name in the same chain is a SyntaxError naming it
    rt.add_module("mem:/miss.js", "import { nope } from './c0.js';").unwrap();
    let e = rt.run_module("mem:/miss.js").expect_err("no such export");
    assert_eq!(e.kind, ErrorKind::Syntax);
    assert!(e.message.contains("nope"));
}

#[test]
fn a_module_that_imports_itself() {
    let mut rt = runtime();
    rt.add_module("mem:/self.js", "import { x as y, f as g } from './self.js'; import * as me from './self.js'; export const x = 1; export function f() { return 'f' } out = [typeof y, g(), me.x].join();").unwrap();
    rt.run_module("mem:/self.js").unwrap();
    rt.run_jobs().unwrap();
    assert_eq!(eval(&mut rt, "out"), "number,f,1");
    rt.add_module("mem:/loop.js", "export * from './loop.js'; import { y } from './loop.js';").unwrap();
    assert_eq!(rt.run_module("mem:/loop.js").expect_err("no such export").kind, ErrorKind::Syntax);
}

#[test]
fn deeply_nested_dynamic_imports() {
    let mut rt = runtime();
    let src = "globalThis.n = (globalThis.n || 0) + 1; const k = n; if (k < 300) await import('./d.js?' + k); out = n;";
    let url = |u: &str| u.split('?').next().unwrap_or(u).to_string();
    rt.add_module("mem:/d.js", src).unwrap();
    let p = rt.run_module("mem:/d.js").unwrap();
    let root = rt.root(p);
    rt.run_jobs().unwrap();
    let mut rounds = 0;
    loop {
        let reqs = rt.take_import_requests();
        if reqs.is_empty() {
            break;
        }
        rounds += 1;
        for (id, u) in reqs {
            assert_eq!(url(&u), "mem:/d.js");
            if !rt.has_module(&u) {
                rt.add_module(&u, src).unwrap();
            }
            rt.finish_import(id, Ok(()));
        }
        rt.run_jobs().unwrap();
    }
    rt.unroot(root);
    assert_eq!(rounds, 299);
    assert_eq!(eval(&mut rt, "out"), "300");
    // an import storm stops on the step budget
    let mut rt = Runtime::new(Limits { step_budget: 200_000, ..Limits::default() });
    let e = rt.eval("for (;;) import('x' + Math.random())", "storm.js").expect_err("storm");
    assert_eq!(e.kind, ErrorKind::StepBudget);
    assert!(!rt.take_import_requests().is_empty());
}
