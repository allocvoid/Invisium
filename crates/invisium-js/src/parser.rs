//! Recursive-descent parser: core state and token helpers. Expressions live in
//! `parser_expr.rs`, statements/functions/classes in `parser_stmt.rs`.

use crate::ast::*;
use crate::lexer::{LexResult, Lexer, LineMap, Tok, Token};

pub const MAX_DEPTH: u32 = 1500;

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub line: u32,
    pub column: u32,
}

pub type PResult<T> = Result<T, ParseError>;

#[derive(Clone, Copy, Default)]
pub struct FnCtx {
    pub in_function: bool,
    pub strict: bool,
    pub allow_super_call: bool,
    pub allow_super_prop: bool,
    /// Inside an async function body: `await` is an operator.
    pub in_async: bool,
    /// Inside a generator body: `yield` is an operator.
    pub in_gen: bool,
    /// Eval code running inside a function: `new.target` is allowed outside any function here.
    pub new_target_ok: bool,
}

/// What direct-eval code inherits from its call site.
#[derive(Clone, Copy, Default)]
pub struct ParseOpts {
    pub strict: bool,
    pub new_target: bool,
    pub super_prop: bool,
    pub super_call: bool,
}

pub fn parse_eval(src: &[char], o: ParseOpts) -> PResult<Program> {
    let mut p = Parser::new(src)?;
    p.ctx.new_target_ok = o.new_target;
    p.ctx.allow_super_prop = o.super_prop;
    p.ctx.allow_super_call = o.super_call;
    let (body, strict) = p.parse_body_with_directives(true, o.strict)?;
    if p.tok.tok != Tok::Eof {
        return Err(p.unexpected());
    }
    Ok(Program { body, strict })
}

pub struct Parser<'a> {
    pub lex: Lexer<'a>,
    pub tok: Token,
    pub prev_end: usize,
    pub lines: LineMap,
    pub depth: u32,
    pub ctx: FnCtx,
    pub in_loop: u32,
    pub in_switch: u32,
    pub labels: Vec<(String, bool)>,
    /// An `await` (or `for await`) appeared outside every function.
    pub top_await: bool,
    /// Module code: `import.meta` is allowed; the import / export declarations collected.
    pub module: Option<ModuleInfo>,
}

/// Parses module code (strict, top-level `await` allowed, `import` / `export` declarations).
pub fn parse_module(src: &[char]) -> PResult<(Program, ModuleInfo)> {
    let mut p = Parser::new(src)?;
    p.module = Some(ModuleInfo::default());
    p.ctx.strict = true;
    p.ctx.in_async = true;
    let mut body = Vec::new();
    while p.tok.tok != Tok::Eof {
        if p.is_kw("import") && !matches!(p.peek_tok(), Tok::Punct("(") | Tok::Punct(".")) {
            p.import_declaration()?;
        } else if p.is_kw("export") {
            if let Some(s) = p.export_declaration()? {
                body.push(s);
            }
        } else {
            body.push(p.statement_list_item()?);
        }
    }
    let mut info = p.module.take().unwrap_or_default();
    info.has_tla = p.top_await;
    Ok((Program { body, strict: true }, info))
}

