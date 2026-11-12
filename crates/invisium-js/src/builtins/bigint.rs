//! The BigInt type inside the runtime: allocation with a size cap, operators with step
//! charging (multiplication, division and radix conversion are quadratic), conversions, and
//! the `BigInt` function with its prototype.

use super::arg;
use crate::bigint::BigInt;
use crate::bigint_bits::BitOp;
use crate::bytecode::Op;
use crate::ops::Hint;
use crate::runtime::*;
use crate::value::*;
use std::rc::Rc;

/// Largest magnitude a BigInt may have, in bits (larger results are a RangeError).
pub const MAX_BIGINT_BITS: usize = 1 << 22;

impl Runtime {
    /// Turns a computed integer into a value, enforcing the size cap and the heap cap.
    pub(crate) fn big_value(&mut self, b: BigInt) -> JsResult<Value> {
        if b.bits() > MAX_BIGINT_BITS {
            return Err(self.range_error("Maximum BigInt size exceeded"));
        }
        if b.mag.len() > 256 {
            self.check_alloc(b.mag.len() * 4).map_err(|t| self.thrown_to_exc(t))?;
        }
        Ok(self.alloc_big(b))
    }

    /// Charges `limb_ops` elementary limb operations to the step budget. Every BigInt
    /// operation calls this before working: the linear ones (add, subtract, bitwise, shift,
    /// negate, compare, wrap) with the larger operand's limb count, the quadratic ones with
    /// the product.
    pub(crate) fn big_charge(&mut self, limb_ops: usize) -> JsResult<()> {
        self.steps = self.steps.saturating_add(limb_ops as u64 / 8);
        if self.steps > self.limits.step_budget {
            return Err(Exc::Abort(ErrorKind::StepBudget));
        }
        Ok(())
    }

    fn big_mul(&mut self, a: &BigInt, b: &BigInt) -> JsResult<BigInt> {
        if a.bits() + b.bits() > MAX_BIGINT_BITS + 1 {
            return Err(self.range_error("Maximum BigInt size exceeded"));
        }
        self.big_charge(a.mag.len() * b.mag.len())?;
        Ok(a.mul(b))
    }

    fn big_pow(&mut self, base: &BigInt, exp: &BigInt) -> JsResult<BigInt> {
        if exp.neg {
            return Err(self.range_error("Exponent must be non-negative"));
        }
        if exp.is_zero() {
            return Ok(BigInt::from_u64(1));
        }
        if base.is_zero() || base.mag == [1] {
            // 0, 1 and -1 never grow
            let odd = exp.mag[0] & 1 == 1;
            return Ok(BigInt::new(base.neg && odd, base.mag.clone()));
        }
        let e = if exp.bits() <= 32 { exp.mag[0] as usize } else { usize::MAX };
        if (base.bits() - 1).saturating_mul(e) > MAX_BIGINT_BITS {
            return Err(self.range_error("Maximum BigInt size exceeded"));
        }
        let (mut result, mut square, mut left) = (BigInt::from_u64(1), base.clone(), e);
        while left > 0 {
            if left & 1 == 1 {
                result = self.big_mul(&result, &square)?;
            }
            left >>= 1;
            if left > 0 {
                square = self.big_mul(&square, &square)?;
            }
        }
        Ok(result)
    }

    fn big_shift(&mut self, a: &BigInt, by: &BigInt, left: bool) -> JsResult<BigInt> {
        let left = left != by.neg;
        let n = if by.bits() <= 31 { by.mag.first().copied().unwrap_or(0) as usize } else { usize::MAX };
        if !left {
            // everything shifted out: 0, or -1 for a negative value
            return Ok(if n >= a.bits() { if a.neg { BigInt::from_i64(-1) } else { BigInt::zero() } } else { a.shr(n) });
        }
        if a.is_zero() {
            return Ok(BigInt::zero());
        }
        if n > MAX_BIGINT_BITS || a.bits() + n > MAX_BIGINT_BITS {
            return Err(self.range_error("Maximum BigInt size exceeded"));
        }
        // the result has n / 32 limbs more than the operand already charged for
        self.big_charge(n / 32)?;
        Ok(a.shl(n))
    }

    /// A binary operator on two BigInts.
    pub(crate) fn big_binary(&mut self, op: Op, a: &BigInt, b: &BigInt) -> JsResult<Value> {
        self.big_charge(a.mag.len().max(b.mag.len()))?;
        let r = match op {
            Op::Add => a.add(b),
            Op::Sub => a.sub(b),
            Op::Mul => self.big_mul(a, b)?,
            Op::Div | Op::Mod => {
                if b.is_zero() {
                    return Err(self.range_error("Division by zero"));
                }
                self.big_charge((a.mag.len().saturating_sub(b.mag.len()) + 1) * b.mag.len())?;
                let (q, r) = a.divrem(b).unwrap_or_default();
                if op == Op::Div { q } else { r }
            }
            Op::Exp => self.big_pow(a, b)?,
            Op::Shl => self.big_shift(a, b, true)?,
            Op::Shr => self.big_shift(a, b, false)?,
            Op::BitAnd => a.bitop(b, BitOp::And),
            Op::BitOr => a.bitop(b, BitOp::Or),
            Op::BitXor => a.bitop(b, BitOp::Xor),
            _ => return Err(self.type_error("BigInts have no unsigned right shift, use >> instead")),
        };
        self.big_value(r)
    }

    pub(crate) fn big_to_string(&mut self, b: &BigInt, radix: u32) -> JsResult<String> {
        self.big_charge(b.mag.len() * b.mag.len())?;
        Ok(b.to_string_radix(radix))
    }

