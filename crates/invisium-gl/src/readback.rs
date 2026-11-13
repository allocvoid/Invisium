//! `readPixels`, `blitFramebuffer`, `copyTexImage2D`, `copyTexSubImage2D/3D`.

use crate::args::*;
use crate::cmd::{Attachment, Cmd, GlDevice, Target, TexFormat, TexKind};
use crate::consts1::*;
use crate::consts2::*;
use crate::fbo::dev_att;
use crate::formats::{channels, class, sized, Class};
use crate::objects::*;
use crate::pixels::{decode_dev, encode, expand, pick, quantize};
use crate::{Arg, Context, Elem, Ret};

/// The colour image reads come from.
pub struct ReadSrc {
    pub target: Target,
    pub color: u32,
    pub fmt: TexFormat,
    pub internal: u32,
    pub w: u32,
    pub h: u32,
    pub att: Option<Att>,
}

/// Clip `[x, x + w)` to `[0, max)`: (first inside, count), all as i64.
fn clip(x: i64, w: i64, max: u32) -> (i64, i64) {
    let a = x.max(0);
    let b = (x + w).min(max as i64);
    (a, (b - a).max(0))
}

impl Context {
    pub(crate) fn read_src(&self) -> R<ReadSrc> {
        let target = self.target_of(self.st.read_fb, false)?;
        match self.objs.framebuffers.get(&self.st.read_fb) {
            None => {
                if self.st.surface_read == NONE {
                    return Err(INVALID_OPERATION);
                }
                let internal = if self.attrs.alpha { RGBA } else { RGB };
                Ok(ReadSrc { target, color: 0, fmt: TexFormat::Rgba8, internal, w: self.size.0, h: self.size.1, att: None })
            }
            Some(f) => {
                let idx = f.read_buffer.checked_sub(COLOR_ATTACHMENT0).filter(|&i| i < 8).ok_or(INVALID_OPERATION)?;
                let att = f.color[idx as usize].ok_or(INVALID_OPERATION)?;
                let i = self.att_info(&att).ok_or(INVALID_FRAMEBUFFER_OPERATION)?;
                Ok(ReadSrc { target, color: idx, fmt: i.fmt, internal: i.internal, w: i.w, h: i.h, att: Some(att) })
            }
        }
    }

