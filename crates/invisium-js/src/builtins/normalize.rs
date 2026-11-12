//! Unicode normalization (NFC, NFD, NFKC, NFKD) over the ranges tabulated in
//! `normalize_data.rs`, plus Hangul syllables by algorithm. Characters outside those ranges
//! have no mapping here and pass through unchanged.

use super::normalize_data::{CANON, CCC, COMPAT, COMPAT_DATA};
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq)]
pub enum Form {
    Nfc,
    Nfd,
    Nfkc,
    Nfkd,
}

const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const S_COUNT: u32 = L_COUNT * V_COUNT * T_COUNT;

fn combining_class(cp: u32) -> u8 {
    if !(0x300..=0xFFFF).contains(&cp) {
        return 0;
    }
    let c = cp as u16;
    match CCC.binary_search_by(|&(lo, hi, _)| if c < lo { std::cmp::Ordering::Greater } else if c > hi { std::cmp::Ordering::Less } else { std::cmp::Ordering::Equal }) {
        Ok(i) => CCC[i].2,
        Err(_) => 0,
    }
}

fn decompose(cp: u32, compat: bool, out: &mut Vec<u32>) {
    if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
        let s = cp - S_BASE;
        out.push(L_BASE + s / (V_COUNT * T_COUNT));
        out.push(V_BASE + (s % (V_COUNT * T_COUNT)) / T_COUNT);
        if s % T_COUNT != 0 {
            out.push(T_BASE + s % T_COUNT);
        }
        return;
    }
    if !(0xA0..=0xFFFF).contains(&cp) {
        out.push(cp);
        return;
    }
    if let Ok(i) = CANON.binary_search_by_key(&(cp as u16), |e| e.0) {
        let (_, first, second) = CANON[i];
        decompose((first & 0x7FFF) as u32, compat, out);
        if second != 0 {
            decompose(second as u32, compat, out);
        }
        return;
    }
    if compat {
        if let Ok(i) = COMPAT.binary_search_by_key(&(cp as u16), |e| e.0) {
            let (_, start, len) = COMPAT[i];
            for &u in COMPAT_DATA.iter().skip(start as usize).take(len as usize) {
                decompose(u as u32, compat, out);
            }
            return;
        }
    }
    out.push(cp);
}

/// Recomposing pairs sorted by (first, second): (first, second, composed).
fn pairs() -> &'static [(u16, u16, u16)] {
    static PAIRS: OnceLock<Vec<(u16, u16, u16)>> = OnceLock::new();
    PAIRS.get_or_init(|| {
        let mut v: Vec<(u16, u16, u16)> = CANON.iter().filter(|e| e.1 & 0x8000 == 0 && e.2 != 0).map(|e| (e.1, e.2, e.0)).collect();
        v.sort_unstable();
        v
    })
}

fn compose_pair(a: u32, b: u32) -> Option<u32> {
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        return Some(S_BASE + ((a - L_BASE) * V_COUNT + (b - V_BASE)) * T_COUNT);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&a) && (a - S_BASE) % T_COUNT == 0 && (T_BASE + 1..T_BASE + T_COUNT).contains(&b) {
        return Some(a + (b - T_BASE));
    }
    if a > 0x7FFF || b > 0xFFFF {
        return None;
    }
    let p = pairs();
    p.binary_search_by_key(&(a as u16, b as u16), |e| (e.0, e.1)).ok().map(|i| p[i].2 as u32)
}

fn code_points(units: &[u16]) -> Vec<u32> {
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = units[i] as u32;
        if (0xD800..0xDC00).contains(&u) && i + 1 < units.len() && (0xDC00..0xE000).contains(&(units[i + 1] as u32)) {
            out.push(0x10000 + ((u - 0xD800) << 10) + (units[i + 1] as u32 - 0xDC00));
            i += 2;
        } else {
            out.push(u);
            i += 1;
        }
    }
    out
}

pub fn normalize(units: &[u16], form: Form) -> Vec<u16> {
    let compat = matches!(form, Form::Nfkc | Form::Nfkd);
    let mut cps: Vec<u32> = Vec::with_capacity(units.len() + 8);
    for cp in code_points(units) {
        decompose(cp, compat, &mut cps);
    }
    // canonical ordering: each run of combining marks is sorted by class (stably)
    let mut i = 0;
    while i < cps.len() {
        if combining_class(cps[i]) == 0 {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < cps.len() && combining_class(cps[j]) != 0 {
            j += 1;
        }
        cps[i..j].sort_by_key(|c| combining_class(*c));
        i = j;
    }
    if matches!(form, Form::Nfc | Form::Nfkc) {
        let mut out: Vec<u32> = Vec::with_capacity(cps.len());
        let mut starter: Option<usize> = None;
        let mut last_class = 0u8;
        for &c in &cps {
            let class = combining_class(c);
            if let Some(si) = starter {
                // a mark is blocked from the starter by an earlier mark of the same or a higher class
                let blocked = out.len() - 1 > si && last_class >= class;
                if !blocked {
                    if let Some(p) = compose_pair(out[si], c) {
                        out[si] = p;
                        continue;
                    }
                }
            }
            if class == 0 {
                starter = Some(out.len());
                last_class = 0;
            } else {
                last_class = class;
            }
            out.push(c);
        }
        cps = out;
    }
    let mut res: Vec<u16> = Vec::with_capacity(cps.len());
    for c in cps {
        if c >= 0x10000 {
            let v = c - 0x10000;
            res.push(0xD800 + (v >> 10) as u16);
            res.push(0xDC00 + (v & 0x3FF) as u16);
        } else {
            res.push(c as u16);
        }
    }
    res
}
