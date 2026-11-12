//! Type conversions and operator semantics.

use crate::bytecode::Op;
use crate::numfmt;
use crate::runtime::*;
use crate::value::*;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq)]
pub enum Hint { Default, Number, String }

pub fn to_int32(n: f64) -> i32 {
    if !n.is_finite() {
        return 0;
    }
    let m = n.trunc() % 4294967296.0;
    let m = if m < 0.0 { m + 4294967296.0 } else { m };
    (m as u64 as u32) as i32
}

pub fn to_uint32(n: f64) -> u32 {
    to_int32(n) as u32
}

pub fn to_integer(n: f64) -> f64 {
    if n.is_nan() { 0.0 } else { n.trunc() + 0.0 }
}

/// A binary arithmetic or bitwise operator on two Numbers.
#[inline]
pub fn number_op(op: Op, x: f64, y: f64) -> f64 {
    match op {
        Op::Add => x + y,
        Op::Sub => x - y,
        Op::Mul => x * y,
        Op::Div => x / y,
        Op::Mod => {
            if y.is_infinite() && x.is_finite() { x } else { x % y }
        }
        Op::Exp => {
            if y.is_nan() || ((x == 1.0 || x == -1.0) && y.is_infinite()) { f64::NAN } else { x.powf(y) }
        }
        _ => {
            let (a, b) = (to_int32(x), to_uint32(y));
            match op {
                Op::Shl => a.wrapping_shl(b & 31) as f64,
                Op::Shr => (a >> (b & 31)) as f64,
                Op::UShr => ((a as u32) >> (b & 31)) as f64,
                Op::BitAnd => (a & b as i32) as f64,
                Op::BitOr => (a | b as i32) as f64,
                _ => (a ^ b as i32) as f64,
            }
        }
    }
}

impl Runtime {
    pub(crate) fn type_error(&mut self, msg: &str) -> Exc {
        Exc::Throw(self.make_error("TypeError", msg))
    }
    pub(crate) fn range_error(&mut self, msg: &str) -> Exc {
        Exc::Throw(self.make_error("RangeError", msg))
    }
    pub(crate) fn reference_error(&mut self, msg: &str) -> Exc {
        Exc::Throw(self.make_error("ReferenceError", msg))
    }

    pub(crate) fn truthy(&self, v: Value) -> bool {
        match v {
            Value::Undefined | Value::Null => false,
            Value::Bool(b) => b,
            Value::Number(n) => !(n == 0.0 || n.is_nan()),
            Value::String(s) => !self.str_units(s).is_empty(),
            Value::Symbol(_) | Value::Object(_) => true,
            Value::BigInt(b) => self.big_ref(b).is_some_and(|b| !b.is_zero()),
        }
    }

    pub(crate) fn is_callable_obj(&self, o: ObjId) -> bool {
        match &self.obj(o).kind {
            ObjKind::Function(_) => true,
            ObjKind::Proxy(p) => p.callable,
            _ => false,
        }
    }

    pub(crate) fn is_callable_val(&self, v: Value) -> bool {
        matches!(v, Value::Object(o) if self.is_callable_obj(o))
    }