    pub(crate) fn read_pixels(&mut self, dev: &mut dyn GlDevice, a: &[Arg]) -> R<Ret> {
        let (x, y, w, h) = (int(a, 0)? as i64, int(a, 1)? as i64, int(a, 2)?, int(a, 3)?);
        let (format, ty) = (uint(a, 4)?, uint(a, 5)?);
        if !crate::formats::is_format_enum(format) || !crate::formats::is_type_enum(ty) {
            return Err(INVALID_ENUM);
        }
        let pbo = self.st.pixel_pack;
        let (view, dst_off) = match arg(a, 6) {
            Arg::Num(_) if pbo != 0 => (None, long(a, 6)?),
            Arg::Bytes(e, b) if pbo == 0 => {
                let ok = match ty {
                    UNSIGNED_BYTE => matches!(e, Elem::U8 | Elem::U8Clamped),
                    FLOAT => e == Elem::F32,
                    INT => e == Elem::I32,
                    UNSIGNED_INT => e == Elem::U32,
                    _ => true,
                };
                if !ok {
                    return Err(INVALID_OPERATION);
                }
                let off = if self.is2() { uint(a, 7)? as i64 } else { 0 };
                (Some((e, b)), off * elem_size(e) as i64)
            }
            Arg::Null if pbo == 0 => return Err(INVALID_VALUE),
            _ => return Err(INVALID_OPERATION),
        };
        if w < 0 || h < 0 {
            return Err(INVALID_VALUE);
        }
        let src = self.read_src()?;
        let allowed = match class(src.fmt) {
            Class::Norm => format == RGBA && ty == UNSIGNED_BYTE,
            Class::Float => format == RGBA && ty == FLOAT,
            Class::Int => format == RGBA_INTEGER && ty == INT,
            Class::Uint => format == RGBA_INTEGER && ty == UNSIGNED_INT,
            Class::Depth => false,
        };
        if !allowed {
            return Err(INVALID_OPERATION);
        }
        let (w, h) = (w as i64, h as i64);
        let bpp: i64 = if ty == UNSIGNED_BYTE { 4 } else { 16 };
        let p = self.st.pack;
        let row_len = if p.row_length > 0 { p.row_length as i64 } else { w };
        let al = p.alignment.max(1) as i64;
        let stride = (row_len * bpp + al - 1) / al * al;
        let skip128 = p.skip_rows as i128 * stride as i128 + p.skip_pixels as i128 * bpp as i128;
        let need128 = if w == 0 || h == 0 { 0 } else { skip128 + (h as i128 - 1) * stride as i128 + w as i128 * bpp as i128 };
        if dst_off < 0 {
            return Err(INVALID_VALUE);
        }
        let avail = match view {
            Some((_, b)) => b.len() as i64 - dst_off,
            None => self.objs.buffers.get(&pbo).map(|b| b.size as i64).unwrap_or(0) - dst_off,
        };
        if need128 > avail as i128 {
            return Err(INVALID_OPERATION);
        }
        let (skip, need) = (skip128 as i64, need128 as i64);
        if need == 0 {
            return Ok(Ret::Undefined);
        }
        let start = (dst_off + skip) as usize;
        let len = (need - skip) as usize;
        let mut out = match view {
            Some((_, b)) => b[start..start + len].to_vec(),
            None => self.buffer_bytes(dev, pbo, start, len).unwrap_or_else(|| vec![0; len]),
        };
        let (cx, cw) = clip(x, w, src.w);
        let (cy, ch) = clip(y, h, src.h);
        if cw > 0 && ch > 0 {
            self.flush(dev);
            let rect = [cx as i32, cy as i32, cw as i32, ch as i32];
            if let Some((fmt, bytes)) = dev.read_pixels(self.id, &src.target, src.color, rect) {
                let tb = fmt.bytes() as usize;
                for j in 0..ch {
                    for i in 0..cw {
                        let s = ((j * cw + i) as usize) * tb;
                        let Some(px) = bytes.get(s..s + tb) else { continue };
                        let v = decode_dev(fmt, px);
                        let o = ((cy + j - y) * stride + (cx + i - x) * bpp) as usize;
                        let mut enc = Vec::with_capacity(16);
                        for c in v {
                            match ty {
                                UNSIGNED_BYTE => enc.push((c.clamp(0.0, 1.0) * 255.0).round() as u8),
                                FLOAT => enc.extend_from_slice(&(c as f32).to_le_bytes()),
                                INT => enc.extend_from_slice(&(c as i32).to_le_bytes()),
                                _ => enc.extend_from_slice(&(c.max(0.0) as u32).to_le_bytes()),
                            }
                        }
                        out[o..o + enc.len()].copy_from_slice(&enc);
                    }
                }
            }
        }
        match view {
            Some(_) => Ok(Ret::Fill { arg: 6, offset: start, bytes: out }),
            None => {
                if let Some(d) = self.objs.buffers.get_mut(&pbo).and_then(|b| b.data_mut()) {
                    d[start..start + len].copy_from_slice(&out);
                }
                self.push(Cmd::BufferWrite { id: pbo, offset: start as u32, bytes: out });
                Ok(Ret::Undefined)
            }
        }
    }

