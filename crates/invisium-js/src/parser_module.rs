//! Module syntax: `import` / `export` declarations (collected into `ModuleInfo`, their
//! declarations kept as ordinary statements), `import(...)` and `import.meta`.
//!
//! `import.meta` reads the hidden module binding `*meta*`; `import(x)` calls the hidden global
//! `*import*` (names no source text can spell). A default export of an expression becomes the
//! constant binding `default`, which no source text can reference either.

use crate::analyze::pattern_names;
use crate::ast::*;
use crate::lexer::Tok;
use crate::parser::{PResult, Parser};

impl<'a> Parser<'a> {
    /// `import(...)` or `import.meta`, with `import` the current token.
    pub fn import_expr(&mut self, pos: Pos) -> PResult<Expr> {
        self.advance()?;
        if self.eat(".")? {
            let name = self.any_name()?;
            if name != "meta" || self.module.is_none() {
                return Err(self.error("Cannot use 'import.meta' outside a module"));
            }
            return Ok(Expr::Ident("*meta*".into(), pos));
        }
        self.expect("(")?;
        let spec = self.assign(false)?;
        let mut args = vec![ArrayItem::Expr(spec)];
        if self.eat(",")? && !self.is(")") {
            // import attributes are evaluated and ignored
            args.push(ArrayItem::Expr(self.assign(false)?));
            self.eat(",")?;
        }
        self.expect(")")?;
        Ok(Expr::Call { callee: Box::new(Expr::Ident("*import*".into(), pos)), args, optional: false, pos })
    }

    fn module_request(&mut self) -> PResult<usize> {
        let Tok::Str(s) = &self.tok.tok else { return Err(self.unexpected()) };
        let spec = String::from_utf16_lossy(s);
        self.advance()?;
        let info = self.module.get_or_insert_with(ModuleInfo::default);
        if let Some(i) = info.requests.iter().position(|r| *r == spec) {
            return Ok(i);
        }
        info.requests.push(spec);
        Ok(info.requests.len() - 1)
    }

    /// `from 'specifier'` plus the optional `with { ... }` attributes clause.
    fn parse_from(&mut self) -> PResult<usize> {
        self.expect_kw("from")?;
        let r = self.module_request()?;
        self.attributes()?;
        Ok(r)
    }

    fn attributes(&mut self) -> PResult<()> {
        if (self.is_kw("with") || self.is_kw("assert")) && !self.tok.nl_before {
            self.advance()?;
            self.expect("{")?;
            while !self.is("}") {
                self.module_export_name()?;
                self.expect(":")?;
                if !matches!(self.tok.tok, Tok::Str(_)) {
                    return Err(self.unexpected());
                }
                self.advance()?;
                if !self.eat(",")? {
                    break;
                }
            }
            self.expect("}")?;
        }
        Ok(())
    }

    /// An identifier name or a string literal (`export { x as "a-b" }`).
    fn module_export_name(&mut self) -> PResult<(String, bool)> {
        match &self.tok.tok {
            Tok::Str(s) => {
                let s = String::from_utf16_lossy(s);
                self.advance()?;
                Ok((s, true))
            }
            Tok::Ident(_) => Ok((self.any_name()?, false)),
            _ => Err(self.unexpected()),
        }
    }

    fn info(&mut self) -> &mut ModuleInfo {
        self.module.get_or_insert_with(ModuleInfo::default)
    }

    pub fn import_declaration(&mut self) -> PResult<()> {
        self.advance()?;
        if matches!(self.tok.tok, Tok::Str(_)) {
            self.module_request()?;
            self.attributes()?;
            return self.semicolon();
        }
        // (import name or None for the namespace, local name, position)
        let mut bindings: Vec<(Option<String>, String, Pos)> = Vec::new();
        let mut more = true;
        if self.is_ident_ref() {
            let p = self.pos();
            bindings.push((Some("default".into()), self.ident_name()?, p));
            more = self.eat(",")?;
        }
        if more {
            if self.eat("*")? {
                self.expect_kw("as")?;
                let p = self.pos();
                bindings.push((None, self.ident_name()?, p));
            } else if self.eat("{")? {
                while !self.is("}") {
                    let p = self.pos();
                    let (name, quoted) = self.module_export_name()?;
                    let local = if self.eat_kw("as")? {
                        self.ident_name()?
                    } else if quoted || crate::parser::is_reserved(&name, true) || name == "eval" || name == "arguments" {
                        return Err(self.error("Unexpected import name"));
                    } else {
                        name.clone()
                    };
                    bindings.push((Some(name), local, p));
                    if !self.eat(",")? {
                        break;
                    }
                }
                self.expect("}")?;
            } else {
                return Err(self.unexpected());
            }
        }
        let request = self.parse_from()?;
        self.semicolon()?;
        for (import_name, local, pos) in bindings {
            self.info().imports.push(ImportEntry { request, import_name, local, pos });
        }
        Ok(())
    }

