//! Milestone 2, acceptance B2 and B4: Unicode property escapes. Every General_Category leaf is
//! tried by all of its names, negated and inside classes; `cases.rs` has the groups, binary
//! properties, scripts and the names that must be refused.

#[path = "../common/mod.rs"]
mod common;

mod cases;

use common::{check, groups, outcome, regex, u16s, Case, NO};

/// `(short name, long name, a member, a non-member)` for every leaf category. The members
/// were checked against Python's `unicodedata` (Unicode 15.0.0). `¦XXXX` is one code unit.
const LEAVES: &[(&str, &str, &str, &str)] = &[
    ("Lu", "Uppercase_Letter", "B", "a"),
    ("Ll", "Lowercase_Letter", "b", "A"),
    ("Lt", "Titlecase_Letter", "\u{1C5}", "a"),
    ("Lm", "Modifier_Letter", "\u{2B0}", "a"),
    ("Lo", "Other_Letter", "\u{5D0}", "a"),
    ("Mn", "Nonspacing_Mark", "\u{301}", "a"),
    ("Mc", "Spacing_Mark", "\u{903}", "a"),
    ("Me", "Enclosing_Mark", "\u{20DD}", "a"),
    ("Nd", "Decimal_Number", "\u{663}", "a"),
    ("Nl", "Letter_Number", "\u{216B}", "a"),
    ("No", "Other_Number", "\u{BD}", "a"),
    ("Pc", "Connector_Punctuation", "_", "a"),
    ("Pd", "Dash_Punctuation", "-", "a"),
    ("Ps", "Open_Punctuation", "(", "a"),
    ("Pe", "Close_Punctuation", ")", "a"),
    ("Pi", "Initial_Punctuation", "\u{AB}", "a"),
    ("Pf", "Final_Punctuation", "\u{BB}", "a"),
    ("Po", "Other_Punctuation", "!", "a"),
    ("Sm", "Math_Symbol", "+", "a"),
    ("Sc", "Currency_Symbol", "$", "a"),
    ("Sk", "Modifier_Symbol", "^", "a"),
    ("So", "Other_Symbol", "\u{A9}", "a"),
    ("Zs", "Space_Separator", " ", "a"),
    ("Zl", "Line_Separator", "\u{2028}", "a"),
    ("Zp", "Paragraph_Separator", "\u{2029}", "a"),
    ("Cc", "Control", "\u{1}", "a"),
    ("Cf", "Format", "\u{AD}", "a"),
    ("Cs", "Surrogate", "¦D800", "a"),
    ("Co", "Private_Use", "\u{E000}", "a"),
    ("Cn", "Unassigned", "\u{378}", "a"),
];

/// The rows generated for one leaf: each name form finds the member after the non-member,
/// and the negated forms find the non-member after the member.
fn leaf_rows(short: &str, long: &str, member: &str, other: &str) -> Vec<(String, String, String)> {
    let (m, o) = (u16s(member).len(), u16s(other).len());
    let ahead = format!("{o},{}", o + m);
    let behind = format!("{m},{}", m + o);
    let (fwd, back) = (format!("{other}{member}"), format!("{member}{other}"));
    let mut rows = Vec::new();
    for name in [short.to_string(), long.to_string(), format!("gc={short}"), format!("General_Category={long}")] {
        rows.push((format!(r"\p{{{name}}}"), fwd.clone(), ahead.clone()));
        rows.push((format!(r"\P{{{name}}}"), back.clone(), behind.clone()));
    }
    rows.push((format!(r"[\p{{{short}}}]"), fwd.clone(), ahead.clone()));
    rows.push((format!(r"[x\p{{gc={long}}}]"), fwd.clone(), ahead));
    rows.push((format!(r"[^\p{{{long}}}]"), back.clone(), behind.clone()));
    rows.push((format!(r"[^\P{{{short}}}]"), back.clone(), format!("0,{m}")));
    rows.push((format!(r"\p{{{short}}}"), other.to_string(), NO.to_string()));
    rows
}

fn leaf_table() -> Vec<(String, String, String)> {
    LEAVES.iter().flat_map(|&(s, l, m, o)| leaf_rows(s, l, m, o)).collect()
}

#[test]
fn every_leaf_category_by_every_name() {
    let mut failures = Vec::new();
    for (pattern, subject, expected) in leaf_table() {
        let got = outcome(&pattern, "u", &subject, 0);
        if got != expected {
            failures.push(format!("  /{pattern}/u on {subject:?}: expected {expected:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{} rows failed:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn groups_and_aliases() {
    check("groups", cases::GROUPS);
}

#[test]
fn binary_properties() {
    check("binary", cases::BINARY);
}

#[test]
fn scripts_by_name_and_code() {
    check("scripts", cases::SCRIPTS);
}

#[test]
fn unknown_and_miscased_names() {
    check("unknown", cases::UNKNOWN);
}

#[test]
fn suite_has_at_least_200_cases() {
    let tables: [&[Case]; 4] = [cases::GROUPS, cases::BINARY, cases::SCRIPTS, cases::UNKNOWN];
    let total = leaf_table().len() + tables.iter().map(|t| t.len()).sum::<usize>();
    assert!(total >= 200, "only {total} property cases");
}

/// Every code point once, as a subject: surrogates are ordered low-then-high so no two of
/// them form a pair.
fn every_code_point() -> Vec<u16> {
    let mut text = Vec::with_capacity(0x110000 + 0x100000);
    text.extend(0..0xD800u16);
    text.extend(0xDC00..=0xDFFFu16);
    text.extend(0xD800..=0xDBFFu16);
    text.extend(0xE000..=0xFFFFu16);
    for cp in 0x10000..=0x10FFFFu32 {
        let v = cp - 0x10000;
        text.push(0xD800 + (v >> 10) as u16);
        text.push(0xDC00 + (v & 0x3FF) as u16);
    }
    text
}

#[test]
fn each_group_is_the_union_of_its_leaves() {
    let text = every_code_point();
    for group in ["L", "LC", "M", "N", "P", "S", "Z", "C"] {
        let leaves: Vec<&str> = match group {
            "LC" => vec!["Lu", "Ll", "Lt"],
            _ => LEAVES.iter().map(|l| l.0).filter(|s| s.starts_with(group)).collect(),
        };
        let union: String = leaves.iter().map(|l| format!(r"\p{{{l}}}")).collect();
        // A code point in the group but in none of the leaves, or the other way round.
        let pattern = format!(r"(?=\p{{{group}}})[^{union}]|(?=[{union}])\P{{{group}}}");
        let re = regex(&pattern, "u");
        let mut steps = u64::MAX;
        let found = re.exec(&text, 0, &mut steps).expect("exec failed");
        assert_eq!(found.and_then(|c| c.get(0)), None, "{group} differs from {leaves:?}");
    }
}

#[test]
fn chess_com_message_format_pattern() {
    let re = regex(r"([^\p{White_Space}\p{Pattern_Syntax}]*)", "uy");
    let some = |s: &str| Some(vec![Some(s.to_string()), Some(s.to_string())]);
    assert_eq!(groups(&re, "hello, world", 0), some("hello"));
    assert_eq!(groups(&re, "hello, world", 5), some(""));
    assert_eq!(groups(&re, "na\u{EF}ve{x}", 0), some("na\u{EF}ve"));
    assert_eq!(groups(&re, "\u{65E5}\u{672C}\u{8A9E} text", 0), some("\u{65E5}\u{672C}\u{8A9E}"));
}
