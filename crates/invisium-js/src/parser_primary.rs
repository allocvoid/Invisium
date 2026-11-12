//! Primary expressions: literals, identifiers, parenthesised/arrow, array, object, templates.

use crate::ast::*;
use crate::lexer::Tok;
use crate::parser::{PResult, Parser};

impl<'a> Parser<'a> {
    pub fn primary(&mut self) -> PResult<Expr> {
        let pos = self.pos();
        let start = self.tok.start;
        match self.tok.tok.clone() {
            Tok::Num(n) => {
                if self.tok.legacy_octal && self.ctx.strict {
                    return Err(self.error("Octal literals are not allowed in strict mode"));
                }
                self.advance()?;
                Ok(Expr::Num(n))
            }
            Tok::BigInt(text) => {
                self.advance()?;
                Ok(Expr::BigInt(text))
            }
            Tok::Str(s) => {
                if self.tok.legacy_octal && self.ctx.strict {
                    return Err(self.error("Octal escape sequences are not allowed in strict mode"));
                }
                self.advance()?;
                Ok(Expr::Str(s))
            }
            Tok::Template { .. } => Ok(Expr::Template(Box::new(self.template_parts(false)?))),
            Tok::Punct("/") | Tok::Punct("/=") => {
                self.rescan_regex()?;
                if let Tok::Regex { pattern, flags } = self.tok.tok.clone() {
                    // an invalid pattern or flag is an early error, as for any other syntax
                    let Some(f) = invisium_regex::Flags::parse(&flags) else {
                        return Err(self.error("Invalid regular expression flags"));
                    };
                    let units: Vec<u16> = pattern.encode_utf16().collect();
                    if let Err(e) = invisium_regex::Regex::new(&units, f) {
                        return Err(self.error(&format!("Invalid regular expression: {}", e.message)));
                    }
                    self.advance()?;
                    Ok(Expr::Regex(pattern, flags))
                } else {
                    Err(self.unexpected())
                }
            }
            Tok::Punct("(") => self.paren_or_arrow(start, pos, false),
            Tok::Punct("[") => self.array_literal(),
            Tok::Punct("{") => self.object_literal(),
            Tok::Ident(ref s) if !self.tok.escaped => match s.as_str() {
                "this" => {
                    self.advance()?;
                    Ok(Expr::This)
                }
                "import" => self.import_expr(pos),
                "null" => {
                    self.advance()?;
                    Ok(Expr::Null)
                }
                "true" | "false" => {
                    let b = s == "true";
                    self.advance()?;
                    Ok(Expr::Bool(b))
                }
                "function" => self.function_expr(false, start, pos),
                "async" if self.is_async_function() => {
                    self.advance()?;
                    self.function_expr(true, start, pos)
                }
                "async" if self.async_arrow_ahead() => {
                    self.advance()?;
                    if self.is("(") {
                        return self.paren_or_arrow(start, pos, true);
                    }
                    let ppos = self.pos();
                    let name = self.ident_name()?;
                    self.expect("=>")?;
                    self.arrow_body(vec![Pattern::Ident(name, ppos)], None, start, pos, true)
                }
                "class" => Ok(Expr::Class(Box::new(self.class(false)?))),
                "super" => {
                    self.advance()?;
                    if self.is("(") {
                        if !self.ctx.allow_super_call {
                            return Err(self.error("'super' keyword unexpected here"));
                        }
                        let args = self.arguments()?;
                        return Ok(Expr::SuperCall(args, pos));
                    }
                    if !self.ctx.allow_super_prop {
                        return Err(self.error("'super' keyword unexpected here"));
                    }
                    if self.eat(".")? {
                        let n = self.any_name()?;
                        Ok(Expr::SuperMember(n, pos))
                    } else if self.eat("[")? {
                        let e = self.expression(false)?;
                        self.expect("]")?;
                        Ok(Expr::SuperIndex(Box::new(e), pos))
                    } else {
                        Err(self.error("'super' keyword unexpected here"))
                    }
                }
                _ => {
                    let n = self.ident_name()?;
                    Ok(Expr::Ident(n, pos))
                }
            },
            Tok::Ident(_) => {
                let n = self.ident_name()?;
                Ok(Expr::Ident(n, pos))
            }
            _ => Err(self.unexpected()),
        }
    }

