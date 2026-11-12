//! Expression compilation.

use crate::ast::*;
use crate::bytecode::{Op, Template};
use crate::compiler::*;

fn bin_op(op: BinOp) -> Op {
    use BinOp::*;
    match op {
        Add => Op::Add, Sub => Op::Sub, Mul => Op::Mul, Div => Op::Div, Mod => Op::Mod, Exp => Op::Exp,
        Shl => Op::Shl, Shr => Op::Shr, UShr => Op::UShr, BitAnd => Op::BitAnd, BitOr => Op::BitOr,
        BitXor => Op::BitXor, Eq => Op::Eq, Ne => Op::Ne, StrictEq => Op::StrictEq, StrictNe => Op::StrictNe,
        Lt => Op::Lt, Gt => Op::Gt, Le => Op::Le, Ge => Op::Ge, In => Op::In, InstanceOf => Op::InstanceOf,
    }
}

pub fn num_key(n: f64) -> Str16 {
    s16(&crate::numfmt::number_to_string(n))
}

impl Compiler {
    pub fn expr(&mut self, e: &Expr) -> CResult<()> {
        self.enter(Pos::default())?;
        let r = self.expr_inner(e);
        self.depth -= 1;
        r
    }

    /// Compiles `e`, giving anonymous functions/classes the binding name of `target`.
    pub fn expr_named(&mut self, e: &Expr, target: &Pattern) -> CResult<()> {
        if let Pattern::Ident(n, _) = target {
            return self.expr_with_name(e, Some(s16(n)));
        }
        self.expr(e)
    }

    pub fn expr_with_name(&mut self, e: &Expr, name: Option<Str16>) -> CResult<()> {
        match e {
            Expr::Function(f) if f.name.is_none() => {
                let idx = self.compile_function(f, name)?;
                self.emit(Op::Closure(idx));
                Ok(())
            }
            Expr::Class(c) if c.name.is_none() => self.class_named(c, name),
            _ => self.expr(e),
        }
    }

    fn items_array(&mut self, items: &[ArrayItem]) -> CResult<()> {
        self.emit(Op::NewArray);
        for it in items {
            match it {
                ArrayItem::Hole => {
                    self.emit(Op::ArrayHole);
                }
                ArrayItem::Expr(e) => {
                    self.expr(e)?;
                    self.emit(Op::ArrayPush);
                }
                ArrayItem::Spread(e) => {
                    self.expr(e)?;
                    self.describe_site(e);
                    self.emit(Op::ArraySpread);
                }
            }
        }
        Ok(())
    }

    /// Pushes arguments; returns Some(argc) or None when they were packed into an array.
    fn args(&mut self, args: &[ArrayItem]) -> CResult<Option<u32>> {
        if args.iter().any(|a| matches!(a, ArrayItem::Spread(_))) {
            self.items_array(args)?;
            return Ok(None);
        }
        for a in args {
            if let ArrayItem::Expr(e) = a {
                self.expr(e)?;
            }
        }
        Ok(Some(args.len() as u32))
    }

    pub fn prop_key(&mut self, k: &PropName) -> CResult<Option<u32>> {
        Ok(match k {
            PropName::Ident(s) => Some(self.name(s)),
            PropName::Str(s) => Some(self.name16(s.clone())),
            PropName::Num(n) => Some(self.name16(num_key(*n))),
            PropName::Computed(_) | PropName::Private(_) => None,
        })
    }

    /// Pushes the key of private name `#name`; it must be declared by an enclosing class.
    /// Returns the name-table index of `#name` (for error messages).
    pub fn get_private(&mut self, name: &str) -> CResult<u32> {
        let key = format!("#{}", name);
        if matches!(self.resolve(&key), Ref::Global) {
            return Err(self.err(&format!("Private field '{}' must be declared in an enclosing class", key), Pos::default()));
        }
        self.get_var(&key);
        Ok(self.name(&key))
    }