    fn export_local(&mut self, name: &str, local: &str, pos: Pos) {
        self.info().exports.push(ExportEntry::Local { name: name.into(), local: local.into(), pos });
    }

    /// Returns the declaration an `export` carries, if any.
    pub fn export_declaration(&mut self) -> PResult<Option<Stmt>> {
        let pos = self.pos();
        self.advance()?;
        if self.eat("*")? {
            if self.eat_kw("as")? {
                let (name, _) = self.module_export_name()?;
                let request = self.parse_from()?;
                self.info().exports.push(ExportEntry::Indirect { name, request, import_name: None, pos });
            } else {
                let request = self.parse_from()?;
                self.info().exports.push(ExportEntry::Star { request, pos });
            }
            self.semicolon()?;
            return Ok(None);
        }
        if self.eat_kw("default")? {
            return self.export_default(pos).map(Some);
        }
        if self.eat("{")? {
            let mut specs: Vec<(String, bool, String, Pos)> = Vec::new();
            while !self.is("}") {
                let p = self.pos();
                let (local, quoted) = self.module_export_name()?;
                let name = if self.eat_kw("as")? { self.module_export_name()?.0 } else { local.clone() };
                specs.push((local, quoted, name, p));
                if !self.eat(",")? {
                    break;
                }
            }
            self.expect("}")?;
            if self.is_kw("from") {
                let request = self.parse_from()?;
                for (local, _, name, p) in specs {
                    self.info().exports.push(ExportEntry::Indirect { name, request, import_name: Some(local), pos: p });
                }
            } else {
                for (local, quoted, name, p) in specs {
                    if quoted || crate::parser::is_reserved(&local, true) {
                        return Err(self.error_at_pos("Unexpected export name", p));
                    }
                    self.export_local(&name, &local, p);
                }
            }
            self.semicolon()?;
            return Ok(None);
        }
        let is_decl = self.is_kw("var") || self.is_kw("let") || self.is_kw("const") || self.is_kw("function") || self.is_async_function() || self.is_kw("class");
        if !is_decl {
            return Err(self.unexpected());
        }
        let stmt = if self.is_kw("var") { self.statement()? } else { self.statement_list_item()? };
        let mut names = Vec::new();
        match &stmt.kind {
            StmtKind::Var(_, decls) => decls.iter().for_each(|(p, _)| pattern_names(p, &mut names)),
            StmtKind::Func(f) => names.extend(f.name.clone()),
            StmtKind::Class(c) => names.extend(c.name.clone()),
            _ => return Err(self.unexpected()),
        }
        for n in names {
            self.export_local(&n, &n, pos);
        }
        Ok(Some(stmt))
    }

    fn export_default(&mut self, pos: Pos) -> PResult<Stmt> {
        if self.is_kw("function") || self.is_async_function() {
            let start = self.tok.start;
            let is_async = self.is_kw("async");
            if is_async {
                self.advance()?;
            }
            self.advance()?;
            let is_gen = self.eat("*")?;
            let name = if self.is_ident_ref() { self.ident_name()? } else { "default".to_string() };
            let f = self.function_rest_ex(Some(name.clone()), FuncKind::Normal, (is_async, is_gen), start, pos)?;
            self.export_local("default", &name, pos);
            return Ok(Stmt { kind: StmtKind::Func(Box::new(f)), pos });
        }
        if self.is_kw("class") {
            let named = matches!(self.peek_token(), Some(t) if matches!(&t.tok, Tok::Ident(n) if n != "extends"));
            let mut c = self.class(named)?;
            let name = c.name.get_or_insert_with(|| "default".to_string()).clone();
            self.export_local("default", &name, pos);
            return Ok(Stmt { kind: StmtKind::Class(Box::new(c)), pos });
        }
        let e = self.assign(false)?;
        self.semicolon()?;
        self.export_local("default", "default", pos);
        Ok(Stmt { kind: StmtKind::Var(VarKind::Const, vec![(Pattern::Ident("default".into(), pos), Some(e))]), pos })
    }

    fn error_at_pos(&self, msg: &str, p: Pos) -> crate::parser::ParseError {
        crate::parser::ParseError { message: msg.to_string(), line: p.line, column: p.col }
    }
}
