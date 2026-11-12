//! `\p{…}` property sets. General_Category comes from the generated table in `gc_table.rs`
//! and the two Pattern properties are exact; everything marked `// approximate` is built from
//! block ranges, from the categories, or from the standard library's `char` predicates.

use crate::charset::{CharSet, MAX_CODE_POINT};
use crate::gc_table::{CATEGORY_NAMES, GC_TABLE};
use crate::script::{script, Ranges};
use std::sync::OnceLock;

/// The set for the text between the braces of `\p{…}`, or `None` if it is not supported.
pub(crate) fn lookup(spec: &str) -> Option<CharSet> {
    match spec.split_once('=') {
        Some((name, value)) => match name {
            "General_Category" | "gc" => gc_mask(value).map(from_table),
            "Script" | "sc" | "Script_Extensions" | "scx" => script(value).map(CharSet::from_ranges),
            _ => None,
        },
        None => gc_mask(spec).map(from_table).or_else(|| binary_property(spec)),
    }
}

/// Long names and other aliases of the General_Category values, with their short names.
const GC_ALIASES: &[(&str, &str)] = &[
    ("Letter", "L"), ("Cased_Letter", "LC"), ("Uppercase_Letter", "Lu"),
    ("Lowercase_Letter", "Ll"), ("Titlecase_Letter", "Lt"), ("Modifier_Letter", "Lm"),
    ("Other_Letter", "Lo"), ("Mark", "M"), ("Combining_Mark", "M"), ("Nonspacing_Mark", "Mn"),
    ("Spacing_Mark", "Mc"), ("Enclosing_Mark", "Me"), ("Number", "N"), ("Decimal_Number", "Nd"),
    ("digit", "Nd"), ("Letter_Number", "Nl"), ("Other_Number", "No"), ("Punctuation", "P"),
    ("punct", "P"), ("Connector_Punctuation", "Pc"), ("Dash_Punctuation", "Pd"),
    ("Open_Punctuation", "Ps"), ("Close_Punctuation", "Pe"), ("Initial_Punctuation", "Pi"),
    ("Final_Punctuation", "Pf"), ("Other_Punctuation", "Po"), ("Symbol", "S"),
    ("Math_Symbol", "Sm"), ("Currency_Symbol", "Sc"), ("Modifier_Symbol", "Sk"),
    ("Other_Symbol", "So"), ("Separator", "Z"), ("Space_Separator", "Zs"),
    ("Line_Separator", "Zl"), ("Paragraph_Separator", "Zp"), ("Other", "C"), ("Control", "Cc"),
    ("cntrl", "Cc"), ("Format", "Cf"), ("Surrogate", "Cs"), ("Private_Use", "Co"),
    ("Unassigned", "Cn"),
];

/// The leaf categories a General_Category value stands for; bit `i` is `CATEGORY_NAMES[i]`.
fn gc_mask(name: &str) -> Option<u32> {
    let short = GC_ALIASES.iter().find(|(alias, _)| *alias == name).map_or(name, |(_, s)| *s);
    let leaf = |n: &str| CATEGORY_NAMES.iter().position(|&c| c == n).map(|i| 1u32 << i);
    if short == "LC" {
        return Some(leaf("Lu")? | leaf("Ll")? | leaf("Lt")?);
    }
    if let Some(bit) = leaf(short) {
        return Some(bit);
    }
    if short.len() == 1 && "LMNPSZC".contains(short) {
        let group = CATEGORY_NAMES.iter().enumerate().filter(|(_, c)| c.starts_with(short));
        return Some(group.fold(0, |mask, (i, _)| mask | 1 << i));
    }
    None
}

/// Every code point whose category is in `mask`: one pass over the table.
fn from_table(mask: u32) -> CharSet {
    let mut set = CharSet::new();
    let ends = GC_TABLE.iter().skip(1).map(|&e| (e >> 8).saturating_sub(1));
    for (&entry, end) in GC_TABLE.iter().zip(ends.chain(std::iter::once(MAX_CODE_POINT))) {
        if mask & 1u32.checked_shl(entry & 0xFF).unwrap_or(0) != 0 {
            set.add_range(entry >> 8, end);
        }
    }
    set.normalize();
    set
}