    /// Pushes a property key value (string or symbol) for a property name.
    pub fn push_key(&mut self, k: &PropName) -> CResult<()> {
        match k {
            PropName::Computed(e) => {
                self.expr(e)?;
                self.emit(Op::ToPropertyKey);
            }
            PropName::Private(n) => {
                self.get_private(n)?;
            }
            _ => {
                let n = self.prop_key(k)?.unwrap_or(0);
                self.emit(Op::Str(n));
            }
        }
        Ok(())
    }

    pub fn static_key_name(k: &PropName) -> Option<Str16> {
        match k {
            PropName::Ident(s) => Some(s16(s)),
            PropName::Str(s) => Some(s.clone()),
            PropName::Num(n) => Some(num_key(*n)),
            PropName::Computed(_) => None,
            PropName::Private(n) => Some(s16(&format!("#{}", n))),
        }
    }

    fn opt_jump(&mut self, pop: u32) {
        let j = self.emit(Op::OptJump(0, pop));
        if let Some(list) = self.f().opt_jumps.last_mut() {
            list.push(j);
        }
    }

    /// Compiles a callee, leaving `this func` (method) or `func` on the stack.
    fn callee(&mut self, callee: &Expr) -> CResult<bool> {
        match callee {
            Expr::Member { obj, name, optional, pos } => {
                self.expr(obj)?;
                if *optional {
                    self.opt_jump(1);
                }
                self.mark(*pos);
                let n = self.name(name);
                self.emit(Op::GetPropKeep(n));
                Ok(true)
            }
            Expr::Index { obj, index, optional, pos } => {
                self.expr(obj)?;
                if *optional {
                    self.opt_jump(1);
                }
                self.expr(index)?;
                self.mark(*pos);
                self.emit(Op::GetElemKeep);
                Ok(true)
            }
            Expr::PrivateMember { obj, name, optional, pos } => {
                self.expr(obj)?;
                if *optional {
                    self.opt_jump(1);
                }
                let n = self.get_private(name)?;
                self.mark(*pos);
                self.emit(Op::PrivGetKeep(n));
                Ok(true)
            }
            Expr::SuperMember(name, pos) => {
                self.mark(*pos);
                self.load_this();
                self.emit(Op::Dup);
                let n = self.name(name);
                self.emit(Op::Str(n));
                self.emit(Op::SuperGetElem);
                Ok(true)
            }
            Expr::SuperIndex(e, pos) => {
                self.mark(*pos);
                self.load_this();
                self.emit(Op::Dup);
                self.expr(e)?;
                self.emit(Op::ToPropertyKey);
                self.emit(Op::SuperGetElem);
                Ok(true)
            }
            Expr::Paren(inner) if matches!(**inner, Expr::Member { .. } | Expr::Index { .. }) => self.callee(inner),
            Expr::Ident(n, pos) if self.dyn_count > 0 => match self.dyn_lookup(n) {
                // a function found in a `with` object is called with that object as `this`
                Some(d) => {
                    self.mark(*pos);
                    self.emit(Op::DynCallee(d));
                    Ok(true)
                }
                None => {
                    self.expr(callee)?;
                    Ok(false)
                }
            },
            _ => {
                self.expr(callee)?;
                Ok(false)
            }
        }
    }

    fn template(&mut self, t: &TemplateParts) -> CResult<()> {
        let first = self.name16(t.cooked[0].clone().unwrap_or_default());
        self.emit(Op::Str(first));
        for (i, e) in t.exprs.iter().enumerate() {
            self.expr(e)?;
            self.emit(Op::ToStr);
            self.emit(Op::Add);
            let q = t.cooked.get(i + 1).cloned().flatten().unwrap_or_default();
            if !q.is_empty() {
                let n = self.name16(q);
                self.emit(Op::Str(n));
                self.emit(Op::Add);
            }
        }
        Ok(())
    }

