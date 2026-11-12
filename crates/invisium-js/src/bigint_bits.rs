//! BigInt: two's-complement bitwise operations, `asIntN` / `asUintN`, and conversion to and
//! from text.

use crate::bigint::{divrem_small, sub_mag, trim, BigInt};

#[derive(Clone, Copy, PartialEq)]
pub enum BitOp {
    And,
    Or,
    Xor,
}

/// `len` limbs of the infinite two's-complement representation.
fn twos(x: &BigInt, len: usize) -> Vec<u32> {
    let mut out: Vec<u32> = (0..len).map(|i| x.mag.get(i).copied().unwrap_or(0)).collect();
    if x.neg {
        let mut carry = true;
        for l in out.iter_mut() {
            let (v, c) = (!*l).overflowing_add(carry as u32);
            *l = v;
            carry = c;
        }
    }
    out
}

/// Reads limbs as a two's-complement number whose sign is its top bit.
fn from_twos(mut limbs: Vec<u32>) -> BigInt {
    let neg = limbs.last().is_some_and(|l| l >> 31 == 1);
    if neg {
        let mut carry = true;
        for l in limbs.iter_mut() {
            let (v, c) = (!*l).overflowing_add(carry as u32);
            *l = v;
            carry = c;
        }
    }
    BigInt::new(neg, limbs)
}

impl BigInt {
    pub fn bitop(&self, o: &BigInt, op: BitOp) -> BigInt {
        // one limb more than either operand, so the top limb is pure sign extension
        let len = self.mag.len().max(o.mag.len()) + 1;
        let (a, b) = (twos(self, len), twos(o, len));
        let out = a
            .iter()
            .zip(&b)
            .map(|(x, y)| match op {
                BitOp::And => x & y,
                BitOp::Or => x | y,
                BitOp::Xor => x ^ y,
            })
            .collect();
        from_twos(out)
    }

    /// `~x` = -x - 1.
    pub fn bitnot(&self) -> BigInt {
        self.negate().sub(&BigInt::from_u64(1))
    }

    /// The value modulo 2^bits, in 0 .. 2^bits.
    pub fn as_uint_n(&self, bits: usize) -> BigInt {
        if bits == 0 || self.is_zero() {
            return BigInt::zero();
        }
        if !self.neg && self.bits() <= bits {
            return self.clone();
        }
        let len = bits.div_ceil(32);
        let mut low = twos(self, len);
        if bits % 32 != 0 {
            if let Some(top) = low.last_mut() {
                *top &= (1u32 << (bits % 32)) - 1;
            }
        }
        BigInt::new(false, low)
    }

    /// The value wrapped into -2^(bits-1) .. 2^(bits-1).
    pub fn as_int_n(&self, bits: usize) -> BigInt {
        if bits == 0 {
            return BigInt::zero();
        }
        let u = self.as_uint_n(bits);
        if u.bits() < bits {
            return u; // the sign bit is clear
        }
        // u - 2^bits
        let mut pow = vec![0u32; bits / 32 + 1];
        pow[bits / 32] = 1 << (bits % 32);
        BigInt::new(true, sub_mag(&trim(pow), &u.mag))
    }

    /// Digits in `radix` (2..=36), lowercase, with a leading `-` for negatives.
    pub fn to_string_radix(&self, radix: u32) -> String {
        if self.is_zero() {
            return "0".to_string();
        }
        // peel off the largest power of the radix that fits a limb
        let (mut chunk, mut digits) = (radix, 1usize);
        while (chunk as u64) * (radix as u64) <= u32::MAX as u64 {
            chunk *= radix;
            digits += 1;
        }
        let mut parts: Vec<u32> = Vec::new();
        let mut cur = self.mag.clone();
        while !cur.is_empty() {
            let (q, r) = divrem_small(&cur, chunk);
            parts.push(r);
            cur = q;
        }
        let mut out = String::with_capacity(parts.len() * digits + 1);
        if self.neg {
            out.push('-');
        }
        for (i, p) in parts.iter().rev().enumerate() {
            let mut buf = Vec::with_capacity(digits);
            let mut v = *p;
            while v > 0 {
                buf.push(char::from_digit(v % radix, radix).unwrap_or('0'));
                v /= radix;
            }
            if i > 0 {
                out.extend(std::iter::repeat('0').take(digits - buf.len()));
            }
            out.extend(buf.iter().rev());
        }
        out
    }

    /// Parses digits of `radix` (no sign, no separators); `None` when empty or malformed.
    pub fn parse_radix(digits: &str, radix: u32) -> Option<BigInt> {
        if digits.is_empty() {
            return None;
        }
        let (mut chunk, mut per) = (radix, 1usize);
        while (chunk as u64) * (radix as u64) <= u32::MAX as u64 {
            chunk *= radix;
            per += 1;
        }
        let bytes = digits.as_bytes();
        let mut mag: Vec<u32> = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let end = (i + if i == 0 && bytes.len() % per != 0 { bytes.len() % per } else { per }).min(bytes.len());
            let (mut v, mut mul) = (0u32, 1u32);
            for &b in &bytes[i..end] {
                v = v * radix + (b as char).to_digit(radix)?;
                mul *= radix;
            }
            // mag = mag * mul + v
            let mut carry = v as u64;
            for l in mag.iter_mut() {
                let t = *l as u64 * mul as u64 + carry;
                *l = t as u32;
                carry = t >> 32;
            }
            if carry != 0 {
                mag.push(carry as u32);
            }
            i = end;
        }
        Some(BigInt::new(false, mag))
    }

    /// StringToBigInt: optional whitespace, then a decimal integer with an optional sign, or
    /// an unsigned `0x` / `0o` / `0b` literal; the empty string is 0.
    pub fn parse_js(text: &str) -> Option<BigInt> {
        let t = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
        if t.is_empty() {
            return Some(BigInt::zero());
        }
        let lower = t.get(..2).map(|p| p.to_ascii_lowercase());
        match lower.as_deref() {
            Some("0x") => return BigInt::parse_radix(&t[2..], 16),
            Some("0o") => return BigInt::parse_radix(&t[2..], 8),
            Some("0b") => return BigInt::parse_radix(&t[2..], 2),
            _ => {}
        }
        let (neg, body) = match t.as_bytes()[0] {
            b'-' => (true, &t[1..]),
            b'+' => (false, &t[1..]),
            _ => (false, t),
        };
        let v = BigInt::parse_radix(body, 10)?;
        Some(BigInt::new(neg, v.mag))
    }
}
