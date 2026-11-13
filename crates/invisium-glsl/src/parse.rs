//! Parser and type checker in one pass: tokens in, typed IR (`ir::Module`) out.
//! Declarations, types and functions live here; statements in `pstmt`, expressions in
//! `pexpr`, typing rules in `check`.

use crate::ir::*;
use crate::lex::{Tok, T};
use crate::pp::Exts;
use crate::{err, Error, Stage};
use std::collections::HashMap;

pub type R<X> = Result<X, Error>;

/// Deepest nesting of statements and parenthesised expressions.
pub const MAX_NEST: u32 = 96;
/// Largest number of elements in an array.
pub const MAX_ARRAY: u32 = 1 << 16;
/// Largest variable or structure, in 4-byte words.
pub const MAX_WORDS: u64 = 1 << 22;
/// Deepest nesting of structure types, and most structure definitions per shader.
pub const MAX_STRUCT_DEPTH: u32 = 32;
pub const MAX_STRUCTS: usize = 1024;

#[derive(Clone)]
pub enum Sym {
    Var(VarId),
    Struct(u32),
    Funcs(Vec<FuncId>),
}

#[derive(Default)]
pub struct Q {
    pub storage: Option<&'static str>,
    pub interp: Interp,
    pub location: Option<u32>,
    pub layout: bool,
    pub any: bool,
}

pub struct Parser<'a> {
    pub t: &'a [Tok],
    pub i: usize,
    pub eof: Tok,
    pub m: Module,
    pub scopes: Vec<HashMap<String, Sym>>,
    pub stage: Stage,
    pub version: u32,
    pub exts: Exts,
    pub depth: u32,
    pub func: Option<FuncId>,
    pub loops: u32,
    pub switches: u32,
    /// Calls made by global initialisers.
    pub init_calls: Vec<(FuncId, u32)>,
    pub used_frag_color: bool,
    pub used_frag_data: bool,
}

const RESERVED: &[&str] = &[
    "asm", "class", "union", "enum", "typedef", "template", "this", "packed", "goto", "inline", "noinline",
    "volatile", "public", "static", "extern", "external", "interface", "long", "short", "double", "half", "fixed",
    "unsigned", "superp", "input", "output", "hvec2", "hvec3", "hvec4", "dvec2", "dvec3", "dvec4", "fvec2",
    "fvec3", "fvec4", "sampler1D", "sampler1DShadow", "sampler2DRect", "sampler3DRect", "sampler2DRectShadow",
    "sizeof", "cast", "namespace", "using", "resource", "patch",
];

const KEYWORDS: &[&str] = &[
    "attribute", "const", "uniform", "varying", "break", "continue", "do", "for", "while", "if", "else", "in", "out",
    "inout", "true", "false", "lowp", "mediump", "highp", "precision", "invariant", "discard", "return", "struct",
    "layout", "centroid", "flat", "smooth", "switch", "case",
];

impl<'a> Parser<'a> {
    pub fn new(t: &'a [Tok], stage: Stage, version: u32, exts: Exts) -> Parser<'a> {
        let last = t.last().map_or(1, |t| t.line);
        let eof = Tok { t: T::P("<end of file>"), line: last, bol: true, sp: true, hs: None };
        let mut p = Parser {
            t,
            i: 0,
            eof,
            m: Module::default(),
            scopes: vec![HashMap::new()],
            stage,
            version,
            exts,
            depth: 0,
            func: None,
            loops: 0,
            switches: 0,
            init_calls: Vec::new(),
            used_frag_color: false,
            used_frag_data: false,
        };
        crate::builtins::declare_vars(&mut p);
        p
    }

    // ---- tokens ----

    pub fn peek(&self) -> &Tok {
        self.t.get(self.i).unwrap_or(&self.eof)
    }
    pub fn peek_at(&self, k: usize) -> &Tok {
        self.t.get(self.i + k).unwrap_or(&self.eof)
    }
    pub fn line(&self) -> u32 {
        self.peek().line
    }
    pub fn is(&self, p: &str) -> bool {
        self.peek().is(p)
    }
    pub fn is_id(&self, s: &str) -> bool {
        self.peek().id() == Some(s)
    }
    pub fn eat(&mut self, p: &str) -> bool {
        let y = self.is(p) || self.is_id(p);
        if y {
            self.i += 1;
        }
        y
    }
    pub fn expect(&mut self, p: &str) -> R<()> {
        if self.eat(p) {
            Ok(())
        } else {
            err(self.line(), format!("syntax error: expected '{p}' but found '{}'", self.peek().t))
        }
    }
    pub fn ident(&mut self) -> R<String> {
        match &self.peek().t {
            T::Id(s) => {
                let s = s.to_string();
                self.i += 1;
                Ok(s)
            }
            t => err(self.line(), format!("syntax error: expected an identifier but found '{t}'")),
        }
    }
    pub fn enter(&mut self) -> R<()> {
        self.depth += 1;
        if self.depth > MAX_NEST {
            return err(self.line(), "nesting too deep");
        }
        Ok(())
    }
    pub fn leave(&mut self) {
        self.depth -= 1;
    }

    // ---- scopes ----

    pub fn lookup(&self, name: &str) -> Option<&Sym> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }
    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    pub fn pop_scope(&mut self) {
        self.scopes.pop();
    }
    pub fn check_name(&self, name: &str, line: u32) -> R<()> {
        if name.starts_with("gl_") {
            return err(line, format!("'{name}' : reserved built-in name"));
        }
        if RESERVED.contains(&name) || KEYWORDS.contains(&name) || self.basic_type(name, line).is_some() {
            return err(line, format!("'{name}' : reserved word or type name used as an identifier"));
        }
        Ok(())
    }
    pub fn add_var(&mut self, v: Var) -> R<VarId> {
        let line = v.line;
        if self.scopes.last().is_some_and(|s| s.contains_key(&v.name)) {
            return err(line, format!("'{}' : redefinition", v.name));
        }
        if v.ty.words(&self.m) > MAX_WORDS {
            return err(line, format!("'{}' : variable too large", v.name));
        }
        let id = self.m.vars.len() as VarId;
        if let Some(s) = self.scopes.last_mut() {
            s.insert(v.name.clone(), Sym::Var(id));
        }
        if v.global {
            self.m.globals.push(id);
        }
        self.m.vars.push(v);
        Ok(id)
    }
    pub fn var(name: &str, ty: Ty, storage: Storage, line: u32, global: bool) -> Var {
        Var { name: name.into(), ty, storage, line, location: None, interp: Interp::default(), cval: None, global }
    }

