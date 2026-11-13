//! Framebuffer objects: binding, attachments, completeness, device targets.

use crate::args::*;
use crate::cmd::{Attachment, Target, TexFormat, TexKind};
use crate::consts1::*;
use crate::consts2::*;
use crate::objects::*;
use crate::{Arg, Context, Kind, Ret};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Point {
    Color(usize),
    Depth,
    Stencil,
    DepthStencil,
}

/// What an attachment's image is.
#[derive(Clone, Copy)]
pub struct AttInfo {
    pub fmt: TexFormat,
    pub internal: u32,
    pub w: u32,
    pub h: u32,
    pub samples: u32,
}

pub fn dev_att(a: Att) -> Attachment {
    match a {
        Att::Tex { id, level, face, layer } => Attachment { texture: id, level, layer: face.max(layer) },
        Att::Rb(id) => Attachment { texture: id, level: 0, layer: 0 },
    }
}

impl Context {
    pub(crate) fn max_color(&self) -> usize {
        if self.is2() || self.ext("WEBGL_draw_buffers") {
            crate::limits::MAX_COLOR_ATTACHMENTS as usize
        } else {
            1
        }
    }

    pub(crate) fn point(&self, attachment: u32) -> R<Point> {
        match attachment {
            DEPTH_ATTACHMENT => Ok(Point::Depth),
            STENCIL_ATTACHMENT => Ok(Point::Stencil),
            DEPTH_STENCIL_ATTACHMENT => Ok(Point::DepthStencil),
            COLOR_ATTACHMENT0..=COLOR_ATTACHMENT15 => {
                let i = (attachment - COLOR_ATTACHMENT0) as usize;
                if i < self.max_color() {
                    Ok(Point::Color(i))
                } else if self.is2() {
                    Err(INVALID_OPERATION)
                } else {
                    Err(INVALID_ENUM)
                }
            }
            _ => Err(INVALID_ENUM),
        }
    }

    /// The framebuffer a target names (`FRAMEBUFFER` is the draw binding).
    pub(crate) fn fb_of(&self, target: u32) -> R<u32> {
        match target {
            FRAMEBUFFER => Ok(self.st.draw_fb),
            DRAW_FRAMEBUFFER if self.is2() => Ok(self.st.draw_fb),
            READ_FRAMEBUFFER if self.is2() => Ok(self.st.read_fb),
            _ => Err(INVALID_ENUM),
        }
    }

    pub(crate) fn att_info(&self, a: &Att) -> Option<AttInfo> {
        match *a {
            Att::Tex { id, level, face, layer } => {
                let t = self.objs.textures.get(&id)?;
                let img = t.image(level as usize, face as usize)?;
                if !img.on_dev || (matches!(t.kind, Some(TexKind::D3 | TexKind::Array)) && layer >= img.d) {
                    return None;
                }
                Some(AttInfo { fmt: img.fmt, internal: img.internal, w: img.w, h: img.h, samples: 0 })
            }
            Att::Rb(id) => {
                let rb = self.objs.renderbuffers.get(&id)?;
                Some(AttInfo { fmt: rb.fmt?, internal: rb.internal, w: rb.w, h: rb.h, samples: rb.samples })
            }
        }
    }

