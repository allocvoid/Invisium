//! Shader and program objects: compile, link, introspection.

use crate::args::*;
use crate::cmd::{AttrSlot, Cmd, OutSlot, ProgramDesc};
use crate::consts1::*;
use crate::consts2::*;
use crate::progobj::*;
use crate::{Arg, Context, Kind, Ret};
use invisium_glsl::Stage;

/// The device's view of a linked program.
pub fn program_desc(p: &invisium_glsl::Program) -> ProgramDesc {
    ProgramDesc {
        vertex_wgsl: p.vertex_wgsl.clone(),
        fragment_wgsl: p.fragment_wgsl.clone(),
        uniform_bytes: p.uniform_bytes,
        blocks: p.blocks.iter().map(|b| b.slot).collect(),
        textures: p.textures.clone(),
        attributes: p.attributes.iter().flat_map(|a| (0..a.columns.max(1)).map(move |c| AttrSlot { location: a.location + c, base: a.base })).collect(),
        outputs: p.outputs.iter().map(|o| OutSlot { location: o.location, base: o.base }).collect(),
        writes_depth: p.writes_depth,
    }
}

fn reserved_name(name: &str) -> bool {
    name.starts_with("gl_") || name.starts_with("webgl_") || name.starts_with("_webgl_")
}

impl Context {
    /// A live shader handle (missing or deleted: `INVALID_VALUE`).
    pub(crate) fn shader_arg(&self, a: &[Arg], i: usize) -> R<u32> {
        match obj(a, i, Kind::Shader)? {
            Some(h) if self.objs.shaders.get(&h).is_some_and(|s| !s.delete_pending) => Ok(h),
            _ => Err(INVALID_VALUE),
        }
    }

    /// A live program handle (missing or deleted: `INVALID_VALUE`).
    pub(crate) fn program_arg(&self, a: &[Arg], i: usize) -> R<u32> {
        match obj(a, i, Kind::Program)? {
            Some(h) if self.objs.programs.get(&h).is_some_and(|p| !p.delete_pending) => Ok(h),
            _ => Err(INVALID_VALUE),
        }
    }

    /// The translator program of a successfully linked program (`INVALID_OPERATION` if not).
    pub(crate) fn linked(&self, h: u32) -> R<&Linked> {
        let p = self.objs.programs.get(&h).ok_or(INVALID_VALUE)?;
        if !p.link_status {
            return Err(INVALID_OPERATION);
        }
        p.exe.as_ref().ok_or(INVALID_OPERATION)
    }

    fn drop_locations(&mut self, h: u32) {
        self.objs.locations.retain(|_, l| l.program != h);
        self.objs.loc_cache.retain(|l, _| l.program != h);
    }

    fn release_shader(&mut self, s: u32) {
        if let Some(sh) = self.objs.shaders.get_mut(&s) {
            sh.attached = sh.attached.saturating_sub(1);
            if sh.delete_pending && sh.attached == 0 {
                self.objs.shaders.remove(&s);
            }
        }
    }

    /// Free a program for good (it is not current).
    pub(crate) fn free_program(&mut self, h: u32) {
        let Some(p) = self.objs.programs.remove(&h) else { return };
        for s in p.shaders {
            self.release_shader(s);
        }
        self.drop_locations(h);
        if p.on_dev {
            self.push(Cmd::ProgramDelete { id: h });
        }
    }

