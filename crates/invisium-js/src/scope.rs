//! Name-addressed scopes: direct `eval`, `with` objects and eval-declared variables. Only
//! functions compiled with such a construct in reach execute these ops.

use crate::bytecode::{DynBase, DynRef, Op};
use crate::runtime::*;
use crate::value::*;
use std::rc::Rc;

impl Runtime {
    /// Returns true when a new frame was pushed.
    pub(crate) fn exec_scope_op(&mut self, op: Op, fi: usize, atoms: &[Atom], strict: bool) -> JsResult<bool> {
        let atom = |n: u32| atoms.get(n as usize).copied().unwrap_or(A_EMPTY);
        match op {
            Op::DirectEval(argc, idx) => return self.op_direct_eval(argc, idx, fi),
            Op::GetImport(hops, slot) => {
                let v = match self.env_slot(fi, hops, slot) {
                    Some(Value::Object(o)) => match self.obj(o).kind {
                        ObjKind::ImportCell(e, s) => self.env(e).and_then(|e| e.slots.get(s as usize).copied().flatten()),
                        _ => Some(Value::Object(o)),
                    },
                    other => other,
                };
                match v {
                    Some(v) => self.stack.push(v),
                    None => return Err(self.tdz_error()),
                }
            }
            Op::BigInt(n) => {
                // parsed once per runtime, then shared (BigInts are immutable)
                let a = atom(n);
                let v = match self.big_literals.get(&a) {
                    Some(b) => Value::BigInt(*b),
                    None => {
                        let text = String::from_utf16_lossy(self.atom_units(a));
                        let b = self.big_parse(&text)?.unwrap_or_default();
                        let v = self.big_value(b)?;
                        if let Value::BigInt(id) = v {
                            self.big_literals.insert(a, id);
                        }
                        v
                    }
                };
                self.stack.push(v);
            }
            Op::ToObject => {
                let v = self.peek(0);
                if !matches!(v, Value::Object(_)) {
                    let o = self.to_object(v)?;
                    if let Some(t) = self.stack.last_mut() {
                        *t = Value::Object(o);
                    }
                }
            }
            Op::DeclareEvalVar(n, hops, slot) => {
                let env = self.env_at(self.frames[fi].env, hops);
                let cur = env.and_then(|e| self.env(e)).and_then(|e| e.slots.get(slot as usize).copied().flatten());
                let vars = match cur {
                    Some(Value::Object(o)) => o,
                    _ => {
                        let o = self.new_object(None);
                        if let Some(s) = env.and_then(|e| self.env_mut(e)).and_then(|e| e.slots.get_mut(slot as usize)) {
                            *s = Some(Value::Object(o));
                        }
                        o
                    }
                };
                let k = PropKey::Str(atom(n));
                if self.obj(vars).props.get(k).is_none() {
                    self.define_value(vars, k, Value::Undefined, DEFAULT_FLAGS);
                }
            }
            Op::DynGet(i) | Op::DynTypeof(i) | Op::DynCallee(i) => {
                let code = self.frames[fi].code.clone();
                let Some(d) = code.dyn_refs.get(i as usize) else { return Err(self.type_error("invalid scope reference")) };
                let a = atom(d.name);
                let (this, v) = match self.dyn_find(fi, d, a)? {
                    Some((o, is_with)) => {
                        let v = self.get_prop(Value::Object(o), PropKey::Str(a))?;
                        (if is_with { Value::Object(o) } else { Value::Undefined }, v)
                    }
                    None => (Value::Undefined, self.dyn_base_get(fi, d, a, matches!(op, Op::DynTypeof(_)))?),
                };
                if matches!(op, Op::DynCallee(_)) {
                    self.stack.push(this);
                }
                self.stack.push(v);
            }
            Op::DynPut(i) => {
                let code = self.frames[fi].code.clone();
                let Some(d) = code.dyn_refs.get(i as usize) else { return Err(self.type_error("invalid scope reference")) };
                let a = atom(d.name);
                let v = self.peek(0);
                match self.dyn_find(fi, d, a)? {
                    Some((o, _)) => {
                        let ok = self.set_prop(Value::Object(o), PropKey::Str(a), v, Value::Object(o))?;
                        if !ok && strict {
                            return Err(self.type_error("Cannot assign to read only property of object"));
                        }
                    }
                    None => self.dyn_base_put(fi, d, a, v, strict)?,
                }
                self.pop();
            }
            Op::DynDelete(i) => {
                let code = self.frames[fi].code.clone();
                let Some(d) = code.dyn_refs.get(i as usize) else { return Err(self.type_error("invalid scope reference")) };
                let a = atom(d.name);
                let r = match self.dyn_find(fi, d, a)? {
                    Some((o, _)) => self.delete_prop_ex(o, PropKey::Str(a))?,
                    None => match d.base {
                        DynBase::Global => {
                            let g = self.global;
                            self.delete_prop(g, PropKey::Str(a))
                        }
                        _ => false,
                    },
                };
                self.stack.push(Value::Bool(r));
            }
            other => self.exec_private_op(other, atoms)?,
        }
        Ok(false)
    }

