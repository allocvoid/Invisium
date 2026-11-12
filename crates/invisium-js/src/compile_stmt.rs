//! Statement compilation, control flow and exits through `finally`.

use crate::ast::*;
use crate::bytecode::Op;
use crate::compiler::*;

#[derive(Clone, Copy, PartialEq)]
enum Exit { Break, Continue, Return }

impl Compiler {
    pub fn stmts(&mut self, list: &[Stmt]) -> CResult<()> {
        for s in list {
            self.stmt(s)?;
        }
        Ok(())
    }

    pub fn block(&mut self, list: &[Stmt]) -> CResult<()> {
        let lex = Self::lexical_names(list);
        if lex.is_empty() {
            return self.stmts(list);
        }
        self.push_scope(&lex);
        self.hoist_functions(list, true)?;
        self.stmts(list)?;
        self.pop_scope();
        Ok(())
    }

    fn push_ctl(&mut self, kind: CtlKind, labels: Vec<String>) {
        let depth = self.fr().scopes.len();
        self.f().controls.push(Control { kind, labels, breaks: Vec::new(), continues: Vec::new(), scope_break: depth, scope_cont: depth });
    }

    fn pop_ctl(&mut self, break_target: u32, continue_target: u32) {
        if let Some(c) = self.f().controls.pop() {
            for b in c.breaks {
                self.patch_to(b, break_target);
            }
            for b in c.continues {
                self.patch_to(b, continue_target);
            }
        }
    }

    pub fn stmt(&mut self, s: &Stmt) -> CResult<()> {
        self.mark(s.pos);
        self.enter(s.pos)?;
        let r = self.stmt_inner(s, Vec::new());
        self.depth -= 1;
        r
    }

