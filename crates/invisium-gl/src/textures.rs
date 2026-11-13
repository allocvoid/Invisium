//! Texture objects: binding, parameters, storage on the device, mipmaps.

use crate::args::*;
use crate::cmd::{Cmd, TexDesc, TexKind};
use crate::consts1::*;
use crate::consts2::*;
use crate::formats::{class, sized, Class, Rend};
use crate::objects::*;
use crate::{Arg, Context, Kind, Ret};

fn unit_index(k: TexKind) -> usize {
    match k {
        TexKind::D2 => 0,
        TexKind::Cube => 1,
        TexKind::D3 => 2,
        TexKind::Array => 3,
    }
}

/// Size of `level` of a texture whose level 0 is `w` x `h` x `d`.
pub fn level_dims(kind: TexKind, w: u32, h: u32, d: u32, level: u32) -> (u32, u32, u32) {
    let sh = |v: u32| v.checked_shr(level).unwrap_or(0).max(1);
    (sh(w), sh(h), if kind == TexKind::D3 { sh(d) } else { d })
}

impl Context {
    /// Bind target of the texture APIs (`TEXTURE_2D`, `TEXTURE_CUBE_MAP`, and the WebGL 2 ones).
    pub(crate) fn tex_kind(&self, target: u32) -> R<TexKind> {
        match kind_of(target) {
            Some(k @ (TexKind::D2 | TexKind::Cube)) => Ok(k),
            Some(k) if self.is2() => Ok(k),
            _ => Err(INVALID_ENUM),
        }
    }

    /// The texture bound to `target` on the active unit (`INVALID_OPERATION` if none).
    pub(crate) fn bound_tex(&self, target: u32) -> R<u32> {
        let k = self.tex_kind(target)?;
        match self.st.units[self.st.active_unit as usize][unit_index(k)] {
            0 => Err(INVALID_OPERATION),
            h => Ok(h),
        }
    }

    pub(crate) fn texture_call(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        match name {
            "createTexture" => {
                let h = self.new_handle();
                self.objs.textures.insert(h, Texture::new());
                Ok(Ret::Object(Kind::Texture, h))
            }
            "isTexture" => Ok(Ret::Bool(match obj(a, 0, Kind::Texture) {
                Ok(Some(h)) => self.objs.textures.get(&h).map(|t| t.kind.is_some()).unwrap_or(false),
                _ => false,
            })),
            "deleteTexture" => {
                let Some(h) = obj(a, 0, Kind::Texture)? else { return Ok(Ret::Undefined) };
                if let Some(t) = self.objs.textures.remove(&h) {
                    for u in self.st.units.iter_mut() {
                        for s in u.iter_mut().filter(|s| **s == h) {
                            *s = 0;
                        }
                    }
                    for fb in self.objs.framebuffers.values_mut() {
                        fb.detach(|a| matches!(a, Att::Tex { id, .. } if *id == h));
                    }
                    if t.dev.is_some() {
                        self.push(Cmd::TextureDelete { id: h });
                    }
                    self.release(h);
                }
                Ok(Ret::Undefined)
            }
            "bindTexture" => {
                let k = self.tex_kind(uint(a, 0)?)?;
                let t = obj(a, 1, Kind::Texture)?;
                if let Some(h) = t {
                    let tex = self.objs.textures.get_mut(&h).ok_or(INVALID_OPERATION)?;
                    if tex.kind.is_some_and(|tk| tk != k) {
                        return Err(INVALID_OPERATION);
                    }
                    tex.kind = Some(k);
                }
                self.st.units[self.st.active_unit as usize][unit_index(k)] = t.unwrap_or(0);
                Ok(Ret::Undefined)
            }
            "texParameteri" | "texParameterf" => {
                let h = self.bound_tex(uint(a, 0)?)?;
                let pname = uint(a, 1)?;
                let f = float(a, 2)?;
                let i = if name == "texParameteri" { int(a, 2)? } else { f as i32 };
                self.tex_param(h, pname, i, f)?;
                Ok(Ret::Undefined)
            }
            "getTexParameter" => {
                let target = uint(a, 0)?;
                self.tex_kind(target)?;
                let pname = uint(a, 1)?;
                let h = self.bound_tex(target)?;
                self.get_tex_param(h, pname)
            }
            "generateMipmap" => self.generate_mipmap(uint(a, 0)?),
            "texStorage2D" | "texStorage3D" => self.tex_storage(name == "texStorage3D", a),
            _ => Ok(Ret::Undefined),
        }
    }

