//! Arbitrary-precision integers behind the BigInt type: sign and magnitude with 32-bit limbs,
//! schoolbook multiplication and Knuth division. Pure arithmetic; the runtime wrappers in
//! `builtins/bigint.rs` charge heap and steps before calling the quadratic operations.

use std::cmp::Ordering;

/// `mag` is little-endian without trailing zero limbs; zero is the empty magnitude, never negative.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct BigInt {
    pub neg: bool,
    pub mag: Vec<u32>,
}

pub(crate) fn trim(mut v: Vec<u32>) -> Vec<u32> {
    while v.last() == Some(&0) {
        v.pop();
    }
    v
}

pub(crate) fn cmp_mag(a: &[u32], b: &[u32]) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.iter().rev().cmp(b.iter().rev()))
}

pub(crate) fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let mut out = Vec::with_capacity(long.len() + 1);
    let mut carry = 0u64;
    for (i, &x) in long.iter().enumerate() {
        let t = x as u64 + short.get(i).copied().unwrap_or(0) as u64 + carry;
        out.push(t as u32);
        carry = t >> 32;
    }
    if carry != 0 {
        out.push(carry as u32);
    }
    out
}

/// `a - b` for `a >= b`.
pub(crate) fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len());
    let mut borrow = 0i64;
    for (i, &x) in a.iter().enumerate() {
        let t = x as i64 - b.get(i).copied().unwrap_or(0) as i64 - borrow;
        out.push(t as u32);
        borrow = (t < 0) as i64;
    }
    trim(out)
}

fn mul_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let mut r = vec![0u32; a.len() + b.len()];
    for (i, &x) in a.iter().enumerate() {
        if x == 0 {
            continue;
        }
        let mut carry = 0u64;
        for (j, &y) in b.iter().enumerate() {
            let t = x as u64 * y as u64 + r[i + j] as u64 + carry;
            r[i + j] = t as u32;
            carry = t >> 32;
        }
        r[i + b.len()] = carry as u32;
    }
    trim(r)
}

/// Divides by one limb; returns (quotient, remainder).
pub(crate) fn divrem_small(a: &[u32], d: u32) -> (Vec<u32>, u32) {
    let mut q = vec![0u32; a.len()];
    let mut rem = 0u64;
    for i in (0..a.len()).rev() {
        let cur = (rem << 32) | a[i] as u64;
        q[i] = (cur / d as u64) as u32;
        rem = cur % d as u64;
    }
    (trim(q), rem as u32)
}

/// Magnitude shifted left by `s` bits (`s` < 32) into `len` limbs.
fn shl_small(a: &[u32], s: u32, len: usize) -> Vec<u32> {
    let mut out = vec![0u32; len];
    let mut carry = 0u32;
    for (i, slot) in out.iter_mut().enumerate() {
        let x = a.get(i).copied().unwrap_or(0);
        *slot = if s == 0 { x } else { (x << s) | carry };
        carry = if s == 0 { 0 } else { x >> (32 - s) };
    }
    out
}

