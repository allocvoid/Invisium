//! Expressions to WGSL. Side effects are lowered into statements emitted before the
//! expression that needs them, keeping GLSL's left-to-right order and conditional evaluation.

use crate::gen::Gen;
use crate::ir::*;
use crate::layout::{mangle, stype, wrapped, wty};

pub const XYZW: &str = "xyzw";

pub fn lit(c: CVal) -> String {
    match c {
        CVal::B(b) => b.to_string(),
        CVal::I(v) if v < 0 => format!("i32({v})"),
        CVal::I(v) => format!("{v}i"),
        CVal::U(v) => format!("{v}u"),
        CVal::F(f) if f.is_nan() => "(0.0f / 0.0f)".into(),
        CVal::F(f) if f.is_infinite() => if f > 0.0 { "3.4028235e38f" } else { "-3.4028235e38f" }.into(),
        CVal::F(f) => format!("{f:?}f"),
    }
}

pub fn swz(c: &[u8]) -> String {
    c.iter().map(|i| XYZW.as_bytes()[*i as usize] as char).collect()
}

fn simple(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}

/// An assignable place: a WGSL reference expression plus an optional swizzle.
pub struct Lv {
    pub base: String,
    pub swz: Vec<u8>,
}

fn is_place(e: &Expr) -> bool {
    match &e.k {
        EK::Var(_) => true,
        EK::Index(a, _) | EK::Field(a, _) => is_place(a),
        _ => false,
    }
}