    fn expr_inner(&mut self, e: &Expr) -> CResult<()> {
        match e {
            Expr::Num(n) => {
                self.emit(Op::Num(*n));
            }
            Expr::BigInt(text) => {
                let n = self.name(text);
                self.emit(Op::BigInt(n));
            }
            Expr::Str(s) => {
                let n = self.name16(s.clone());
                self.emit(Op::Str(n));
            }
            Expr::Bool(b) => {
                self.emit(if *b { Op::True } else { Op::False });
            }
            Expr::Null => {
                self.emit(Op::Null);
            }
            Expr::Template(t) => self.template(t)?,
            Expr::Tagged(tag, t, pos) => {
                let method = self.callee(tag)?;
                let idx = self.fr().templates.len() as u32;
                self.f().templates.push(Template { cooked: t.cooked.clone(), raw: t.raw.clone() });
                self.emit(Op::TemplateObject(idx));
                for e in &t.exprs {
                    self.expr(e)?;
                }
                self.mark(*pos);
                self.describe_site(tag);
                let argc = t.exprs.len() as u32 + 1;
                self.emit(if method { Op::CallMethod(argc) } else { Op::Call(argc) });
            }
            Expr::Regex(p, f) => {
                let p = self.name(p);
                let f = self.name(f);
                self.emit(Op::Regex(p, f));
            }
            Expr::Ident(n, pos) => {
                self.mark(*pos);
                if n == "undefined" && self.dyn_count == 0 && matches!(self.resolve(n), Ref::Global) {
                    self.emit(Op::Undef);
                } else {
                    self.get_var(n);
                }
            }
            Expr::This => self.load_this(),
            Expr::NewTarget => {
                self.emit(Op::LoadNewTarget);
            }
            Expr::Array(items) => self.items_array(items)?,
            Expr::Object(props) => self.object(props)?,
            Expr::Function(f) => {
                let idx = self.compile_function(f, None)?;
                self.emit(Op::Closure(idx));
            }
            Expr::Class(c) => self.class(c)?,
            Expr::Unary(op, arg, pos) => self.unary(*op, arg, *pos)?,
            Expr::Update { inc, prefix, target, pos } => self.update(*inc, *prefix, target, *pos)?,
            Expr::Binary(op, a, b, pos) => {
                self.expr(a)?;
                self.expr(b)?;
                self.mark(*pos);
                self.emit(bin_op(*op));
            }
            Expr::Logical(op, a, b) => {
                self.expr(a)?;
                let j = self.emit(match op {
                    LogicOp::And => Op::AndJump(0),
                    LogicOp::Or => Op::OrJump(0),
                    LogicOp::Nullish => Op::NullishJump(0),
                });
                self.expr(b)?;
                self.patch(j);
            }
            Expr::Assign(op, target, value, pos) => self.assign(*op, target, value, *pos)?,
            Expr::Cond(t, a, b) => {
                self.expr(t)?;
                let j = self.emit(Op::JumpIfFalse(0));
                self.expr(a)?;
                let j2 = self.emit(Op::Jump(0));
                self.patch(j);
                self.expr(b)?;
                self.patch(j2);
            }
            Expr::Call { callee, args, optional: false, pos }
                if matches!(&**callee, Expr::Ident(n, _) if n == "eval") && !args.iter().any(|a| matches!(a, ArrayItem::Spread(_))) =>
            {
                // direct eval: the code sees this scope, described by a snapshot of it
                self.get_var("eval");
                let argc = self.args(args)?.unwrap_or(0);
                self.mark(*pos);
                let snap = std::sync::Arc::new(self.snapshot_scopes());
                let f = self.f();
                f.eval_scopes.push(snap);
                let idx = f.eval_scopes.len() as u32 - 1;
                self.emit(Op::DirectEval(argc, idx));
            }
            Expr::Call { callee, args, optional, pos } => {
                let method = self.callee(callee)?;
                if *optional {
                    self.opt_jump(if method { 2 } else { 1 });
                }
                let argc = self.args(args)?;
                self.mark(*pos);
                self.describe_site(callee);
                self.emit(match (method, argc) {
                    (true, Some(n)) => Op::CallMethod(n),
                    (false, Some(n)) => Op::Call(n),
                    (true, None) => Op::CallMethodArr,
                    (false, None) => Op::CallArr,
                });
            }
            Expr::New { callee, args, pos } => {
                self.expr(callee)?;
                let argc = self.args(args)?;
                self.mark(*pos);
                self.describe_site(callee);
                self.emit(match argc {
                    Some(n) => Op::New(n),
                    None => Op::NewArr,
                });
            }
            Expr::Member { obj, name, optional, pos } => {
                self.expr(obj)?;
                if *optional {
                    self.opt_jump(1);
                }
                self.mark(*pos);
                let n = self.name(name);
                self.emit(Op::GetProp(n));
            }
            Expr::Index { obj, index, optional, pos } => {
                self.expr(obj)?;
                if *optional {
                    self.opt_jump(1);
                }
                self.expr(index)?;
                self.mark(*pos);
                self.emit(Op::GetElem);
            }
            Expr::PrivateMember { obj, name, optional, pos } => {
                self.expr(obj)?;
                if *optional {
                    self.opt_jump(1);
                }
                let n = self.get_private(name)?;
                self.mark(*pos);
                self.emit(Op::PrivGet(n));
            }
            Expr::PrivateIn(name, obj, pos) => {
                self.get_private(name)?;
                self.expr(obj)?;
                self.mark(*pos);
                self.emit(Op::PrivIn);
            }
            Expr::PrivateDefine { name, value, kind } => {
                self.load_this();
                let n = self.get_private(name)?;
                self.expr_with_name(value, Some(s16(&format!("#{}", name))))?;
                self.emit(Op::PrivDefine(*kind as u32, n));
            }
            Expr::SuperMember(..) | Expr::SuperIndex(..) => {
                self.callee(e)?;
                self.emit(Op::Swap);
                self.emit(Op::Pop);
            }
            Expr::SuperCall(args, pos) => {
                let argc = self.args(args)?;
                self.mark(*pos);
                self.emit(match argc {
                    Some(n) => Op::SuperCall(n),
                    None => Op::SuperCallArr,
                });
                self.emit(Op::Dup);
                self.init_this();
            }
            Expr::OptChain(inner) => {
                self.f().opt_jumps.push(Vec::new());
                self.expr(inner)?;
                let jumps = self.f().opt_jumps.pop().unwrap_or_default();
                let end = self.emit(Op::Jump(0));
                for j in jumps {
                    self.patch(j);
                }
                self.emit(Op::Undef);
                self.patch(end);
            }
            Expr::Seq(list) => {
                for (i, e) in list.iter().enumerate() {
                    self.expr(e)?;
                    if i + 1 < list.len() {
                        self.emit(Op::Pop);
                    }
                }
            }
            Expr::Paren(e) => self.expr(e)?,
            Expr::Await(e) => {
                self.expr(e)?;
                self.emit(Op::Await);
            }
            Expr::Yield(Some(arg), true) => self.yield_delegate(arg)?,
            Expr::Yield(arg, _) => {
                match arg {
                    Some(a) => self.expr(a)?,
                    None => {
                        self.emit(Op::Undef);
                    }
                }
                if self.fr().is_async {
                    self.emit(Op::Await);
                }
                self.emit(Op::Yield);
            }
        }
        Ok(())
    }

