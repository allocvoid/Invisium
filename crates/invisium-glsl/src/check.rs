//! Typing rules: operators, assignments, l-values, constructors, calls, constant folding.

use crate::ir::*;
use crate::parse::{Parser, Sym, R};
use crate::err;

/// Result type of an arithmetic operator, or `None` if the operands do not fit.
pub fn arith(op: BOp, a: &Ty, b: &Ty) -> Option<Ty> {
    let (ka, kb) = (a.kind()?, b.kind()?);
    if ka != kb || ka == K::Bool {
        return None;
    }
    match op {
        BOp::Add | BOp::Sub | BOp::Div | BOp::Mul | BOp::Rem => {
            if op == BOp::Rem && (ka == K::Float) {
                return None;
            }
            if op == BOp::Mul {
                match (a, b) {
                    (Ty::M(c1, r1), Ty::M(c2, r2)) => return (c1 == r2).then(|| Ty::M(*c2, *r1)),
                    (Ty::V(_, n), Ty::M(c, r)) => return (*n == *r).then(|| Ty::V(K::Float, *c)),
                    (Ty::M(c, r), Ty::V(_, n)) => return (*n == *c).then(|| Ty::V(K::Float, *r)),
                    _ => {}
                }
            }
            if (a.is_mat() || b.is_mat()) && op == BOp::Rem {
                return None;
            }
            match (a, b) {
                _ if a == b => Some(a.clone()),
                (Ty::S(_), Ty::V(..) | Ty::M(..)) => Some(b.clone()),
                (Ty::V(..) | Ty::M(..), Ty::S(_)) => Some(a.clone()),
                _ => None,
            }
        }
        BOp::And | BOp::Or | BOp::Xor => {
            if ka == K::Float || a.is_mat() || b.is_mat() {
                return None;
            }
            match (a, b) {
                _ if a == b => Some(a.clone()),
                (Ty::S(_), Ty::V(..)) => Some(b.clone()),
                (Ty::V(..), Ty::S(_)) => Some(a.clone()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn shift(a: &Ty, b: &Ty) -> Option<Ty> {
    if !a.is_int() || !b.is_int() {
        return None;
    }
    match (a, b) {
        (Ty::S(_), Ty::S(_)) => Some(a.clone()),
        (Ty::V(_, n), Ty::V(_, m)) if n == m => Some(a.clone()),
        (Ty::V(..), Ty::S(_)) => Some(a.clone()),
        _ => None,
    }
}

impl Parser<'_> {
    pub fn binop(&mut self, op: &str, a: Expr, b: Expr, line: u32) -> R<Expr> {
        let v3 = self.version == 300;
        let bad = |p: &Parser| {
            err(line, format!(
                "'{op}' : wrong operand types - no operation '{op}' exists that takes a left-hand operand of type '{}' and a right operand of type '{}'",
                a.ty.name(&p.m),
                b.ty.name(&p.m)
            ))
        };
        let (bop, ty) = match op {
            "&&" | "||" => {
                if a.ty != BOOL || b.ty != BOOL {
                    return bad(self);
                }
                let k = if op == "&&" { EK::LogAnd(Box::new(a), Box::new(b)) } else { EK::LogOr(Box::new(a), Box::new(b)) };
                return self.mk(BOOL, k);
            }
            "^^" => {
                if a.ty != BOOL || b.ty != BOOL {
                    return bad(self);
                }
                (BOp::LogXor, BOOL)
            }
            "==" | "!=" => {
                if a.ty != b.ty || a.ty == Ty::Void || a.ty.has_sampler(&self.m) {
                    return bad(self);
                }
                (if op == "==" { BOp::Eq } else { BOp::Ne }, BOOL)
            }
            "<" | ">" | "<=" | ">=" => {
                if a.ty != b.ty || !a.ty.is_scalar() || !a.ty.is_numeric() {
                    return bad(self);
                }
                let o = match op {
                    "<" => BOp::Lt,
                    ">" => BOp::Gt,
                    "<=" => BOp::Le,
                    _ => BOp::Ge,
                };
                (o, BOOL)
            }
            "<<" | ">>" => {
                if !v3 {
                    return err(line, format!("'{op}' : bit operators require version 300"));
                }
                match shift(&a.ty, &b.ty) {
                    Some(t) => (if op == "<<" { BOp::Shl } else { BOp::Shr }, t),
                    None => return bad(self),
                }
            }
            _ => {
                let o = match op {
                    "+" => BOp::Add,
                    "-" => BOp::Sub,
                    "*" => BOp::Mul,
                    "/" => BOp::Div,
                    "%" => BOp::Rem,
                    "&" => BOp::And,
                    "|" => BOp::Or,
                    _ => BOp::Xor,
                };
                if !v3 && matches!(o, BOp::Rem | BOp::And | BOp::Or | BOp::Xor) {
                    return err(line, format!("'{op}' : operator requires version 300"));
                }
                match arith(o, &a.ty, &b.ty) {
                    Some(t) => (o, t),
                    None => return bad(self),
                }
            }
        };
        self.mk(ty, EK::Bin(bop, Box::new(a), Box::new(b)))
    }

    pub fn unop(&mut self, op: &str, e: Expr, line: u32) -> R<Expr> {
        let ok = match op {
            "+" | "-" => e.ty.is_numeric(),
            "!" => e.ty == BOOL,
            _ => self.version == 300 && e.ty.is_int(),
        };
        if !ok {
            return err(line, format!("'{op}' : wrong operand type '{}'", e.ty.name(&self.m)));
        }
        let u = match op {
            "+" => return Ok(e),
            "-" => UOp::Neg,
            "!" => UOp::Not,
            _ => UOp::BitNot,
        };
        self.mk(e.ty.clone(), EK::Un(u, Box::new(e)))
    }

    pub fn make_assign(&mut self, op: Option<BOp>, l: Expr, r: Expr, line: u32) -> R<Expr> {
        self.lvalue_check(&l, line)?;
        let ok = match op {
            None => l.ty == r.ty,
            Some(BOp::Shl | BOp::Shr) => self.version == 300 && shift(&l.ty, &r.ty).as_ref() == Some(&l.ty),
            Some(o) => {
                (self.version == 300 || !matches!(o, BOp::Rem | BOp::And | BOp::Or | BOp::Xor))
                    && arith(o, &l.ty, &r.ty).as_ref() == Some(&l.ty)
            }
        };
        if !ok || l.ty.has_sampler(&self.m) {
            return err(line, format!("assignment : cannot convert from '{}' to '{}'", r.ty.name(&self.m), l.ty.name(&self.m)));
        }
        self.mk(l.ty.clone(), EK::Assign(op, Box::new(l), Box::new(r)))
    }

    pub fn lvalue_check(&self, e: &Expr, line: u32) -> R<()> {
        let bad = |what: &str| err(line, format!("l-value required ({what})"));
        match &e.k {
            EK::Var(v) => {
                let v = &self.m.vars[*v as usize];
                let what = match v.storage {
                    Storage::Const => "can't modify a const",
                    Storage::Uniform => "can't modify a uniform",
                    Storage::Attribute => "can't modify a vertex input",
                    Storage::VaryingIn => "can't modify a fragment input",
                    Storage::Builtin(BV::FragCoord | BV::FrontFacing | BV::PointCoord | BV::VertexID | BV::InstanceID) => {
                        "can't modify a read-only built-in variable"
                    }
                    _ => return Ok(()),
                };
                err(line, format!("'{}' : l-value required ({what})", v.name))
            }
            EK::Index(a, i) => {
                if matches!(a.k, EK::Swz(..)) && self.fold(i).is_none() {
                    return bad("swizzles can only be indexed with constants when assigned");
                }
                self.lvalue_check(a, line)
            }
            EK::Field(a, _) => self.lvalue_check(a, line),
            EK::Swz(a, c) => {
                for (k, x) in c.iter().enumerate() {
                    if c[..k].contains(x) {
                        return bad("vector swizzle with duplicate components");
                    }
                }
                self.lvalue_check(a, line)
            }
            _ => bad("not assignable"),
        }
    }

    pub fn construct(&mut self, ty: Ty, args: Vec<Expr>, line: u32) -> R<Expr> {
        let bad = |p: &Parser, m: &str| err(line, format!("'{}' : {m}", ty.name(&p.m)));
        if args.is_empty() {
            return bad(self, "constructor needs arguments");
        }
        match &ty {
            Ty::Array(e, n) => {
                if (*n != 0 && args.len() != *n as usize) || args.iter().any(|a| a.ty != **e) {
                    return bad(self, "wrong arguments for array constructor");
                }
                let t = Ty::Array(e.clone(), args.len() as u32);
                return self.mk(t, EK::Ctor(args));
            }
            Ty::Struct(s) => {
                let f = &self.m.structs[*s as usize].fields;
                if f.len() != args.len() || f.iter().zip(&args).any(|(f, a)| f.1 != a.ty) {
                    return bad(self, "wrong arguments for structure constructor");
                }
                return self.mk(ty.clone(), EK::Ctor(args));
            }
            Ty::S(_) | Ty::V(..) | Ty::M(..) => {}
            _ => return bad(self, "cannot construct this type"),
        }
        if args.iter().any(|a| a.ty.comps() == 0) {
            return bad(self, "constructor arguments must be scalars, vectors or matrices");
        }
        let need = ty.comps();
        if args.len() == 1 {
            let a = &args[0];
            if ty.is_mat() || a.ty.comps() >= need || a.ty.is_scalar() {
                return self.mk(ty.clone(), EK::Ctor(args));
            }
            return bad(self, "not enough data provided for construction");
        }
        if ty.is_scalar() {
            return bad(self, "too many arguments");
        }
        if ty.is_mat() && args.iter().any(|a| a.ty.is_mat()) {
            return bad(self, "a matrix argument must be the only argument");
        }
        let total: u32 = args.iter().map(|a| a.ty.comps()).sum();
        let last = args.last().map_or(0, |a| a.ty.comps());
        if total < need {
            return bad(self, "not enough data provided for construction");
        }
        if total - last >= need {
            return bad(self, "too many arguments");
        }
        self.mk(ty.clone(), EK::Ctor(args))
    }

    pub fn call(&mut self, name: &str, args: Vec<Expr>, line: u32) -> R<Expr> {
        match self.lookup(name) {
            Some(Sym::Funcs(fs)) => {
                let fs = fs.clone();
                for f in fs {
                    let fd = &self.m.funcs[f as usize];
                    if fd.params.len() == args.len() && fd.params.iter().zip(&args).all(|(p, a)| self.m.vars[*p as usize].ty == a.ty) {
                        for (p, a) in fd.params.iter().zip(&args) {
                            if matches!(self.m.vars[*p as usize].storage, Storage::Param(PQ::Out | PQ::InOut)) {
                                self.lvalue_check(a, line)?;
                            }
                        }
                        let ret = fd.ret.clone();
                        match self.func {
                            Some(cur) => self.m.funcs[cur as usize].calls.push((f, line)),
                            None => self.init_calls.push((f, line)),
                        }
                        return self.mk(ret, EK::Call(f, args));
                    }
                }
                err(line, format!("'{name}' : no matching overloaded function found"))
            }
            Some(_) => err(line, format!("'{name}' : not a function")),
            None => {
                let types: Vec<Ty> = args.iter().map(|a| a.ty.clone()).collect();
                match crate::builtins::sig(self, name, &types) {
                    None => err(line, format!("'{name}' : no matching function (undeclared identifier)")),
                    Some(Err(m)) => err(line, format!("'{name}' : {m}")),
                    Some(Ok((n, ret))) => {
                        if n == "modf" {
                            self.lvalue_check(&args[1], line)?;
                        }
                        self.mk(ret, EK::Bi(n, args))
                    }
                }
            }
        }
    }

    /// Value of a constant scalar expression.
    pub fn fold(&self, e: &Expr) -> Option<CVal> {
        fold(&self.m, e)
    }
}

/// Value of a constant scalar expression.
pub fn fold(m: &Module, e: &Expr) -> Option<CVal> {
    use CVal::*;
    Some(match &e.k {
        EK::Lit(c) => *c,
        EK::Var(v) => m.vars[*v as usize].cval?,
        EK::Un(op, a) => match (op, fold(m, a)?) {
            (UOp::Neg, I(x)) => I(x.wrapping_neg()),
            (UOp::Neg, U(x)) => U(x.wrapping_neg()),
            (UOp::Neg, F(x)) => F(-x),
            (UOp::Not, B(x)) => B(!x),
            (UOp::BitNot, I(x)) => I(!x),
            (UOp::BitNot, U(x)) => U(!x),
            _ => return None,
        },
        EK::Bin(op, a, b) => {
            let (x, y) = (fold(m, a)?, fold(m, b)?);
            match (x, y) {
                (I(x), I(y)) => fold_int(*op, x as i64, y as i64).map(|v| match v {
                    Ok(v) => I(v as i32),
                    Err(b) => B(b),
                })?,
                (U(x), U(y)) => fold_int(*op, x as i64, y as i64).map(|v| match v {
                    Ok(v) => U(v as u32),
                    Err(b) => B(b),
                })?,
                (F(x), F(y)) => match op {
                    BOp::Add => F(x + y),
                    BOp::Sub => F(x - y),
                    BOp::Mul => F(x * y),
                    BOp::Div => F(x / y),
                    BOp::Lt => B(x < y),
                    BOp::Gt => B(x > y),
                    BOp::Le => B(x <= y),
                    BOp::Ge => B(x >= y),
                    BOp::Eq => B(x == y),
                    BOp::Ne => B(x != y),
                    _ => return None,
                },
                (B(x), B(y)) => match op {
                    BOp::Eq => B(x == y),
                    BOp::Ne | BOp::LogXor => B(x != y),
                    _ => return None,
                },
                _ => return None,
            }
        }
        EK::LogAnd(a, b) => B(fold(m, a)? == B(true) && fold(m, b)? == B(true)),
        EK::LogOr(a, b) => B(fold(m, a)? == B(true) || fold(m, b)? == B(true)),
        EK::Cond(c, a, b) => {
            if fold(m, c)? == B(true) {
                fold(m, a)?
            } else {
                fold(m, b)?
            }
        }
        EK::Ctor(args) if args.len() == 1 && e.ty.is_scalar() => {
            let x = fold(m, &args[0])?;
            let f = match x {
                B(b) => b as i32 as f64,
                I(i) => i as f64,
                U(u) => u as f64,
                F(f) => f as f64,
            };
            match e.ty {
                Ty::S(K::Float) => F(f as f32),
                Ty::S(K::Int) => match x {
                    U(u) => I(u as i32),
                    _ => I(f as i32),
                },
                Ty::S(K::Uint) => match x {
                    I(i) => U(i as u32),
                    _ => U(f as u32),
                },
                _ => B(f != 0.0),
            }
        }
        _ => return None,
    })
}

/// Integer operator on constants; `Err(b)` for comparisons.
fn fold_int(op: BOp, x: i64, y: i64) -> Option<Result<i64, bool>> {
    Some(match op {
        BOp::Add => Ok(x.wrapping_add(y)),
        BOp::Sub => Ok(x.wrapping_sub(y)),
        BOp::Mul => Ok(x.wrapping_mul(y)),
        BOp::Div if y != 0 => Ok(x.wrapping_div(y)),
        BOp::Rem if y != 0 => Ok(x.wrapping_rem(y)),
        BOp::Shl => Ok(x.wrapping_shl((y & 31) as u32)),
        BOp::Shr => Ok(x.wrapping_shr((y & 31) as u32)),
        BOp::And => Ok(x & y),
        BOp::Or => Ok(x | y),
        BOp::Xor => Ok(x ^ y),
        BOp::Lt => Err(x < y),
        BOp::Gt => Err(x > y),
        BOp::Le => Err(x <= y),
        BOp::Ge => Err(x >= y),
        BOp::Eq => Err(x == y),
        BOp::Ne => Err(x != y),
        _ => return None,
    })
}
