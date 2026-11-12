//! Promise, job queue ordering, combinators, thenables, queueMicrotask.

pub const CASES: &[(&str, &str)] = &[
    // basics
    ("var out; new Promise(r => r(5)).then(v => { out = v * 2; });", "10"),
    ("var out = []; Promise.resolve(1).then(v => out.push('then ' + v)); out.push('sync');", "sync,then 1"),
    ("var out; Promise.reject(new Error('x')).catch(e => { out = e.message; });", "x"),
    ("var out = typeof Promise + ' ' + Promise.length + ' ' + Object.prototype.toString.call(Promise.resolve());", "function 1 [object Promise]"),
    ("Promise()", "TypeError!"),
    ("new Promise(1)", "TypeError!"),
    ("var out; new Promise(() => { throw new RangeError('boom'); }).catch(e => { out = e.name + ':' + e.message; });", "RangeError:boom"),
    ("var out = []; new Promise((res, rej) => { res(1); rej(2); res(3); }).then(v => out.push('ok' + v), e => out.push('err' + e));", "ok1"),
    ("var out; const p = Promise.resolve(7); out = Promise.resolve(p) === p;", "true"),
    ("var out; const p = new Promise(r => { globalThis.r = r; }); r(p); p.catch(e => { out = e.name; });", "TypeError"),
    // chaining and value propagation
    ("var out; Promise.resolve(1).then(v => v + 1).then(v => v * 10).then(v => { out = v; });", "20"),
    ("var out; Promise.resolve(1).then(v => Promise.resolve(v + 41)).then(v => { out = v; });", "42"),
    ("var out; Promise.resolve('a').then().then(undefined, undefined).then(v => { out = v; });", "a"),
    ("var out; Promise.reject('r').then(v => 'no').then(v => 'no2').catch(e => e + '!').then(v => { out = v; });", "r!"),
    ("var out; Promise.resolve(1).then(() => { throw 'thrown'; }).then(() => 'skipped', e => 'caught ' + e).then(v => { out = v; });", "caught thrown"),
    // ordering puzzles
    ("var out = []; Promise.resolve().then(() => out.push(1)).then(() => out.push(3)); Promise.resolve().then(() => out.push(2)).then(() => out.push(4));", "1,2,3,4"),
    ("var out = []; out.push('a'); queueMicrotask(() => out.push('micro')); Promise.resolve().then(() => out.push('then')); out.push('b');", "a,b,micro,then"),
    ("var out = []; const p = Promise.resolve(); p.then(() => { out.push(1); p.then(() => out.push(3)); }); p.then(() => out.push(2));", "1,2,3"),
    // resolving with a promise costs two extra ticks compared with a plain value
    ("var out = []; new Promise(r => r(Promise.resolve('p'))).then(v => out.push(v)); Promise.resolve().then(() => out.push(1)).then(() => out.push(2)).then(() => out.push(3));", "1,2,p,3"),
    ("var out = []; Promise.resolve().then(() => Promise.resolve('inner')).then(v => out.push(v)); Promise.resolve().then(() => out.push(1)).then(() => out.push(2)).then(() => out.push(3)).then(() => out.push(4));", "1,2,3,inner,4"),
    // thenable assimilation
    ("var out; Promise.resolve({ then(res) { res('from thenable'); } }).then(v => { out = v; });", "from thenable"),
    ("var out = []; const t = { then(res, rej) { out.push('then called'); rej('nope'); res('late'); } }; Promise.resolve(t).then(v => out.push(v), e => out.push('rej ' + e)); out.push('sync');", "sync,then called,rej nope"),
    ("var out; Promise.resolve({ get then() { throw new Error('getter'); } }).catch(e => { out = e.message; });", "getter"),
    ("var out; Promise.resolve({ then(res) { res(1); throw new Error('after'); } }).then(v => { out = 'ok ' + v; }, e => { out = 'bad'; });", "ok 1"),
    ("var out; const o = { then: 5, x: 1 }; Promise.resolve(o).then(v => { out = v === o; });", "true"),
    // finally
    ("var out = []; Promise.resolve(1).finally(() => { out.push('fin'); return 99; }).then(v => out.push(v));", "fin,1"),
    ("var out = []; Promise.reject('e').finally(() => out.push('fin')).catch(e => out.push('caught ' + e));", "fin,caught e"),
    ("var out; Promise.resolve(1).finally(() => { throw 'from finally'; }).catch(e => { out = e; });", "from finally"),
    ("var out; Promise.reject('orig').finally(() => Promise.reject('replaced')).catch(e => { out = e; });", "replaced"),
    ("var out = []; Promise.resolve('v').finally(() => new Promise(r => { out.push('waiting'); r(); })).then(v => out.push(v));", "waiting,v"),
    ("var out; Promise.reject('r').finally().catch(e => { out = 'passed ' + e; });", "passed r"),
    // combinators
    ("var out; Promise.all([1, Promise.resolve(2), { then(r) { r(3); } }]).then(v => { out = v.join('+'); });", "1+2+3"),
    ("var out; Promise.all([]).then(v => { out = Array.isArray(v) + ':' + v.length; });", "true:0"),
    ("var out; Promise.all([Promise.resolve(1), Promise.reject('second fails'), Promise.resolve(3)]).then(v => { out = 'ok'; }, e => { out = e; });", "second fails"),
    ("var out; Promise.all([Promise.reject('a'), Promise.reject('b')]).catch(e => { out = e; });", "a"),
    ("var out; Promise.all(5).catch(e => { out = e.name; });", "TypeError"),
    ("var out; const slow = new Promise(r => queueMicrotask(() => queueMicrotask(() => r('slow')))); Promise.all([slow, 'fast']).then(v => { out = v.join(); });", "slow,fast"),
    ("var out; Promise.allSettled([1, Promise.reject('no'), Promise.resolve('yes')]).then(rs => { out = rs.map(r => r.status + '=' + (r.status === 'fulfilled' ? r.value : r.reason)).join(' '); });", "fulfilled=1 rejected=no fulfilled=yes"),
    ("var out; Promise.race([new Promise(() => {}), Promise.resolve('winner'), Promise.reject('loser')]).then(v => { out = v; });", "winner"),
    ("var out; Promise.race([Promise.reject('first'), Promise.resolve('second')]).then(v => { out = v; }, e => { out = 'rejected ' + e; });", "rejected first"),
    ("var out; Promise.any([Promise.reject(1), Promise.resolve('any ok'), Promise.resolve('later')]).then(v => { out = v; });", "any ok"),
    ("var out; Promise.any([Promise.reject('a'), Promise.reject('b')]).catch(e => { out = [e instanceof AggregateError, e instanceof Error, e.name, e.errors.join('|')].join(); });", "true,true,AggregateError,a|b"),
    ("var out; Promise.any([]).catch(e => { out = e.name + ':' + e.errors.length; });", "AggregateError:0"),
    ("const e = new AggregateError([1, 2], 'msg'); [e.message, e.errors.length, String(e), Object.getPrototypeOf(AggregateError) === Error].join()", "msg,2,AggregateError: msg,true"),
    // subclassing, misc
    ("var out; class P extends Promise {} const p = new P(r => r(3)); out = [p instanceof P, p instanceof Promise].join(); ", "true,true"),
    ("var out = []; queueMicrotask(() => { out.push(1); queueMicrotask(() => out.push(3)); }); queueMicrotask(() => out.push(2));", "1,2,3"),
    ("queueMicrotask(5)", "TypeError!"),
    ("Promise.prototype.then.call({}, x => x)", "TypeError!"),
    ("var out; const p = Promise.reject('late handler'); queueMicrotask(() => p.catch(e => { out = e; }));", "late handler"),
    ("var out = []; const p = new Promise(r => setTimeoutLike(r)); function setTimeoutLike(f) { queueMicrotask(() => f('done')); } p.then(v => out.push(v)); p.then(v => out.push(v + '2'));", "done,done2"),
];
