//! The public contract: flags, error values, group metadata and the `Captures` accessors.

mod common;

use common::{compile, regex, u16s};
use invisium_regex::{Captures, ExecError, Flags, Regex, SyntaxError};

#[test]
fn flags_parse_and_print() {
    assert_eq!(Flags::parse(""), Some(Flags::default()));
    let all = Flags::parse("ygmsiud").unwrap();
    assert!(all.has_indices && all.global && all.ignore_case && all.multiline && all.dot_all && all.unicode && all.sticky);
    assert!(!all.unicode_sets);
    assert_eq!(all.to_string(), "dgimsuy");
    let v = Flags::parse("v").unwrap();
    assert!(v.unicode && v.unicode_sets, "`v` also sets `unicode`");
    assert_eq!(v.to_string(), "v");
    assert_eq!(Flags::parse("yvd").unwrap().to_string(), "dvy");
    for bad in ["x", "gg", "uv", "vu", "iI", "g i", "dgimsuvy"] {
        assert_eq!(Flags::parse(bad), None, "{bad:?}");
    }
    for good in ["d", "g", "i", "m", "s", "u", "v", "y", "dgimsuy", "dgimsvy"] {
        assert_eq!(Flags::parse(good).unwrap().to_string(), good);
    }
}

#[test]
fn regex_metadata() {
    let re = regex(r"(?<year>\d{4})-(\d\d)-(?<day>\d\d)|(x)", "gi");
    assert_eq!(re.group_count(), 4);
    assert_eq!(re.group_names(), [("year".to_string(), 1), ("day".to_string(), 3)]);
    assert_eq!(re.flags(), Flags::parse("gi").unwrap());
    assert!(re.memory_bytes() > 0);
    assert_eq!(regex("abc", "").group_count(), 0);
    assert!(regex("abc", "").group_names().is_empty());
    // A larger pattern reports more memory than a smaller one.
    assert!(regex(&"[a-f]x".repeat(500), "").memory_bytes() > regex("[a-f]x", "").memory_bytes());
}

#[test]
fn captures_accessors() {
    let re = regex("(a)|(b)", "");
    let mut steps = 1000;
    let caps: Captures = re.exec(&u16s("xb"), 0, &mut steps).unwrap().unwrap();
    assert_eq!(caps.len(), re.group_count() + 1);
    assert_eq!(caps.get(0), Some((1, 2)));
    assert_eq!(caps.get(1), None);
    assert_eq!(caps.get(2), Some((1, 2)));
    assert_eq!(caps.get(3), None);
    assert_eq!(caps.get(usize::MAX), None);
    assert_eq!(caps.clone(), caps);
    assert!(steps < 1000, "exec must spend budget");
}

#[test]
fn start_beyond_the_text_is_no_match() {
    let re = regex("", "");
    let mut steps = 1000;
    assert!(re.exec(&u16s("ab"), 2, &mut steps).unwrap().is_some());
    assert!(re.exec(&u16s("ab"), 3, &mut steps).unwrap().is_none());
    assert!(re.exec(&[], usize::MAX, &mut steps).unwrap().is_none());
    assert_eq!(re.exec(&[], 0, &mut steps).unwrap().unwrap().get(0), Some((0, 0)));
}

#[test]
fn global_is_carried_but_not_interpreted() {
    // With or without `g`, exec finds the leftmost match from `start` and keeps no state.
    for flags in ["", "g", "d", "dg"] {
        let re = regex("a", flags);
        let text = u16s("aXa");
        let mut steps = 1000;
        assert_eq!(re.exec(&text, 0, &mut steps).unwrap().unwrap().get(0), Some((0, 1)));
        assert_eq!(re.exec(&text, 0, &mut steps).unwrap().unwrap().get(0), Some((0, 1)));
        assert_eq!(re.exec(&text, 1, &mut steps).unwrap().unwrap().get(0), Some((2, 3)));
    }
}

#[test]
fn syntax_errors_carry_a_message_and_an_offset() {
    let error: SyntaxError = compile("ab(?<n>c)[z-a]", "").unwrap_err();
    assert!(!error.message.is_empty());
    assert_eq!(error.offset, 10, "{error:?}");
    assert_eq!(compile("a)", "").unwrap_err().offset, 1);
    assert_eq!(compile("abc\\", "").unwrap_err().offset, 3);
    assert_eq!(compile("ab*+", "").unwrap_err().offset, 3);
    assert_eq!(compile(r"a\p{Nope}", "u").unwrap_err().offset, 1);
    assert_eq!(error.clone(), error);
    assert!(error.to_string().contains(&error.message));
    assert_ne!(ExecError::StepLimit, ExecError::StackLimit);
}

#[test]
fn regex_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Regex>();
    assert_send_sync::<Captures>();
    // One compiled expression can be shared by threads, each with its own budget.
    let re = regex(r"(\w+)@(\w+)", "");
    std::thread::scope(|scope| {
        for name in ["ann", "bob", "cy"] {
            let re = &re;
            scope.spawn(move || {
                let text = u16s(&format!("mail {name}@host now"));
                let mut steps = 10_000;
                let caps = re.exec(&text, 0, &mut steps).unwrap().unwrap();
                assert_eq!(caps.get(1), Some((5, 5 + name.len())));
            });
        }
    });
}