    // ---- types ----

    /// A built-in type name. `Some(Err)` for names that exist but not in this version.
    pub fn basic_type(&self, name: &str, line: u32) -> Option<R<Ty>> {
        let v3 = self.version == 300;
        let need = |t: Ty, ok: bool| Some(if ok { Ok(t) } else { err(line, format!("'{name}' : requires version 300")) });
        let vk = |s: &str| match s {
            "vec" => Some(K::Float),
            "ivec" => Some(K::Int),
            "uvec" => Some(K::Uint),
            "bvec" => Some(K::Bool),
            _ => None,
        };
        match name {
            "void" => return Some(Ok(Ty::Void)),
            "bool" => return Some(Ok(BOOL)),
            "int" => return Some(Ok(INT)),
            "float" => return Some(Ok(FLOAT)),
            "uint" => return need(UINT, v3),
            _ => {}
        }
        if let Some(n) = name.strip_suffix(['2', '3', '4']) {
            let size = name.as_bytes()[name.len() - 1] - b'0';
            if let Some(k) = vk(n) {
                return need(Ty::V(k, size), k != K::Uint || v3);
            }
            if n == "mat" {
                return Some(Ok(Ty::M(size, size)));
            }
        }
        if let Some(rest) = name.strip_prefix("mat") {
            let b = rest.as_bytes();
            if b.len() == 3 && b[1] == b'x' && (b'2'..=b'4').contains(&b[0]) && (b'2'..=b'4').contains(&b[2]) {
                if !v3 {
                    return need(FLOAT, false);
                }
                if b[0] != b[2] {
                    return Some(err(line, format!("'{name}' : non-square matrices are not supported yet")));
                }
                return Some(Ok(Ty::M(b[0] - b'0', b[0] - b'0')));
            }
        }
        let s = SAMPLERS.iter().find(|s| s.0 == name)?;
        need(Ty::Sampler(s.1), v3 || matches!(s.1, Samp::S2D | Samp::Cube))
    }

    fn is_precision(&self) -> bool {
        self.is_id("lowp") || self.is_id("mediump") || self.is_id("highp")
    }

    /// Does a type start here (for telling declarations from expressions)?
    pub fn at_type(&self) -> bool {
        match self.peek().id() {
            Some("struct") => true,
            Some(s) => self.basic_type(s, 0).is_some() || matches!(self.lookup(s), Some(Sym::Struct(_))),
            None => false,
        }
    }

    pub fn type_spec(&mut self) -> R<Ty> {
        while self.is_precision() {
            self.i += 1;
        }
        let line = self.line();
        let name = self.ident()?;
        let ty = if name == "struct" {
            self.struct_def(line)?
        } else if let Some(t) = self.basic_type(&name, line) {
            t?
        } else if let Some(Sym::Struct(s)) = self.lookup(&name) {
            Ty::Struct(*s)
        } else {
            return err(line, format!("'{name}' : syntax error: expected a type"));
        };
        if self.is("[") {
            if self.version != 300 {
                return err(line, "array types require version 300");
            }
            self.i += 1;
            let n = self.array_size()?;
            return Ok(Ty::Array(Box::new(ty), n));
        }
        Ok(ty)
    }

