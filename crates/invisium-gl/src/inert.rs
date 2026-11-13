//! Sampler objects, and the inert stand-ins: queries (report 0), sync objects (always
//! signalled), transform feedback (validated state, nothing captured);
//! `getInternalformatParameter`.

use crate::args::*;
use crate::consts1::*;
use crate::consts2::*;
use crate::formats::{class, sized, Class};
use crate::objects::TexParams;
use crate::progobj::{Query, Sampler, Tf};
use crate::textures::{get_param, set_param};
use crate::{Arg, Context, Kind, Ret};

impl Context {
    pub(crate) fn inert_call(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        match name {
            "createSampler" => {
                let h = self.new_handle();
                self.objs.samplers.insert(h, Sampler { params: TexParams::default() });
                Ok(Ret::Object(Kind::Sampler, h))
            }
            "isSampler" => Ok(Ret::Bool(matches!(obj(a, 0, Kind::Sampler), Ok(Some(h)) if self.objs.samplers.contains_key(&h)))),
            "deleteSampler" => {
                if let Some(h) = obj(a, 0, Kind::Sampler)? {
                    if self.objs.samplers.remove(&h).is_some() {
                        self.st.sampler_units.iter_mut().filter(|s| **s == h).for_each(|s| *s = 0);
                    }
                }
                Ok(Ret::Undefined)
            }
            "bindSampler" => {
                let unit = uint(a, 0)?;
                let s = obj(a, 1, Kind::Sampler)?;
                if unit >= crate::limits::MAX_COMBINED_TEXTURE_IMAGE_UNITS {
                    return Err(INVALID_VALUE);
                }
                if s.is_some_and(|h| !self.objs.samplers.contains_key(&h)) {
                    return Err(INVALID_OPERATION);
                }
                self.st.sampler_units[unit as usize] = s.unwrap_or(0);
                Ok(Ret::Undefined)
            }
            "samplerParameteri" | "samplerParameterf" => {
                let h = obj(a, 0, Kind::Sampler)?.ok_or(INVALID_VALUE)?;
                let (pname, f) = (uint(a, 1)?, float(a, 2)?);
                let i = if name == "samplerParameteri" { int(a, 2)? } else { f as i32 };
                let s = self.objs.samplers.get_mut(&h).ok_or(INVALID_OPERATION)?;
                set_param(&mut s.params, pname, i, f, true, true)?;
                Ok(Ret::Undefined)
            }
            "getSamplerParameter" => {
                let h = obj(a, 0, Kind::Sampler)?.ok_or(INVALID_VALUE)?;
                let s = self.objs.samplers.get(&h).ok_or(INVALID_OPERATION)?;
                get_param(&s.params, uint(a, 1)?, true, true)
            }
            "createQuery" => {
                let h = self.new_handle();
                self.objs.queries.insert(h, Query { target: 0 });
                Ok(Ret::Object(Kind::Query, h))
            }
            "isQuery" => Ok(Ret::Bool(matches!(obj(a, 0, Kind::Query), Ok(Some(h)) if self.objs.queries.get(&h).is_some_and(|q| q.target != 0)))),
            "deleteQuery" => {
                if let Some(h) = obj(a, 0, Kind::Query)? {
                    self.objs.queries.remove(&h);
                    self.st.queries.retain(|q| q.1 != h);
                }
                Ok(Ret::Undefined)
            }
            "beginQuery" => {
                let target = query_target(uint(a, 0)?)?;
                let h = obj(a, 1, Kind::Query)?.ok_or(INVALID_OPERATION)?;
                let q = self.objs.queries.get_mut(&h).ok_or(INVALID_OPERATION)?;
                let occlusion = |t: u32| t == ANY_SAMPLES_PASSED || t == ANY_SAMPLES_PASSED_CONSERVATIVE;
                if (q.target != 0 && q.target != target) || self.st.queries.iter().any(|x| x.1 == h || x.0 == target || (occlusion(x.0) && occlusion(target))) {
                    return Err(INVALID_OPERATION);
                }
                q.target = target;
                self.st.queries.push((target, h));
                Ok(Ret::Undefined)
            }
            "endQuery" => {
                let target = query_target(uint(a, 0)?)?;
                let i = self.st.queries.iter().position(|q| q.0 == target).ok_or(INVALID_OPERATION)?;
                self.st.queries.remove(i);
                Ok(Ret::Undefined)
            }
            "getQuery" => {
                let target = query_target(uint(a, 0)?)?;
                if uint(a, 1)? != CURRENT_QUERY {
                    return Err(INVALID_ENUM);
                }
                Ok(match self.st.queries.iter().find(|q| q.0 == target) {
                    Some(q) => Ret::Object(Kind::Query, q.1),
                    None => Ret::Null,
                })
            }
            "getQueryParameter" => {
                let h = obj(a, 0, Kind::Query)?.ok_or(INVALID_OPERATION)?;
                let q = self.objs.queries.get(&h).ok_or(INVALID_OPERATION)?;
                if q.target == 0 || self.st.queries.iter().any(|x| x.1 == h) {
                    return Err(INVALID_OPERATION);
                }
                match uint(a, 1)? {
                    QUERY_RESULT => Ok(if q.target == TRANSFORM_FEEDBACK_PRIMITIVES_WRITTEN { Ret::Num(0.0) } else { Ret::Bool(false) }),
                    QUERY_RESULT_AVAILABLE => Ok(Ret::Bool(true)),
                    _ => Err(INVALID_ENUM),
                }
            }
            "fenceSync" => {
                if uint(a, 0)? != SYNC_GPU_COMMANDS_COMPLETE {
                    return Err(INVALID_ENUM);
                }
                if uint(a, 1)? != 0 {
                    return Err(INVALID_VALUE);
                }
                let h = self.new_handle();
                self.objs.syncs.insert(h);
                Ok(Ret::Object(Kind::Sync, h))
            }
            "isSync" => Ok(Ret::Bool(matches!(obj(a, 0, Kind::Sync), Ok(Some(h)) if self.objs.syncs.contains(&h)))),
            "deleteSync" => {
                if let Some(h) = obj(a, 0, Kind::Sync)? {
                    self.objs.syncs.remove(&h);
                }
                Ok(Ret::Undefined)
            }
            "clientWaitSync" | "waitSync" => {
                let h = obj(a, 0, Kind::Sync)?.ok_or(INVALID_VALUE)?;
                if !self.objs.syncs.contains(&h) {
                    return Err(INVALID_VALUE);
                }
                let flags = uint(a, 1)?;
                if name == "waitSync" {
                    return if flags != 0 { Err(INVALID_VALUE) } else { Ok(Ret::Undefined) };
                }
                if flags & !SYNC_FLUSH_COMMANDS_BIT != 0 {
                    return Err(INVALID_VALUE);
                }
                Ok(Ret::Num(ALREADY_SIGNALED as f64))
            }
            "getSyncParameter" => {
                let h = obj(a, 0, Kind::Sync)?.ok_or(INVALID_VALUE)?;
                if !self.objs.syncs.contains(&h) {
                    return Err(INVALID_VALUE);
                }
                Ok(Ret::Num(match uint(a, 1)? {
                    OBJECT_TYPE => SYNC_FENCE,
                    SYNC_STATUS => SIGNALED,
                    SYNC_CONDITION => SYNC_GPU_COMMANDS_COMPLETE,
                    SYNC_FLAGS => 0,
                    _ => return Err(INVALID_ENUM),
                } as f64))
            }
            "getInternalformatParameter" => {
                let (target, internal, pname) = (uint(a, 0)?, uint(a, 1)?, uint(a, 2)?);
                if target != RENDERBUFFER || pname != SAMPLES {
                    return Err(INVALID_ENUM);
                }
                let s = sized(internal).filter(|s| class(s.fmt) == Class::Depth || self.color_renderable(internal, s.fmt));
                match (internal, s) {
                    (STENCIL_INDEX8 | DEPTH_STENCIL, _) => Ok(Ret::I32s(vec![4])),
                    (_, Some(s)) if matches!(class(s.fmt), Class::Int | Class::Uint) => Ok(Ret::I32s(vec![])),
                    (_, Some(_)) => Ok(Ret::I32s(vec![4])),
                    _ => Err(INVALID_ENUM),
                }
            }
            _ => self.tf_call(name, a),
        }
    }