    /// `[obj, fn] -> [obj, fn]`, jumping when `fn` is null or undefined.
    fn jump_if_nullish_keep(&mut self) -> usize {
        self.emit(Op::Dup);
        self.emit(Op::Null);
        self.emit(Op::Eq);
        self.emit(Op::JumpIfTrue(0))
    }

    /// `yield* arg`: drives the inner iterator with ordinary method calls, forwarding the
    /// resume mode (next / throw / return) the outer generator receives.
    fn yield_delegate(&mut self, arg: &Expr) -> CResult<()> {
        let is_async = self.fr().is_async;
        let (t_iter, t_recv) = (self.alloc_local(), self.alloc_local());
        let (n_next, n_throw, n_return) = (self.name("next"), self.name("throw"), self.name("return"));
        let (n_done, n_value) = (self.name("done"), self.name("value"));
        self.expr(arg)?;
        self.emit(if is_async { Op::GetAsyncIter } else { Op::GetIter });
        self.emit(Op::InitLocal(t_iter));
        self.emit(Op::Undef);
        self.emit(Op::InitLocal(t_recv));
        let l_next = self.here();
        self.emit(Op::GetLocal(t_iter));
        self.emit(Op::GetPropKeep(n_next));
        self.emit(Op::GetLocal(t_recv));
        self.emit(Op::CallMethod(1));
        let l_result = self.here(); // [result]
        if is_async {
            self.emit(Op::Await);
        }
        self.emit(Op::CheckIterResult);
        self.emit(Op::Dup);
        self.emit(Op::GetProp(n_done));
        let j_done = self.emit(Op::JumpIfTrue(0));
        let l_yield = self.here(); // [result]
        self.emit(Op::GetProp(n_value));
        self.emit(Op::YieldDelegate); // [received, mode]
        self.emit(Op::Dup);
        self.emit(Op::Num(0.0));
        self.emit(Op::StrictEq);
        let j_not_next = self.emit(Op::JumpIfFalse(0));
        self.emit(Op::Pop);
        self.emit(Op::InitLocal(t_recv));
        self.emit(Op::Jump(l_next));
        self.patch(j_not_next); // [received, mode]
        self.emit(Op::Num(1.0));
        self.emit(Op::StrictEq);
        let j_return = self.emit(Op::JumpIfFalse(0));
        // throw(): forward to the inner iterator when it has a `throw` method
        self.emit(Op::InitLocal(t_recv));
        self.emit(Op::GetLocal(t_iter));
        self.emit(Op::GetPropKeep(n_throw)); // [iter, fn]
        let j_no_throw = self.jump_if_nullish_keep();
        self.emit(Op::GetLocal(t_recv));
        self.emit(Op::CallMethod(1));
        self.emit(Op::Jump(l_result));
        self.patch(j_no_throw);
        self.emit(Op::Pop);
        self.emit(Op::IterClose);
        let msg = self.name("The iterator does not provide a 'throw' method");
        self.emit(Op::ThrowTypeError(msg));
        // return(): forward to the inner iterator's `return`, then finish this generator
        self.patch(j_return); // [received]
        self.emit(Op::InitLocal(t_recv));
        self.emit(Op::GetLocal(t_iter));
        self.emit(Op::GetPropKeep(n_return)); // [iter, fn]
        let j_no_return = self.jump_if_nullish_keep();
        self.emit(Op::GetLocal(t_recv));
        self.emit(Op::CallMethod(1)); // [result]
        if is_async {
            self.emit(Op::Await);
        }
        self.emit(Op::CheckIterResult);
        self.emit(Op::Dup);
        self.emit(Op::GetProp(n_done));
        self.emit(Op::JumpIfFalse(l_yield));
        self.emit(Op::GetProp(n_value));
        self.emit(Op::GenReturn);
        self.patch(j_no_return); // [iter, fn]
        self.emit(Op::Pop);
        self.emit(Op::Pop);
        self.emit(Op::GetLocal(t_recv));
        self.emit(Op::GenReturn);
        self.patch(j_done); // [result]
        self.emit(Op::GetProp(n_value));
        Ok(())
    }

