//! Program introspection: active attributes and uniforms, locations, uniform blocks.

use crate::args::*;
use crate::consts1::*;
use crate::consts2::*;
use crate::progobj::Loc;
use crate::{Arg, Context, Elem, Kind, Ret};
use invisium_glsl::Place;

/// Split `name[3]` into (`name`, 3).
fn split_index(name: &str) -> Option<(&str, u32)> {
    let base = name.strip_suffix(']')?;
    let open = base.rfind('[')?;
    let idx = base[open + 1..].parse().ok()?;
    Some((&base[..open], idx))
}

impl Context {
    /// Resolve a uniform name of a linked program to (uniform index, array element).
    fn find_uniform(&self, h: u32, name: &str) -> R<Option<(usize, u32)>> {
        let l = self.linked(h)?;
        if name.starts_with("gl_") || name.starts_with("webgl_") || name.starts_with("_webgl_") {
            return Ok(None);
        }
        let us = &l.prog.uniforms;
        let found = if let Some(i) = us.iter().position(|u| u.name == name) {
            Some((i, 0))
        } else if let Some(i) = us.iter().position(|u| u.name.strip_suffix("[0]") == Some(name)) {
            Some((i, 0))
        } else if let Some((base, idx)) = split_index(name) {
            us.iter().position(|u| u.name.strip_suffix("[0]") == Some(base) && idx < u.size).map(|i| (i, idx))
        } else {
            None
        };
        Ok(found.filter(|(i, _)| !matches!(us[*i].place, Place::Named { .. })))
    }

    /// The location object of (program, uniform, element), created once.
    fn location(&mut self, l: Loc) -> u32 {
        if let Some(&h) = self.objs.loc_cache.get(&l) {
            return h;
        }
        let h = self.new_handle();
        self.objs.loc_cache.insert(l, h);
        self.objs.locations.insert(h, l);
        h
    }

