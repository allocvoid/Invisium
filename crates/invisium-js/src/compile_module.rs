//! Module code: every top-level binding lives in one module environment created by the
//! runtime (so other modules can link to it before the body runs). Two functions are compiled
//! against that environment, like direct-eval code against its call site: `init` creates the
//! hoisted functions and `undefined` vars (run at link time), `body` runs the statements (an
//! async function when the module awaits at top level).

use crate::analyze::{analyze, var_names};
use crate::ast::*;
use crate::bytecode::*;
use crate::compiler::*;
use crate::parser::ParseError;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub struct ModuleCode {
    pub names: Vec<Str16>,
    pub init: Arc<FuncCode>,
    pub body: Arc<FuncCode>,
    pub nslots: u32,
    /// Env slot of each top-level binding.
    pub slots: HashMap<String, u32>,
    /// Env slot of `import.meta`.
    pub meta_slot: u32,
    pub info: ModuleInfo,
}

fn err(msg: String, pos: Pos) -> ParseError {
    ParseError { message: msg, line: pos.line, column: pos.col }
}

pub fn compile_module(prog: &Program, info: ModuleInfo, source: Arc<[char]>, url: &str) -> CResult<ModuleCode> {
    // the module scope: import.meta, imports, vars and functions, lexical declarations
    let mut bindings: Vec<Binding> = Vec::new();
    let add = |bindings: &mut Vec<Binding>, name: &str, kind: BKind| {
        let slot = bindings.len() as u32;
        bindings.push(Binding { name: name.to_string(), loc: Loc::Env(slot), kind });
    };
    add(&mut bindings, "*meta*", BKind::Const);
    for im in &info.imports {
        if bindings.iter().any(|b| b.name == im.local) {
            return Err(err(format!("Identifier '{}' has already been declared", im.local), im.pos));
        }
        add(&mut bindings, &im.local, if im.import_name.is_some() { BKind::Import } else { BKind::Const });
    }
    let n_imports = bindings.len();
    let mut vars = Vec::new();
    var_names(&prog.body, &mut vars);
    let lex = Compiler::lexical_names(&prog.body);
    for (name, kind) in vars.iter().map(|v| (v.clone(), BKind::Var)).chain(lex.iter().cloned()) {
        if let Some(i) = bindings.iter().position(|b| b.name == name) {
            let clash = i < n_imports || kind != BKind::Var || bindings[i].kind != BKind::Var;
            if clash {
                let pos = prog.body.first().map(|s| s.pos).unwrap_or_default();
                return Err(err(format!("Identifier '{}' has already been declared", name), pos));
            }
            continue;
        }
        add(&mut bindings, &name, kind);
    }
    let slots: HashMap<String, u32> = bindings.iter().filter_map(|b| match b.loc {
        Loc::Env(s) => Some((b.name.clone(), s)),
        Loc::Local(_) => None,
    }).collect();
    // exports must name declared bindings, each export name once
    let mut seen: HashSet<&str> = HashSet::new();
    for e in &info.exports {
        let (name, pos) = match e {
            ExportEntry::Local { name, local, pos } => {
                if !slots.contains_key(local) || local.starts_with('*') {
                    return Err(err(format!("Export '{}' is not defined in module", local), *pos));
                }
                (name.as_str(), *pos)
            }
            ExportEntry::Indirect { name, pos, .. } => (name.as_str(), *pos),
            ExportEntry::Star { .. } => continue,
        };
        if !seen.insert(name) {
            return Err(err(format!("Duplicate export of '{}'", name), pos));
        }
    }
    let nslots = bindings.len() as u32;
    let mut c = Compiler { fns: Vec::new(), names: Vec::new(), name_map: HashMap::new(), source, script: url.into(), depth: 0, dyn_count: 0, dynamic: false };
    c.new_fn(FuncKind::Normal, true, HashSet::new(), false);
    c.f().capture_all = true;
    c.f().scopes.push(Scope { bindings, env: true, env_slots: nslots, ..Default::default() });
    let len = c.source.len();

    // init: `var`s start as undefined, function declarations are created
    c.new_fn(FuncKind::Arrow, true, HashSet::new(), false);
    c.f().scopes.push(Scope::default());
    for v in &vars {
        c.emit(Op::Undef);
        c.init_var(v, VarKind::Let);
    }
    c.hoist_functions(&prog.body, false)?;
    c.emit(Op::Undef);
    c.emit(Op::Return);
    let init = Arc::new(c.finish_fn(Vec::new(), 0, (0, len)));

    // body: everything else, in order
    let refs = analyze(&[], None, &prog.body);
    c.new_fn(FuncKind::Arrow, true, refs.inner, false);
    c.f().capture_all = refs.has_eval;
    c.f().is_async = info.has_tla;
    c.f().scopes.push(Scope::default());
    for s in &prog.body {
        if !matches!(s.kind, StmtKind::Func(_)) {
            c.stmt(s)?;
        }
    }
    c.emit(Op::Undef);
    c.emit(Op::Return);
    let body = Arc::new(c.finish_fn(Vec::new(), 0, (0, len)));
    Ok(ModuleCode { names: c.names, init, body, nslots, slots, meta_slot: 0, info })
}