    fn init_this(&mut self) {
        match self.resolve("this") {
            Ref::Local(i, _) => self.emit(Op::InitLocal(i)),
            Ref::Env(h, s, _) => self.emit(Op::InitEnv(h, s)),
            Ref::Global => self.emit(Op::Pop),
        };
    }

    fn unary(&mut self, op: UnOp, arg: &Expr, pos: Pos) -> CResult<()> {
        match op {
            UnOp::TypeOf => {
                if let Expr::Ident(n, _) = arg {
                    if let Some(d) = self.dyn_lookup(n) {
                        self.emit(Op::DynTypeof(d));
                        self.emit(Op::TypeOf);
                        return Ok(());
                    }
                    if let Ref::Global = self.resolve(n) {
                        let id = self.name(n);
                        self.emit(Op::GetGlobalTypeof(id));
                        self.emit(Op::TypeOf);
                        return Ok(());
                    }
                }
                self.expr(arg)?;
                self.emit(Op::TypeOf);
            }
            UnOp::Delete => match arg {
                Expr::Member { obj, name, .. } => {
                    self.expr(obj)?;
                    let n = self.name(name);
                    self.mark(pos);
                    self.emit(Op::DeleteProp(n));
                }
                Expr::Index { obj, index, .. } => {
                    self.expr(obj)?;
                    self.expr(index)?;
                    self.mark(pos);
                    self.emit(Op::DeleteElem);
                }
                Expr::Ident(n, _) => {
                    if let Some(d) = self.dyn_lookup(n) {
                        self.emit(Op::DynDelete(d));
                    } else if let Ref::Global = self.resolve(n) {
                        let id = self.name(n);
                        self.emit(Op::DeleteGlobal(id));
                    } else {
                        self.emit(Op::False);
                    }
                }
                Expr::Paren(inner) => return self.unary(op, inner, pos),
                _ => {
                    self.expr(arg)?;
                    self.emit(Op::Pop);
                    self.emit(Op::True);
                }
            },
            _ => {
                self.expr(arg)?;
                self.mark(pos);
                match op {
                    UnOp::Neg => self.emit(Op::Neg),
                    UnOp::Plus => self.emit(Op::Plus),
                    UnOp::Not => self.emit(Op::Not),
                    UnOp::BitNot => self.emit(Op::BitNot),
                    _ => {
                        self.emit(Op::Pop);
                        self.emit(Op::Undef)
                    }
                };
            }
        }
        Ok(())
    }