    fn tf_call(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        match name {
            "createTransformFeedback" => {
                let h = self.new_handle();
                self.objs.tfs.insert(h, Tf::default());
                Ok(Ret::Object(Kind::TransformFeedback, h))
            }
            "isTransformFeedback" => Ok(Ret::Bool(matches!(obj(a, 0, Kind::TransformFeedback), Ok(Some(h)) if self.objs.tfs.get(&h).is_some_and(|t| t.bound_once)))),
            "deleteTransformFeedback" => {
                if let Some(h) = obj(a, 0, Kind::TransformFeedback)? {
                    if self.st.tf == h && self.st.tf_active {
                        return Err(INVALID_OPERATION);
                    }
                    if self.objs.tfs.remove(&h).is_some() && self.st.tf == h {
                        self.st.tf = 0;
                    }
                }
                Ok(Ret::Undefined)
            }
            "bindTransformFeedback" => {
                if uint(a, 0)? != TRANSFORM_FEEDBACK {
                    return Err(INVALID_ENUM);
                }
                let t = obj(a, 1, Kind::TransformFeedback)?;
                if self.st.tf_active && !self.st.tf_paused {
                    return Err(INVALID_OPERATION);
                }
                if let Some(h) = t {
                    self.objs.tfs.get_mut(&h).ok_or(INVALID_OPERATION)?.bound_once = true;
                }
                self.st.tf = t.unwrap_or(0);
                Ok(Ret::Undefined)
            }
            "beginTransformFeedback" => {
                if !matches!(uint(a, 0)?, POINTS | LINES | TRIANGLES) {
                    return Err(INVALID_ENUM);
                }
                if self.st.tf_active || self.st.program == 0 {
                    return Err(INVALID_OPERATION);
                }
                self.st.tf_active = true;
                self.st.tf_paused = false;
                Ok(Ret::Undefined)
            }
            "endTransformFeedback" | "pauseTransformFeedback" | "resumeTransformFeedback" => {
                let ok = match name {
                    "endTransformFeedback" => self.st.tf_active,
                    "pauseTransformFeedback" => self.st.tf_active && !self.st.tf_paused,
                    _ => self.st.tf_active && self.st.tf_paused,
                };
                if !ok {
                    return Err(INVALID_OPERATION);
                }
                match name {
                    "endTransformFeedback" => {
                        self.st.tf_active = false;
                        self.st.tf_paused = false;
                    }
                    "pauseTransformFeedback" => self.st.tf_paused = true,
                    _ => self.st.tf_paused = false,
                }
                Ok(Ret::Undefined)
            }
            "transformFeedbackVaryings" => {
                let p = self.program_arg(a, 0)?;
                let count = match arg(a, 1) {
                    Arg::Str(_) => 1,
                    Arg::Nums(v) => v.len() as u32,
                    Arg::Null | Arg::Undefined => 0,
                    _ => return Err(INVALID_OPERATION),
                };
                let mode = uint(a, 2)?;
                if mode != INTERLEAVED_ATTRIBS && mode != SEPARATE_ATTRIBS {
                    return Err(INVALID_ENUM);
                }
                let prog = self.objs.programs.get_mut(&p).ok_or(INVALID_VALUE)?;
                prog.tf_varyings = count;
                prog.tf_mode = mode;
                Ok(Ret::Undefined)
            }
            "getTransformFeedbackVarying" => {
                let p = self.program_arg(a, 0)?;
                self.linked(p)?;
                // Nothing is captured, so no varying index is valid.
                Err(INVALID_VALUE)
            }
            _ => Ok(Ret::Undefined),
        }
    }
}

fn query_target(t: u32) -> R<u32> {
    match t {
        ANY_SAMPLES_PASSED | ANY_SAMPLES_PASSED_CONSERVATIVE | TRANSFORM_FEEDBACK_PRIMITIVES_WRITTEN => Ok(t),
        _ => Err(INVALID_ENUM),
    }
}
