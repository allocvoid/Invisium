//! Acceptance criterion A2: the table-driven conformance suite. Every expected value is what a
//! JavaScript engine returns for the same pattern, flags, subject and `lastIndex`.

#[path = "../common/mod.rs"]
mod common;

mod annexb;
mod asserts;
mod basic;
mod icase;
mod loops;
mod semantics;
mod unicode;

use common::{check, Case};

const TABLES: &[(&str, &[Case])] = &[
    ("basic", basic::CASES),
    ("semantics", semantics::CASES),
    ("loops", loops::CASES),
    ("asserts", asserts::CASES),
    ("icase", icase::CASES),
    ("unicode", unicode::CASES),
    ("annexb", annexb::CASES),
];

#[test]
fn basic_syntax() {
    check("basic", basic::CASES);
}

#[test]
fn loop_backreference_and_lookaround_semantics() {
    check("semantics", semantics::CASES);
}

#[test]
fn counted_and_nested_loops() {
    check("loops", loops::CASES);
}

#[test]
fn anchors_dot_boundaries_sticky() {
    check("asserts", asserts::CASES);
}

#[test]
fn case_insensitive() {
    check("icase", icase::CASES);
}

#[test]
fn surrogates_properties_named_groups() {
    check("unicode", unicode::CASES);
}

#[test]
fn annex_b_and_unicode_mode_twins() {
    check("annexb", annexb::CASES);
}

#[test]
fn suite_has_at_least_250_cases() {
    let total: usize = TABLES.iter().map(|(_, t)| t.len()).sum();
    assert!(total >= 250, "only {total} table cases");
}