    /// `function` / `function*` expression; the current token is `function`.
    fn function_expr(&mut self, is_async: bool, start: usize, pos: Pos) -> PResult<Expr> {
        self.advance()?;
        let is_gen = self.eat("*")?;
        let name = if self.is_ident_ref() { Some(self.ident_name()?) } else { None };
        let has_name = name.is_some();
        let mut f = self.function_rest_ex(name, FuncKind::Normal, (is_async, is_gen), start, pos)?;
        f.self_binding = has_name;
        Ok(Expr::Function(Box::new(f)))
    }

    /// At `async`: is it followed on the same line by `ident =>` or by `(`?
    /// (`async (…)` without `=>` after the parentheses is an ordinary call of `async`.)
    fn async_arrow_ahead(&self) -> bool {
        match self.peek2() {
            (Some(a), b) if !a.nl_before => match (&a.tok, b.map(|t| t.tok)) {
                (Tok::Punct("("), _) => true,
                (Tok::Ident(k), Some(Tok::Punct("=>"))) => k != "function",
                _ => false,
            },
            _ => false,
        }
    }

    /// Parenthesised expression or arrow function. With `after_async` the caller consumed an
    /// `async` identifier: the result is an async arrow, or a call `async(...)`.
    fn paren_or_arrow(&mut self, start: usize, pos: Pos, after_async: bool) -> PResult<Expr> {
        self.expect("(")?;
        let mut items: Vec<Expr> = Vec::new();
        let mut rest: Option<Pattern> = None;
        let mut trailing_comma = false;
        while !self.is(")") {
            if self.eat("...")? {
                rest = Some(self.binding_target()?);
                break;
            }
            items.push(self.assign(false)?);
            if !self.eat(",")? {
                break;
            }
            if self.is(")") {
                trailing_comma = true;
            }
        }
        self.expect(")")?;
        let is_arrow = self.is("=>") && !self.tok.nl_before;
        if is_arrow {
            self.advance()?;
            let mut params = Vec::new();
            for e in items {
                params.push(self.to_pattern(e, true, start)?);
            }
            return self.arrow_body(params, rest, start, pos, after_async);
        }
        if after_async && rest.is_none() {
            let args = items.into_iter().map(ArrayItem::Expr).collect();
            return Ok(Expr::Call { callee: Box::new(Expr::Ident("async".into(), pos)), args, optional: false, pos });
        }
        if items.is_empty() || rest.is_some() || trailing_comma {
            return Err(self.error_at("Unexpected token ')'", self.prev_end.saturating_sub(1)));
        }
        let inner = if items.len() == 1 { items.pop().unwrap_or(Expr::Null) } else { Expr::Seq(items) };
        Ok(Expr::Paren(Box::new(inner)))
    }

    pub fn arrow_body(&mut self, params: Vec<Pattern>, rest: Option<Pattern>, start: usize, pos: Pos, is_async: bool) -> PResult<Expr> {
        let saved = self.ctx;
        let saved_loop = (self.in_loop, self.in_switch, std::mem::take(&mut self.labels));
        self.in_loop = 0;
        self.in_switch = 0;
        self.ctx.in_function = true;
        self.ctx.in_async = is_async;
        self.ctx.in_gen = false;
        let simple = rest.is_none() && params.iter().all(|p| matches!(p, Pattern::Ident(..)));
        let (body, strict) = if self.is("{") {
            self.advance()?;
            let r = self.parse_body_with_directives(false, false)?;
            self.expect("}")?;
            r
        } else {
            let p = self.pos();
            let e = self.assign(false)?;
            (vec![Stmt { kind: StmtKind::Return(Some(e)), pos: p }], self.ctx.strict)
        };
        self.ctx = saved;
        self.in_loop = saved_loop.0;
        self.in_switch = saved_loop.1;
        self.labels = saved_loop.2;
        Ok(Expr::Function(Box::new(Function {
            name: None,
            self_binding: false,
            params,
            rest,
            body,
            kind: FuncKind::Arrow,
            is_async,
            is_gen: false,
            strict,
            simple_params: simple,
            pos,
            source: (start, self.prev_end),
        })))
    }

