//! Statements, declarations, binding patterns, functions and classes.

use crate::ast::*;
use crate::lexer::Tok;
use crate::parser::{PResult, Parser};

impl<'a> Parser<'a> {
    pub fn statement_list_item(&mut self) -> PResult<Stmt> {
        let pos = self.pos();
        if self.is_kw("function") || self.is_async_function() {
            let start = self.tok.start;
            let is_async = self.is_kw("async");
            if is_async {
                self.advance()?;
            }
            self.advance()?;
            let is_gen = self.eat("*")?;
            let name = self.ident_name()?;
            let f = self.function_rest_ex(Some(name), FuncKind::Normal, (is_async, is_gen), start, pos)?;
            return Ok(Stmt { kind: StmtKind::Func(Box::new(f)), pos });
        }
        if self.is_kw("class") {
            let c = self.class(true)?;
            return Ok(Stmt { kind: StmtKind::Class(Box::new(c)), pos });
        }
        if self.is_kw("const") || (self.is_kw("let") && self.let_is_decl()) {
            let kind = if self.is_kw("const") { VarKind::Const } else { VarKind::Let };
            self.advance()?;
            let decls = self.var_decls(kind, false)?;
            self.semicolon()?;
            return Ok(Stmt { kind: StmtKind::Var(kind, decls), pos });
        }
        self.statement()
    }

    fn let_is_decl(&self) -> bool {
        match self.peek_tok() {
            Tok::Ident(s) => !matches!(s.as_str(), "in" | "instanceof" | "of"),
            Tok::Punct("[") | Tok::Punct("{") => true,
            _ => false,
        }
    }

    pub fn var_decls(&mut self, kind: VarKind, no_in: bool) -> PResult<Vec<(Pattern, Option<Expr>)>> {
        let mut out = Vec::new();
        loop {
            let target = self.binding_target()?;
            let init = if self.eat("=")? { Some(self.assign(no_in)?) } else { None };
            out.push((target, init));
            if !self.eat(",")? {
                break;
            }
        }
        // missing initialisers are checked by callers that know the context (for-in/of heads)
        let _ = kind;
        Ok(out)
    }

    pub fn binding_target(&mut self) -> PResult<Pattern> {
        self.enter()?;
        let r = self.binding_target_inner();
        self.leave();
        r
    }

    fn binding_target_inner(&mut self) -> PResult<Pattern> {
        let pos = self.pos();
        if self.eat("[")? {
            let mut elems = Vec::new();
            let mut rest = None;
            loop {
                if self.is("]") {
                    break;
                }
                if self.eat(",")? {
                    elems.push(None);
                    continue;
                }
                if self.eat("...")? {
                    rest = Some(Box::new(self.binding_target()?));
                    break;
                }
                elems.push(Some(self.binding_element()?));
                if !self.is("]") {
                    self.expect(",")?;
                }
            }
            self.expect("]")?;
            return Ok(Pattern::Array { elems, rest });
        }
        if self.eat("{")? {
            let mut props = Vec::new();
            let mut rest = None;
            while !self.is("}") {
                if self.eat("...")? {
                    let p = self.pos();
                    rest = Some(Box::new(Pattern::Ident(self.ident_name()?, p)));
                    break;
                }
                let p = self.pos();
                let is_ident = matches!(self.tok.tok, Tok::Ident(_));
                let key = self.property_name()?;
                if self.eat(":")? {
                    props.push((key, self.binding_element()?));
                } else if is_ident {
                    let PropName::Ident(name) = &key else { return Err(self.unexpected()) };
                    if crate::parser::is_reserved(name, self.ctx.strict) {
                        return Err(self.unexpected());
                    }
                    let mut pat = Pattern::Ident(name.clone(), p);
                    if self.eat("=")? {
                        pat = Pattern::Default(Box::new(pat), Box::new(self.assign(false)?));
                    }
                    props.push((key, pat));
                } else {
                    return Err(self.unexpected());
                }
                if !self.is("}") {
                    self.expect(",")?;
                }
            }
            self.expect("}")?;
            return Ok(Pattern::Object { props, rest });
        }
        let name = self.ident_name()?;
        if self.ctx.strict && (name == "eval" || name == "arguments") {
            return Err(self.error("Unexpected eval or arguments in strict mode"));
        }
        Ok(Pattern::Ident(name, pos))
    }

