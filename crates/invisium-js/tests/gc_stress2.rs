//! Milestone 3d: GC stress beyond the corpus. Programs aimed at the places where a value can
//! live only in a Rust local while the engine runs script (and so may collect), each run with
//! a collection at every collection point.

use invisium_js::{Limits, Runtime, Value};

fn stressed() -> Runtime {
    let mut rt = Runtime::new(Limits::default());
    rt.set_gc_stress(1);
    rt
}

fn run(src: &str) -> String {
    let mut rt = stressed();
    let r = match rt.eval(src, "probe.js") {
        Ok(v) => {
            let root = rt.root(v);
            let r = match rt.run_jobs() {
                Ok(()) => {
                    let out = rt.eval("typeof out === 'undefined' ? undefined : out", "out.js").unwrap_or(Value::Undefined);
                    if out == Value::Undefined { rt.to_string_lossy(v) } else { rt.to_string_lossy(out) }
                }
                Err(e) => format!("{}!", e.name),
            };
            rt.unroot(root);
            r
        }
        Err(e) => format!("{}!", e.name),
    };
    r
}

const PROBES: &[(&str, &str)] = &[
    // sort comparator, array callbacks
    ("[3, 1, 2].map(x => ({ v: x })).sort((a, b) => ({ d: a.v - b.v }).d).map(o => o.v).join()", "1,2,3"),
    ("['b', 'a'].map(s => s + s).sort((a, b) => (a + '!').localeCompare(b + '!')).join()", "aa,bb"),
    ("[1, 2, 3].reduce((acc, x) => ({ s: acc.s + x, pad: [x, x] }), { s: 0 }).s", "6"),
    ("[1, 2].flatMap(x => [{ x }, { x: x * 10 }]).map(o => o.x).join()", "1,10,2,20"),
    ("Array.from({ length: 3 }, (_, i) => ({ i })).map(o => o.i).join()", "0,1,2"),
    ("var a = [{ k: 1 }, { k: 2 }]; Array.from(a, (x, i) => { a.length = 0; return x.k + i }).join()", "1,3"),
    ("Array.from((function* () { yield { a: 1 }; yield { a: 2 } })(), o => [o.a]).join()", "1,2"),
    // JSON
    ("JSON.stringify(JSON.parse('{\"a\":[1,{\"b\":2}],\"c\":\"xy\"}', (k, v) => typeof v === 'number' ? { n: v } : v))", "{\"a\":[{\"n\":1},{\"b\":{\"n\":2}}],\"c\":\"xy\"}"),
    ("JSON.stringify({ a: 1, b: [2] }, (k, v) => typeof v === 'number' ? 'n' + v : v)", "{\"a\":\"n1\",\"b\":[\"n2\"]}"),
    ("JSON.stringify({ x: { toJSON() { return { y: [1, 2].map(n => 'v' + n) } } } })", "{\"x\":{\"y\":[\"v1\",\"v2\"]}}"),
    // promise combinators
    ("var out; Promise.all([1, 2].map(x => Promise.resolve({ x }))).then(v => { out = v.map(o => o.x).join() });", "1,2"),
    ("var out; Promise.allSettled([Promise.reject({ e: 1 }), { then(r) { r({ ok: 2 }) } }]).then(v => { out = v[0].reason.e + ',' + v[1].value.ok });", "1,2"),
    ("var out; Promise.any([Promise.reject(1), Promise.resolve({ w: 'w' })]).then(v => { out = v.w });", "w"),
    ("var out; Promise.race([new Promise(r => r({ r: 'r' }))]).then(v => { out = v.r });", "r"),
    // construction
    ("function F(a) { this.a = a } var o = Reflect.construct(F, [{ q: 7 }]); o.a.q", "7"),
    ("class A { constructor(x) { this.x = x } } class B extends A { constructor() { super({ v: 'b' }); this.y = [1] } } var b = new B(); b.x.v + b.y.length", "b1"),
    ("class A { f = { k: 'f' }; m() { return this.f.k } } class B extends A { g = [this.m()] } new B().g[0]", "f"),
    ("var P = new Proxy(function () {}, { construct(t, a) { return { made: a[0].s } } }); new P({ s: 'p' }).made", "p"),
    // iteration, spread, destructuring
    ("function* g() { yield { a: 1 }; yield 'x' + 'y'; } var [p, q] = g(); p.a + q", "1xy"),
    ("function* g() { var t = { keep: 'k' }; yield 1; yield t.keep; } var it = g(); it.next(); it.next().value", "k"),
    ("function* g() { yield* [{ v: 1 }, { v: 2 }] } Math.max(...[...g()].map(o => o.v))", "2"),
    ("var m = new Map((function* () { yield [{ k: 1 }, { v: 2 }] })()); [...m].map(([k, v]) => k.k + v.v).join()", "3"),
    ("var { a, ...rest } = { a: { x: 1 }, b: { y: 2 }, c: 'cc' + 'c' }; a.x + rest.b.y + rest.c", "3ccc"),
    ("var it = { [Symbol.iterator]() { var i = 0; return { next: () => ({ value: { i: i++ }, done: i > 3 }) } } }; [...it].map(o => o.i).join()", "0,1,2"),
    // strings, regexps, templates
    ("'a-b-c'.replace(/(\\w)/g, (m, c) => ({ s: c.toUpperCase() + c }).s)", "Aa-Bb-Cc"),
    ("'abc'.replaceAll('b', () => [1, 2].join('+'))", "a1+2c"),
    ("function tag(s, ...v) { return s.raw.join('|') + v.map(x => x.n).join() } tag`a${{ n: 1 }}b${{ n: 2 }}c`", "a|b|c1,2"),
    ("var o1 = { toString() { return 'x' + 'y' } }, o2 = { toString() { return 'z' + 'w' } }; o1 + o2", "xyzw"),
    ("var o = { valueOf() { return { } }, toString() { return 'p' + 'q' } }; `${o}` + (o + '')", "pqpq"),
    ("var a = { valueOf() { return 2n ** 70n } }, b = { valueOf() { return [1].length ? 1n : 0n } }; String(a + b)", "1180591620717411303425"),
    ("var a = { valueOf() { return 'a' + 'b' } }, b = { valueOf() { return 'c' + 'd' } }; [a < b, b < a].join()", "true,false"),
    // VM operations that convert a key or operand by running script
    ("var k = { toString() { return 'k' } }; var o = {}; o[k] = { v: 1 }; o.k.v", "1"),
    ("var k = { toString() { return 'k' } }; ({ k: 'got' })[k]", "got"),
    ("var k = { toString() { return 'k' } }; var o = { [k]: { v: 'lit' } }; o.k.v", "lit"),
    ("var k = { toString() { return 'm' } }; class C { [k]() { return 'cm' } } new C().m()", "cm"),
    ("var k = { toString() { return 'k' } }; var o = { k: 'a' }; o[k] += 'b' + 'c'; o.k", "abc"),
    ("var k = { toString() { return 'k' } }; k in { k: 1 }", "true"),
    ("class X { static [Symbol.hasInstance](v) { return v.tag === 't' + 't' } } ({ tag: 'tt' }) instanceof X", "true"),
    ("var { a = { d: 'def' }, b = [a.d] } = {}; b[0]", "def"),
    ("function F(...xs) { this.n = xs.map(x => x.v).join() } new F(...[{ v: 1 }, { v: 2 }]).n", "1,2"),
    ("var o = { set s(v) { this.got = v.w } }; o.s = { w: 'set' }; o.got", "set"),
    // getters from natives, typed arrays, clone, eval
    ("var src = { get a() { return { v: 'A' } }, get b() { return ['B'] } }; var t = Object.assign({}, src); t.a.v + t.b[0]", "AB"),
    ("Object.entries({ get g() { return 'g' + 'h' }, k: { z: 1 } }).map(([k, v]) => k + (typeof v)).join()", "gstring,kobject"),
    ("new Uint8Array((function* () { yield 1; yield 2 })()).join()", "1,2"),
    ("var c = structuredClone({ a: [{ b: 'bb' }], m: new Map([[1, { z: 2 }]]) }); c.a[0].b + c.m.get(1).z", "bb2"),
    ("eval('var e = { v: [1, 2] }; e.v.map(x => x * 2)').join()", "2,4"),
    ("(0, eval)('({ n: \"in\" + \"direct\" })').n", "indirect"),
    ("new Function('a', 'return { r: a + 1 }')(1).r", "2"),
    ("class P extends Promise {} var out; (async () => { var r = await P.resolve({ v: 'x' }); out = r.v + 'y' })();", "xy"),
    ("var out; var p = new Proxy(function (a) { return { s: a.t + '!' } }, { apply: Reflect.apply }); out = p({ t: 'ap' }).s", "ap!"),
    ("var P = new Proxy(function F(a) { this.a = a }, { construct: Reflect.construct }); new P({ q: 'c' }).a.q", "c"),
    ("var o = { k: 1 }; var px = new Proxy(o, { get: Reflect.get, set: Reflect.set, ownKeys: Reflect.ownKeys }); px.n = { z: 'z' }; Object.keys(px).join() + px.n.z", "k,nz"),
    // the reported pattern
    ("function F() {} F.prototype = { text() { return 't' }, attr() { return 'a' } }; var s = ''; for (var i = 0; i < 50; i++) { var junk = [{}, {}, 'j' + i]; s = new F().text() + new F().attr() } s", "ta"),
    // BigInt methods after BigInt is gone from the global object
    ("delete globalThis.BigInt; [1, 2, 3].map(x => ({ x })); (5n).toString() + typeof (5n).toLocaleString", "5function"),
    // Intl.Segmenter's segments prototype is reachable only through the engine
    ("[...new Intl.Segmenter().segment('ab')].map(s => s.segment).join()", "a,b"),
];

#[test]
fn stress_probes() {
    let mut failed = 0;
    for (src, want) in PROBES {
        let got = run(src);
        if got != *want {
            failed += 1;
            eprintln!("PROBE FAIL: {}\n   want: {:?}\n    got: {:?}", src, want, got);
        }
    }
    assert_eq!(failed, 0, "{} of {} probes failed under GC stress", failed, PROBES.len());
}