    pub(crate) fn program_call(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        match name {
            "createShader" => {
                let ty = uint(a, 0)?;
                if ty != VERTEX_SHADER && ty != FRAGMENT_SHADER {
                    return Err(INVALID_ENUM);
                }
                let h = self.new_handle();
                let s = Shader { ty, source: String::new(), compiled: None, compile_status: false, log: String::new(), delete_pending: false, attached: 0 };
                self.objs.shaders.insert(h, s);
                Ok(Ret::Object(Kind::Shader, h))
            }
            "isShader" => Ok(Ret::Bool(matches!(obj(a, 0, Kind::Shader), Ok(Some(h)) if self.objs.shaders.get(&h).is_some_and(|s| !s.delete_pending)))),
            "deleteShader" => {
                let Some(h) = obj(a, 0, Kind::Shader)? else { return Ok(Ret::Undefined) };
                if let Some(s) = self.objs.shaders.get_mut(&h) {
                    if s.attached > 0 {
                        s.delete_pending = true;
                    } else {
                        self.objs.shaders.remove(&h);
                    }
                }
                Ok(Ret::Undefined)
            }
            "shaderSource" => {
                let h = self.shader_arg(a, 0)?;
                let Arg::Str(src) = arg(a, 1) else { return Err(INVALID_OPERATION) };
                if let Some(s) = self.objs.shaders.get_mut(&h) {
                    s.source = src.to_string();
                }
                Ok(Ret::Undefined)
            }
            "compileShader" => {
                let h = self.shader_arg(a, 0)?;
                let webgl2 = self.is2();
                let s = self.objs.shaders.get_mut(&h).ok_or(INVALID_VALUE)?;
                let stage = if s.ty == VERTEX_SHADER { Stage::Vertex } else { Stage::Fragment };
                match invisium_glsl::compile(stage, &s.source, webgl2) {
                    Ok(c) => {
                        s.compiled = Some(c);
                        s.compile_status = true;
                        s.log.clear();
                    }
                    Err(log) => {
                        s.compiled = None;
                        s.compile_status = false;
                        s.log = log;
                    }
                }
                Ok(Ret::Undefined)
            }
            "getShaderParameter" => {
                let h = self.shader_arg(a, 0)?;
                let s = &self.objs.shaders[&h];
                match uint(a, 1)? {
                    SHADER_TYPE => Ok(Ret::Num(s.ty as f64)),
                    DELETE_STATUS => Ok(Ret::Bool(s.delete_pending)),
                    COMPILE_STATUS => Ok(Ret::Bool(s.compile_status)),
                    _ => Err(INVALID_ENUM),
                }
            }
            "getShaderInfoLog" => Ok(Ret::Str(self.objs.shaders[&self.shader_arg(a, 0)?].log.clone())),
            "getShaderSource" => Ok(Ret::Str(self.objs.shaders[&self.shader_arg(a, 0)?].source.clone())),
            "createProgram" => {
                let h = self.new_handle();
                self.objs.programs.insert(h, Program { tf_mode: INTERLEAVED_ATTRIBS, ..Program::default() });
                Ok(Ret::Object(Kind::Program, h))
            }
            "isProgram" => Ok(Ret::Bool(matches!(obj(a, 0, Kind::Program), Ok(Some(h)) if self.objs.programs.get(&h).is_some_and(|p| !p.delete_pending)))),
            "deleteProgram" => {
                let Some(h) = obj(a, 0, Kind::Program)? else { return Ok(Ret::Undefined) };
                if self.st.program == h {
                    if let Some(p) = self.objs.programs.get_mut(&h) {
                        p.delete_pending = true;
                    }
                } else {
                    self.free_program(h);
                }
                Ok(Ret::Undefined)
            }
            "attachShader" => {
                let (p, s) = (self.program_arg(a, 0)?, self.shader_arg(a, 1)?);
                let ty = self.objs.shaders[&s].ty;
                let prog = &self.objs.programs[&p];
                if prog.shaders.iter().any(|x| *x == s || self.objs.shaders.get(x).is_some_and(|o| o.ty == ty)) {
                    return Err(INVALID_OPERATION);
                }
                self.objs.programs.get_mut(&p).ok_or(INVALID_VALUE)?.shaders.push(s);
                if let Some(sh) = self.objs.shaders.get_mut(&s) {
                    sh.attached += 1;
                }
                Ok(Ret::Undefined)
            }
            "detachShader" => {
                let p = self.program_arg(a, 0)?;
                let s = obj(a, 1, Kind::Shader)?.ok_or(INVALID_VALUE)?;
                let prog = self.objs.programs.get_mut(&p).ok_or(INVALID_VALUE)?;
                let i = prog.shaders.iter().position(|x| *x == s).ok_or(INVALID_OPERATION)?;
                prog.shaders.remove(i);
                self.release_shader(s);
                Ok(Ret::Undefined)
            }
            "getAttachedShaders" => {
                let p = self.program_arg(a, 0)?;
                Ok(Ret::Objects(Kind::Shader, self.objs.programs[&p].shaders.clone()))
            }
            "bindAttribLocation" => {
                let (p, index) = (self.program_arg(a, 0)?, uint(a, 1)?);
                let Arg::Str(n) = arg(a, 2) else { return Err(INVALID_OPERATION) };
                if index >= crate::limits::MAX_VERTEX_ATTRIBS {
                    return Err(INVALID_VALUE);
                }
                if reserved_name(n) {
                    return Err(INVALID_OPERATION);
                }
                let b = &mut self.objs.programs.get_mut(&p).ok_or(INVALID_VALUE)?.bindings;
                b.retain(|x| x.0 != n);
                b.push((n.to_string(), index));
                Ok(Ret::Undefined)
            }
            "linkProgram" => self.link(a),
            "useProgram" => {
                let p = match obj(a, 0, Kind::Program)? {
                    None => 0,
                    Some(h) => match self.objs.programs.get(&h) {
                        Some(p) if !p.delete_pending && p.link_status => h,
                        Some(p) if !p.delete_pending => return Err(INVALID_OPERATION),
                        _ => return Err(INVALID_VALUE),
                    },
                };
                if self.st.tf_active && !self.st.tf_paused {
                    return Err(INVALID_OPERATION);
                }
                let old = std::mem::replace(&mut self.st.program, p);
                if old != p && self.objs.programs.get(&old).is_some_and(|o| o.delete_pending) {
                    self.free_program(old);
                } else if old != p {
                    // The executable kept after a failed relink goes when the program stops being used.
                    if let Some(o) = self.objs.programs.get_mut(&old).filter(|o| !o.link_status) {
                        o.exe = None;
                        self.drop_locations(old);
                    }
                }
                Ok(Ret::Undefined)
            }
            "validateProgram" => {
                let p = self.program_arg(a, 0)?;
                let prog = self.objs.programs.get_mut(&p).ok_or(INVALID_VALUE)?;
                prog.validate_status = prog.link_status;
                Ok(Ret::Undefined)
            }
            "getProgramInfoLog" => Ok(Ret::Str(self.objs.programs[&self.program_arg(a, 0)?].log.clone())),
            "getProgramParameter" => self.program_param(self.program_arg(a, 0)?, uint(a, 1)?),
            _ => self.introspect_call(name, a),
        }
    }

