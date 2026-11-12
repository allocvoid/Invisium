//! Function bodies, methods and classes.

use crate::ast::*;
use crate::lexer::Tok;
use crate::parser::{FnCtx, PResult, Parser};

impl<'a> Parser<'a> {
    /// Parses `(params) { body }` with a fresh function context.
    pub fn function_rest(&mut self, name: Option<String>, kind: FuncKind, start: usize, pos: Pos) -> PResult<Function> {
        self.function_rest_ex(name, kind, (false, false), start, pos)
    }

    /// `flags` = (is_async, is_generator).
    pub fn function_rest_ex(&mut self, name: Option<String>, kind: FuncKind, flags: (bool, bool), start: usize, pos: Pos) -> PResult<Function> {
        let saved = self.ctx;
        let saved_loop = (self.in_loop, self.in_switch, std::mem::take(&mut self.labels));
        self.in_loop = 0;
        self.in_switch = 0;
        self.ctx = FnCtx {
            in_function: true,
            strict: saved.strict,
            allow_super_call: kind == FuncKind::DerivedCtor,
            allow_super_prop: !matches!(kind, FuncKind::Normal),
            in_async: flags.0,
            in_gen: flags.1,
            new_target_ok: false,
        };
        let r = self.function_inner(name, kind, start, pos).map(|mut f| {
            f.is_async = flags.0;
            f.is_gen = flags.1;
            f
        });
        self.ctx = saved;
        self.in_loop = saved_loop.0;
        self.in_switch = saved_loop.1;
        self.labels = saved_loop.2;
        r
    }

    /// Parses the optional `async` and `*` method modifiers; returns (is_async, is_generator).
    pub fn method_modifiers(&mut self) -> PResult<(bool, bool)> {
        let mut flags = (false, false);
        if self.is_kw("async") {
            let next = self.peek_token();
            let is_modifier = match &next {
                Some(t) if !t.nl_before => matches!(t.tok, Tok::Ident(_) | Tok::Str(_) | Tok::Num(_) | Tok::Punct("[") | Tok::Punct("*") | Tok::Punct("#")),
                _ => false,
            };
            if is_modifier {
                self.advance()?;
                flags.0 = true;
            }
        }
        if self.eat("*")? {
            flags.1 = true;
        }
        Ok(flags)
    }

    fn function_inner(&mut self, name: Option<String>, kind: FuncKind, start: usize, pos: Pos) -> PResult<Function> {
        self.expect("(")?;
        let mut params = Vec::new();
        let mut rest = None;
        while !self.is(")") {
            if self.eat("...")? {
                rest = Some(self.binding_target()?);
                break;
            }
            params.push(self.binding_element()?);
            if !self.eat(",")? {
                break;
            }
        }
        self.expect(")")?;
        if kind == FuncKind::Getter && (!params.is_empty() || rest.is_some()) {
            return Err(self.error("Getter must not have any formal parameters"));
        }
        if kind == FuncKind::Setter && (params.len() != 1 || rest.is_some()) {
            return Err(self.error("Setter must have exactly one formal parameter"));
        }
        self.expect("{")?;
        let (body, strict) = self.parse_body_with_directives(false, false)?;
        self.expect("}")?;
        let simple = rest.is_none() && params.iter().all(|p| matches!(p, Pattern::Ident(..)));
        Ok(Function {
            name,
            self_binding: false,
            params,
            rest,
            body,
            kind,
            is_async: false,
            is_gen: false,
            strict,
            simple_params: simple,
            pos,
            source: (start, self.prev_end),
        })
    }

    pub fn method_rest(&mut self, kind: FuncKind, start: usize, pos: Pos, _is_class: bool) -> PResult<Function> {
        self.function_rest(None, kind, start, pos)
    }

    pub fn class(&mut self, is_decl: bool) -> PResult<Class> {
        let start = self.tok.start;
        let pos = self.pos();
        self.expect_kw("class")?;
        let saved_strict = self.ctx.strict;
        self.ctx.strict = true;
        let r = self.class_inner(is_decl, start, pos);
        self.ctx.strict = saved_strict;
        r
    }