    fn tex_param(&mut self, h: u32, pname: u32, i: i32, f: f32) -> R<()> {
        let webgl2 = self.is2();
        let p = &mut self.objs.textures.get_mut(&h).ok_or(INVALID_OPERATION)?.params;
        set_param(p, pname, i, f, webgl2, false)
    }

    fn get_tex_param(&self, h: u32, pname: u32) -> R<Ret> {
        let t = self.objs.textures.get(&h).ok_or(INVALID_OPERATION)?;
        let webgl2 = self.is2();
        match pname {
            TEXTURE_IMMUTABLE_FORMAT if webgl2 => Ok(Ret::Bool(t.immutable)),
            TEXTURE_IMMUTABLE_LEVELS if webgl2 => Ok(Ret::Num(t.imm_levels as f64)),
            _ => get_param(&t.params, pname, webgl2, false),
        }
    }
}

/// Set a texture or sampler parameter (`sampler`: base and max level do not exist).
pub fn set_param(p: &mut TexParams, pname: u32, i: i32, f: f32, webgl2: bool, sampler: bool) -> R<()> {
        let e = i as u32;
        match pname {
            TEXTURE_BASE_LEVEL | TEXTURE_MAX_LEVEL if sampler => return Err(INVALID_ENUM),
            TEXTURE_MAG_FILTER if e == NEAREST || e == LINEAR => p.mag_filter = e,
            TEXTURE_MIN_FILTER if matches!(e, NEAREST | LINEAR | NEAREST_MIPMAP_NEAREST | LINEAR_MIPMAP_NEAREST | NEAREST_MIPMAP_LINEAR | LINEAR_MIPMAP_LINEAR) => {
                p.min_filter = e
            }
            TEXTURE_WRAP_S | TEXTURE_WRAP_T | TEXTURE_WRAP_R if matches!(e, REPEAT | CLAMP_TO_EDGE | MIRRORED_REPEAT) => {
                if pname == TEXTURE_WRAP_R && !webgl2 {
                    return Err(INVALID_ENUM);
                }
                p.wrap[match pname {
                    TEXTURE_WRAP_S => 0,
                    TEXTURE_WRAP_T => 1,
                    _ => 2,
                }] = e;
            }
            TEXTURE_BASE_LEVEL | TEXTURE_MAX_LEVEL if webgl2 => {
                if i < 0 {
                    return Err(INVALID_VALUE);
                }
                if pname == TEXTURE_BASE_LEVEL {
                    p.base_level = i;
                } else {
                    p.max_level = i;
                }
            }
            TEXTURE_MIN_LOD if webgl2 => p.min_lod = f,
            TEXTURE_MAX_LOD if webgl2 => p.max_lod = f,
            TEXTURE_COMPARE_MODE if webgl2 && (e == NONE || e == COMPARE_REF_TO_TEXTURE) => p.compare_mode = e,
            TEXTURE_COMPARE_FUNC if webgl2 && (NEVER..=ALWAYS).contains(&e) => p.compare_func = e,
            _ => return Err(INVALID_ENUM),
        }
        Ok(())
}