    fn array_literal(&mut self) -> PResult<Expr> {
        self.expect("[")?;
        let mut items = Vec::new();
        loop {
            if self.is("]") {
                break;
            }
            if self.is(",") {
                self.advance()?;
                items.push(ArrayItem::Hole);
                continue;
            }
            if self.eat("...")? {
                items.push(ArrayItem::Spread(self.assign(false)?));
            } else {
                items.push(ArrayItem::Expr(self.assign(false)?));
            }
            if !self.is("]") {
                self.expect(",")?;
            }
        }
        self.expect("]")?;
        Ok(Expr::Array(items))
    }

    pub fn property_name(&mut self) -> PResult<PropName> {
        match self.tok.tok.clone() {
            Tok::Ident(s) => {
                self.advance()?;
                Ok(PropName::Ident(s))
            }
            Tok::Str(s) => {
                self.advance()?;
                Ok(PropName::Str(s))
            }
            Tok::Num(n) => {
                self.advance()?;
                Ok(PropName::Num(n))
            }
            Tok::Punct("[") => {
                self.advance()?;
                let e = self.assign(false)?;
                self.expect("]")?;
                Ok(PropName::Computed(Box::new(e)))
            }
            Tok::Punct("#") => Err(self.error("Unexpected private name")),
            _ => Err(self.unexpected()),
        }
    }

    /// A class member name: a property name or `#private`.
    pub fn member_name(&mut self) -> PResult<PropName> {
        if self.eat("#")? {
            return Ok(PropName::Private(self.any_name()?));
        }
        self.property_name()
    }

    /// True when the current `get`/`set`/`static` keyword is actually a modifier.
    pub fn is_modifier_followed_by_name(&self) -> bool {
        match self.peek_tok() {
            Tok::Ident(_) | Tok::Str(_) | Tok::Num(_) | Tok::Punct("[") | Tok::Punct("#") => true,
            _ => false,
        }
    }

    fn object_literal(&mut self) -> PResult<Expr> {
        self.expect("{")?;
        let mut props = Vec::new();
        while !self.is("}") {
            let start = self.tok.start;
            let pos = self.pos();
            if self.eat("...")? {
                props.push(ObjProp::Spread(self.assign(false)?));
            } else if (self.is_kw("get") || self.is_kw("set")) && self.is_modifier_followed_by_name() {
                let is_get = self.is_kw("get");
                self.advance()?;
                let key = self.property_name()?;
                let kind = if is_get { FuncKind::Getter } else { FuncKind::Setter };
                let f = self.method_rest(kind, start, pos, false)?;
                props.push(if is_get { ObjProp::Get(key, Box::new(f)) } else { ObjProp::Set(key, Box::new(f)) });
            } else if self.is("*") || (self.is_kw("async") && matches!(self.peek_token(), Some(t) if !t.nl_before && matches!(t.tok, Tok::Ident(_) | Tok::Str(_) | Tok::Num(_) | Tok::Punct("[") | Tok::Punct("*")))) {
                let flags = self.method_modifiers()?;
                let key = self.property_name()?;
                let f = self.function_rest_ex(None, FuncKind::Method, flags, start, pos)?;
                props.push(ObjProp::Method(key, Box::new(f)));
            } else {
                let shorthand_ok = matches!(self.tok.tok, Tok::Ident(_));
                let ident_pos = self.pos();
                let ident_off = self.tok.start;
                let key = self.property_name()?;
                if self.is("(") {
                    let f = self.method_rest(FuncKind::Method, start, pos, false)?;
                    props.push(ObjProp::Method(key, Box::new(f)));
                } else if self.eat(":")? {
                    props.push(ObjProp::Init(key, self.assign(false)?));
                } else if shorthand_ok {
                    let PropName::Ident(name) = key else { return Err(self.unexpected()) };
                    if crate::parser::is_reserved(&name, self.ctx.strict) {
                        return Err(self.error_at("Unexpected reserved word", ident_off));
                    }
                    if self.is("=") {
                        self.advance()?;
                        let d = self.assign(false)?;
                        props.push(ObjProp::CoverInit(name, d));
                    } else {
                        props.push(ObjProp::Shorthand(name, ident_pos));
                    }
                } else {
                    return Err(self.unexpected());
                }
            }
            if !self.is("}") {
                self.expect(",")?;
            }
        }
        self.expect("}")?;
        Ok(Expr::Object(props))
    }
}
