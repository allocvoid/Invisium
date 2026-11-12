//! `Intl.PluralRules`, `Intl.Collator` and `String.prototype.localeCompare`.

pub const CASES: &[(&str, &str)] = &[
    // --- PluralRules
    ("var p = new Intl.PluralRules('en-US'); [0, 1, 2, 11, 21, 101, 1.5].map(n => p.select(n)).join()", "other,one,other,other,other,other,other"),
    ("var p = new Intl.PluralRules('en-US', { type: 'ordinal' }); [1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101, 111, 112].map(n => p.select(n)).join()", "one,two,few,other,other,other,other,one,two,few,one,other,other"),
    ("var p = new Intl.PluralRules('en', { type: 'ordinal' }); var s = { one: 'st', two: 'nd', few: 'rd', other: 'th' }; [1, 2, 3, 4, 11, 21, 102, 1000].map(n => n + s[p.select(n)]).join()", "1st,2nd,3rd,4th,11th,21st,102nd,1000th"),
    ("[new Intl.PluralRules('en', { minimumFractionDigits: 1 }).select(1), new Intl.PluralRules('en').select(1.0), new Intl.PluralRules('en').select(-1), new Intl.PluralRules('en', { maximumFractionDigits: 0 }).select(1.4), new Intl.PluralRules('en').select('1')].join()", "other,one,one,one,one"),
    ("var p = new Intl.PluralRules(); [p.select(NaN), p.select(Infinity), p.select(-0), p.select(1e21)].join()", "other,other,other,other"),
    ("var r = new Intl.PluralRules('fr', { type: 'ordinal' }).resolvedOptions(); [r.locale, r.type, r.pluralCategories.join(' '), r.minimumIntegerDigits, r.minimumFractionDigits, r.maximumFractionDigits].join()", "en-US,ordinal,one two few other,1,0,3"),
    ("new Intl.PluralRules().resolvedOptions().pluralCategories.join()", "one,other"),
    ("var r = []; try { Intl.PluralRules() } catch (e) { r.push(e.name) } try { new Intl.PluralRules('en', { type: 'dual' }) } catch (e) { r.push(e.name) } try { new Intl.PluralRules().selectRange(NaN, 1) } catch (e) { r.push(e.name) } try { new Intl.PluralRules().selectRange(1) } catch (e) { r.push(e.name) } r.join()", "TypeError,RangeError,RangeError,TypeError"),
    ("var p = new Intl.PluralRules(); [p.selectRange(1, 2), p.selectRange(0, 1), Object.prototype.toString.call(p)].join()", "other,one,[object Intl.PluralRules]"),
    // --- Collator orders
    ("['b', 'a', 'A', 'B', 'c'].sort(new Intl.Collator().compare).join()", "a,A,b,B,c"),
    ("['b', 'a', 'A', 'B', 'c'].sort(new Intl.Collator('en', { caseFirst: 'upper' }).compare).join()", "A,a,B,b,c"),
    ("['résumé', 'resume', 'Resume', 'Résumé'].sort(new Intl.Collator().compare).join()", "resume,Resume,résumé,Résumé"),
    ("['a', 'á', 'A', 'Á'].sort(new Intl.Collator().compare).join()", "a,A,á,Á"),
    ("['Émile', 'Zoe', 'adam', 'éclair', 'Eve'].sort((a, b) => a.localeCompare(b)).join()", "adam,éclair,Émile,Eve,Zoe"),
    ("['b', '1', 'a', ' ', '-', 'Z'].sort(new Intl.Collator().compare).join('|')", " |-|1|a|b|Z"),
    ("[['я', 'b', 'α', 'a'], ['Б', 'б', 'а'], ['ω', 'Α', 'β']].map(l => l.sort(new Intl.Collator().compare).join('')).join('|')", "abαя|абБ|Αβω"),
    ("var c = new Intl.Collator('en', { numeric: true }).compare; [['item10', 'item9', 'item1'].sort(c).join(), ['item10', 'item9', 'item1'].sort(new Intl.Collator().compare).join(), c('007', '7'), c('2', '10')].join('|')", "item1,item9,item10|item1,item10,item9|0|-1"),
    ("var c = new Intl.Collator('en-u-kn'); [c.compare('2', '10'), c.resolvedOptions().numeric, new Intl.Collator('en-u-kf-upper').compare('a', 'A'), new Intl.Collator('en-u-kn', { numeric: false }).compare('2', '10')].join()", "-1,true,1,1"),
    // --- sensitivity
    ("var s = x => new Intl.Collator('en', { sensitivity: x }).compare; ['base', 'accent', 'case', 'variant'].map(x => [s(x)('a', 'A'), s(x)('a', 'á'), s(x)('a', 'b')].join(' ')).join('|')", "0 0 -1|0 -1 -1|-1 0 -1|-1 -1 -1"),
    ("var b = new Intl.Collator('en', { sensitivity: 'base' }).compare; [b('ß', 'ss'), new Intl.Collator().compare('ß', 'ss'), b('Æ', 'ae'), b('ø', 'o'), new Intl.Collator('en', { sensitivity: 'accent' }).compare('ø', 'o')].join()", "0,1,0,0,1"),
    ("var c = new Intl.Collator().compare; [c('é', 'e\\u0301'), c('', 'a'), c('abc', 'abc'), c('a', 'ab'), new Intl.Collator('en', { ignorePunctuation: true }).compare('a-b', 'ab')].join()", "0,-1,0,-1,0"),
    // --- resolvedOptions and the bound compare
    ("var r = new Intl.Collator('sv').resolvedOptions(); [r.locale, r.usage, r.sensitivity, r.ignorePunctuation, r.collation, r.numeric, r.caseFirst].join()", "en-US,sort,variant,false,default,false,false"),
    ("var r = new Intl.Collator('en', { usage: 'search', sensitivity: 'base', caseFirst: 'lower' }).resolvedOptions(); [r.usage, r.sensitivity, r.caseFirst].join()", "search,base,lower"),
    ("var x = new Intl.Collator(); var c = x.compare; [c('a', 'b'), c === x.compare, c.length, Intl.Collator('en').compare('b', 'a'), Object.prototype.toString.call(x)].join()", "-1,true,2,1,[object Intl.Collator]"),
    ("var r = []; for (var o of [{ sensitivity: 'loose' }, { caseFirst: 'up' }, { usage: 'find' }]) { try { new Intl.Collator('en', o) } catch (e) { r.push(e.name) } } r.join()", "RangeError,RangeError,RangeError"),
    // --- localeCompare
    ("['a'.localeCompare('B'), 'a'.localeCompare('A'), 'résumé'.localeCompare('RESUME', 'en', { sensitivity: 'base' }), 'ä'.localeCompare('z', 'de'), 'b'.localeCompare('a'), 'a'.localeCompare('a')].join()", "-1,-1,0,-1,1,0"),
    ("['10', '9', '1'].sort((a, b) => a.localeCompare(b, undefined, { numeric: true })).join() + '|' + 'x'.localeCompare() + '|' + 'undefined'.localeCompare()", "1,9,10|1|0"),
    ("var r = []; try { String.prototype.localeCompare.call(null, 'a') } catch (e) { r.push(e.name) } try { 'a'.localeCompare('b', 'x!') } catch (e) { r.push(e.name) } r.join()", "TypeError,RangeError"),
];
