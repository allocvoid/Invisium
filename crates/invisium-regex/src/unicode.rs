//! Character helpers: surrogates, the ECMAScript escape sets, and the two `Canonicalize`
//! flavours used by case-insensitive matching.

use crate::charset::CharSet;
use std::sync::OnceLock;

#[inline]
pub(crate) fn is_high_surrogate(u: u32) -> bool {
    (0xD800..=0xDBFF).contains(&u)
}

#[inline]
pub(crate) fn is_low_surrogate(u: u32) -> bool {
    (0xDC00..=0xDFFF).contains(&u)
}

#[inline]
pub(crate) fn is_surrogate(u: u32) -> bool {
    (0xD800..=0xDFFF).contains(&u)
}

#[inline]
pub(crate) fn combine_surrogates(hi: u32, lo: u32) -> u32 {
    0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
}

/// Split a supplementary code point into its surrogate pair.
pub(crate) fn split_surrogates(c: u32) -> (u32, u32) {
    let v = c.saturating_sub(0x10000);
    (0xD800 + (v >> 10), 0xDC00 + (v & 0x3FF))
}

#[inline]
pub(crate) fn is_line_terminator(c: u32) -> bool {
    c == 0x0A || c == 0x0D || c == 0x2028 || c == 0x2029
}

#[inline]
pub(crate) fn is_ascii_word(c: u32) -> bool {
    matches!(c, 0x30..=0x39 | 0x41..=0x5A | 0x5F | 0x61..=0x7A)
}

/// `\d`
pub(crate) fn digit_set() -> CharSet {
    CharSet::from_ranges(&[(0x30, 0x39)])
}

/// `\s`: ECMAScript WhiteSpace plus LineTerminator.
pub(crate) fn space_set() -> CharSet {
    CharSet::from_ranges(&[
        (0x09, 0x0D),
        (0x20, 0x20),
        (0xA0, 0xA0),
        (0x1680, 0x1680),
        (0x2000, 0x200A),
        (0x2028, 0x2029),
        (0x202F, 0x202F),
        (0x205F, 0x205F),
        (0x3000, 0x3000),
        (0xFEFF, 0xFEFF),
    ])
}

/// `\w`. With `extra` (Unicode mode and `i`) it also holds U+017F and U+212A, the two
/// non-ASCII characters whose case folding is an ASCII letter.
pub(crate) fn word_set(extra: bool) -> CharSet {
    let mut set = CharSet::from_ranges(&[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)]);
    if extra {
        set.add(0x17F);
        set.add(0x212A);
        set.normalize();
    }
    set
}

fn single_char(mut it: impl Iterator<Item = char>) -> Option<char> {
    let first = it.next()?;
    if it.next().is_some() {
        return None;
    }
    Some(first)
}

/// `Canonicalize` without the `u` flag: upper-case mapping of a single code unit, never
/// mapping a non-ASCII character to an ASCII one.
pub(crate) fn canon_legacy(c: u32) -> u32 {
    if c < 128 {
        return if (0x61..=0x7A).contains(&c) { c - 32 } else { c };
    }
    let Some(ch) = char::from_u32(c) else { return c };
    let Some(up) = single_char(ch.to_uppercase()) else { return c };
    let up = up as u32;
    if !(128..=0xFFFF).contains(&up) {
        return c;
    }
    up
}

/// `Canonicalize` with the `u` flag: simple case folding, approximated from the standard
/// library's mappings (upper-case, then lower-case, each only when it is a single character).
pub(crate) fn fold_simple(c: u32) -> u32 {
    if c < 128 {
        return if (0x41..=0x5A).contains(&c) { c + 32 } else { c };
    }
    match c {
        0x131 => return c, // dotless i folds to itself
        0x1FD3 => return 0x390,
        0x1FE3 => return 0x3B0,
        0xFB05 => return 0xFB06,
        _ => {}
    }
    let Some(ch) = char::from_u32(c) else { return c };
    let up = single_char(ch.to_uppercase()).unwrap_or(ch);
    let low = single_char(up.to_lowercase()).unwrap_or(up);
    low as u32
}

#[inline]
pub(crate) fn canonicalize(c: u32, unicode: bool) -> u32 {
    if unicode {
        fold_simple(c)
    } else {
        canon_legacy(c)
    }
}

/// Every character that belongs to a case-equivalence class of two or more members, as
/// `(canonical value, member)` pairs sorted by canonical value.
struct CaseTable {
    pairs: Vec<(u32, u32)>,
}

