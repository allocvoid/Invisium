//! WGSL generator: module assembly, statements and the entry points. Expressions are in
//! `gexpr`, built-in functions in `gbuiltin`.

use crate::ir::*;
use crate::layout::{storage_decls, stype, wty};
use crate::Stage;
use std::collections::{BTreeMap, HashMap};

/// Largest WGSL module emitted.
pub const MAX_WGSL: usize = 8 << 20;

/// What linking decided for one module.
pub struct Ctx<'a> {
    pub m: &'a Module,
    pub stage: Stage,
    /// Uniforms of the default block this module uses, with their offsets.
    pub uniforms: Vec<(VarId, u32)>,
    pub ubytes: u32,
    /// Sampler uniform -> first slot.
    pub samplers: HashMap<VarId, u32>,
    /// Vertex inputs and their first locations.
    pub attrs: Vec<(VarId, u32)>,
    /// Varyings exchanged with the other stage and their first locations.
    pub varyings: Vec<(VarId, u32)>,
    /// Fragment outputs: source expression, location, WGSL type.
    pub outputs: Vec<(String, u32, String)>,
    pub writes_depth: bool,
    pub reachable: Vec<bool>,
}

pub struct Gen<'a> {
    pub c: &'a Ctx<'a>,
    pub m: &'a Module,
    pub out: String,
    pub ind: usize,
    pub tmp: u32,
    pub helpers: BTreeMap<String, String>,
    pub overflow: bool,
}

pub fn bv_name(b: BV) -> String {
    format!("bi_{b:?}")
}

/// Number of locations a varying takes (`flatten(t).len()` without building the list).
pub fn flat_count(t: &Ty) -> u32 {
    match t {
        Ty::M(c, _) => *c as u32,
        Ty::Array(e, n) => flat_count(e).saturating_mul(*n),
        _ => 1,
    }
}

/// Scalar, vector or matrix leaves of a varying, as (access suffix, type).
pub fn flatten(t: &Ty) -> Vec<(String, Ty)> {
    match t {
        Ty::M(c, r) => (0..*c).map(|j| (format!("[{j}]"), Ty::V(K::Float, *r))).collect(),
        Ty::Array(e, n) => {
            let inner = flatten(e);
            (0..*n).flat_map(|i| inner.iter().map(move |(s, t)| (format!("[{i}]{s}"), t.clone()))).collect()
        }
        t => vec![(String::new(), t.clone())],
    }
}

pub fn generate(c: &Ctx) -> Result<String, String> {
    let mut g = Gen { c, m: c.m, out: String::new(), ind: 0, tmp: 0, helpers: BTreeMap::new(), overflow: false };
    let mut body = String::new();
    for (f, fd) in c.m.funcs.iter().enumerate() {
        if c.reachable[f] && fd.body.is_some() {
            g.func(f as FuncId);
            body.push_str(&std::mem::take(&mut g.out));
        }
    }
    g.line("fn init_globals() {");
    g.ind += 1;
    for (v, e) in &c.m.inits {
        let s = g.expr(e);
        let n = g.vname(*v);
        g.line(&format!("{n} = {s};"));
    }
    g.ind -= 1;
    g.line("}");
    g.entry();
    body.push_str(&std::mem::take(&mut g.out));
    if g.overflow {
        return Err("ERROR: generated WGSL is too large\n".into());
    }

    let m = c.m;
    let mut s = String::new();
    if c.stage == Stage::Fragment {
        s.push_str("diagnostic(off, derivative_uniformity);\n");
    }
    for (k, sd) in m.structs.iter().enumerate() {
        s.push_str(&format!("struct {} {{\n", wty(m, &Ty::Struct(k as u32))));
        for (f, t) in &sd.fields {
            s.push_str(&format!("  m_{f}: {},\n", wty(m, t)));
        }
        s.push_str("}\n");
    }
    if !c.uniforms.is_empty() {
        let mut done = Vec::new();
        let mut decls = String::new();
        for (v, _) in &c.uniforms {
            storage_decls(m, &m.vars[*v as usize].ty, &mut done, &mut decls);
        }
        s.push_str(&decls);
        s.push_str("struct UBlock {\n");
        let first = c.uniforms[0].1;
        if first > 0 {
            s.push_str(&format!("  @size({first}) pad: u32,\n"));
        }
        for (k, (v, off)) in c.uniforms.iter().enumerate() {
            let var = &m.vars[*v as usize];
            let end = c.uniforms.get(k + 1).map_or(c.ubytes, |u| u.1);
            let align = if matches!(var.ty, Ty::Struct(_) | Ty::Array(..)) { "@align(16) " } else { "" };
            s.push_str(&format!("  {align}@size({}) u_{}: {},\n", end - off, var.name, stype(m, &var.ty)));
        }
        s.push_str("}\n@group(0) @binding(0) var<uniform> U: UBlock;\n");
    }
    let mut slots: Vec<(u32, Samp)> = Vec::new();
    for (v, slot) in &c.samplers {
        match &m.vars[*v as usize].ty {
            Ty::Sampler(k) => slots.push((*slot, *k)),
            Ty::Array(e, n) => {
                if let Ty::Sampler(k) = **e {
                    slots.extend((0..*n).map(|i| (slot + i, k)));
                }
            }
            _ => {}
        }
    }
    slots.sort_by_key(|x| x.0);
    for (slot, k) in slots {
        let (tt, st) = k.wgsl();
        s.push_str(&format!("@group(1) @binding({}) var tx{slot}: {tt};\n", slot * 2));
        s.push_str(&format!("@group(1) @binding({}) var sx{slot}: {st};\n", slot * 2 + 1));
    }
    for v in &m.globals {
        let var = &m.vars[*v as usize];
        if matches!(var.storage, Storage::Uniform) {
            continue;
        }
        s.push_str(&format!("var<private> {}: {};\n", g.vname(*v), wty(m, &var.ty)));
    }
    for h in g.helpers.values() {
        s.push_str(h);
    }
    s.push_str(&body);
    if s.len() > MAX_WGSL {
        return Err("ERROR: generated WGSL is too large\n".into());
    }
    Ok(s)
}