    fn link(&mut self, a: &[Arg]) -> R<Ret> {
        let h = self.program_arg(a, 0)?;
        if h == self.st.program && self.st.tf_active {
            return Err(INVALID_OPERATION);
        }
        let prog = &self.objs.programs[&h];
        let find = |ty: u32| prog.shaders.iter().filter_map(|s| self.objs.shaders.get(s)).find(|s| s.ty == ty);
        let result = match (find(VERTEX_SHADER), find(FRAGMENT_SHADER)) {
            (Some(v), Some(f)) => match (&v.compiled, &f.compiled) {
                (Some(vc), Some(fc)) => invisium_glsl::link(vc, fc, &prog.bindings),
                _ => Err("ERROR: an attached shader did not compile\n".to_string()),
            },
            _ => Err("ERROR: a vertex and a fragment shader must be attached\n".to_string()),
        };
        let current = h == self.st.program;
        // A failed relink of the program in use keeps its executable and its locations.
        if result.is_ok() || !current {
            self.drop_locations(h);
        }
        let p = self.objs.programs.get_mut(&h).ok_or(INVALID_VALUE)?;
        p.validate_status = false;
        match result {
            Ok(linked) => {
                let desc = program_desc(&linked);
                p.exe = Some(Linked::new(linked));
                p.link_status = true;
                p.log.clear();
                p.on_dev = true;
                self.push(Cmd::ProgramCreate { id: h, desc: Box::new(desc) });
            }
            Err(log) => {
                p.link_status = false;
                p.log = log;
                if !current {
                    p.exe = None;
                }
            }
        }
        Ok(Ret::Undefined)
    }

    fn program_param(&self, h: u32, pname: u32) -> R<Ret> {
        let p = &self.objs.programs[&h];
        let exe = p.exe.as_ref().filter(|_| p.link_status).map(|e| &e.prog);
        let count = |f: fn(&invisium_glsl::Program) -> usize| Ret::Num(exe.map(f).unwrap_or(0) as f64);
        let webgl2 = self.is2();
        Ok(match pname {
            DELETE_STATUS => Ret::Bool(p.delete_pending),
            LINK_STATUS => Ret::Bool(p.link_status),
            VALIDATE_STATUS => Ret::Bool(p.validate_status),
            ATTACHED_SHADERS => Ret::Num(p.shaders.len() as f64),
            ACTIVE_ATTRIBUTES => count(|e| e.attributes.len()),
            ACTIVE_UNIFORMS => count(|e| e.uniforms.len()),
            ACTIVE_UNIFORM_BLOCKS if webgl2 => count(|e| e.blocks.len()),
            TRANSFORM_FEEDBACK_BUFFER_MODE if webgl2 => Ret::Num(p.tf_mode as f64),
            TRANSFORM_FEEDBACK_VARYINGS if webgl2 => Ret::Num(if p.link_status { p.tf_varyings } else { 0 } as f64),
            _ => return Err(INVALID_ENUM),
        })
    }
}