    fn stmt_inner(&mut self, s: &Stmt, labels: Vec<String>) -> CResult<()> {
        match &s.kind {
            StmtKind::Expr(e) => {
                self.expr(e)?;
                match self.fr().completion {
                    Some(c) => {
                        self.emit(Op::PutLocal(c));
                    }
                    _ => {
                        self.emit(Op::Pop);
                    }
                }
            }
            StmtKind::Var(kind, decls) => {
                for (p, init) in decls {
                    match init {
                        Some(e) => {
                            self.expr_named(e, p)?;
                            self.pattern_bind(p, Some(*kind))?;
                        }
                        None => {
                            if *kind != VarKind::Var {
                                self.emit(Op::Undef);
                                self.pattern_bind(p, Some(*kind))?;
                            }
                        }
                    }
                }
            }
            StmtKind::Func(_) => {}
            StmtKind::Class(c) => {
                self.class(c)?;
                let name = c.name.clone().unwrap_or_default();
                self.init_var(&name, VarKind::Let);
            }
            StmtKind::Return(e) => {
                match e {
                    Some(e) => self.expr(e)?,
                    None => {
                        if self.fr().kind == FuncKind::DerivedCtor {
                            self.load_this();
                        } else {
                            self.emit(Op::Undef);
                        }
                    }
                }
                if self.fr().kind == FuncKind::DerivedCtor && e.is_some() {
                    let j = self.emit(Op::JumpIfObject(0));
                    self.emit(Op::Pop);
                    self.load_this();
                    self.patch(j);
                }
                if self.fr().is_async && self.fr().is_gen && e.is_some() {
                    // an async generator awaits its return value
                    self.emit(Op::Await);
                }
                if self.fr().controls.is_empty() {
                    self.emit(Op::Return);
                } else {
                    let tmp = self.alloc_local();
                    self.emit(Op::InitLocal(tmp));
                    self.exit(Exit::Return, None, s.pos)?;
                    self.emit(Op::GetLocal(tmp));
                    self.emit(Op::Return);
                }
            }
            StmtKind::If(t, a, b) => {
                self.expr(t)?;
                let j = self.emit(Op::JumpIfFalse(0));
                self.stmt(a)?;
                if let Some(b) = b {
                    let j2 = self.emit(Op::Jump(0));
                    self.patch(j);
                    self.stmt(b)?;
                    self.patch(j2);
                } else {
                    self.patch(j);
                }
            }
            StmtKind::Block(b) => {
                if !labels.is_empty() {
                    self.push_ctl(CtlKind::Block, labels);
                    self.block(b)?;
                    let h = self.here();
                    self.pop_ctl(h, h);
                } else {
                    self.block(b)?;
                }
            }
            StmtKind::Labeled(l, body) => {
                let mut labels = labels;
                labels.push(l.clone());
                let is_loop = matches!(body.kind, StmtKind::For { .. } | StmtKind::ForIn(..) | StmtKind::ForOf(..) | StmtKind::ForAwait(..) | StmtKind::While(..) | StmtKind::DoWhile(..) | StmtKind::Labeled(..));
                if is_loop {
                    return self.stmt_inner(body, labels);
                }
                self.push_ctl(CtlKind::Block, labels);
                self.stmt(body)?;
                let h = self.here();
                self.pop_ctl(h, h);
            }
            StmtKind::While(t, body) => {
                let top = self.here();
                self.expr(t)?;
                let j = self.emit(Op::JumpIfFalse(0));
                self.push_ctl(CtlKind::Loop { iter: 0 }, labels);
                self.stmt(body)?;
                self.emit(Op::Jump(top));
                self.patch(j);
                let h = self.here();
                self.pop_ctl(h, top);
            }
            StmtKind::DoWhile(body, t) => {
                let top = self.here();
                self.push_ctl(CtlKind::Loop { iter: 0 }, labels);
                self.stmt(body)?;
                let cont = self.here();
                self.expr(t)?;
                self.emit(Op::JumpIfTrue(top));
                let h = self.here();
                self.pop_ctl(h, cont);
            }
            StmtKind::For { init, test, update, body } => self.for_loop(init.as_ref(), test.as_ref(), update.as_ref(), body, labels)?,
            StmtKind::ForIn(head, obj, body) => self.for_in_of(head, obj, body, labels, 1)?,
            StmtKind::ForOf(head, obj, body) => self.for_in_of(head, obj, body, labels, 2)?,
            StmtKind::ForAwait(head, obj, body) => self.for_in_of(head, obj, body, labels, 3)?,
            StmtKind::Break(l) => self.exit(Exit::Break, l.as_deref(), s.pos)?,
            StmtKind::Continue(l) => self.exit(Exit::Continue, l.as_deref(), s.pos)?,
            StmtKind::Throw(e) => {
                self.expr(e)?;
                self.mark(s.pos);
                self.emit(Op::Throw);
            }
            StmtKind::Try { block, param, handler, finalizer } => self.try_stmt(block, param.as_ref(), handler.as_deref(), finalizer.as_deref())?,
            StmtKind::Switch(d, cases) => self.switch(d, cases, labels)?,
            StmtKind::With(obj, body) => {
                // the object goes into a one-slot environment that name lookups consult first
                self.expr(obj)?;
                self.emit(Op::ToObject);
                self.f().scopes.push(Scope { env: true, env_slots: 1, obj_slot: Some(0), is_with: true, ..Default::default() });
                self.dyn_count += 1;
                self.emit(Op::PushEnv(1));
                self.emit(Op::InitEnv(0, 0));
                self.stmt(body)?;
                self.pop_scope();
            }
            StmtKind::Empty => {}
            StmtKind::Debugger => {
                self.emit(Op::Debugger);
            }
        }
        Ok(())
    }