    fn class_inner(&mut self, is_decl: bool, start: usize, pos: Pos) -> PResult<Class> {
        let name = if self.is_ident_ref() && !self.is_kw("extends") {
            Some(self.ident_name()?)
        } else if is_decl {
            return Err(self.error("Class name required"));
        } else {
            None
        };
        let extends = if self.eat_kw("extends")? {
            Some(self.lhs_for_extends()?)
        } else {
            None
        };
        let derived = extends.is_some();
        self.expect("{")?;
        let mut ctor = None;
        let mut members = Vec::new();
        while !self.is("}") {
            if self.eat(";")? {
                continue;
            }
            let mut mstart = self.tok.start;
            let mpos = self.pos();
            let mut is_static = false;
            if self.is_kw("static") && (self.is_modifier_followed_by_name() || matches!(self.peek_tok(), Tok::Punct("{") | Tok::Punct("*"))) {
                self.advance()?;
                is_static = true;
                // a method's source text starts after `static`
                mstart = self.tok.start;
                if self.is("{") {
                    self.advance()?;
                    let saved = self.ctx;
                    self.ctx = FnCtx { in_function: true, strict: true, allow_super_prop: true, ..FnCtx::default() };
                    let body = self.block_body();
                    self.ctx = saved;
                    members.push(ClassMember { is_static, key: PropName::Ident(String::new()), kind: MemberKind::StaticBlock(body?) });
                    continue;
                }
            }
            let flags = self.method_modifiers()?;
            if flags.0 || flags.1 {
                let key = self.member_name()?;
                if !is_static && matches!(&key, PropName::Ident(n) if n == "constructor") {
                    return Err(self.error("Class constructor may not be a generator or an async method"));
                }
                let f = self.function_rest_ex(None, FuncKind::Method, flags, mstart, mpos)?;
                members.push(ClassMember { is_static, key, kind: MemberKind::Method(Box::new(f)) });
                continue;
            }
            if (self.is_kw("get") || self.is_kw("set")) && self.is_modifier_followed_by_name() {
                let is_get = self.is_kw("get");
                self.advance()?;
                let key = self.member_name()?;
                let kind = if is_get { FuncKind::Getter } else { FuncKind::Setter };
                let f = Box::new(self.function_rest(None, kind, mstart, mpos)?);
                members.push(ClassMember { is_static, key, kind: if is_get { MemberKind::Get(f) } else { MemberKind::Set(f) } });
                continue;
            }
            let key = self.member_name()?;
            if self.is("(") {
                let is_ctor = !is_static && matches!(&key, PropName::Ident(n) if n == "constructor")
                    || !is_static && matches!(&key, PropName::Str(s) if *s == "constructor".encode_utf16().collect::<Vec<u16>>());
                if is_ctor {
                    if ctor.is_some() {
                        return Err(self.error("A class may only have one constructor"));
                    }
                    let kind = if derived { FuncKind::DerivedCtor } else { FuncKind::BaseCtor };
                    let mut f = self.function_rest(name.clone(), kind, start, mpos)?;
                    f.source = (start, 0);
                    ctor = Some(Box::new(f));
                } else {
                    let f = self.function_rest(None, FuncKind::Method, mstart, mpos)?;
                    members.push(ClassMember { is_static, key, kind: MemberKind::Method(Box::new(f)) });
                }
                continue;
            }
            // field
            let init = if self.eat("=")? {
                let saved = self.ctx;
                self.ctx = FnCtx { in_function: true, strict: true, allow_super_prop: true, ..FnCtx::default() };
                let e = self.assign(false);
                self.ctx = saved;
                Some(e?)
            } else {
                None
            };
            self.semicolon()?;
            members.push(ClassMember { is_static, key, kind: MemberKind::Field(init) });
        }
        self.expect("}")?;
        if let Some(c) = ctor.as_mut() {
            c.source = (start, self.prev_end);
        }
        Ok(Class { name, extends, ctor, members, pos, source: (start, self.prev_end) })
    }

    fn lhs_for_extends(&mut self) -> PResult<Expr> {
        // LeftHandSideExpression: parse a unary-level expression and reject binary continuations.
        let off = self.tok.start;
        let e = self.assign(false)?;
        match e {
            Expr::Binary(..) | Expr::Logical(..) | Expr::Assign(..) | Expr::Cond(..) | Expr::Seq(..) => {
                Err(self.error_at("Invalid class heritage", off))
            }
            e => Ok(e),
        }
    }
}
