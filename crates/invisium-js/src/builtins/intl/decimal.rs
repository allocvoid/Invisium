//! Exact decimal digits for `Intl.NumberFormat`: a magnitude as `0.DIGITS x 10^point`,
//! rounding at a digit position with every ECMA-402 rounding mode, and the integer/fraction
//! strings with padding and en-US grouping. A double contributes its shortest round-trip
//! digits (as ICU does, so `1.005` rounds to `1.01`); a BigInt its exact decimal digits.

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Dec {
    /// Significant digits (0..=9), no leading or trailing zeros; empty means zero.
    pub digits: Vec<u8>,
    pub point: i32,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RMode {
    Ceil,
    Floor,
    Expand,
    Trunc,
    HalfCeil,
    HalfFloor,
    HalfExpand,
    HalfTrunc,
    HalfEven,
}

pub(super) const RMODES: [&str; 9] = ["ceil", "floor", "expand", "trunc", "halfCeil", "halfFloor", "halfExpand", "halfTrunc", "halfEven"];

impl RMode {
    pub fn from_index(i: u8) -> RMode {
        [RMode::Ceil, RMode::Floor, RMode::Expand, RMode::Trunc, RMode::HalfCeil, RMode::HalfFloor, RMode::HalfExpand, RMode::HalfTrunc, RMode::HalfEven][i as usize % 9]
    }
    pub fn name(self) -> &'static str {
        RMODES[self as usize]
    }
}

impl Dec {
    pub fn zero() -> Dec {
        Dec { digits: Vec::new(), point: 0 }
    }

    /// The magnitude of a finite double.
    pub fn from_f64(x: f64) -> Dec {
        if x == 0.0 || !x.is_finite() {
            return Dec::zero();
        }
        let s = format!("{:e}", x.abs());
        let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let digits: Vec<u8> = mant.bytes().filter(|b| b.is_ascii_digit()).map(|b| b - b'0').collect();
        Dec::normalized(digits, exp.parse::<i32>().unwrap_or(0) + 1)
    }

    /// The magnitude of a string of decimal digits (an integer).
    pub fn from_int_digits(s: &str) -> Dec {
        let digits: Vec<u8> = s.bytes().filter(|b| b.is_ascii_digit()).map(|b| b - b'0').collect();
        let n = digits.len() as i32;
        Dec::normalized(digits, n)
    }

    fn normalized(mut digits: Vec<u8>, mut point: i32) -> Dec {
        let lead = digits.iter().take_while(|d| **d == 0).count();
        digits.drain(..lead);
        point -= lead as i32;
        while digits.last() == Some(&0) {
            digits.pop();
        }
        if digits.is_empty() {
            return Dec::zero();
        }
        Dec { digits, point }
    }

    pub fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    /// Multiplies by `10^n`.
    pub fn shift(&mut self, n: i32) {
        if !self.is_zero() {
            self.point = self.point.saturating_add(n);
        }
    }

    /// The power of ten of the leading digit (0 for zero).
    pub fn magnitude(&self) -> i32 {
        if self.is_zero() { 0 } else { self.point - 1 }
    }

    /// Rounds so that the last kept digit has magnitude `min_mag` (keeps `10^min_mag` and up).
    pub fn round_at(&self, min_mag: i32, neg: bool, mode: RMode) -> Dec {
        if self.is_zero() {
            return Dec::zero();
        }
        let keep = self.point as i64 - min_mag as i64;
        if keep >= self.digits.len() as i64 {
            return self.clone();
        }
        // how the discarded tail compares with half a unit in the last kept place
        let (half, nonzero) = if keep < 0 {
            (std::cmp::Ordering::Less, true)
        } else {
            let tail = &self.digits[keep as usize..];
            let first = tail[0];
            let rest = tail[1..].iter().any(|d| *d != 0);
            let half = if first > 5 || (first == 5 && rest) {
                std::cmp::Ordering::Greater
            } else if first == 5 {
                std::cmp::Ordering::Equal
            } else {
                std::cmp::Ordering::Less
            };
            (half, true)
        };
        use std::cmp::Ordering::*;
        let kept: Vec<u8> = if keep > 0 { self.digits[..keep as usize].to_vec() } else { Vec::new() };
        let odd = kept.last().map(|d| d % 2 == 1).unwrap_or(false);
        let up = match mode {
            RMode::Ceil => !neg && nonzero,
            RMode::Floor => neg && nonzero,
            RMode::Expand => nonzero,
            RMode::Trunc => false,
            RMode::HalfCeil => half == Greater || (half == Equal && !neg),
            RMode::HalfFloor => half == Greater || (half == Equal && neg),
            RMode::HalfExpand => half != Less,
            RMode::HalfTrunc => half == Greater,
            RMode::HalfEven => half == Greater || (half == Equal && odd),
        };
        if !up {
            return Dec::normalized(kept, self.point);
        }
        if keep <= 0 {
            return Dec { digits: vec![1], point: min_mag.saturating_add(1) };
        }
        let mut kept = kept;
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, 1);
                return Dec::normalized(kept, self.point + 1);
            }
            i -= 1;
            if kept[i] == 9 {
                kept[i] = 0;
            } else {
                kept[i] += 1;
                return Dec::normalized(kept, self.point);
            }
        }
    }

    /// Rounds to `sig` significant digits.
    pub fn round_sig(&self, sig: u32, neg: bool, mode: RMode) -> Dec {
        self.round_at(self.point - sig as i32, neg, mode)
    }

    /// Integer and fraction digit strings with at least `min_int` integer and `min_frac`
    /// fraction digits.
    pub fn split(&self, min_int: usize, min_frac: usize) -> (String, String) {
        let mut int = String::new();
        let mut frac = String::new();
        if !self.is_zero() {
            let p = self.point;
            if p > 0 {
                for i in 0..p as usize {
                    int.push(char::from(b'0' + self.digits.get(i).copied().unwrap_or(0)));
                }
                for d in self.digits.iter().skip(p as usize) {
                    frac.push(char::from(b'0' + d));
                }
            } else {
                frac.extend(std::iter::repeat_n('0', (-p) as usize));
                frac.extend(self.digits.iter().map(|d| char::from(b'0' + d)));
            }
        }
        if int.len() < min_int.max(1) {
            int = format!("{}{}", "0".repeat(min_int.max(1) - int.len()), int);
        }
        while frac.len() < min_frac {
            frac.push('0');
        }
        (int, frac)
    }

    /// Fraction digits needed to show `min_sig` significant digits.
    pub fn frac_for_sig(&self, min_sig: u32) -> usize {
        if self.is_zero() {
            min_sig.saturating_sub(1) as usize
        } else {
            (min_sig as i64 - self.point as i64).max(0) as usize
        }
    }
}

/// Splits an integer digit string into en-US groups of three.
pub(super) fn groups(int: &str) -> Vec<&str> {
    let first = match int.len() % 3 {
        0 => 3,
        n => n,
    };
    let mut out = vec![&int[..first.min(int.len())]];
    let mut i = first;
    while i < int.len() {
        out.push(&int[i..i + 3]);
        i += 3;
    }
    out
}
