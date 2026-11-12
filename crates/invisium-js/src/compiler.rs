//! AST -> bytecode compiler: state, scopes and name resolution, function driver.

use crate::analyze::{analyze, block_func_names, pattern_names, var_names};
use crate::ast::*;
use crate::bytecode::*;
use crate::parser::ParseError;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BKind {
    Var,
    Let,
    Const,
    /// A module's import binding: its env slot holds a cell naming the exporter's binding.
    Import,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Loc { Local(u32), Env(u32) }

pub struct Binding {
    pub name: String,
    pub loc: Loc,
    pub kind: BKind,
}

#[derive(Default)]
pub struct Scope {
    pub bindings: Vec<Binding>,
    pub env: bool,
    pub env_slots: u32,
    /// Env slot holding a scope object consulted by name at run time: a `with` object, or
    /// the variables a sloppy direct `eval` declared in this function.
    pub obj_slot: Option<u32>,
    pub is_with: bool,
}

pub enum CtlKind {
    /// `iter`: what the loop keeps on the operand stack — 0 nothing, 1 for-in keys,
    /// 2 a sync iterator (for-of), 3 an async iterator (for-await).
    Loop { iter: u8 },
    Switch,
    Block,
    Finally { body: Vec<Stmt>, pops_try: bool },
}

pub struct Control {
    pub kind: CtlKind,
    pub labels: Vec<String>,
    pub breaks: Vec<usize>,
    pub continues: Vec<usize>,
    pub scope_break: usize,
    pub scope_cont: usize,
}

pub struct FnState {
    pub ops: Vec<Op>,
    pub lines: Vec<(u32, Pos)>,
    pub children: Vec<Arc<FuncCode>>,
    pub templates: Vec<Template>,
    pub scopes: Vec<Scope>,
    pub nlocals: u32,
    pub captured: HashSet<String>,
    pub controls: Vec<Control>,
    pub kind: FuncKind,
    pub is_async: bool,
    pub is_gen: bool,
    pub strict: bool,
    pub is_script: bool,
    pub opt_jumps: Vec<Vec<usize>>,
    pub completion: Option<u32>,
    pub descs: Vec<(u32, Box<str>)>,
    /// Contains a direct `eval`: every binding lives in a heap environment.
    pub capture_all: bool,
    /// Sloppy eval code: its `var`s belong to the enclosing function.
    pub is_eval: bool,
    pub dyn_refs: Vec<DynRef>,
    pub eval_scopes: Vec<Arc<EvalScope>>,
    /// Sloppy mode: block-level function names that are also `var`s of this function.
    pub annexb: Vec<String>,
}

pub enum Ref { Local(u32, BKind), Env(u32, u32, BKind), Global }

pub struct Compiler {
    pub fns: Vec<FnState>,
    pub names: Vec<Str16>,
    pub name_map: HashMap<Str16, u32>,
    pub source: Arc<[char]>,
    pub script: Arc<str>,
    pub depth: u32,
    /// Scope objects (`Scope::obj_slot`) currently in scope, over all enclosing functions.
    pub dyn_count: u32,
    /// The code is compiled on behalf of a script (see `FuncCode::dynamic`).
    pub dynamic: bool,
}

pub type CResult<T> = Result<T, ParseError>;

pub fn compile_program(prog: &Program, source: Arc<[char]>, script: &str, dynamic: bool) -> CResult<CompiledScript> {
    let mut c = Compiler { fns: Vec::new(), names: Vec::new(), name_map: HashMap::new(), source, script: script.into(), depth: 0, dyn_count: 0, dynamic };
    let top = c.compile_script(prog)?;
    Ok(CompiledScript { names: c.names, top })
}

pub fn s16(s: &str) -> Str16 {
    s.encode_utf16().collect()
}

fn is_ident_text(s: &str) -> bool {
    let mut it = s.chars();
    matches!(it.next(), Some(c) if crate::lexer::is_id_start(c)) && it.all(crate::lexer::is_id_part)
}

/// Renders a callee-like expression the way engines name it in "x.foo is not a function".
pub fn expr_text(e: &Expr, depth: u32) -> Option<String> {
    if depth > 12 {
        return None;
    }
    let sub = |o: &Expr| expr_text(o, depth + 1).unwrap_or_else(|| "(intermediate value)".to_string());
    let t = match e {
        Expr::Ident(n, _) => n.clone(),
        Expr::This => "this".to_string(),
        Expr::Member { obj, name, optional, .. } => format!("{}{}{}", sub(obj), if *optional { "?." } else { "." }, name),
        Expr::Index { obj, index, .. } => match &**index {
            Expr::Str(s) => {
                let k = String::from_utf16_lossy(s);
                if is_ident_text(&k) { format!("{}.{}", sub(obj), k) } else { format!("{}[...]", sub(obj)) }
            }
            Expr::Num(n) => format!("{}[{}]", sub(obj), crate::numfmt::number_to_string(*n)),
            Expr::Ident(k, _) => format!("{}[{}]", sub(obj), k),
            _ => format!("{}[...]", sub(obj)),
        },
        Expr::Call { callee, .. } => format!("{}(...)", sub(callee)),
        Expr::SuperMember(n, _) => format!("super.{}", n),
        Expr::PrivateMember { obj, name, .. } => format!("{}.#{}", sub(obj), name),
        Expr::Paren(inner) | Expr::OptChain(inner) => return expr_text(inner, depth + 1),
        _ => return None,
    };
    if t.chars().count() > 100 {
        return Some(t.chars().take(97).chain("...".chars()).collect());
    }
    Some(t)
}

impl Compiler {
    pub fn f(&mut self) -> &mut FnState {
        self.fns.last_mut().expect("function state")
    }

    pub fn fr(&self) -> &FnState {
        self.fns.last().expect("function state")
    }

    pub fn err(&self, msg: &str, pos: Pos) -> ParseError {
        ParseError { message: msg.to_string(), line: pos.line, column: pos.col }
    }

    pub fn enter(&mut self, pos: Pos) -> CResult<()> {
        self.depth += 1;
        if self.depth > 20000 {
            return Err(self.err("Expression too deeply nested", pos));
        }
        Ok(())
    }

    pub fn name16(&mut self, s: Str16) -> u32 {
        if let Some(&i) = self.name_map.get(&s) {
            return i;
        }
        let i = self.names.len() as u32;
        self.names.push(s.clone());
        self.name_map.insert(s, i);
        i
    }

    pub fn name(&mut self, s: &str) -> u32 {
        self.name16(s16(s))
    }

    pub fn emit(&mut self, op: Op) -> usize {
        let f = self.f();
        f.ops.push(op);
        f.ops.len() - 1
    }

    pub fn here(&self) -> u32 {
        self.fr().ops.len() as u32
    }

    pub fn mark(&mut self, pos: Pos) {
        if pos.line == 0 {
            return;
        }
        let pc = self.here();
        let f = self.f();
        if let Some(last) = f.lines.last_mut() {
            if last.0 == pc {
                last.1 = pos;
                return;
            }
            if last.1 == pos {
                return;
            }
        }
        f.lines.push((pc, pos));
    }

    /// Records the source-like text of `e` for the op about to be emitted (error messages).
    pub fn describe_site(&mut self, e: &Expr) {
        if let Some(t) = expr_text(e, 0) {
            let pc = self.here();
            let f = self.f();
            if f.descs.last().map(|d| d.0) != Some(pc) {
                f.descs.push((pc, t.into_boxed_str()));
            }
        }
    }

    pub fn patch(&mut self, at: usize) {
        let t = self.here();
        self.patch_to(at, t);
    }

    pub fn patch_to(&mut self, at: usize, t: u32) {
        let op = &mut self.f().ops[at];
        *op = match *op {
            Op::Jump(_) => Op::Jump(t),
            Op::JumpIfFalse(_) => Op::JumpIfFalse(t),
            Op::JumpIfTrue(_) => Op::JumpIfTrue(t),
            Op::AndJump(_) => Op::AndJump(t),
            Op::OrJump(_) => Op::OrJump(t),
            Op::NullishJump(_) => Op::NullishJump(t),
            Op::OptJump(_, n) => Op::OptJump(t, n),
            Op::JumpIfObject(_) => Op::JumpIfObject(t),
            Op::PushTry(_) => Op::PushTry(t),
            Op::IterNext(_) => Op::IterNext(t),
            Op::ForInNext(_) => Op::ForInNext(t),
            other => other,
        };
    }

    pub fn alloc_local(&mut self) -> u32 {
        let f = self.f();
        f.nlocals += 1;
        f.nlocals - 1
    }

    /// Declares a binding in the innermost scope. The scope's `env` flag must be set by
    /// `push_scope_for` before any binding is declared.
    pub fn declare(&mut self, name: &str, kind: BKind) -> Loc {
        if let Some(b) = self.fr().scopes.last().and_then(|s| s.bindings.iter().find(|b| b.name == name)) {
            return b.loc;
        }
        let captured = self.fr().capture_all || self.fr().captured.contains(name);
        let loc = if captured {
            let s = self.f().scopes.last_mut().expect("scope");
            s.env_slots += 1;
            Loc::Env(s.env_slots - 1)
        } else {
            Loc::Local(self.alloc_local())
        };
        let s = self.f().scopes.last_mut().expect("scope");
        s.bindings.push(Binding { name: name.to_string(), loc, kind });
        loc
    }

    /// Pushes a block scope for the given names; emits PushEnv when any is captured.
    pub fn push_scope(&mut self, names: &[(String, BKind)]) {
        let env = (self.fr().capture_all && !names.is_empty()) || names.iter().any(|(n, _)| self.fr().captured.contains(n));
        self.f().scopes.push(Scope { env, ..Default::default() });
        for (n, k) in names {
            self.declare(n, *k);
        }
        if env {
            let slots = self.fr().scopes.last().map(|s| s.env_slots).unwrap_or(0);
            self.emit(Op::PushEnv(slots));
        }
        // TDZ for lexical locals (blocks may be re-entered in loops)
        let locals: Vec<(Loc, BKind)> = self.fr().scopes.last().map(|s| s.bindings.iter().map(|b| (b.loc, b.kind)).collect()).unwrap_or_default();
        for (loc, kind) in locals {
            if let (Loc::Local(i), BKind::Let | BKind::Const) = (loc, kind) {
                self.emit(Op::ClearLocal(i));
            }
        }
    }

    pub fn pop_scope(&mut self) {
        if let Some(s) = self.f().scopes.pop() {
            if s.env {
                self.emit(Op::PopEnv);
            }
            if s.obj_slot.is_some() {
                self.dyn_count -= 1;
            }
        }
    }

    /// The scope objects a reference to `name` passes before reaching its static binding.
    fn dyn_chain(&self, name: &str) -> Vec<(u32, u32, bool)> {
        let mut out = Vec::new();
        let mut hops = 0;
        for f in self.fns.iter().rev() {
            for s in f.scopes.iter().rev() {
                if s.bindings.iter().any(|b| b.name == name) {
                    return out;
                }
                if let Some(slot) = s.obj_slot {
                    out.push((hops, slot, s.is_with));
                }
                if s.env {
                    hops += 1;
                }
            }
        }
        out
    }

    /// Registers a dynamic reference when `name` must be looked up in scope objects first.
    pub fn dyn_lookup(&mut self, name: &str) -> Option<u32> {
        // (the hidden `#...` bindings of private class members are never looked up by name)
        if self.dyn_count == 0 || name.starts_with('#') {
            return None;
        }
        let objs = self.dyn_chain(name);
        if objs.is_empty() {
            return None;
        }
        let (base, is_const) = match self.resolve(name) {
            Ref::Local(i, k) => (DynBase::Local(i), k == BKind::Const),
            Ref::Env(h, s, k) => (DynBase::Env(h, s), k == BKind::Const),
            Ref::Global => (DynBase::Global, false),
        };
        let n = self.name(name);
        let f = self.f();
        f.dyn_refs.push(DynRef { name: n, objs, base, is_const });
        Some(f.dyn_refs.len() as u32 - 1)
    }

    pub fn resolve(&self, name: &str) -> Ref {
        let pseudo = name == "this";
        let mut hops = 0;
        let n = self.fns.len();
        for (fi, f) in self.fns.iter().enumerate().rev() {
            for s in f.scopes.iter().rev() {
                if let Some(b) = s.bindings.iter().rev().find(|b| b.name == name) {
                    return match b.loc {
                        Loc::Local(i) if fi + 1 == n => Ref::Local(i, b.kind),
                        Loc::Local(_) => Ref::Global,
                        Loc::Env(slot) => Ref::Env(hops, slot, b.kind),
                    };
                }
                if s.env {
                    hops += 1;
                }
            }
            if pseudo && f.kind != FuncKind::Arrow {
                break;
            }
        }
        Ref::Global
    }

    pub fn is_script_top(&self) -> bool {
        let f = self.fr();
        f.is_script && f.scopes.len() == 1
    }

    pub fn get_var(&mut self, name: &str) {
        if let Some(d) = self.dyn_lookup(name) {
            self.emit(Op::DynGet(d));
            return;
        }
        match self.resolve(name) {
            Ref::Local(i, _) => self.emit(Op::GetLocal(i)),
            Ref::Env(h, s, BKind::Import) => self.emit(Op::GetImport(h, s)),
            Ref::Env(h, s, _) => self.emit(Op::GetEnv(h, s)),
            Ref::Global => {
                let n = self.name(name);
                self.emit(Op::GetGlobal(n))
            }
        };
    }

    /// Assignment (pops the value).
    pub fn put_var(&mut self, name: &str) {
        if let Some(d) = self.dyn_lookup(name) {
            self.emit(Op::DynPut(d));
            return;
        }
        match self.resolve(name) {
            Ref::Local(_, BKind::Const) | Ref::Env(_, _, BKind::Const | BKind::Import) => {
                let n = self.name(name);
                self.emit(Op::ThrowConstAssign(n))
            }
            Ref::Local(i, _) => self.emit(Op::PutLocal(i)),
            Ref::Env(h, s, _) => self.emit(Op::PutEnv(h, s)),
            Ref::Global => {
                let n = self.name(name);
                self.emit(Op::PutGlobal(n))
            }
        };
    }

    /// Initialisation of a declaration in the current scope (pops the value).
    pub fn init_var(&mut self, name: &str, kind: VarKind) {
        if self.is_script_top() {
            let n = self.name(name);
            if kind == VarKind::Var {
                self.emit(Op::PutGlobal(n));
            } else {
                self.emit(Op::InitGlobalLex(n));
            }
            return;
        }
        if kind == VarKind::Var {
            if let Some(d) = self.dyn_lookup(name) {
                self.emit(Op::DynPut(d));
                return;
            }
            // var re-declarations inside blocks assign to the function-level binding
            match self.resolve(name) {
                Ref::Local(i, _) => self.emit(Op::InitLocal(i)),
                Ref::Env(h, s, _) => self.emit(Op::InitEnv(h, s)),
                Ref::Global => {
                    let n = self.name(name);
                    self.emit(Op::PutGlobal(n))
                }
            };
            return;
        }
        match self.resolve(name) {
            Ref::Local(i, _) => self.emit(Op::InitLocal(i)),
            Ref::Env(h, s, _) => self.emit(Op::InitEnv(h, s)),
            Ref::Global => {
                let n = self.name(name);
                self.emit(Op::InitGlobalLex(n))
            }
        };
    }

    pub fn load_this(&mut self) {
        match self.resolve("this") {
            Ref::Local(i, _) => self.emit(Op::GetLocal(i)),
            Ref::Env(h, s, _) => self.emit(Op::GetEnv(h, s)),
            Ref::Global => self.emit(Op::LoadThis),
        };
    }

    pub fn new_fn(&mut self, kind: FuncKind, strict: bool, captured: HashSet<String>, is_script: bool) {
        self.fns.push(FnState {
            ops: Vec::new(),
            lines: Vec::new(),
            children: Vec::new(),
            templates: Vec::new(),
            scopes: Vec::new(),
            nlocals: 0,
            captured,
            controls: Vec::new(),
            kind,
            is_async: false,
            is_gen: false,
            strict,
            is_script,
            opt_jumps: Vec::new(),
            completion: None,
            descs: Vec::new(),
            capture_all: false,
            is_eval: false,
            dyn_refs: Vec::new(),
            eval_scopes: Vec::new(),
            annexb: Vec::new(),
        });
    }

    pub fn finish_fn(&mut self, name: Str16, length: u32, range: (usize, usize)) -> FuncCode {
        let f = self.fns.pop().expect("function state");
        self.dyn_count -= f.scopes.iter().filter(|s| s.obj_slot.is_some()).count() as u32;
        FuncCode {
            name,
            ops: f.ops,
            children: f.children,
            templates: f.templates,
            lines: f.lines,
            nlocals: f.nlocals,
            length,
            kind: f.kind,
            is_async: f.is_async,
            is_gen: f.is_gen,
            strict: f.strict,
            source: self.source.clone(),
            source_range: range,
            script: self.script.clone(),
            descs: f.descs,
            dyn_refs: f.dyn_refs,
            eval_scopes: f.eval_scopes,
            dynamic: self.dynamic,
        }
    }

    fn compile_script(&mut self, prog: &Program) -> CResult<Arc<FuncCode>> {
        let refs = analyze(&[], None, &prog.body);
        self.new_fn(FuncKind::Normal, prog.strict, refs.inner, true);
        self.f().capture_all = refs.has_eval;
        // the script's top-level scope: declarations there are global
        self.f().scopes.push(Scope::default());
        let mut vars = Vec::new();
        var_names(&prog.body, &mut vars);
        if !prog.strict {
            let mut block_funcs = Vec::new();
            block_func_names(&prog.body, &mut block_funcs);
            for n in &block_funcs {
                if !vars.contains(n) {
                    vars.push(n.clone());
                }
            }
            self.f().annexb = block_funcs;
        }
        for v in &vars {
            let n = self.name(v);
            self.emit(Op::DeclareVar(n));
        }
        for s in &prog.body {
            match &s.kind {
                StmtKind::Var(k @ (VarKind::Let | VarKind::Const), d) => {
                    let mut names = Vec::new();
                    d.iter().for_each(|(p, _)| pattern_names(p, &mut names));
                    for n in names {
                        let id = self.name(&n);
                        self.emit(Op::DeclareLex(id, *k == VarKind::Const));
                    }
                }
                StmtKind::Class(c) => {
                    if let Some(n) = &c.name {
                        let id = self.name(n);
                        self.emit(Op::DeclareLex(id, false));
                    }
                }
                _ => {}
            }
        }
        for s in &prog.body {
            if let StmtKind::Func(f) = &s.kind {
                let idx = self.compile_function(f, None)?;
                self.emit(Op::Closure(idx));
                let n = self.name(f.name.as_deref().unwrap_or(""));
                self.emit(Op::DeclareFunc(n));
            }
        }
        let completion = self.alloc_local();
        self.emit(Op::Undef);
        self.emit(Op::InitLocal(completion));
        self.f().completion = Some(completion);
        for s in &prog.body {
            if !matches!(s.kind, StmtKind::Func(_)) {
                self.stmt(s)?;
            }
        }
        self.emit(Op::GetLocal(completion));
        self.emit(Op::Return);
        let len = self.source.len();
        Ok(Arc::new(self.finish_fn(Vec::new(), 0, (0, len))))
    }

    /// Lexically-scoped declarations directly inside a statement list.
    pub fn lexical_names(stmts: &[Stmt]) -> Vec<(String, BKind)> {
        let mut out = Vec::new();
        for s in stmts {
            match &s.kind {
                StmtKind::Var(k @ (VarKind::Let | VarKind::Const), d) => {
                    let mut names = Vec::new();
                    d.iter().for_each(|(p, _)| pattern_names(p, &mut names));
                    let bk = if *k == VarKind::Const { BKind::Const } else { BKind::Let };
                    out.extend(names.into_iter().map(|n| (n, bk)));
                }
                StmtKind::Class(c) => {
                    if let Some(n) = &c.name {
                        out.push((n.clone(), BKind::Let));
                    }
                }
                StmtKind::Func(f) => {
                    if let Some(n) = &f.name {
                        out.push((n.clone(), BKind::Var));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Hoists function declarations of a statement list (after its scope was pushed).
    /// `block`: the list is a block inside a function; in sloppy code its functions are then
    /// also assigned to the function-level `var` of the same name (Annex B.3.3).
    pub fn hoist_functions(&mut self, stmts: &[Stmt], block: bool) -> CResult<()> {
        for s in stmts {
            if let StmtKind::Func(f) = &s.kind {
                let idx = self.compile_function(f, None)?;
                self.emit(Op::Closure(idx));
                let name = f.name.clone().unwrap_or_default();
                if block && self.fr().annexb.contains(&name) && !self.lexically_shadowed(&name) {
                    self.emit(Op::Dup);
                    self.init_var(&name, VarKind::Let);
                    self.put_function_var(&name);
                } else {
                    self.init_var(&name, VarKind::Let);
                }
            }
        }
        Ok(())
    }

    /// True when a block between the function scope and the innermost one declares `name`
    /// with let / const / class (a `var` of that name could not be hoisted past it).
    fn lexically_shadowed(&self, name: &str) -> bool {
        let scopes = &self.fr().scopes;
        let inner = scopes.len().saturating_sub(1);
        scopes.iter().take(inner).skip(1).any(|s| s.bindings.iter().any(|b| b.name == name && b.kind != BKind::Var))
    }

    /// Assigns the value on the stack to the function-level `var` binding `name`, bypassing
    /// any block binding of the same name.
    fn put_function_var(&mut self, name: &str) {
        if self.fr().is_script {
            let n = self.name(name);
            self.emit(Op::PutGlobal(n));
            return;
        }
        let f = self.fr();
        let hops = f.scopes.iter().skip(1).filter(|s| s.env).count() as u32;
        let loc = f.scopes.first().and_then(|s| s.bindings.iter().find(|b| b.name == name)).map(|b| b.loc);
        match loc {
            Some(Loc::Local(i)) => self.emit(Op::PutLocal(i)),
            Some(Loc::Env(s)) => self.emit(Op::PutEnv(hops, s)),
            None => self.emit(Op::Pop),
        };
    }

    /// Compiles a nested function; returns its child index in the current function.
    pub fn compile_function(&mut self, f: &Function, name_hint: Option<Str16>) -> CResult<u32> {
        self.enter(f.pos)?;
        let refs = analyze(&f.params, f.rest.as_ref(), &f.body);
        let mut captured = refs.inner;
        if f.kind == FuncKind::DerivedCtor && refs.arrow_uses_this {
            captured.insert("this".to_string());
        }
        self.new_fn(f.kind, f.strict, captured, false);
        self.f().is_async = f.is_async;
        self.f().is_gen = f.is_gen;
        self.f().capture_all = refs.has_eval;
        // a sloppy function calling `eval` directly keeps the variables eval declares in an object
        let eval_vars = refs.direct_eval && !f.strict;
        let mut param_names = Vec::new();
        f.params.iter().for_each(|p| pattern_names(p, &mut param_names));
        if let Some(r) = &f.rest {
            pattern_names(r, &mut param_names);
        }
        let mut vars = Vec::new();
        var_names(&f.body, &mut vars);
        let lex = Self::lexical_names(&f.body);
        if !f.strict {
            let mut block_funcs = Vec::new();
            block_func_names(&f.body, &mut block_funcs);
            block_funcs.retain(|n| !param_names.contains(n) && !lex.iter().any(|(m, k)| m == n && *k != BKind::Var));
            for n in &block_funcs {
                if !vars.contains(n) {
                    vars.push(n.clone());
                }
            }
            self.f().annexb = block_funcs;
        }
        let mut all: Vec<(String, BKind)> = Vec::new();
        for n in &param_names {
            all.push((n.clone(), BKind::Var));
        }
        for n in &vars {
            all.push((n.clone(), BKind::Var));
        }
        all.extend(lex.iter().cloned());
        let declared = |n: &str| all.iter().any(|(m, _)| m == n);
        let needs_arguments = f.kind != FuncKind::Arrow && (refs.uses_arguments || refs.eval_uses_args) && !declared("arguments");
        // sloppy functions with a plain parameter list: `arguments[i]` aliases parameter i,
        // so the parameters live in the heap environment the arguments object points at
        let mapped_args = needs_arguments && !f.strict && f.simple_params && !f.params.is_empty();
        if mapped_args {
            for n in &param_names {
                self.f().captured.insert(n.clone());
            }
        }
        let self_name = if f.self_binding { f.name.clone().filter(|n| !declared(n)) } else { None };
        if needs_arguments {
            all.push(("arguments".into(), BKind::Var));
        }
        if let Some(n) = &self_name {
            all.push((n.clone(), BKind::Var));
        }
        if f.kind == FuncKind::DerivedCtor {
            all.push(("this".into(), BKind::Let));
        }
        let env = eval_vars || (refs.has_eval && !all.is_empty()) || all.iter().any(|(n, _)| self.fr().captured.contains(n));
        self.f().scopes.push(Scope { env, ..Default::default() });
        for (n, k) in &all {
            self.declare(n, *k);
        }
        if eval_vars {
            let s = &mut self.f().scopes[0];
            s.env_slots += 1;
            s.obj_slot = Some(s.env_slots - 1);
            self.dyn_count += 1;
        }
        if env {
            let slots = self.fr().scopes[0].env_slots;
            self.emit(Op::PushEnv(slots));
        }
        for v in &vars {
            if !param_names.contains(v) {
                self.emit(Op::Undef);
                self.init_var(v, VarKind::Let);
            }
        }
        if needs_arguments {
            self.emit(Op::CreateArguments);
            if mapped_args {
                for (i, n) in param_names.iter().enumerate() {
                    // a repeated name is the binding of its last occurrence only
                    if param_names[i + 1..].contains(n) {
                        continue;
                    }
                    if let Ref::Env(0, slot, _) = self.resolve(n) {
                        self.emit(Op::MapArgument(i as u32, slot));
                    }
                }
            }
            self.init_var("arguments", VarKind::Let);
        }
        if let Some(n) = &self_name {
            self.emit(Op::LoadCallee);
            self.init_var(n, VarKind::Let);
        }
        if f.simple_params {
            for (i, p) in f.params.iter().enumerate() {
                if let Pattern::Ident(n, _) = p {
                    self.emit(Op::GetArg(i as u32));
                    self.init_var(n, VarKind::Let);
                }
            }
        } else {
            for (i, p) in f.params.iter().enumerate() {
                self.emit(Op::GetArg(i as u32));
                self.pattern_bind(p, Some(VarKind::Let))?;
            }
            if let Some(r) = &f.rest {
                self.emit(Op::RestArgs(f.params.len() as u32));
                self.pattern_bind(r, Some(VarKind::Let))?;
            }
        }
        if f.is_gen {
            self.emit(Op::GenStart);
        }
        if f.kind == FuncKind::BaseCtor {
            self.emit(Op::RunFieldInit);
        }
        self.hoist_functions(&f.body, false)?;
        for s in &f.body {
            if !matches!(s.kind, StmtKind::Func(_)) {
                self.stmt(s)?;
            }
        }
        self.implicit_return();
        let length = f.params.iter().take_while(|p| !matches!(p, Pattern::Default(..))).count() as u32;
        let name = match (&f.name, name_hint) {
            (Some(n), _) => s16(n),
            (None, Some(h)) => h,
            (None, None) => Vec::new(),
        };
        let code = self.finish_fn(name, length, f.source);
        self.depth -= 1;
        let parent = self.f();
        parent.children.push(Arc::new(code));
        Ok(parent.children.len() as u32 - 1)
    }

    pub fn implicit_return(&mut self) {
        if self.fr().kind == FuncKind::DerivedCtor {
            self.load_this();
        } else {
            self.emit(Op::Undef);
        }
        self.emit(Op::Return);
    }

    /// Compiles a synthetic function (class field initialiser / static block) from statements.
    pub fn compile_synthetic(&mut self, body: &[Stmt], kind: FuncKind, pos: Pos) -> CResult<u32> {
        let f = Function {
            name: None,
            self_binding: false,
            params: Vec::new(),
            rest: None,
            body: body.to_vec(),
            kind,
            is_async: false,
            is_gen: false,
            strict: true,
            simple_params: true,
            pos,
            source: (0, 0),
        };
        self.compile_function(&f, None)
    }
}