    /// `checkFramebufferStatus` of framebuffer `fb` (0 = the drawing buffer).
    pub(crate) fn fb_status(&self, fb: u32) -> u32 {
        let Some(f) = self.objs.framebuffers.get(&fb) else { return FRAMEBUFFER_COMPLETE };
        let mut infos = Vec::new();
        let webgl1 = !self.is2();
        for a in f.color.iter().flatten() {
            match self.att_info(a) {
                Some(i) if i.w > 0 && i.h > 0 && self.color_renderable(i.internal, i.fmt) => infos.push(i),
                _ => return FRAMEBUFFER_INCOMPLETE_ATTACHMENT,
            }
        }
        let checks: [(&Option<Att>, bool, bool); 3] = [(&f.depth, true, false), (&f.stencil, false, true), (&f.depth_stencil, true, true)];
        for (slot, depth, stencil) in checks {
            let Some(a) = slot else { continue };
            let Some(i) = self.att_info(a) else { return FRAMEBUFFER_INCOMPLETE_ATTACHMENT };
            let ok_d = !depth || i.fmt.has_depth();
            let ok_s = !stencil || i.fmt.has_stencil();
            let exact = !webgl1 || (depth && stencil) || (i.fmt.has_depth() != i.fmt.has_stencil());
            if !ok_d || !ok_s || !exact || i.w == 0 || i.h == 0 {
                return FRAMEBUFFER_INCOMPLETE_ATTACHMENT;
            }
            infos.push(i);
        }
        if infos.is_empty() {
            return FRAMEBUFFER_INCOMPLETE_MISSING_ATTACHMENT;
        }
        let ds = [f.depth, f.stencil, f.depth_stencil].iter().filter(|a| a.is_some()).count();
        if (webgl1 && ds > 1) || (f.depth.is_some() && f.stencil.is_some() && f.depth != f.stencil) {
            return FRAMEBUFFER_UNSUPPORTED;
        }
        if infos.iter().any(|i| i.w != infos[0].w || i.h != infos[0].h) {
            return FRAMEBUFFER_INCOMPLETE_DIMENSIONS;
        }
        if infos.iter().any(|i| i.samples != infos[0].samples) {
            return FRAMEBUFFER_INCOMPLETE_MULTISAMPLE;
        }
        FRAMEBUFFER_COMPLETE
    }

    pub(crate) fn fb_depth_stencil(&self, f: &Framebuffer) -> Option<Att> {
        f.depth_stencil.or(f.depth).or(f.stencil)
    }

    /// A complete framebuffer as a device target; `by_draw_buffer` orders colours by draw
    /// buffer (drawing, clearing) instead of by attachment index (reading).
    pub(crate) fn target_of(&self, fb: u32, by_draw_buffer: bool) -> R<Target> {
        let Some(f) = self.objs.framebuffers.get(&fb) else { return Ok(Target::Surface) };
        if self.fb_status(fb) != FRAMEBUFFER_COMPLETE {
            return Err(INVALID_FRAMEBUFFER_OPERATION);
        }
        let color = if by_draw_buffer {
            f.draw_buffers
                .iter()
                .map(|&b| match b {
                    COLOR_ATTACHMENT0..=COLOR_ATTACHMENT7 => f.color[(b - COLOR_ATTACHMENT0) as usize].map(dev_att),
                    _ => None,
                })
                .collect()
        } else {
            f.color.iter().map(|a| a.map(dev_att)).collect()
        };
        Ok(Target::Fbo { color, depth_stencil: self.fb_depth_stencil(f).map(dev_att) })
    }

