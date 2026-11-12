//! Item 3, first half: BigInt literals, the `BigInt` function, arithmetic and bitwise
//! operators against known values.

pub const CASES: &[(&str, &str)] = &[
    // --- literals and typeof
    (r#"[typeof 1n, 123n, 0xffn, 0XFFn, 0b101n, 0o17n, 1_000n, 0n, -0n, 0x0n].join()"#, "bigint,123,255,255,5,15,1000,0,0,0"),
    (r#"typeof 1n === 'bigint' && typeof BigInt(1) === 'bigint' && typeof Object(1n) === 'object' && typeof BigInt === 'function'"#, "true"),
    (r#"var r = []; for (var src of ['1.5n', '1e3n', '01n', '08n', '1n2', '0x1.0n', '1_n', '.5n']) { try { eval(src); r.push('ok') } catch (e) { r.push(e.name) } } r.join()"#, "SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError"),
    (r#"[1_000_000n === 1000000n, 0xFF_FFn, 12345678901234567890123456789012345678901234567890n].join()"#, "true,65535,12345678901234567890123456789012345678901234567890"),
    (r#"function f() { return 10n } f() === f() && f() === 10n && 0xan === 10n && 0b1010n === 10n"#, "true"),
    // --- BigInt(): numbers, strings, booleans
    (r#"[BigInt(5), BigInt(-5), BigInt(0), BigInt(-0), BigInt(2 ** 53), BigInt(1e21), BigInt(Number.MAX_SAFE_INTEGER) + 2n, BigInt(1.7976931348623157e308) > 10n ** 308n].join()"#, "5,-5,0,0,9007199254740992,1000000000000000000000,9007199254740993,true"),
    (r#"[BigInt('42'), BigInt('  42  '), BigInt('-7'), BigInt('+7'), BigInt(''), BigInt('   '), BigInt('0x10'), BigInt('0b11'), BigInt('0o17'), BigInt('-0'), BigInt('00012')].join()"#, "42,42,-7,7,0,0,16,3,15,0,12"),
    (r#"[BigInt(true), BigInt(false), BigInt(10n), BigInt(Object(3n)), BigInt({ valueOf() { return 8 } }), BigInt([9]), BigInt('123456789012345678901234567890')].join()"#, "1,0,10,3,8,9,123456789012345678901234567890"),
    (r#"var r = []; for (var f of [() => BigInt(1.5), () => BigInt(NaN), () => BigInt(Infinity), () => BigInt('1.5'), () => BigInt('1e3'), () => BigInt('0x'), () => BigInt('-0x10'), () => BigInt('12n'), () => BigInt('abc')]) { try { f(); r.push('ok') } catch (e) { r.push(e.name) } } r.join()"#, "RangeError,RangeError,RangeError,SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError"),
    (r#"var r = []; for (var f of [() => new BigInt(1), () => BigInt(undefined), () => BigInt(null), () => BigInt(Symbol()), () => BigInt()]) { try { f(); r.push('ok') } catch (e) { r.push(e.name) } } r.join() + '|' + BigInt.length + '|' + BigInt.name"#, "TypeError,TypeError,TypeError,TypeError,TypeError|1|BigInt"),
    // --- arithmetic against known values
    (r#"var f = 1n; for (var i = 1n; i <= 30n; i++) f *= i; f.toString()"#, "265252859812191058636308480000000"),
    (r#"[2n ** 64n, 2n ** 100n, (-2n) ** 63n, (-2n) ** 64n, 10n ** 30n, 0n ** 0n, 7n ** 0n, 1n ** 1000000n, (-1n) ** 1000001n].join()"#, "18446744073709551616,1267650600228229401496703205376,-9223372036854775808,18446744073709551616,1000000000000000000000000000000,1,1,1,-1"),
    (r#"2n ** 200n"#, "1606938044258990275541962092341162602522202993782792835301376"),
    (r#"3n ** 100n"#, "515377520732011331036461129765621272702107522001"),
    (r#"[7n / 2n, -7n / 2n, 7n / -2n, -7n / -2n, 7n % 3n, -7n % 3n, 7n % -3n, -7n % -3n, 0n / 5n, 5n % 5n].join()"#, "3,-3,-3,3,1,-1,1,-1,0,0"),
    (r#"[2n ** 64n / 3n, (2n ** 64n) % 1000000007n, 10n ** 40n / 10n ** 20n, (10n ** 40n + 7n) % 10n ** 20n, 123456789012345678901234567890n / 987654321n, 123456789012345678901234567890n % 987654321n].join()"#, "6148914691236517205,582344008,100000000000000000000,7,124999998873437499901,574845669"),
    (r#"var a = 2n ** 127n - 1n, b = 2n ** 61n - 1n; var q = a / b, r = a % b; [q, r, q * b + r === a, (-a) / b === -q, (-a) % b === -r].join()"#, "73786976294838206496,31,true,true,true"),
    (r#"[1n + 2n, 5n - 10n, 3n * -4n, -(5n), -(-5n), 18446744073709551615n + 1n, 4294967296n * 4294967296n, 1n - 18446744073709551616n, 0n * -5n].join()"#, "3,-5,-12,-5,5,18446744073709551616,18446744073709551616,-18446744073709551615,0"),
    (r#"var x = 5n; x++; x += 10n; x **= 2n; var y = x--; x -= 0n; x *= 1n; x /= 1n; x %= 1000n; [x, y, --x, x++ + 1n, typeof x].join()"#, "255,256,254,255,bigint"),
    (r#"var fib = [0n, 1n]; for (var i = 2; i <= 200; i++) fib.push(fib[i - 1] + fib[i - 2]); fib[200].toString()"#, "280571172992510140037611932413038677189525"),
    (r#"function gcd(a, b) { while (b) { [a, b] = [b, a % b] } return a } [gcd(2n ** 64n, 2n ** 40n) === 2n ** 40n, gcd(1071n, 462n), gcd(17n ** 20n, 17n ** 15n) === 17n ** 15n].join()"#, "true,21,true"),
    // --- shifts and bitwise operators (two's complement)
    (r#"[1n << 70n, 1n << 0n, 5n << 2n, -5n << 2n, 5n >> 1n, -5n >> 1n, -1n >> 100n, 1n >> 100n, -8n >> 2n, -9n >> 2n, 1n << -1n, 4n >> -2n, (2n ** 100n) >> 98n].join()"#, "1180591620717411303424,1,20,-20,2,-3,-1,0,-2,-3,0,16,4"),
    (r#"[255n & 15n, -1n & 255n, -256n & 255n, 5n | 2n, -5n | 2n, 5n ^ 1n, 5n ^ -1n, -5n ^ -3n, ~5n, ~-1n, ~0n, ~(2n ** 64n)].join()"#, "15,255,0,7,-5,4,-6,6,-6,0,-1,-18446744073709551617"),
    (r#"[(2n ** 64n - 1n) & (2n ** 32n - 1n), (2n ** 64n) | 1n, (2n ** 64n) ^ (2n ** 64n), -(2n ** 64n) & (2n ** 65n - 1n), -(2n ** 64n) | 5n, (-1n) ^ (2n ** 70n)].join()"#, "4294967295,18446744073709551617,0,18446744073709551616,-18446744073709551611,-1180591620717411303425"),
    (r#"var x = 0xf0n; x &= 0x3cn; var a = x; x |= 1n; x ^= 0xffn; x <<= 4n; x >>= 2n; [a, x].join()"#, "48,824"),
    // --- asIntN / asUintN wrap-around
    (r#"[BigInt.asIntN(8, 255n), BigInt.asIntN(8, 128n), BigInt.asIntN(8, 127n), BigInt.asIntN(8, -129n), BigInt.asIntN(8, 256n), BigInt.asIntN(1, 1n), BigInt.asIntN(0, 5n), BigInt.asIntN(64, 2n ** 63n), BigInt.asIntN(64, 2n ** 64n + 5n)].join()"#, "-1,-128,127,127,0,-1,0,-9223372036854775808,5"),
    (r#"[BigInt.asUintN(8, -1n), BigInt.asUintN(8, 256n), BigInt.asUintN(8, 257n), BigInt.asUintN(64, -1n), BigInt.asUintN(3, 13n), BigInt.asUintN(0, -7n), BigInt.asUintN(100, -1n) === 2n ** 100n - 1n, BigInt.asUintN(32, -(2n ** 32n))].join()"#, "255,0,1,18446744073709551615,5,0,true,0"),
    (r#"[BigInt.asIntN(32, BigInt(0x80000000)), BigInt.asIntN(200, 5n), BigInt.asIntN(200, -5n), BigInt.asUintN(200, 5n), BigInt.asIntN('8', '255'), BigInt.asIntN.length].join()"#, "-2147483648,5,-5,5,-1,2"),
    (r#"var r = []; for (var f of [() => BigInt.asIntN(-1, 1n), () => BigInt.asIntN(8, 1), () => BigInt.asUintN(2 ** 60, -1n), () => BigInt.asUintN(8)]) { try { f(); r.push('ok') } catch (e) { r.push(e.name) } } r.join() + '|' + BigInt.asIntN(2 ** 40, -1n) + '|' + BigInt.asUintN(2 ** 40, 7n)"#, "RangeError,TypeError,RangeError,TypeError|-1|7"),
    // --- mixing with Numbers is a TypeError
    (r#"var r = []; for (var f of [() => 1n + 1, () => 1 + 1n, () => 1n * 2, () => 2 ** 2n, () => 1n - 1.5, () => 5n / 2, () => 5 % 2n, () => 1n << 1, () => 1 & 1n, () => 1n | 0, () => 1n ^ 1]) { try { f(); r.push('ok') } catch (e) { r.push(e.name + ':' + e.message) } } [...new Set(r)].join('|')"#, "TypeError:Cannot mix BigInt and other types, use explicit conversions"),
    (r#"var r = []; for (var f of [() => +1n, () => 1n >>> 0n, () => Math.max(1n), () => Math.floor(1n), () => isNaN(1n), () => 1n / 0n, () => 1n % 0n, () => 2n ** -1n, () => new Date(1n), () => [].at(1n), () => 'x'.repeat(2n)]) { try { f(); r.push('ok') } catch (e) { r.push(e.name) } } r.join()"#, "TypeError,TypeError,TypeError,TypeError,TypeError,RangeError,RangeError,RangeError,TypeError,TypeError,TypeError"),
    (r#"var x = 1n; var r = []; try { x += 1 } catch (e) { r.push(e.name) } try { x++; r.push(x) } catch (e) { r.push(e.name) } var n = 1; try { n += 1n } catch (e) { r.push(e.name) } r.join() + '|' + (1n + '1') + '|' + ('1' + 1n) + '|' + (1n + [2]) + '|' + typeof (1n + '')"#, "TypeError,2,TypeError|11|11|12|string"),
    (r#"var o = { valueOf() { return 2n } }; [o * 3n, o + 1n, 10n - o, -o, o ** o, Object(5n) * 2n, Object(5n) + 1n].join()"#, "6,3,8,-2,4,10,6"),
];