/// Knuth's algorithm D: (quotient, remainder) of magnitudes, `b` non-zero.
fn divrem_mag(a: &[u32], b: &[u32]) -> (Vec<u32>, Vec<u32>) {
    if cmp_mag(a, b) == Ordering::Less {
        return (Vec::new(), a.to_vec());
    }
    if b.len() == 1 {
        let (q, r) = divrem_small(a, b[0]);
        return (q, if r == 0 { Vec::new() } else { vec![r] });
    }
    let s = b[b.len() - 1].leading_zeros();
    let n = b.len();
    let b = shl_small(b, s, n);
    let mut a = shl_small(a, s, a.len() + 1);
    let m = a.len() - n;
    let mut q = vec![0u32; m];
    for j in (0..m).rev() {
        let num = ((a[j + n] as u64) << 32) | a[j + n - 1] as u64;
        let mut qhat = num / b[n - 1] as u64;
        let mut rhat = num % b[n - 1] as u64;
        while qhat >= 1 << 32 || qhat * b[n - 2] as u64 > ((rhat << 32) | a[j + n - 2] as u64) {
            qhat -= 1;
            rhat += b[n - 1] as u64;
            if rhat >= 1 << 32 {
                break;
            }
        }
        let (mut borrow, mut carry) = (0i64, 0u64);
        for i in 0..n {
            let p = qhat * b[i] as u64 + carry;
            carry = p >> 32;
            let t = a[i + j] as i64 - borrow - (p & 0xFFFF_FFFF) as i64;
            a[i + j] = t as u32;
            borrow = (t < 0) as i64;
        }
        let t = a[j + n] as i64 - borrow - carry as i64;
        a[j + n] = t as u32;
        if t < 0 {
            // the estimate was one too large: add the divisor back
            qhat -= 1;
            let mut c = 0u64;
            for i in 0..n {
                let s2 = a[i + j] as u64 + b[i] as u64 + c;
                a[i + j] = s2 as u32;
                c = s2 >> 32;
            }
            a[j + n] = (a[j + n] as u64 + c) as u32;
        }
        q[j] = qhat as u32;
    }
    let mut r = vec![0u32; n];
    for i in 0..n {
        let hi = if s == 0 { 0 } else { a.get(i + 1).copied().unwrap_or(0) << (32 - s) };
        r[i] = if s == 0 { a[i] } else { (a[i] >> s) | hi };
    }
    (trim(q), trim(r))
}

impl BigInt {
    pub fn zero() -> BigInt {
        BigInt::default()
    }

    pub fn is_zero(&self) -> bool {
        self.mag.is_empty()
    }

    pub(crate) fn new(neg: bool, mag: Vec<u32>) -> BigInt {
        let mag = trim(mag);
        BigInt { neg: neg && !mag.is_empty(), mag }
    }

    pub fn from_u64(v: u64) -> BigInt {
        BigInt::new(false, vec![v as u32, (v >> 32) as u32])
    }

    pub fn from_i64(v: i64) -> BigInt {
        let m = v.unsigned_abs();
        BigInt::new(v < 0, vec![m as u32, (m >> 32) as u32])
    }

    /// The exact value of an integral, finite `n`; `None` otherwise.
    pub fn from_f64(n: f64) -> Option<BigInt> {
        if !n.is_finite() || n.fract() != 0.0 {
            return None;
        }
        if n == 0.0 {
            return Some(BigInt::zero());
        }
        let bits = n.abs().to_bits();
        let exp = ((bits >> 52) & 0x7FF) as i64 - 1075;
        let mant = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
        let m = BigInt::from_u64(if exp < 0 { mant >> (-exp).min(63) } else { mant });
        let m = if exp > 0 { m.shl(exp as usize) } else { m };
        Some(BigInt::new(n < 0.0, m.mag))
    }

    /// Number of bits in the magnitude.
    pub fn bits(&self) -> usize {
        match self.mag.last() {
            Some(top) => self.mag.len() * 32 - top.leading_zeros() as usize,
            None => 0,
        }
    }

    /// Nearest f64 (ties to even), ±Infinity when out of range.
    pub fn to_f64(&self) -> f64 {
        let bits = self.bits();
        let v = if bits <= 64 {
            (self.mag.first().copied().unwrap_or(0) as u64 | (self.mag.get(1).copied().unwrap_or(0) as u64) << 32) as f64
        } else if bits > 1100 {
            f64::INFINITY
        } else {
            // the top 64 bits, with every lower bit folded into a sticky bit
            let shift = bits - 64;
            let top = self.shr_mag(shift);
            let mut hi = top.first().copied().unwrap_or(0) as u64 | (top.get(1).copied().unwrap_or(0) as u64) << 32;
            let low_limbs = shift / 32;
            let sticky = self.mag.iter().take(low_limbs).any(|l| *l != 0) || (shift % 32 != 0 && self.mag[low_limbs] & ((1u32 << (shift % 32)) - 1) != 0);
            hi |= sticky as u64;
            (hi as f64) * 2f64.powi(shift as i32)
        };
        if self.neg { -v } else { v }
    }