    fn for_loop(&mut self, init: Option<&ForInit>, test: Option<&Expr>, update: Option<&Expr>, body: &Stmt, labels: Vec<String>) -> CResult<()> {
        let mut scoped = false;
        let mut per_iter_env = false;
        match init {
            Some(ForInit::Var(k @ (VarKind::Let | VarKind::Const), decls)) => {
                let mut names = Vec::new();
                decls.iter().for_each(|(p, _)| crate::analyze::pattern_names(p, &mut names));
                let bk = if *k == VarKind::Const { BKind::Const } else { BKind::Let };
                let lex: Vec<(String, BKind)> = names.into_iter().map(|n| (n, bk)).collect();
                self.push_scope(&lex);
                per_iter_env = self.fr().scopes.last().map(|s| s.env).unwrap_or(false);
                scoped = true;
                for (p, i) in decls {
                    match i {
                        Some(e) => self.expr_named(e, p)?,
                        None => {
                            self.emit(Op::Undef);
                        }
                    }
                    self.pattern_bind(p, Some(*k))?;
                }
            }
            Some(ForInit::Var(VarKind::Var, decls)) => {
                for (p, i) in decls {
                    if let Some(e) = i {
                        self.expr_named(e, p)?;
                        self.pattern_bind(p, Some(VarKind::Var))?;
                    }
                }
            }
            Some(ForInit::Expr(e)) => {
                self.expr(e)?;
                self.emit(Op::Pop);
            }
            _ => {}
        }
        let top = self.here();
        let exit_jump = match test {
            Some(t) => {
                self.expr(t)?;
                Some(self.emit(Op::JumpIfFalse(0)))
            }
            None => None,
        };
        self.push_ctl(CtlKind::Loop { iter: 0 }, labels);
        self.stmt(body)?;
        let cont = self.here();
        if per_iter_env {
            self.emit(Op::CopyEnv);
        }
        if let Some(u) = update {
            self.expr(u)?;
            self.emit(Op::Pop);
        }
        self.emit(Op::Jump(top));
        if let Some(j) = exit_jump {
            self.patch(j);
        }
        let brk = self.here();
        self.pop_ctl(brk, cont);
        if scoped {
            self.pop_scope();
        }
        Ok(())
    }

    /// [iter] -> [] : awaits `iter.return()` when the async iterator has one.
    fn async_iter_close(&mut self) {
        let n = self.name("return");
        self.emit(Op::GetPropKeep(n));
        self.emit(Op::Dup);
        self.emit(Op::Null);
        self.emit(Op::Eq);
        let none = self.emit(Op::JumpIfTrue(0));
        self.emit(Op::CallMethod(0));
        self.emit(Op::Await);
        self.emit(Op::Pop);
        let end = self.emit(Op::Jump(0));
        self.patch(none);
        self.emit(Op::Pop);
        self.emit(Op::Pop);
        self.patch(end);
    }

    /// `kind`: 1 = for-in, 2 = for-of, 3 = for-await (see `CtlKind::Loop`).
    fn for_in_of(&mut self, head: &ForHead, obj: &Expr, body: &Stmt, labels: Vec<String>, kind: u8) -> CResult<()> {
        // TDZ scope for the head expression is approximated by evaluating it outside.
        self.expr(obj)?;
        if kind == 2 {
            self.describe_site(obj);
        }
        self.emit(match kind {
            1 => Op::ForInStart,
            2 => Op::GetIter,
            _ => Op::GetAsyncIter,
        });
        let top = self.here();
        let next = if kind == 3 {
            // [iter] -> [iter, value], or jump with [iter, result] when done
            let (n_next, n_done, n_value) = (self.name("next"), self.name("done"), self.name("value"));
            self.emit(Op::Dup);
            self.emit(Op::GetPropKeep(n_next));
            self.emit(Op::CallMethod(0));
            self.emit(Op::Await);
            self.emit(Op::CheckIterResult);
            self.emit(Op::Dup);
            self.emit(Op::GetProp(n_done));
            let done = self.emit(Op::JumpIfTrue(0));
            self.emit(Op::GetProp(n_value));
            done
        } else {
            self.emit(if kind == 2 { Op::IterNext(0) } else { Op::ForInNext(0) })
        };
        let depth_before = self.fr().scopes.len();
        let mut scoped = false;
        match head {
            ForHead::Var(k @ (VarKind::Let | VarKind::Const), p) => {
                let mut names = Vec::new();
                crate::analyze::pattern_names(p, &mut names);
                let bk = if *k == VarKind::Const { BKind::Const } else { BKind::Let };
                let lex: Vec<(String, BKind)> = names.into_iter().map(|n| (n, bk)).collect();
                self.push_scope(&lex);
                scoped = true;
                self.pattern_bind(p, Some(*k))?;
            }
            ForHead::Var(VarKind::Var, p) => self.pattern_bind(p, Some(VarKind::Var))?,
            ForHead::Pattern(p) => self.pattern_bind(p, None)?,
        }
        self.push_ctl(CtlKind::Loop { iter: kind }, labels);
        if let Some(c) = self.f().controls.last_mut() {
            c.scope_break = depth_before;
        }
        // a for-of body runs under a handler that closes the iterator when the body throws;
        // every other way out of the body (normal end, break, continue, return) pops it
        let guard = if kind == 2 { Some(self.emit(Op::PushTry(0))) } else { None };
        if guard.is_some() {
            self.push_ctl(CtlKind::Finally { body: Vec::new(), pops_try: true }, Vec::new());
        }
        self.stmt(body)?;
        if guard.is_some() {
            self.f().controls.pop();
            self.emit(Op::PopTry);
        }
        let cont = self.here();
        if scoped {
            self.pop_scope();
        }
        self.emit(Op::Jump(top));
        if let Some(g) = guard {
            self.patch(g);
            self.emit(Op::IterCloseThrow);
        }
        self.patch(next);
        if kind == 3 {
            self.emit(Op::Pop); // the final iterator result
        }
        let brk = self.here();
        self.emit(Op::Pop);
        // `break` lands after the iterator was already closed/popped by the exit sequence.
        self.pop_ctl(brk, cont);
        Ok(())
    }