    pub(crate) fn typeof_str(&self, v: Value) -> &'static str {
        match v {
            Value::Undefined => "undefined",
            Value::Null => "object",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Symbol(_) => "symbol",
            Value::BigInt(_) => "bigint",
            Value::Object(o) => if self.is_callable_obj(o) { "function" } else { "object" },
        }
    }

    pub(crate) fn to_primitive(&mut self, v: Value, hint: Hint) -> JsResult<Value> {
        let Value::Object(o) = v else { return Ok(v) };
        let exotic = self.get_prop(v, PropKey::Sym(SYM_TO_PRIMITIVE))?;
        if !exotic.is_nullish() {
            if !self.is_callable_val(exotic) {
                return Err(self.type_error("Symbol.toPrimitive is not a function"));
            }
            let h = match hint {
                Hint::Default => "default",
                Hint::Number => "number",
                Hint::String => "string",
            };
            let hv = self.new_str(h);
            let r = self.call_value(exotic, v, &[hv])?;
            if matches!(r, Value::Object(_)) {
                return Err(self.type_error("Cannot convert object to primitive value"));
            }
            return Ok(r);
        }
        let order = if hint == Hint::String { [A_TO_STRING, A_VALUE_OF] } else { [A_VALUE_OF, A_TO_STRING] };
        for a in order {
            let f = self.get_prop(Value::Object(o), PropKey::Str(a))?;
            if self.is_callable_val(f) {
                let r = self.call_value(f, v, &[])?;
                if !matches!(r, Value::Object(_)) {
                    return Ok(r);
                }
            }
        }
        Err(self.type_error("Cannot convert object to primitive value"))
    }

    pub(crate) fn to_num(&mut self, v: Value) -> JsResult<f64> {
        Ok(match v {
            Value::Undefined => f64::NAN,
            Value::Null => 0.0,
            Value::Bool(b) => b as u8 as f64,
            Value::Number(n) => n,
            Value::String(s) => self.str_to_num(s),
            Value::Symbol(_) => return Err(self.type_error("Cannot convert a Symbol value to a number")),
            Value::BigInt(_) => return Err(self.type_error("Cannot convert a BigInt value to a number")),
            Value::Object(_) => {
                let p = self.to_primitive(v, Hint::Number)?;
                return self.to_num(p);
            }
        })
    }

    /// ToNumeric: a Number or a BigInt.
    pub(crate) fn to_numeric(&mut self, v: Value) -> JsResult<Value> {
        match v {
            Value::Number(_) | Value::BigInt(_) => Ok(v),
            _ => match self.to_primitive(v, Hint::Number)? {
                p @ Value::BigInt(_) => Ok(p),
                p => Ok(Value::Number(self.to_num(p)?)),
            },
        }
    }

    /// A binary arithmetic or bitwise operator when the operands are not two Numbers:
    /// conversions, then Number or BigInt arithmetic (never a mix of the two).
    pub(crate) fn numeric_binary(&mut self, op: Op, a: Value, b: Value) -> JsResult<Value> {
        let x = self.to_numeric(a)?;
        let y = self.to_numeric(b)?;
        match (x, y) {
            (Value::Number(x), Value::Number(y)) => Ok(Value::Number(number_op(op, x, y))),
            (Value::BigInt(x), Value::BigInt(y)) => {
                let (x, y) = (self.big(x), self.big(y));
                self.big_binary(op, &x, &y)
            }
            _ => Err(self.type_error("Cannot mix BigInt and other types, use explicit conversions")),
        }
    }

    /// Unary `-`, `~`, `++` (delta 1) and `--` (delta -1) on a non-Number operand.
    pub(crate) fn numeric_unary(&mut self, op: Op, v: Value) -> JsResult<Value> {
        match self.to_numeric(v)? {
            Value::BigInt(b) => {
                let big = self.big(b);
                self.big_charge(big.mag.len())?;
                let r = match op {
                    Op::Neg => big.negate(),
                    Op::BitNot => big.bitnot(),
                    Op::Inc => big.add(&crate::bigint::BigInt::from_i64(1)),
                    _ => big.add(&crate::bigint::BigInt::from_i64(-1)),
                };
                self.big_value(r)
            }
            Value::Number(n) => Ok(Value::Number(match op {
                Op::Neg => -n,
                Op::BitNot => !to_int32(n) as f64,
                Op::Inc => n + 1.0,
                _ => n - 1.0,
            })),
            other => Ok(other),
        }
    }

    pub(crate) fn str_to_num(&self, s: StrId) -> f64 {
        let u = self.str_units(s);
        if u.len() <= 9 && !u.is_empty() && u.iter().all(|c| (b'0' as u16..=b'9' as u16).contains(c)) {
            return u.iter().fold(0.0, |a, c| a * 10.0 + (c - b'0' as u16) as f64);
        }
        numfmt::string_to_num(&String::from_utf16_lossy(u))
    }

    pub(crate) fn number_str(&mut self, n: f64) -> StrId {
        if n >= 0.0 && n < 1000.0 && n.fract() == 0.0 && !(n == 0.0 && n.is_sign_negative()) {
            let s = numfmt::number_to_string(n);
            return StrId::from_atom(self.intern(&s));
        }
        let s = numfmt::number_to_string(n);
        match self.new_str(&s) {
            Value::String(id) => id,
            _ => StrId::from_atom(A_EMPTY),
        }
    }

    pub(crate) fn to_str(&mut self, v: Value) -> JsResult<StrId> {
        Ok(match v {
            Value::Undefined => StrId::from_atom(A_UNDEFINED),
            Value::Null => StrId::from_atom(A_NULL),
            Value::Bool(true) => StrId::from_atom(A_TRUE),
            Value::Bool(false) => StrId::from_atom(A_FALSE),
            Value::Number(n) => self.number_str(n),
            Value::String(s) => s,
            Value::Symbol(_) => return Err(self.type_error("Cannot convert a Symbol value to a string")),
            Value::BigInt(b) => {
                let big = self.big(b);
                let text = self.big_to_string(&big, 10)?;
                match self.new_str(&text) {
                    Value::String(id) => id,
                    _ => StrId::from_atom(A_EMPTY),
                }
            }
            Value::Object(_) => {
                let p = self.to_primitive(v, Hint::String)?;
                return self.to_str(p);
            }
        })
    }

    pub(crate) fn to_rust_string(&mut self, v: Value) -> JsResult<String> {
        let s = self.to_str(v)?;
        Ok(self.rust_string(s))
    }

    pub(crate) fn to_units(&mut self, v: Value) -> JsResult<Rc<[u16]>> {
        let s = self.to_str(v)?;
        Ok(self.str_rc(s))
    }

    pub(crate) fn to_property_key(&mut self, v: Value) -> JsResult<PropKey> {
        match v {
            Value::String(s) => Ok(PropKey::Str(match s.as_atom() {
                Some(a) => a,
                None => {
                    let u = self.str_rc(s);
                    self.intern16(&u)
                }
            })),
            Value::Symbol(s) => Ok(PropKey::Sym(s)),
            Value::Number(n) if n >= 0.0 && n.fract() == 0.0 && n < 1e9 => {
                let s = (n as u64).to_string();
                Ok(PropKey::Str(self.intern(&s)))
            }
            Value::Object(_) => {
                let p = self.to_primitive(v, Hint::String)?;
                self.to_property_key(p)
            }
            _ => {
                let s = self.to_str(v)?;
                self.to_property_key(Value::String(s))
            }
        }
    }

    pub(crate) fn key_to_value(&self, k: PropKey) -> Value {
        match k {
            PropKey::Str(a) => self.atom_value(a),
            PropKey::Sym(s) => Value::Symbol(s),
        }
    }

    pub(crate) fn to_object(&mut self, v: Value) -> JsResult<ObjId> {
        let (proto, kind) = match v {
            Value::Object(o) => return Ok(o),
            Value::Undefined | Value::Null => return Err(self.type_error("Cannot convert undefined or null to object")),
            Value::Bool(b) => (self.intr.boolean_proto, ObjKind::Boolean(b)),
            Value::Number(n) => (self.intr.number_proto, ObjKind::Number(n)),
            Value::String(s) => (self.intr.string_proto, ObjKind::String(s)),
            Value::Symbol(s) => (self.intr.symbol_proto, ObjKind::Symbol(s)),
            Value::BigInt(b) => (self.intr.bigint_proto, ObjKind::BigInt(b)),
        };
        Ok(self.alloc(Object::new(proto, kind)))
    }

    pub(crate) fn str_eq(&self, a: StrId, b: StrId) -> bool {
        a == b || self.str_units(a) == self.str_units(b)
    }

    pub(crate) fn strict_equals(&self, a: Value, b: Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => x == y,
            (Value::String(x), Value::String(y)) => self.str_eq(x, y),
            (Value::BigInt(x), Value::BigInt(y)) => {
                x == y || {
                    let (a, b) = (self.big_ref(x), self.big_ref(y));
                    self.big_note(a.map_or(0, |a| a.mag.len()).max(b.map_or(0, |b| b.mag.len())));
                    a == b
                }
            }
            _ => a == b,
        }
    }

    pub(crate) fn same_value_zero(&self, a: Value, b: Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => x == y || (x.is_nan() && y.is_nan()),
            _ => self.strict_equals(a, b),
        }
    }

    pub(crate) fn same_value(&self, a: Value, b: Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                if x.is_nan() && y.is_nan() {
                    true
                } else {
                    x == y && x.is_sign_negative() == y.is_sign_negative()
                }
            }
            _ => self.strict_equals(a, b),
        }
    }

    pub(crate) fn loose_equals(&mut self, a: Value, b: Value) -> JsResult<bool> {
        use Value::*;
        Ok(match (a, b) {
            (Undefined | Null, Undefined | Null) => true,
            (Undefined | Null, _) | (_, Undefined | Null) => false,
            (Number(_), Number(_)) | (String(_), String(_)) | (Bool(_), Bool(_)) | (Symbol(_), Symbol(_)) | (Object(_), Object(_)) => {
                self.strict_equals(a, b)
            }
            (Number(x), String(s)) | (String(s), Number(x)) => x == self.str_to_num(s),
            (BigInt(_), BigInt(_)) => self.strict_equals(a, b),
            (BigInt(x), Number(n)) | (Number(n), BigInt(x)) => self.big(x).cmp_f64(n) == Some(std::cmp::Ordering::Equal),
            (BigInt(x), String(s)) | (String(s), BigInt(x)) => {
                let text = self.rust_string(s);
                match self.big_parse(&text)? {
                    Some(parsed) => *self.big(x) == parsed,
                    None => false,
                }
            }
            (Bool(x), _) => return self.loose_equals(Number(x as u8 as f64), b),
            (_, Bool(y)) => return self.loose_equals(a, Number(y as u8 as f64)),
            (Object(_), _) => {
                let p = self.to_primitive(a, Hint::Default)?;
                return self.loose_equals(p, b);
            }
            (_, Object(_)) => {
                let p = self.to_primitive(b, Hint::Default)?;
                return self.loose_equals(a, p);
            }
            _ => false,
        })
    }

    pub(crate) fn concat(&mut self, a: StrId, b: StrId) -> JsResult<Value> {
        let (lx, ly) = (self.str_units(a).len(), self.str_units(b).len());
        if lx == 0 {
            return Ok(Value::String(b));
        }
        if ly == 0 {
            return Ok(Value::String(a));
        }
        let len = lx + ly;
        if len > (1 << 28) {
            return Err(self.range_error("Invalid string length"));
        }
        if len > 4096 {
            self.check_alloc(len * 2).map_err(|t| self.thrown_to_exc(t))?;
        }
        let (x, y) = (self.str_units(a), self.str_units(b));
        let mut v = Vec::with_capacity(len);
        v.extend_from_slice(x);
        v.extend_from_slice(y);
        Ok(self.new_str16(v))
    }

    pub(crate) fn add(&mut self, a: Value, b: Value) -> JsResult<Value> {
        if let (Value::Number(x), Value::Number(y)) = (a, b) {
            return Ok(Value::Number(x + y));
        }
        if let (Value::String(x), Value::String(y)) = (a, b) {
            return self.concat(x, y);
        }
        let pa = self.to_primitive(a, Hint::Default)?;
        let pb = self.to_primitive(b, Hint::Default)?;
        if matches!(pa, Value::String(_)) || matches!(pb, Value::String(_)) {
            let sa = self.to_str(pa)?;
            let sb = self.to_str(pb)?;
            return self.concat(sa, sb);
        }
        self.numeric_binary(Op::Add, pa, pb)
    }

    /// Abstract relational comparison a < b. None = undefined (NaN involved).
    pub(crate) fn less_than(&mut self, a: Value, b: Value, left_first: bool) -> JsResult<Option<bool>> {
        let (pa, pb) = if left_first {
            let pa = self.to_primitive(a, Hint::Number)?;
            (pa, self.to_primitive(b, Hint::Number)?)
        } else {
            let pb = self.to_primitive(b, Hint::Number)?;
            (self.to_primitive(a, Hint::Number)?, pb)
        };
        if let (Value::String(x), Value::String(y)) = (pa, pb) {
            return Ok(Some(self.str_units(x) < self.str_units(y)));
        }
        if matches!(pa, Value::BigInt(_)) || matches!(pb, Value::BigInt(_)) {
            return self.big_less_than(pa, pb);
        }
        let x = self.to_num(pa)?;
        let y = self.to_num(pb)?;
        if x.is_nan() || y.is_nan() {
            return Ok(None);
        }
        Ok(Some(x < y))
    }

    /// `a < b` for primitives of which at least one is a BigInt: compared by mathematical
    /// value against Numbers, and against strings that parse as integers.
    fn big_less_than(&mut self, a: Value, b: Value) -> JsResult<Option<bool>> {
        use std::cmp::Ordering;
        let side = |rt: &mut Runtime, v: Value| -> JsResult<Option<Result<Rc<crate::bigint::BigInt>, f64>>> {
            Ok(match v {
                Value::BigInt(x) => Some(Ok(rt.big(x))),
                Value::String(s) => {
                    let text = rt.rust_string(s);
                    rt.big_parse(&text)?.map(|p| Ok(Rc::new(p)))
                }
                other => Some(Err(rt.to_num(other)?)),
            })
        };
        let (Some(x), Some(y)) = (side(self, a)?, side(self, b)?) else { return Ok(None) };
        if let (Ok(x), Ok(y)) = (&x, &y) {
            self.big_charge(x.mag.len().max(y.mag.len()))?;
        }
        let ord = match (x, y) {
            (Ok(x), Ok(y)) => Some(x.cmp(&y)),
            (Ok(x), Err(n)) => x.cmp_f64(n),
            (Err(n), Ok(y)) => y.cmp_f64(n).map(Ordering::reverse),
            (Err(m), Err(n)) => m.partial_cmp(&n),
        };
        Ok(ord.map(|o| o == Ordering::Less))
    }

    pub(crate) fn instance_of(&mut self, v: Value, target: Value) -> JsResult<bool> {
        let Value::Object(t) = target else {
            return Err(self.type_error("Right-hand side of 'instanceof' is not callable"));
        };
        let h = self.get_prop(target, PropKey::Sym(SYM_HAS_INSTANCE))?;
        if !h.is_nullish() && Some(h) != self.fn_has_instance.map(Value::Object) {
            let r = self.call_value(h, target, &[v])?;
            return Ok(self.truthy(r));
        }
        if !self.is_callable_obj(t) {
            return Err(self.type_error("Right-hand side of 'instanceof' is not callable"));
        }
        self.ordinary_has_instance(t, v)
    }

    pub(crate) fn ordinary_has_instance(&mut self, t: ObjId, v: Value) -> JsResult<bool> {
        if let ObjKind::Function(f) = &self.obj(t).kind {
            if let FuncData::Bound { target, .. } = &**f {
                let target = *target;
                return self.instance_of(v, Value::Object(target));
            }
        }
        let Value::Object(mut o) = v else { return Ok(false) };
        let proto = self.get_prop(Value::Object(t), PropKey::Str(A_PROTOTYPE))?;
        let Value::Object(p) = proto else {
            return Err(self.type_error("Function has non-object prototype in instanceof check"));
        };
        let mut guard = 0;
        while let Some(next) = self.obj(o).proto {
            if next == p {
                return Ok(true);
            }
            o = next;
            guard += 1;
            if guard > 100_000 {
                break;
            }
        }
        Ok(false)
    }
}
