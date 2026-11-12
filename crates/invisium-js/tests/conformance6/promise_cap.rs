//! NewPromiseCapability with foreign constructors: subclasses, userland thenable classes as
//! `Symbol.species` and as the `this` of the statics, bad executors, nullish species, await.

/// Prefixes a case with the userland promise class from the React report.
macro_rules! t {
    ($s:literal) => {
        concat!(
            "function Thenable(executor) { var self = this; executor(function (v) { self.v = v; self.state = 'ok'; }, function (e) { self.e = e; self.state = 'err'; }); } Thenable.prototype.then = function () {}; ",
            $s
        )
    };
}

pub const CASES: &[(&str, &str)] = &[
    // a userland class as Symbol.species of a native promise
    (t!("var p = Promise.resolve(1); p.constructor = { [Symbol.species]: Thenable }; var r = p.then(function () {}); var out = (r instanceof Thenable) + ',' + (r instanceof Promise);"), "true,false"),
    (t!("var p = Promise.resolve(5); p.constructor = { [Symbol.species]: Thenable }; var r = p.then(v => v * 2); var out = () => r.state + r.v;"), "ok10"),
    (t!("var p = Promise.reject(3); p.constructor = { [Symbol.species]: Thenable }; var r = p.then(null, e => { throw e + 1; }); var out = () => r.state + r.e;"), "err4"),
    (t!("var p = Promise.resolve('x'); p.constructor = { [Symbol.species]: Thenable }; var r = p.then(); var out = () => r.state + r.v;"), "okx"),
    (t!("var p = Promise.reject('y'); p.constructor = { [Symbol.species]: Thenable }; var r = p.then(); var out = () => r.state + r.e;"), "erry"),
    (t!("var p = Promise.reject('c'); p.constructor = { [Symbol.species]: Thenable }; var r = p.catch(e => e + e); var out = () => (r instanceof Thenable) + r.v;"), "truecc"),
    (t!("var p = Promise.resolve('f'); p.constructor = { [Symbol.species]: Thenable }; var log = []; var r = p.finally(() => log.push('fin')); var out = () => (r instanceof Thenable) + ',' + log;"), "true,fin"),
    // React-style: the foreign capability is attached while the promise is still pending
    (t!("var res; var p = new Promise(r => res = r); p.constructor = { [Symbol.species]: Thenable }; var r = p.then(v => v + '!'); var before = r.state; res('done'); var out = () => before + ',' + r.v;"), "undefined,done!"),
    (t!("var res; var p = new Promise(r => res = r); p.constructor = { [Symbol.species]: Thenable }; var seen = []; var t = p.then(v => { seen.push(v); return v; }); Promise.resolve().then(() => res('late')); var out = () => seen + ':' + t.v;"), "late:late"),
    (t!("var p = Promise.resolve(6); p.constructor = { [Symbol.species]: Thenable }; async function f() { return p; } var out; f().then(v => { out = v; });"), "6"),
    (t!("var p = Promise.resolve(5); p.constructor = { [Symbol.species]: Thenable }; var out; (async () => { out = await p; })();"), "5"),
    // a subclass through then / catch / finally
    ("class P extends Promise {} var r = P.resolve(1).then(v => v); var out = (r instanceof P) + ',' + (r.constructor === P);", "true,true"),
    ("class P extends Promise {} var out; P.resolve(2).then(v => v * 3).then(v => { out = v; });", "6"),
    ("class P extends Promise {} var r = P.reject(1).catch(e => e); var out = r instanceof P;", "true"),
    ("class P extends Promise {} var r = P.resolve(1).finally(() => {}); var out = r instanceof P;", "true"),
    ("class P extends Promise {} var out; P.resolve(4).finally(() => 9).then(v => { out = v; });", "4"),
    ("class P extends Promise {} var out; P.reject('e').finally(() => 1).catch(v => { out = v; });", "e"),
    ("var n = 0; class P extends Promise { constructor(ex) { n++; super(ex); } } P.resolve(1).then(() => {}); var out = n;", "2"),
    // a subclass through every static
    ("class P extends Promise {} var a = P.resolve(1); var out = (a instanceof P) + ',' + (P.resolve(a) === a) + ',' + (Promise.resolve(a) === a);", "true,true,false"),
    ("class P extends Promise {} var r = P.reject(1); r.catch(() => {}); var out = r instanceof P;", "true"),
    ("class P extends Promise {} var out; var r = P.all([1, P.resolve(2), Promise.resolve(3)]); r.then(v => { out = (r instanceof P) + ':' + v; });", "true:1,2,3"),
    ("class P extends Promise {} var out; var r = P.allSettled([1, P.reject(2)]); r.then(v => { out = (r instanceof P) + ':' + v.map(x => x.status); });", "true:fulfilled,rejected"),
    ("class P extends Promise {} var out; var r = P.race([P.resolve('a'), 'b']); r.then(v => { out = (r instanceof P) + v; });", "truea"),
    ("class P extends Promise {} var out; var r = P.any([P.reject(1), P.resolve(2)]); r.then(v => { out = (r instanceof P) + ':' + v; });", "true:2"),
    ("class P extends Promise {} var out; P.any([P.reject(1)]).catch(e => { out = e.constructor.name + e.errors; });", "AggregateError1"),
    ("class P extends Promise {} var w = P.withResolvers(); var out; w.promise.then(v => { out = (w.promise instanceof P) + v; }); w.resolve('r');", "truer"),
    // a userland class as the `this` of the statics
    (t!("var r = Promise.resolve.call(Thenable, 7); var out = () => (r instanceof Thenable) + r.state + r.v;"), "trueok7"),
    (t!("var r = Promise.reject.call(Thenable, 8); var out = () => r.state + r.e;"), "err8"),
    (t!("var r = Promise.all.call(Thenable, [1]); var out = () => r.state + (r.e instanceof TypeError);"), "errtrue"),
    ("function T2(ex) { var s = this; ex(v => { s.v = v; }, e => { s.e = e; }); } T2.resolve = v => Promise.resolve(v); var r = Promise.all.call(T2, [1, 2]); var out = () => (r instanceof T2) + ':' + r.v;", "true:1,2"),
    ("function T2(ex) { var s = this; ex(v => { s.v = v; }, e => { s.e = e; }); } T2.resolve = v => Promise.resolve(v); var r = Promise.race.call(T2, [Promise.resolve('w'), new Promise(() => {})]); var out = () => r.v;", "w"),
    ("function T2(ex) { var s = this; ex(v => { s.v = v; }, e => { s.e = e; }); } T2.resolve = v => Promise.resolve(v); var r = Promise.allSettled.call(T2, [Promise.reject(1)]); var out = () => r.v[0].status;", "rejected"),
    ("function T2(ex) { var s = this; ex(v => { s.v = v; }, e => { s.e = e; }); } T2.resolve = v => Promise.resolve(v); var r = Promise.any.call(T2, [Promise.reject(1)]); var out = () => r.e.constructor.name;", "AggregateError"),
    (t!("var w = Promise.withResolvers.call(Thenable); w.resolve(3); var out = (w.promise instanceof Thenable) + ',' + w.promise.v;"), "true,3"),
    // bad executors: TypeError
    ("function S() {} var p = Promise.resolve(); p.constructor = { [Symbol.species]: S }; var out; try { p.then(); } catch (e) { out = e.constructor.name; }", "TypeError"),
    ("function S(ex) { ex(1, 2); } var p = Promise.resolve(); p.constructor = { [Symbol.species]: S }; var out; try { p.then(); } catch (e) { out = e.constructor.name; }", "TypeError"),
    ("function S(ex) { ex(function () {}); } var p = Promise.resolve(); p.constructor = { [Symbol.species]: S }; var out; try { p.then(); } catch (e) { out = e.constructor.name; }", "TypeError"),
    ("function S(ex) { ex(function () {}, function () {}); ex(function () {}, function () {}); } var p = Promise.resolve(); p.constructor = { [Symbol.species]: S }; var out; try { p.then(); } catch (e) { out = e.constructor.name; }", "TypeError"),
    ("function S() {} var out; try { Promise.resolve.call(S, 1); } catch (e) { out = e.constructor.name; }", "TypeError"),
    ("function S(ex) { ex(); ex(function () {}, function () {}); this.ok = 1; } var p = Promise.resolve(); p.constructor = { [Symbol.species]: S }; var out = p.then().ok;", "1"),
    // nullish species and constructor fall back to the intrinsic
    ("var p = Promise.resolve(1); p.constructor = { [Symbol.species]: undefined }; var out = Object.getPrototypeOf(p.then()) === Promise.prototype;", "true"),
    ("var p = Promise.resolve(1); p.constructor = { [Symbol.species]: null }; var out = Object.getPrototypeOf(p.then()) === Promise.prototype;", "true"),
    ("var p = Promise.resolve(1); p.constructor = undefined; var out = Object.getPrototypeOf(p.then()) === Promise.prototype;", "true"),
    ("var p = Promise.resolve(1); p.constructor = { [Symbol.species]: 5 }; var out; try { p.then(); } catch (e) { out = e.constructor.name; }", "TypeError"),
    // a resolve function that throws inside a job is reported, not fatal
    ("function S(ex) { ex(() => { throw 'bad'; }, () => {}); } var p = Promise.resolve(1); p.constructor = { [Symbol.species]: S }; p.then(v => v); var out = 'survived';", "survived"),
];