/// The union of the named General_Category values.
fn categories(names: &[&str]) -> CharSet {
    from_table(names.iter().filter_map(|n| gc_mask(n)).fold(0, |a, b| a | b))
}

fn union(parts: &[Ranges]) -> CharSet {
    let mut set = CharSet::new();
    for part in parts {
        for &(lo, hi) in *part {
            set.add_range(lo, hi);
        }
    }
    set.normalize();
    set
}

fn binary_property(name: &str) -> Option<CharSet> {
    Some(match name {
        "Alphabetic" | "Alpha" => cached(Pred::Alphabetic),
        "White_Space" | "space" => cached(Pred::WhiteSpace),
        "Uppercase" | "Upper" => cached(Pred::Uppercase),
        "Lowercase" | "Lower" => cached(Pred::Lowercase),
        "ASCII" => CharSet::from_ranges(&[(0, 0x7F)]),
        "Any" => CharSet::from_ranges(&[(0, MAX_CODE_POINT)]),
        "Assigned" => categories(&["Cn"]).negated(),
        "Pattern_Syntax" | "Pat_Syn" => union(&[PATTERN_SYNTAX]),
        "Pattern_White_Space" | "Pat_WS" => union(&[PATTERN_WHITE_SPACE]),
        // approximate: the XID forms are the same sets as the ID forms.
        "ID_Start" | "IDS" | "XID_Start" | "XIDS" => identifier(false),
        "ID_Continue" | "IDC" | "XID_Continue" | "XIDC" => identifier(true),
        "ASCII_Hex_Digit" | "AHex" => union(&[ASCII_HEX]),
        "Hex_Digit" | "Hex" => union(&[ASCII_HEX, FULLWIDTH_HEX]),
        "Dash" => categories(&["Pd"]),   // approximate
        "Cased" => categories(&["LC"]),  // approximate
        "Ideographic" | "Ideo" => union(&[IDEOGRAPHIC]),
        "Noncharacter_Code_Point" | "NChar" => noncharacters(),
        "Join_Control" | "Join_C" => union(&[&[(0x200C, 0x200D)]]),
        "Variation_Selector" | "VS" => union(&[VARIATION_SELECTOR]),
        "Regional_Indicator" | "RI" => union(&[REGIONAL_INDICATOR]),
        "Emoji" => union(&[KEYCAP_BASES, PICTOGRAPHS]),
        "Emoji_Presentation" | "EPres" => union(&[EMOJI_PRESENTATION]),
        "Emoji_Modifier" | "EMod" => union(&[EMOJI_MODIFIER]),
        "Emoji_Modifier_Base" | "EBase" => union(&[EMOJI_MODIFIER_BASE]),
        "Emoji_Component" | "EComp" => union(&[KEYCAP_BASES, REGIONAL_INDICATOR, EMOJI_MODIFIER, EMOJI_COMPONENT]),
        "Extended_Pictographic" | "ExtPict" => {
            let pictographs = union(&[PICTOGRAPHS, &[(0x1FC00, 0x1FFFD)]]);
            pictographs.difference(&union(&[REGIONAL_INDICATOR, EMOJI_MODIFIER]))
        }
        _ => return None,
    })
}

/// ID_Start (or ID_Continue): letters, letter numbers and Other_ID_Start (plus marks, digits,
/// connector punctuation and Other_ID_Continue), minus both Pattern properties.
fn identifier(continues: bool) -> CharSet {
    let names: &[&str] = if continues { &["L", "Nl", "Mn", "Mc", "Nd", "Pc"] } else { &["L", "Nl"] };
    let mut set = categories(names);
    set.add_set(&union(&[OTHER_ID_START]));
    if continues {
        set.add_set(&union(&[OTHER_ID_CONTINUE]));
    }
    set.normalize();
    set.difference(&union(&[PATTERN_SYNTAX, PATTERN_WHITE_SPACE]))
}

/// U+FDD0–U+FDEF and the last two code points of every plane.
fn noncharacters() -> CharSet {
    let mut set = CharSet::new();
    set.add_range(0xFDD0, 0xFDEF);
    for plane in 0..=0x10u32 {
        set.add_range(plane << 16 | 0xFFFE, plane << 16 | 0xFFFF);
    }
    set.normalize();
    set
}