    pub fn binding_element(&mut self) -> PResult<Pattern> {
        let t = self.binding_target()?;
        if self.eat("=")? {
            return Ok(Pattern::Default(Box::new(t), Box::new(self.assign(false)?)));
        }
        Ok(t)
    }

    pub fn statement(&mut self) -> PResult<Stmt> {
        self.enter()?;
        let r = self.statement_inner();
        self.leave();
        r
    }

    fn sub_statement(&mut self) -> PResult<Stmt> {
        if self.is_kw("class") || self.is_kw("const") || (self.is_kw("let") && matches!(self.peek_tok(), Tok::Punct("["))) {
            return Err(self.error("Lexical declaration cannot appear in a single-statement context"));
        }
        if self.is_kw("function") {
            let s = self.statement_list_item()?;
            return Ok(Stmt { pos: s.pos, kind: StmtKind::Block(vec![s]) });
        }
        self.statement()
    }

    fn loop_body(&mut self) -> PResult<Stmt> {
        self.in_loop += 1;
        let r = self.sub_statement();
        self.in_loop -= 1;
        r
    }

    fn statement_inner(&mut self) -> PResult<Stmt> {
        let pos = self.pos();
        let kind = match self.tok.tok.clone() {
            Tok::Punct("{") => {
                self.advance()?;
                let b = self.block_body()?;
                StmtKind::Block(b)
            }
            Tok::Punct(";") => {
                self.advance()?;
                StmtKind::Empty
            }
            Tok::Ident(k) if !self.tok.escaped => match k.as_str() {
                "var" => {
                    self.advance()?;
                    let d = self.var_decls(VarKind::Var, false)?;
                    self.semicolon()?;
                    StmtKind::Var(VarKind::Var, d)
                }
                "if" => {
                    self.advance()?;
                    self.expect("(")?;
                    let t = self.expression(false)?;
                    self.expect(")")?;
                    let c = self.sub_statement()?;
                    let a = if self.eat_kw("else")? { Some(Box::new(self.sub_statement()?)) } else { None };
                    StmtKind::If(t, Box::new(c), a)
                }
                "for" => return self.for_statement(pos),
                "while" => {
                    self.advance()?;
                    self.expect("(")?;
                    let t = self.expression(false)?;
                    self.expect(")")?;
                    StmtKind::While(t, Box::new(self.loop_body()?))
                }
                "do" => {
                    self.advance()?;
                    let body = self.loop_body()?;
                    self.expect_kw("while")?;
                    self.expect("(")?;
                    let t = self.expression(false)?;
                    self.expect(")")?;
                    self.eat(";")?;
                    StmtKind::DoWhile(Box::new(body), t)
                }
                "break" | "continue" => {
                    let is_break = k == "break";
                    self.advance()?;
                    let label = if !self.tok.nl_before && self.is_ident_ref() {
                        let l = self.ident_name()?;
                        match self.labels.iter().rev().find(|(n, _)| *n == l) {
                            None => return Err(self.error(&format!("Undefined label '{}'", l))),
                            Some((_, is_loop)) if !is_break && !*is_loop => return Err(self.error(&format!("Illegal continue statement: '{}' does not denote an iteration statement", l))),
                            _ => {}
                        }
                        Some(l)
                    } else {
                        if is_break && self.in_loop == 0 && self.in_switch == 0 {
                            return Err(self.error("Illegal break statement"));
                        }
                        if !is_break && self.in_loop == 0 {
                            return Err(self.error("Illegal continue statement: no surrounding iteration statement"));
                        }
                        None
                    };
                    self.semicolon()?;
                    if is_break { StmtKind::Break(label) } else { StmtKind::Continue(label) }
                }
                "return" => {
                    if !self.ctx.in_function {
                        return Err(self.error("Illegal return statement"));
                    }
                    self.advance()?;
                    let arg = if self.is(";") || self.is("}") || self.tok.tok == Tok::Eof || self.tok.nl_before {
                        None
                    } else {
                        Some(self.expression(false)?)
                    };
                    self.semicolon()?;
                    StmtKind::Return(arg)
                }
                "throw" => {
                    self.advance()?;
                    if self.tok.nl_before {
                        return Err(self.error("Illegal newline after throw"));
                    }
                    let e = self.expression(false)?;
                    self.semicolon()?;
                    StmtKind::Throw(e)
                }
                "try" => self.try_statement()?,
                "switch" => {
                    self.advance()?;
                    self.expect("(")?;
                    let d = self.expression(false)?;
                    self.expect(")")?;
                    self.expect("{")?;
                    let mut cases = Vec::new();
                    let mut seen_default = false;
                    self.in_switch += 1;
                    while !self.is("}") {
                        let test = if self.eat_kw("default")? {
                            if seen_default {
                                return Err(self.error("More than one default clause in switch statement"));
                            }
                            seen_default = true;
                            None
                        } else {
                            self.expect_kw("case")?;
                            Some(self.expression(false)?)
                        };
                        self.expect(":")?;
                        let mut body = Vec::new();
                        while !self.is("}") && !self.is_kw("case") && !self.is_kw("default") {
                            if self.tok.tok == Tok::Eof {
                                return Err(self.unexpected());
                            }
                            body.push(self.statement_list_item()?);
                        }
                        cases.push(Case { test, body });
                    }
                    self.in_switch -= 1;
                    self.advance()?;
                    StmtKind::Switch(d, cases)
                }
                "debugger" => {
                    self.advance()?;
                    self.semicolon()?;
                    StmtKind::Debugger
                }
                "with" => {
                    if self.ctx.strict {
                        return Err(self.error("Strict mode code may not include a with statement"));
                    }
                    self.advance()?;
                    self.expect("(")?;
                    let obj = self.expression(false)?;
                    self.expect(")")?;
                    StmtKind::With(obj, Box::new(self.sub_statement()?))
                }
                "import" if matches!(self.peek_tok(), Tok::Punct("(") | Tok::Punct(".")) => return self.expression_or_label(pos),
                "import" | "export" => return Err(self.error("Cannot use import/export outside a module")),
                "function" | "class" | "const" => return self.statement_list_item(),
                "let" if self.let_is_decl() => return self.statement_list_item(),
                _ => return self.expression_or_label(pos),
            },
            _ => return self.expression_or_label(pos),
        };
        Ok(Stmt { kind, pos })
    }

