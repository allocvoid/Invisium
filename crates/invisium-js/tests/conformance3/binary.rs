//! ArrayBuffer, the nine typed arrays and DataView: element conversion, views sharing one
//! buffer, bounds, the array-like methods and endianness.

pub const CASES: &[(&str, &str)] = &[
    // ---- construction and element conversion ----
    (r#"var a = new Uint8Array(4); a.length + ':' + a.byteLength + ':' + a.byteOffset + ':' + a.buffer.byteLength + ':' + a.join()"#, "4:4:0:4:0,0,0,0"),
    (r#"new Uint8Array([1, 2, 300, -1, 1.9]).join()"#, "1,2,44,255,1"),
    (r#"new Uint8ClampedArray([300, -5, 1.5, 2.5, 0.5, NaN, 254.5, 255.5]).join()"#, "255,0,2,2,0,0,254,255"),
    (r#"new Int8Array([127, 128, 255, -129]).join() + '|' + new Int16Array([32768, -32769, 65535]).join() + '|' + new Uint16Array([-1, 65536]).join()"#, "127,-128,-1,127|-32768,32767,-1|65535,0"),
    (r#"new Int32Array([2147483648, -2147483649, 4294967295]).join() + '|' + new Uint32Array([-1, 4294967296, 1e10]).join()"#, "-2147483648,2147483647,-1|4294967295,0,1410065408"),
    (r#"var f = new Float32Array([1.1, 16777217, 1e40, -0]); [f[0] === 1.1, f[0].toFixed(4), f[1], f[2], Object.is(f[3], -0)].join()"#, "false,1.1000,16777216,Infinity,true"),
    (r#"var d = new Float64Array([0.1, NaN, -Infinity]); d[0] + ':' + d[1] + ':' + d[2] + ':' + Float64Array.BYTES_PER_ELEMENT + ':' + d.BYTES_PER_ELEMENT + ':' + Int16Array.BYTES_PER_ELEMENT"#, "0.1:NaN:-Infinity:8:8:2"),
    (r#"[new Uint8Array().length, new Uint8Array(undefined).length, new Uint8Array('3').length, new Uint8Array({ length: 2, 0: 7, 1: 8 }).join(), new Uint8Array(new Set([5, 6])).join(), new Int8Array(new Uint8Array([255, 1])).join()].join('|')"#, "0|0|3|7,8|5,6|-1,1"),
    (r#"var e = []; for (var f of [() => new Uint8Array(-1), () => new Uint32Array(new ArrayBuffer(6)), () => new Uint16Array(new ArrayBuffer(8), 1), () => new Uint8Array(new ArrayBuffer(4), 2, 3), () => new Uint8Array(new ArrayBuffer(4), 5), () => Uint8Array(2), () => new ArrayBuffer(-1), () => new ArrayBuffer(1e12), () => new Float64Array(1e12)]) { try { f(); e.push('ok'); } catch (x) { e.push(x.name); } } e.join()"#, "RangeError,RangeError,RangeError,RangeError,RangeError,TypeError,RangeError,RangeError,RangeError"),
    (r#"var a = new Int16Array(3); a[0] = '12'; a[1] = { valueOf() { return 7; } }; a[2] = true; a.join() + '|' + (delete a[0]) + '|' + (delete a[9])"#, "12,7,1|false|true"),
    (r#"var a = new Uint8Array(1); try { a[0] = Symbol(); } catch (e) { e.name }"#, "TypeError"),
    (r#"var a = new Uint8Array(3); a[0] = 1; a[1] = a[0] + 1; a[2] = a[1] * 200; var s = 0; for (var i = 0; i < a.length; i++) s += a[i]; s + '|' + a[2]"#, "147|144"),
    // ---- views sharing one buffer ----
    (r#"var buf = new ArrayBuffer(8); var u8 = new Uint8Array(buf); var u32 = new Uint32Array(buf); var u16 = new Uint16Array(buf, 4, 2); u32[0] = 0x01020304; u16[1] = 0xABCD; u8.join() + '|' + u32[1] + '|' + (u8.buffer === u32.buffer) + '|' + u16.byteOffset + ':' + u16.length"#, "4,3,2,1,0,0,205,171|2882338816|true|4:2"),
    (r#"var u8 = new Uint8Array([1, 2, 3, 4]); var f = new Float32Array(u8.buffer); f[0] = 1; u8.join()"#, "0,0,128,63"),
    (r#"var b = new ArrayBuffer(16); var a = new Float64Array(b, 8, 1); a[0] = 1; new Uint8Array(b).slice(8).join() + '|' + a.byteLength + '|' + new Float64Array(b).length + '|' + new Uint8Array(b, 16).length"#, "0,0,0,0,0,0,240,63|8|2|0"),
    (r#"var a = new Uint8Array([1, 2, 3, 4, 5]); var s = a.subarray(1, 4); var c = a.slice(1, 4); s[0] = 9; c[1] = 7; a.join() + '|' + s.join() + '|' + c.join() + '|' + s.byteOffset + '|' + (s.buffer === a.buffer) + '|' + (c.buffer === a.buffer)"#, "1,9,3,4,5|9,3,4|2,7,4|1|true|false"),
    (r#"var a = new Uint8Array([1, 2, 3, 4, 5]); a.subarray(-2).join() + '|' + a.subarray(1, -1).join() + '|' + a.slice(-3, -1).join() + '|' + a.subarray(4, 2).length + '|' + a.subarray(1).subarray(1).byteOffset"#, "4,5|2,3,4|3,4|0|2"),
    (r#"var i16 = new Int16Array(2); var u8 = new Uint8Array(i16.buffer); i16[0] = -2; i16[1] = 258; u8.join() + '|' + new Int8Array(i16.buffer)[0]"#, "254,255,2,1|-2"),
    (r#"var b = new Uint8Array([1, 2, 3, 4]).buffer; var s = b.slice(1, 3); var t = b.slice(-1); [s.byteLength, new Uint8Array(s).join(), new Uint8Array(t).join(), b.slice(3, 1).byteLength, s !== b, b.byteLength].join('|')"#, "2|2,3|4|0|true|4"),
    // ---- bounds and odd keys ----
    (r#"var a = new Uint8Array(2); a[5] = 9; a[-1] = 9; a[1.5] = 9; a['x'] = 7; [a[5], a[-1], a[1.5], a.x, a.length, 5 in a, 1 in a, Object.keys(a).join()].join('|')"#, "|||7|2|false|true|0,1,x"),
    (r#"[new Uint8Array(0).fill(1).reverse().sort().join(), new DataView(new ArrayBuffer(0)).byteLength, new Uint8Array(new ArrayBuffer(0), 0, 0).length, new Float64Array(NaN).length, new Uint8Array(4).subarray(NaN, NaN).length, new Uint8Array(2).at(5), new ArrayBuffer(0).slice(5, 9).byteLength].join('|')"#, "|0|0|0|0||0"),
    (r#"var a = new Uint8Array([5, 6]); var k = []; for (var i in a) k.push(i); var d = Object.getOwnPropertyDescriptor(a, '1'); [k.join(), d.value, d.writable, d.enumerable, Object.hasOwn(a, '2'), a.hasOwnProperty(0), Object.getOwnPropertyDescriptor(a, 'length')].join('|')"#, "0,1|6|true|true|false|true|"),
    (r#"var d = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Uint8Array.prototype), 'length'); typeof d.get + '|' + (() => { try { d.get.call([]) } catch (e) { return e.name } })()"#, "function|TypeError"),
    (r#"'use strict'; var a = new Uint8Array(1); a[3] = 1; a.length + ':' + a[3]"#, "1:undefined"),
    // ---- set / fill / copyWithin / sort ----
    (r#"var a = new Uint8Array(6); a.set([1, 2, 3]); a.set(new Uint8Array([9, 8]), 4); a.join()"#, "1,2,3,0,9,8"),
    (r#"var a = new Uint8Array([1, 2, 3, 4]); a.set(a.subarray(0, 3), 1); a.join()"#, "1,1,2,3"),
    (r#"var r = []; try { new Uint8Array(2).set([1, 2, 3]) } catch (e) { r.push(e.name) } try { new Uint8Array(2).set([1], -1) } catch (e) { r.push(e.name) } try { new Uint8Array(2).set([1], 2) } catch (e) { r.push(e.name) } r.join()"#, "RangeError,RangeError,RangeError"),
    (r#"var a = new Uint8Array(5).fill(7, 1, 3); var b = new Uint8Array([1, 2, 3]).reverse(); var c = new Uint8Array([1, 2, 3, 4, 5]).copyWithin(0, 3); var d = new Uint8Array([1, 2, 3, 4, 5]).copyWithin(1, 0, 3); [a.join(), b.join(), c.join(), d.join()].join('|')"#, "0,7,7,0,0|3,2,1|4,5,3,4,5|1,1,2,3,5"),
    (r#"new Float64Array([3, -1, NaN, 10, -0, 0, 2]).sort().join() + '|' + new Uint8Array([10, 9, 1]).sort().join() + '|' + new Int8Array([1, 3, 2]).sort((a, b) => b - a).join()"#, "-1,0,0,2,3,10,NaN|1,9,10|3,2,1"),
    // ---- iteration and the array-like methods ----
    (r#"var a = new Uint16Array([10, 20, 30]); var s = 0; for (var v of a) s += v; [s, [...a].join(), [...a.keys()].join(), [...a.entries()].join(';'), Array.from(a).length, Array.isArray(a)].join('|')"#, "60|10,20,30|0,1,2|0,10;1,20;2,30|3|false"),
    (r#"var a = new Int32Array([1, 2, 3, 4]); [a.map(x => x * 2).join(), a.filter(x => x % 2).join(), a.reduce((s, x) => s + x, 0), a.some(x => x > 3), a.every(x => x > 1), a.find(x => x > 2), a.findIndex(x => x > 2), a.indexOf(3), a.includes(5), a.at(-1), a.lastIndexOf(4)].join('|')"#, "2,4,6,8|1,3|10|true|false|3|2|2|false|4|3"),
    (r#"var a = new Uint8Array([1, 2, 3]); var m = a.map(x => x * 100); (m instanceof Uint8Array) + '|' + m.join() + '|' + (a.filter(x => x > 1) instanceof Uint8Array)"#, "true|100,200,44|true"),
    (r#"[Uint8Array.from([1, 2, 3], x => x * 2).join(), Uint8Array.of(7, 8).join(), Float32Array.from({ length: 2, 0: 1.5 }).join(), Int8Array.from('123').join(), Uint8Array.from(new Uint16Array([257])).join()].join('|')"#, "2,4,6|7,8|1.5,NaN|1,2,3|1"),
    (r#"var a = new Uint8Array([1, 2]); [String(a), Object.prototype.toString.call(a), Object.prototype.toString.call(a.buffer), a.constructor.name, Uint8Array.name, Uint8Array.length, Object.getPrototypeOf(Uint8Array.prototype) === Object.getPrototypeOf(Float64Array.prototype), ArrayBuffer.isView(a), ArrayBuffer.isView(a.buffer)].join('|')"#, "1,2|[object Uint8Array]|[object ArrayBuffer]|Uint8Array|Uint8Array|3|true|true|false"),
    (r#"JSON.stringify(new Uint8Array([1, 2])) + '|' + JSON.stringify({ a: new Float32Array([0.5]) })"#, "{\"0\":1,\"1\":2}|{\"a\":{\"0\":0.5}}"),
    (r#"class Bytes extends Uint8Array { sum() { return this.reduce((a, b) => a + b, 0); } } var b = new Bytes([1, 2, 3]); [b.sum(), b instanceof Bytes, b instanceof Uint8Array, b.subarray(1) instanceof Bytes, Bytes.from([4]) instanceof Bytes].join()"#, "6,true,true,true,true"),
    (r#"var a = new Uint8Array([3, 1, 2]); [Math.max(...a), Array.prototype.slice.call(a, 1).join(), Array.prototype.map.call(a, x => x + 1).join(), [].concat([...a]).length, a.join('-')].join('|')"#, "3|1,2|4,2,3|3|3-1-2"),
    (r#"var a = new Uint8Array(1000); for (var i = 0; i < a.length; i++) a[i] = i; var s = 0; a.forEach(v => { s += v; }); s + '|' + a[999] + '|' + a.subarray(256, 258).join()"#, "124716|231|0,1"),
    // ---- DataView ----
    (r#"var dv = new DataView(new ArrayBuffer(8)); dv.setUint16(0, 0x1234); dv.setUint16(2, 0x1234, true); [dv.getUint8(0), dv.getUint8(1), dv.getUint8(2), dv.getUint8(3), dv.getUint16(0), dv.getUint16(0, true), dv.getUint16(2, true)].join()"#, "18,52,52,18,4660,13330,4660"),
    (r#"var dv = new DataView(new ArrayBuffer(8)); dv.setInt32(0, -2); dv.setFloat32(4, 1.5, true); [dv.getInt32(0), dv.getUint32(0), dv.getInt32(0, true), dv.getFloat32(4, true), dv.getInt8(3), dv.getUint8(3)].join()"#, "-2,4294967294,-16777217,1.5,-2,254"),
    (r#"var dv = new DataView(new ArrayBuffer(8)); dv.setFloat64(0, Math.PI); dv.getFloat64(0) === Math.PI && dv.getFloat64(0, true) !== Math.PI && new Uint8Array(dv.buffer)[0] === 0x40"#, "true"),
    (r#"var dv = new DataView(new ArrayBuffer(4)); var r = []; for (var f of [() => dv.getUint8(4), () => dv.getUint16(3), () => dv.getInt32(1), () => dv.setFloat64(0, 1), () => dv.getUint8(-1), () => dv.getUint8(Infinity), () => new DataView(new ArrayBuffer(2), 3), () => new DataView(new ArrayBuffer(2), 1, 2), () => new DataView({}), () => DataView(new ArrayBuffer(1))]) { try { f(); r.push('ok'); } catch (e) { r.push(e.name[0]); } } r.join('')"#, "RRRRRRRRTT"),
    (r#"var b = new ArrayBuffer(8); var dv = new DataView(b, 2, 4); dv.setUint8(0, 7); dv.setUint8(3, 9); [dv.byteOffset, dv.byteLength, dv.buffer === b, new Uint8Array(b).join(), Object.prototype.toString.call(dv), dv.getUint8()].join('|')"#, "2|4|true|0,0,7,0,0,9,0,0|[object DataView]|7"),
    (r#"var b = new ArrayBuffer(4); new DataView(b).setUint32(0, 0x11223344); new Uint32Array(b)[0].toString(16) + '|' + new DataView(b).getUint32(0, true).toString(16) + '|' + new Uint8Array(b).join()"#, "44332211|44332211|17,34,51,68"),
    (r#"var dv = new DataView(new ArrayBuffer(2)); dv.setInt16(0, 65535); dv.setInt8(1, 128); dv.getInt16(0) + '|' + dv.getUint16(0) + '|' + dv.getInt8(1)"#, "-128|65408|-128"),
    (r#"var dv = new DataView(new ArrayBuffer(4)); dv.setUint8(0, '255'); dv.setUint8(1, 256); dv.setUint8(2, NaN); dv.setUint8(3, -1); new Uint8Array(dv.buffer).join()"#, "255,0,0,255"),
    (r#"var u = new Uint16Array([0x0102]); var dv = new DataView(u.buffer); dv.getUint16(0, true) === 0x0102 && dv.getUint16(0, false) === 0x0201 && dv.getFloat32.length === 1 && dv.setFloat32.length === 2"#, "true"),
];