    pub(crate) fn blit(&mut self, a: &[Arg]) -> R<Ret> {
        let mut c = [0i64; 8];
        for (i, v) in c.iter_mut().enumerate() {
            *v = int(a, i)? as i64;
        }
        let (mask, filter) = (uint(a, 8)?, uint(a, 9)?);
        if mask & !(COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT | STENCIL_BUFFER_BIT) != 0 {
            return Err(INVALID_VALUE);
        }
        if filter != NEAREST && filter != LINEAR {
            return Err(INVALID_ENUM);
        }
        let linear = filter == LINEAR;
        if linear && mask & (DEPTH_BUFFER_BIT | STENCIL_BUFFER_BIT) != 0 {
            return Err(INVALID_OPERATION);
        }
        let src_t = self.target_of(self.st.read_fb, false)?;
        let dst_t = self.target_of(self.st.draw_fb, true)?;
        let rect = |x0: i64, y0: i64, x1: i64, y1: i64| -> R<[i32; 4]> {
            let v = [x0, y0, x1 - x0, y1 - y0];
            if v.iter().any(|&n| n < i32::MIN as i64 || n > i32::MAX as i64) {
                return Err(INVALID_VALUE);
            }
            Ok(v.map(|n| n as i32))
        };
        let (src_rect, dst_rect) = (rect(c[0], c[1], c[2], c[3])?, rect(c[4], c[5], c[6], c[7])?);
        let read_samples = match self.objs.framebuffers.get(&self.st.read_fb) {
            None => if self.attrs.antialias { 4 } else { 0 },
            Some(f) => f.all().find_map(|a| self.att_info(&a)).map(|i| i.samples).unwrap_or(0),
        };
        let draw_samples = match self.objs.framebuffers.get(&self.st.draw_fb) {
            None => if self.attrs.antialias { 4 } else { 0 },
            Some(f) => f.all().find_map(|a| self.att_info(&a)).map(|i| i.samples).unwrap_or(0),
        };
        let mut color = mask & COLOR_BUFFER_BIT != 0;
        let mut src_color = 0;
        if color {
            match self.read_src() {
                Ok(src) => {
                    src_color = src.color;
                    let sc = class(src.fmt);
                    for i in 0..crate::limits::MAX_DRAW_BUFFERS as usize {
                        let Some(df) = self.draw_buffer_format(i) else { continue };
                        let dc = class(df);
                        let int_mismatch = (sc == Class::Int) != (dc == Class::Int) || (sc == Class::Uint) != (dc == Class::Uint);
                        if int_mismatch || (linear && matches!(sc, Class::Int | Class::Uint)) || (read_samples > 0 && df != src.fmt) {
                            return Err(INVALID_OPERATION);
                        }
                        if let (Some(sa), Some(f)) = (src.att, self.objs.framebuffers.get(&self.st.draw_fb)) {
                            let idx = f.draw_buffers[i].wrapping_sub(COLOR_ATTACHMENT0) as usize;
                            if f.color.get(idx).copied().flatten().map(dev_att) == Some(dev_att(sa)) {
                                return Err(INVALID_OPERATION);
                            }
                        }
                    }
                }
                Err(_) => color = false,
            }
        }
        let ds = |fb: u32| -> Option<(Att, TexFormat)> {
            let f = self.objs.framebuffers.get(&fb)?;
            let a = self.fb_depth_stencil(f)?;
            Some((a, self.att_info(&a)?.fmt))
        };
        let surface_ds = |s: &Self| Some(if s.attrs.stencil { TexFormat::Depth24Stencil8 } else { TexFormat::Depth24 });
        let (sd, dd) = (ds(self.st.read_fb), ds(self.st.draw_fb));
        let sfmt = if self.st.read_fb == 0 { surface_ds(self).filter(|_| self.attrs.depth || self.attrs.stencil) } else { sd.map(|x| x.1) };
        let dfmt = if self.st.draw_fb == 0 { surface_ds(self).filter(|_| self.attrs.depth || self.attrs.stencil) } else { dd.map(|x| x.1) };
        let mut depth = mask & DEPTH_BUFFER_BIT != 0 && sfmt.is_some_and(|f| f.has_depth()) && dfmt.is_some_and(|f| f.has_depth());
        let mut stencil = mask & STENCIL_BUFFER_BIT != 0 && sfmt.is_some_and(|f| f.has_stencil()) && dfmt.is_some_and(|f| f.has_stencil());
        if (depth || stencil) && sfmt != dfmt {
            return Err(INVALID_OPERATION);
        }
        if (depth || stencil) && sd.is_some() && sd.map(|x| dev_att(x.0)) == dd.map(|x| dev_att(x.0)) {
            return Err(INVALID_OPERATION);
        }
        if draw_samples > 0 || (read_samples > 0 && src_rect != dst_rect) {
            return Err(INVALID_OPERATION);
        }
        depth &= self.st.read_fb != 0 || self.attrs.depth;
        stencil &= self.st.read_fb != 0 || self.attrs.stencil;
        if !color && !depth && !stencil {
            return Ok(Ret::Undefined);
        }
        if dst_t == Target::Surface {
            self.drawn = true;
        }
        self.push(Cmd::Blit { src: src_t, src_color, src_rect, dst: dst_t, dst_rect, color, depth, stencil, linear });
        Ok(Ret::Undefined)
    }