    fn switch(&mut self, d: &Expr, cases: &[Case], labels: Vec<String>) -> CResult<()> {
        self.expr(d)?;
        let tmp = self.alloc_local();
        self.emit(Op::InitLocal(tmp));
        let all: Vec<Stmt> = cases.iter().flat_map(|c| c.body.iter().cloned()).collect();
        let lex = Self::lexical_names(&all);
        let scoped = !lex.is_empty();
        if scoped {
            self.push_scope(&lex);
            self.hoist_functions(&all, true)?;
        }
        let mut jumps = Vec::new();
        for c in cases {
            if let Some(t) = &c.test {
                self.emit(Op::GetLocal(tmp));
                self.expr(t)?;
                self.emit(Op::StrictEq);
                jumps.push(Some(self.emit(Op::JumpIfTrue(0))));
            } else {
                jumps.push(None);
            }
        }
        let default_jump = self.emit(Op::Jump(0));
        self.push_ctl(CtlKind::Switch, labels);
        let mut has_default = false;
        for (c, j) in cases.iter().zip(jumps) {
            match j {
                Some(j) => self.patch(j),
                None => {
                    has_default = true;
                    self.patch(default_jump);
                }
            }
            self.stmts(&c.body)?;
        }
        if !has_default {
            self.patch(default_jump);
        }
        let brk = self.here();
        self.pop_ctl(brk, brk);
        if scoped {
            self.pop_scope();
        }
        Ok(())
    }

    fn try_stmt(&mut self, block: &[Stmt], param: Option<&Pattern>, handler: Option<&[Stmt]>, finalizer: Option<&[Stmt]>) -> CResult<()> {
        let try_jump = self.emit(Op::PushTry(0));
        self.push_ctl(CtlKind::Finally { body: finalizer.map(|f| f.to_vec()).unwrap_or_default(), pops_try: true }, Vec::new());
        self.block(block)?;
        self.f().controls.pop();
        self.emit(Op::PopTry);
        if let Some(fin) = finalizer {
            self.block(fin)?;
        }
        let mut end_jumps = vec![self.emit(Op::Jump(0))];
        self.patch(try_jump);
        if let Some(h) = handler {
            // exception value is on the stack
            let inner_try = if finalizer.is_some() { Some(self.emit(Op::PushTry(0))) } else { None };
            if self.fr().is_gen {
                // a generator's return() unwinds as a marker that `catch` must not observe
                self.emit(Op::RethrowMarker);
            }
            if let Some(fin) = finalizer {
                // keep the exception below the handler: store it first
                self.push_ctl(CtlKind::Finally { body: fin.to_vec(), pops_try: true }, Vec::new());
            }
            match param {
                Some(p) => {
                    let mut names = Vec::new();
                    crate::analyze::pattern_names(p, &mut names);
                    let lex: Vec<(String, BKind)> = names.into_iter().map(|n| (n, BKind::Let)).collect();
                    self.push_scope(&lex);
                    self.pattern_bind(p, Some(VarKind::Let))?;
                    self.block(h)?;
                    self.pop_scope();
                }
                None => {
                    self.emit(Op::Pop);
                    self.block(h)?;
                }
            }
            if let Some(fin) = finalizer {
                self.f().controls.pop();
                self.emit(Op::PopTry);
                self.block(fin)?;
                end_jumps.push(self.emit(Op::Jump(0)));
                if let Some(t) = inner_try {
                    self.patch(t);
                }
            } else {
                end_jumps.push(self.emit(Op::Jump(0)));
            }
        }
        if let Some(fin) = finalizer {
            // exceptional path: exception on stack
            let tmp = self.alloc_local();
            self.emit(Op::InitLocal(tmp));
            self.block(fin)?;
            self.emit(Op::GetLocal(tmp));
            self.emit(Op::Throw);
        }
        for j in end_jumps {
            self.patch(j);
        }
        Ok(())
    }

