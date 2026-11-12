//! Expression parsing.

use crate::ast::*;
use crate::lexer::Tok;
use crate::parser::{PResult, Parser};

const MAX_CHAIN: u32 = 10000;

fn binop_info(p: &str, no_in: bool) -> Option<(u8, Result<BinOp, LogicOp>)> {
    use BinOp::*;
    Some(match p {
        "??" => (1, Err(LogicOp::Nullish)),
        "||" => (2, Err(LogicOp::Or)),
        "&&" => (3, Err(LogicOp::And)),
        "|" => (4, Ok(BitOr)),
        "^" => (5, Ok(BitXor)),
        "&" => (6, Ok(BitAnd)),
        "==" => (7, Ok(Eq)),
        "!=" => (7, Ok(Ne)),
        "===" => (7, Ok(StrictEq)),
        "!==" => (7, Ok(StrictNe)),
        "<" => (8, Ok(Lt)),
        ">" => (8, Ok(Gt)),
        "<=" => (8, Ok(Le)),
        ">=" => (8, Ok(Ge)),
        "instanceof" => (8, Ok(InstanceOf)),
        "in" if !no_in => (8, Ok(In)),
        "<<" => (9, Ok(Shl)),
        ">>" => (9, Ok(Shr)),
        ">>>" => (9, Ok(UShr)),
        "+" => (10, Ok(Add)),
        "-" => (10, Ok(Sub)),
        "*" => (11, Ok(Mul)),
        "/" => (11, Ok(Div)),
        "%" => (11, Ok(Mod)),
        "**" => (12, Ok(Exp)),
        _ => return None,
    })
}

fn assign_op(p: &str) -> Option<AssignOp> {
    use BinOp::*;
    Some(match p {
        "=" => AssignOp::Assign,
        "+=" => AssignOp::Bin(Add),
        "-=" => AssignOp::Bin(Sub),
        "*=" => AssignOp::Bin(Mul),
        "/=" => AssignOp::Bin(Div),
        "%=" => AssignOp::Bin(Mod),
        "**=" => AssignOp::Bin(Exp),
        "<<=" => AssignOp::Bin(Shl),
        ">>=" => AssignOp::Bin(Shr),
        ">>>=" => AssignOp::Bin(UShr),
        "&=" => AssignOp::Bin(BitAnd),
        "|=" => AssignOp::Bin(BitOr),
        "^=" => AssignOp::Bin(BitXor),
        "&&=" => AssignOp::Logic(LogicOp::And),
        "||=" => AssignOp::Logic(LogicOp::Or),
        "??=" => AssignOp::Logic(LogicOp::Nullish),
        _ => return None,
    })
}

