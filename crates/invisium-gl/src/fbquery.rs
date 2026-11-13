//! Framebuffer queries and draw/read buffer selection.

use crate::args::*;
use crate::consts1::*;
use crate::consts2::*;
use crate::fbo::Point;
use crate::formats::{bits, class, Class};
use crate::objects::*;
use crate::{Arg, Context, Elem, Kind, Ret};

impl Context {
    pub(crate) fn fb_query_call(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        match name {
            "getFramebufferAttachmentParameter" => self.fb_att_param(uint(a, 0)?, uint(a, 1)?, uint(a, 2)?),
            "drawBuffers" => {
                let list: Vec<u32> = nums(a, 0, Elem::U32)?.iter().map(|&n| n as u32).collect();
                self.draw_buffers(&list)
            }
            "readBuffer" => {
                let src = uint(a, 0)?;
                let fb = self.st.read_fb;
                if fb == 0 {
                    if src != BACK && src != NONE {
                        return Err(if (COLOR_ATTACHMENT0..=COLOR_ATTACHMENT15).contains(&src) { INVALID_OPERATION } else { INVALID_ENUM });
                    }
                    self.st.surface_read = src;
                } else {
                    let ok = src == NONE || (COLOR_ATTACHMENT0..COLOR_ATTACHMENT0 + crate::limits::MAX_COLOR_ATTACHMENTS).contains(&src);
                    if !ok {
                        return Err(if src == BACK || (COLOR_ATTACHMENT0..=COLOR_ATTACHMENT15).contains(&src) { INVALID_OPERATION } else { INVALID_ENUM });
                    }
                    if let Some(f) = self.objs.framebuffers.get_mut(&fb) {
                        f.read_buffer = src;
                    }
                }
                Ok(Ret::Undefined)
            }
            "invalidateFramebuffer" | "invalidateSubFramebuffer" => {
                let fb = self.fb_of(uint(a, 0)?)?;
                let list = nums(a, 1, Elem::U32)?;
                if name == "invalidateSubFramebuffer" && (int(a, 4)? < 0 || int(a, 5)? < 0) {
                    return Err(INVALID_VALUE);
                }
                for &n in &list {
                    let e = n as u32;
                    if fb == 0 {
                        if !matches!(e, COLOR | DEPTH | STENCIL) {
                            return Err(INVALID_ENUM);
                        }
                    } else {
                        self.point(e)?;
                    }
                }
                Ok(Ret::Undefined)
            }
            _ => Ok(Ret::Undefined),
        }
    }

    fn draw_buffers(&mut self, list: &[u32]) -> R<Ret> {
        if list.len() > crate::limits::MAX_DRAW_BUFFERS as usize {
            return Err(INVALID_VALUE);
        }
        let fb = self.st.draw_fb;
        if fb == 0 {
            if list.len() != 1 || (list[0] != BACK && list[0] != NONE) {
                return Err(INVALID_OPERATION);
            }
            self.st.surface_draw = list[0];
            return Ok(Ret::Undefined);
        }
        for (i, &b) in list.iter().enumerate() {
            if b == NONE || b == COLOR_ATTACHMENT0 + i as u32 {
                continue;
            }
            return Err(if b == BACK || (COLOR_ATTACHMENT0..=COLOR_ATTACHMENT15).contains(&b) { INVALID_OPERATION } else { INVALID_ENUM });
        }
        if let Some(f) = self.objs.framebuffers.get_mut(&fb) {
            f.draw_buffers = [NONE; 8];
            f.draw_buffers[..list.len()].copy_from_slice(list);
        }
        Ok(Ret::Undefined)
    }

