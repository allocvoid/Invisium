//! Item 1: error messages that name the culprit, and stack traces.

pub const CASES: &[(&str, &str)] = &[
    // --- "x is not a function" names the callee expression
    (r#"var x = {}; try { x.foo() } catch (e) { e.name + ': ' + e.message }"#, "TypeError: x.foo is not a function"),
    (r#"var foo; try { foo() } catch (e) { e.message }"#, "foo is not a function"),
    (r#"var foo = 5; try { foo(1, 2) } catch (e) { e.message }"#, "foo is not a function"),
    (r#"var o = { a: { b: {} } }; try { o.a.b.c() } catch (e) { e.message }"#, "o.a.b.c is not a function"),
    (r#"var o = { a: {} }; try { o.a['b']() } catch (e) { e.message }"#, "o.a.b is not a function"),
    (r#"var o = {}, k = 'zz'; try { o[k]() } catch (e) { e.message }"#, "o[k] is not a function"),
    (r#"var list = [1]; try { list[0]() } catch (e) { e.message }"#, "list[0] is not a function"),
    (r#"function C() { this.v = 1 } C.prototype.run = function () { return this.go() }; try { new C().run() } catch (e) { e.message }"#, "this.go is not a function"),
    (r#"function f() { return {} } try { f().g() } catch (e) { e.message }"#, "f(...).g is not a function"),
    (r#"try { (function () { return 1 })()() } catch (e) { e.message }"#, "(intermediate value)(...) is not a function"),
    (r#"var o = { f: 1 }; try { o.f(...[1, 2]) } catch (e) { e.message }"#, "o.f is not a function"),
    (r#"var tag; try { tag`x` } catch (e) { e.message }"#, "tag is not a function"),
    (r#"var o = {}; try { o?.a.b() } catch (e) { e.message }"#, "Cannot read properties of undefined (reading 'b')"),
    (r#"var o = { a: {} }; try { o?.a.b() } catch (e) { e.message }"#, "o?.a.b is not a function"),
    // --- "x is not a constructor"
    (r#"var x = {}; try { new x.foo() } catch (e) { e.name + ': ' + e.message }"#, "TypeError: x.foo is not a constructor"),
    (r#"var Foo; try { new Foo(1) } catch (e) { e.message }"#, "Foo is not a constructor"),
    (r#"var arrow = () => 1; try { new arrow() } catch (e) { e.message }"#, "arrow is not a constructor"),
    (r#"var ns = { K: 5 }; try { new ns.K(...[1]) } catch (e) { e.message }"#, "ns.K is not a constructor"),
    // --- property reads on undefined / null
    (r#"var o = {}; try { o.a.bar } catch (e) { e.message }"#, "Cannot read properties of undefined (reading 'bar')"),
    (r#"var n = null; try { n.bar() } catch (e) { e.message }"#, "Cannot read properties of null (reading 'bar')"),
    (r#"var o = {}; try { o.a[0] } catch (e) { e.message }"#, "Cannot read properties of undefined (reading '0')"),
    (r#"var o = {}; try { o.a.b = 1 } catch (e) { e.message }"#, "Cannot set properties of undefined (setting 'b')"),
    // --- "x is not iterable"
    (r#"var a = 1; try { for (var q of a) {} } catch (e) { e.message }"#, "a is not iterable"),
    (r#"var cfg = {}; try { for (var q of cfg.items) {} } catch (e) { e.message }"#, "cfg.items is not iterable"),
    (r#"var u; try { [...u] } catch (e) { e.message }"#, "u is not iterable"),
    (r#"var o = {}; try { Math.max(...o.list) } catch (e) { e.message }"#, "o.list is not iterable"),
    (r#"try { new Map(5) } catch (e) { e.message }"#, "5 is not iterable"),
    (r#"try { var [p] = {} } catch (e) { e.message }"#, "object is not iterable"),
    // --- "Cannot convert undefined or null to object"
    (r#"try { Object.keys(undefined) } catch (e) { e.message }"#, "Cannot convert undefined or null to object"),
    (r#"try { Object.assign(null, {}) } catch (e) { e.message }"#, "Cannot convert undefined or null to object"),
    (r#"try { Object.getPrototypeOf(null) } catch (e) { e.message }"#, "Cannot convert undefined or null to object"),
    // --- stack traces: one line per active frame, innermost first
    (r#"function a() { return new Error('boom').stack } function b() { return a() } function c() { return b() }
c()"#, "Error: boom\n    at a (case.js:1:23)\n    at b (case.js:1:72)\n    at c (case.js:1:100)\n    at <anonymous> (case.js:2:2)"),
    (r#"function inner() { null.x }
function middle() { inner() }
function outer() { middle() }
try { outer() } catch (e) { e.stack }"#, "TypeError: Cannot read properties of null (reading 'x')\n    at inner (case.js:1:24)\n    at middle (case.js:2:26)\n    at outer (case.js:3:26)\n    at <anonymous> (case.js:4:12)"),
    (r#"var o = { m() { return (() => new Error('in arrow').stack)() } }; o.m().split('\n').map(l => l.trim().split(' ')[1]).join()"#, "in,<anonymous>,m,<anonymous>"),
    (r#"class A { constructor() { this.s = new Error('ctor').stack } } new A().s.split('\n')[1].trim()"#, "at A (case.js:1:36)"),
    (r#"[1].map(function cb() { return new Error('cb').stack })[0].split('\n').slice(1).map(l => l.trim().split(' ')[1]).join()"#, "cb,<anonymous>"),
    (r#"function r(n) { return n ? r(n - 1) : new Error('deep').stack } r(50).split('\n').length"#, "11"),
    (r#"function r(n) { return n ? r(n - 1) : new Error('deep').stack } var s = r(390); s.split('\n').length + '|' + (s.length < 400)"#, "11|true"),
    (r#"var e = new RangeError('bad'); e.stack.split('\n')[0] + '|' + (typeof e.stack) + '|' + Object.getOwnPropertyDescriptor(e, 'stack').enumerable"#, "RangeError: bad|string|false"),
    (r#"class MyErr extends Error { constructor(m) { super(m); this.name = 'MyErr' } } function f() { throw new MyErr('x') } try { f() } catch (e) { e.stack.split('\n').slice(0, 3).map(l => l.trim()).join('|') }"#, "Error: x|at MyErr (case.js:1:46)|at f (case.js:1:101)"),
    // --- Error.stackTraceLimit and Error.captureStackTrace
    (r#"Error.stackTraceLimit"#, "10"),
    (r#"Error.stackTraceLimit = 2; function r(n) { return n ? r(n - 1) : new Error('lim').stack } r(20)"#, "Error: lim\n    at r (case.js:1:66)\n    at r (case.js:1:56)"),
    (r#"Error.stackTraceLimit = 100; function r(n) { return n ? r(n - 1) : new Error('lim').stack } r(40).split('\n').length"#, "11"),
    (r#"Error.stackTraceLimit = 0; function f() { return new Error('none').stack } f()"#, "Error: none"),
    (r#"function f() { var o = { name: 'MyErr', message: 'custom' }; Error.captureStackTrace(o); return o.stack } f()"#, "MyErr: custom\n    at f (case.js:1:85)\n    at <anonymous> (case.js:1:108)"),
    (r#"function outer() { return inner() } function inner() { var o = {}; Error.captureStackTrace(o, inner); return o.stack } outer()"#, "Error\n    at outer (case.js:1:32)\n    at <anonymous> (case.js:1:125)"),
    (r#"typeof Error.captureStackTrace + '|' + Error.captureStackTrace.length + '|' + (function () { try { Error.captureStackTrace(1) } catch (e) { return e.name } })()"#, "function|2|TypeError"),
    (r#"function Legacy(m) { this.message = m; Error.captureStackTrace(this, Legacy) } Legacy.prototype = Object.create(Error.prototype); Legacy.prototype.name = 'Legacy'; function mk() { return new Legacy('old') } mk().stack.split('\n').slice(0, 2).map(l => l.trim()).join('|')"#, "Legacy: old|at mk (case.js:1:188)"),
];
