//! `Intl.RelativeTimeFormat` and `Intl.ListFormat`.

macro_rules! rl {
    ($s:expr) => {
        concat!(
            "var R = (v, u, o) => new Intl.RelativeTimeFormat('en', o).format(v, u); ",
            "var L = (l, o) => new Intl.ListFormat('en', o).format(l); ",
            $s
        )
    };
}

pub const CASES: &[(&str, &str)] = &[
    // --- RelativeTimeFormat
    (rl!("[R(3, 'day'), R(-3, 'day'), R(1, 'day'), R(-1, 'days'), R(0, 'day'), R(-0, 'day')].join('|')"), "in 3 days|3 days ago|in 1 day|1 day ago|in 0 days|0 days ago"),
    (rl!("[-1, 0, 1, 2, -2].map(v => R(v, 'day', { numeric: 'auto' })).join('|')"), "yesterday|today|tomorrow|in 2 days|2 days ago"),
    (rl!("var A = { numeric: 'auto' }; [R(-1, 'year', A), R(0, 'month', A), R(1, 'week', A), R(1, 'quarter', A), R(0, 'second', A), R(0, 'minute', A), R(0, 'hour', A), R(-1, 'hour', A)].join('|')"), "last year|this month|next week|next quarter|now|this minute|this hour|1 hour ago"),
    (rl!("var S = { style: 'short' }; [R(3, 'hour', S), R(-5, 'minute', S), R(2, 'month', S), R(1, 'quarter', S), R(2, 'quarters', S), R(10, 'second', S), R(-1, 'year', { style: 'short', numeric: 'auto' }), R(3, 'week', { style: 'narrow' })].join('|')"), "in 3 hr.|5 min. ago|in 2 mo.|in 1 qtr.|in 2 qtrs.|in 10 sec.|last yr.|in 3 wk."),
    (rl!("[R(1000.5, 'day'), R(1.5, 'hours'), R(-2, 'years'), R(12, 'months'), R('4', 'seconds')].join('|')"), "in 1,000.5 days|in 1.5 hours|2 years ago|in 12 months|in 4 seconds"),
    (rl!("new Intl.RelativeTimeFormat('en').formatToParts(-1000, 'day').map(p => p.type + (p.unit ? '/' + p.unit : '') + '=' + p.value).join()"), "integer/day=1,group/day=,,integer/day=000,literal= days ago"),
    (rl!("new Intl.RelativeTimeFormat('en', { numeric: 'auto' }).formatToParts(-1, 'day').map(p => p.type + '=' + p.value).join() + '|' + new Intl.RelativeTimeFormat('en').formatToParts(2, 'week').map(p => p.type).join()"), "literal=yesterday|literal,integer,literal"),
    (rl!("var r = []; for (var f of [() => R(1, 'decade'), () => R(NaN, 'day'), () => R(Infinity, 'day'), () => Intl.RelativeTimeFormat('en'), () => R(1, 'day', { style: 'tiny' }), () => R(1, 'day', { numeric: 'sometimes' })]) { try { f(); r.push('ok') } catch (e) { r.push(e.name) } } r.join()"), "RangeError,RangeError,RangeError,TypeError,RangeError,RangeError"),
    (rl!("var r = new Intl.RelativeTimeFormat('de', { style: 'short', numeric: 'auto' }).resolvedOptions(); [r.locale, r.style, r.numeric, r.numberingSystem].join()"), "en-US,short,auto,latn"),
    // --- ListFormat
    (rl!("[[], ['A'], ['A', 'B'], ['A', 'B', 'C'], ['A', 'B', 'C', 'D']].map(l => L(l)).join('|')"), "|A|A and B|A, B, and C|A, B, C, and D"),
    (rl!("[['A'], ['A', 'B'], ['A', 'B', 'C'], ['A', 'B', 'C', 'D']].map(l => L(l, { type: 'disjunction' })).join('|')"), "A|A or B|A, B, or C|A, B, C, or D"),
    (rl!("[L(['A', 'B'], { style: 'short' }), L(['A', 'B', 'C'], { style: 'short' }), L(['A', 'B'], { style: 'narrow' }), L(['A', 'B', 'C'], { style: 'narrow' })].join('|')"), "A & B|A, B, & C|A, B|A, B, C"),
    (rl!("[L(['A', 'B'], { type: 'unit' }), L(['A', 'B', 'C'], { type: 'unit', style: 'short' }), L(['A', 'B'], { type: 'unit', style: 'narrow' }), L(['A', 'B', 'C'], { type: 'unit', style: 'narrow' })].join('|')"), "A, B|A, B, C|A B|A B C"),
    (rl!("new Intl.ListFormat('en').formatToParts(['a', 'b']).map(p => p.type + '=' + p.value).join() + '|' + new Intl.ListFormat('en', { type: 'disjunction' }).formatToParts(['x', 'y', 'z']).map(p => p.type).join()"), "element=a,literal= and ,element=b|element,literal,element,literal,element"),
    (rl!("[L(new Set(['x', 'y'])), L('ab'), new Intl.ListFormat().format(), L((function* () { yield 'g1'; yield 'g2'; yield 'g3' })())].join('|')"), "x and y|a and b||g1, g2, and g3"),
    (rl!("var r = []; for (var f of [() => L([1, 2]), () => L(['a'], { type: 'bogus' }), () => Intl.ListFormat(), () => L(5)]) { try { f(); r.push('ok') } catch (e) { r.push(e.name) } } r.join()"), "TypeError,RangeError,TypeError,TypeError"),
    (rl!("var r = new Intl.ListFormat('fr', { type: 'disjunction', style: 'short' }).resolvedOptions(); [r.locale, r.type, r.style, Object.prototype.toString.call(new Intl.ListFormat())].join()"), "en-US,disjunction,short,[object Intl.ListFormat]"),
];