    fn struct_def(&mut self, line: u32) -> R<Ty> {
        self.enter()?;
        let name = if self.peek().id().is_some() { self.ident()? } else { format!("anon{}", self.m.structs.len()) };
        self.check_name(&name, line)?;
        self.expect("{")?;
        let mut fields: Vec<(String, Ty)> = Vec::new();
        while !self.eat("}") {
            let fl = self.line();
            let base = self.type_spec()?;
            if matches!(base, Ty::Void | Ty::Sampler(_)) {
                return err(fl, "structure members cannot be void or samplers");
            }
            loop {
                let fl = self.line();
                let f = self.ident()?;
                let mut ty = base.clone();
                if self.eat("[") {
                    if matches!(ty, Ty::Array(..)) {
                        return err(fl, "arrays of arrays are not allowed");
                    }
                    let n = self.array_size()?;
                    if n == 0 {
                        return err(fl, "structure member arrays need a size");
                    }
                    ty = Ty::Array(Box::new(ty), n);
                }
                if fields.iter().any(|x| x.0 == f) {
                    return err(fl, format!("'{f}' : duplicate field name in structure"));
                }
                fields.push((f, ty));
                if !self.eat(",") {
                    break;
                }
            }
            self.expect(";")?;
        }
        if fields.is_empty() {
            return err(line, "structures must have at least one member");
        }
        if self.m.structs.len() >= MAX_STRUCTS {
            return err(line, "too many structure definitions");
        }
        let words = fields.iter().fold(0u64, |a, f| a.saturating_add(f.1.words(&self.m)));
        if words > MAX_WORDS {
            return err(line, format!("'{name}' : structure too large"));
        }
        let depth = 1 + fields.iter().map(|f| f.1.struct_depth(&self.m)).max().unwrap_or(0);
        if depth > MAX_STRUCT_DEPTH {
            return err(line, format!("'{name}' : structures nested too deeply (limit {MAX_STRUCT_DEPTH})"));
        }
        let sampler = fields.iter().any(|f| f.1.has_sampler(&self.m));
        let (offs, size) = crate::layout::compute_struct_layout(&self.m, &fields);
        let id = self.m.structs.len() as u32;
        self.m.structs.push(StructDef { name: name.clone(), fields, words, sampler, depth, offs, size });
        if self.scopes.last().is_some_and(|s| s.contains_key(&name)) {
            return err(line, format!("'{name}' : redefinition"));
        }
        if let Some(s) = self.scopes.last_mut() {
            s.insert(name, Sym::Struct(id));
        }
        self.leave();
        Ok(Ty::Struct(id))
    }

    /// After `[`: a constant size and `]`. 0 means "no size given".
    pub fn array_size(&mut self) -> R<u32> {
        let line = self.line();
        if self.eat("]") {
            return Ok(0);
        }
        let e = self.cond()?;
        self.expect("]")?;
        match self.fold(&e).and_then(|c| c.as_i64()) {
            Some(n) if n > 0 && n <= MAX_ARRAY as i64 => Ok(n as u32),
            Some(n) if n > 0 => err(line, "array size too large"),
            _ => err(line, "array size must be a constant integer expression greater than zero"),
        }
    }

    pub fn qualifiers(&mut self) -> R<Q> {
        let mut q = Q::default();
        loop {
            let line = self.line();
            let Some(w) = self.peek().id() else { break };
            let st: &'static str = match w {
                "const" => "const",
                "attribute" => "attribute",
                "varying" => "varying",
                "uniform" => "uniform",
                "in" => "in",
                "out" => "out",
                "inout" => "inout",
                "flat" | "smooth" | "centroid" if self.version == 300 || w == "centroid" => {
                    q.interp.flat |= w == "flat";
                    q.interp.centroid |= w == "centroid";
                    self.i += 1;
                    q.any = true;
                    continue;
                }
                "invariant" | "lowp" | "mediump" | "highp" => {
                    self.i += 1;
                    continue;
                }
                "layout" if self.version == 300 => {
                    self.i += 1;
                    self.layout(&mut q)?;
                    q.any = true;
                    continue;
                }
                _ => break,
            };
            if q.storage.is_some() {
                return err(line, format!("'{w}' : more than one storage qualifier"));
            }
            q.storage = Some(st);
            q.any = true;
            self.i += 1;
        }
        Ok(q)
    }

    fn layout(&mut self, q: &mut Q) -> R<()> {
        q.layout = true;
        self.expect("(")?;
        loop {
            let line = self.line();
            let k = self.ident()?;
            if self.eat("=") {
                let v = match self.peek().t {
                    T::Int(v, _) => v,
                    _ => return err(line, "layout qualifier value must be an integer constant"),
                };
                self.i += 1;
                match k.as_str() {
                    "location" if v < 64 => q.location = Some(v as u32),
                    "location" => return err(line, "layout location out of range"),
                    "binding" | "offset" => return err(line, format!("'{k}' : layout qualifier not supported")),
                    _ => return err(line, format!("'{k}' : unknown layout qualifier")),
                }
            } else if !matches!(k.as_str(), "std140" | "shared" | "packed" | "row_major" | "column_major") {
                return err(line, format!("'{k}' : unknown layout qualifier"));
            }
            if !self.eat(",") {
                break;
            }
        }
        self.expect(")")
    }
}