    /// Emits the unwinding sequence for break/continue/return.
    fn exit(&mut self, kind: Exit, label: Option<&str>, pos: Pos) -> CResult<()> {
        let ctls = &self.fr().controls;
        let target = match kind {
            Exit::Return => None,
            _ => {
                let found = ctls.iter().rposition(|c| match (&c.kind, label) {
                    (CtlKind::Finally { .. }, _) => false,
                    (_, Some(l)) => c.labels.iter().any(|x| x == l) && (kind == Exit::Break || matches!(c.kind, CtlKind::Loop { .. })),
                    (CtlKind::Loop { .. }, None) => true,
                    (CtlKind::Switch, None) => kind == Exit::Break,
                    _ => false,
                });
                match found {
                    Some(i) => Some(i),
                    None => return Err(self.err("Illegal break/continue target", pos)),
                }
            }
        };
        let start = target.map(|t| t + 1).unwrap_or(0);
        let mut cur_depth = self.fr().scopes.len();
        let n = self.fr().controls.len();
        for i in (start..n).rev() {
            let (is_fin, iter, depth) = {
                let c = &self.fr().controls[i];
                match &c.kind {
                    CtlKind::Finally { .. } => (true, 0, c.scope_break),
                    CtlKind::Loop { iter } => (false, *iter, c.scope_break),
                    _ => (false, 0, c.scope_break),
                }
            };
            self.unwind_envs(cur_depth, depth);
            cur_depth = depth;
            if is_fin {
                let (body, pops) = match &self.fr().controls[i].kind {
                    CtlKind::Finally { body, pops_try } => (body.clone(), *pops_try),
                    _ => (Vec::new(), false),
                };
                if pops {
                    self.emit(Op::PopTry);
                }
                let saved_scopes = self.f().scopes.split_off(depth);
                let saved_ctls = self.f().controls.split_off(i);
                let r = self.block(&body);
                self.f().scopes.extend(saved_scopes);
                self.f().controls.extend(saved_ctls);
                r?;
            } else if iter == 2 {
                self.emit(Op::IterClose);
            } else if iter == 3 {
                self.async_iter_close();
            } else if iter == 1 {
                self.emit(Op::Pop);
            }
        }
        if let Some(t) = target {
            let (depth, iter) = {
                let c = &self.fr().controls[t];
                let d = if kind == Exit::Break { c.scope_break } else { c.scope_cont };
                match c.kind {
                    CtlKind::Loop { iter } => (d, iter),
                    _ => (d, 0),
                }
            };
            if kind == Exit::Break {
                // close the iterator, then balance the Pop at the loop exit
                // (a for-in loop leaves its key list for that Pop)
                if iter == 2 {
                    self.emit(Op::IterClose);
                    self.emit(Op::Undef);
                } else if iter == 3 {
                    self.async_iter_close();
                    self.emit(Op::Undef);
                }
            }
            self.unwind_envs(cur_depth, depth);
            let j = self.emit(Op::Jump(0));
            let c = &mut self.f().controls[t];
            if kind == Exit::Break {
                c.breaks.push(j);
            } else {
                c.continues.push(j);
            }
        }
        Ok(())
    }

    fn unwind_envs(&mut self, from: usize, to: usize) {
        if to >= from {
            return;
        }
        let count = self.fr().scopes[to..from].iter().filter(|s| s.env).count();
        for _ in 0..count {
            self.emit(Op::PopEnv);
        }
    }
}