impl Gen<'_> {
    /// Bind `s` to a temporary and return its name.
    pub fn hoist(&mut self, s: &str, ty: &Ty) -> String {
        if simple(s) {
            return s.to_string();
        }
        let t = self.fresh();
        let kw = if matches!(ty, Ty::Array(..) | Ty::M(..) | Ty::Struct(_)) { "var" } else { "let" };
        self.line(&format!("{kw} {t} = {s};"));
        t
    }

    /// Evaluate operands in order; any operand followed by one with side effects is saved first.
    pub fn operands(&mut self, es: &[&Expr]) -> Vec<String> {
        let last = es.iter().rposition(|e| e.fx);
        let mut v = Vec::with_capacity(es.len());
        for (i, e) in es.iter().enumerate() {
            let mut s = self.expr(e);
            if last.is_some_and(|l| i < l) && !matches!(e.k, EK::Lit(_)) {
                let t = self.fresh();
                let kw = if matches!(e.ty, Ty::Array(..) | Ty::M(..) | Ty::Struct(_)) { "var" } else { "let" };
                self.line(&format!("{kw} {t} = {s};"));
                s = t;
            }
            v.push(s);
        }
        v
    }

    pub fn stmt_expr(&mut self, e: &Expr) {
        match &e.k {
            EK::Comma(a, b) => {
                self.stmt_expr(a);
                self.stmt_expr(b);
            }
            EK::Cond(c, a, b) if e.fx => {
                let sc = self.expr(c);
                self.line(&format!("if ({sc}) {{"));
                self.ind += 1;
                self.stmt_expr(a);
                self.ind -= 1;
                self.line("} else {");
                self.ind += 1;
                self.stmt_expr(b);
                self.ind -= 1;
                self.line("}");
            }
            EK::LogAnd(a, b) | EK::LogOr(a, b) if b.fx => {
                let sa = self.expr(a);
                let not = if matches!(e.k, EK::LogOr(..)) { "!" } else { "" };
                self.line(&format!("if ({not}({sa})) {{"));
                self.ind += 1;
                self.stmt_expr(b);
                self.ind -= 1;
                self.line("}");
            }
            _ if e.fx => {
                let _ = self.expr(e);
            }
            _ => {}
        }
    }

    pub fn expr(&mut self, e: &Expr) -> String {
        match &e.k {
            EK::Lit(c) => lit(*c),
            EK::Var(v) => {
                let var = &self.m.vars[*v as usize];
                if let (Storage::Const, Some(c)) = (var.storage, var.cval) {
                    return lit(c);
                }
                if var.storage == Storage::Uniform {
                    return self.uconv(&format!("U.u_{}", var.name), &e.ty);
                }
                self.vname(*v)
            }
            EK::Un(op, a) => {
                let s = self.expr(a);
                match op {
                    UOp::Neg if a.ty.is_mat() => format!("({s} * -1.0f)"),
                    UOp::Neg => format!("(-{s})"),
                    UOp::Not => format!("(!{s})"),
                    UOp::BitNot => format!("(~{s})"),
                }
            }
            EK::Bin(op, a, b) => {
                let v = self.operands(&[a, b]);
                self.binop(*op, &a.ty, &b.ty, &v[0], &v[1])
            }
            EK::LogAnd(a, b) | EK::LogOr(a, b) => {
                let and = matches!(e.k, EK::LogAnd(..));
                let sa = self.expr(a);
                if !b.fx {
                    let sb = self.expr(b);
                    return format!("({sa} {} {sb})", if and { "&&" } else { "||" });
                }
                let t = self.fresh();
                self.line(&format!("var {t}: bool = {sa};"));
                self.line(&format!("if ({}{t}) {{", if and { "" } else { "!" }));
                self.ind += 1;
                let sb = self.expr(b);
                self.line(&format!("{t} = {sb};"));
                self.ind -= 1;
                self.line("}");
                t
            }
            EK::Assign(op, l, r) => {
                let lv = self.lvalue(l);
                let sr = self.expr(r);
                let val = match op {
                    None => sr,
                    Some(o) => {
                        let cur = self.lv_read(&lv);
                        self.binop(*o, &l.ty, &r.ty, &cur, &sr)
                    }
                };
                self.store(&lv, &val);
                self.lv_read(&lv)
            }
            EK::Inc { pre, dec, e: x } => {
                let lv = self.lvalue(x);
                let k = x.ty.kind().unwrap_or(K::Float);
                let one = lit(match k {
                    K::Int => CVal::I(1),
                    K::Uint => CVal::U(1),
                    _ => CVal::F(1.0),
                });
                let op = if *dec { BOp::Sub } else { BOp::Add };
                let cur = self.lv_read(&lv);
                if *pre {
                    let nv = self.binop(op, &x.ty, &Ty::S(k), &cur, &one);
                    self.store(&lv, &nv);
                    self.lv_read(&lv)
                } else {
                    let t = self.fresh();
                    let kw = if x.ty.is_mat() { "var" } else { "let" };
                    self.line(&format!("{kw} {t} = {cur};"));
                    let nv = self.binop(op, &x.ty, &Ty::S(k), &t, &one);
                    self.store(&lv, &nv);
                    t
                }
            }
            EK::Cond(c, a, b) => {
                let sc = self.expr(c);
                if !a.fx && !b.fx && e.ty.is_sv() {
                    let sa = self.expr(a);
                    let sb = self.expr(b);
                    return format!("select({sb}, {sa}, {sc})");
                }
                let t = self.fresh();
                self.line(&format!("var {t}: {};", wty(self.m, &e.ty)));
                self.line(&format!("if ({sc}) {{"));
                self.ind += 1;
                let sa = self.expr(a);
                self.line(&format!("{t} = {sa};"));
                self.ind -= 1;
                self.line("} else {");
                self.ind += 1;
                let sb = self.expr(b);
                self.line(&format!("{t} = {sb};"));
                self.ind -= 1;
                self.line("}");
                t
            }
            EK::Comma(a, b) => {
                self.stmt_expr(a);
                self.expr(b)
            }
            EK::Ctor(args) => self.ctor(&e.ty, args),
            EK::Swz(a, c) => {
                let s = self.expr(a);
                format!("{s}.{}", swz(c))
            }
            EK::Index(a, i) => {
                if let Some(p) = self.upath(e) {
                    return self.uconv(&p, &e.ty);
                }
                let v = self.operands(&[a, i]);
                let base = if matches!(a.ty, Ty::Array(..) | Ty::M(..)) && !is_place(a) {
                    let t = self.fresh();
                    self.line(&format!("var {t} = {};", v[0]));
                    t
                } else {
                    v[0].clone()
                };
                format!("{base}[{}]", v[1])
            }
            EK::Field(a, k) => {
                if let Some(p) = self.upath(e) {
                    return self.uconv(&p, &e.ty);
                }
                let s = self.expr(a);
                format!("{s}.m_{}", self.field_name(&a.ty, *k))
            }
            EK::Call(f, args) => self.call(*f, args, &e.ty),
            EK::Bi(n, args) => self.builtin(n, args, &e.ty),
        }
    }

    pub fn field_name(&self, t: &Ty, k: u32) -> String {
        match t {
            Ty::Struct(s) => self.m.structs[*s as usize].fields[k as usize].0.clone(),
            _ => "x".into(),
        }
    }

    /// The stored-form path of a uniform (or part of one) that `e` names.
    fn upath(&mut self, e: &Expr) -> Option<String> {
        match &e.k {
            EK::Var(v) => {
                let var = &self.m.vars[*v as usize];
                (var.storage == Storage::Uniform).then(|| format!("U.u_{}", var.name))
            }
            EK::Field(a, k) => {
                let p = self.upath(a)?;
                Some(format!("{p}.m_{}", self.field_name(&a.ty, *k)))
            }
            EK::Index(a, i) => {
                let Ty::Array(el, _) = &a.ty else { return None };
                let w = if wrapped(el) { ".v" } else { "" };
                let p = self.upath(a)?;
                let si = self.expr(i);
                Some(format!("{p}[{si}]{w}"))
            }
            _ => None,
        }
    }

    /// Read a uniform's stored form as a value of type `t`.
    pub fn uconv(&mut self, p: &str, t: &Ty) -> String {
        match t {
            Ty::S(K::Bool) => format!("({p} != 0u)"),
            Ty::V(K::Bool, n) => format!("({p} != vec{n}<u32>(0u))"),
            // Whole structures and arrays convert through one helper per type, so the
            // generated code stays linear in the size of the source.
            Ty::Struct(s) => {
                let name = format!("h_uc_s{s}");
                if !self.helpers.contains_key(&name) {
                    let fields = self.m.structs[*s as usize].fields.clone();
                    let parts: Vec<String> = fields.iter().map(|(f, ft)| self.uconv(&format!("x.m_{f}"), ft)).collect();
                    let w = wty(self.m, t);
                    let code = format!("fn {name}(x: {}) -> {w} {{ return {w}({}); }}\n", stype(self.m, t), parts.join(", "));
                    self.helpers.insert(name.clone(), code);
                }
                format!("{name}({p})")
            }
            Ty::Array(e, n) => {
                let name = format!("h_uc_{}", mangle(t));
                if !self.helpers.contains_key(&name) {
                    let w = if wrapped(e) { ".v" } else { "" };
                    let el = self.uconv(&format!("y[i]{w}"), e);
                    let wt = wty(self.m, t);
                    let code = format!(
                        "fn {name}(x: {}) -> {wt} {{ var y = x; var r: {wt}; for (var i = 0u; i < {n}u; i++) {{ r[i] = {el}; }} return r; }}\n",
                        stype(self.m, t)
                    );
                    self.helpers.insert(name.clone(), code);
                }
                format!("{name}({p})")
            }
            _ => p.to_string(),
        }
    }

    pub fn lvalue(&mut self, e: &Expr) -> Lv {
        match &e.k {
            EK::Var(v) => Lv { base: self.vname(*v), swz: vec![] },
            EK::Index(a, i) => {
                let inner = self.lvalue(a);
                if !inner.swz.is_empty() {
                    let k = crate::check::fold(self.m, i).and_then(|c| c.as_i64()).unwrap_or(0) as usize;
                    return Lv { base: inner.base, swz: vec![inner.swz[k.min(inner.swz.len() - 1)]] };
                }
                let si = self.expr(i);
                let si = if matches!(i.k, EK::Lit(_)) { si } else { self.hoist(&si, &i.ty) };
                Lv { base: format!("{}[{si}]", inner.base), swz: vec![] }
            }
            EK::Field(a, k) => {
                let inner = self.lvalue(a);
                Lv { base: format!("{}.m_{}", inner.base, self.field_name(&a.ty, *k)), swz: vec![] }
            }
            EK::Swz(a, c) => {
                let inner = self.lvalue(a);
                let swz = if inner.swz.is_empty() { c.clone() } else { c.iter().map(|x| inner.swz[*x as usize]).collect() };
                Lv { base: inner.base, swz }
            }
            _ => {
                let s = self.expr(e);
                let t = self.fresh();
                self.line(&format!("var {t} = {s};"));
                Lv { base: t, swz: vec![] }
            }
        }
    }

    pub fn lv_read(&self, lv: &Lv) -> String {
        if lv.swz.is_empty() {
            lv.base.clone()
        } else {
            format!("{}.{}", lv.base, swz(&lv.swz))
        }
    }

    pub fn store(&mut self, lv: &Lv, val: &str) {
        match lv.swz.len() {
            0 => self.line(&format!("{} = {val};", lv.base)),
            1 => self.line(&format!("{}.{} = {val};", lv.base, swz(&lv.swz))),
            _ => {
                let t = if simple(val) {
                    val.to_string()
                } else {
                    let t = self.fresh();
                    self.line(&format!("let {t} = {val};"));
                    t
                };
                for (i, c) in lv.swz.iter().enumerate() {
                    self.line(&format!("{}.{} = {t}.{};", lv.base, swz(&[*c]), swz(&[i as u8])));
                }
            }
        }
    }

    fn call(&mut self, f: FuncId, args: &[Expr], ret: &Ty) -> String {
        let m = self.m;
        let fd = &m.funcs[f as usize];
        let last = args.iter().rposition(|a| a.fx);
        let mut parts = Vec::new();
        let mut back = Vec::new();
        for (k, (p, a)) in fd.params.iter().zip(args).enumerate() {
            let pv = &m.vars[*p as usize];
            match pv.storage {
                _ if matches!(pv.ty, Ty::Sampler(_)) => {
                    let (t, s) = self.tex_pair(a);
                    parts.push(t);
                    parts.push(s);
                }
                Storage::Param(PQ::Out | PQ::InOut) => {
                    let lv = self.lvalue(a);
                    let t = self.fresh();
                    let ty = wty(m, &pv.ty);
                    if pv.storage == Storage::Param(PQ::InOut) {
                        let cur = self.lv_read(&lv);
                        self.line(&format!("var {t}: {ty} = {cur};"));
                    } else {
                        self.line(&format!("var {t}: {ty};"));
                    }
                    parts.push(format!("&{t}"));
                    back.push((lv, t));
                }
                _ => {
                    let s = self.expr(a);
                    let s = if last.is_some_and(|l| k < l) && !matches!(a.k, EK::Lit(_)) {
                        let t = self.fresh();
                        self.line(&format!("let {t} = {s};"));
                        t
                    } else {
                        s
                    };
                    parts.push(s);
                }
            }
        }
        let call = format!("f{f}_{}({})", fd.name, parts.join(", "));
        let r = if *ret == Ty::Void {
            self.line(&format!("{call};"));
            String::new()
        } else {
            let t = self.fresh();
            let kw = if matches!(ret, Ty::Array(..) | Ty::M(..) | Ty::Struct(_)) { "var" } else { "let" };
            self.line(&format!("{kw} {t} = {call};"));
            t
        };
        for (lv, t) in back {
            self.store(&lv, &t);
        }
        r
    }

    /// Texture and sampler variables of a sampler expression.
    pub fn tex_pair(&mut self, e: &Expr) -> (String, String) {
        let (v, k) = match &e.k {
            EK::Var(v) => (*v, 0),
            EK::Index(a, i) => match &a.k {
                EK::Var(v) => (*v, crate::check::fold(self.m, i).and_then(|c| c.as_i64()).unwrap_or(0) as u32),
                _ => (0, 0),
            },
            _ => (0, 0),
        };
        if let Storage::Param(_) = self.m.vars[v as usize].storage {
            return (format!("a{v}t"), format!("a{v}s"));
        }
        let slot = self.c.samplers.get(&v).copied().unwrap_or(0) + k;
        (format!("tx{slot}"), format!("sx{slot}"))
    }

    /// Register a helper function once.
    pub fn helper(&mut self, name: &str, code: impl FnOnce() -> String) -> String {
        if !self.helpers.contains_key(name) {
            let c = code();
            self.helpers.insert(name.to_string(), c);
        }
        name.to_string()
    }
}
