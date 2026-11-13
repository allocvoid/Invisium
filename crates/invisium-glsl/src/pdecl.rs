//! Global declarations, variable declarators, functions and the whole-shader checks.

use crate::ir::*;
use crate::parse::{Parser, Sym, Q, R};
use crate::{err, Stage};

impl Parser<'_> {
    pub fn translation_unit(&mut self) -> R<()> {
        while self.i < self.t.len() {
            self.external()?;
        }
        self.finish()
    }

    fn external(&mut self) -> R<()> {
        if self.eat(";") {
            return Ok(());
        }
        let line = self.line();
        if self.is_id("precision") {
            return self.precision_stmt();
        }
        if self.is_id("invariant") && self.peek_at(1).id().is_some_and(|n| matches!(self.lookup(n), Some(Sym::Var(_)))) {
            self.i += 1;
            loop {
                let l = self.line();
                let n = self.ident()?;
                if !matches!(self.lookup(&n), Some(Sym::Var(_))) {
                    return err(l, format!("'{n}' : undeclared identifier"));
                }
                if !self.eat(",") {
                    break;
                }
            }
            return self.expect(";");
        }
        let q = self.qualifiers()?;
        if q.storage == Some("uniform") && self.is(";") {
            self.i += 1;
            return Ok(());
        }
        if q.storage == Some("uniform") && self.peek().id().is_some() && self.peek_at(1).is("{") && !self.at_type() {
            return err(line, "uniform blocks are not supported yet");
        }
        let ty = self.type_spec()?;
        if self.eat(";") {
            return Ok(());
        }
        let nl = self.line();
        let name = self.ident()?;
        if self.is("(") {
            if q.storage.is_some() || q.layout {
                return err(line, format!("'{name}' : functions cannot have storage qualifiers"));
            }
            return self.function(ty, name, nl);
        }
        self.declarators(&q, ty, name, nl, true).map(|_| ())
    }

    pub fn precision_stmt(&mut self) -> R<()> {
        let line = self.line();
        self.i += 1;
        if !matches!(self.peek().id(), Some("lowp" | "mediump" | "highp")) {
            return err(line, "precision statement needs lowp, mediump or highp");
        }
        self.i += 1;
        let ty = self.type_spec()?;
        if !matches!(ty, Ty::S(K::Int | K::Float) | Ty::Sampler(_)) {
            return err(line, "precision can only be set for int, float and sampler types");
        }
        self.expect(";")
    }

    /// The declarators after a type: `a, b[3] = ..., c = ...;`. Returns the local declarations.
    pub fn declarators(&mut self, q: &Q, base: Ty, first: String, line: u32, global: bool) -> R<Vec<Stmt>> {
        let mut out = Vec::new();
        let mut name = first;
        let mut line = line;
        loop {
            self.check_name(&name, line)?;
            let mut ty = base.clone();
            if self.eat("[") {
                if matches!(ty, Ty::Array(..)) {
                    return err(line, "arrays of arrays are not allowed");
                }
                ty = Ty::Array(Box::new(ty), self.array_size()?);
            }
            let init = if self.eat("=") { Some(self.assign()?) } else { None };
            if let (Ty::Array(e, 0), Some(i)) = (&ty, &init) {
                if let Ty::Array(ie, n) = &i.ty {
                    if ie == e {
                        ty = Ty::Array(e.clone(), *n);
                    }
                }
            }
            if matches!(ty, Ty::Array(_, 0)) {
                return err(line, format!("'{name}' : array size missing"));
            }
            if ty == Ty::Void {
                return err(line, format!("'{name}' : variables cannot be void"));
            }
            let storage = self.storage_of(q, global, &ty, &name, line)?;
            if let Some(i) = &init {
                if matches!(storage, Storage::Uniform | Storage::Attribute | Storage::VaryingIn | Storage::VaryingOut | Storage::FragOut) {
                    return err(line, format!("'{name}' : this kind of variable cannot have an initializer"));
                }
                if i.ty != ty {
                    return err(line, format!("'=' : cannot convert from '{}' to '{}'", i.ty.name(&self.m), ty.name(&self.m)));
                }
            } else if storage == Storage::Const {
                return err(line, format!("'{name}' : constants must be initialized"));
            }
            let cval = if storage == Storage::Const { init.as_ref().and_then(|i| self.fold(i)) } else { None };
            let mut v = Parser::var(&name, ty, storage, line, global);
            v.location = q.location;
            v.interp = q.interp;
            v.cval = cval;
            let id = self.add_var(v)?;
            if global {
                if let Some(i) = init {
                    self.m.inits.push((id, i));
                }
            } else {
                out.push(Stmt::Decl(id, init));
            }
            if !self.eat(",") {
                break;
            }
            line = self.line();
            name = self.ident()?;
        }
        self.expect(";")?;
        Ok(out)
    }

    fn storage_of(&self, q: &Q, global: bool, ty: &Ty, name: &str, line: u32) -> R<Storage> {
        let v3 = self.version == 300;
        let vert = self.stage == Stage::Vertex;
        let bad = |what: &str| err(line, format!("'{name}' : {what}"));
        let st = match q.storage {
            None => {
                if global {
                    Storage::Global
                } else {
                    Storage::Local
                }
            }
            Some("const") => Storage::Const,
            Some(s) if !global => return bad(&format!("'{s}' is only allowed at global scope")),
            Some("uniform") => Storage::Uniform,
            Some("attribute") if !v3 && vert => Storage::Attribute,
            Some("varying") if !v3 => {
                if vert {
                    Storage::VaryingOut
                } else {
                    Storage::VaryingIn
                }
            }
            Some("in") if v3 => {
                if vert {
                    Storage::Attribute
                } else {
                    Storage::VaryingIn
                }
            }
            Some("out") if v3 => {
                if vert {
                    Storage::VaryingOut
                } else {
                    Storage::FragOut
                }
            }
            Some(s) => return bad(&format!("storage qualifier '{s}' is not allowed here")),
        };
        if ty.has_sampler(&self.m) && st != Storage::Uniform {
            return bad("samplers must be uniforms or function parameters");
        }
        if let Ty::Sampler(_) = ty {
        } else if let Ty::Array(e, _) = ty {
            if matches!(**e, Ty::Sampler(_)) && st != Storage::Uniform {
                return bad("samplers must be uniforms");
            }
        }
        if ty.has_sampler(&self.m) && !matches!(ty, Ty::Sampler(_)) && !matches!(ty, Ty::Array(e, _) if matches!(**e, Ty::Sampler(_))) {
            return bad("samplers inside structures are not supported yet");
        }
        let elem = match ty {
            Ty::Array(e, _) => &**e,
            t => t,
        };
        match st {
            Storage::Attribute => {
                let ok = matches!(ty, Ty::S(K::Float) | Ty::V(K::Float, _) | Ty::M(..))
                    || (v3 && matches!(ty, Ty::S(K::Int | K::Uint) | Ty::V(K::Int | K::Uint, _)));
                if !ok {
                    return bad("invalid type for a vertex input");
                }
            }
            Storage::VaryingIn | Storage::VaryingOut => {
                if !matches!(elem, Ty::S(_) | Ty::V(..) | Ty::M(..)) || elem.is_bool() {
                    return bad("invalid type for a varying");
                }
                if elem.is_int() && !q.interp.flat {
                    return bad("integer varyings must be qualified 'flat'");
                }
            }
            Storage::FragOut => {
                if !elem.is_sv() || elem.is_bool() {
                    return bad("invalid type for a fragment output");
                }
            }
            _ => {}
        }
        if q.location.is_some() && !matches!(st, Storage::Attribute | Storage::FragOut) {
            return bad("layout(location) is only allowed on vertex inputs and fragment outputs");
        }
        if (q.interp.flat || q.interp.centroid) && !matches!(st, Storage::VaryingIn | Storage::VaryingOut) {
            return bad("interpolation qualifiers are only allowed on varyings");
        }
        Ok(st)
    }

    fn function(&mut self, ret: Ty, name: String, line: u32) -> R<()> {
        if crate::builtins::is_builtin(self, &name) {
            return err(line, format!("'{name}' : cannot redefine or overload a built-in function"));
        }
        self.check_name(&name, line)?;
        if matches!(ret, Ty::Sampler(_)) || ret.has_sampler(&self.m) {
            return err(line, "functions cannot return samplers");
        }
        self.expect("(")?;
        self.push_scope();
        let mut params = Vec::new();
        if self.is_id("void") && self.peek_at(1).is(")") {
            self.i += 1;
        }
        if !self.eat(")") {
            loop {
                let pl = self.line();
                let mut pq = PQ::In;
                loop {
                    match self.peek().id() {
                        Some("const" | "lowp" | "mediump" | "highp") => {}
                        Some("in") => pq = PQ::In,
                        Some("out") => pq = PQ::Out,
                        Some("inout") => pq = PQ::InOut,
                        _ => break,
                    }
                    self.i += 1;
                }
                let mut ty = self.type_spec()?;
                let pname = if self.peek().id().is_some() { self.ident()? } else { format!("_p{}", params.len()) };
                if self.eat("[") {
                    if matches!(ty, Ty::Array(..)) {
                        return err(pl, "arrays of arrays are not allowed");
                    }
                    let n = self.array_size()?;
                    if n == 0 {
                        return err(pl, "parameter arrays need a size");
                    }
                    ty = Ty::Array(Box::new(ty), n);
                }
                if ty == Ty::Void {
                    return err(pl, "parameters cannot be void");
                }
                if ty.has_sampler(&self.m) && !matches!(ty, Ty::Sampler(_)) {
                    return err(pl, "arrays or structures of samplers as parameters are not supported yet");
                }
                if matches!(ty, Ty::Sampler(_)) && pq != PQ::In {
                    return err(pl, "samplers can only be 'in' parameters");
                }
                if !pname.starts_with("_p") {
                    self.check_name(&pname, pl)?;
                }
                let v = Parser::var(&pname, ty, Storage::Param(pq), pl, false);
                let id = self.add_var(v)?;
                params.push(id);
                if !self.eat(",") {
                    break;
                }
            }
            self.expect(")")?;
        }
        let types: Vec<Ty> = params.iter().map(|p| self.m.vars[*p as usize].ty.clone()).collect();
        let existing = match self.scopes[0].get(&name) {
            Some(Sym::Funcs(fs)) => fs.clone(),
            Some(_) => return err(line, format!("'{name}' : redefinition")),
            None => Vec::new(),
        };
        let mut fid = None;
        for f in existing {
            let fd = &self.m.funcs[f as usize];
            let ft: Vec<&Ty> = fd.params.iter().map(|p| &self.m.vars[*p as usize].ty).collect();
            if ft.len() == types.len() && ft.iter().zip(&types).all(|(a, b)| *a == b) {
                if fd.ret != ret {
                    return err(line, format!("'{name}' : overloaded functions must not differ only in return type"));
                }
                fid = Some(f);
            }
        }
        let fid = match fid {
            Some(f) => f,
            None => {
                let f = self.m.funcs.len() as FuncId;
                self.m.funcs.push(Func { name: name.clone(), ret: ret.clone(), params: params.clone(), body: None, line, calls: Vec::new() });
                match self.scopes[0].get_mut(&name) {
                    Some(Sym::Funcs(fs)) => fs.push(f),
                    _ => {
                        self.scopes[0].insert(name.clone(), Sym::Funcs(vec![f]));
                    }
                }
                f
            }
        };
        if self.eat(";") {
            self.pop_scope();
            return Ok(());
        }
        if self.m.funcs[fid as usize].body.is_some() {
            return err(line, format!("'{name}' : function already has a body"));
        }
        if name == "main" && (ret != Ty::Void || !params.is_empty()) {
            return err(line, "main() must be 'void main()'");
        }
        self.m.funcs[fid as usize].params = params;
        self.m.funcs[fid as usize].line = line;
        self.expect("{")?;
        self.func = Some(fid);
        let mut body = Vec::new();
        while !self.eat("}") {
            if self.i >= self.t.len() {
                return err(self.line(), "syntax error: unexpected end of file, missing '}'");
            }
            body.extend(self.statement()?);
        }
        self.func = None;
        self.pop_scope();
        self.m.funcs[fid as usize].body = Some(body);
        Ok(())
    }

    fn finish(&mut self) -> R<()> {
        let last = self.eof.line;
        let main = self.m.funcs.iter().position(|f| f.name == "main" && f.body.is_some());
        let Some(main) = main else { return err(last, "missing main() function") };
        self.m.main = main as FuncId;
        if self.used_frag_color && self.used_frag_data {
            return err(last, "cannot use both gl_FragColor and gl_FragData");
        }
        // Recursion: depth-first search with an explicit stack; colour 1 = on the path.
        let n = self.m.funcs.len();
        let mut colour = vec![0u8; n];
        for root in std::iter::once(main).chain(0..n) {
            if colour[root] != 0 {
                continue;
            }
            let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
            colour[root] = 1;
            while let Some(top) = stack.last_mut() {
                let (f, k) = *top;
                if let Some(&(callee, cl)) = self.m.funcs[f].calls.get(k) {
                    top.1 += 1;
                    let c = callee as usize;
                    match colour[c] {
                        0 => {
                            colour[c] = 1;
                            stack.push((c, 0));
                        }
                        1 => return err(cl, format!("'{}' : recursive function calls are not allowed", self.m.funcs[c].name)),
                        _ => {}
                    }
                } else {
                    colour[f] = 2;
                    stack.pop();
                }
            }
        }
        // Every function reachable from main or a global initialiser must have a body.
        let mut seen = vec![false; n];
        let mut work: Vec<(FuncId, u32)> = self.init_calls.clone();
        work.push((self.m.main, 0));
        while let Some((f, cl)) = work.pop() {
            if std::mem::replace(&mut seen[f as usize], true) {
                continue;
            }
            let fd = &self.m.funcs[f as usize];
            if fd.body.is_none() {
                return err(cl, format!("'{}' : function has no definition", fd.name));
            }
            work.extend(fd.calls.iter().copied());
        }
        Ok(())
    }
}