    fn update(&mut self, inc: bool, prefix: bool, target: &Expr, pos: Pos) -> CResult<()> {
        let step = if inc { Op::Inc } else { Op::Dec };
        match target {
            Expr::Ident(n, _) => {
                self.get_var(n);
                self.emit(Op::ToNumeric);
                if prefix {
                    self.emit(step);
                    self.emit(Op::Dup);
                } else {
                    self.emit(Op::Dup);
                    self.emit(step);
                }
                self.put_var(n);
            }
            Expr::Member { obj, name, pos: p, .. } => {
                self.expr(obj)?;
                self.emit(Op::Dup);
                self.mark(*p);
                let n = self.name(name);
                self.emit(Op::GetProp(n));
                self.emit(Op::ToNumeric);
                if prefix {
                    self.emit(step);
                    self.emit(Op::PutProp(n));
                } else {
                    self.emit(Op::Dup);
                    self.emit(Op::Rot(3));
                    self.emit(step);
                    self.emit(Op::PutProp(n));
                    self.emit(Op::Pop);
                }
            }
            Expr::Index { obj, index, pos: p, .. } => {
                self.expr(obj)?;
                self.expr(index)?;
                self.emit(Op::ToPropertyKey);
                self.emit(Op::Dup2);
                self.mark(*p);
                self.emit(Op::GetElem);
                self.emit(Op::ToNumeric);
                if prefix {
                    self.emit(step);
                    self.emit(Op::PutElem);
                } else {
                    self.emit(Op::Dup);
                    self.emit(Op::Rot(4));
                    self.emit(step);
                    self.emit(Op::PutElem);
                    self.emit(Op::Pop);
                }
            }
            Expr::PrivateMember { obj, name, pos: p, .. } => {
                self.expr(obj)?;
                let n = self.get_private(name)?;
                self.emit(Op::Dup2);
                self.mark(*p);
                self.emit(Op::PrivGet(n));
                self.emit(Op::ToNumeric);
                if prefix {
                    self.emit(step);
                    self.emit(Op::PrivPut(n));
                } else {
                    self.emit(Op::Dup);
                    self.emit(Op::Rot(4));
                    self.emit(step);
                    self.emit(Op::PrivPut(n));
                    self.emit(Op::Pop);
                }
            }
            _ => return Err(self.err("Invalid left-hand side expression in update operation", pos)),
        }
        Ok(())
    }
}