const PATTERN_WHITE_SPACE: Ranges = &[(0x09, 0x0D), (0x20, 0x20), (0x85, 0x85), (0x200E, 0x200F), (0x2028, 0x2029)];

const PATTERN_SYNTAX: Ranges = &[
    (0x21, 0x2F), (0x3A, 0x40), (0x5B, 0x5E), (0x60, 0x60), (0x7B, 0x7E), (0xA1, 0xA7),
    (0xA9, 0xA9), (0xAB, 0xAC), (0xAE, 0xAE), (0xB0, 0xB1), (0xB6, 0xB6), (0xBB, 0xBB),
    (0xBF, 0xBF), (0xD7, 0xD7), (0xF7, 0xF7), (0x2010, 0x2027), (0x2030, 0x203E),
    (0x2041, 0x2053), (0x2055, 0x205E), (0x2190, 0x245F), (0x2500, 0x2775), (0x2794, 0x2BFF),
    (0x2E00, 0x2E7F), (0x3001, 0x3003), (0x3008, 0x3020), (0x3030, 0x3030), (0xFD3E, 0xFD3F),
    (0xFE45, 0xFE46),
];

const OTHER_ID_START: Ranges = &[(0x1885, 0x1886), (0x2118, 0x2118), (0x212E, 0x212E), (0x309B, 0x309C)];
const OTHER_ID_CONTINUE: Ranges = &[(0xB7, 0xB7), (0x387, 0x387), (0x1369, 0x1371), (0x19DA, 0x19DA)];

const ASCII_HEX: Ranges = &[(0x30, 0x39), (0x41, 0x46), (0x61, 0x66)];
const FULLWIDTH_HEX: Ranges = &[(0xFF10, 0xFF19), (0xFF21, 0xFF26), (0xFF41, 0xFF46)];

// approximate: the CJK ideograph blocks and the ideographic iteration marks and numbers.
const IDEOGRAPHIC: Ranges = &[
    (0x3006, 0x3007), (0x3021, 0x3029), (0x3038, 0x303A), (0x3400, 0x4DBF), (0x4E00, 0x9FFF),
    (0xF900, 0xFAFF), (0x20000, 0x323AF),
];

const VARIATION_SELECTOR: Ranges = &[(0x180B, 0x180D), (0x180F, 0x180F), (0xFE00, 0xFE0F), (0xE0100, 0xE01EF)];
const REGIONAL_INDICATOR: Ranges = &[(0x1F1E6, 0x1F1FF)];
const EMOJI_MODIFIER: Ranges = &[(0x1F3FB, 0x1F3FF)];
const KEYCAP_BASES: Ranges = &[(0x23, 0x23), (0x2A, 0x2A), (0x30, 0x39)];

// approximate. Pictographs are taken from these blocks: Latin-1 Supplement (© ®), General
// Punctuation (‼ ⁉), Letterlike Symbols (™ ℹ), Arrows, Miscellaneous Technical, Enclosed
// Alphanumerics (Ⓜ), Geometric Shapes, Miscellaneous Symbols and Dingbats (2600–27BF whole),
// Supplemental Arrows-B, Miscellaneous Symbols and Arrows, CJK Symbols (〰 〽), Enclosed CJK
// (㊗ ㊙), and every supplementary pictograph block from Mahjong Tiles to Symbols and
// Pictographs Extended-A (1F000–1FAFF whole).
const PICTOGRAPHS: Ranges = &[
    (0xA9, 0xA9), (0xAE, 0xAE), (0x203C, 0x203C), (0x2049, 0x2049), (0x2122, 0x2122),
    (0x2139, 0x2139), (0x2194, 0x2199), (0x21A9, 0x21AA), (0x231A, 0x231B), (0x2328, 0x2328),
    (0x23CF, 0x23CF), (0x23E9, 0x23F3), (0x23F8, 0x23FA), (0x24C2, 0x24C2), (0x25AA, 0x25AB),
    (0x25B6, 0x25B6), (0x25C0, 0x25C0), (0x25FB, 0x25FE), (0x2600, 0x27BF), (0x2934, 0x2935),
    (0x2B05, 0x2B07), (0x2B1B, 0x2B1C), (0x2B50, 0x2B50), (0x2B55, 0x2B55), (0x3030, 0x3030),
    (0x303D, 0x303D), (0x3297, 0x3297), (0x3299, 0x3299), (0x1F000, 0x1FAFF),
];