    /// Shared part of the texture copies: checks, then `CopyToTexture` of the part inside the
    /// read framebuffer.
    #[allow(clippy::too_many_arguments)]
    fn copy_into(&mut self, dev: &mut dyn GlDevice, h: u32, level: u32, layer: u32, img: &Image, dx: i64, dy: i64, x: i64, y: i64, w: i64, hh: i64) -> R<()> {
        let src = self.read_src()?;
        let (sc, dc) = (class(src.fmt), class(img.fmt));
        let need = channels(img.internal);
        let have = channels(src.internal);
        let compat = match (sc, dc) {
            (Class::Norm, Class::Norm) | (Class::Float, Class::Float) | (Class::Int, Class::Int) | (Class::Uint, Class::Uint) => true,
            (Class::Float, Class::Norm) => !self.is2(),
            _ => false,
        };
        if !compat || need.iter().zip(have).any(|(n, h)| *n && !h) {
            return Err(INVALID_OPERATION);
        }
        let dst = Attachment { texture: h, level, layer };
        if src.att.map(dev_att) == Some(dst) {
            return Err(INVALID_OPERATION);
        }
        let (cx, cw) = clip(x, w, src.w);
        let (cy, ch) = clip(y, hh, src.h);
        if cw == 0 || ch == 0 || !img.on_dev {
            return Ok(());
        }
        let (dst_x, dst_y) = ((dx + cx - x) as u32, (dy + cy - y) as u32);
        // Destinations with fewer GL channels than device channels are converted on the CPU,
        // exactly as an upload of their format would be.
        let reduced = matches!(img.format, LUMINANCE | ALPHA | LUMINANCE_ALPHA)
            || (matches!(img.format, RGB | RGB_INTEGER) && img.fmt != TexFormat::R11G11B10F);
        if !reduced {
            self.push(Cmd::CopyToTexture { src: src.target, src_color: src.color, src_x: cx as u32, src_y: cy as u32, dst, dst_x, dst_y, width: cw as u32, height: ch as u32 });
            return Ok(());
        }
        self.flush(dev);
        let rect = [cx as i32, cy as i32, cw as i32, ch as i32];
        let Some((fmt, bytes)) = dev.read_pixels(self.id, &src.target, src.color, rect) else { return Ok(()) };
        let count = (cw * ch) as usize;
        let tb = fmt.bytes() as usize;
        if bytes.len() < count * tb {
            return Ok(());
        }
        let mut out = Vec::with_capacity(count * img.fmt.bytes() as usize);
        for px in bytes.chunks_exact(tb).take(count) {
            let v = expand(img.format, pick(img.format, decode_dev(fmt, px)));
            encode(img.fmt, quantize(img.ty, v), &mut out);
        }
        self.push(Cmd::TextureWrite { id: h, level, layer, x: dst_x, y: dst_y, width: cw as u32, height: ch as u32, depth: 1, bytes: out });
        Ok(())
    }