impl CaseTable {
    fn build(canon: fn(u32) -> u32, limit: u32) -> CaseTable {
        let mut pairs = Vec::new();
        for c in 0..=limit {
            let k = canon(c);
            if k != c {
                pairs.push((k, c));
            }
        }
        pairs.sort_unstable();
        let mut keys: Vec<u32> = pairs.iter().map(|p| p.0).collect();
        keys.dedup();
        for k in keys {
            if canon(k) == k {
                pairs.push((k, k));
            }
        }
        pairs.sort_unstable();
        CaseTable { pairs }
    }

    fn class_of(&self, key: u32) -> &[(u32, u32)] {
        let lo = self.pairs.partition_point(|p| p.0 < key);
        let hi = self.pairs.partition_point(|p| p.0 <= key);
        self.pairs.get(lo..hi).unwrap_or(&[])
    }
}

fn case_table(unicode: bool) -> &'static CaseTable {
    static LEGACY: OnceLock<CaseTable> = OnceLock::new();
    static UNICODE: OnceLock<CaseTable> = OnceLock::new();
    if unicode {
        // No cased characters exist beyond plane 1.
        UNICODE.get_or_init(|| CaseTable::build(fold_simple, 0x1FFFF))
    } else {
        LEGACY.get_or_init(|| CaseTable::build(canon_legacy, 0xFFFF))
    }
}

/// The set of all characters whose canonical form equals that of some member of `set`
/// (`set` must be normalized). This is what a case-insensitive class matches.
pub(crate) fn case_closure(set: &CharSet, unicode: bool) -> CharSet {
    let table = case_table(unicode);
    let mut out = set.clone();
    if set.count() <= 64 {
        for &(lo, hi) in set.ranges() {
            for c in lo..=hi {
                for p in table.class_of(canonicalize(c, unicode)) {
                    out.add(p.1);
                }
            }
        }
    } else {
        let pairs = &table.pairs;
        let mut i = 0;
        while i < pairs.len() {
            let key = pairs[i].0;
            let mut j = i;
            let mut hit = false;
            while j < pairs.len() && pairs[j].0 == key {
                hit = hit || set.contains(pairs[j].1);
                j += 1;
            }
            if hit {
                for p in &pairs[i..j] {
                    out.add(p.1);
                }
            }
            i = j;
        }
    }
    out.normalize();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_canonicalize() {
        assert_eq!(canon_legacy('a' as u32), 'A' as u32);
        assert_eq!(canon_legacy(0xDF), 0xDF); // ß upper-cases to two characters
        assert_eq!(canon_legacy(0x3C2), 0x3A3); // ς → Σ
        assert_eq!(canon_legacy(0x3C3), 0x3A3); // σ → Σ
        assert_eq!(canon_legacy(0x131), 0x131); // ı must not become ASCII I
        assert_eq!(canon_legacy(0x17F), 0x17F); // ſ must not become ASCII S
        assert_eq!(canon_legacy(0x130), 0x130);
        assert_eq!(canon_legacy(0xD83D), 0xD83D);
    }

    #[test]
    fn simple_folding() {
        assert_eq!(fold_simple('A' as u32), 'a' as u32);
        assert_eq!(fold_simple(0x1E9E), 0xDF); // ẞ → ß
        assert_eq!(fold_simple(0xDF), 0xDF);
        assert_eq!(fold_simple(0x3C2), 0x3C3); // ς → σ
        assert_eq!(fold_simple(0x3A3), 0x3C3);
        assert_eq!(fold_simple(0x17F), 's' as u32);
        assert_eq!(fold_simple(0x212A), 'k' as u32);
        assert_eq!(fold_simple(0x130), 0x130);
        assert_eq!(fold_simple(0x131), 0x131);
        assert_eq!(fold_simple(0x10400), 0x10428); // Deseret
    }

    #[test]
    fn closures() {
        let s = case_closure(&CharSet::single('k' as u32), true);
        assert!(s.contains('K' as u32) && s.contains(0x212A) && s.count() == 3);
        let s = case_closure(&CharSet::single('k' as u32), false);
        assert!(s.contains('K' as u32) && !s.contains(0x212A));
        let s = case_closure(&CharSet::from_ranges(&[(0x61, 0x7A), (0x3B1, 0x3C9)]), false);
        assert!(s.contains('Q' as u32) && s.contains(0x3A3) && !s.contains(0x130));
    }
}
