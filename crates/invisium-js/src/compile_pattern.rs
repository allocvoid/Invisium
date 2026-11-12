//! Assignment, destructuring, object literals and classes.

use crate::ast::*;
use crate::bytecode::Op;
use crate::compiler::*;

impl Compiler {
    /// Binds/assigns the value on top of the stack to `p` (consumes it).
    /// `kind` = Some(declaration kind) for declarations, None for assignment.
    pub fn pattern_bind(&mut self, p: &Pattern, kind: Option<VarKind>) -> CResult<()> {
        match p {
            Pattern::Ident(n, pos) => {
                self.mark(*pos);
                match kind {
                    Some(k) => self.init_var(n, k),
                    None => self.put_var(n),
                }
            }
            Pattern::Expr(e) => {
                let tmp = self.alloc_local();
                self.emit(Op::InitLocal(tmp));
                match e {
                    Expr::Member { obj, name, .. } => {
                        self.expr(obj)?;
                        self.emit(Op::GetLocal(tmp));
                        let n = self.name(name);
                        self.emit(Op::PutProp(n));
                    }
                    Expr::Index { obj, index, .. } => {
                        self.expr(obj)?;
                        self.expr(index)?;
                        self.emit(Op::GetLocal(tmp));
                        self.emit(Op::PutElem);
                    }
                    Expr::PrivateMember { obj, name, .. } => {
                        self.expr(obj)?;
                        let n = self.get_private(name)?;
                        self.emit(Op::GetLocal(tmp));
                        self.emit(Op::PrivPut(n));
                    }
                    Expr::SuperMember(name, _) => {
                        self.load_this();
                        let n = self.name(name);
                        self.emit(Op::Str(n));
                        self.emit(Op::GetLocal(tmp));
                        self.emit(Op::SuperPutElem);
                    }
                    Expr::SuperIndex(k, _) => {
                        self.load_this();
                        self.expr(k)?;
                        self.emit(Op::ToPropertyKey);
                        self.emit(Op::GetLocal(tmp));
                        self.emit(Op::SuperPutElem);
                    }
                    _ => return Err(self.err("Invalid assignment target", Pos::default())),
                }
                self.emit(Op::Pop);
            }
            Pattern::Default(inner, d) => {
                self.emit(Op::Dup);
                self.emit(Op::Undef);
                self.emit(Op::StrictEq);
                let j = self.emit(Op::JumpIfFalse(0));
                self.emit(Op::Pop);
                self.expr_named(d, inner)?;
                self.patch(j);
                self.pattern_bind(inner, kind)?;
            }
            Pattern::Object { props, rest } => {
                self.emit(Op::CheckObjectCoercible);
                let mut key_temps = Vec::new();
                for (k, v) in props {
                    self.emit(Op::Dup);
                    if rest.is_some() {
                        self.push_key(k)?;
                        let t = self.alloc_local();
                        self.emit(Op::Dup);
                        self.emit(Op::InitLocal(t));
                        key_temps.push(t);
                        self.emit(Op::GetElem);
                    } else {
                        match self.prop_key(k)? {
                            Some(n) => {
                                self.emit(Op::GetProp(n));
                            }
                            None => {
                                self.push_key(k)?;
                                self.emit(Op::GetElem);
                            }
                        }
                    }
                    self.pattern_bind(v, kind)?;
                }
                match rest {
                    Some(r) => {
                        for t in &key_temps {
                            self.emit(Op::GetLocal(*t));
                        }
                        self.emit(Op::ObjRest(key_temps.len() as u32));
                        self.pattern_bind(r, kind)?;
                    }
                    None => {
                        self.emit(Op::Pop);
                    }
                }
            }
            Pattern::Array { elems, rest } => {
                self.emit(Op::GetIter);
                for el in elems {
                    self.emit(Op::IterNextOrUndef);
                    match el {
                        Some(p) => self.pattern_bind(p, kind)?,
                        None => {
                            self.emit(Op::Pop);
                        }
                    }
                }
                match rest {
                    Some(r) => {
                        // a rest element drains the iterator: nothing to close
                        self.emit(Op::IterRest);
                        self.pattern_bind(r, kind)?;
                        self.emit(Op::Pop);
                    }
                    None => {
                        // close the iterator so that a generator's `finally` blocks run
                        self.emit(Op::IterClose);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn assign(&mut self, op: AssignOp, target: &Pattern, value: &Expr, pos: Pos) -> CResult<()> {
        match op {
            AssignOp::Assign => match target {
                Pattern::Ident(n, _) => {
                    self.expr_named(value, target)?;
                    self.emit(Op::Dup);
                    self.mark(pos);
                    self.put_var(n);
                }
                Pattern::Expr(Expr::Member { obj, name, pos: p, .. }) => {
                    self.expr(obj)?;
                    self.expr(value)?;
                    self.mark(*p);
                    let n = self.name(name);
                    self.emit(Op::PutProp(n));
                }
                Pattern::Expr(Expr::Index { obj, index, pos: p, .. }) => {
                    self.expr(obj)?;
                    self.expr(index)?;
                    self.emit(Op::ToPropertyKey);
                    self.expr(value)?;
                    self.mark(*p);
                    self.emit(Op::PutElem);
                }
                Pattern::Expr(Expr::PrivateMember { obj, name, pos: p, .. }) => {
                    self.expr(obj)?;
                    let n = self.get_private(name)?;
                    self.expr(value)?;
                    self.mark(*p);
                    self.emit(Op::PrivPut(n));
                }
                _ => {
                    self.expr(value)?;
                    self.emit(Op::Dup);
                    self.pattern_bind(target, None)?;
                }
            },
            AssignOp::Bin(b) => {
                let op_code = b;
                match target {
                    Pattern::Ident(n, _) => {
                        self.get_var(n);
                        self.expr(value)?;
                        self.mark(pos);
                        self.emit_bin(op_code);
                        self.emit(Op::Dup);
                        self.put_var(n);
                    }
                    Pattern::Expr(Expr::Member { obj, name, .. }) => {
                        self.expr(obj)?;
                        self.emit(Op::Dup);
                        let n = self.name(name);
                        self.mark(pos);
                        self.emit(Op::GetProp(n));
                        self.expr(value)?;
                        self.emit_bin(op_code);
                        self.emit(Op::PutProp(n));
                    }
                    Pattern::Expr(Expr::Index { obj, index, .. }) => {
                        self.expr(obj)?;
                        self.expr(index)?;
                        self.emit(Op::ToPropertyKey);
                        self.emit(Op::Dup2);
                        self.mark(pos);
                        self.emit(Op::GetElem);
                        self.expr(value)?;
                        self.emit_bin(op_code);
                        self.emit(Op::PutElem);
                    }
                    Pattern::Expr(Expr::PrivateMember { obj, name, .. }) => {
                        self.expr(obj)?;
                        let n = self.get_private(name)?;
                        self.emit(Op::Dup2);
                        self.mark(pos);
                        self.emit(Op::PrivGet(n));
                        self.expr(value)?;
                        self.emit_bin(op_code);
                        self.emit(Op::PrivPut(n));
                    }
                    _ => return Err(self.err("Invalid compound assignment target", pos)),
                }
            }
            AssignOp::Logic(l) => {
                let jop = match l {
                    LogicOp::And => Op::AndJump(0),
                    LogicOp::Or => Op::OrJump(0),
                    LogicOp::Nullish => Op::NullishJump(0),
                };
                match target {
                    Pattern::Ident(n, _) => {
                        self.get_var(n);
                        let j = self.emit(jop);
                        self.expr_named(value, target)?;
                        self.emit(Op::Dup);
                        self.put_var(n);
                        self.patch(j);
                    }
                    Pattern::Expr(Expr::Member { obj, name, .. }) => {
                        self.expr(obj)?;
                        self.emit(Op::Dup);
                        let n = self.name(name);
                        self.emit(Op::GetProp(n));
                        let j = self.emit(jop);
                        self.expr(value)?;
                        self.emit(Op::PutProp(n));
                        let end = self.emit(Op::Jump(0));
                        self.patch(j);
                        self.emit(Op::Swap);
                        self.emit(Op::Pop);
                        self.patch(end);
                    }
                    Pattern::Expr(Expr::Index { obj, index, .. }) => {
                        self.expr(obj)?;
                        self.expr(index)?;
                        self.emit(Op::ToPropertyKey);
                        self.emit(Op::Dup2);
                        self.emit(Op::GetElem);
                        let j = self.emit(jop);
                        self.expr(value)?;
                        self.emit(Op::PutElem);
                        let end = self.emit(Op::Jump(0));
                        self.patch(j);
                        self.emit(Op::Rot(3));
                        self.emit(Op::Pop);
                        self.emit(Op::Pop);
                        self.patch(end);
                    }
                    Pattern::Expr(Expr::PrivateMember { obj, name, .. }) => {
                        self.expr(obj)?;
                        let n = self.get_private(name)?;
                        self.emit(Op::Dup2);
                        self.emit(Op::PrivGet(n));
                        let j = self.emit(jop);
                        self.expr(value)?;
                        self.emit(Op::PrivPut(n));
                        let end = self.emit(Op::Jump(0));
                        self.patch(j);
                        self.emit(Op::Rot(3));
                        self.emit(Op::Pop);
                        self.emit(Op::Pop);
                        self.patch(end);
                    }
                    _ => return Err(self.err("Invalid logical assignment target", pos)),
                }
            }
        }
        Ok(())
    }

    fn emit_bin(&mut self, b: BinOp) {
        use BinOp::*;
        self.emit(match b {
            Add => Op::Add, Sub => Op::Sub, Mul => Op::Mul, Div => Op::Div, Mod => Op::Mod, Exp => Op::Exp,
            Shl => Op::Shl, Shr => Op::Shr, UShr => Op::UShr, BitAnd => Op::BitAnd, BitOr => Op::BitOr,
            BitXor => Op::BitXor, Eq => Op::Eq, Ne => Op::Ne, StrictEq => Op::StrictEq, StrictNe => Op::StrictNe,
            Lt => Op::Lt, Gt => Op::Gt, Le => Op::Le, Ge => Op::Ge, In => Op::In, InstanceOf => Op::InstanceOf,
        });
    }

    pub fn object(&mut self, props: &[ObjProp]) -> CResult<()> {
        let is_proto = |p: &ObjProp| match p {
            ObjProp::Init(PropName::Ident(s), _) => s == "__proto__",
            ObjProp::Init(PropName::Str(s), _) => *s == s16("__proto__"),
            _ => false,
        };
        if props.iter().filter(|p| is_proto(p)).count() > 1 {
            return Err(self.err("Duplicate __proto__ fields are not allowed in object literals", Pos::default()));
        }
        self.emit(Op::NewObject);
        for p in props {
            match p {
                ObjProp::Init(k, v) => {
                    let is_proto = matches!(k, PropName::Ident(s) if s == "__proto__")
                        || matches!(k, PropName::Str(s) if *s == s16("__proto__"));
                    if is_proto {
                        self.expr(v)?;
                        self.emit(Op::SetProtoLiteral);
                    } else if let Some(n) = self.prop_key(k)? {
                        let name = Self::static_key_name(k);
                        self.expr_with_name(v, name)?;
                        self.emit(Op::DefineProp(n));
                    } else {
                        self.push_key(k)?;
                        self.expr(v)?;
                        self.emit(Op::DefineElem);
                    }
                }
                ObjProp::Shorthand(n, _) | ObjProp::CoverInit(n, _) => {
                    self.get_var(n);
                    let id = self.name(n);
                    self.emit(Op::DefineProp(id));
                }
                ObjProp::Method(k, f) => {
                    self.push_key(k)?;
                    let idx = self.compile_function(f, Self::static_key_name(k))?;
                    self.emit(Op::Closure(idx));
                    self.emit(Op::DefineMethod(true));
                }
                ObjProp::Get(k, f) | ObjProp::Set(k, f) => {
                    let is_get = matches!(p, ObjProp::Get(..));
                    self.push_key(k)?;
                    let idx = self.compile_function(f, Self::static_key_name(k))?;
                    self.emit(Op::Closure(idx));
                    self.emit(Op::DefineAccessor(is_get, true));
                }
                ObjProp::Spread(e) => {
                    self.expr(e)?;
                    self.emit(Op::CopyDataProps);
                }
            }
        }
        Ok(())
    }

    pub fn class(&mut self, c: &Class) -> CResult<()> {
        self.class_named(c, None)
    }

    fn field_stmt(key: &PropName, value: Option<&Expr>, pos: Pos) -> Stmt {
        let this = Box::new(Expr::This);
        if let PropName::Private(n) = key {
            let v = value.cloned().unwrap_or(Expr::Unary(UnOp::Void, Box::new(Expr::Num(0.0)), pos));
            return Stmt { kind: StmtKind::Expr(Expr::PrivateDefine { name: n.clone(), value: Box::new(v), kind: 0 }), pos };
        }
        let target = match key {
            PropName::Ident(n) => Expr::Member { obj: this, name: n.clone(), optional: false, pos },
            PropName::Str(s) => Expr::Index { obj: this, index: Box::new(Expr::Str(s.clone())), optional: false, pos },
            PropName::Num(n) => Expr::Index { obj: this, index: Box::new(Expr::Num(*n)), optional: false, pos },
            PropName::Computed(e) => Expr::Index { obj: this, index: e.clone(), optional: false, pos },
            PropName::Private(_) => Expr::This,
        };
        let v = value.cloned().unwrap_or(Expr::Unary(UnOp::Void, Box::new(Expr::Num(0.0)), pos));
        Stmt { kind: StmtKind::Expr(Expr::Assign(AssignOp::Assign, Box::new(Pattern::Expr(target)), Box::new(v), pos)), pos }
    }

    pub fn class_named(&mut self, c: &Class, hint: Option<Str16>) -> CResult<()> {
        let name16 = c.name.as_ref().map(|n| s16(n)).or(hint);
        // the class scope: the class name, plus hidden bindings for private members — `#x`
        // holds the private name, `#x:m` / `#x:g` / `#x:s` an instance method / getter / setter
        let mut scope_names: Vec<(String, BKind)> = c.name.iter().map(|n| (n.clone(), BKind::Const)).collect();
        let mut private_names: Vec<String> = Vec::new();
        for m in &c.members {
            let PropName::Private(n) = &m.key else { continue };
            let key = format!("#{}", n);
            if !private_names.contains(&key) {
                private_names.push(key.clone());
                scope_names.push((key.clone(), BKind::Const));
            }
            let suffix = match (&m.kind, m.is_static) {
                (MemberKind::Method(_), false) => ":m",
                (MemberKind::Get(_), false) => ":g",
                (MemberKind::Set(_), false) => ":s",
                _ => continue,
            };
            scope_names.push((format!("{}{}", key, suffix), BKind::Const));
        }
        // a private name is declared once; only a getter and a setter may share one
        for key in &private_names {
            let (mut full, mut gets, mut sets, mut statics) = (0, 0, 0, 0);
            for m in &c.members {
                if !matches!(&m.key, PropName::Private(n) if key[1..] == **n) {
                    continue;
                }
                statics += m.is_static as u32;
                match &m.kind {
                    MemberKind::Get(_) => gets += 1,
                    MemberKind::Set(_) => sets += 1,
                    _ => full += 1,
                }
            }
            let total = full + gets + sets;
            if (full > 0 && total > 1) || gets > 1 || sets > 1 || (statics != 0 && statics != total) {
                return Err(self.err(&format!("Identifier '{}' has already been declared", key), c.pos));
            }
        }
        let scoped = !scope_names.is_empty();
        if scoped {
            self.push_scope(&scope_names);
        }
        for key in &private_names {
            self.emit(Op::NewPrivateName);
            self.init_var(key, VarKind::Const);
        }
        if let Some(e) = &c.extends {
            self.expr(e)?;
        }
        let default_ctor;
        let ctor: &Function = match &c.ctor {
            Some(f) => f,
            None => {
                let derived = c.extends.is_some();
                default_ctor = Function {
                    name: None,
                    self_binding: false,
                    params: Vec::new(),
                    rest: if derived { Some(Pattern::Ident("args".into(), c.pos)) } else { None },
                    body: if derived {
                        vec![Stmt {
                            kind: StmtKind::Expr(Expr::SuperCall(vec![ArrayItem::Spread(Expr::Ident("args".into(), c.pos))], c.pos)),
                            pos: c.pos,
                        }]
                    } else {
                        Vec::new()
                    },
                    kind: if derived { FuncKind::DerivedCtor } else { FuncKind::BaseCtor },
                    is_async: false,
                    is_gen: false,
                    strict: true,
                    simple_params: !derived,
                    pos: c.pos,
                    source: c.source,
                };
                &default_ctor
            }
        };
        let idx = self.compile_function(ctor, name16.clone())?;
        self.emit(Op::MakeClass(idx, c.extends.is_some()));
        let tproto = self.alloc_local();
        let tctor = self.alloc_local();
        self.emit(Op::InitLocal(tproto));
        self.emit(Op::InitLocal(tctor));
        if let Some(n) = &c.name {
            self.emit(Op::GetLocal(tctor));
            self.init_var(n, VarKind::Const);
        }
        let mut inst_fields = Vec::new();
        let mut inst_private = Vec::new();
        let mut statics = Vec::new();
        for m in &c.members {
            match &m.kind {
                MemberKind::Method(f) | MemberKind::Get(f) | MemberKind::Set(f) if matches!(m.key, PropName::Private(_)) => {
                    let PropName::Private(n) = &m.key else { continue };
                    let (kind, suffix) = match &m.kind {
                        MemberKind::Method(_) => (1u8, ":m"),
                        MemberKind::Get(_) => (2, ":g"),
                        _ => (3, ":s"),
                    };
                    if m.is_static {
                        self.emit(Op::GetLocal(tctor));
                        let id = self.get_private(n)?;
                        let fi = self.compile_function(f, Self::static_key_name(&m.key))?;
                        self.emit(Op::Closure(fi));
                        self.emit(Op::GetLocal(tctor));
                        self.emit(Op::SetHome);
                        self.emit(Op::PrivDefine(kind as u32, id));
                        self.emit(Op::Pop);
                    } else {
                        // one function object per class; every instance gets it at construction
                        let hidden = format!("#{}{}", n, suffix);
                        let fi = self.compile_function(f, Self::static_key_name(&m.key))?;
                        self.emit(Op::Closure(fi));
                        self.emit(Op::GetLocal(tproto));
                        self.emit(Op::SetHome);
                        self.init_var(&hidden, VarKind::Const);
                        let install = Expr::PrivateDefine { name: n.clone(), value: Box::new(Expr::Ident(hidden, c.pos)), kind };
                        inst_private.push(Stmt { kind: StmtKind::Expr(install), pos: c.pos });
                    }
                }
                MemberKind::Method(f) | MemberKind::Get(f) | MemberKind::Set(f) => {
                    self.emit(Op::GetLocal(if m.is_static { tctor } else { tproto }));
                    self.push_key(&m.key)?;
                    let fi = self.compile_function(f, Self::static_key_name(&m.key))?;
                    self.emit(Op::Closure(fi));
                    self.emit(match &m.kind {
                        MemberKind::Method(_) => Op::DefineMethod(false),
                        MemberKind::Get(_) => Op::DefineAccessor(true, false),
                        _ => Op::DefineAccessor(false, false),
                    });
                    self.emit(Op::Pop);
                }
                MemberKind::Field(v) => {
                    let s = Self::field_stmt(&m.key, v.as_ref(), c.pos);
                    if m.is_static {
                        statics.push(s);
                    } else {
                        inst_fields.push(s);
                    }
                }
                MemberKind::StaticBlock(b) => statics.push(Stmt { kind: StmtKind::Block(b.clone()), pos: c.pos }),
            }
        }
        // private methods are installed before any field initialiser runs
        inst_private.append(&mut inst_fields);
        let inst_fields = inst_private;
        if !inst_fields.is_empty() {
            let fi = self.compile_synthetic(&inst_fields, FuncKind::ClassInit, c.pos)?;
            self.emit(Op::GetLocal(tctor));
            self.emit(Op::Closure(fi));
            self.emit(Op::GetLocal(tproto));
            self.emit(Op::SetHome);
            self.emit(Op::SetFieldInit);
        }
        if !statics.is_empty() {
            let fi = self.compile_synthetic(&statics, FuncKind::Method, c.pos)?;
            self.emit(Op::GetLocal(tctor));
            self.emit(Op::Closure(fi));
            self.emit(Op::GetLocal(tctor));
            self.emit(Op::SetHome);
            self.emit(Op::CallMethod(0));
            self.emit(Op::Pop);
        }
        self.emit(Op::GetLocal(tctor));
        if scoped {
            self.pop_scope();
        }
        Ok(())
    }
}
