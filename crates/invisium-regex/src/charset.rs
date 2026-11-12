//! Sets of code points as sorted, non-overlapping, inclusive ranges.

use std::cmp::Ordering;

pub(crate) const MAX_CODE_POINT: u32 = 0x10FFFF;

/// A set under construction or in normal form. Call [`CharSet::normalize`] after the last
/// `add*` and before `contains`, `negated` or `ranges`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct CharSet {
    ranges: Vec<(u32, u32)>,
}

impl CharSet {
    pub(crate) fn new() -> CharSet {
        CharSet { ranges: Vec::new() }
    }

    pub(crate) fn from_ranges(ranges: &[(u32, u32)]) -> CharSet {
        let mut set = CharSet { ranges: ranges.to_vec() };
        set.normalize();
        set
    }

    pub(crate) fn single(c: u32) -> CharSet {
        CharSet { ranges: vec![(c, c)] }
    }

    pub(crate) fn add(&mut self, c: u32) {
        self.ranges.push((c, c));
    }

    pub(crate) fn add_range(&mut self, lo: u32, hi: u32) {
        if lo <= hi {
            self.ranges.push((lo, hi));
        }
    }

    pub(crate) fn add_set(&mut self, other: &CharSet) {
        self.ranges.extend_from_slice(&other.ranges);
    }

    /// Sort and merge overlapping or adjacent ranges.
    pub(crate) fn normalize(&mut self) {
        if self.ranges.len() < 2 {
            return;
        }
        self.ranges.sort_unstable();
        let mut out: Vec<(u32, u32)> = Vec::with_capacity(self.ranges.len());
        for &(lo, hi) in &self.ranges {
            match out.last_mut() {
                Some(last) if lo <= last.1.saturating_add(1) => {
                    if hi > last.1 {
                        last.1 = hi;
                    }
                }
                _ => out.push((lo, hi)),
            }
        }
        self.ranges = out;
    }

    /// Complement within `0 ..= 0x10FFFF`.
    pub(crate) fn negated(&self) -> CharSet {
        let mut out = Vec::with_capacity(self.ranges.len() + 1);
        let mut next = 0u32;
        for &(lo, hi) in &self.ranges {
            if lo > next {
                out.push((next, lo - 1));
            }
            next = hi.saturating_add(1);
        }
        if next <= MAX_CODE_POINT {
            out.push((next, MAX_CODE_POINT));
        }
        CharSet { ranges: out }
    }

    pub(crate) fn contains(&self, c: u32) -> bool {
        search(&self.ranges, c)
    }

    pub(crate) fn ranges(&self) -> &[(u32, u32)] {
        &self.ranges
    }

    /// Number of members, saturating.
    pub(crate) fn count(&self) -> u32 {
        let mut n = 0u32;
        for &(lo, hi) in &self.ranges {
            n = n.saturating_add(hi - lo).saturating_add(1);
        }
        n
    }

    /// The only member, if the set has exactly one.
    pub(crate) fn single_member(&self) -> Option<u32> {
        match self.ranges.as_slice() {
            [(lo, hi)] if lo == hi => Some(*lo),
            _ => None,
        }
    }
}

fn search(ranges: &[(u32, u32)], c: u32) -> bool {
    ranges
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                Ordering::Less
            } else if lo > c {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

/// A set in the form the matcher uses: a bitmap for ASCII plus the sorted ranges.
#[derive(Debug)]
pub(crate) struct SetData {
    ascii: u128,
    ranges: Box<[(u32, u32)]>,
}

impl SetData {
    /// `set` must be normalized.
    pub(crate) fn new(set: &CharSet) -> SetData {
        let mut ascii = 0u128;
        for &(lo, hi) in set.ranges() {
            if lo > 127 {
                break;
            }
            for c in lo..=hi.min(127) {
                ascii |= 1u128 << c;
            }
        }
        SetData { ascii, ranges: set.ranges().to_vec().into_boxed_slice() }
    }

    #[inline]
    pub(crate) fn contains(&self, c: u32) -> bool {
        if c < 128 {
            (self.ascii >> c) & 1 != 0
        } else {
            search(&self.ranges, c)
        }
    }

    pub(crate) fn memory_bytes(&self) -> usize {
        std::mem::size_of::<SetData>() + self.ranges.len() * std::mem::size_of::<(u32, u32)>()
    }
}

/// Accumulates a set from many pieces, compacting now and then so that a large piece named
/// thousands of times (a property class, say) cannot pile up in memory.
pub(crate) struct SetBuilder {
    set: CharSet,
    compact_at: usize,
}

impl SetBuilder {
    pub(crate) fn new() -> SetBuilder {
        SetBuilder { set: CharSet::new(), compact_at: 4096 }
    }

    pub(crate) fn add(&mut self, c: u32) {
        self.set.add(c);
        self.compact();
    }

    pub(crate) fn add_range(&mut self, lo: u32, hi: u32) {
        self.set.add_range(lo, hi);
        self.compact();
    }

    pub(crate) fn add_set(&mut self, other: &CharSet) {
        self.set.add_set(other);
        self.compact();
    }

    fn compact(&mut self) {
        if self.set.ranges.len() > self.compact_at {
            self.set.normalize();
            self.compact_at = (self.set.ranges.len() * 2).max(4096);
        }
    }

    /// The normalized set.
    pub(crate) fn finish(mut self) -> CharSet {
        self.set.normalize();
        self.set
    }
}
