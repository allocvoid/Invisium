//! Renderbuffer objects (device textures, possibly multisampled) and colour renderability.

use crate::args::*;
use crate::cmd::{Cmd, TexDesc, TexFormat, TexKind};
use crate::consts1::*;
use crate::consts2::*;
use crate::formats::{bits, class, sized, Class, Rend};
use crate::objects::{Att, Renderbuffer};
use crate::{Arg, Context, Kind, Ret};

impl Context {
    /// Whether an image of this internal format can be a colour attachment.
    pub(crate) fn color_renderable(&self, internal: u32, fmt: TexFormat) -> bool {
        if !fmt.is_color() {
            return false;
        }
        match internal {
            RGBA | RGB => match fmt {
                TexFormat::Rgba8 => true,
                TexFormat::Rgba32F => !self.is2() && internal == RGBA && self.ext("WEBGL_color_buffer_float"),
                _ => false,
            },
            LUMINANCE | LUMINANCE_ALPHA | ALPHA => false,
            RGBA4 | RGB565 | RGB5_A1 if !self.is2() => true,
            RGBA32F if !self.is2() => self.ext("WEBGL_color_buffer_float"),
            _ if !self.is2() => false,
            _ => match sized(internal).map(|s| s.rend) {
                Some(Rend::Yes) => true,
                Some(Rend::Float) => self.ext("EXT_color_buffer_float"),
                Some(Rend::Half) => self.ext("EXT_color_buffer_float") || self.ext("EXT_color_buffer_half_float"),
                Some(Rend::HalfOnly) => self.ext("EXT_color_buffer_half_float"),
                _ => false,
            },
        }
    }

    /// The device format of a renderbuffer internal format.
    fn rb_format(&self, internal: u32) -> Option<TexFormat> {
        match internal {
            STENCIL_INDEX8 => Some(TexFormat::Stencil8),
            DEPTH_STENCIL => Some(TexFormat::Depth24Stencil8),
            DEPTH_COMPONENT16 => Some(TexFormat::Depth16),
            RGBA4 | RGB565 | RGB5_A1 => Some(TexFormat::Rgba8),
            _ if !self.is2() => (internal == RGBA32F && self.color_renderable(internal, TexFormat::Rgba32F)).then_some(TexFormat::Rgba32F),
            _ => {
                let s = sized(internal)?;
                (class(s.fmt) == Class::Depth || self.color_renderable(internal, s.fmt)).then_some(s.fmt)
            }
        }
    }

    pub(crate) fn renderbuffer_call(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        match name {
            "createRenderbuffer" => {
                let h = self.new_handle();
                self.objs.renderbuffers.insert(h, Renderbuffer { internal: RGBA4, fmt: None, w: 0, h: 0, samples: 0, bound_once: false });
                Ok(Ret::Object(Kind::Renderbuffer, h))
            }
            "isRenderbuffer" => Ok(Ret::Bool(match obj(a, 0, Kind::Renderbuffer) {
                Ok(Some(h)) => self.objs.renderbuffers.get(&h).is_some_and(|r| r.bound_once),
                _ => false,
            })),
            "deleteRenderbuffer" => {
                let Some(h) = obj(a, 0, Kind::Renderbuffer)? else { return Ok(Ret::Undefined) };
                if let Some(rb) = self.objs.renderbuffers.remove(&h) {
                    if self.st.renderbuffer == h {
                        self.st.renderbuffer = 0;
                    }
                    for fb in self.objs.framebuffers.values_mut() {
                        fb.detach(|a| *a == Att::Rb(h));
                    }
                    if rb.fmt.is_some() && rb.w > 0 && rb.h > 0 {
                        self.push(Cmd::TextureDelete { id: h });
                    }
                    self.release(h);
                }
                Ok(Ret::Undefined)
            }
            "bindRenderbuffer" => {
                if uint(a, 0)? != RENDERBUFFER {
                    return Err(INVALID_ENUM);
                }
                let r = obj(a, 1, Kind::Renderbuffer)?;
                if let Some(h) = r {
                    self.objs.renderbuffers.get_mut(&h).ok_or(INVALID_OPERATION)?.bound_once = true;
                }
                self.st.renderbuffer = r.unwrap_or(0);
                Ok(Ret::Undefined)
            }
            "renderbufferStorage" => self.rb_storage(uint(a, 0)?, 0, uint(a, 1)?, int(a, 2)?, int(a, 3)?),
            "renderbufferStorageMultisample" => self.rb_storage(uint(a, 0)?, int(a, 1)?, uint(a, 2)?, int(a, 3)?, int(a, 4)?),
            "getRenderbufferParameter" => {
                if uint(a, 0)? != RENDERBUFFER {
                    return Err(INVALID_ENUM);
                }
                let pname = uint(a, 1)?;
                let rb = self.objs.renderbuffers.get(&self.st.renderbuffer).ok_or(INVALID_OPERATION)?;
                let b = rb.fmt.map(|f| bits(rb.internal, f)).unwrap_or([0; 6]);
                let v = match pname {
                    RENDERBUFFER_WIDTH => rb.w,
                    RENDERBUFFER_HEIGHT => rb.h,
                    RENDERBUFFER_INTERNAL_FORMAT => rb.internal,
                    RENDERBUFFER_RED_SIZE..=RENDERBUFFER_STENCIL_SIZE => b[(pname - RENDERBUFFER_RED_SIZE) as usize],
                    RENDERBUFFER_SAMPLES if self.is2() => rb.samples,
                    _ => return Err(INVALID_ENUM),
                };
                Ok(Ret::Num(v as f64))
            }
            _ => Ok(Ret::Undefined),
        }
    }

    fn rb_storage(&mut self, target: u32, samples: i32, internal: u32, w: i32, h: i32) -> R<Ret> {
        if target != RENDERBUFFER {
            return Err(INVALID_ENUM);
        }
        let fmt = self.rb_format(internal).ok_or(INVALID_ENUM)?;
        let id = self.st.renderbuffer;
        if !self.objs.renderbuffers.contains_key(&id) {
            return Err(INVALID_OPERATION);
        }
        let max = crate::limits::MAX_RENDERBUFFER_SIZE as i32;
        if w < 0 || h < 0 || samples < 0 || w > max || h > max {
            return Err(INVALID_VALUE);
        }
        if samples as u32 > crate::limits::MAX_SAMPLES || (samples > 0 && matches!(class(fmt), Class::Int | Class::Uint)) {
            return Err(INVALID_OPERATION);
        }
        let (w, h) = (w as u32, h as u32);
        let dev_samples = if samples > 0 { crate::limits::MAX_SAMPLES } else { 1 };
        let had = self.objs.renderbuffers.get(&id).is_some_and(|r| r.fmt.is_some() && r.w > 0 && r.h > 0);
        if w > 0 && h > 0 {
            let desc = TexDesc { kind: TexKind::D2, format: fmt, width: w, height: h, depth: 1, mips: 1, samples: dev_samples };
            self.reserve(id, crate::objects::tex_bytes(&desc))?;
            self.push(Cmd::TextureCreate { id, desc });
        } else if had {
            self.push(Cmd::TextureDelete { id });
            self.release(id);
        }
        if let Some(rb) = self.objs.renderbuffers.get_mut(&id) {
            *rb = Renderbuffer { internal, fmt: Some(fmt), w, h, samples: if samples > 0 { dev_samples } else { 0 }, bound_once: true };
        }
        Ok(Ret::Undefined)
    }
}
