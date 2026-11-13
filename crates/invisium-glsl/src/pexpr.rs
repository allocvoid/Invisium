//! Expressions: precedence climbing over the GLSL operator table.

use crate::ir::*;
use crate::lex::T;
use crate::parse::{Parser, Sym, R};
use crate::err;

/// Deepest expression tree.
pub const MAX_EXPR_DEPTH: u32 = 200;

fn bin_prec(p: &str) -> Option<u8> {
    Some(match p {
        "||" => 1,
        "^^" => 2,
        "&&" => 3,
        "|" => 4,
        "^" => 5,
        "&" => 6,
        "==" | "!=" => 7,
        "<" | ">" | "<=" | ">=" => 8,
        "<<" | ">>" => 9,
        "+" | "-" => 10,
        "*" | "/" | "%" => 11,
        _ => return None,
    })
}

impl Parser<'_> {
    pub fn mk(&self, ty: Ty, k: EK) -> R<Expr> {
        let e = Expr::new(ty, k);
        if e.depth > MAX_EXPR_DEPTH {
            return err(self.line(), "expression too complex");
        }
        Ok(e)
    }

    pub fn expr(&mut self) -> R<Expr> {
        let mut e = self.assign()?;
        while self.is(",") {
            self.i += 1;
            let r = self.assign()?;
            e = self.mk(r.ty.clone(), EK::Comma(Box::new(e), Box::new(r)))?;
        }
        Ok(e)
    }

    pub fn assign(&mut self) -> R<Expr> {
        self.enter()?;
        let r = self.assign_inner();
        self.leave();
        r
    }

    fn assign_inner(&mut self) -> R<Expr> {
        let l = self.cond()?;
        let op = match &self.peek().t {
            T::P("=") => None,
            T::P(p) => Some(match *p {
                "+=" => BOp::Add,
                "-=" => BOp::Sub,
                "*=" => BOp::Mul,
                "/=" => BOp::Div,
                "%=" => BOp::Rem,
                "<<=" => BOp::Shl,
                ">>=" => BOp::Shr,
                "&=" => BOp::And,
                "|=" => BOp::Or,
                "^=" => BOp::Xor,
                _ => return Ok(l),
            }),
            _ => return Ok(l),
        };
        let line = self.line();
        self.i += 1;
        let r = self.assign()?;
        self.make_assign(op, l, r, line)
    }

    pub fn cond(&mut self) -> R<Expr> {
        let c = self.binary(1)?;
        if !self.is("?") {
            return Ok(c);
        }
        let line = self.line();
        self.i += 1;
        if c.ty != BOOL {
            return err(line, "'?:' : boolean expression expected");
        }
        let a = self.expr()?;
        self.expect(":")?;
        let b = self.assign()?;
        if a.ty != b.ty || a.ty == Ty::Void || a.ty.has_sampler(&self.m) {
            return err(line, format!("'?:' : wrong operand types ('{}' and '{}')", a.ty.name(&self.m), b.ty.name(&self.m)));
        }
        self.mk(a.ty.clone(), EK::Cond(Box::new(c), Box::new(a), Box::new(b)))
    }

    fn binary(&mut self, min: u8) -> R<Expr> {
        let mut l = self.unary()?;
        loop {
            let (op, p) = match &self.peek().t {
                T::P(op) => match bin_prec(op) {
                    Some(p) => (*op, p),
                    None => break,
                },
                _ => break,
            };
            if p < min {
                break;
            }
            let line = self.line();
            self.i += 1;
            let r = self.binary(p + 1)?;
            l = self.binop(op, l, r, line)?;
        }
        Ok(l)
    }

    fn unary(&mut self) -> R<Expr> {
        self.enter()?;
        let r = self.unary_inner();
        self.leave();
        r
    }

    fn unary_inner(&mut self) -> R<Expr> {
        let line = self.line();
        let op = match &self.peek().t {
            T::P(p @ ("++" | "--" | "+" | "-" | "!" | "~")) => *p,
            _ => return self.postfix(),
        };
        self.i += 1;
        let e = self.unary()?;
        match op {
            "++" | "--" => self.incdec(e, true, op == "--", line),
            _ => self.unop(op, e, line),
        }
    }

    pub fn incdec(&mut self, e: Expr, pre: bool, dec: bool, line: u32) -> R<Expr> {
        self.lvalue_check(&e, line)?;
        if !e.ty.is_numeric() {
            return err(line, format!("'{}' : wrong operand type '{}'", if dec { "--" } else { "++" }, e.ty.name(&self.m)));
        }
        self.mk(e.ty.clone(), EK::Inc { pre, dec, e: Box::new(e) })
    }

    fn postfix(&mut self) -> R<Expr> {
        let mut e = self.primary()?;
        loop {
            let line = self.line();
            if self.eat("[") {
                let idx = self.expr()?;
                self.expect("]")?;
                e = self.index(e, idx, line)?;
            } else if self.eat(".") {
                let name = self.ident()?;
                if name == "length" && self.is("(") {
                    self.expect("(")?;
                    self.expect(")")?;
                    match e.ty {
                        Ty::Array(_, n) if self.version == 300 => e = Expr::lit(CVal::I(n as i32)),
                        _ => return err(line, "'length' : only arrays have a length() method"),
                    }
                } else {
                    e = self.member(e, &name, line)?;
                }
            } else if self.is("++") || self.is("--") {
                let dec = self.is("--");
                self.i += 1;
                e = self.incdec(e, false, dec, line)?;
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn primary(&mut self) -> R<Expr> {
        let line = self.line();
        let t = self.peek().t.clone();
        match t {
            T::Int(v, u) => {
                self.i += 1;
                if u {
                    if self.version != 300 {
                        return err(line, "unsigned integer literals require version 300");
                    }
                    Ok(Expr::lit(CVal::U(v as u32)))
                } else {
                    Ok(Expr::lit(CVal::I(v as u32 as i32)))
                }
            }
            T::Float(f) => {
                self.i += 1;
                Ok(Expr::lit(CVal::F(f as f32)))
            }
            T::P("(") => {
                self.i += 1;
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            T::Id(name) => {
                if &*name == "true" || &*name == "false" {
                    self.i += 1;
                    return Ok(Expr::lit(CVal::B(&*name == "true")));
                }
                if self.at_type() {
                    let ty = self.type_spec()?;
                    let args = self.args()?;
                    return self.construct(ty, args, line);
                }
                if self.peek_at(1).is("(") {
                    self.i += 1;
                    let args = self.args()?;
                    return self.call(&name, args, line);
                }
                self.i += 1;
                match self.lookup(&name) {
                    Some(Sym::Var(id)) => {
                        let id = *id;
                        let v = &self.m.vars[id as usize];
                        match v.storage {
                            Storage::Builtin(BV::FragColor) => self.used_frag_color = true,
                            Storage::Builtin(BV::FragData) => self.used_frag_data = true,
                            _ => {}
                        }
                        Ok(Expr::new(v.ty.clone(), EK::Var(id)))
                    }
                    Some(_) => err(line, format!("'{name}' : not a variable")),
                    None => err(line, format!("'{name}' : undeclared identifier")),
                }
            }
            t => err(line, format!("syntax error: unexpected '{t}'")),
        }
    }

    fn args(&mut self) -> R<Vec<Expr>> {
        self.expect("(")?;
        let mut v = Vec::new();
        if self.eat(")") {
            return Ok(v);
        }
        if self.is_id("void") && self.peek_at(1).is(")") {
            self.i += 2;
            return Ok(v);
        }
        loop {
            v.push(self.assign()?);
            if !self.eat(",") {
                break;
            }
        }
        self.expect(")")?;
        Ok(v)
    }

    fn index(&mut self, e: Expr, idx: Expr, line: u32) -> R<Expr> {
        if !matches!(idx.ty, Ty::S(K::Int | K::Uint)) {
            return err(line, "'[]' : integer expression required for an index");
        }
        let (ty, n) = match &e.ty {
            Ty::Array(t, n) => ((**t).clone(), *n),
            Ty::V(k, n) => (Ty::S(*k), *n as u32),
            Ty::M(c, r) => (Ty::V(K::Float, *r), *c as u32),
            t => return err(line, format!("'[]' : '{}' cannot be indexed", t.name(&self.m))),
        };
        let c = self.fold(&idx).and_then(|c| c.as_i64());
        if let Some(c) = c {
            if c < 0 || c >= n as i64 {
                return err(line, format!("'[]' : index {c} is out of range"));
            }
        } else if matches!(ty, Ty::Sampler(_)) {
            return err(line, "sampler arrays can only be indexed with constant expressions");
        }
        self.mk(ty, EK::Index(Box::new(e), Box::new(idx)))
    }

    fn member(&mut self, e: Expr, name: &str, line: u32) -> R<Expr> {
        match &e.ty {
            Ty::Struct(s) => {
                let sd = &self.m.structs[*s as usize];
                match sd.fields.iter().position(|f| f.0 == name) {
                    Some(k) => {
                        let ty = sd.fields[k].1.clone();
                        self.mk(ty, EK::Field(Box::new(e), k as u32))
                    }
                    None => err(line, format!("'{name}' : no such field in structure '{}'", sd.name)),
                }
            }
            Ty::V(k, n) => {
                let (k, n) = (*k, *n);
                let set = ["xyzw", "rgba", "stpq"].into_iter().find(|s| name.chars().next().is_some_and(|c| s.contains(c)));
                let bad = || err(line, format!("'{name}' : illegal vector field selection"));
                let Some(set) = set else { return bad() };
                if name.len() > 4 {
                    return bad();
                }
                let mut comps = Vec::new();
                for c in name.chars() {
                    match set.find(c) {
                        Some(i) if (i as u8) < n => comps.push(i as u8),
                        _ => return bad(),
                    }
                }
                let ty = Ty::vec(k, comps.len() as u8);
                self.mk(ty, EK::Swz(Box::new(e), comps))
            }
            t => err(line, format!("'{name}' : field selection requires a structure or vector, not '{}'", t.name(&self.m))),
        }
    }
}
