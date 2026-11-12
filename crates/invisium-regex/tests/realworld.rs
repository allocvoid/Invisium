//! Acceptance criterion A4 (part 1): real-world patterns with realistic subjects and the
//! captures a JavaScript engine returns. Groups are joined with `¶`; `∅` is an unset group.

mod common;

use common::{groups, regex};

const NONE: &str = "<no match>";

/// `(pattern, flags, subject, expected groups)`
type Row = (&'static str, &'static str, &'static str, &'static str);

const UA_CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const URL_SPLIT: &str = r"^(([^:/?#]+):)?(//([^/?#]*))?([^?#]*)(\?([^#]*))?(#(.*))?";
const ISO_DATE: &str = r"^(\d{4})-(\d{2})-(\d{2})(?:T(\d{2}):(\d{2})(?::(\d{2})(?:\.(\d+))?)?(Z|[+-]\d{2}:\d{2})?)?$";
const UUID: &str = "^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";
const SEMVER: &str = r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$";
const RQUICK: &str = r"^(?:\s*(<[\w\W]+>)[^>]*|#([\w-]+))$";
const IPV4: &str = r"^(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)$";
const NUMBER: &str = r"^[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?$";
const PASSWORD: &str = r"^(?=.*[a-z])(?=.*[A-Z])(?=.*\d)(?=.*[^\w\s]).{8,}$";
const BASE64: &str = "^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$";