    pub(crate) fn introspect_call(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        let p = self.program_arg(a, 0)?;
        match name {
            "getAttribLocation" | "getFragDataLocation" => {
                let Arg::Str(n) = arg(a, 1) else { return Err(INVALID_OPERATION) };
                let l = self.linked(p)?;
                let loc = if name == "getAttribLocation" {
                    l.prog.attributes.iter().find(|x| x.name == n).map(|x| x.location)
                } else {
                    l.prog.outputs.iter().find(|x| x.name == n).map(|x| x.location)
                };
                Ok(Ret::Num(loc.map(|v| v as f64).unwrap_or(-1.0)))
            }
            "getActiveAttrib" | "getActiveUniform" => {
                let i = uint(a, 1)? as usize;
                let Some(l) = self.objs.programs[&p].exe.as_ref().filter(|_| self.objs.programs[&p].link_status) else {
                    return Err(INVALID_VALUE);
                };
                if name == "getActiveAttrib" {
                    let x = l.prog.attributes.get(i).ok_or(INVALID_VALUE)?;
                    Ok(Ret::ActiveInfo { name: x.name.clone(), size: 1, gl_type: x.gl_type })
                } else {
                    let x = l.prog.uniforms.get(i).ok_or(INVALID_VALUE)?;
                    Ok(Ret::ActiveInfo { name: x.name.clone(), size: x.size as i32, gl_type: x.gl_type })
                }
            }
            "getUniformLocation" => {
                let Arg::Str(n) = arg(a, 1) else { return Err(INVALID_OPERATION) };
                match self.find_uniform(p, n)? {
                    Some((uniform, elem)) => Ok(Ret::Object(Kind::UniformLocation, self.location(Loc { program: p, uniform, elem }))),
                    None => Ok(Ret::Null),
                }
            }
            "getUniformBlockIndex" => {
                let Arg::Str(n) = arg(a, 1) else { return Err(INVALID_OPERATION) };
                let l = self.linked(p)?;
                Ok(Ret::Num(l.prog.blocks.iter().position(|b| b.name == n).map(|i| i as u32).unwrap_or(INVALID_INDEX) as f64))
            }
            "getActiveUniformBlockName" => {
                let l = self.linked(p)?;
                Ok(Ret::Str(l.prog.blocks.get(uint(a, 1)? as usize).ok_or(INVALID_VALUE)?.name.clone()))
            }
            "getActiveUniformBlockParameter" => {
                let (i, pname) = (uint(a, 1)? as usize, uint(a, 2)?);
                let l = self.linked(p)?;
                let b = l.prog.blocks.get(i).ok_or(INVALID_VALUE)?;
                let members: Vec<u32> = l.prog.uniforms.iter().enumerate().filter(|(_, u)| matches!(u.place, Place::Named { block, .. } if block as usize == i)).map(|(k, _)| k as u32).collect();
                Ok(match pname {
                    UNIFORM_BLOCK_BINDING => Ret::Num(l.block_bindings[i] as f64),
                    UNIFORM_BLOCK_DATA_SIZE => Ret::Num(b.slot.bytes as f64),
                    UNIFORM_BLOCK_ACTIVE_UNIFORMS => Ret::Num(members.len() as f64),
                    UNIFORM_BLOCK_ACTIVE_UNIFORM_INDICES => Ret::U32s(members),
                    UNIFORM_BLOCK_REFERENCED_BY_VERTEX_SHADER => Ret::Bool(b.in_vertex),
                    UNIFORM_BLOCK_REFERENCED_BY_FRAGMENT_SHADER => Ret::Bool(b.in_fragment),
                    _ => return Err(INVALID_ENUM),
                })
            }
            "uniformBlockBinding" => {
                let (i, binding) = (uint(a, 1)? as usize, uint(a, 2)?);
                self.linked(p)?;
                if binding >= crate::limits::MAX_UNIFORM_BUFFER_BINDINGS {
                    return Err(INVALID_VALUE);
                }
                let l = self.objs.programs.get_mut(&p).and_then(|x| x.exe.as_mut()).ok_or(INVALID_OPERATION)?;
                *l.block_bindings.get_mut(i).ok_or(INVALID_VALUE)? = binding;
                Ok(Ret::Undefined)
            }
            "getUniformIndices" => {
                // The binding hands a sequence<DOMString> over as one string per call.
                let names: Vec<&str> = match arg(a, 1) {
                    Arg::Str(s) => vec![s],
                    _ => return Err(INVALID_OPERATION),
                };
                let l = self.linked(p)?;
                let idx = names
                    .iter()
                    .map(|n| l.prog.uniforms.iter().position(|u| u.name == *n || u.name.strip_suffix("[0]") == Some(n)).map(|i| i as u32).unwrap_or(INVALID_INDEX))
                    .collect();
                Ok(Ret::U32s(idx))
            }
            "getActiveUniforms" => {
                let idx: Vec<usize> = nums(a, 1, Elem::U32)?.iter().map(|&v| v as usize).collect();
                let pname = uint(a, 2)?;
                let l = self.linked(p)?;
                let mut us = Vec::new();
                for i in idx {
                    us.push(l.prog.uniforms.get(i).ok_or(INVALID_VALUE)?);
                }
                let named = |u: &invisium_glsl::Uniform| match u.place {
                    Place::Named { block, offset } => Some((block as i32, offset as i32)),
                    _ => None,
                };
                Ok(match pname {
                    UNIFORM_TYPE => Ret::U32s(us.iter().map(|u| u.gl_type).collect()),
                    UNIFORM_SIZE => Ret::U32s(us.iter().map(|u| u.size).collect()),
                    UNIFORM_BLOCK_INDEX => Ret::I32s(us.iter().map(|u| named(u).map(|n| n.0).unwrap_or(-1)).collect()),
                    UNIFORM_OFFSET => Ret::I32s(us.iter().map(|u| named(u).map(|n| n.1).unwrap_or(-1)).collect()),
                    UNIFORM_ARRAY_STRIDE | UNIFORM_MATRIX_STRIDE => Ret::I32s(us.iter().map(|_| -1).collect()),
                    UNIFORM_IS_ROW_MAJOR => Ret::Bools(us.iter().map(|_| false).collect()),
                    _ => return Err(INVALID_ENUM),
                })
            }
            _ => Ok(Ret::Undefined),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn split() {
        assert_eq!(super::split_index("a[3]"), Some(("a", 3)));
        assert_eq!(super::split_index("l[1].b[2]"), Some(("l[1].b", 2)));
        assert_eq!(super::split_index("a"), None);
    }
}