    fn env_slot(&self, fi: usize, hops: u32, slot: u32) -> Option<Value> {
        let e = self.env_at(self.frames[fi].env, hops)?;
        self.env(e)?.slots.get(slot as usize).copied().flatten()
    }

    /// The innermost scope object that has `name`: (object, is a `with` object).
    fn dyn_find(&mut self, fi: usize, d: &DynRef, name: Atom) -> JsResult<Option<(ObjId, bool)>> {
        let key = PropKey::Str(name);
        for &(hops, slot, is_with) in &d.objs {
            let Some(Value::Object(o)) = self.env_slot(fi, hops, slot) else { continue };
            if !self.has_property(o, key)? {
                continue;
            }
            if is_with {
                let u = self.get_prop(Value::Object(o), PropKey::Sym(SYM_UNSCOPABLES))?;
                if matches!(u, Value::Object(_)) {
                    let blocked = self.get_prop(u, key)?;
                    if self.truthy(blocked) {
                        continue;
                    }
                }
            }
            return Ok(Some((o, is_with)));
        }
        Ok(None)
    }

    fn dyn_base_get(&mut self, fi: usize, d: &DynRef, name: Atom, typeof_mode: bool) -> JsResult<Value> {
        let v = match d.base {
            DynBase::Local(i) => self.locals.get(self.frames[fi].locals_base + i as usize).copied().flatten(),
            DynBase::Env(h, s) => self.env_slot(fi, h, s),
            DynBase::Global => return self.get_global(name, typeof_mode),
        };
        match v {
            Some(v) => Ok(v),
            None => Err(self.tdz_error()),
        }
    }

    fn dyn_base_put(&mut self, fi: usize, d: &DynRef, name: Atom, v: Value, strict: bool) -> JsResult<()> {
        if d.is_const {
            return Err(self.type_error("Assignment to constant variable."));
        }
        let slot = match d.base {
            DynBase::Local(i) => {
                let at = self.frames[fi].locals_base + i as usize;
                self.locals.get_mut(at)
            }
            DynBase::Env(h, s) => {
                let e = self.env_at(self.frames[fi].env, h);
                e.and_then(|e| self.env_mut(e)).and_then(|e| e.slots.get_mut(s as usize))
            }
            DynBase::Global => return self.put_global(name, v, strict),
        };
        match slot {
            Some(slot @ Some(_)) => {
                *slot = Some(v);
                Ok(())
            }
            _ => Err(self.tdz_error()),
        }
    }

    /// A call written `eval(...)`: when the callee is the built-in `eval` and the argument a
    /// string, compiles it against the call site's scope and runs it as a frame of its own.
    fn op_direct_eval(&mut self, argc: u32, idx: u32, fi: usize) -> JsResult<bool> {
        let fpos = self.stack.len().saturating_sub(argc as usize + 1);
        let f = self.stack.get(fpos).copied().unwrap_or(Value::Undefined);
        if f.as_obj().is_none() || f.as_obj() != self.intr.eval_fn {
            return self.invoke(f, Value::Undefined, fpos + 1, fpos);
        }
        let x = if argc > 0 { self.stack.get(fpos + 1).copied().unwrap_or(Value::Undefined) } else { Value::Undefined };
        let Value::String(s) = x else {
            self.stack.truncate(fpos);
            self.stack.push(x);
            return Ok(false);
        };
        self.safepoint()?;
        let src = self.rust_string(s);
        self.charge_compile(src.len())?;
        let code = self.frames[fi].code.clone();
        let Some(scope) = code.eval_scopes.get(idx as usize).cloned() else { return Err(self.type_error("invalid eval site")) };
        let script = match crate::compile_eval_source(&src, scope) {
            Ok(s) => s,
            Err(e) => return Err(Exc::Throw(self.make_error("SyntaxError", &e.message))),
        };
        let atoms: Rc<[Atom]> = script.names.iter().map(|n| self.intern16(n)).collect::<Vec<Atom>>().into();
        // the retained source text is heap the script is responsible for
        self.grow(script.top.source.len() * 4);
        let fr = &self.frames[fi];
        let c = Closure { code: script.top, atoms, env: fr.env, this_val: fr.this, new_target: fr.new_target, home: fr.home, ctor_callee: fr.ctor_callee, fields: None };
        let fp = self.intr.function_proto;
        let fobj = self.alloc(Object::new(fp, ObjKind::Function(Box::new(FuncData::Closure(c)))));
        // the closure replaces `eval` and its arguments on the stack, which keeps it alive
        self.stack.truncate(fpos);
        self.stack.push(Value::Object(fobj));
        self.push_frame(fobj, Value::Undefined, Value::Undefined, fpos + 1, fpos, false)?;
        Ok(true)
    }
}