impl Gen<'_> {
    pub fn line(&mut self, s: &str) {
        if self.out.len() > MAX_WGSL {
            self.overflow = true;
            return;
        }
        for _ in 0..self.ind {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    pub fn fresh(&mut self) -> String {
        self.tmp += 1;
        format!("t{}", self.tmp)
    }

    pub fn vname(&self, v: VarId) -> String {
        let var = &self.m.vars[v as usize];
        match var.storage {
            Storage::Builtin(b) => bv_name(b),
            Storage::Param(PQ::Out | PQ::InOut) => format!("(*a{v})"),
            Storage::Uniform => format!("U.u_{}", var.name),
            _ if var.global => format!("g{v}_{}", var.name),
            _ => format!("l{v}_{}", var.name),
        }
    }

    fn func(&mut self, f: FuncId) {
        let m = self.m;
        let fd = &m.funcs[f as usize];
        let mut ps = Vec::new();
        for p in &fd.params {
            let pv = &m.vars[*p as usize];
            match (&pv.ty, pv.storage) {
                (Ty::Sampler(k), _) => {
                    let (tt, st) = k.wgsl();
                    ps.push(format!("a{p}t: {tt}"));
                    ps.push(format!("a{p}s: {st}"));
                }
                (t, Storage::Param(PQ::Out | PQ::InOut)) => ps.push(format!("a{p}: ptr<function, {}>", wty(m, t))),
                (t, _) => ps.push(format!("a{p}: {}", wty(m, t))),
            }
        }
        let ret = if fd.ret == Ty::Void { String::new() } else { format!(" -> {}", wty(m, &fd.ret)) };
        self.line(&format!("fn f{f}_{}({}){ret} {{", fd.name, ps.join(", ")));
        self.ind += 1;
        for p in &fd.params {
            let pv = &m.vars[*p as usize];
            if pv.storage == Storage::Param(PQ::In) && !matches!(pv.ty, Ty::Sampler(_)) {
                let n = self.vname(*p);
                self.line(&format!("var {n} = a{p};"));
            }
        }
        let body = fd.body.as_deref().unwrap_or(&[]);
        self.tmp = 0;
        for s in body {
            self.stmt(s);
        }
        if fd.ret != Ty::Void && !matches!(body.last(), Some(Stmt::Return(_))) {
            self.line(&format!("return {}();", wty(m, &fd.ret)));
        }
        self.ind -= 1;
        self.line("}");
    }

    /// An `if / else if / else` chain, emitted without nesting: as WGSL `else if` when the
    /// chain is short and its later conditions need no statements of their own, otherwise
    /// as a run of `if (!taken)` blocks at one level (naga limits brace nesting).
    fn if_chain(&mut self, chain: &[(Expr, Stmt)], els: Option<&Stmt>) {
        let Some(((c0, _), rest)) = chain.split_first() else { return };
        let s0 = self.expr(c0);
        let mut conds = vec![(String::new(), s0)];
        for (c, _) in rest {
            let saved = std::mem::take(&mut self.out);
            let s = self.expr(c);
            let pre = std::mem::replace(&mut self.out, saved);
            conds.push((pre, s));
        }
        if chain.len() <= 16 && conds.iter().all(|c| c.0.is_empty()) {
            for (k, ((_, body), (_, s))) in chain.iter().zip(&conds).enumerate() {
                self.line(&format!("{}if ({s}) {{", if k == 0 { "" } else { "} else " }));
                self.ind += 1;
                self.body(body);
                self.ind -= 1;
            }
            if let Some(b) = els {
                self.line("} else {");
                self.ind += 1;
                self.body(b);
                self.ind -= 1;
            }
            self.line("}");
            return;
        }
        let t = self.fresh();
        self.line(&format!("var {t} = false;"));
        for (k, ((_, body), (pre, s))) in chain.iter().zip(&conds).enumerate() {
            if k > 0 {
                self.line(&format!("if (!{t}) {{"));
                self.out.push_str(pre);
            }
            self.line(&format!("if ({s}) {{"));
            self.ind += 1;
            self.line(&format!("{t} = true;"));
            self.body(body);
            self.ind -= 1;
            self.line("}");
            if k > 0 {
                self.line("}");
            }
        }
        if let Some(b) = els {
            self.line(&format!("if (!{t}) {{"));
            self.ind += 1;
            self.body(b);
            self.ind -= 1;
            self.line("}");
        }
    }

    fn body(&mut self, s: &Stmt) {
        match s {
            Stmt::Block(v) => v.iter().for_each(|s| self.stmt(s)),
            s => self.stmt(s),
        }
    }

    pub fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Decl(v, init) => {
                let n = self.vname(*v);
                let t = wty(self.m, &self.m.vars[*v as usize].ty);
                match init {
                    Some(e) => {
                        let s = self.expr(e);
                        self.line(&format!("var {n}: {t} = {s};"));
                    }
                    None => self.line(&format!("var {n}: {t};")),
                }
            }
            Stmt::Expr(e) => self.stmt_expr(e),
            Stmt::Block(v) => {
                if !v.is_empty() {
                    self.line("{");
                    self.ind += 1;
                    v.iter().for_each(|s| self.stmt(s));
                    self.ind -= 1;
                    self.line("}");
                }
            }
            Stmt::If(chain, els) => self.if_chain(chain, els.as_deref()),
            Stmt::For { init, cond, step, body } => {
                self.line("{");
                self.ind += 1;
                init.iter().for_each(|s| self.stmt(s));
                self.line("loop {");
                self.ind += 1;
                if let Some(c) = cond {
                    let sc = self.expr(c);
                    self.line(&format!("if (!({sc})) {{ break; }}"));
                }
                self.body(body);
                if let Some(st) = step {
                    self.line("continuing {");
                    self.ind += 1;
                    self.stmt_expr(st);
                    self.ind -= 1;
                    self.line("}");
                }
                self.ind -= 1;
                self.line("}");
                self.ind -= 1;
                self.line("}");
            }
            Stmt::While(c, body) => {
                self.line("loop {");
                self.ind += 1;
                let sc = self.expr(c);
                self.line(&format!("if (!({sc})) {{ break; }}"));
                self.body(body);
                self.ind -= 1;
                self.line("}");
            }
            Stmt::DoWhile(body, c) => {
                self.line("loop {");
                self.ind += 1;
                self.body(body);
                self.line("continuing {");
                self.ind += 1;
                let sc = self.expr(c);
                self.line(&format!("break if !({sc});"));
                self.ind -= 1;
                self.line("}");
                self.ind -= 1;
                self.line("}");
            }
            Stmt::Switch(sel, cases) => {
                let s = self.expr(sel);
                self.line(&format!("switch ({s}) {{"));
                self.ind += 1;
                let mut has_default = false;
                for c in cases {
                    let labels: Vec<String> = c
                        .labels
                        .iter()
                        .map(|l| match l {
                            Some(v) => crate::gexpr::lit(*v),
                            None => {
                                has_default = true;
                                "default".into()
                            }
                        })
                        .collect();
                    self.line(&format!("case {}: {{", labels.join(", ")));
                    self.ind += 1;
                    c.body.iter().for_each(|s| self.stmt(s));
                    self.ind -= 1;
                    self.line("}");
                }
                if !has_default {
                    self.line("default: {}");
                }
                self.ind -= 1;
                self.line("}");
            }
            Stmt::Break => self.line("break;"),
            Stmt::Continue => self.line("continue;"),
            Stmt::Discard => self.line("discard;"),
            Stmt::Return(None) => self.line("return;"),
            Stmt::Return(Some(e)) => {
                let s = self.expr(e);
                self.line(&format!("return {s};"));
            }
        }
    }
}