/// Read a texture or sampler parameter.
pub fn get_param(p: &TexParams, pname: u32, webgl2: bool, sampler: bool) -> R<Ret> {
        let n = |v: u32| Ok(Ret::Num(v as f64));
        match pname {
            TEXTURE_BASE_LEVEL | TEXTURE_MAX_LEVEL if sampler => Err(INVALID_ENUM),
            TEXTURE_MAG_FILTER => n(p.mag_filter),
            TEXTURE_MIN_FILTER => n(p.min_filter),
            TEXTURE_WRAP_S => n(p.wrap[0]),
            TEXTURE_WRAP_T => n(p.wrap[1]),
            TEXTURE_WRAP_R if webgl2 => n(p.wrap[2]),
            TEXTURE_BASE_LEVEL if webgl2 => Ok(Ret::Num(p.base_level as f64)),
            TEXTURE_MAX_LEVEL if webgl2 => Ok(Ret::Num(p.max_level as f64)),
            TEXTURE_MIN_LOD if webgl2 => Ok(Ret::Num(p.min_lod as f64)),
            TEXTURE_MAX_LOD if webgl2 => Ok(Ret::Num(p.max_lod as f64)),
            TEXTURE_COMPARE_MODE if webgl2 => n(p.compare_mode),
            TEXTURE_COMPARE_FUNC if webgl2 => n(p.compare_func),
            _ => Err(INVALID_ENUM),
        }
}

impl Context {

    /// Create (or replace) the device texture of `h`; the memory budget is checked first.
    pub(crate) fn create_dev_tex(&mut self, h: u32, desc: TexDesc) -> R<()> {
        self.reserve(h, crate::objects::tex_bytes(&desc))?;
        if let Some(t) = self.objs.textures.get_mut(&h) {
            t.dev = Some(desc);
        }
        self.push(Cmd::TextureCreate { id: h, desc });
        Ok(())
    }

    /// Give the device texture `mips` levels if it has fewer.
    pub(crate) fn grow_dev_tex(&mut self, h: u32, mips: u32) -> R<bool> {
        let Some(mut desc) = self.objs.textures.get(&h).and_then(|t| t.dev) else { return Ok(false) };
        if desc.mips >= mips {
            return Ok(false);
        }
        desc.mips = mips;
        self.reserve(h, crate::objects::tex_bytes(&desc))?;
        if let Some(t) = self.objs.textures.get_mut(&h) {
            t.dev = Some(desc);
        }
        self.push(Cmd::TextureGrow { id: h, mips });
        Ok(true)
    }

    /// Whether an image of `level` with these properties fits the device texture's chain.
    pub(crate) fn fits_dev(&self, h: u32, level: u32, img: &Image) -> bool {
        let Some(d) = self.objs.textures.get(&h).and_then(|t| t.dev) else { return false };
        let (w, hh, dd) = level_dims(d.kind, d.width, d.height, d.depth, level);
        (level as usize) < MAX_LEVELS && d.format == img.fmt && img.w == w && img.h == hh && img.d == dd
    }

    fn generate_mipmap(&mut self, target: u32) -> R<Ret> {
        let k = match target {
            TEXTURE_2D | TEXTURE_CUBE_MAP => self.tex_kind(target)?,
            TEXTURE_3D | TEXTURE_2D_ARRAY if self.is2() => self.tex_kind(target)?,
            _ => return Err(INVALID_ENUM),
        };
        let h = self.bound_tex(target)?;
        let webgl2 = self.is2();
        let t = self.objs.textures.get(&h).ok_or(INVALID_OPERATION)?;
        let mut base = t.params.base_level.max(0) as usize;
        if t.immutable {
            base = base.min(t.imm_levels.saturating_sub(1) as usize);
        }
        let img = *t.image(base, 0).ok_or(INVALID_OPERATION)?;
        for f in 1..t.faces() {
            let o = t.image(base, f).ok_or(INVALID_OPERATION)?;
            if o.w != img.w || o.h != img.h || o.internal != img.internal || o.fmt != img.fmt {
                return Err(INVALID_OPERATION);
            }
        }
        let ok_format = match class(img.fmt) {
            Class::Norm => match sized(img.internal) {
                Some(s) => s.rend == Rend::Yes && s.filterable,
                None => true,
            },
            Class::Float => webgl2 && sized(img.internal).is_some_and(|s| s.filterable && s.rend != Rend::No && (self.ext("EXT_color_buffer_float") || (s.rend != Rend::Float && self.ext("EXT_color_buffer_half_float")))),
            _ => false,
        };
        let pot = |v: u32| v.is_power_of_two();
        if !ok_format || (!webgl2 && (!pot(img.w) || !pot(img.h))) || (k == TexKind::Cube && img.w != img.h) {
            return Err(INVALID_OPERATION);
        }
        let depth_dim = if k == TexKind::D3 { img.d } else { 1 };
        let chain = full_levels(img.w, img.h, depth_dim) as usize;
        let mut last = (base + chain - 1).min(MAX_LEVELS - 1).min(t.params.max_level.max(0) as usize);
        if t.immutable {
            last = last.min(t.imm_levels as usize - 1);
        }
        let faces = t.faces();
        if let Some(d) = t.dev {
            if d.mips < last as u32 + 1 {
                self.grow_dev_tex(h, last as u32 + 1)?;
            }
        }
        for level in base + 1..=last {
            let (w, hh, dd) = level_dims(k, img.w, img.h, img.d, (level - base) as u32);
            let mut li = Image { w, h: hh, d: dd, ..img };
            li.on_dev = self.fits_dev(h, level as u32, &li);
            let t = self.objs.textures.get_mut(&h).ok_or(INVALID_OPERATION)?;
            for f in 0..faces {
                t.images[level * 6 + f] = Some(li);
            }
        }
        if self.objs.textures.get(&h).is_some_and(|t| t.dev.is_some()) && last > base {
            self.push(Cmd::GenerateMips { id: h });
        }
        Ok(Ret::Undefined)
    }

