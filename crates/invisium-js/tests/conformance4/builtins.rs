//! Item 4, first half: text built-ins and the array / object / promise additions.

pub const CASES: &[(&str, &str)] = &[
    // --- String.fromCodePoint
    (r#"[String.fromCodePoint(65, 0x1F600, 0x10FFFF).length, String.fromCodePoint(), String.fromCodePoint(97, 98), String.fromCodePoint.length].join('|')"#, "5||ab|1"),
    (r#"String.fromCodePoint(0x1F600) === '😀' && String.fromCodePoint(0x1F600).codePointAt(0) === 0x1F600"#, "true"),
    (r#"var r = []; for (var v of [-1, 1.5, 0x110000, NaN, Infinity, 'x']) { try { String.fromCodePoint(v); r.push('ok') } catch (e) { r.push(e.name) } } r.join()"#, "RangeError,RangeError,RangeError,RangeError,RangeError,RangeError"),
    // --- Annex B HTML methods
    (r#"['a'.anchor('x"y'), 'a'.big(), 'a'.blink(), 'a'.bold(), 'a'.fixed()].join(' ')"#, "<a name=\"x&quot;y\">a</a> <big>a</big> <blink>a</blink> <b>a</b> <tt>a</tt>"),
    (r#"['a'.fontcolor('red'), 'a'.fontsize(7), 'a'.italics(), 'a'.link('u')].join(' ')"#, "<font color=\"red\">a</font> <font size=\"7\">a</font> <i>a</i> <a href=\"u\">a</a>"),
    (r#"['a'.small(), 'a'.strike(), 'a'.sub(), 'a'.sup(), 'x'.link(), ''.bold.length, ''.anchor.length].join(' ')"#, "<small>a</small> <strike>a</strike> <sub>a</sub> <sup>a</sup> <a href=\"undefined\">x</a> 0 1"),
    (r#"try { String.prototype.bold.call(null) } catch (e) { e.name }"#, "TypeError"),
    (r#"String.prototype.sub.call(42) + String.prototype.link.call(true, 1)"#, "<sub>42</sub><a href=\"1\">true</a>"),
    // --- escape / unescape
    (r#"[escape('a b+c/d@e*f_g-h.i'), escape('ä€'), escape('%'), escape(undefined), escape('"<>&')].join('|')"#, "a%20b+c/d@e*f_g-h.i|%E4%u20AC|%25|undefined|%22%3C%3E%26"),
    (r#"[unescape('%E4%u20AC') === 'ä€', unescape('%zz%u12%'), unescape('%41%u0042c'), unescape(escape('Ā x😀')) === 'Ā x😀'].join('|')"#, "true|%zz%u12%|ABc|true"),
    // --- String.prototype.normalize: Latin
    (r#"['é'.normalize('NFD') === 'é', 'é'.normalize('NFC') === 'é', 'é'.normalize() === 'é', 'é'.normalize(undefined).length].join('|')"#, "true|true|true|1"),
    (r#"var s = 'ÀÉÎÕÜçñÿĀčęőžƠǕǺȘȳ'; var d = s.normalize('NFD'); [d.length > s.length, d.normalize('NFC') === s, /^[\x00-\x7f̀-ͯ]+$/.test(d)].join()"#, "true,true,true"),
    (r#"var s = 'ḀḈḜḮṌṤṸẤẬỘỢỸ'; var d = s.normalize('NFD'); [d.normalize('NFC') === s, 'Ộ'.normalize('NFD') === 'Ộ', 'Ộ'.normalize('NFD').length].join()"#, "true,true,3"),
    (r#"['ậ'.normalize('NFC') === 'ậ', 'ậ'.normalize('NFC') === 'ậ', 'ậ'.normalize('NFD') === 'ậ', 'q̣̇'.normalize('NFC') === 'q̣̇'].join()"#, "true,true,true,true"),
    (r#"['Ǆ'.normalize('NFKC') === 'DŽ', 'Ǆ'.normalize('NFC') === 'Ǆ', '½'.normalize('NFKD') === '1⁄2', 'µ'.normalize('NFKC') === 'μ', 'ſ'.normalize('NFKD'), ' '.normalize('NFKC') === ' '].join()"#, "true,true,true,true,s,true"),
    (r#"['ẛ̣'.normalize('NFC') === 'ẛ̣', 'ẛ̣'.normalize('NFD') === 'ẛ̣', 'ẛ̣'.normalize('NFKC') === 'ṩ', 'ẛ̣'.normalize('NFKD') === 'ṩ'].join()"#, "true,true,true,true"),
    (r#"['̀'.normalize('NFC') === '̀', '̈́'.normalize('NFD') === '̈́', 'Å'.normalize('NFC') === 'Å', 'Å'.normalize('NFC') === 'Å'].join()"#, "true,true,true,true"),
    // --- normalize: Greek, Cyrillic, Hangul, pass-through
    (r#"['ά'.normalize('NFD') === 'ά', 'ά'.normalize('NFC') === 'ά', 'ΐ'.normalize('NFD') === 'ΐ', 'ΐ'.normalize('NFC') === 'ΐ', 'ᾂ'.normalize('NFD').length, 'ᾂ'.normalize('NFD').normalize('NFC') === 'ᾂ'].join()"#, "true,true,true,true,4,true"),
    (r#"['Ӑ'.normalize('NFC') === 'Ӑ', 'й'.normalize('NFD') === 'й', 'ё'.normalize('NFD') === 'ё', 'ЙЁїӹ'.normalize('NFD').normalize('NFC') === 'ЙЁїӹ'].join()"#, "true,true,true,true"),
    (r#"['가'.normalize('NFD') === '가', '가'.normalize('NFC') === '가', '한'.normalize('NFD') === '한', '한'.normalize('NFC') === '한', '힣'.normalize('NFD').normalize('NFKC') === '힣'].join()"#, "true,true,true,true,true"),
    (r#"var s = 'plain ascii 中文 😀 א \ud800'; ['NFC', 'NFD', 'NFKC', 'NFKD'].every(f => s.normalize(f) === s) + '|' + ''.normalize().length + '|' + String.prototype.normalize.length"#, "true|0|0"),
    (r#"var r = []; for (var f of ['NFX', 'nfc', '', null]) { try { 'a'.normalize(f); r.push('ok') } catch (e) { r.push(e.name) } } try { String.prototype.normalize.call(undefined) } catch (e) { r.push(e.name) } r.join()"#, "RangeError,RangeError,RangeError,RangeError,TypeError"),
    // --- Array.prototype.copyWithin / toSpliced
    (r#"[[1, 2, 3, 4, 5].copyWithin(0, 3).join(), [1, 2, 3, 4, 5].copyWithin(1, 0, 3).join(), [1, 2, 3, 4, 5].copyWithin(-2, -5, -3).join(), [1, 2, 3].copyWithin(5, 0).join(), [].copyWithin.length].join('|')"#, "4,5,3,4,5|1,1,2,3,5|1,2,3,1,2|1,2,3|2"),
    (r#"var a = [1, , 3, 4]; a.copyWithin(2, 0, 2); (2 in a) + '|' + (3 in a) + '|' + a.length + '|' + JSON.stringify(Array.prototype.copyWithin.call({ length: 3, 0: 'a', 1: 'b', 2: 'c' }, 1, 0))"#, "true|false|4|{\"0\":\"a\",\"1\":\"a\",\"2\":\"b\",\"length\":3}"),
    (r#"var a = [1, 2, 3, 4]; var b = a.toSpliced(1, 2, 'x', 'y', 'z'); [a.join(), b.join(), a.toSpliced(1).join(), a.toSpliced().join(), a.toSpliced(-1, 0, 9).join(), a.toSpliced(0, 99).length, b !== a].join('|')"#, "1,2,3,4|1,x,y,z,4|1|1,2,3,4|1,2,3,9,4|0|true"),
    (r#"var s = [1, , 3].toSpliced(0, 0); (1 in s) + '|' + s.length + '|' + Array.prototype.toSpliced.call('abc', 1, 1, 'X').join('')"#, "true|3|aXc"),
    // --- Object.groupBy / Map.groupBy
    (r#"var g = Object.groupBy([1, 2, 3, 4, 5], x => x % 2 ? 'odd' : 'even'); [Object.keys(g).join(), g.odd.join(), g.even.join(), Object.getPrototypeOf(g), Array.isArray(g.odd)].join('|')"#, "odd,even|1,3,5|2,4||true"),
    (r#"var g = Object.groupBy('abcab', (ch, i) => ch + (i > 2 ? '!' : '')); JSON.stringify(g)"#, "{\"a\":[\"a\"],\"b\":[\"b\"],\"c\":[\"c\"],\"a!\":[\"a\"],\"b!\":[\"b\"]}"),
    (r#"var k1 = {}, k2 = {}; var m = Map.groupBy([1, 2, 3, 4], x => x % 2 ? k1 : k2); [m instanceof Map, m.size, m.get(k1).join(), m.get(k2).join(), [...Map.groupBy([1.5, 2.5, 1.2, -0, 0], Math.floor).keys()].join()].join('|')"#, "true|2|1,3|2,4|1,2,0"),
    (r#"var r = []; for (var f of [() => Object.groupBy(null, x => x), () => Object.groupBy([1], 5), () => Map.groupBy(5, x => x), () => Object.groupBy([1], () => { throw new RangeError('cb') })]) { try { f(); r.push('ok') } catch (e) { r.push(e.name) } } r.join()"#, "TypeError,TypeError,TypeError,RangeError"),
    // --- Object.hasOwn
    (r#"[Object.hasOwn({ a: 1 }, 'a'), Object.hasOwn({}, 'toString'), Object.hasOwn([1], 0), Object.hasOwn('ab', 'length'), Object.hasOwn(Object.create({ p: 1 }), 'p')].join()"#, "true,false,true,true,false"),
    // --- Promise.withResolvers / Array.fromAsync
    (r#"var r = Promise.withResolvers(); r.promise.then(v => { out = 'got ' + v }); r.resolve(5); r.reject(6); 0"#, "got 5"),
    (r#"var r = Promise.withResolvers(); r.promise.catch(e => { out = [e, r.promise instanceof Promise, typeof r.resolve, Object.keys(r).join()].join('|') }); r.reject('no'); 0"#, "no|true|function|promise,resolve,reject"),
    (r#"Array.fromAsync([1, Promise.resolve(2), 3], x => x * 2).then(a => { out = a.join() + '|' + Array.isArray(a) }); 0"#, "2,4,6|true"),
    (r#"async function* g() { yield 1; yield 2 } Array.fromAsync(g()).then(a => { out = a.join() + '|' + a.length }); 0"#, "1,2|2"),
    (r#"Array.fromAsync({ length: 2, 0: 'a', 1: Promise.resolve('b') }).then(a => { out = a.join() }); 0"#, "a,b"),
    (r#"Array.fromAsync([1], 5).catch(e => { out = e.name }); Array.fromAsync(null).catch(e => { out += '|' + e.name }); typeof Array.fromAsync([]).then"#, "TypeError|TypeError"),
    (r#"Array.fromAsync(new Set([3, 1]), async x => x + 1).then(a => { out = a.join() }); 0"#, "4,2"),
    // --- RegExp.prototype.compile, Date legacy methods, DataCloneError
    (r#"var r = /a/g; r.lastIndex = 5; var same = r.compile('b+', 'i') === r; [same, r.source, r.flags, r.lastIndex, r.test('xBBx'), r.compile().source].join()"#, "true,b+,i,0,true,(?:)"),
    (r#"var r = /a/; r.compile(/x(y)/g); var t = []; try { r.compile(/z/, 'g') } catch (e) { t.push(e.name) } try { r.compile('(', '') } catch (e) { t.push(e.name) } try { RegExp.prototype.compile.call({}, 'a') } catch (e) { t.push(e.name) } r.source + '|' + r.flags + '|' + t.join()"#, "x(y)|g|TypeError,SyntaxError,TypeError"),
    (r#"var d = new Date(2024, 0, 15, 10, 20, 30); var y = d.getYear(); d.setYear(99); [y, d.getFullYear(), d.toGMTString === d.toUTCString || d.toGMTString() === d.toUTCString(), new Date(0).toGMTString()].join('|')"#, "124|1999|true|Thu, 01 Jan 1970 00:00:00 GMT"),
    (r#"var r = []; for (var v of [() => 1, Symbol('s'), new WeakMap()]) { try { structuredClone(v) } catch (e) { r.push(e.name + ':' + (e instanceof Error)) } } r.join()"#, "DataCloneError:true,DataCloneError:true,DataCloneError:true"),
];
