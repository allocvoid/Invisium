//! Counted and nested loops: counters survive backtracking, captures are reset per iteration,
//! and the empty-iteration rule applies only once the minimum is met.

use crate::common::Case;

pub const CASES: &[Case] = &[
    ("(?:(?:ab){2}c){2}", "", "ababcababc", 0, "0,10"),
    ("(?:a{1,2}b){2,3}", "", "abaababab", 0, "0,7"),
    ("(?:x(?:ab){2,3}){2}", "", "xababxababab", 0, "0,12"),
    ("(?:(?:a){2}){3}", "", "aaaaaaa", 0, "0,6"),
    ("(?:a{2}){2,}", "", "aaaaaaa", 0, "0,6"),
    ("((a){2}){2}", "", "aaaa", 0, "0,4 2,4 3,4"),
    (r"(?:\d{1,3},){2}\d+", "", "1,22,333", 0, "0,8"),
    (r"^(?:(\d+)\.){3}(\d+)$", "", "10.20.30.40", 0, "0,11 6,8 9,11"),
    // Backtracking into an earlier iteration restores the counter.
    ("(a|ab){2}c", "", "abac", 0, "0,4 2,3"),
    ("(a{2,3}){2}", "", "aaaaa", 0, "0,5 3,5"),
    ("(a{2,3}){2}", "", "aaaa", 0, "0,4 2,4"),
    ("(a+|b+){2}", "", "aabb", 0, "0,4 2,4"),
    ("(a+|b+){2}", "", "aaa", 0, "0,3 2,3"),
    // Lazy counted loops.
    ("(?:ab){1,3}?c", "", "ababc", 0, "0,5"),
    ("(?:a|b){2,}?c", "", "abbc", 0, "0,4"),
    ("(x{2}|y)+?z", "", "xxyxxz", 0, "0,6 3,5"),
    ("(?:a?)*?b", "", "ab", 0, "0,2"),
    // Capture reset inside counted and nested loops.
    ("(?:(a)|(b)){2,3}", "", "ab", 0, "0,2 - 1,2"),
    ("(?:(a)|(b)|(c))*", "", "abcab", 0, "0,5 - 4,5 -"),
    ("(a(b)?)+", "", "aba", 0, "0,3 2,3 -"),
    ("(?:a(?:b(?:c)?)?)+", "", "abcaba", 0, "0,6"),
    (r"(?:(a)|b\1)+", "", "aba", 0, "0,3 2,3"),
    (r"(?:(a)|b\1){2}", "", "ab", 0, "0,2 -"),
    ("(a){0}b", "", "ab", 0, "1,2 -"),
    // Empty iterations.
    ("(a*){0,5}", "", "b", 0, "0,0 -"),
    ("(a*?)*b", "", "aab", 0, "0,3 1,2"),
    ("(a|)+", "", "b", 0, "0,0 0,0"),
    ("(|a)+", "", "aa", 0, "0,2 1,2"),
    ("(|a)*", "", "aa", 0, "0,2 1,2"),
    ("(?:a|()){3}", "", "a", 0, "0,1 1,1"),
    // Loops read backwards inside a lookbehind.
    ("(?<=(?:ab){2})c", "", "ababc", 0, "4,5"),
    (r"(?<=(\d){3})x", "", "123x", 0, "3,4 0,1"),
    (r"(?<=(\d){2,}?)x", "", "123x", 0, "3,4 1,2"),
];
