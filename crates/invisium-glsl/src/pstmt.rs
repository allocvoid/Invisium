//! Statements.

use crate::ir::*;
use crate::lex::T;
use crate::parse::{Parser, Sym, R};
use crate::{err, Stage};

impl Parser<'_> {
    /// One statement. A declaration with several declarators gives several `Decl`s.
    pub fn statement(&mut self) -> R<Vec<Stmt>> {
        self.enter()?;
        let r = self.statement_inner();
        self.leave();
        r
    }

    fn statement_inner(&mut self) -> R<Vec<Stmt>> {
        let line = self.line();
        let word = self.peek().id().map(|s| s.to_string());
        let s = match word.as_deref() {
            _ if self.is("{") => {
                self.i += 1;
                self.push_scope();
                let mut v = Vec::new();
                while !self.eat("}") {
                    if self.i >= self.t.len() {
                        return err(self.line(), "syntax error: unexpected end of file, missing '}'");
                    }
                    v.extend(self.statement()?);
                }
                self.pop_scope();
                Stmt::Block(v)
            }
            _ if self.is(";") => {
                self.i += 1;
                return Ok(vec![]);
            }
            Some("if") => {
                // `else if` continues the chain in this loop: a chain is not nesting.
                let mut chain = Vec::new();
                let mut els = None;
                loop {
                    self.i += 1;
                    self.expect("(")?;
                    let c = self.bool_expr()?;
                    self.expect(")")?;
                    chain.push((c, self.sub()?));
                    if self.eat("else") {
                        if self.is_id("if") {
                            continue;
                        }
                        els = Some(Box::new(self.sub()?));
                    }
                    break;
                }
                Stmt::If(chain, els)
            }
            Some("for") => {
                self.i += 1;
                self.expect("(")?;
                self.push_scope();
                let init = if self.eat(";") {
                    vec![]
                } else if self.at_decl() {
                    self.local_decl()?
                } else {
                    let e = self.expr()?;
                    self.expect(";")?;
                    vec![Stmt::Expr(e)]
                };
                let cond = if self.is(";") { None } else { Some(self.bool_expr()?) };
                self.expect(";")?;
                let step = if self.is(")") { None } else { Some(self.expr()?) };
                self.expect(")")?;
                self.loops += 1;
                let body = self.sub();
                self.loops -= 1;
                self.pop_scope();
                Stmt::For { init, cond, step, body: Box::new(body?) }
            }
            Some("while") => {
                self.i += 1;
                self.expect("(")?;
                let c = self.bool_expr()?;
                self.expect(")")?;
                self.loops += 1;
                let body = self.sub();
                self.loops -= 1;
                Stmt::While(c, Box::new(body?))
            }
            Some("do") => {
                self.i += 1;
                self.loops += 1;
                let body = self.sub();
                self.loops -= 1;
                let body = body?;
                self.expect("while")?;
                self.expect("(")?;
                let c = self.bool_expr()?;
                self.expect(")")?;
                self.expect(";")?;
                Stmt::DoWhile(Box::new(body), c)
            }
            Some("switch") => self.switch()?,
            Some("case" | "default") => return err(line, "'case' outside of a switch statement"),
            Some("break") => {
                self.i += 1;
                if self.loops == 0 && self.switches == 0 {
                    return err(line, "'break' is only allowed in loops and switch statements");
                }
                self.expect(";")?;
                Stmt::Break
            }
            Some("continue") => {
                self.i += 1;
                if self.loops == 0 {
                    return err(line, "'continue' is only allowed in loops");
                }
                self.expect(";")?;
                Stmt::Continue
            }
            Some("return") => {
                self.i += 1;
                let ret = self.func.map_or(Ty::Void, |f| self.m.funcs[f as usize].ret.clone());
                if self.eat(";") {
                    if ret != Ty::Void {
                        return err(line, "'return' : non-void function must return a value");
                    }
                    Stmt::Return(None)
                } else {
                    let e = self.expr()?;
                    self.expect(";")?;
                    if e.ty != ret {
                        return err(line, format!("'return' : function return is not matching type ('{}' vs '{}')", e.ty.name(&self.m), ret.name(&self.m)));
                    }
                    Stmt::Return(Some(e))
                }
            }
            Some("discard") => {
                self.i += 1;
                if self.stage != Stage::Fragment {
                    return err(line, "'discard' is only allowed in fragment shaders");
                }
                self.expect(";")?;
                Stmt::Discard
            }
            Some("precision") => {
                self.precision_stmt()?;
                return Ok(vec![]);
            }
            _ if self.at_decl() => return self.local_decl(),
            _ => {
                let e = self.expr()?;
                self.expect(";")?;
                Stmt::Expr(e)
            }
        };
        Ok(vec![s])
    }

    fn sub(&mut self) -> R<Stmt> {
        self.push_scope();
        let s = self.statement();
        self.pop_scope();
        Ok(Stmt::Block(s?))
    }

    fn bool_expr(&mut self) -> R<Expr> {
        let line = self.line();
        let e = self.expr()?;
        if e.ty != BOOL {
            return err(line, "boolean expression expected");
        }
        Ok(e)
    }

    /// Does a declaration start here (rather than an expression)?
    fn at_decl(&self) -> bool {
        match self.peek().id() {
            Some("const" | "lowp" | "mediump" | "highp" | "struct" | "invariant" | "uniform" | "in" | "out" | "attribute" | "varying" | "flat" | "layout") => true,
            Some(s) => {
                let is_type = self.basic_type(s, 0).is_some() || matches!(self.lookup(s), Some(Sym::Struct(_)));
                if !is_type {
                    return false;
                }
                let mut k = 1;
                if self.peek_at(1).is("[") {
                    let mut level = 0;
                    loop {
                        let t = self.peek_at(k);
                        if matches!(t.t, T::P("<end of file>")) {
                            return true;
                        }
                        if t.is("[") {
                            level += 1;
                        } else if t.is("]") {
                            level -= 1;
                            if level == 0 {
                                k += 1;
                                break;
                            }
                        }
                        k += 1;
                    }
                }
                !self.peek_at(k).is("(") && !self.peek_at(k).is(".")
            }
            None => false,
        }
    }

    fn local_decl(&mut self) -> R<Vec<Stmt>> {
        let line = self.line();
        let q = self.qualifiers()?;
        if q.storage.is_some_and(|s| s != "const") || q.layout || q.interp.flat || q.interp.centroid {
            return err(line, "storage qualifiers other than 'const' are not allowed in functions");
        }
        let ty = self.type_spec()?;
        if self.eat(";") {
            return Ok(vec![]);
        }
        let nl = self.line();
        let name = self.ident()?;
        self.declarators(&q, ty, name, nl, false)
    }

    fn switch(&mut self) -> R<Stmt> {
        let line = self.line();
        self.i += 1;
        self.expect("(")?;
        let sel = self.expr()?;
        if !matches!(sel.ty, Ty::S(K::Int | K::Uint)) {
            return err(line, "'switch' : expression must be a scalar integer");
        }
        self.expect(")")?;
        self.expect("{")?;
        self.push_scope();
        self.switches += 1;
        let r = self.switch_body(&sel.ty);
        self.switches -= 1;
        self.pop_scope();
        let cases = r?;
        for (k, c) in cases.iter().enumerate() {
            let last = k + 1 == cases.len();
            let mut tail = c.body.last();
            while let Some(Stmt::Block(b)) = tail {
                tail = b.last();
            }
            let ends = matches!(tail, Some(Stmt::Break | Stmt::Return(_) | Stmt::Continue | Stmt::Discard));
            if !last && !c.body.is_empty() && !ends {
                return err(line, "fall-through from a non-empty case is not supported yet");
            }
        }
        Ok(Stmt::Switch(sel, cases))
    }

    fn switch_body(&mut self, sel: &Ty) -> R<Vec<Case>> {
        let mut cases: Vec<Case> = Vec::new();
        let mut seen: Vec<Option<CVal>> = Vec::new();
        loop {
            let line = self.line();
            if self.eat("}") {
                break;
            }
            if self.i >= self.t.len() {
                return err(line, "syntax error: unexpected end of file in switch");
            }
            let label = if self.eat("case") {
                let e = self.cond()?;
                self.expect(":")?;
                match self.fold(&e) {
                    Some(c) if c.ty() == *sel => Some(Some(c)),
                    _ => return err(line, "'case' : label must be a constant expression of the switch's type"),
                }
            } else if self.eat("default") {
                self.expect(":")?;
                Some(None)
            } else {
                None
            };
            match label {
                Some(l) => {
                    if seen.contains(&l) {
                        return err(line, "duplicate case label");
                    }
                    seen.push(l);
                    match cases.last_mut() {
                        Some(c) if c.body.is_empty() => c.labels.push(l),
                        _ => cases.push(Case { labels: vec![l], body: vec![] }),
                    }
                }
                None => {
                    let s = self.statement()?;
                    match cases.last_mut() {
                        Some(c) => c.body.extend(s),
                        None => return err(line, "statement before the first 'case' in a switch"),
                    }
                }
            }
        }
        Ok(cases)
    }
}