    fn expression_or_label(&mut self, pos: Pos) -> PResult<Stmt> {
        if self.is_ident_ref() && matches!(self.peek_tok(), Tok::Punct(":")) {
            let label = self.ident_name()?;
            self.expect(":")?;
            let is_loop = self.is_kw("for") || self.is_kw("while") || self.is_kw("do");
            self.labels.push((label.clone(), is_loop));
            let body = if self.is_kw("function") { self.statement_list_item() } else { self.statement() };
            self.labels.pop();
            return Ok(Stmt { kind: StmtKind::Labeled(label, Box::new(body?)), pos });
        }
        let e = self.expression(false)?;
        self.semicolon()?;
        Ok(Stmt { kind: StmtKind::Expr(e), pos })
    }

    pub fn block_body(&mut self) -> PResult<Vec<Stmt>> {
        let mut body = Vec::new();
        while !self.is("}") {
            if self.tok.tok == Tok::Eof {
                return Err(self.unexpected());
            }
            body.push(self.statement_list_item()?);
        }
        self.advance()?;
        Ok(body)
    }

    fn try_statement(&mut self) -> PResult<StmtKind> {
        self.advance()?;
        self.expect("{")?;
        let block = self.block_body()?;
        let mut param = None;
        let mut handler = None;
        if self.eat_kw("catch")? {
            if self.eat("(")? {
                param = Some(self.binding_target()?);
                self.expect(")")?;
            }
            self.expect("{")?;
            handler = Some(self.block_body()?);
        }
        let finalizer = if self.eat_kw("finally")? {
            self.expect("{")?;
            Some(self.block_body()?)
        } else {
            None
        };
        if handler.is_none() && finalizer.is_none() {
            return Err(self.error("Missing catch or finally after try"));
        }
        Ok(StmtKind::Try { block, param, handler, finalizer })
    }

