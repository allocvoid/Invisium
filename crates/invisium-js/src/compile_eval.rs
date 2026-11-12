//! Direct `eval`: the scope snapshot taken at a call site, and compilation of eval code
//! against such a snapshot (the code runs as an arrow-like function in the caller's
//! environment chain).

use crate::analyze::{analyze, var_names};
use crate::ast::*;
use crate::bytecode::*;
use crate::compiler::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn kind_code(k: BKind) -> u8 {
    match k {
        BKind::Var => 0,
        BKind::Let => 1,
        BKind::Const => 2,
        BKind::Import => 3,
    }
}

fn code_kind(c: u8) -> BKind {
    match c {
        0 => BKind::Var,
        1 => BKind::Let,
        3 => BKind::Import,
        _ => BKind::Const,
    }
}

/// Compiles eval code that runs at the call site described by `scope`.
pub fn compile_eval(prog: &Program, source: Arc<[char]>, script: &str, scope: &EvalScope) -> CResult<CompiledScript> {
    let mut c = Compiler { fns: Vec::new(), names: Vec::new(), name_map: HashMap::new(), source, script: script.into(), depth: 0, dyn_count: 0, dynamic: true };
    for info in &scope.fns {
        c.push_stub(info);
    }
    let top = c.compile_eval_code(prog)?;
    Ok(CompiledScript { names: c.names, top })
}

impl Compiler {
    /// The scope chain at the current position, for a direct `eval` call compiled here.
    pub fn snapshot_scopes(&self) -> EvalScope {
        let fns = self
            .fns
            .iter()
            .map(|f| FnInfo {
                kind: f.kind,
                strict: f.strict,
                is_script: f.is_script,
                is_eval: f.is_eval,
                scopes: f
                    .scopes
                    .iter()
                    .map(|s| ScopeInfo {
                        bindings: s
                            .bindings
                            .iter()
                            .filter_map(|b| match b.loc {
                                Loc::Env(slot) => Some((b.name.clone(), slot, kind_code(b.kind))),
                                Loc::Local(_) => None,
                            })
                            .collect(),
                        env: s.env,
                        obj_slot: s.obj_slot,
                        is_with: s.is_with,
                    })
                    .collect(),
            })
            .collect();
        // `this`, `new.target` and `super` belong to the nearest non-arrow function
        let owner = self.fns.iter().rev().find(|f| f.kind != FuncKind::Arrow);
        let (new_target, super_prop, super_call) = match owner {
            Some(f) if !f.is_script => (true, f.kind != FuncKind::Normal, f.kind == FuncKind::DerivedCtor),
            _ => (false, false, false),
        };
        EvalScope { fns, strict: self.fr().strict, new_target, super_prop, super_call }
    }

    /// Rebuilds an enclosing function's compile-time state from a snapshot (name resolution only).
    fn push_stub(&mut self, info: &FnInfo) {
        self.new_fn(info.kind, info.strict, HashSet::new(), info.is_script);
        let f = self.f();
        f.capture_all = true;
        f.is_eval = info.is_eval;
        for s in &info.scopes {
            let bindings = s.bindings.iter().map(|(n, slot, k)| Binding { name: n.clone(), loc: Loc::Env(*slot), kind: code_kind(*k) }).collect();
            f.scopes.push(Scope { bindings, env: s.env, env_slots: 0, obj_slot: s.obj_slot, is_with: s.is_with });
        }
        self.dyn_count += info.scopes.iter().filter(|s| s.obj_slot.is_some()).count() as u32;
    }

    /// Declares a `var` (or function) of sloppy eval code in the caller's variable scope:
    /// an existing binding is reused; otherwise the name goes into the enclosing function's
    /// variable object, or onto the global object for script-level code.
    fn declare_eval_var(&mut self, name: &str) {
        let mut hops = 0;
        let mut target = None;
        'outer: for f in self.fns.iter().rev() {
            for s in f.scopes.iter().rev() {
                if s.bindings.iter().any(|b| b.name == name) {
                    return;
                }
                if let (Some(slot), false, false) = (s.obj_slot, s.is_with, f.is_eval) {
                    target = Some((hops, slot));
                    break 'outer;
                }
                if s.env {
                    hops += 1;
                }
            }
            if !f.is_eval {
                if !f.is_script {
                    return; // a function without a variable object: the name stays unresolved
                }
                break;
            }
        }
        let n = self.name(name);
        match target {
            Some((h, slot)) => self.emit(Op::DeclareEvalVar(n, h, slot)),
            None => self.emit(Op::DeclareVar(n)),
        };
    }

    fn compile_eval_code(&mut self, prog: &Program) -> CResult<Arc<FuncCode>> {
        let refs = analyze(&[], None, &prog.body);
        let strict = prog.strict;
        self.new_fn(FuncKind::Arrow, strict, refs.inner, false);
        self.f().capture_all = refs.has_eval;
        self.f().is_eval = !strict;
        let mut vars = Vec::new();
        var_names(&prog.body, &mut vars);
        let lex = Self::lexical_names(&prog.body);
        // strict eval code keeps its `var`s and functions to itself; sloppy code only its
        // lexical declarations
        let mut all: Vec<(String, BKind)> = Vec::new();
        if strict {
            all.extend(vars.iter().map(|v| (v.clone(), BKind::Var)));
            all.extend(lex.iter().cloned());
        } else {
            all.extend(lex.iter().filter(|(_, k)| *k != BKind::Var).cloned());
        }
        let env = (refs.has_eval && !all.is_empty()) || all.iter().any(|(n, _)| self.fr().captured.contains(n));
        self.f().scopes.push(Scope { env, ..Default::default() });
        for (n, k) in &all {
            self.declare(n, *k);
        }
        if env {
            let slots = self.fr().scopes[0].env_slots;
            self.emit(Op::PushEnv(slots));
        }
        if strict {
            for v in &vars {
                self.emit(Op::Undef);
                self.init_var(v, VarKind::Let);
            }
        } else {
            for v in &vars {
                self.declare_eval_var(v);
            }
            for (n, k) in &lex {
                if *k == BKind::Var {
                    self.declare_eval_var(n);
                }
            }
        }
        let completion = self.alloc_local();
        self.emit(Op::Undef);
        self.emit(Op::InitLocal(completion));
        self.f().completion = Some(completion);
        for s in &prog.body {
            if let StmtKind::Func(f) = &s.kind {
                let idx = self.compile_function(f, None)?;
                self.emit(Op::Closure(idx));
                let name = f.name.clone().unwrap_or_default();
                if strict {
                    self.init_var(&name, VarKind::Let);
                } else {
                    self.put_var(&name);
                }
            }
        }
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
}