    fn shr_mag(&self, n: usize) -> Vec<u32> {
        let (limbs, s) = (n / 32, (n % 32) as u32);
        let mut out: Vec<u32> = self.mag.iter().skip(limbs).copied().collect();
        if s != 0 {
            for i in 0..out.len() {
                let hi = out.get(i + 1).copied().unwrap_or(0);
                out[i] = (out[i] >> s) | (hi << (32 - s));
            }
        }
        trim(out)
    }

    pub fn shl(&self, n: usize) -> BigInt {
        if self.is_zero() {
            return BigInt::zero();
        }
        let (limbs, s) = (n / 32, (n % 32) as u32);
        let mut mag = vec![0u32; limbs];
        mag.extend(shl_small(&self.mag, s, self.mag.len() + 1));
        BigInt::new(self.neg, mag)
    }

    /// Arithmetic shift right: floor(self / 2^n).
    pub fn shr(&self, n: usize) -> BigInt {
        if !self.neg {
            return BigInt::new(false, self.shr_mag(n));
        }
        // floor for negatives: -(((|x| - 1) >> n) + 1)
        let less = BigInt::new(false, sub_mag(&self.mag, &[1]));
        BigInt::new(true, add_mag(&less.shr_mag(n), &[1]))
    }

    pub fn cmp(&self, o: &BigInt) -> Ordering {
        match (self.neg, o.neg) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => cmp_mag(&self.mag, &o.mag),
            (true, true) => cmp_mag(&o.mag, &self.mag),
        }
    }

    /// Mathematical comparison with a Number; `None` for NaN.
    pub fn cmp_f64(&self, n: f64) -> Option<Ordering> {
        if n.is_nan() {
            return None;
        }
        if n.is_infinite() {
            return Some(if n > 0.0 { Ordering::Less } else { Ordering::Greater });
        }
        let whole = BigInt::from_f64(n.trunc()).unwrap_or_default();
        Some(self.cmp(&whole).then(if n > n.trunc() { Ordering::Less } else if n < n.trunc() { Ordering::Greater } else { Ordering::Equal }))
    }

    pub fn negate(&self) -> BigInt {
        BigInt::new(!self.neg, self.mag.clone())
    }

    pub fn add(&self, o: &BigInt) -> BigInt {
        if self.neg == o.neg {
            return BigInt::new(self.neg, add_mag(&self.mag, &o.mag));
        }
        match cmp_mag(&self.mag, &o.mag) {
            Ordering::Less => BigInt::new(o.neg, sub_mag(&o.mag, &self.mag)),
            _ => BigInt::new(self.neg, sub_mag(&self.mag, &o.mag)),
        }
    }

    pub fn sub(&self, o: &BigInt) -> BigInt {
        self.add(&o.negate())
    }

    pub fn mul(&self, o: &BigInt) -> BigInt {
        BigInt::new(self.neg != o.neg, mul_mag(&self.mag, &o.mag))
    }

    /// Truncating division: (quotient, remainder with the dividend's sign); `None` for a zero divisor.
    pub fn divrem(&self, o: &BigInt) -> Option<(BigInt, BigInt)> {
        if o.is_zero() {
            return None;
        }
        let (q, r) = divrem_mag(&self.mag, &o.mag);
        Some((BigInt::new(self.neg != o.neg, q), BigInt::new(self.neg, r)))
    }

    /// The low 64 bits of the two's-complement representation.
    pub fn to_u64_wrapping(&self) -> u64 {
        let m = self.mag.first().copied().unwrap_or(0) as u64 | (self.mag.get(1).copied().unwrap_or(0) as u64) << 32;
        if self.neg { m.wrapping_neg() } else { m }
    }
}
