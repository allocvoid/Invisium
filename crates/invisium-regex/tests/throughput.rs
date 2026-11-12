//! Acceptance criterion A5: throughput sanity. A global scan over a long subject must not
//! run the matcher at every index; each scan here has to finish well within a second.

mod common;

use common::{regex, u16s};
use invisium_regex::Regex;
use std::time::{Duration, Instant};

/// Count the matches of a global scan and check each one against `verify`.
fn count_matches(re: &Regex, text: &[u16], mut verify: impl FnMut(&[u16])) -> usize {
    let mut steps = u64::MAX;
    let (mut at, mut count) = (0, 0);
    while let Some(caps) = re.exec(text, at, &mut steps).expect("exec failed") {
        let (s, e) = caps.get(0).expect("group 0");
        assert!(e > s, "empty match in a throughput scan");
        verify(&text[s..e]);
        count += 1;
        at = e;
    }
    count
}

/// Deterministic pseudo-random lower-case words.
struct Words(u32);

impl Words {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (self.0 >> 16) & 0x7FFF
    }

    fn word(&mut self) -> String {
        let len = 3 + self.next() % 7;
        (0..len).map(|_| char::from(b'a' + (self.next() % 26) as u8)).collect()
    }
}

#[test]
fn email_scan_over_200k_characters() {
    let mut words = Words(42);
    let mut subject = String::new();
    let mut emails = 0;
    while subject.len() < 200_000 {
        if words.next().is_multiple_of(12) {
            subject.push_str(&format!("{}@{}.com ", words.word(), words.word()));
            emails += 1;
        } else {
            subject.push_str(&words.word());
            subject.push(' ');
        }
    }
    let text = u16s(&subject);
    assert!(text.len() >= 200_000 && emails > 1000);
    let re = regex(r"[a-z]+@[a-z]+\.com", "");
    let at_sign = u16s("@");
    let started = Instant::now();
    let found = count_matches(&re, &text, |m| assert!(m.contains(&at_sign[0]) && m.ends_with(&u16s(".com"))));
    let elapsed = started.elapsed();
    assert_eq!(found, emails);
    assert!(elapsed < Duration::from_secs(1), "email scan took {elapsed:?}");
}

#[test]
fn literal_scan_over_one_million_characters() {
    // Filler that shares the needle's first letters, so the prefilter has real work to do.
    let filler = "the needful thing is never the needle-like nee ";
    let mut subject = String::new();
    let mut needles = 0;
    while subject.len() < 1_000_000 {
        for _ in 0..20 {
            subject.push_str(filler);
        }
        subject.push_str("a needle in a haystack. ");
        needles += 1;
    }
    let text = u16s(&subject);
    assert!(text.len() >= 1_000_000);
    let needle = u16s("needle in a haystack");
    let re = regex("needle in a haystack", "");
    let started = Instant::now();
    let found = count_matches(&re, &text, |m| assert_eq!(m, needle));
    let elapsed = started.elapsed();
    assert_eq!(found, needles);
    assert!(elapsed < Duration::from_secs(1), "literal scan took {elapsed:?}");

    // A literal that never occurs: one pass over the text, almost no matcher steps.
    let absent = regex("zebra crossing", "");
    let mut steps = 1_000_000u64;
    let started = Instant::now();
    assert!(absent.exec(&text, 0, &mut steps).unwrap().is_none());
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(steps > 999_000, "the prefilter should skip without spending steps");
}

/// Milestone 2, B5: table-backed property names are cheap to compile.
#[test]
fn compiling_a_property_class_a_thousand_times() {
    let started = Instant::now();
    for _ in 0..1000 {
        let re = regex(r"\p{L}", "u");
        assert_eq!(re.group_count(), 0);
    }
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(1), "1000 compiles took {elapsed:?}");
}

/// Milestone 2, B5: a property class scan over 100 000 characters of mixed scripts.
#[test]
fn property_class_scan_over_mixed_scripts() {
    let words = ["hello", "na\u{EF}ve", "\u{65E5}\u{672C}\u{8A9E}", "\u{43F}\u{440}\u{438}\u{432}\u{435}\u{442}", "e\u{301}t\u{E9}_1", "\u{639}\u{631}\u{628}\u{64A}", "\u{939}\u{93F}\u{928}\u{94D}\u{926}\u{940}", "x\u{1D7CE}\u{20000}"];
    let separators = [" ", ", ", "! ", " \u{2014} ", "\u{3000}"];
    let mut subject = String::new();
    let (mut expected, mut i) = (0, 0usize);
    while subject.chars().count() < 100_000 {
        subject.push_str(words[i % words.len()]);
        subject.push_str(separators[i % separators.len()]);
        expected += 1;
        i += 1;
    }
    let text = u16s(&subject);
    let re = regex(r"[\p{L}\p{M}\p{N}_]+", "u");
    let started = Instant::now();
    let found = count_matches(&re, &text, |m| assert!(!m.is_empty()));
    let elapsed = started.elapsed();
    assert_eq!(found, expected);
    assert!(elapsed < Duration::from_secs(1), "property scan took {elapsed:?}");
}

#[test]
fn case_insensitive_and_class_led_scans_stay_fast() {
    let mut subject = "lorem ipsum dolor sit amet, consectetur adipiscing elit ".repeat(4000);
    subject.push_str("Order #48213 shipped");
    let text = u16s(&subject);
    let started = Instant::now();
    let mut steps = u64::MAX;
    let caps = regex(r"#(\d+)", "").exec(&text, 0, &mut steps).unwrap().unwrap();
    assert_eq!(caps.get(1).map(|(s, e)| e - s), Some(5));
    let caps = regex("ORDER", "i").exec(&text, 0, &mut steps).unwrap().unwrap();
    assert_eq!(caps.get(0).map(|(s, _)| s), Some(text.len() - 20));
    assert!(started.elapsed() < Duration::from_secs(1), "took {:?}", started.elapsed());
}
