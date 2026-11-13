//! Operators and constructors with GLSL semantics in WGSL.

use crate::gen::Gen;
use crate::gexpr::XYZW;
use crate::ir::*;
use crate::layout::{mangle, wty};

fn op_str(op: BOp) -> &'static str {
    match op {
        BOp::Add => "+",
        BOp::Sub => "-",
        BOp::Mul => "*",
        BOp::Div => "/",
        BOp::Rem => "%",
        BOp::Shl => "<<",
        BOp::Shr => ">>",
        BOp::And => "&",
        BOp::Or => "|",
        BOp::Xor => "^",
        BOp::Lt => "<",
        BOp::Gt => ">",
        BOp::Le => "<=",
        BOp::Ge => ">=",
        BOp::Eq => "==",
        BOp::Ne | BOp::LogXor => "!=",
    }
}

/// Convert scalar or vector `s` of type `from` to component kind `to`.
pub fn conv(s: &str, from: &Ty, to: K) -> String {
    match from {
        Ty::S(k) if *k == to => s.to_string(),
        Ty::S(_) => format!("{}({s})", to.wgsl()),
        Ty::V(k, _) if *k == to => s.to_string(),
        Ty::V(_, n) => format!("vec{n}<{}>({s})", to.wgsl()),
        _ => s.to_string(),
    }
}