    pub(crate) fn framebuffer_call(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        match name {
            "createFramebuffer" => {
                let h = self.new_handle();
                self.objs.framebuffers.insert(h, Framebuffer::new());
                Ok(Ret::Object(Kind::Framebuffer, h))
            }
            "isFramebuffer" => Ok(Ret::Bool(match obj(a, 0, Kind::Framebuffer) {
                Ok(Some(h)) => self.objs.framebuffers.get(&h).is_some_and(|f| f.bound_once),
                _ => false,
            })),
            "deleteFramebuffer" => {
                let Some(h) = obj(a, 0, Kind::Framebuffer)? else { return Ok(Ret::Undefined) };
                if self.objs.framebuffers.remove(&h).is_some() {
                    if self.st.draw_fb == h {
                        self.st.draw_fb = 0;
                    }
                    if self.st.read_fb == h {
                        self.st.read_fb = 0;
                    }
                }
                Ok(Ret::Undefined)
            }
            "bindFramebuffer" => {
                let target = uint(a, 0)?;
                self.fb_of(target)?;
                let f = obj(a, 1, Kind::Framebuffer)?;
                if let Some(h) = f {
                    self.objs.framebuffers.get_mut(&h).ok_or(INVALID_OPERATION)?.bound_once = true;
                }
                let h = f.unwrap_or(0);
                if target != READ_FRAMEBUFFER {
                    self.st.draw_fb = h;
                }
                if target != DRAW_FRAMEBUFFER {
                    self.st.read_fb = h;
                }
                Ok(Ret::Undefined)
            }
            "checkFramebufferStatus" => {
                let fb = self.fb_of(uint(a, 0)?)?;
                Ok(Ret::Num(self.fb_status(fb) as f64))
            }
            "framebufferTexture2D" => {
                let (target, attachment, textarget, level) = (uint(a, 0)?, uint(a, 1)?, uint(a, 2)?, int(a, 4)?);
                let fb = self.fb_of(target)?;
                let p = self.point(attachment)?;
                let want = match textarget {
                    TEXTURE_2D => TexKind::D2,
                    TEXTURE_CUBE_MAP_POSITIVE_X..=TEXTURE_CUBE_MAP_NEGATIVE_Z => TexKind::Cube,
                    _ => return Err(INVALID_ENUM),
                };
                let tex = obj(a, 3, Kind::Texture)?;
                let att = match tex {
                    None => None,
                    Some(h) => {
                        let t = self.objs.textures.get(&h).ok_or(INVALID_OPERATION)?;
                        if t.kind != Some(want) {
                            return Err(INVALID_OPERATION);
                        }
                        if level < 0 || (!self.is2() && level != 0) || level as usize >= MAX_LEVELS {
                            return Err(INVALID_VALUE);
                        }
                        Some(Att::Tex { id: h, level: level as u32, face: face_index(textarget), layer: 0 })
                    }
                };
                self.attach(fb, p, att)
            }
            "framebufferRenderbuffer" => {
                let (target, attachment, rbt) = (uint(a, 0)?, uint(a, 1)?, uint(a, 2)?);
                let fb = self.fb_of(target)?;
                let p = self.point(attachment)?;
                if rbt != RENDERBUFFER {
                    return Err(INVALID_ENUM);
                }
                let att = match obj(a, 3, Kind::Renderbuffer)? {
                    None => None,
                    Some(h) if self.objs.renderbuffers.contains_key(&h) => Some(Att::Rb(h)),
                    Some(_) => return Err(INVALID_OPERATION),
                };
                self.attach(fb, p, att)
            }
            "framebufferTextureLayer" => {
                let (target, attachment, level, layer) = (uint(a, 0)?, uint(a, 1)?, int(a, 3)?, int(a, 4)?);
                let fb = self.fb_of(target)?;
                let p = self.point(attachment)?;
                let att = match obj(a, 2, Kind::Texture)? {
                    None => None,
                    Some(h) => {
                        let t = self.objs.textures.get(&h).ok_or(INVALID_OPERATION)?;
                        let max_layer = match t.kind {
                            Some(TexKind::D3) => crate::limits::MAX_3D_TEXTURE_SIZE,
                            Some(TexKind::Array) => crate::limits::MAX_ARRAY_TEXTURE_LAYERS,
                            _ => return Err(INVALID_OPERATION),
                        };
                        if level < 0 || layer < 0 || layer as u32 >= max_layer || level as usize >= MAX_LEVELS {
                            return Err(INVALID_VALUE);
                        }
                        Some(Att::Tex { id: h, level: level as u32, face: 0, layer: layer as u32 })
                    }
                };
                self.attach(fb, p, att)
            }
            _ => self.fb_query_call(name, a),
        }
    }

    fn attach(&mut self, fb: u32, p: Point, att: Option<Att>) -> R<Ret> {
        let webgl2 = self.is2();
        let f = self.objs.framebuffers.get_mut(&fb).ok_or(INVALID_OPERATION)?;
        match p {
            Point::Color(i) => f.color[i] = att,
            Point::Depth => f.depth = att,
            Point::Stencil => f.stencil = att,
            Point::DepthStencil if webgl2 => {
                f.depth = att;
                f.stencil = att;
            }
            Point::DepthStencil => f.depth_stencil = att,
        }
        Ok(Ret::Undefined)
    }
}
