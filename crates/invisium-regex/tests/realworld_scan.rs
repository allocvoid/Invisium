//! Acceptance criterion A4 (part 2): real-world patterns used the way scripts use them —
//! global scans (`match`/`replace` with `g`), sticky tokenizer loops and key/value parsing.

mod common;

use common::{find_all, regex, u16s};
use invisium_regex::Regex;

/// `(pattern, flags, subject, every match of a global scan)`
type Scan = (&'static str, &'static str, &'static str, &'static [&'static str]);

const SCANS: &[Scan] = &[
    // String.prototype.trim by hand.
    (r"^\s+|\s+$", "", "  hi there \n", &["  ", " \n"]),
    // camelCase splitting, by capture and by lookaround.
    ("([a-z])([A-Z])", "", "camelCaseString", &["lC", "eS"]),
    ("(?<=[a-z])(?=[A-Z])", "", "camelCaseString", &["", ""]),
    // Template placeholders.
    (r"\{\{\s*([\w.]+)\s*\}\}", "", "Hello {{ user.name }}, you have {{count}} new {{ items }}", &["{{ user.name }}", "{{count}}", "{{ items }}"]),
    (r"\s{2,}", "", "a  b \t c    d", &["  ", " \t ", "    "]),
    (r"\r\n|\r|\n", "", "a\r\nb\nc\rd", &["\r\n", "\n", "\r"]),
    (r"/\*[\s\S]*?\*/", "", "a /* x */ b /* y\n z */ c", &["/* x */", "/* y\n z */"]),
    (r"&(?:#(\d+)|#x([0-9a-f]+)|([a-z]+));", "i", "x &lt; y &#38; z &#x1F600;", &["&lt;", "&#38;", "&#x1F600;"]),
    (r#"(?:^|,)("(?:[^"]|"")*"|[^,]*)"#, "", r#"a,"b,c",,d"#, &["a", ",\"b,c\"", ",", ",d"]),
    (r"(?<![\w/])#(\w+)", "", "fix #123 and #abc, not a/#b or x#y", &["#123", "#abc"]),
    (r"(\d{4})(?=\d)", "", "4111111111111111", &["4111", "1111", "1111"]),
    (r"<\/?[^>]+>", "", "<p>Hello <b>world</b></p>", &["<p>", "<b>", "</b>", "</p>"]),
    ("[^a-z0-9]+", "i", "Hello, World! 2024", &[", ", "! "]),
    (".", "su", "a😀\n", &["a", "😀", "\n"]),
    (r"\\u([0-9a-fA-F]{4})", "", r"\u0041\u00e9", &[r"\u0041", r"\u00e9"]),
    (r"(\d+)|(\D+)", "", "1.20.3-rc1", &["1", ".", "20", ".", "3", "-rc", "1"]),
    (r"\S+", "", " btn  btn-primary\tactive ", &["btn", "btn-primary", "active"]),
    (r"^\s*([\w.-]+)\s*[:=]\s*(.*?)\s*$", "m", "name = Invisium  \n# c\nport: 8080", &["name = Invisium  ", "port: 8080"]),
    (r"\b[A-Z][a-z]+\b", "", "Alice met Bob in Paris, not paris", &["Alice", "Bob", "Paris"]),
    (r"\d+(?:\.\d+)?(?=\s*(?:px|em))", "", "margin: 4px 1.5em 3 10 px;", &["4", "1.5", "10"]),
];

#[test]
fn global_scans() {
    let mut failures = Vec::new();
    for &(pattern, flags, subject, expected) in SCANS {
        let got = find_all(&regex(pattern, flags), subject);
        if got != expected {
            failures.push(format!("  /{pattern}/{flags} on {subject:?}\n    expected {expected:?}\n    got      {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} scans failed:\n{}", failures.len(), SCANS.len(), failures.join("\n"));
}

/// Text of group `g` of a match, or `None` if the group is unset.
fn group(text: &[u16], caps: &invisium_regex::Captures, g: usize) -> Option<String> {
    caps.get(g).map(|(s, e)| String::from_utf16_lossy(&text[s..e]))
}

/// Run `re` repeatedly from the end of the previous match and hand every match to `each`.
fn each_match(re: &Regex, subject: &str, mut each: impl FnMut(&[u16], &invisium_regex::Captures)) -> usize {
    let text = u16s(subject);
    let mut at = 0;
    let mut steps = 1_000_000u64;
    while let Some(caps) = re.exec(&text, at, &mut steps).expect("exec failed") {
        let (s, e) = caps.get(0).expect("group 0");
        each(&text, &caps);
        at = if e == s { e + 1 } else { e };
    }
    at
}

#[test]
fn thousands_separators() {
    // "1234567".replace(/\B(?=(\d{3})+(?!\d))/g, ",")
    let re = regex(r"\B(?=(\d{3})+(?!\d))", "");
    let subject = "1234567";
    let mut cuts = Vec::new();
    each_match(&re, subject, |_, caps| cuts.push(caps.get(0).unwrap().0));
    assert_eq!(cuts, [1, 4]);
    let mut out = String::new();
    for (i, ch) in subject.chars().enumerate() {
        if cuts.contains(&i) {
            out.push(',');
        }
        out.push(ch);
    }
    assert_eq!(out, "1,234,567");
    let mut none = 0;
    each_match(&re, "123", |_, _| none += 1);
    assert_eq!(none, 0);
}

#[test]
fn query_string_and_cookie_parsing() {
    let mut pairs = Vec::new();
    each_match(&regex(r"[?&]([^=&#]+)=([^&#]*)", ""), "?q=rust+regex&page=2&empty=#top", |text, caps| {
        pairs.push((group(text, caps, 1).unwrap(), group(text, caps, 2).unwrap()));
    });
    let expected = [("q", "rust+regex"), ("page", "2"), ("empty", "")];
    assert_eq!(pairs, expected.map(|(k, v)| (k.to_string(), v.to_string())));

    let mut cookies = Vec::new();
    each_match(&regex(r"(?:^|;\s*)([^=;\s]+)=([^;]*)", ""), "sid=abc123; theme=dark; empty=", |text, caps| {
        cookies.push(format!("{}={}", group(text, caps, 1).unwrap(), group(text, caps, 2).unwrap()));
    });
    assert_eq!(cookies, ["sid=abc123", "theme=dark", "empty="]);
}

#[test]
fn sticky_expression_tokenizer() {
    // The classic lexer loop: one sticky regex, lastIndex advanced by hand.
    let re = regex(r"\s*(?:(\d+\.?\d*)|([A-Za-z_]\w*)|(.))", "y");
    let mut tokens = Vec::new();
    let end = each_match(&re, "x1 = 3.14*(y+2)", |text, caps| {
        let token = match (group(text, caps, 1), group(text, caps, 2), group(text, caps, 3)) {
            (Some(n), _, _) => format!("num:{n}"),
            (_, Some(id), _) => format!("id:{id}"),
            (_, _, Some(op)) => format!("op:{op}"),
            _ => unreachable!("one alternative always participates"),
        };
        tokens.push(token);
    });
    assert_eq!(tokens, ["id:x1", "op:=", "num:3.14", "op:*", "op:(", "id:y", "op:+", "num:2", "op:)"]);
    assert_eq!(end, 15);
    // Sticky means no skipping ahead: from index 2 the next token must start right there.
    let text = u16s("ab cd");
    let word = regex(r"\w+", "y");
    let mut steps = 1000;
    assert!(word.exec(&text, 2, &mut steps).unwrap().is_none());
    assert_eq!(word.exec(&text, 3, &mut steps).unwrap().unwrap().get(0), Some((3, 5)));
}

#[test]
fn sticky_css_selector_tokenizer() {
    let re = regex(r#"\s*([>+~])\s*|(\s+)|#([\w-]+)|\.([\w-]+)|\[([\w-]+)(?:([~|^$*]?=)"([^"]*)")?\]|([\w-]+|\*)"#, "y");
    let mut tokens = Vec::new();
    let end = each_match(&re, "div#main > ul.nav li[data-x=\"1\"]", |text, caps| {
        let g = |i| group(text, caps, i);
        let token = if let Some(c) = g(1) {
            format!("comb:{c}")
        } else if g(2).is_some() {
            "descendant".to_string()
        } else if let Some(id) = g(3) {
            format!("id:{id}")
        } else if let Some(class) = g(4) {
            format!("class:{class}")
        } else if let Some(attr) = g(5) {
            format!("attr:{attr}{}{}", g(6).unwrap_or_default(), g(7).unwrap_or_default())
        } else {
            format!("tag:{}", g(8).unwrap())
        };
        tokens.push(token);
    });
    assert_eq!(tokens, ["tag:div", "id:main", "comb:>", "tag:ul", "class:nav", "descendant", "tag:li", "attr:data-x=1"]);
    assert_eq!(end, 32);
}
