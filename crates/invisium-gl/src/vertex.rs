//! Vertex attribute state and vertex array objects.

use crate::args::*;
use crate::cmd::ConstValue;
use crate::consts1::*;
use crate::consts2::*;
use crate::progobj::{AttribState, Vao};
use crate::{Arg, Context, Elem, Kind, Ret};

pub fn attrib_type_size(ty: u32) -> u32 {
    match ty {
        BYTE | UNSIGNED_BYTE => 1,
        SHORT | UNSIGNED_SHORT | HALF_FLOAT => 2,
        _ => 4,
    }
}

impl Context {
    pub(crate) fn attribs(&self) -> &[AttribState] {
        &self.objs.vaos[&self.st.vao].attribs
    }

    fn attrib_mut(&mut self, index: u32) -> R<&mut AttribState> {
        let vao = self.st.vao;
        self.objs.vaos.get_mut(&vao).and_then(|v| v.attribs.get_mut(index as usize)).ok_or(INVALID_VALUE)
    }

    /// Bind vertex array `h` (0 = the default one), swapping the element-array binding.
    fn bind_vao(&mut self, h: u32) {
        let old = self.st.vao;
        if let Some(v) = self.objs.vaos.get_mut(&old) {
            v.element = self.st.element_buffer;
        }
        self.st.vao = h;
        self.st.element_buffer = self.objs.vaos.get(&h).map(|v| v.element).unwrap_or(0);
    }

    /// Forget a deleted buffer in every vertex array.
    pub(crate) fn unbind_buffer_from_vaos(&mut self, h: u32) {
        for v in self.objs.vaos.values_mut() {
            if v.element == h {
                v.element = 0;
            }
            for a in v.attribs.iter_mut().filter(|a| a.buffer == h) {
                a.buffer = 0;
            }
        }
    }

    pub(crate) fn vertex_call(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        match name {
            "createVertexArray" => {
                let h = self.new_handle();
                self.objs.vaos.insert(h, Vao::new());
                Ok(Ret::Object(Kind::VertexArray, h))
            }
            "isVertexArray" => Ok(Ret::Bool(matches!(obj(a, 0, Kind::VertexArray), Ok(Some(h)) if h != 0 && self.objs.vaos.get(&h).is_some_and(|v| v.bound_once)))),
            "deleteVertexArray" => {
                let Some(h) = obj(a, 0, Kind::VertexArray)? else { return Ok(Ret::Undefined) };
                if h != 0 && self.objs.vaos.contains_key(&h) {
                    if self.st.vao == h {
                        self.bind_vao(0);
                    }
                    self.objs.vaos.remove(&h);
                }
                Ok(Ret::Undefined)
            }
            "bindVertexArray" => {
                let h = obj(a, 0, Kind::VertexArray)?.unwrap_or(0);
                let v = self.objs.vaos.get_mut(&h).ok_or(INVALID_OPERATION)?;
                v.bound_once = true;
                self.bind_vao(h);
                Ok(Ret::Undefined)
            }
            "enableVertexAttribArray" | "disableVertexAttribArray" => {
                let on = name.starts_with("enable");
                self.attrib_mut(uint(a, 0)?)?.enabled = on;
                Ok(Ret::Undefined)
            }
            "vertexAttribPointer" | "vertexAttribIPointer" => self.attrib_pointer(name == "vertexAttribIPointer", a),
            "vertexAttribDivisor" => {
                let (i, d) = (uint(a, 0)?, uint(a, 1)?);
                self.attrib_mut(i)?.divisor = d;
                Ok(Ret::Undefined)
            }
            "getVertexAttribOffset" => {
                let i = uint(a, 0)?;
                if uint(a, 1)? != VERTEX_ATTRIB_ARRAY_POINTER {
                    return Err(INVALID_ENUM);
                }
                Ok(Ret::Num(self.attribs().get(i as usize).ok_or(INVALID_VALUE)?.offset as f64))
            }
            "getVertexAttrib" => self.get_vertex_attrib(uint(a, 0)?, uint(a, 1)?),
            _ => self.attrib_value(name, a),
        }
    }