const ROWS: &[Row] = &[
    // RFC 3986 appendix B.
    (URL_SPLIT, "", "http://www.ics.uci.edu/pub/ietf/uri/#Related",
        "http://www.ics.uci.edu/pub/ietf/uri/#Related¶http:¶http¶//www.ics.uci.edu¶www.ics.uci.edu¶/pub/ietf/uri/¶∅¶∅¶#Related¶Related"),
    (URL_SPLIT, "", "https://example.com:8080/a/b?x=1&y=2#frag",
        "https://example.com:8080/a/b?x=1&y=2#frag¶https:¶https¶//example.com:8080¶example.com:8080¶/a/b¶?x=1&y=2¶x=1&y=2¶#frag¶frag"),
    (URL_SPLIT, "", "mailto:someone@example.org", "mailto:someone@example.org¶mailto:¶mailto¶∅¶∅¶someone@example.org¶∅¶∅¶∅¶∅"),
    // ISO 8601 dates.
    (ISO_DATE, "", "2024-02-29T13:45:10.123Z", "2024-02-29T13:45:10.123Z¶2024¶02¶29¶13¶45¶10¶123¶Z"),
    (ISO_DATE, "", "2024-02-29", "2024-02-29¶2024¶02¶29¶∅¶∅¶∅¶∅¶∅"),
    (ISO_DATE, "", "2024-02-29T08:00+02:00", "2024-02-29T08:00+02:00¶2024¶02¶29¶08¶00¶∅¶∅¶+02:00"),
    (ISO_DATE, "", "2024-2-29", NONE),
    // UUID, semver, colours.
    (UUID, "i", "123E4567-E89B-12D3-A456-426614174000", "123E4567-E89B-12D3-A456-426614174000"),
    (UUID, "i", "123e4567-e89b-62d3-a456-426614174000", NONE),
    (SEMVER, "", "1.2.3-alpha.1+build.5", "1.2.3-alpha.1+build.5¶1¶2¶3¶alpha.1¶build.5"),
    (SEMVER, "", "10.0.0", "10.0.0¶10¶0¶0¶∅¶∅"),
    (SEMVER, "", "1.02.3", NONE),
    (r"^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$", "i", "#FFa07A", "#FFa07A¶FF¶a0¶7A"),
    ("^#([0-9a-f]{3}|[0-9a-f]{6})$", "i", "#abc", "#abc¶abc"),
    ("^#([0-9a-f]{3}|[0-9a-f]{6})$", "i", "#abcd", NONE),
    (r"^rgba?\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})\s*(?:,\s*([\d.]+)\s*)?\)$", "", "rgba(255, 128,0 , 0.5)", "rgba(255, 128,0 , 0.5)¶255¶128¶0¶0.5"),
    (r"^([+-]?(?:\d+|\d*\.\d+))(px|em|rem|%|vh|vw)?$", "", "12.5px", "12.5px¶12.5¶px"),
    (r"^([+-]?(?:\d+|\d*\.\d+))(px|em|rem|%|vh|vw)?$", "", "-3", "-3¶-3¶∅"),
    // jQuery's rquickExpr and friends.
    (RQUICK, "", "#main-content", "#main-content¶∅¶main-content"),
    (RQUICK, "", "  <div class='a'>x</div>  ", "  <div class='a'>x</div>  ¶<div class='a'>x</div>¶∅"),
    (RQUICK, "", "div.foo", NONE),
    (r"^(?:#([\w-]+)|(\w+)|\.([\w-]+))$", "", ".btn-primary", ".btn-primary¶∅¶∅¶btn-primary"),
    (r"^(?:#([\w-]+)|(\w+)|\.([\w-]+))$", "", "div", "div¶∅¶div¶∅"),
    (r#"^\[\s*([\w-]+)\s*(?:([*^$|!~]?=)\s*(?:"([^"]*)"|'([^']*)'|([^\]\s]+)))?\s*\]$"#, "", "[data-id ^= 'ab c']", "[data-id ^= 'ab c']¶data-id¶^=¶∅¶ab c¶∅"),
    // HTML.
    (r"<([a-z][a-z0-9]*)\b[^>]*>(.*?)</\1>", "i", "<p>one</p><DIV class=\"x\">two <b>bold</b></div>", "<p>one</p>¶p¶one"),
    (r"<(div)\b[^>]*>(.*?)</\1>", "i", "<p>one</p><DIV class=\"x\">two <b>bold</b></div>", "<DIV class=\"x\">two <b>bold</b></div>¶DIV¶two <b>bold</b>"),
    (r#"<(\w+)((?:\s+[\w-]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s'">=]+))?)*)\s*(/?)>"#, "", "<img src=\"a.png\" alt='x' hidden/>", "<img src=\"a.png\" alt='x' hidden/>¶img¶ src=\"a.png\" alt='x' hidden¶/"),
    (r"\{\{#(\w+)\}\}([\s\S]*?)\{\{\/\1\}\}", "", "{{#list}}a{{/other}}b{{/list}}", "{{#list}}a{{/other}}b{{/list}}¶list¶a{{/other}}b"),
    // User-agent sniffing.
    (r"(Chrome|Firefox|Safari|Edg)\/(\d+)\.(\d+)", "", UA_CHROME, "Chrome/120.0¶Chrome¶120¶0"),
    (r"\b(?:Win(?:dows)?|Mac(?:intosh)?|Linux|X11)\b", "", UA_CHROME, "Windows"),
    (r"MSIE (\d+\.\d+);", "", "Mozilla/4.0 (compatible; MSIE 8.0; Windows NT 6.1)", "MSIE 8.0;¶8.0"),
    (r"(?:iPhone|iPad|iPod).*OS (\d+)_(\d+)", "", "Mozilla/5.0 (iPhone; CPU iPhone OS 17_2 like Mac OS X)", "iPhone; CPU iPhone OS 17_2¶17¶2"),
    (r"Android\s([0-9.]+)", "", "Mozilla/5.0 (Linux; Android 13; Pixel 7)", "Android 13¶13"),
    // Validation.
    (r"^[\w.+-]+@[\w-]+(?:\.[\w-]+)+$", "", "john.doe+tag@example.co.uk", "john.doe+tag@example.co.uk"),
    (r"^[\w.+-]+@[\w-]+(?:\.[\w-]+)+$", "", "john@@example.com", NONE),
    (IPV4, "", "192.168.1.254", "192.168.1.254"),
    (IPV4, "", "256.1.1.1", NONE),
    (NUMBER, "", "-12.5e+3", "-12.5e+3"),
    (NUMBER, "", ".5", ".5"),
    (NUMBER, "", "1e", NONE),
    (PASSWORD, "", "Passw0rd!", "Passw0rd!"),
    (PASSWORD, "", "password1", NONE),
    (BASE64, "", "SGVsbG8gd29ybGQ=", "SGVsbG8gd29ybGQ="),
    (BASE64, "", "SGVsbG8=d29y", NONE),
    (r"^[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*$", "", "window.app.$store", "window.app.$store"),
    (r"^([0-9A-F]{2})([:-])(?:[0-9A-F]{2}\2){4}[0-9A-F]{2}$", "i", "00:1a:2B:3c:4D:5e", "00:1a:2B:3c:4D:5e¶00¶:"),
    (r"^([0-9A-F]{2})([:-])(?:[0-9A-F]{2}\2){4}[0-9A-F]{2}$", "i", "00:1a-2B:3c:4D:5e", NONE),
    // Dates, times, durations.
    (r"^(\d{1,2})([-/.])(\d{1,2})\2(\d{4})$", "", "31-12-2024", "31-12-2024¶31¶-¶12¶2024"),
    (r"^(\d{1,2})([-/.])(\d{1,2})\2(\d{4})$", "", "31-12/2024", NONE),
    (r"^([01]?\d|2[0-3]):([0-5]\d)(?::([0-5]\d))?\s*([ap]m)?$", "i", "9:05 PM", "9:05 PM¶9¶05¶∅¶PM"),
    (r"^P(?:(\d+)Y)?(?:(\d+)M)?(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+(?:\.\d+)?)S)?)?$", "", "P1Y2DT3H4.5S", "P1Y2DT3H4.5S¶1¶∅¶2¶3¶∅¶4.5"),
    // Text extraction.
    (r"\{\{\s*([\w.]+)\s*\}\}", "", "Hello {{ user.name }}, you have {{count}} new", "{{ user.name }}¶user.name"),
    (r"\$\{([^}]+)\}", "", "a ${x + 1} b", "${x + 1}¶x + 1"),
    (r#"\[([^\]]+)\]\(([^)\s]+)(?:\s+"([^"]*)")?\)"#, "", "see [the docs](https://x.dev/a \"Docs\") now", "[the docs](https://x.dev/a \"Docs\")¶the docs¶https://x.dev/a¶Docs"),
    (r#""((?:[^"\\]|\\.)*)""#, "", r#"say "he said \"hi\"" ok"#, r#""he said \"hi\""¶he said \"hi\""#),
    (r"\b(\w+)\s+\1\b", "i", "This is is a test", "is is¶is"),
    (r"^(.*[\\/])?([^\\/]+?)(\.[^.\\/]*)?$", "", "/usr/local/lib/libfoo.so.1", "/usr/local/lib/libfoo.so.1¶/usr/local/lib/¶libfoo.so¶.1"),
    (r"^(?:https?:\/\/)?(?:www\.)?([^\/:?#]+)", "i", "HTTPS://www.Example.com:8080/path", "HTTPS://www.Example.com¶Example.com"),
    (r"(?:youtube\.com\/(?:watch\?v=|embed\/)|youtu\.be\/)([\w-]{11})", "", "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1", "youtube.com/watch?v=dQw4w9WgXcQ¶dQw4w9WgXcQ"),
    (r"%(\d+\$)?([-+ 0#]*)(\d+|\*)?(?:\.(\d+|\*))?([hlLzjt]*)([diouxXeEfgGcs%])", "", "val=%-08.3lf!", "%-08.3lf¶∅¶-0¶8¶3¶l¶f"),
    (r#"^([\w!#$%&'*+.^`|~-]+)\/([\w!#$%&'*+.^`|~-]+)(?:\s*;\s*charset=("?)([\w-]+)\3)?"#, "i", "text/html; charset=\"UTF-8\"", "text/html; charset=\"UTF-8\"¶text¶html¶\"¶UTF-8"),
    (r"^data:([a-z]+\/[a-z0-9.+-]+)?(;charset=[a-z0-9-]+)?(;base64)?,(.*)$", "i", "data:image/png;base64,iVBORw0KGgo=", "data:image/png;base64,iVBORw0KGgo=¶image/png¶∅¶;base64¶iVBORw0KGgo="),
    (r"^0+(?=\d)", "", "000123", "000"),
    (r"^0+(?=\d)", "", "0", NONE),
    (r"\/+$", "", "http://a/b///", "///"),
    (r"@(\p{L}[\p{L}\p{N}_]*)", "u", "hi @Jos\u{e9}_1!", "@Jos\u{e9}_1¶Jos\u{e9}_1"),
    (r"\B(?=(\d{3})+(?!\d))", "", "1234567", "¶567"),
];

#[test]
fn real_world_patterns() {
    let mut failures = Vec::new();
    for &(pattern, flags, subject, expected) in ROWS {
        let got = match groups(&regex(pattern, flags), subject, 0) {
            Some(found) => found.iter().map(|g| g.as_deref().unwrap_or("∅")).collect::<Vec<_>>().join("¶"),
            None => NONE.to_string(),
        };
        if got != expected {
            failures.push(format!("  /{pattern}/{flags} on {subject:?}\n    expected {expected:?}\n    got      {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} rows failed:\n{}", failures.len(), ROWS.len(), failures.join("\n"));
}

#[test]
fn at_least_forty_distinct_patterns() {
    let mut patterns: Vec<&str> = ROWS.iter().map(|r| r.0).collect();
    patterns.sort_unstable();
    patterns.dedup();
    assert!(patterns.len() >= 40, "only {} distinct patterns", patterns.len());
}
