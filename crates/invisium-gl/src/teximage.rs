//! `texImage2D/3D`, `texSubImage2D/3D`: overload resolution, validation, device uploads.

use crate::args::*;
use crate::cmd::{Cmd, GlDevice, TexDesc, TexKind};
use crate::consts1::*;
use crate::consts2::*;
use crate::formats::{class, sized, tex_format, Class, Feat};
use crate::objects::*;
use crate::textures::level_dims;
use crate::upload::{unpack_pixels, unpack_view, zeros, Spec};
use crate::{Arg, Context, Elem, Ret};

enum Source<'a> {
    None,
    View(&'a [u8]),
    Pbo(i64),
    Dom { w: u32, h: u32, rgba: &'a [u8] },
}

fn elem_ok(ty: u32, e: Elem) -> bool {
    match ty {
        UNSIGNED_BYTE => matches!(e, Elem::U8 | Elem::U8Clamped),
        BYTE => e == Elem::I8,
        UNSIGNED_SHORT | UNSIGNED_SHORT_4_4_4_4 | UNSIGNED_SHORT_5_5_5_1 | UNSIGNED_SHORT_5_6_5 | HALF_FLOAT | HALF_FLOAT_OES => e == Elem::U16,
        SHORT => e == Elem::I16,
        UNSIGNED_INT | UNSIGNED_INT_2_10_10_10_REV | UNSIGNED_INT_10F_11F_11F_REV | UNSIGNED_INT_5_9_9_9_REV | UNSIGNED_INT_24_8 => e == Elem::U32,
        INT => e == Elem::I32,
        FLOAT => e == Elem::F32,
        _ => false,
    }
}

/// A box of texels: offset and size.
#[derive(Clone, Copy)]
struct Region {
    x: u32,
    y: u32,
    z: u32,
    w: u32,
    h: u32,
    d: u32,
}

impl Context {
    pub(crate) fn feat(&self) -> Feat {
        Feat { webgl2: self.is2(), tex_float: self.ext("OES_texture_float"), tex_half: self.ext("OES_texture_half_float"), depth_tex: self.ext("WEBGL_depth_texture") }
    }

    /// The pixel source at argument `i` (with WebGL 2's `srcOffset` at `i + 1`).
    fn source<'a>(&self, a: &[Arg<'a>], i: usize, ty: u32) -> R<Source<'a>> {
        let pbo = self.st.pixel_unpack != 0;
        let src = match arg(a, i) {
            Arg::Null | Arg::Undefined if !pbo => Source::None,
            Arg::Bytes(e, b) if !pbo => {
                if !elem_ok(ty, e) {
                    return Err(INVALID_OPERATION);
                }
                let es = elem_size(e);
                let off = if self.is2() { uint(a, i + 1)? as usize } else { 0 };
                if off > b.len() / es {
                    return Err(INVALID_OPERATION);
                }
                Source::View(&b[off * es..])
            }
            Arg::Num(_) if pbo => Source::Pbo(long(a, i)?),
            Arg::Pixels { width, height, rgba } if !pbo && self.is2() => Source::Dom { w: width, h: height, rgba },
            _ => return Err(INVALID_OPERATION),
        };
        Ok(src)
    }

    /// Device bytes for a region, or `None` for "no data".
    fn convert(&self, src: &Source, s: Spec, r: Region, three_d: bool, sized_dom: bool) -> R<Option<Vec<u8>>> {
        if class(s.fmt) == Class::Depth {
            return Ok(None);
        }
        let u = &self.st.unpack;
        Ok(match *src {
            Source::None => None,
            Source::View(b) => Some(unpack_view(u, b, s, r.w, r.h, r.d, three_d)?),
            Source::Dom { w, h, rgba } => Some(unpack_pixels(u, w, h, rgba, s, r.w, r.h, r.d, sized_dom)?),
            Source::Pbo(_) => return Err(INVALID_OPERATION),
        })
    }

    /// The pixel unpack buffer's bytes from `off` on (read back from the device).
    fn pbo_bytes(&mut self, dev: &mut dyn GlDevice, off: i64, ty: u32) -> R<Vec<u8>> {
        if off < 0 {
            return Err(INVALID_VALUE);
        }
        if off % crate::pixels::type_size(ty) as i64 != 0 {
            return Err(INVALID_OPERATION);
        }
        let h = self.st.pixel_unpack;
        let size = self.objs.buffers.get(&h).map(|b| b.size).unwrap_or(0) as i64;
        if off > size {
            return Err(INVALID_OPERATION);
        }
        self.buffer_bytes(dev, h, off as usize, (size - off) as usize).ok_or(INVALID_OPERATION)
    }

    pub(crate) fn teximage_call(&mut self, dev: &mut dyn GlDevice, name: &str, a: &[Arg]) -> R<Ret> {
        match name {
            "texImage2D" => self.tex_image(dev, a, false),
            "texImage3D" => self.tex_image(dev, a, true),
            "texSubImage2D" => self.tex_sub_image(dev, a, false),
            "texSubImage3D" => self.tex_sub_image(dev, a, true),
            _ => Err(INVALID_ENUM),
        }
    }

    fn target_2d(&self, target: u32) -> R<(TexKind, usize)> {
        match target {
            TEXTURE_2D => Ok((TexKind::D2, 0)),
            TEXTURE_CUBE_MAP_POSITIVE_X..=TEXTURE_CUBE_MAP_NEGATIVE_Z => Ok((TexKind::Cube, face_index(target) as usize)),
            _ => Err(INVALID_ENUM),
        }
    }

    fn target_3d(&self, target: u32) -> R<TexKind> {
        match target {
            TEXTURE_3D => Ok(TexKind::D3),
            TEXTURE_2D_ARRAY => Ok(TexKind::Array),
            _ => Err(INVALID_ENUM),
        }
    }

    fn tex_image(&mut self, dev: &mut dyn GlDevice, a: &[Arg], three: bool) -> R<Ret> {
        let target = uint(a, 0)?;
        let (kind, face) = if three { (self.target_3d(target)?, 0) } else { self.target_2d(target)? };
        let level = int(a, 1)?;
        let internal = int(a, 2)? as u32;
        let dom6 = !three && matches!(arg(a, 5), Arg::Pixels { .. });
        let (w, h, d, border, fi) = if dom6 {
            let Arg::Pixels { width, height, .. } = arg(a, 5) else { return Err(INVALID_OPERATION) };
            (width.min(i32::MAX as u32) as i32, height.min(i32::MAX as u32) as i32, 1, 0, 3)
        } else if three {
            (int(a, 3)?, int(a, 4)?, int(a, 5)?, int(a, 6)?, 7)
        } else {
            (int(a, 3)?, int(a, 4)?, 1, int(a, 5)?, 6)
        };
        let (format, ty) = (uint(a, fi)?, uint(a, fi + 1)?);
        let h_tex = self.bound_tex(bind_target(target))?;
        let fmt = tex_format(self.feat(), internal, format, ty)?;
        let max = if kind == TexKind::D3 { crate::limits::MAX_3D_TEXTURE_SIZE } else { crate::limits::MAX_TEXTURE_SIZE };
        let max_level = 31 - max.leading_zeros() as i32;
        if level < 0 || w < 0 || h < 0 || d < 0 || level > max_level || border != 0 {
            return Err(INVALID_VALUE);
        }
        let lmax = (max >> level) as i32;
        let dmax = match kind {
            TexKind::D3 => lmax,
            TexKind::Array => crate::limits::MAX_ARRAY_TEXTURE_LAYERS as i32,
            _ => 1,
        };
        if w > lmax || h > lmax || d > dmax || (kind == TexKind::Cube && w != h) {
            return Err(INVALID_VALUE);
        }
        if !self.is2() && level > 0 && (!(w as u32).is_power_of_two() || !(h as u32).is_power_of_two()) {
            return Err(INVALID_VALUE);
        }
        let depth = class(fmt) == Class::Depth;
        let src = if dom6 { self.dom(a, 5)? } else { self.source(a, fi + 2, ty)? };
        if depth && (kind == TexKind::D3 || (!self.is2() && (kind == TexKind::Cube || level != 0 || !matches!(src, Source::None)))) {
            return Err(INVALID_OPERATION);
        }
        if depth && matches!(src, Source::Dom { .. }) {
            return Err(INVALID_OPERATION);
        }
        if three && matches!(src, Source::View(_) | Source::Pbo(_)) && (self.st.unpack.flip_y || self.st.unpack.premultiply) {
            return Err(INVALID_OPERATION);
        }
        if self.objs.textures.get(&h_tex).is_some_and(|t| t.immutable) {
            return Err(INVALID_OPERATION);
        }
        let r = Region { x: 0, y: 0, z: 0, w: w as u32, h: h as u32, d: d as u32 };
        let pbo;
        let src = match src {
            Source::Pbo(off) if !depth => {
                pbo = self.pbo_bytes(dev, off, ty)?;
                Source::View(&pbo)
            }
            Source::Pbo(_) => Source::None,
            s => s,
        };
        let data = self.convert(&src, Spec { format, ty, fmt }, r, three, !dom6)?;
        let img = Image { w: r.w, h: r.h, d: r.d, internal, format, ty, fmt, on_dev: false };
        self.define_level(h_tex, kind, level as usize, face, img, data)?;
        Ok(Ret::Undefined)
    }

    /// The DOM forms sized by their source (never with a pixel unpack buffer bound).
    fn dom<'a>(&self, a: &[Arg<'a>], i: usize) -> R<Source<'a>> {
        match arg(a, i) {
            Arg::Pixels { width, height, rgba } if self.st.pixel_unpack == 0 => Ok(Source::Dom { w: width, h: height, rgba }),
            _ => Err(INVALID_OPERATION),
        }
    }

    fn tex_sub_image(&mut self, dev: &mut dyn GlDevice, a: &[Arg], three: bool) -> R<Ret> {
        let target = uint(a, 0)?;
        let (_, face) = if three { (self.target_3d(target)?, 0) } else { self.target_2d(target)? };
        let level = int(a, 1)?;
        let dom7 = !three && matches!(arg(a, 6), Arg::Pixels { .. });
        let (x, y, z) = (int(a, 2)?, int(a, 3)?, if three { int(a, 4)? } else { 0 });
        let (w, h, d, fi) = if dom7 {
            let Arg::Pixels { width, height, .. } = arg(a, 6) else { return Err(INVALID_OPERATION) };
            (width.min(i32::MAX as u32) as i32, height.min(i32::MAX as u32) as i32, 1, 4)
        } else if three {
            (int(a, 5)?, int(a, 6)?, int(a, 7)?, 8)
        } else {
            (int(a, 4)?, int(a, 5)?, 1, 6)
        };
        let (format, ty) = (uint(a, fi)?, uint(a, fi + 1)?);
        if !crate::formats::is_format_enum(format) || !crate::formats::is_type_enum(ty) {
            return Err(INVALID_ENUM);
        }
        let h_tex = self.bound_tex(bind_target(target))?;
        if level < 0 || x < 0 || y < 0 || z < 0 || w < 0 || h < 0 || d < 0 || level as usize >= MAX_LEVELS {
            return Err(INVALID_VALUE);
        }
        let img = *self.objs.textures.get(&h_tex).and_then(|t| t.image(level as usize, face)).ok_or(INVALID_OPERATION)?;
        let fits = |o: i32, n: i32, max: u32| (o as i64 + n as i64) <= max as i64;
        if !fits(x, w, img.w) || !fits(y, h, img.h) || !fits(z, d, img.d) {
            return Err(INVALID_VALUE);
        }
        let matches = if sized(img.internal).is_some() { tex_format(self.feat(), img.internal, format, ty).is_ok() } else { format == img.format && ty == img.ty };
        if !matches {
            return Err(INVALID_OPERATION);
        }
        let src = if dom7 { self.dom(a, 6)? } else { self.source(a, fi + 2, ty)? };
        if class(img.fmt) == Class::Depth && !self.is2() {
            return Err(INVALID_OPERATION);
        }
        if matches!(src, Source::None) {
            return Err(INVALID_VALUE);
        }
        if three && matches!(src, Source::View(_) | Source::Pbo(_)) && (self.st.unpack.flip_y || self.st.unpack.premultiply) {
            return Err(INVALID_OPERATION);
        }
        let r = Region { x: x as u32, y: y as u32, z: z as u32, w: w as u32, h: h as u32, d: d as u32 };
        if class(img.fmt) == Class::Depth || !img.on_dev {
            return Ok(Ret::Undefined);
        }
        let pbo;
        let src = match src {
            Source::Pbo(off) => {
                pbo = self.pbo_bytes(dev, off, ty)?;
                Source::View(&pbo)
            }
            s => s,
        };
        let data = self.convert(&src, Spec { format, ty, fmt: img.fmt }, r, three, !dom7)?;
        if let (true, Some(bytes)) = (img.on_dev, data) {
            self.write_region(h_tex, level as u32, if three { r.z } else { face as u32 }, r, bytes);
        }
        Ok(Ret::Undefined)
    }

    fn write_region(&mut self, id: u32, level: u32, layer: u32, r: Region, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.push(Cmd::TextureWrite { id, level, layer, x: r.x, y: r.y, width: r.w, height: r.h, depth: r.d, bytes });
        }
    }

    /// Define one image (level, face) of a mutable texture, managing the device texture.
    pub(crate) fn define_level(&mut self, h: u32, kind: TexKind, level: usize, face: usize, mut img: Image, data: Option<Vec<u8>>) -> R<()> {
        let t = self.objs.textures.get(&h).ok_or(INVALID_OPERATION)?;
        let dev = t.dev;
        let ddepth = if matches!(kind, TexKind::D3 | TexKind::Array) { img.d } else { 1 };
        let mut fresh = false;
        if level == 0 {
            let same = dev.is_some_and(|d| d.format == img.fmt && d.width == img.w && d.height == img.h && d.depth == ddepth);
            let others = (0..t.faces()).any(|f| f != face && t.image(0, f).is_some());
            let keep_cube = kind == TexKind::Cube && others && dev.is_some();
            if !same && !keep_cube {
                if img.w == 0 || img.h == 0 || img.d == 0 {
                    if dev.is_some() {
                        self.push(Cmd::TextureDelete { id: h });
                        self.release(h);
                    }
                    if let Some(t) = self.objs.textures.get_mut(&h) {
                        t.dev = None;
                    }
                } else {
                    let mut mips = 1;
                    while mips < MAX_LEVELS
                        && t.image(mips, face).is_some_and(|i| (i.w, i.h, i.d) == level_dims(kind, img.w, img.h, img.d, mips as u32) && i.fmt == img.fmt)
                    {
                        mips += 1;
                    }
                    let desc = TexDesc { kind, format: img.fmt, width: img.w, height: img.h, depth: ddepth, mips: mips as u32, samples: 1 };
                    self.create_dev_tex(h, desc)?;
                    fresh = true;
                }
            }
        } else {
            img.on_dev = false;
            if self.fits_dev(h, level as u32, &img) {
                fresh = self.grow_dev_tex(h, level as u32 + 1)?;
            }
        }
        img.on_dev = self.fits_dev(h, level as u32, &img);
        if let Some(t) = self.objs.textures.get_mut(&h) {
            t.images[level * 6 + face] = Some(img);
        }
        if fresh && level == 0 {
            for l in 1..MAX_LEVELS {
                for f in 0..6 {
                    let Some(mut i) = self.objs.textures.get(&h).and_then(|t| t.images[l * 6 + f]) else { continue };
                    i.on_dev = self.fits_dev(h, l as u32, &i);
                    if let Some(t) = self.objs.textures.get_mut(&h) {
                        t.images[l * 6 + f] = Some(i);
                    }
                }
            }
        }
        if img.on_dev && class(img.fmt) != Class::Depth {
            let bytes = match data {
                Some(b) => b,
                None if fresh => return Ok(()),
                None => zeros(img.fmt, img.w, img.h, img.d)?,
            };
            let layer = if kind == TexKind::Cube { face as u32 } else { 0 };
            let r = Region { x: 0, y: 0, z: 0, w: img.w, h: img.h, d: img.d };
            self.write_region(h, level as u32, layer, r, bytes);
        }
        Ok(())
    }
}