    fn for_statement(&mut self, pos: Pos) -> PResult<Stmt> {
        self.advance()?;
        let is_await = self.is_kw("await");
        if is_await {
            if !self.ctx.in_async {
                return Err(self.error("for await is only valid in async functions"));
            }
            if !self.ctx.in_function {
                self.top_await = true;
            }
            self.advance()?;
        }
        let for_of = |head: ForHead, right: Expr, body: Stmt| {
            if is_await { StmtKind::ForAwait(head, right, Box::new(body)) } else { StmtKind::ForOf(head, right, Box::new(body)) }
        };
        self.expect("(")?;
        let mut init = None;
        if self.is(";") {
        } else if self.is_kw("var") || self.is_kw("const") || (self.is_kw("let") && self.let_is_decl()) {
            let kind = if self.is_kw("var") { VarKind::Var } else if self.is_kw("const") { VarKind::Const } else { VarKind::Let };
            self.advance()?;
            let decls = self.var_decls(kind, true)?;
            if decls.len() == 1 && decls[0].1.is_none() && (self.is_kw("of") || self.is_kw("in")) {
                let is_of = self.is_kw("of");
                self.advance()?;
                let right = if is_of { self.assign(false)? } else { self.expression(false)? };
                self.expect(")")?;
                let body = self.loop_body()?;
                let head = ForHead::Var(kind, decls.into_iter().next().map(|d| d.0).unwrap_or(Pattern::Ident(String::new(), pos)));
                if is_await && !is_of {
                    return Err(self.error("for await requires 'of'"));
                }
                let kind = if is_of { for_of(head, right, body) } else { StmtKind::ForIn(head, right, Box::new(body)) };
                return Ok(Stmt { kind, pos });
            }
            init = Some(ForInit::Var(kind, decls));
        } else {
            let off = self.tok.start;
            let e = self.expression(true)?;
            if self.is_kw("of") || self.is_kw("in") {
                let is_of = self.is_kw("of");
                let target = self.to_pattern(e, false, off)?;
                self.advance()?;
                let right = if is_of { self.assign(false)? } else { self.expression(false)? };
                self.expect(")")?;
                let body = self.loop_body()?;
                let head = ForHead::Pattern(target);
                if is_await && !is_of {
                    return Err(self.error("for await requires 'of'"));
                }
                let kind = if is_of { for_of(head, right, body) } else { StmtKind::ForIn(head, right, Box::new(body)) };
                return Ok(Stmt { kind, pos });
            }
            init = Some(ForInit::Expr(e));
        }
        if is_await {
            return Err(self.error("for await requires 'of'"));
        }
        self.expect(";")?;
        let test = if self.is(";") { None } else { Some(self.expression(false)?) };
        self.expect(";")?;
        let update = if self.is(")") { None } else { Some(self.expression(false)?) };
        self.expect(")")?;
        let body = self.loop_body()?;
        Ok(Stmt { kind: StmtKind::For { init, test, update, body: Box::new(body) }, pos })
    }
}
