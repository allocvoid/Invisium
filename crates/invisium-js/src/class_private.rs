//! Private class members. A private name `#x` is a key no script value can express: each
//! evaluation of a class body mints one (a symbol id with the private bit set, held in a
//! hidden binding of the class scope), and members live as own properties under that key.
//! Property enumeration skips such keys, and access never consults prototypes, proxies or
//! host hooks, so having the key is the brand check.

use crate::bytecode::Op;
use crate::runtime::*;
use crate::value::*;

pub const PRIVATE_SYM_BIT: u32 = 0x8000_0000;

impl PropKey {
    pub(crate) fn is_private(self) -> bool {
        matches!(self, PropKey::Sym(s) if s.0 & PRIVATE_SYM_BIT != 0)
    }
}

impl Runtime {
    fn private_prop(&self, obj: Value, key: Value) -> Option<(ObjId, PropKey, Option<Prop>)> {
        match (obj, key) {
            (Value::Object(o), Value::Symbol(s)) => Some((o, PropKey::Sym(s), self.obj(o).props.get(PropKey::Sym(s)).copied())),
            _ => None,
        }
    }

    pub(crate) fn exec_private_op(&mut self, op: Op, atoms: &[Atom]) -> JsResult<()> {
        let name = |rt: &Runtime, n: u32| String::from_utf16_lossy(rt.atom_units(atoms.get(n as usize).copied().unwrap_or(A_EMPTY)));
        match op {
            Op::NewPrivateName => {
                if self.next_private >= PRIVATE_SYM_BIT - 1 {
                    return Err(self.range_error("Too many private names"));
                }
                self.next_private += 1;
                self.stack.push(Value::Symbol(SymId(PRIVATE_SYM_BIT | self.next_private)));
            }
            Op::PrivGet(n) | Op::PrivGetKeep(n) => {
                let (obj, key) = (self.peek(1), self.peek(0));
                let Some((_, _, Some(p))) = self.private_prop(obj, key) else {
                    let n = name(self, n);
                    return Err(self.type_error(&format!("Cannot read private member {} from an object whose class did not declare it", n)));
                };
                let v = if p.is_accessor() {
                    if p.value == Value::Undefined {
                        let n = name(self, n);
                        return Err(self.type_error(&format!("'{}' was defined without a getter", n)));
                    }
                    self.call_value(p.value, obj, &[])?
                } else {
                    p.value
                };
                self.pop();
                if matches!(op, Op::PrivGet(_)) {
                    self.pop();
                }
                self.stack.push(v);
            }
            Op::PrivPut(n) => {
                let (obj, key, v) = (self.peek(2), self.peek(1), self.peek(0));
                let Some((o, k, Some(p))) = self.private_prop(obj, key) else {
                    let n = name(self, n);
                    return Err(self.type_error(&format!("Cannot write private member {} to an object whose class did not declare it", n)));
                };
                if p.is_accessor() {
                    if p.setter == Value::Undefined {
                        let n = name(self, n);
                        return Err(self.type_error(&format!("'{}' was defined without a setter", n)));
                    }
                    self.call_value(p.setter, obj, &[v])?;
                } else if p.flags & WRITABLE == 0 {
                    let n = name(self, n);
                    return Err(self.type_error(&format!("Private method '{}' is not writable", n)));
                } else if let Some(slot) = self.obj_mut(o).props.get_mut(k) {
                    slot.value = v;
                }
                let l = self.stack.len();
                self.stack.truncate(l.saturating_sub(3));
                self.stack.push(v);
            }
            Op::PrivIn => {
                let (key, obj) = (self.peek(1), self.peek(0));
                let Some((_, _, p)) = self.private_prop(obj, key) else {
                    return Err(self.type_error("Cannot use 'in' operator to search for a private field in a non-object"));
                };
                let l = self.stack.len();
                self.stack.truncate(l.saturating_sub(2));
                self.stack.push(Value::Bool(p.is_some()));
            }
            Op::PrivDefine(kind, n) => {
                let (obj, key, v) = (self.peek(2), self.peek(1), self.peek(0));
                let Some((o, k, existing)) = self.private_prop(obj, key) else {
                    return Err(self.type_error("Cannot define a private member on a non-object"));
                };
                let prop = match (kind, existing) {
                    (2, Some(e)) if e.is_accessor() && e.value == Value::Undefined => Prop { value: v, ..e },
                    (3, Some(e)) if e.is_accessor() && e.setter == Value::Undefined => Prop { setter: v, ..e },
                    (_, Some(_)) => {
                        let n = name(self, n);
                        return Err(self.type_error(&format!("Cannot initialize {} twice on the same object", n)));
                    }
                    (2, None) => Prop { value: v, setter: Value::Undefined, flags: ACCESSOR },
                    (3, None) => Prop { value: Value::Undefined, setter: v, flags: ACCESSOR },
                    (0, None) => Prop::data(v, WRITABLE),
                    (_, None) => Prop::data(v, 0),
                };
                if self.obj_mut(o).props.insert(k, prop) {
                    self.grow(48);
                }
                let l = self.stack.len();
                self.stack.truncate(l.saturating_sub(2));
            }
            _ => {}
        }
        Ok(())
    }
}