    fn tex_storage(&mut self, three: bool, a: &[Arg]) -> R<Ret> {
        let target = uint(a, 0)?;
        let k = match (three, target) {
            (false, TEXTURE_2D) => TexKind::D2,
            (false, TEXTURE_CUBE_MAP) => TexKind::Cube,
            (true, TEXTURE_3D) => TexKind::D3,
            (true, TEXTURE_2D_ARRAY) => TexKind::Array,
            _ => return Err(INVALID_ENUM),
        };
        let (levels, internal, w, hh) = (int(a, 1)?, uint(a, 2)?, int(a, 3)?, int(a, 4)?);
        let d = if three { int(a, 5)? } else { 1 };
        let s = sized(internal).ok_or(INVALID_ENUM)?;
        let h = self.bound_tex(target)?;
        if levels < 1 || w < 1 || hh < 1 || d < 1 {
            return Err(INVALID_VALUE);
        }
        let (w, hh, d) = (w as u32, hh as u32, d as u32);
        let max = match k {
            TexKind::D3 => crate::limits::MAX_3D_TEXTURE_SIZE,
            _ => crate::limits::MAX_TEXTURE_SIZE,
        };
        let max_d = match k {
            TexKind::D3 => crate::limits::MAX_3D_TEXTURE_SIZE,
            TexKind::Array => crate::limits::MAX_ARRAY_TEXTURE_LAYERS,
            _ => 1,
        };
        if w > max || hh > max || d > max_d || (k == TexKind::Cube && w != hh) {
            return Err(INVALID_VALUE);
        }
        if k == TexKind::D3 && class(s.fmt) == Class::Depth {
            return Err(INVALID_OPERATION);
        }
        let chain = full_levels(w, hh, if k == TexKind::D3 { d } else { 1 });
        if levels as u32 > chain || self.objs.textures.get(&h).is_some_and(|t| t.immutable) {
            return Err(INVALID_OPERATION);
        }
        let desc = TexDesc { kind: k, format: s.fmt, width: w, height: hh, depth: d, mips: levels as u32, samples: 1 };
        self.create_dev_tex(h, desc)?;
        let t = self.objs.textures.get_mut(&h).ok_or(INVALID_OPERATION)?;
        t.immutable = true;
        t.imm_levels = levels as u32;
        t.images.iter_mut().for_each(|i| *i = None);
        let faces = t.faces();
        for level in 0..levels as usize {
            let (lw, lh, ld) = level_dims(k, w, hh, d, level as u32);
            let img = Image { w: lw, h: lh, d: ld, internal, format: s.format, ty: s.types[0], fmt: s.fmt, on_dev: true };
            for f in 0..faces {
                t.images[level * 6 + f] = Some(img);
            }
        }
        Ok(Ret::Undefined)
    }
}
