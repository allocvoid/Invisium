//! Acceptance B3: the generated General_Category table is well formed and puts every code
//! point in exactly one leaf category.

use crate::charset::MAX_CODE_POINT;
use crate::gc_table::{CATEGORY_NAMES, GC_TABLE};
use crate::props::lookup;

#[test]
fn table_is_sorted_merged_and_under_32_kb() {
    assert_eq!(GC_TABLE.first().map(|e| e >> 8), Some(0), "the table must start at U+0000");
    for pair in GC_TABLE.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        assert!(a >> 8 < b >> 8, "not sorted at {:X}", b >> 8);
        assert_ne!(a & 0xFF, b & 0xFF, "neighbouring runs share a category at {:X}", b >> 8);
    }
    for entry in GC_TABLE {
        assert!(entry >> 8 <= MAX_CODE_POINT);
        assert!(((entry & 0xFF) as usize) < CATEGORY_NAMES.len());
    }
    let bytes = std::mem::size_of_val(&GC_TABLE);
    assert!(bytes < 32 * 1024, "table is {bytes} bytes");
}

#[test]
fn every_code_point_is_in_exactly_one_leaf_category() {
    let mut all = Vec::new();
    for name in CATEGORY_NAMES {
        let set = lookup(name).unwrap_or_else(|| panic!("no set for {name}"));
        all.extend(set.ranges().iter().map(|&(lo, hi)| (lo, hi, name)));
    }
    all.sort_unstable();
    // Sorted ranges that tile 0..=10FFFF with no gap and no overlap: each code point is in
    // exactly one leaf.
    let mut next = 0u32;
    for (lo, hi, name) in all {
        assert_eq!(lo, next, "{name}: gap or overlap at U+{lo:04X}");
        next = hi + 1;
    }
    assert_eq!(next, MAX_CODE_POINT + 1);
}

#[test]
fn unassigned_and_assigned_cover_the_code_space() {
    let cn = lookup("Cn").expect("Cn");
    let assigned = lookup("Assigned").expect("Assigned");
    assert_eq!(cn.count() + assigned.count(), 0x110000);
    assert!(cn.intersection(&assigned).ranges().is_empty());
}