impl Gen<'_> {
    pub fn binop(&mut self, op: BOp, ta: &Ty, tb: &Ty, a: &str, b: &str) -> String {
        let o = op_str(op);
        match op {
            BOp::Add | BOp::Sub | BOp::Div if ta.is_mat() || tb.is_mat() => {
                if ta.is_mat() && tb.is_mat() && op != BOp::Div {
                    return format!("({a} {o} {b})");
                }
                let mt = if ta.is_mat() { ta } else { tb };
                let Ty::M(c, r) = *mt else { return format!("({a} {o} {b})") };
                let name = format!("h_{}_{}_{}", ["add", "sub", "div"][[BOp::Add, BOp::Sub, BOp::Div].iter().position(|x| *x == op).unwrap_or(0)], mangle(ta), mangle(tb));
                let (wa, wb, wm) = (wty(self.m, ta), wty(self.m, tb), wty(self.m, mt));
                let cols: Vec<String> = (0..c)
                    .map(|j| {
                        let x = if ta.is_mat() { format!("a[{j}]") } else { "a".into() };
                        let y = if tb.is_mat() { format!("b[{j}]") } else { "b".into() };
                        format!("{x} {o} {y}")
                    })
                    .collect();
                let _ = r;
                self.helper(&name, || format!("fn {name}(a: {wa}, b: {wb}) -> {wm} {{ return {wm}({}); }}\n", cols.join(", ")));
                format!("{name}({a}, {b})")
            }
            BOp::Shl | BOp::Shr => {
                let rhs = match (ta, tb) {
                    (Ty::V(_, n), Ty::S(_)) => format!("vec{n}<u32>({} & 31u)", conv(b, tb, K::Uint)),
                    (Ty::V(_, n), _) => format!("({} & vec{n}<u32>(31u))", conv(b, tb, K::Uint)),
                    _ => format!("({} & 31u)", conv(b, tb, K::Uint)),
                };
                format!("({a} {o} {rhs})")
            }
            BOp::And | BOp::Or | BOp::Xor => {
                let (a, b) = match (ta, tb) {
                    (Ty::V(k, n), Ty::S(_)) => (a.to_string(), format!("vec{n}<{}>({b})", k.wgsl())),
                    (Ty::S(_), Ty::V(k, n)) => (format!("vec{n}<{}>({a})", k.wgsl()), b.to_string()),
                    _ => (a.to_string(), b.to_string()),
                };
                format!("({a} {o} {b})")
            }
            BOp::Eq => self.eq(ta, a, b),
            BOp::Ne => format!("(!{})", self.eq(ta, a, b)),
            _ => format!("({a} {o} {b})"),
        }
    }

    /// GLSL `==` (one boolean for any type).
    pub fn eq(&mut self, t: &Ty, a: &str, b: &str) -> String {
        match t {
            Ty::S(_) => format!("({a} == {b})"),
            Ty::V(..) => format!("all({a} == {b})"),
            _ => {
                let name = self.eq_helper(t);
                format!("{name}({a}, {b})")
            }
        }
    }

    fn eq_helper(&mut self, t: &Ty) -> String {
        let name = format!("h_eq_{}", mangle(t));
        if self.helpers.contains_key(&name) {
            return name;
        }
        let w = wty(self.m, t);
        let body = match t {
            Ty::M(c, _) => (0..*c).map(|j| format!("all(a[{j}] == b[{j}])")).collect::<Vec<_>>().join(" && "),
            Ty::Struct(s) => {
                let fields = self.m.structs[*s as usize].fields.clone();
                let parts: Vec<String> = fields.iter().map(|(f, ft)| self.eq(ft, &format!("a.m_{f}"), &format!("b.m_{f}"))).collect();
                parts.join(" && ")
            }
            Ty::Array(e, n) => {
                let inner = self.eq(e, "x[i]", "y[i]");
                let code = format!(
                    "fn {name}(a: {w}, b: {w}) -> bool {{ var x = a; var y = b; var r = true; for (var i = 0; i < {n}; i++) {{ r = r && {inner}; }} return r; }}\n"
                );
                self.helpers.insert(name.clone(), code);
                return name;
            }
            _ => "true".into(),
        };
        self.helpers.insert(name.clone(), format!("fn {name}(a: {w}, b: {w}) -> bool {{ return {body}; }}\n"));
        name
    }

    pub fn ctor(&mut self, t: &Ty, args: &[Expr]) -> String {
        let refs: Vec<&Expr> = args.iter().collect();
        let v = self.operands(&refs);
        let w = wty(self.m, t);
        match t {
            Ty::S(k) => {
                let a = &args[0].ty;
                let first = match a {
                    Ty::V(..) => format!("{}.x", v[0]),
                    Ty::M(..) => format!("{}[0].x", v[0]),
                    _ => v[0].clone(),
                };
                conv(&first, &Ty::S(a.kind().unwrap_or(K::Float)), *k)
            }
            Ty::V(k, n) => {
                if args.len() == 1 {
                    match &args[0].ty {
                        Ty::S(_) => return format!("{w}({})", conv(&v[0], &args[0].ty, *k)),
                        Ty::V(ka, m) => {
                            let s = if m > n { format!("{}.{}", v[0], &XYZW[..*n as usize]) } else { v[0].clone() };
                            return conv(&s, &Ty::V(*ka, *n), *k);
                        }
                        _ => {}
                    }
                }
                let pieces = self.pieces(args, v, *n as u32, *k, false);
                format!("{w}({})", pieces.join(", "))
            }
            Ty::M(c, r) => {
                let (c, r) = (*c, *r);
                if args.len() == 1 {
                    match &args[0].ty {
                        Ty::S(_) => {
                            let s = self.hoist(&conv(&v[0], &args[0].ty, K::Float), &FLOAT);
                            let cols: Vec<String> = (0..c)
                                .map(|j| {
                                    let comps: Vec<String> = (0..r).map(|i| if i == j { s.clone() } else { "0.0f".into() }).collect();
                                    format!("vec{r}<f32>({})", comps.join(", "))
                                })
                                .collect();
                            return format!("{w}({})", cols.join(", "));
                        }
                        Ty::M(sc, sr) => {
                            let (sc, sr) = (*sc, *sr);
                            if sc == c && sr == r {
                                return v[0].clone();
                            }
                            let s = self.hoist(&v[0], &args[0].ty);
                            let cols: Vec<String> = (0..c)
                                .map(|j| {
                                    if j >= sc {
                                        let comps: Vec<&str> = (0..r).map(|i| if i == j { "1.0f" } else { "0.0f" }).collect();
                                        format!("vec{r}<f32>({})", comps.join(", "))
                                    } else if r <= sr {
                                        if r == sr {
                                            format!("{s}[{j}]")
                                        } else {
                                            format!("{s}[{j}].{}", &XYZW[..r as usize])
                                        }
                                    } else {
                                        let pad: Vec<&str> = (sr..r).map(|i| if i == j { "1.0f" } else { "0.0f" }).collect();
                                        format!("vec{r}<f32>({s}[{j}], {})", pad.join(", "))
                                    }
                                })
                                .collect();
                            return format!("{w}({})", cols.join(", "));
                        }
                        _ => {}
                    }
                }
                if args.len() == c as usize && args.iter().all(|a| matches!(a.ty, Ty::V(_, n) if n == r)) {
                    let cols: Vec<String> = v.iter().zip(args).map(|(s, a)| conv(s, &a.ty, K::Float)).collect();
                    return format!("{w}({})", cols.join(", "));
                }
                let pieces = self.pieces(args, v, c as u32 * r as u32, K::Float, true);
                format!("{w}({})", pieces.join(", "))
            }
            _ => format!("{w}({})", v.join(", ")),
        }
    }

    /// Components for a constructor: `need` components of kind `k`, as scalars and vectors
    /// (only scalars when `scalars_only`).
    fn pieces(&mut self, args: &[Expr], v: Vec<String>, need: u32, k: K, scalars_only: bool) -> Vec<String> {
        let mut out = Vec::new();
        let mut rem = need;
        for (s, a) in v.into_iter().zip(args) {
            if rem == 0 {
                break;
            }
            match &a.ty {
                Ty::S(_) => {
                    out.push(conv(&s, &a.ty, k));
                    rem -= 1;
                }
                Ty::V(ka, m) => {
                    let take = (*m as u32).min(rem);
                    if scalars_only {
                        let s = self.hoist(&s, &a.ty);
                        for i in 0..take as usize {
                            out.push(conv(&format!("{s}.{}", &XYZW[i..i + 1]), &Ty::S(*ka), k));
                        }
                    } else {
                        let p = if take < *m as u32 { format!("{s}.{}", &XYZW[..take as usize]) } else { s };
                        out.push(conv(&p, &Ty::vec(*ka, take as u8), k));
                    }
                    rem -= take;
                }
                Ty::M(c, r) => {
                    let s = self.hoist(&s, &a.ty);
                    for j in 0..*c {
                        for i in 0..*r as usize {
                            if rem == 0 {
                                break;
                            }
                            out.push(conv(&format!("{s}[{j}].{}", &XYZW[i..i + 1]), &FLOAT, k));
                            rem -= 1;
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }
}