    /// StringToBigInt with the conversion charged; `None` when the text is not an integer.
    pub(crate) fn big_parse(&mut self, text: &str) -> JsResult<Option<BigInt>> {
        if text.len() > MAX_BIGINT_BITS {
            return Err(self.range_error("Maximum BigInt size exceeded"));
        }
        self.big_charge(text.len() * text.len() / 64)?;
        Ok(BigInt::parse_js(text))
    }

    /// ToBigInt: BigInts, booleans and integer strings convert; Numbers do not.
    pub(crate) fn to_bigint(&mut self, v: Value) -> JsResult<Rc<BigInt>> {
        let p = self.to_primitive(v, Hint::Number)?;
        match p {
            Value::BigInt(b) => Ok(self.big(b)),
            Value::Bool(b) => Ok(Rc::new(BigInt::from_u64(b as u64))),
            Value::String(s) => {
                let text = self.rust_string(s);
                match self.big_parse(&text)? {
                    Some(b) => Ok(Rc::new(b)),
                    None => {
                        let shown: String = text.chars().take(40).collect();
                        Err(Exc::Throw(self.make_error("SyntaxError", &format!("Cannot convert {} to a BigInt", shown))))
                    }
                }
            }
            other => {
                let d = self.describe(other);
                Err(self.type_error(&format!("Cannot convert {} to a BigInt", d)))
            }
        }
    }
}

fn bigint_fn(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    if rt.native_new_target != Value::Undefined {
        return Err(rt.type_error("BigInt is not a constructor"));
    }
    let p = rt.to_primitive(arg(a, 0), Hint::Number)?;
    if let Value::Number(n) = p {
        return match BigInt::from_f64(n) {
            Some(b) => rt.big_value(b),
            None => {
                let shown = crate::numfmt::number_to_string(n);
                Err(rt.range_error(&format!("The number {} cannot be converted to a BigInt because it is not an integer", shown)))
            }
        };
    }
    if let Value::BigInt(_) = p {
        return Ok(p);
    }
    let b = rt.to_bigint(p)?;
    rt.big_charge(b.mag.len())?;
    rt.big_value((*b).clone())
}

fn this_big(rt: &mut Runtime, this: Value) -> JsResult<Rc<BigInt>> {
    match this {
        Value::BigInt(b) => Ok(rt.big(b)),
        Value::Object(o) => match rt.obj(o).kind {
            ObjKind::BigInt(b) => Ok(rt.big(b)),
            _ => Err(rt.type_error("BigInt.prototype method requires that 'this' be a BigInt")),
        },
        _ => Err(rt.type_error("BigInt.prototype method requires that 'this' be a BigInt")),
    }
}

fn to_string(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    let b = this_big(rt, this)?;
    let radix = match arg(a, 0) {
        Value::Undefined => 10.0,
        v => crate::ops::to_integer(rt.to_num(v)?),
    };
    if !(2.0..=36.0).contains(&radix) {
        return Err(rt.range_error("toString() radix must be between 2 and 36"));
    }
    let s = rt.big_to_string(&b, radix as u32)?;
    Ok(rt.new_str(&s))
}

/// Formats through `Intl.NumberFormat` (always en-US), honouring `options`.
fn to_locale_string(rt: &mut Runtime, this: Value, a: &[Value]) -> JsResult<Value> {
    this_big(rt, this)?;
    let prim = match this {
        Value::Object(o) => match rt.obj(o).kind {
            ObjKind::BigInt(b) => Value::BigInt(b),
            _ => this,
        },
        _ => this,
    };
    let s = super::intl::number_to_locale(rt, prim, arg(a, 0), arg(a, 1))?;
    Ok(rt.new_str(&s))
}

fn value_of(rt: &mut Runtime, this: Value, _: &[Value]) -> JsResult<Value> {
    match this {
        Value::BigInt(_) => Ok(this),
        _ => {
            let b = this_big(rt, this)?;
            rt.big_charge(b.mag.len())?;
            rt.big_value((*b).clone())
        }
    }
}

/// `BigInt.asIntN(bits, x)` (signed) / `BigInt.asUintN(bits, x)`.
fn as_n(rt: &mut Runtime, a: &[Value], signed: bool) -> JsResult<Value> {
    let bits = rt.to_index(arg(a, 0), "bit count")?;
    let x = rt.to_bigint(arg(a, 1))?;
    rt.big_charge(x.mag.len().max(bits.min(MAX_BIGINT_BITS) / 32))?;
    if bits > MAX_BIGINT_BITS {
        // wider than any BigInt here: unchanged, unless a negative value would have to wrap
        if signed || !x.neg {
            return rt.big_value((*x).clone());
        }
        return Err(rt.range_error("Maximum BigInt size exceeded"));
    }
    let r = if signed { x.as_int_n(bits) } else { x.as_uint_n(bits) };
    rt.big_value(r)
}

fn as_int_n(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    as_n(rt, a, true)
}
fn as_uint_n(rt: &mut Runtime, _: Value, a: &[Value]) -> JsResult<Value> {
    as_n(rt, a, false)
}

pub(super) fn setup(rt: &mut Runtime) {
    let op = rt.intr.object_proto;
    let bp = rt.new_object(op);
    rt.intr.bigint_proto = Some(bp);
    let c = rt.constructor("BigInt", 1, bigint_fn, bp);
    rt.method(c, "asIntN", 2, as_int_n);
    rt.method(c, "asUintN", 2, as_uint_n);
    rt.method(bp, "toString", 0, to_string);
    rt.method(bp, "toLocaleString", 0, to_locale_string);
    rt.method(bp, "valueOf", 0, value_of);
    let tag = rt.new_str("BigInt");
    rt.define_value(bp, PropKey::Sym(SYM_TO_STRING_TAG), tag, CONFIGURABLE);
}
