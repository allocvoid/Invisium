//! Unicode identifiers: ID_Start / ID_Continue, ZWNJ / ZWJ, escapes, and the errors.

pub const CASES: &[(&str, &str)] = &[
    // identifiers in many scripts
    ("var αβγ = 3; var out = αβγ * 2;", "6"),
    ("var привет = 'hi'; var out = привет;", "hi"),
    ("var 变量 = 5; var out = 变量 + 1;", "6"),
    ("var مرحبا = 7; var out = مرحبا;", "7"),
    ("var नमस्ते = 1; var out = नमस्ते;", "1"),
    ("var ကြ = 2; var out = ကြ;", "2"),
    ("var ภาษาไทย = 4; var out = ภาษาไทย;", "4"),
    ("var cafe\u{301} = 'c'; var out = cafe\u{301};", "c"),
    // object keys and property access
    ("var o = { \"\u{1031}\u{102b}\": \"aw\", \u{1000}\u{103a}: \"et\" }; var out = o.\u{1000}\u{103a} + o[\"\u{1031}\u{102b}\"];", "etaw"),
    ("var o = { မြန်မာ: 'mm' }; var out = o.မြန်မာ;", "mm"),
    ("var o = {}; o.Ωmega = 1; var out = o.Ωmega;", "1"),
    ("var o = { ñandú: 1 }; var out = Object.keys(o)[0] === 'ñandú';", "true"),
    ("var o = { ά: { β: 1 } }; var out = o?.ά?.β;", "1"),
    ("var { 名前: 値 } = { 名前: 'v' }; var out = 値;", "v"),
    ("var 名 = 1; var o = { 名 }; var out = o.名;", "1"),
    // functions, parameters, templates
    ("function 函数() { return 函数.name } var out = 函数();", "函数"),
    ("var out = ((ä, ö) => ä + ö)(1, 2);", "3"),
    ("var ψ = 2; var out = `${ψ}`;", "2"),
    ("eval('var ж = 5'); var out = ж;", "5"),
    ("var out = new Function('ш', 'return ш * 2')(4);", "8"),
    // class members and private names
    ("class K { μέθοδος() { return 'm' } get ήλιος() { return 's' } } var k = new K(); var out = k.μέθοδος() + k.ήλιος;", "ms"),
    ("class K { static 字段 = 4 } var out = K.字段;", "4"),
    ("class K { #секрет = 9; get() { return this.#секрет } } var out = new K().get();", "9"),
    ("class K { #काम() { return 'k' } run() { return this.#काम() } } var out = new K().run();", "k"),
    ("class K { #\u{1000}\u{103a} = 1; has(o) { return #\u{1000}\u{103a} in o } } var out = new K().has(new K());", "true"),
    // labels
    ("var n = 0; 外: for (var i = 0; i < 3; i++) { for (;;) { n++; continue 外; } } var out = n;", "3"),
    ("حلقة: { break حلقة; } var out = 'ok';", "ok"),
    // ID_Continue beyond letters: ZWNJ / ZWJ, digits of other scripts, connectors, Other_ID_*
    ("var a\u{200c}b = 1; var ab = 2; var out = a\u{200c}b;", "1"),
    ("var x\u{200d}y = 5; var out = x\u{200d}y;", "5"),
    ("var x\u{661}\u{662} = 12; var out = x\u{661}\u{662};", "12"),
    ("var क१ = 3; var out = क१;", "3"),
    ("var a\u{203f}b = 4; var out = a\u{203f}b;", "4"),
    ("var l\u{b7}l = 2; var out = l\u{b7}l;", "2"),
    ("var \u{2118} = 1; var out = \u{2118};", "1"),
    ("var $\u{301} = 3; var out = $\u{301};", "3"),
    // escapes
    ("var \\u0061bc = 6; var out = abc;", "6"),
    ("var \\u{1d4d0} = 7; var out = \u{1d4d0};", "7"),
    ("var a\\u0062c = 8; var out = abc;", "8"),
    ("var e\\u0301 = 9; var out = e\u{301};", "9"),
    ("var o = { \\u0078: 1 }; var out = o.x;", "1"),
    ("var o = { \\u0069f: 2 }; var out = o.if;", "2"),
    ("var ñ = 1; var out = \\u00f1 + \\u{f1};", "2"),
    ("var out = /a/gi.flags;", "gi"),
    // errors: illegal first characters
    ("var \u{301}a = 1;", "SyntaxError!"),
    ("var \u{103a} = 1;", "SyntaxError!"),
    ("var \u{93f}x = 1;", "SyntaxError!"),
    ("var \u{661}x = 1;", "SyntaxError!"),
    ("var १क = 1;", "SyntaxError!"),
    ("var \u{200c}a = 1;", "SyntaxError!"),
    ("var \u{24b6} = 1;", "SyntaxError!"),
    // errors: escapes that decode to an illegal character
    ("var a\\u002d = 1;", "SyntaxError!"),
    ("var \\u0031a = 1;", "SyntaxError!"),
    ("var \\u{301} = 1;", "SyntaxError!"),
    ("var a\\u{110000} = 1;", "SyntaxError!"),
    ("var a\\u0020b = 1;", "SyntaxError!"),
    // errors: an escaped reserved word is not a keyword
    ("\\u0069f (true) 1;", "SyntaxError!"),
    ("\\u0076ar x = 1;", "SyntaxError!"),
    ("var x = n\\u0065w Object();", "SyntaxError!"),
    ("var \\u0069f = 1;", "SyntaxError!"),
];