    fn attrib_pointer(&mut self, integer: bool, a: &[Arg]) -> R<Ret> {
        let (index, size, ty) = (uint(a, 0)?, int(a, 1)?, uint(a, 2)?);
        let (normalized, si) = if integer { (false, 3) } else { (boolean(a, 3)?, 4) };
        let (stride, offset) = (int(a, si)?, long(a, si + 1)?);
        let webgl2 = self.is2();
        let ok_type = match ty {
            BYTE | UNSIGNED_BYTE | SHORT | UNSIGNED_SHORT => true,
            FLOAT | HALF_FLOAT | INT_2_10_10_10_REV | UNSIGNED_INT_2_10_10_10_REV => !integer && (ty == FLOAT || webgl2),
            INT | UNSIGNED_INT => webgl2,
            _ => false,
        };
        if !ok_type {
            return Err(INVALID_ENUM);
        }
        if index >= crate::limits::MAX_VERTEX_ATTRIBS || !(1..=4).contains(&size) || !(0..=255).contains(&stride) || offset < 0 {
            return Err(INVALID_VALUE);
        }
        if matches!(ty, INT_2_10_10_10_REV | UNSIGNED_INT_2_10_10_10_REV) && size != 4 {
            return Err(INVALID_OPERATION);
        }
        let ts = attrib_type_size(ty) as i64;
        if offset % ts != 0 || stride as i64 % ts != 0 {
            return Err(INVALID_OPERATION);
        }
        let buffer = self.st.array_buffer;
        if buffer == 0 && offset != 0 {
            return Err(INVALID_OPERATION);
        }
        let at = self.attrib_mut(index)?;
        *at = AttribState { buffer, size: size as u8, ty, normalized, integer, stride: stride as u32, offset: offset as u64, ..*at };
        Ok(Ret::Undefined)
    }

    fn attrib_value(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        let index = uint(a, 0)?;
        let v = name.ends_with('v');
        let (kind, n) = match name {
            "vertexAttribI4i" | "vertexAttribI4iv" => ('i', 4),
            "vertexAttribI4ui" | "vertexAttribI4uiv" => ('u', 4),
            _ => ('f', name.as_bytes().get(12).map(|c| c.wrapping_sub(b'0') as usize).filter(|n| (1..=4).contains(n)).ok_or(INVALID_OPERATION)?),
        };
        let vals: Vec<f64> = if v {
            let elem = match kind {
                'f' => Elem::F32,
                'i' => Elem::I32,
                _ => Elem::U32,
            };
            let all = nums(a, 1, elem)?;
            if all.len() < n {
                return Err(INVALID_VALUE);
            }
            all[..n].to_vec()
        } else {
            (0..n).map(|i| num(a, 1 + i)).collect::<R<_>>()?
        };
        if index >= crate::limits::MAX_VERTEX_ATTRIBS {
            return Err(INVALID_VALUE);
        }
        let mut f = [0.0, 0.0, 0.0, 1.0];
        f[..n].copy_from_slice(&vals);
        self.st.attrib_values[index as usize] = match kind {
            'f' => ConstValue::F(f.map(|x| x as f32)),
            'i' => ConstValue::I(f.map(|x| if x.is_finite() { x.trunc().rem_euclid(4294967296.0) as u32 as i32 } else { 0 })),
            _ => ConstValue::U(f.map(|x| if x.is_finite() { x.trunc().rem_euclid(4294967296.0) as u32 } else { 0 })),
        };
        Ok(Ret::Undefined)
    }

    fn get_vertex_attrib(&self, index: u32, pname: u32) -> R<Ret> {
        if index >= crate::limits::MAX_VERTEX_ATTRIBS {
            return Err(INVALID_VALUE);
        }
        let at = self.attribs()[index as usize];
        let webgl2 = self.is2();
        Ok(match pname {
            CURRENT_VERTEX_ATTRIB => match self.st.attrib_values[index as usize] {
                ConstValue::F(v) => Ret::F32s(v.to_vec()),
                ConstValue::I(v) => Ret::I32s(v.to_vec()),
                ConstValue::U(v) => Ret::U32s(v.to_vec()),
            },
            VERTEX_ATTRIB_ARRAY_BUFFER_BINDING => if at.buffer == 0 { Ret::Null } else { Ret::Object(Kind::Buffer, at.buffer) },
            VERTEX_ATTRIB_ARRAY_ENABLED => Ret::Bool(at.enabled),
            VERTEX_ATTRIB_ARRAY_SIZE => Ret::Num(at.size as f64),
            VERTEX_ATTRIB_ARRAY_STRIDE => Ret::Num(at.stride as f64),
            VERTEX_ATTRIB_ARRAY_TYPE => Ret::Num(at.ty as f64),
            VERTEX_ATTRIB_ARRAY_NORMALIZED => Ret::Bool(at.normalized),
            VERTEX_ATTRIB_ARRAY_INTEGER if webgl2 => Ret::Bool(at.integer),
            VERTEX_ATTRIB_ARRAY_DIVISOR if webgl2 || self.ext("ANGLE_instanced_arrays") => Ret::Num(at.divisor as f64),
            _ => return Err(INVALID_ENUM),
        })
    }
}
