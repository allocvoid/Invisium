//! Acceptance criterion A3: hostile patterns and subjects. Everything here runs on the default
//! test-thread stack; nothing may panic, hang, or use unbounded memory.

mod common;

use common::{compile, regex, spans, u16s};
use invisium_regex::{ExecError, Flags, Regex};
use std::time::{Duration, Instant};

fn exec(re: &Regex, text: &[u16], budget: u64) -> (Result<Option<String>, ExecError>, u64) {
    let mut steps = budget;
    let result = re.exec(text, 0, &mut steps).map(|m| m.map(|c| spans(&c)));
    (result, budget - steps)
}

#[test]
fn catastrophic_patterns_hit_the_step_limit() {
    let a40 = "a".repeat(40);
    let cases: [(&str, String); 6] = [
        ("(a+)+$", format!("{a40}!")),
        ("(a*)*b", a40.clone()),
        ("(a|aa)+$", format!("{}!", "a".repeat(60))),
        ("(.*)*x", a40.clone()),
        ("(?:a?){40}a{40}", "a".repeat(39)),
        ("(x+x+)+y", "x".repeat(200)),
    ];
    for (pattern, subject) in cases {
        let re = regex(pattern, "");
        let started = Instant::now();
        let mut steps = 1_000_000u64;
        let result = re.exec(&u16s(&subject), 0, &mut steps);
        assert_eq!(result, Err(ExecError::StepLimit), "/{pattern}/");
        assert_eq!(steps, 0, "/{pattern}/ must leave no budget");
        assert!(started.elapsed() < Duration::from_secs(2), "/{pattern}/ took {:?}", started.elapsed());
    }
}

#[test]
fn zero_budget_is_a_step_limit_and_steps_are_counted() {
    let re = regex("a+b", "");
    let mut steps = 0;
    assert_eq!(re.exec(&u16s("aab"), 0, &mut steps), Err(ExecError::StepLimit));
    let (result, used) = exec(&re, &u16s("aab"), 1000);
    assert_eq!(result, Ok(Some("0,3".to_string())));
    assert!(used > 0 && used < 20, "used {used} steps");
}

#[test]
fn long_subjects() {
    // 100 000 characters: a lazy scan to the comma, then a greedy scan to the end.
    let subject = u16s(&format!("{},{}", "a".repeat(50_000), "b".repeat(49_999)));
    let re = regex("^(.*?),(.*)$", "");
    let (result, used) = exec(&re, &subject, 1_000_000);
    assert_eq!(result, Ok(Some("0,100000 0,50000 50001,100000".to_string())));
    assert!(used < 400_000, "used {used} steps");
    // No comma at all: one linear attempt, and `^` rules out every other start.
    let (result, used) = exec(&re, &u16s(&"a".repeat(100_000)), 1_000_000);
    assert_eq!(result, Ok(None));
    assert!(used < 400_000, "used {used} steps");

    let xs = u16s(&"x".repeat(100_000));
    let started = Instant::now();
    let (result, _) = exec(&regex("(?:x+x+)+y", ""), &xs, 1_000_000);
    assert_eq!(result, Err(ExecError::StepLimit));
    assert!(started.elapsed() < Duration::from_secs(2));
    // The same subject is fine for a pattern that does not backtrack into it.
    let (result, _) = exec(&regex("x+$", ""), &xs, 1_000_000);
    assert_eq!(result, Ok(Some("0,100000".to_string())));
}

#[test]
fn deep_nesting_is_a_syntax_error_not_a_stack_overflow() {
    for opener in ["(", "(?:", "(?=", "(?<=", "(?<n>"] {
        assert!(compile(&opener.repeat(100_000), "").is_err(), "{opener} x 100000");
        assert!(compile(&opener.repeat(100_000), "u").is_err(), "{opener} x 100000 /u");
    }
    let balanced = |n: usize| format!("{}a{}", "(".repeat(n), ")".repeat(n));
    assert!(compile(&balanced(100_000), "").is_err());
    assert!(compile(&balanced(1001), "").is_err());
    // 1000 levels is allowed, and compiles and matches without deep recursion.
    let re = regex(&balanced(1000), "");
    assert_eq!(re.group_count(), 1000);
    let (result, _) = exec(&re, &u16s("xa"), 1_000_000);
    let caps = result.unwrap().unwrap();
    assert!(caps.starts_with("1,2 1,2 ") && caps.split(' ').count() == 1001);
    // Nested quantified groups and lookarounds, 1000 deep: any outcome but a crash.
    for (open, close) in [("(", ")*"), ("(?:", ")+"), ("(?=", ")"), ("(?<!", ")"), ("(?:", "){2,3}?")] {
        let pattern = format!("{}a{}", open.repeat(1000), close.repeat(1000));
        if let Ok(re) = compile(&pattern, "") {
            let _ = exec(&re, &u16s("aaaa"), 200_000);
        }
    }
}