    pub(crate) fn copy_tex_call(&mut self, dev: &mut dyn GlDevice, name: &str, a: &[Arg]) -> R<Ret> {
        let target = uint(a, 0)?;
        let three = name == "copyTexSubImage3D";
        let (kind, face) = match target {
            TEXTURE_2D if !three => (TexKind::D2, 0),
            TEXTURE_CUBE_MAP_POSITIVE_X..=TEXTURE_CUBE_MAP_NEGATIVE_Z if !three => (TexKind::Cube, face_index(target)),
            TEXTURE_3D if three => (TexKind::D3, 0),
            TEXTURE_2D_ARRAY if three => (TexKind::Array, 0),
            _ => return Err(INVALID_ENUM),
        };
        let level = int(a, 1)?;
        let h = self.bound_tex(bind_target(target))?;
        if name == "copyTexImage2D" {
            let internal = uint(a, 2)?;
            let (x, y, w, hh, border) = (int(a, 3)?, int(a, 4)?, int(a, 5)?, int(a, 6)?, int(a, 7)?);
            let (fmt, format, ty) = match internal {
                RGBA | RGB | LUMINANCE | LUMINANCE_ALPHA | ALPHA => (TexFormat::Rgba8, internal, UNSIGNED_BYTE),
                _ if self.is2() => match sized(internal) {
                    Some(s) if class(s.fmt) != Class::Depth => (s.fmt, s.format, s.types[0]),
                    Some(_) => return Err(INVALID_OPERATION),
                    None => return Err(INVALID_ENUM),
                },
                _ => return Err(INVALID_ENUM),
            };
            let max = crate::limits::MAX_TEXTURE_SIZE as i32;
            if !(0..=13).contains(&level) || w < 0 || hh < 0 || border != 0 || w > max >> level || hh > max >> level || (kind == TexKind::Cube && w != hh) {
                return Err(INVALID_VALUE);
            }
            if !self.is2() && level > 0 && (!(w as u32).is_power_of_two() || !(hh as u32).is_power_of_two()) {
                return Err(INVALID_VALUE);
            }
            if self.objs.textures.get(&h).is_some_and(|t| t.immutable) {
                return Err(INVALID_OPERATION);
            }
            self.read_src()?;
            let img = Image { w: w as u32, h: hh as u32, d: 1, internal, format, ty, fmt, on_dev: false };
            let probe = Image { on_dev: true, ..img };
            self.copy_check_only(&probe, Attachment { texture: h, level: level as u32, layer: face })?;
            self.define_level(h, kind, level as usize, face as usize, img, None)?;
            let img = *self.objs.textures.get(&h).and_then(|t| t.image(level as usize, face as usize)).ok_or(INVALID_OPERATION)?;
            self.copy_into(dev, h, level as u32, face, &img, 0, 0, x as i64, y as i64, w as i64, hh as i64)?;
            return Ok(Ret::Undefined);
        }
        let (xo, yo) = (int(a, 2)? as i64, int(a, 3)? as i64);
        let (zo, i) = if three { (int(a, 4)? as i64, 5) } else { (0, 4) };
        let (x, y, w, hh) = (int(a, i)? as i64, int(a, i + 1)? as i64, int(a, i + 2)? as i64, int(a, i + 3)? as i64);
        if level < 0 || level as usize >= MAX_LEVELS || xo < 0 || yo < 0 || zo < 0 || w < 0 || hh < 0 {
            return Err(INVALID_VALUE);
        }
        let img = *self.objs.textures.get(&h).and_then(|t| t.image(level as usize, face as usize)).ok_or(INVALID_OPERATION)?;
        if xo + w > img.w as i64 || yo + hh > img.h as i64 || zo >= img.d as i64 {
            return Err(INVALID_VALUE);
        }
        if class(img.fmt) == Class::Depth {
            return Err(INVALID_OPERATION);
        }
        let layer = if three { zo as u32 } else { face };
        self.copy_into(dev, h, level as u32, layer, &img, xo, yo, x, y, w, hh)?;
        Ok(Ret::Undefined)
    }

    /// Format compatibility of a copy before the destination image is (re)defined.
    fn copy_check_only(&self, img: &Image, dst: Attachment) -> R<()> {
        let src = self.read_src()?;
        if src.att.map(dev_att) == Some(dst) {
            return Err(INVALID_OPERATION);
        }
        let (sc, dc) = (class(src.fmt), class(img.fmt));
        let ok = sc == dc || (sc == Class::Float && dc == Class::Norm && !self.is2());
        if !ok || channels(img.internal).iter().zip(channels(src.internal)).any(|(n, h)| *n && !h) {
            return Err(INVALID_OPERATION);
        }
        Ok(())
    }
}
