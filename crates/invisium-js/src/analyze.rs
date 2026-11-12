//! Pre-compilation scans: which names nested functions reference (those bindings must live in
//! heap environments), pseudo-variable usage, and var/lexical declaration collection.

use crate::ast::*;
use std::collections::HashSet;

#[derive(Default)]
pub struct Refs {
    /// Every identifier mentioned inside a nested function or class.
    pub inner: HashSet<String>,
    /// `arguments` / `this` / `super()` used directly or through arrow functions only.
    pub uses_arguments: bool,
    pub arrow_uses_this: bool,
    /// A direct `eval(...)` call somewhere inside, nested functions included: every binding of
    /// this function must then be reachable by name (heap environments).
    pub has_eval: bool,
    /// A direct `eval(...)` call in this function's own body (it may declare `var`s here).
    pub direct_eval: bool,
    /// A direct `eval(...)` call that sees this function's `arguments` object.
    pub eval_uses_args: bool,
}

struct Walker<'r> {
    refs: &'r mut Refs,
    /// 0 = the function being analysed; >0 inside nested functions.
    depth: u32,
    /// Depth counting only non-arrow function boundaries.
    fn_depth: u32,
}

pub fn analyze(params: &[Pattern], rest: Option<&Pattern>, body: &[Stmt]) -> Refs {
    let mut refs = Refs::default();
    let mut w = Walker { refs: &mut refs, depth: 0, fn_depth: 0 };
    for p in params {
        w.pattern(p);
    }
    if let Some(r) = rest {
        w.pattern(r);
    }
    for s in body {
        w.stmt(s);
    }
    refs
}

impl<'r> Walker<'r> {
    fn name(&mut self, n: &str) {
        if self.depth > 0 {
            self.refs.inner.insert(n.to_string());
        }
        if n == "arguments" && self.fn_depth == 0 {
            self.refs.uses_arguments = true;
        }
    }

    fn func(&mut self, f: &Function) {
        let arrow = f.kind == FuncKind::Arrow;
        self.depth += 1;
        if !arrow {
            self.fn_depth += 1;
        }
        if let Some(n) = &f.name {
            self.name(n);
        }
        for p in &f.params {
            self.pattern(p);
        }
        if let Some(r) = &f.rest {
            self.pattern(r);
        }
        for s in &f.body {
            self.stmt(s);
        }
        if !arrow {
            self.fn_depth -= 1;
        }
        self.depth -= 1;
    }

    fn class(&mut self, c: &Class) {
        if let Some(n) = &c.name {
            self.name(n);
        }
        if let Some(e) = &c.extends {
            self.expr(e);
        }
        self.depth += 1;
        self.fn_depth += 1;
        if let Some(f) = &c.ctor {
            self.func(f);
        }
        for m in &c.members {
            if let PropName::Private(n) = &m.key {
                // the hidden class-scope bindings of a private member (see `class_named`)
                for suffix in ["", ":m", ":g", ":s"] {
                    self.name(&format!("#{}{}", n, suffix));
                }
            }
            self.prop_name(&m.key);
            match &m.kind {
                MemberKind::Method(f) | MemberKind::Get(f) | MemberKind::Set(f) => self.func(f),
                MemberKind::Field(Some(e)) => self.expr(e),
                MemberKind::Field(None) => {}
                MemberKind::StaticBlock(b) => b.iter().for_each(|s| self.stmt(s)),
            }
        }
        self.fn_depth -= 1;
        self.depth -= 1;
    }

    fn prop_name(&mut self, k: &PropName) {
        if let PropName::Computed(e) = k {
            self.expr(e);
        }
    }