#[test]
fn many_alternatives() {
    // 10 000 alternatives compile to a flat chain; the last one is reached.
    let words: Vec<String> = (0..10_000).map(|i| format!("k{i}")).collect();
    let re = regex(&format!("^(?:{})$", words.join("|")), "");
    let (result, used) = exec(&re, &u16s("k9999"), 1_000_000);
    assert_eq!(result, Ok(Some("0,5".to_string())));
    assert!(used < 100_000, "used {used} steps");
    assert_eq!(exec(&re, &u16s("k10000"), 1_000_000).0, Ok(None));
    // 100 000 alternatives exceed the 1 MiB program cap: a clean SyntaxError, promptly.
    let started = Instant::now();
    let huge = vec!["a"; 100_000].join("|");
    let error = compile(&huge, "").expect_err("program cap");
    assert!(error.message.contains("too large"), "{error:?}");
    assert!(compile(&"|".repeat(100_000), "u").is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn counted_repetition_is_a_loop_not_a_copy() {
    let re = regex("^a{99999}$", "");
    assert!(re.memory_bytes() < 4096, "{} bytes", re.memory_bytes());
    assert_eq!(exec(&re, &u16s(&"a".repeat(99_999)), 1_000_000).0, Ok(Some("0,99999".to_string())));
    assert_eq!(exec(&re, &u16s(&"a".repeat(99_998)), 1_000_000).0, Ok(None));

    let re = regex("(?:a{1000}){1000}", "y");
    assert!(re.memory_bytes() < 4096, "{} bytes", re.memory_bytes());
    let million = u16s(&"a".repeat(1_000_000));
    assert_eq!(exec(&re, &million, 5_000_000).0, Ok(Some("0,1000000".to_string())));
    assert_eq!(exec(&re, &million[1..], 5_000_000).0, Ok(None));

    let re = regex("(?:(a)|b){3,5}c", "");
    assert_eq!(exec(&re, &u16s("abbbbbc"), 10_000).0, Ok(Some("1,7 -".to_string())));
    for pattern in ["a{2,1}", "a{5,3}?", "(?:a){10,9}", "[ab]{2,1}", "a{99999999999999999999,5}", "(a){1,0}"] {
        assert!(compile(pattern, "").is_err(), "/{pattern}/");
        assert!(compile(pattern, "u").is_err(), "/{pattern}/u");
    }
    // Absurd counts are clamped, not overflowed.
    assert_eq!(exec(&regex("a{99999999999999999999}", ""), &u16s("aaa"), 10_000).0, Ok(None));
    assert_eq!(exec(&regex("a{0,99999999999999999999}", ""), &u16s("aaa"), 10_000).0, Ok(Some("0,3".to_string())));
    assert_eq!(exec(&regex("(?:){99999}", ""), &u16s(""), 10_000_000).0, Ok(Some("0,0".to_string())));
    assert_eq!(exec(&regex("(?:){4294967295}x", ""), &u16s("x"), 100_000).0, Err(ExecError::StepLimit));
}

#[test]
fn backtrack_memory_is_capped() {
    // Every iteration leaves choice points and capture undo records behind.
    let re = regex("(a|b)*c", "");
    let started = Instant::now();
    let mut steps = u64::MAX;
    assert_eq!(re.exec(&u16s(&"a".repeat(600_000)), 0, &mut steps), Err(ExecError::StackLimit));
    assert!(started.elapsed() < Duration::from_secs(5));
    // Well below the cap the same shape of pattern simply matches.
    let subject = u16s(&format!("{}c", "ab".repeat(50_000)));
    let mut steps = u64::MAX;
    let caps = re.exec(&subject, 0, &mut steps).unwrap().unwrap();
    assert_eq!(spans(&caps), "0,100001 99999,100000");
}

/// xorshift64*: a small deterministic generator, so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn garbage_patterns_never_panic() {
    // Mostly syntax characters, plus a few letters, digits, non-ASCII and lone surrogates.
    let mut alphabet: Vec<u16> = u16s(r"()()[]{}|*+?^$\\\.-,:=!<>/abckpuxdwsbDWSBPn0123789_ é\u{3c3}");
    alphabet.extend_from_slice(&[0xD83D, 0xDE00, 0xDE00, 0x212A, 0x2028, 0x0A]);
    let subjects: Vec<Vec<u16>> = ["", "a", "aaaaaaaaaaaaaaaaaaaaab", "ab\nAB 12_-,k<a>{2}\u{3c3}\u{3c2}😀x", "\u{212a}\u{17f}ss\\c"]
        .iter()
        .map(|s| u16s(s))
        .chain([vec![0xDE00, 0xD83D, 0xD83D, 0xDE00, 0x61]])
        .collect();
    let flag_sets: Vec<Flags> = ["", "i", "u", "iu", "m", "s", "y", "v", "imsuy"].iter().map(|f| Flags::parse(f).unwrap()).collect();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut compiled, mut rejected, mut matched) = (0u32, 0u32, 0u32);
    for round in 0..6000 {
        let len = 1 + rng.below(24);
        let pattern: Vec<u16> = (0..len).map(|_| alphabet[rng.below(alphabet.len())]).collect();
        let flags = flag_sets[round % flag_sets.len()];
        let Ok(re) = Regex::new(&pattern, flags) else {
            rejected += 1;
            continue;
        };
        compiled += 1;
        assert!(re.group_names().len() <= re.group_count());
        for subject in &subjects {
            for start in [0, 1, subject.len()] {
                let mut steps = 20_000;
                if let Ok(Some(caps)) = re.exec(subject, start, &mut steps) {
                    matched += 1;
                    assert_eq!(caps.len(), re.group_count() + 1);
                    for g in 0..caps.len() {
                        if let Some((s, e)) = caps.get(g) {
                            assert!(s <= e && e <= subject.len(), "group {g} = {s}..{e} for {pattern:?}");
                        }
                    }
                }
            }
        }
    }
    assert!(compiled > 500 && rejected > 500 && matched > 500, "{compiled} compiled, {rejected} rejected, {matched} matched");
}

