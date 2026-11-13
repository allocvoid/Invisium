//! `clear` and `clearBuffer*`.

use crate::args::*;
use crate::cmd::{ClearColor, Cmd, Target, TexFormat};
use crate::consts1::*;
use crate::consts2::*;
use crate::formats::{class, Class};
use crate::{Arg, Context, Elem, Ret};

impl Context {
    /// Format of draw buffer `i` of the draw framebuffer (`None`: nothing is drawn there).
    pub(crate) fn draw_buffer_format(&self, i: usize) -> Option<TexFormat> {
        match self.objs.framebuffers.get(&self.st.draw_fb) {
            None => (i == 0 && self.st.surface_draw == BACK).then_some(TexFormat::Rgba8),
            Some(f) => {
                let b = *f.draw_buffers.get(i)?;
                let idx = b.checked_sub(COLOR_ATTACHMENT0).filter(|&k| k < 8)? as usize;
                f.color[idx].and_then(|a| self.att_info(&a)).map(|x| x.fmt)
            }
        }
    }

    /// Whether the draw framebuffer has depth / stencil planes.
    pub(crate) fn draw_planes(&self) -> (bool, bool) {
        match self.objs.framebuffers.get(&self.st.draw_fb) {
            None => (self.attrs.depth, self.attrs.stencil),
            Some(f) => match self.fb_depth_stencil(f).and_then(|a| self.att_info(&a)) {
                Some(i) => (i.fmt.has_depth(), i.fmt.has_stencil()),
                None => (false, false),
            },
        }
    }

    fn masks(&self, target: &Target) -> ([bool; 4], u32, Option<[i32; 4]>) {
        let mut cm = self.st.color_mask;
        if *target == Target::Surface && !self.attrs.alpha {
            cm[3] = false;
        }
        let scissor = self.st.caps.scissor_test.then_some(self.st.scissor);
        (cm, self.st.stencil[0].write_mask, scissor)
    }

    fn push_clear(&mut self, target: Target, colors: Vec<(u32, ClearColor)>, depth: Option<f32>, stencil: Option<u32>) {
        if colors.is_empty() && depth.is_none() && stencil.is_none() {
            return;
        }
        if self.st.caps.rasterizer_discard {
            return;
        }
        let (color_mask, stencil_mask, scissor) = self.masks(&target);
        if target == Target::Surface {
            self.drawn = true;
        }
        self.push(Cmd::Clear { target, colors, depth, stencil, color_mask, stencil_mask, scissor });
    }

    pub(crate) fn clear_call(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        if name == "clear" {
            let mask = uint(a, 0)?;
            if mask & !(COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT | STENCIL_BUFFER_BIT) != 0 {
                return Err(INVALID_VALUE);
            }
            let target = self.target_of(self.st.draw_fb, true)?;
            let mut colors = Vec::new();
            if mask & COLOR_BUFFER_BIT != 0 {
                for i in 0..crate::limits::MAX_DRAW_BUFFERS as usize {
                    if let Some(f) = self.draw_buffer_format(i) {
                        if matches!(class(f), Class::Int | Class::Uint) {
                            return Err(INVALID_OPERATION);
                        }
                        colors.push((i as u32, ClearColor::F(self.st.clear_color)));
                    }
                }
            }
            let (has_d, has_s) = self.draw_planes();
            let depth = (mask & DEPTH_BUFFER_BIT != 0 && has_d).then_some(self.st.clear_depth);
            let stencil = (mask & STENCIL_BUFFER_BIT != 0 && has_s).then_some(self.st.clear_stencil as u32);
            self.push_clear(target, colors, depth, stencil);
            return Ok(Ret::Undefined);
        }
        let buffer = uint(a, 0)?;
        let draw = int(a, 1)?;
        let (want, elem) = match (name, buffer) {
            ("clearBufferfv", COLOR) => (4, Elem::F32),
            ("clearBufferfv", DEPTH) => (1, Elem::F32),
            ("clearBufferiv", COLOR) => (4, Elem::I32),
            ("clearBufferiv", STENCIL) => (1, Elem::I32),
            ("clearBufferuiv", COLOR) => (4, Elem::U32),
            ("clearBufferfi", DEPTH_STENCIL) => (0, Elem::F32),
            _ => return Err(INVALID_ENUM),
        };
        if draw < 0 || draw as u32 >= crate::limits::MAX_DRAW_BUFFERS || (buffer != COLOR && draw != 0) {
            return Err(INVALID_VALUE);
        }
        let values = if want > 0 {
            let v = nums(a, 2, elem)?;
            let off = uint(a, 3)? as usize;
            if v.len() < want || off > v.len() - want {
                return Err(INVALID_VALUE);
            }
            v[off..off + want].to_vec()
        } else {
            vec![float(a, 2)? as f64, int(a, 3)? as f64]
        };
        let target = self.target_of(self.st.draw_fb, true)?;
        let (has_d, has_s) = self.draw_planes();
        match buffer {
            COLOR => {
                let Some(f) = self.draw_buffer_format(draw as usize) else { return Ok(Ret::Undefined) };
                let v4 = [values[0], values[1], values[2], values[3]];
                let value = match (class(f), elem) {
                    (Class::Norm | Class::Float, Elem::F32) => ClearColor::F(v4.map(|x| x as f32)),
                    (Class::Int, Elem::I32) => ClearColor::I(v4.map(|x| x as i32)),
                    (Class::Uint, Elem::U32) => ClearColor::U(v4.map(|x| x as u32)),
                    _ => return Err(INVALID_OPERATION),
                };
                self.push_clear(target, vec![(draw as u32, value)], None, None);
            }
            DEPTH => self.push_clear(target, vec![], has_d.then_some(values[0].clamp(0.0, 1.0) as f32), None),
            STENCIL => self.push_clear(target, vec![], None, has_s.then_some(values[0] as i32 as u32)),
            _ => {
                let d = has_d.then_some(values[0].clamp(0.0, 1.0) as f32);
                self.push_clear(target, vec![], d, has_s.then_some(values[1] as i32 as u32));
            }
        }
        Ok(Ret::Undefined)
    }
}