// approximate: a few wide-by-default symbols plus the emoticon, transport and pictograph blocks.
const EMOJI_PRESENTATION: Ranges = &[
    (0x231A, 0x231B), (0x23E9, 0x23EC), (0x23F0, 0x23F0), (0x23F3, 0x23F3), (0x25FD, 0x25FE),
    (0x2614, 0x2615), (0x2648, 0x2653), (0x267F, 0x267F), (0x2693, 0x2693), (0x26A1, 0x26A1),
    (0x26AA, 0x26AB), (0x26BD, 0x26BE), (0x26C4, 0x26C5), (0x26CE, 0x26CE), (0x26D4, 0x26D4),
    (0x26EA, 0x26EA), (0x26F2, 0x26F3), (0x26F5, 0x26F5), (0x26FA, 0x26FA), (0x26FD, 0x26FD),
    (0x2705, 0x2705), (0x270A, 0x270B), (0x2728, 0x2728), (0x274C, 0x274C), (0x274E, 0x274E),
    (0x2753, 0x2755), (0x2757, 0x2757), (0x2795, 0x2797), (0x27B0, 0x27B0), (0x27BF, 0x27BF),
    (0x2B1B, 0x2B1C), (0x2B50, 0x2B50), (0x2B55, 0x2B55), (0x1F1E6, 0x1F1FF), (0x1F300, 0x1F64F),
    (0x1F680, 0x1F6FF), (0x1F900, 0x1F9FF), (0x1FA70, 0x1FAFF),
];

// approximate: hands, body parts and people.
const EMOJI_MODIFIER_BASE: Ranges = &[
    (0x261D, 0x261D), (0x26F9, 0x26F9), (0x270A, 0x270D), (0x1F385, 0x1F385), (0x1F442, 0x1F450),
    (0x1F466, 0x1F478), (0x1F481, 0x1F487), (0x1F4AA, 0x1F4AA), (0x1F645, 0x1F64F),
    (0x1F6B4, 0x1F6B6), (0x1F918, 0x1F91F), (0x1F926, 0x1F926), (0x1F930, 0x1F939),
    (0x1F9D1, 0x1F9DD), (0x1FAF0, 0x1FAF8),
];

// approximate: ZWJ, the keycap mark, VS16, hair components and the tag characters.
const EMOJI_COMPONENT: Ranges = &[
    (0x200D, 0x200D), (0x20E3, 0x20E3), (0xFE0F, 0xFE0F), (0x1F9B0, 0x1F9B3), (0xE0020, 0xE007F),
];

#[derive(Clone, Copy)]
enum Pred {
    Alphabetic,
    WhiteSpace,
    Uppercase,
    Lowercase,
}

/// Predicate-based sets need a scan of the whole code space, so each is built once.
// approximate: these follow the standard library's Unicode version, not the table's.
fn cached(pred: Pred) -> CharSet {
    static CACHE: [OnceLock<CharSet>; 4] = [OnceLock::new(), OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let test: fn(char) -> bool = match pred {
        Pred::Alphabetic => char::is_alphabetic,
        Pred::WhiteSpace => char::is_whitespace,
        Pred::Uppercase => char::is_uppercase,
        Pred::Lowercase => char::is_lowercase,
    };
    match CACHE.get(pred as usize) {
        Some(cell) => cell.get_or_init(|| from_predicate(test)).clone(),
        None => from_predicate(test),
    }
}

fn from_predicate(test: fn(char) -> bool) -> CharSet {
    let mut set = CharSet::new();
    let mut start: Option<u32> = None;
    for c in 0..=MAX_CODE_POINT {
        let hit = char::from_u32(c).is_some_and(test);
        match (hit, start) {
            (true, None) => start = Some(c),
            (false, Some(s)) => {
                set.add_range(s, c - 1);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        set.add_range(s, MAX_CODE_POINT);
    }
    set
}