    fn pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Ident(n, _) => self.name(n),
            Pattern::Expr(e) => self.expr(e),
            Pattern::Object { props, rest } => {
                for (k, v) in props {
                    self.prop_name(k);
                    self.pattern(v);
                }
                if let Some(r) = rest {
                    self.pattern(r);
                }
            }
            Pattern::Array { elems, rest } => {
                for e in elems.iter().flatten() {
                    self.pattern(e);
                }
                if let Some(r) = rest {
                    self.pattern(r);
                }
            }
            Pattern::Default(p, d) => {
                self.pattern(p);
                self.expr(d);
            }
        }
    }

    fn items(&mut self, items: &[ArrayItem]) {
        for it in items {
            match it {
                ArrayItem::Hole => {}
                ArrayItem::Expr(e) | ArrayItem::Spread(e) => self.expr(e),
            }
        }
    }

    fn template(&mut self, t: &TemplateParts) {
        t.exprs.iter().for_each(|e| self.expr(e));
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Num(_) | Expr::BigInt(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null | Expr::Regex(..) | Expr::NewTarget => {}
            Expr::This => {
                if self.fn_depth == 0 && self.depth > 0 {
                    self.refs.arrow_uses_this = true;
                }
            }
            Expr::SuperCall(args, _) => {
                if self.fn_depth == 0 && self.depth > 0 {
                    self.refs.arrow_uses_this = true;
                }
                self.items(args);
            }
            Expr::Template(t) => self.template(t),
            Expr::Tagged(tag, t, _) => {
                self.expr(tag);
                self.template(t);
            }
            Expr::Ident(n, _) => self.name(n),
            Expr::Array(items) => self.items(items),
            Expr::Object(props) => {
                for p in props {
                    match p {
                        ObjProp::Init(k, v) => {
                            self.prop_name(k);
                            self.expr(v);
                        }
                        ObjProp::Shorthand(n, _) => self.name(n),
                        ObjProp::CoverInit(n, d) => {
                            self.name(n);
                            self.expr(d);
                        }
                        ObjProp::Method(k, f) | ObjProp::Get(k, f) | ObjProp::Set(k, f) => {
                            self.prop_name(k);
                            self.func(f);
                        }
                        ObjProp::Spread(e) => self.expr(e),
                    }
                }
            }
            Expr::Function(f) => self.func(f),
            Expr::Class(c) => self.class(c),
            Expr::Unary(_, a, _) => self.expr(a),
            Expr::Update { target, .. } => self.expr(target),
            Expr::Binary(_, a, b, _) | Expr::Logical(_, a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Assign(_, p, v, _) => {
                self.pattern(p);
                self.expr(v);
            }
            Expr::Cond(a, b, c) => {
                self.expr(a);
                self.expr(b);
                self.expr(c);
            }
            Expr::Call { callee, args, .. } => {
                if matches!(&**callee, Expr::Ident(n, _) if n == "eval") {
                    self.refs.has_eval = true;
                    if self.depth == 0 {
                        self.refs.direct_eval = true;
                    }
                    if self.fn_depth == 0 {
                        self.refs.eval_uses_args = true;
                    }
                }
                self.expr(callee);
                self.items(args);
            }
            Expr::New { callee, args, .. } => {
                self.expr(callee);
                self.items(args);
            }
            Expr::Member { obj, .. } => self.expr(obj),
            Expr::PrivateMember { obj, name, .. } => {
                self.expr(obj);
                self.name(&format!("#{}", name));
            }
            Expr::PrivateIn(name, e, _) => {
                self.name(&format!("#{}", name));
                self.expr(e);
            }
            Expr::PrivateDefine { name, value, .. } => {
                self.name(&format!("#{}", name));
                self.expr(value);
            }
            Expr::Index { obj, index, .. } => {
                self.expr(obj);
                self.expr(index);
            }
            Expr::SuperMember(..) => {
                if self.fn_depth == 0 && self.depth > 0 {
                    self.refs.arrow_uses_this = true;
                }
            }
            Expr::SuperIndex(i, _) => {
                if self.fn_depth == 0 && self.depth > 0 {
                    self.refs.arrow_uses_this = true;
                }
                self.expr(i)
            }
            Expr::OptChain(e) | Expr::Paren(e) | Expr::Await(e) => self.expr(e),
            Expr::Yield(e, _) => {
                if let Some(e) = e {
                    self.expr(e)
                }
            }
            Expr::Seq(v) => v.iter().for_each(|e| self.expr(e)),
        }
    }

    fn decls(&mut self, d: &[(Pattern, Option<Expr>)]) {
        for (p, i) in d {
            self.pattern(p);
            if let Some(i) = i {
                self.expr(i);
            }
        }
    }

    fn head(&mut self, h: &ForHead) {
        match h {
            ForHead::Var(_, p) | ForHead::Pattern(p) => self.pattern(p),
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Expr(e) | StmtKind::Throw(e) => self.expr(e),
            StmtKind::Var(_, d) => self.decls(d),
            StmtKind::Func(f) => {
                if let Some(n) = &f.name {
                    self.name(n);
                }
                self.func(f)
            }
            StmtKind::Class(c) => self.class(c),
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.expr(e)
                }
            }
            StmtKind::If(t, a, b) => {
                self.expr(t);
                self.stmt(a);
                if let Some(b) = b {
                    self.stmt(b);
                }
            }
            StmtKind::Block(b) => b.iter().for_each(|s| self.stmt(s)),
            StmtKind::For { init, test, update, body } => {
                match init {
                    Some(ForInit::Var(_, d)) => self.decls(d),
                    Some(ForInit::Expr(e)) => self.expr(e),
                    None => {}
                }
                if let Some(t) = test {
                    self.expr(t);
                }
                if let Some(u) = update {
                    self.expr(u);
                }
                self.stmt(body);
            }
            StmtKind::ForIn(h, e, b) | StmtKind::ForOf(h, e, b) | StmtKind::ForAwait(h, e, b) => {
                self.head(h);
                self.expr(e);
                self.stmt(b);
            }
            StmtKind::While(e, b) | StmtKind::DoWhile(b, e) => {
                self.expr(e);
                self.stmt(b);
            }
            StmtKind::Try { block, param, handler, finalizer } => {
                block.iter().for_each(|s| self.stmt(s));
                if let Some(p) = param {
                    self.pattern(p);
                }
                if let Some(h) = handler {
                    h.iter().for_each(|s| self.stmt(s));
                }
                if let Some(f) = finalizer {
                    f.iter().for_each(|s| self.stmt(s));
                }
            }
            StmtKind::Switch(d, cases) => {
                self.expr(d);
                for c in cases {
                    if let Some(t) = &c.test {
                        self.expr(t);
                    }
                    c.body.iter().for_each(|s| self.stmt(s));
                }
            }
            StmtKind::Labeled(_, b) => self.stmt(b),
            StmtKind::With(e, b) => {
                self.expr(e);
                self.stmt(b);
            }
            StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Empty | StmtKind::Debugger => {}
        }
    }
}