pub fn parse_program(src: &[char]) -> PResult<Program> {
    let mut p = Parser::new(src)?;
    let (body, strict) = p.parse_body_with_directives(true, false)?;
    if p.tok.tok != Tok::Eof {
        return Err(p.unexpected());
    }
    Ok(Program { body, strict })
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a [char]) -> PResult<Self> {
        let lines = LineMap::new(src);
        let mut lex = Lexer::new(src);
        let first = lex.next_token();
        let mut p = Parser {
            lex,
            tok: Token { tok: Tok::Eof, start: 0, end: 0, nl_before: false, escaped: false, legacy_octal: false },
            prev_end: 0,
            lines,
            depth: 0,
            ctx: FnCtx::default(),
            in_loop: 0,
            in_switch: 0,
            labels: Vec::new(),
            top_await: false,
            module: None,
        };
        p.tok = p.lexed(first)?;
        Ok(p)
    }

    fn lexed(&self, r: LexResult<Token>) -> PResult<Token> {
        r.map_err(|(m, off)| self.error_at(&m, off))
    }

    pub fn error_at(&self, msg: &str, off: usize) -> ParseError {
        let (line, column) = self.lines.pos(off);
        ParseError { message: msg.to_string(), line, column }
    }

    pub fn error(&self, msg: &str) -> ParseError {
        self.error_at(msg, self.tok.start)
    }

    pub fn unexpected(&self) -> ParseError {
        let what = match &self.tok.tok {
            Tok::Eof => "end of input".to_string(),
            Tok::Ident(s) => format!("identifier '{}'", s),
            Tok::Num(_) | Tok::BigInt(_) => "number".to_string(),
            Tok::Str(_) => "string".to_string(),
            Tok::Template { .. } => "template".to_string(),
            Tok::Regex { .. } => "regular expression".to_string(),
            Tok::Punct(p) => format!("token '{}'", p),
        };
        self.error(&format!("Unexpected {}", what))
    }

    pub fn pos(&self) -> Pos {
        let (line, col) = self.lines.pos(self.tok.start);
        Pos { line, col }
    }

    pub fn advance(&mut self) -> PResult<Token> {
        let next = self.lex.next_token();
        let next = self.lexed(next)?;
        self.prev_end = self.tok.end;
        Ok(std::mem::replace(&mut self.tok, next))
    }

    pub fn peek_tok(&self) -> Tok {
        let mut l = self.lex.clone();
        l.next_token().map(|t| t.tok).unwrap_or(Tok::Eof)
    }

    pub fn peek_token(&self) -> Option<Token> {
        let mut l = self.lex.clone();
        l.next_token().ok()
    }

    /// The two tokens after the current one.
    pub fn peek2(&self) -> (Option<Token>, Option<Token>) {
        let mut l = self.lex.clone();
        let a = l.next_token().ok();
        let b = l.next_token().ok();
        (a, b)
    }

    /// `async` used as a modifier: followed on the same line by `function`.
    pub fn is_async_function(&self) -> bool {
        self.is_kw("async") && matches!(self.peek_token(), Some(t) if !t.nl_before && matches!(&t.tok, Tok::Ident(k) if k == "function"))
    }

    pub fn is(&self, p: &str) -> bool {
        matches!(&self.tok.tok, Tok::Punct(q) if *q == p)
    }

    pub fn is_kw(&self, k: &str) -> bool {
        matches!(&self.tok.tok, Tok::Ident(s) if s == k) && !self.tok.escaped
    }

    pub fn eat(&mut self, p: &str) -> PResult<bool> {
        if self.is(p) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn eat_kw(&mut self, k: &str) -> PResult<bool> {
        if self.is_kw(k) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn expect(&mut self, p: &str) -> PResult<()> {
        if self.is(p) {
            self.advance()?;
            Ok(())
        } else {
            Err(self.unexpected())
        }
    }

    pub fn expect_kw(&mut self, k: &str) -> PResult<()> {
        if self.is_kw(k) {
            self.advance()?;
            Ok(())
        } else {
            Err(self.unexpected())
        }
    }

    /// Automatic semicolon insertion.
    pub fn semicolon(&mut self) -> PResult<()> {
        if self.eat(";")? {
            return Ok(());
        }
        if self.is("}") || self.tok.tok == Tok::Eof || self.tok.nl_before {
            return Ok(());
        }
        Err(self.unexpected())
    }

    pub fn enter(&mut self) -> PResult<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("Maximum nesting depth exceeded"));
        }
        Ok(())
    }

    pub fn leave(&mut self) {
        self.depth -= 1;
    }

    pub fn rescan_regex(&mut self) -> PResult<()> {
        let r = self.lex.rescan_regex(self.tok.start, self.tok.nl_before);
        self.tok = self.lexed(r)?;
        Ok(())
    }

    pub fn rescan_template(&mut self) -> PResult<()> {
        let r = self.lex.rescan_template(self.tok.start);
        self.tok = self.lexed(r)?;
        Ok(())
    }

    /// Identifier usable as a binding / reference (not a reserved word).
    pub fn ident_name(&mut self) -> PResult<String> {
        if let Tok::Ident(s) = &self.tok.tok {
            let s = s.clone();
            // an escaped reserved word is neither a keyword nor a usable identifier
            if is_reserved(&s, self.ctx.strict) {
                return Err(self.unexpected());
            }
            self.advance()?;
            Ok(s)
        } else {
            Err(self.unexpected())
        }
    }

    /// Any IdentifierName (keywords allowed), used after `.` and in property keys.
    pub fn any_name(&mut self) -> PResult<String> {
        if let Tok::Ident(s) = &self.tok.tok {
            let s = s.clone();
            self.advance()?;
            Ok(s)
        } else {
            Err(self.unexpected())
        }
    }

    pub fn is_ident_ref(&self) -> bool {
        match &self.tok.tok {
            Tok::Ident(s) => !is_reserved(s, self.ctx.strict),
            _ => false,
        }
    }

    pub fn parse_body_with_directives(&mut self, top: bool, mut strict: bool) -> PResult<(Vec<Stmt>, bool)> {
        let mut body = Vec::new();
        let mut directives = true;
        let saved_strict = self.ctx.strict;
        self.ctx.strict = self.ctx.strict || strict;
        loop {
            if self.tok.tok == Tok::Eof || (!top && self.is("}")) {
                break;
            }
            if directives {
                if let Tok::Str(s) = &self.tok.tok {
                    let raw_len = self.tok.end - self.tok.start;
                    let is_use_strict = raw_len == 12 && *s == "use strict".encode_utf16().collect::<Vec<u16>>();
                    let next = self.peek_token();
                    let ends = match &next {
                        Some(t) => matches!(t.tok, Tok::Punct(";") | Tok::Punct("}") | Tok::Eof) || t.nl_before,
                        None => false,
                    };
                    if ends && is_use_strict {
                        strict = true;
                        self.ctx.strict = true;
                    }
                    if !ends {
                        directives = false;
                    }
                } else {
                    directives = false;
                }
            }
            body.push(self.statement_list_item()?);
        }
        let s = self.ctx.strict;
        self.ctx.strict = saved_strict;
        Ok((body, s || strict))
    }
}

pub fn is_reserved(s: &str, strict: bool) -> bool {
    matches!(
        s,
        "break" | "case" | "catch" | "class" | "const" | "continue" | "debugger" | "default" | "delete"
            | "do" | "else" | "export" | "extends" | "finally" | "for" | "function" | "if" | "import"
            | "in" | "instanceof" | "new" | "return" | "super" | "switch" | "this" | "throw" | "try"
            | "typeof" | "var" | "void" | "while" | "with" | "null" | "true" | "false" | "enum"
    ) || (strict
        && matches!(
            s,
            "implements" | "interface" | "package" | "private" | "protected" | "public" | "static" | "yield" | "let"
        ))
}