impl<'a> Parser<'a> {
    pub fn expression(&mut self, no_in: bool) -> PResult<Expr> {
        let first = self.assign(no_in)?;
        if !self.is(",") {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat(",")? {
            items.push(self.assign(no_in)?);
        }
        Ok(Expr::Seq(items))
    }

    pub fn assign(&mut self, no_in: bool) -> PResult<Expr> {
        self.enter()?;
        let r = self.assign_inner(no_in);
        self.leave();
        r
    }

    fn assign_inner(&mut self, no_in: bool) -> PResult<Expr> {
        if self.ctx.in_gen && self.is_kw("yield") {
            self.advance()?;
            let ends = self.tok.nl_before
                || self.tok.tok == Tok::Eof
                || matches!(self.tok.tok, Tok::Punct(")") | Tok::Punct("]") | Tok::Punct("}") | Tok::Punct(",") | Tok::Punct(";") | Tok::Punct(":"))
                || self.is_kw("in")
                || self.is_kw("of");
            if ends {
                return Ok(Expr::Yield(None, false));
            }
            let delegate = self.eat("*")?;
            let arg = self.assign(no_in)?;
            return Ok(Expr::Yield(Some(Box::new(arg)), delegate));
        }
        // single-identifier arrow: `x => ...`
        if self.is_ident_ref() {
            if let Tok::Punct("=>") = self.peek_tok() {
                let start = self.tok.start;
                let pos = self.pos();
                let name = self.ident_name()?;
                if self.tok.nl_before {
                    return Err(self.unexpected());
                }
                self.expect("=>")?;
                return self.arrow_body(vec![Pattern::Ident(name, pos)], None, start, pos, false);
            }
        }
        let start_tok = self.tok.start;
        let pos = self.pos();
        let lhs = self.conditional(no_in)?;
        if let Expr::Function(f) = &lhs {
            if f.kind == FuncKind::Arrow {
                return Ok(lhs);
            }
        }
        if let Tok::Punct(p) = self.tok.tok {
            if let Some(op) = assign_op(p) {
                let target = if op == AssignOp::Assign {
                    self.to_pattern(lhs, false, start_tok)?
                } else {
                    self.simple_target(lhs, start_tok)?
                };
                self.advance()?;
                let value = self.assign(no_in)?;
                return Ok(Expr::Assign(op, Box::new(target), Box::new(value), pos));
            }
        }
        Ok(lhs)
    }

    pub fn simple_target(&self, e: Expr, off: usize) -> PResult<Pattern> {
        match e {
            Expr::Ident(n, p) => {
                if self.ctx.strict && (n == "eval" || n == "arguments") {
                    return Err(self.error_at("Invalid assignment target", off));
                }
                Ok(Pattern::Ident(n, p))
            }
            Expr::Member { optional: false, .. } | Expr::Index { optional: false, .. } | Expr::PrivateMember { optional: false, .. } | Expr::SuperMember(..) | Expr::SuperIndex(..) => {
                Ok(Pattern::Expr(e))
            }
            Expr::Paren(inner) => self.simple_target(*inner, off),
            _ => Err(self.error_at("Invalid left-hand side in assignment", off)),
        }
    }

    /// Converts a cover expression into a destructuring pattern.
    pub fn to_pattern(&self, e: Expr, binding: bool, off: usize) -> PResult<Pattern> {
        match e {
            Expr::Ident(n, p) => self.simple_target(Expr::Ident(n, p), off),
            Expr::Assign(AssignOp::Assign, target, value, _) => Ok(Pattern::Default(target, value)),
            Expr::Array(items) => {
                let mut elems = Vec::new();
                let mut rest = None;
                let n = items.len();
                for (i, it) in items.into_iter().enumerate() {
                    match it {
                        ArrayItem::Hole => elems.push(None),
                        ArrayItem::Expr(e) => elems.push(Some(self.to_pattern(e, binding, off)?)),
                        ArrayItem::Spread(e) => {
                            if i + 1 != n {
                                return Err(self.error_at("Rest element must be last element", off));
                            }
                            rest = Some(Box::new(self.to_pattern(e, binding, off)?));
                        }
                    }
                }
                Ok(Pattern::Array { elems, rest })
            }
            Expr::Object(props) => {
                let mut out = Vec::new();
                let mut rest = None;
                for p in props {
                    match p {
                        ObjProp::Init(k, v) => out.push((k, self.to_pattern(v, binding, off)?)),
                        ObjProp::Shorthand(n, pos) => out.push((PropName::Ident(n.clone()), Pattern::Ident(n, pos))),
                        ObjProp::CoverInit(n, d) => {
                            out.push((PropName::Ident(n.clone()), Pattern::Default(Box::new(Pattern::Ident(n, Pos::default())), Box::new(d))))
                        }
                        ObjProp::Spread(e) => rest = Some(Box::new(self.to_pattern(e, binding, off)?)),
                        _ => return Err(self.error_at("Invalid destructuring assignment target", off)),
                    }
                }
                Ok(Pattern::Object { props: out, rest })
            }
            Expr::Paren(inner) if !binding => match *inner {
                e @ (Expr::Ident(..) | Expr::Member { .. } | Expr::Index { .. }) => self.simple_target(e, off),
                _ => Err(self.error_at("Invalid destructuring assignment target", off)),
            },
            e @ (Expr::Member { .. } | Expr::Index { .. } | Expr::PrivateMember { .. } | Expr::SuperMember(..) | Expr::SuperIndex(..)) if !binding => self.simple_target(e, off),
            _ => Err(self.error_at("Invalid destructuring assignment target", off)),
        }
    }

    fn conditional(&mut self, no_in: bool) -> PResult<Expr> {
        let test = self.binary(0, no_in)?;
        if !self.is("?") {
            return Ok(test);
        }
        self.advance()?;
        let cons = self.assign(false)?;
        self.expect(":")?;
        let alt = self.assign(no_in)?;
        Ok(Expr::Cond(Box::new(test), Box::new(cons), Box::new(alt)))
    }

    fn binary(&mut self, min: u8, no_in: bool) -> PResult<Expr> {
        let mut left = self.unary()?;
        let mut chain = 0;
        loop {
            let name: &str = match &self.tok.tok {
                Tok::Punct(p) => p,
                Tok::Ident(s) if (s == "in" || s == "instanceof") && !self.tok.escaped => s.as_str(),
                _ => break,
            };
            let Some((prec, op)) = binop_info(name, no_in) else { break };
            if prec < min {
                break;
            }
            let pos = self.pos();
            self.advance()?;
            chain += 1;
            if self.depth + chain > MAX_CHAIN {
                return Err(self.error("Expression too deeply nested"));
            }
            let right = if prec == 12 { self.binary(12, no_in)? } else { self.binary(prec + 1, no_in)? };
            left = match op {
                Ok(b) => Expr::Binary(b, Box::new(left), Box::new(right), pos),
                Err(l) => Expr::Logical(l, Box::new(left), Box::new(right)),
            };
        }
        Ok(left)
    }

    fn unary(&mut self) -> PResult<Expr> {
        self.enter()?;
        let r = self.unary_inner();
        self.leave();
        r
    }

    fn unary_inner(&mut self) -> PResult<Expr> {
        let pos = self.pos();
        if self.eat("#")? {
            // `#name in obj`: the brand check
            let name = self.any_name()?;
            self.expect_kw("in")?;
            let right = self.binary(9, false)?;
            return Ok(Expr::PrivateIn(name, Box::new(right), pos));
        }
        if self.ctx.in_async && self.is_kw("await") {
            if !self.ctx.in_function {
                self.top_await = true;
            }
            self.advance()?;
            let arg = self.unary()?;
            return Ok(Expr::Await(Box::new(arg)));
        }
        let op = match &self.tok.tok {
            Tok::Punct("!") => Some(UnOp::Not),
            Tok::Punct("-") => Some(UnOp::Neg),
            Tok::Punct("+") => Some(UnOp::Plus),
            Tok::Punct("~") => Some(UnOp::BitNot),
            Tok::Ident(s) if !self.tok.escaped && s == "typeof" => Some(UnOp::TypeOf),
            Tok::Ident(s) if !self.tok.escaped && s == "void" => Some(UnOp::Void),
            Tok::Ident(s) if !self.tok.escaped && s == "delete" => Some(UnOp::Delete),
            _ => None,
        };
        if let Some(op) = op {
            self.advance()?;
            let arg = self.unary()?;
            if self.is("**") {
                return Err(self.error("Unary operator used immediately before exponentiation expression"));
            }
            if op == UnOp::Delete && matches!(arg, Expr::PrivateMember { .. }) {
                return Err(self.error("Private fields can not be deleted"));
            }
            if op == UnOp::Delete && self.ctx.strict {
                if let Expr::Ident(..) = arg {
                    return Err(self.error("Delete of an unqualified identifier in strict mode"));
                }
            }
            return Ok(Expr::Unary(op, Box::new(arg), pos));
        }
        if self.is("++") || self.is("--") {
            let inc = self.is("++");
            self.advance()?;
            let off = self.tok.start;
            let target = self.unary()?;
            let target = self.simple_target(target, off)?;
            return Ok(Expr::Update { inc, prefix: true, target: Box::new(pattern_expr(target)), pos });
        }
        let off = self.tok.start;
        let e = self.lhs()?;
        if (self.is("++") || self.is("--")) && !self.tok.nl_before {
            let inc = self.is("++");
            self.advance()?;
            let target = self.simple_target(e, off)?;
            return Ok(Expr::Update { inc, prefix: false, target: Box::new(pattern_expr(target)), pos });
        }
        Ok(e)
    }

    pub fn arguments(&mut self) -> PResult<Vec<ArrayItem>> {
        self.expect("(")?;
        let mut args = Vec::new();
        while !self.is(")") {
            if self.eat("...")? {
                args.push(ArrayItem::Spread(self.assign(false)?));
            } else {
                args.push(ArrayItem::Expr(self.assign(false)?));
            }
            if !self.eat(",")? {
                break;
            }
        }
        self.expect(")")?;
        Ok(args)
    }

    fn lhs(&mut self) -> PResult<Expr> {
        let mut e = if self.is_kw("new") { self.new_expr()? } else { self.primary()? };
        let mut in_chain = false;
        loop {
            let pos = self.pos();
            if self.is(".") {
                self.advance()?;
                if self.eat("#")? {
                    let name = self.any_name()?;
                    e = Expr::PrivateMember { obj: Box::new(e), name, optional: false, pos };
                } else {
                    let name = self.any_name()?;
                    e = Expr::Member { obj: Box::new(e), name, optional: false, pos };
                }
            } else if self.is("?.") {
                self.advance()?;
                in_chain = true;
                if self.is("(") {
                    let args = self.arguments()?;
                    e = Expr::Call { callee: Box::new(e), args, optional: true, pos };
                } else if self.eat("[")? {
                    let idx = self.expression(false)?;
                    self.expect("]")?;
                    e = Expr::Index { obj: Box::new(e), index: Box::new(idx), optional: true, pos };
                } else if self.eat("#")? {
                    let name = self.any_name()?;
                    e = Expr::PrivateMember { obj: Box::new(e), name, optional: true, pos };
                } else {
                    let name = self.any_name()?;
                    e = Expr::Member { obj: Box::new(e), name, optional: true, pos };
                }
            } else if self.is("[") {
                self.advance()?;
                let idx = self.expression(false)?;
                self.expect("]")?;
                e = Expr::Index { obj: Box::new(e), index: Box::new(idx), optional: false, pos };
            } else if self.is("(") {
                let args = self.arguments()?;
                e = Expr::Call { callee: Box::new(e), args, optional: false, pos };
            } else if let Tok::Template { .. } = self.tok.tok {
                if in_chain {
                    return Err(self.error("Invalid tagged template on optional chain"));
                }
                let parts = self.template_parts(true)?;
                e = Expr::Tagged(Box::new(e), Box::new(parts), pos);
            } else {
                break;
            }
        }
        if in_chain {
            e = Expr::OptChain(Box::new(e));
        }
        Ok(e)
    }

    fn new_expr(&mut self) -> PResult<Expr> {
        let pos = self.pos();
        self.expect_kw("new")?;
        if self.eat(".")? {
            let n = self.any_name()?;
            if n != "target" || !(self.ctx.in_function || self.ctx.new_target_ok) {
                return Err(self.error("Invalid new.target"));
            }
            return Ok(Expr::NewTarget);
        }
        self.enter()?;
        let mut callee = if self.is_kw("new") { self.new_expr()? } else { self.primary()? };
        loop {
            let p = self.pos();
            if self.eat(".")? {
                let name = self.any_name()?;
                callee = Expr::Member { obj: Box::new(callee), name, optional: false, pos: p };
            } else if self.eat("[")? {
                let idx = self.expression(false)?;
                self.expect("]")?;
                callee = Expr::Index { obj: Box::new(callee), index: Box::new(idx), optional: false, pos: p };
            } else if let Tok::Template { .. } = self.tok.tok {
                let parts = self.template_parts(true)?;
                callee = Expr::Tagged(Box::new(callee), Box::new(parts), p);
            } else {
                break;
            }
        }
        self.leave();
        if self.is("?.") {
            return Err(self.error("Invalid optional chain from new expression"));
        }
        let args = if self.is("(") { self.arguments()? } else { Vec::new() };
        Ok(Expr::New { callee: Box::new(callee), args, pos })
    }

    pub fn template_parts(&mut self, tagged: bool) -> PResult<TemplateParts> {
        let mut parts = TemplateParts { cooked: Vec::new(), raw: Vec::new(), exprs: Vec::new() };
        loop {
            let Tok::Template { cooked, raw, tail } = self.tok.tok.clone() else { return Err(self.unexpected()) };
            if cooked.is_none() && !tagged {
                return Err(self.error("Invalid escape sequence in template"));
            }
            parts.cooked.push(cooked);
            parts.raw.push(raw);
            if tail {
                self.advance()?;
                return Ok(parts);
            }
            self.advance()?;
            parts.exprs.push(self.expression(false)?);
            if !self.is("}") {
                return Err(self.unexpected());
            }
            self.rescan_template()?;
        }
    }
}

pub fn pattern_expr(p: Pattern) -> Expr {
    match p {
        Pattern::Ident(n, pos) => Expr::Ident(n, pos),
        Pattern::Expr(e) => e,
        _ => Expr::Null,
    }
}