#[test]
fn compile_time_is_bounded_for_hostile_patterns() {
    let started = Instant::now();
    // Tens of thousands of named groups: name lookups must not be quadratic.
    let named: String = (0..40_000).map(|i| format!("(?<g{i}>)")).collect();
    let re = regex(&named, "");
    assert_eq!((re.group_count(), re.group_names().len()), (40_000, 40_000));
    let refs: String = (0..20_000).map(|i| format!(r"(?<g{i}>)\k<g{i}>")).collect();
    assert_eq!(regex(&refs, "").group_count(), 20_000);
    // More groups than the engine supports is an error, not a crash.
    assert!(compile(&"()".repeat(70_000), "").is_err());
    // A large property class named many times is stored once.
    let re = regex(&r"\p{L}".repeat(20_000), "u");
    assert!(re.memory_bytes() < 400_000, "{} bytes", re.memory_bytes());
    let re = regex(&format!("[{}]+", r"\p{L}\p{N}".repeat(50_000)), "iu");
    assert_eq!(exec(&re, &u16s("-ab1-"), 1000).0, Ok(Some("1,4".to_string())));
    // Patterns far beyond the program cap are refused promptly.
    assert!(compile(&"a".repeat(2_000_000), "").is_err());
    assert!(compile(&"(?:)".repeat(600_000), "").is_err());
    assert!(compile(&"a{0}".repeat(600_000), "u").is_err());
    assert!(compile(&"[^ab]".repeat(300_000), "i").is_err());
    // Many distinct case-insensitive classes.
    let classes: String = (0..5_000).map(|i| format!("[^{}]", char::from_u32(0x4E00 + i).unwrap())).collect();
    assert!(compile(&classes, "i").is_ok());
    assert!(started.elapsed() < Duration::from_secs(10), "took {:?}", started.elapsed());
}