pub fn pattern_names(p: &Pattern, out: &mut Vec<String>) {
    match p {
        Pattern::Ident(n, _) => out.push(n.clone()),
        Pattern::Expr(_) => {}
        Pattern::Object { props, rest } => {
            props.iter().for_each(|(_, v)| pattern_names(v, out));
            if let Some(r) = rest {
                pattern_names(r, out);
            }
        }
        Pattern::Array { elems, rest } => {
            elems.iter().flatten().for_each(|e| pattern_names(e, out));
            if let Some(r) = rest {
                pattern_names(r, out);
            }
        }
        Pattern::Default(p, _) => pattern_names(p, out),
    }
}

/// Names of plain function declarations nested in blocks of these statements: in sloppy code
/// they are also `var`s of the enclosing function (Annex B.3.3).
pub fn block_func_names(stmts: &[Stmt], out: &mut Vec<String>) {
    stmts.iter().for_each(|s| block_funcs_stmt(s, false, out));
}

fn block_funcs_stmt(s: &Stmt, nested: bool, out: &mut Vec<String>) {
    let all = |list: &[Stmt], out: &mut Vec<String>| list.iter().for_each(|s| block_funcs_stmt(s, true, out));
    match &s.kind {
        StmtKind::Func(f) => {
            if let (true, Some(n), false, false) = (nested, &f.name, f.is_async, f.is_gen) {
                if !out.contains(n) {
                    out.push(n.clone());
                }
            }
        }
        StmtKind::If(_, a, b) => {
            block_funcs_stmt(a, true, out);
            if let Some(b) = b {
                block_funcs_stmt(b, true, out);
            }
        }
        StmtKind::Block(b) => all(b, out),
        StmtKind::For { body, .. } => block_funcs_stmt(body, true, out),
        StmtKind::ForIn(_, _, b) | StmtKind::ForOf(_, _, b) | StmtKind::ForAwait(_, _, b) => block_funcs_stmt(b, true, out),
        StmtKind::While(_, b) | StmtKind::DoWhile(b, _) | StmtKind::With(_, b) => block_funcs_stmt(b, true, out),
        StmtKind::Labeled(_, b) => block_funcs_stmt(b, nested, out),
        StmtKind::Try { block, handler, finalizer, .. } => {
            all(block, out);
            if let Some(h) = handler {
                all(h, out);
            }
            if let Some(f) = finalizer {
                all(f, out);
            }
        }
        StmtKind::Switch(_, cases) => cases.iter().for_each(|c| all(&c.body, out)),
        _ => {}
    }
}

/// `var` names declared anywhere in these statements (not inside nested functions).
pub fn var_names(stmts: &[Stmt], out: &mut Vec<String>) {
    for s in stmts {
        var_names_stmt(s, out);
    }
}

fn var_names_stmt(s: &Stmt, out: &mut Vec<String>) {
    match &s.kind {
        StmtKind::Var(VarKind::Var, d) => d.iter().for_each(|(p, _)| pattern_names(p, out)),
        StmtKind::If(_, a, b) => {
            var_names_stmt(a, out);
            if let Some(b) = b {
                var_names_stmt(b, out);
            }
        }
        StmtKind::Block(b) => var_names(b, out),
        StmtKind::For { init, body, .. } => {
            if let Some(ForInit::Var(VarKind::Var, d)) = init {
                d.iter().for_each(|(p, _)| pattern_names(p, out));
            }
            var_names_stmt(body, out);
        }
        StmtKind::ForIn(h, _, b) | StmtKind::ForOf(h, _, b) | StmtKind::ForAwait(h, _, b) => {
            if let ForHead::Var(VarKind::Var, p) = h {
                pattern_names(p, out);
            }
            var_names_stmt(b, out);
        }
        StmtKind::While(_, b) | StmtKind::DoWhile(b, _) | StmtKind::Labeled(_, b) | StmtKind::With(_, b) => var_names_stmt(b, out),
        StmtKind::Try { block, handler, finalizer, .. } => {
            var_names(block, out);
            if let Some(h) = handler {
                var_names(h, out);
            }
            if let Some(f) = finalizer {
                var_names(f, out);
            }
        }
        StmtKind::Switch(_, cases) => cases.iter().for_each(|c| var_names(&c.body, out)),
        _ => {}
    }
}