    fn fb_att_param(&self, target: u32, attachment: u32, pname: u32) -> R<Ret> {
        let fb = self.fb_of(target)?;
        let webgl2 = self.is2();
        if fb == 0 {
            if !webgl2 {
                return Err(INVALID_OPERATION);
            }
            let at = self.attrs;
            let (exists, b) = match attachment {
                BACK => (true, [8, 8, 8, if at.alpha { 8 } else { 0 }, 0, 0]),
                DEPTH => (at.depth, [0, 0, 0, 0, if at.depth { 24 } else { 0 }, 0]),
                STENCIL => (at.stencil, [0, 0, 0, 0, 0, if at.stencil { 8 } else { 0 }]),
                _ => return Err(INVALID_ENUM),
            };
            return match pname {
                FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE => Ok(Ret::Num(if exists { FRAMEBUFFER_DEFAULT } else { NONE } as f64)),
                _ if !exists => Err(INVALID_OPERATION),
                FRAMEBUFFER_ATTACHMENT_RED_SIZE..=FRAMEBUFFER_ATTACHMENT_STENCIL_SIZE => Ok(Ret::Num(b[(pname - FRAMEBUFFER_ATTACHMENT_RED_SIZE) as usize] as f64)),
                FRAMEBUFFER_ATTACHMENT_COMPONENT_TYPE => Ok(Ret::Num(if attachment == STENCIL { INT } else { UNSIGNED_NORMALIZED } as f64)),
                FRAMEBUFFER_ATTACHMENT_COLOR_ENCODING => Ok(Ret::Num(LINEAR as f64)),
                _ => Err(INVALID_ENUM),
            };
        }
        let f = self.objs.framebuffers.get(&fb).ok_or(INVALID_OPERATION)?;
        let att = match self.point(attachment)? {
            Point::Color(i) => f.color[i],
            Point::Depth => f.depth,
            Point::Stencil => f.stencil,
            Point::DepthStencil if webgl2 => {
                if f.depth != f.stencil {
                    return Err(INVALID_OPERATION);
                }
                f.depth
            }
            Point::DepthStencil => f.depth_stencil,
        };
        let Some(att) = att else {
            return match pname {
                FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE => Ok(Ret::Num(NONE as f64)),
                FRAMEBUFFER_ATTACHMENT_OBJECT_NAME => Ok(Ret::Null),
                _ => Err(if webgl2 { INVALID_OPERATION } else { INVALID_ENUM }),
            };
        };
        let info = self.att_info(&att);
        let ext_sizes = webgl2 || self.ext("WEBGL_color_buffer_float");
        match pname {
            FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE => Ok(Ret::Num(match att {
                Att::Tex { .. } => TEXTURE,
                Att::Rb(_) => RENDERBUFFER,
            } as f64)),
            FRAMEBUFFER_ATTACHMENT_OBJECT_NAME => Ok(match att {
                Att::Tex { id, .. } => Ret::Object(Kind::Texture, id),
                Att::Rb(id) => Ret::Object(Kind::Renderbuffer, id),
            }),
            FRAMEBUFFER_ATTACHMENT_TEXTURE_LEVEL | FRAMEBUFFER_ATTACHMENT_TEXTURE_CUBE_MAP_FACE | FRAMEBUFFER_ATTACHMENT_TEXTURE_LAYER => {
                let Att::Tex { id, level, face, layer } = att else { return Err(INVALID_ENUM) };
                let cube = self.objs.textures.get(&id).is_some_and(|t| t.kind == Some(crate::cmd::TexKind::Cube));
                Ok(Ret::Num(match pname {
                    FRAMEBUFFER_ATTACHMENT_TEXTURE_LEVEL => level,
                    FRAMEBUFFER_ATTACHMENT_TEXTURE_CUBE_MAP_FACE => if cube { TEXTURE_CUBE_MAP_POSITIVE_X + face } else { 0 },
                    _ if webgl2 => layer,
                    _ => return Err(INVALID_ENUM),
                } as f64))
            }
            FRAMEBUFFER_ATTACHMENT_RED_SIZE..=FRAMEBUFFER_ATTACHMENT_STENCIL_SIZE if webgl2 => {
                let b = info.map(|i| bits(i.internal, i.fmt)).unwrap_or([0; 6]);
                Ok(Ret::Num(b[(pname - FRAMEBUFFER_ATTACHMENT_RED_SIZE) as usize] as f64))
            }
            FRAMEBUFFER_ATTACHMENT_COMPONENT_TYPE if ext_sizes => {
                let Some(i) = info else { return Ok(Ret::Num(NONE as f64)) };
                if attachment == DEPTH_STENCIL_ATTACHMENT && webgl2 {
                    return Err(INVALID_OPERATION);
                }
                let snorm = matches!(i.fmt, crate::cmd::TexFormat::R8Snorm | crate::cmd::TexFormat::Rg8Snorm | crate::cmd::TexFormat::Rgba8Snorm);
                Ok(Ret::Num(match class(i.fmt) {
                    Class::Norm if snorm => SIGNED_NORMALIZED,
                    Class::Norm => UNSIGNED_NORMALIZED,
                    Class::Float => FLOAT,
                    Class::Int => INT,
                    Class::Uint => UNSIGNED_INT,
                    Class::Depth if i.fmt == crate::cmd::TexFormat::Depth32F || i.fmt == crate::cmd::TexFormat::Depth32FStencil8 => FLOAT,
                    Class::Depth if attachment == STENCIL_ATTACHMENT => UNSIGNED_INT,
                    Class::Depth => UNSIGNED_NORMALIZED,
                } as f64))
            }
            FRAMEBUFFER_ATTACHMENT_COLOR_ENCODING if webgl2 => {
                let srgb = info.is_some_and(|i| i.fmt == crate::cmd::TexFormat::Srgb8A8);
                Ok(Ret::Num(if srgb { SRGB } else { LINEAR } as f64))
            }
            _ => Err(INVALID_ENUM),
        }
    }
}
