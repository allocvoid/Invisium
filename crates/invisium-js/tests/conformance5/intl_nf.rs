//! `Intl.NumberFormat` and the Number / BigInt `toLocaleString` methods routed through it.

macro_rules! nf {
    ($s:expr) => {
        concat!("var N = (o, x) => new Intl.NumberFormat('en-US', o).format(x); ", $s)
    };
}

pub const CASES: &[(&str, &str)] = &[
    // --- decimal
    (nf!("[N({}, 1234567.891), N({}, 0.1 + 0.2), N({}, -1234.5), N({}, 1e21)].join('|')"), "1,234,567.891|0.3|-1,234.5|1,000,000,000,000,000,000,000"),
    (nf!("[N({ maximumFractionDigits: 2 }, 1.005), N({}, -0), N({}, NaN), N({}, Infinity), N({}, -Infinity)].join('|')"), "1.01|-0|NaN|∞|-∞"),
    (nf!("[N({ minimumFractionDigits: 2 }, 5), N({ maximumFractionDigits: 0 }, 2.5), N({ maximumFractionDigits: 0 }, -2.5), N({ minimumIntegerDigits: 3 }, 5), N({ minimumIntegerDigits: 3 }, 1234)].join('|')"), "5.00|3|-3|005|1,234"),
    (nf!("[N({}, 1e-7), N({ maximumSignificantDigits: 3 }, 1e-7), N({ maximumFractionDigits: 100 }, 0.1), N({}, '1234.5')].join('|')"), "0|0.0000001|0.1|1,234.5"),
    (nf!("[N({ maximumSignificantDigits: 3 }, 123456), N({ minimumSignificantDigits: 5 }, 1.5), N({ maximumSignificantDigits: 2 }, 0.012345), N({ minimumSignificantDigits: 3 }, 0)].join('|')"), "123,000|1.5000|0.012|0.00"),
    (nf!("[N({ maximumFractionDigits: 2, maximumSignificantDigits: 2, roundingPriority: 'lessPrecision' }, 1.234), N({ maximumFractionDigits: 2, maximumSignificantDigits: 2, roundingPriority: 'morePrecision' }, 1.234)].join('|')"), "1.2|1.23"),
    // --- rounding modes at half
    (nf!("var r = []; for (var m of ['ceil', 'floor', 'expand', 'trunc', 'halfCeil', 'halfFloor', 'halfExpand', 'halfTrunc', 'halfEven']) r.push(m + ':' + [2.5, -2.5, 3.5].map(x => N({ maximumFractionDigits: 0, roundingMode: m }, x)).join()); r.join(' ')"),
     "ceil:3,-2,4 floor:2,-3,3 expand:3,-3,4 trunc:2,-2,3 halfCeil:3,-2,4 halfFloor:2,-3,3 halfExpand:3,-3,4 halfTrunc:2,-2,3 halfEven:2,-2,4"),
    (nf!("[N({ maximumFractionDigits: 1, roundingMode: 'halfEven' }, 0.25), N({ maximumFractionDigits: 1, roundingMode: 'halfEven' }, 0.35), N({ maximumFractionDigits: 2, roundingMode: 'floor' }, -1.001), N({ maximumFractionDigits: 2, roundingMode: 'trunc' }, 9.999)].join('|')"), "0.2|0.4|-1.01|9.99"),
    (nf!("[N({ minimumFractionDigits: 2, maximumFractionDigits: 2, roundingIncrement: 5 }, 1.23), N({ minimumFractionDigits: 2, maximumFractionDigits: 2, roundingIncrement: 25 }, 1.13)].join('|')"), "1.25|1.25"),
    // --- grouping and sign display
    (nf!("[N({ useGrouping: false }, 1234567), N({ useGrouping: 'min2' }, 1234), N({ useGrouping: 'min2' }, 12345), N({ useGrouping: 'always' }, 1234), N({ useGrouping: true }, 1234)].join('|')"), "1234567|1234|12,345|1,234|1,234"),
    (nf!("var r = []; for (var s of ['auto', 'always', 'exceptZero', 'never', 'negative']) r.push(s + ':' + [1, -1, 0, -0].map(x => N({ signDisplay: s }, x)).join()); r.join(' ')"), "auto:1,-1,0,-0 always:+1,-1,+0,-0 exceptZero:+1,-1,0,0 never:1,1,0,0 negative:1,-1,0,0"),
    (nf!("[N({ signDisplay: 'exceptZero', maximumFractionDigits: 1 }, 0.01), N({ signDisplay: 'always', style: 'percent' }, 0.5)].join('|')"), "0|+50%"),
    // --- percent
    (nf!("[N({ style: 'percent' }, 0.256), N({ style: 'percent', maximumFractionDigits: 1 }, 0.256), N({ style: 'percent' }, -0.5), N({ style: 'percent' }, 12.345), N({ style: 'percent' }, 0.0001)].join('|')"), "26%|25.6%|-50%|1,235%|0%"),
    // --- currency
    (nf!("var C = (c, x, o) => N(Object.assign({ style: 'currency', currency: c }, o), x); [C('USD', 1234.5), C('USD', -5), C('EUR', 1234.5), C('JPY', 1234.5), C('GBP', 0.5), C('USD', 0.005)].join('|')"), "$1,234.50|-$5.00|€1,234.50|¥1,235|£0.50|$0.01"),
    (nf!("var C = (c, x, o) => N(Object.assign({ style: 'currency', currency: c }, o), x); [C('USD', 1234.5, { currencyDisplay: 'code' }), C('CHF', 5), C('XYZ', 5), C('CAD', 5), C('CAD', 5, { currencyDisplay: 'narrowSymbol' })].join('|')"), "USD\u{a0}1,234.50|CHF\u{a0}5.00|XYZ\u{a0}5.00|CA$5.00|$5.00"),
    (nf!("var C = (c, x, o) => N(Object.assign({ style: 'currency', currency: c }, o), x); [C('USD', 1234.5, { currencyDisplay: 'name' }), C('USD', 1, { currencyDisplay: 'name', maximumFractionDigits: 0 }), C('USD', 1, { currencyDisplay: 'name' }), C('EUR', 2, { currencyDisplay: 'name' })].join('|')"), "1,234.50 US dollars|1 US dollar|1.00 US dollars|2.00 euros"),
    (nf!("var C = (c, x, o) => N(Object.assign({ style: 'currency', currency: c }, o), x); [C('USD', -5, { currencySign: 'accounting' }), C('USD', 5, { currencySign: 'accounting' }), C('usd', 5, { trailingZeroDisplay: 'stripIfInteger' }), C('USD', 5.1, { trailingZeroDisplay: 'stripIfInteger' })].join('|')"), "($5.00)|$5.00|$5|$5.10"),
    (nf!("[N({ style: 'currency', currency: 'USD', notation: 'compact' }, 1234567), N({ style: 'currency', currency: 'KRW' }, 50000), N({ style: 'currency', currency: 'INR' }, 100)].join('|')"), "$1.2M|₩50,000|₹100.00"),
    // --- units
    (nf!("var U = (u, x, d) => N({ style: 'unit', unit: u, unitDisplay: d }, x); [U('kilometer-per-hour', 50), U('kilometer-per-hour', 50, 'long'), U('liter', 1, 'long'), U('kilobyte', 16), U('celsius', 20), U('percent', 5)].join('|')"), "50 km/h|50 kilometers per hour|1 liter|16 kB|20°C|5%"),
    (nf!("var U = (u, x, d) => N({ style: 'unit', unit: u, unitDisplay: d }, x); [U('day', 3, 'long'), U('day', 1), U('day', 2), U('megabyte-per-second', 5), U('kilometer', 5, 'narrow'), U('foot', 1, 'long'), U('foot', 2, 'long'), U('hour', 2)].join('|')"), "3 days|1 day|2 days|5 MB/s|5km|1 foot|2 feet|2 hr"),
    // --- compact, scientific, engineering
    (nf!("[1234, 12345, 123456, 1234567, 999999, 1e12, 1.5e15, 999, 0.123, -1234].map(x => N({ notation: 'compact' }, x)).join('|')"), "1.2K|12K|123K|1.2M|1M|1T|1500T|999|0.12|-1.2K"),
    (nf!("[1234, 2.5e9, 1e6, 15].map(x => N({ notation: 'compact', compactDisplay: 'long' }, x)).join('|')"), "1.2 thousand|2.5 billion|1 million|15"),
    (nf!("[N({ notation: 'compact', maximumFractionDigits: 2 }, 1234), N({ notation: 'compact', maximumSignificantDigits: 4 }, 123456)].join('|')"), "1.23K|123.5K"),
    (nf!("[N({ notation: 'scientific' }, 1234.5), N({ notation: 'scientific' }, 0.00012), N({ notation: 'scientific' }, -0.00012), N({ notation: 'scientific' }, 0), N({ notation: 'engineering' }, 12345), N({ notation: 'scientific', maximumFractionDigits: 1 }, 9.96)].join('|')"), "1.235E3|1.2E-4|-1.2E-4|0E0|12.345E3|1E1"),
    // --- formatToParts
    (nf!("new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' }).formatToParts(-1234.5).map(p => p.type).join()"), "minusSign,currency,integer,group,integer,decimal,fraction"),
    (nf!("[new Intl.NumberFormat('en', { style: 'percent' }).formatToParts(0.5).map(p => p.type + '=' + p.value).join(), new Intl.NumberFormat('en', { notation: 'compact' }).formatToParts(1234).map(p => p.type).join()].join('|')"), "integer=50,percentSign=%|integer,decimal,fraction,compact"),
    (nf!("[new Intl.NumberFormat('en', { notation: 'scientific' }).formatToParts(0.001).map(p => p.type).join(), new Intl.NumberFormat('en', { style: 'unit', unit: 'meter' }).formatToParts(3).map(p => p.type + '=' + p.value).join()].join('|')"), "integer,exponentSeparator,exponentMinusSign,exponentInteger|integer=3,literal= ,unit=m"),
    (nf!("new Intl.NumberFormat().formatToParts(NaN).map(p => p.type).join() + '|' + new Intl.NumberFormat().formatToParts(-Infinity).map(p => p.type).join()"), "nan|minusSign,infinity"),
    // --- resolvedOptions
    (nf!("var r = new Intl.NumberFormat('de-DE').resolvedOptions(); [r.locale, r.numberingSystem, r.style, r.minimumIntegerDigits, r.minimumFractionDigits, r.maximumFractionDigits, r.useGrouping, r.notation, r.signDisplay, r.roundingMode, r.roundingPriority, 'minimumSignificantDigits' in r].join()"), "en-US,latn,decimal,1,0,3,auto,standard,auto,halfExpand,auto,false"),
    (nf!("var r = new Intl.NumberFormat('en', { style: 'currency', currency: 'jpy' }).resolvedOptions(); [r.currency, r.currencyDisplay, r.currencySign, r.minimumFractionDigits, r.maximumFractionDigits].join()"), "JPY,symbol,standard,0,0"),
    (nf!("var r = new Intl.NumberFormat('en', { notation: 'compact' }).resolvedOptions(); [r.useGrouping, r.compactDisplay, r.roundingPriority, r.minimumSignificantDigits, r.maximumSignificantDigits, r.maximumFractionDigits].join()"), "min2,short,morePrecision,1,2,0"),
    (nf!("var r = new Intl.NumberFormat('en', { maximumSignificantDigits: 3, useGrouping: false }).resolvedOptions(); [r.minimumSignificantDigits, r.maximumSignificantDigits, 'maximumFractionDigits' in r, r.useGrouping].join()"), "1,3,false,false"),
    // --- errors and absurd options
    (nf!("var r = []; for (var o of [{ style: 'currency' }, { currency: 'US' }, { style: 'unit' }, { style: 'unit', unit: 'parsec' }, { maximumFractionDigits: 101 }, { minimumFractionDigits: 3, maximumFractionDigits: 1 }, { maximumSignificantDigits: 0 }, { roundingIncrement: 3 }, { minimumIntegerDigits: Infinity }, { maximumFractionDigits: NaN }, { style: 'bogus' }, { maximumSignificantDigits: 1e300 }]) { try { new Intl.NumberFormat('en', o); r.push('ok') } catch (e) { r.push(e.name) } } r.join()"),
     "TypeError,RangeError,TypeError,RangeError,RangeError,RangeError,RangeError,RangeError,RangeError,RangeError,RangeError,RangeError"),
    (nf!("var r = []; try { new Intl.NumberFormat('en', { roundingIncrement: 5 }) } catch (e) { r.push(e.name) } try { new Intl.NumberFormat('en', { roundingIncrement: 5, maximumSignificantDigits: 2, minimumFractionDigits: 2, maximumFractionDigits: 2 }) } catch (e) { r.push(e.name) } r.join()"), "TypeError"),
    // --- BigInt and toLocaleString routing
    (nf!("[N({}, 12345678901234567890123n), (123n).toLocaleString('en-US', { style: 'currency', currency: 'USD' }), N({ notation: 'compact' }, 10n ** 15n), (-5n).toLocaleString(), N({ notation: 'scientific' }, 123456789012345678901234567890n)].join('|')"), "12,345,678,901,234,567,890,123|$123.00|1000T|-5|1.235E29"),
    (nf!("[(0.5).toLocaleString('de-DE', { style: 'percent' }), (1234.5).toLocaleString(undefined, { minimumFractionDigits: 2 }), (1234.5).toLocaleString(), Number.prototype.toLocaleString.call(7)].join('|')"), "50%|1,234.50|1,234.5|7"),
    (nf!("var x = new Intl.NumberFormat(); [1, 2000].map(x.format).join('|') + '|' + (x.format === x.format) + '|' + Object.prototype.toString.call(x)"), "1|2,000|true|[object Intl.NumberFormat]"),
    (nf!("Intl.NumberFormat.supportedLocalesOf('en-US').join() + '|' + Intl.NumberFormat('en').format(1e3) + '|' + Intl.NumberFormat.length"), "en-US|1,000|0"),
];
