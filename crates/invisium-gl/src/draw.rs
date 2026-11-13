//! `drawArrays`, `drawElements` and their instanced / ranged forms.

use crate::args::*;
use crate::cmd::*;
use crate::consts1::*;
use crate::consts2::*;
use crate::vertex::attrib_type_size;
use crate::{Arg, Context, Ret};

fn attr_type(t: u32) -> AttrType {
    match t {
        BYTE => AttrType::I8,
        UNSIGNED_BYTE => AttrType::U8,
        SHORT => AttrType::I16,
        UNSIGNED_SHORT => AttrType::U16,
        INT => AttrType::I32,
        UNSIGNED_INT => AttrType::U32,
        HALF_FLOAT => AttrType::F16,
        INT_2_10_10_10_REV => AttrType::I1010102,
        UNSIGNED_INT_2_10_10_10_REV => AttrType::U1010102,
        _ => AttrType::F32,
    }
}

fn index_value(c: &[u8]) -> u32 {
    match *c {
        [a] => a as u32,
        [a, b] => u16::from_le_bytes([a, b]) as u32,
        [a, b, c, d] => u32::from_le_bytes([a, b, c, d]),
        _ => 0,
    }
}

/// The highest index of element data (`restart`: the primitive restart value is skipped);
/// `None` if every index is a restart. Scans the bytes in place.
fn max_index(data: &[u8], size: usize, restart: bool) -> Option<u32> {
    let restart_value = if size == 4 { u32::MAX } else { (1u32 << (8 * size)) - 1 };
    data.chunks_exact(size).map(index_value).filter(|&i| !restart || i != restart_value).max()
}

struct Elements {
    buffer: u32,
    ty: u32,
    offset: usize,
}

impl Context {
    pub(crate) fn draw_call(&mut self, name: &str, a: &[Arg]) -> R<Ret> {
        let mode = uint(a, 0)?;
        let instanced = name.contains("Instanced");
        let (first, count, elems, inst) = match name {
            "drawArrays" | "drawArraysInstanced" => (int(a, 1)?, int(a, 2)?, None, if instanced { int(a, 3)? } else { 1 }),
            "drawRangeElements" => {
                let (start, end) = (uint(a, 1)?, uint(a, 2)?);
                if end < start {
                    return Err(INVALID_VALUE);
                }
                (0, int(a, 3)?, Some((uint(a, 4)?, long(a, 5)?)), 1)
            }
            _ => (0, int(a, 1)?, Some((uint(a, 2)?, long(a, 3)?)), if instanced { int(a, 4)? } else { 1 }),
        };
        if mode > TRIANGLE_FAN {
            return Err(INVALID_ENUM);
        }
        if let Some((ty, _)) = elems {
            let wide_ok = self.is2() || self.ext("OES_element_index_uint");
            if !(ty == UNSIGNED_BYTE || ty == UNSIGNED_SHORT || (ty == UNSIGNED_INT && wide_ok)) {
                return Err(INVALID_ENUM);
            }
        }
        if first < 0 || count < 0 || inst < 0 || elems.is_some_and(|e| e.1 < 0) {
            return Err(INVALID_VALUE);
        }
        let elems = match elems {
            Some((ty, off)) => {
                let size = attrib_type_size(ty) as i64;
                if off % size != 0 {
                    return Err(INVALID_OPERATION);
                }
                Some(Elements { buffer: self.st.element_buffer, ty, offset: off as usize })
            }
            None => None,
        };
        let prog = self.st.program;
        let exe = self.objs.programs.get(&prog).and_then(|p| p.exe.as_ref()).ok_or(INVALID_OPERATION)?;
        let attributes = exe.prog.attributes.clone();
        let slots = exe.prog.textures.clone();
        let units = exe.units.clone();
        let outputs: Vec<(u32, Scalar)> = exe.prog.outputs.iter().map(|o| (o.location, o.base)).collect();
        let blocks: Vec<(BlockSlot, u32)> = exe.prog.blocks.iter().zip(&exe.block_bindings).map(|(b, &bb)| (b.slot, bb)).collect();
        let target = self.target_of(self.st.draw_fb, true)?;
        if count == 0 || inst == 0 {
            return Ok(Ret::Undefined);
        }
        let (count, inst) = (count as usize, inst as u32);
        // Index data and the vertices it reads.
        let restart = self.is2();
        let vertices: u64 = match &elems {
            None => first as u64 + count as u64,
            Some(e) => {
                let b = self.objs.buffers.get_mut(&e.buffer).ok_or(INVALID_OPERATION)?;
                let size = attrib_type_size(e.ty) as usize;
                let end = e.offset.checked_add(count.checked_mul(size).ok_or(INVALID_OPERATION)?).ok_or(INVALID_OPERATION)?;
                let data = b.data.as_deref().unwrap_or(&[]);
                if end > b.size || end > data.len() {
                    return Err(INVALID_OPERATION);
                }
                let key = (e.offset, count, e.ty);
                let max = match b.scan {
                    Some((k, m)) if k == key => m,
                    _ => {
                        let m = max_index(&data[e.offset..end], size, restart);
                        b.scan = Some((key, m));
                        m
                    }
                };
                max.map(|m| m as u64 + 1).unwrap_or(0)
            }
        };
        let (attrs, consts) = self.draw_attributes(&attributes, vertices, inst, instanced)?;
        let mut block_binds = Vec::with_capacity(blocks.len());
        for (slot, binding) in blocks {
            let ib = self.st.uniform_bindings.get(binding as usize).copied().unwrap_or_default();
            let buf = self.objs.buffers.get(&ib.buffer).ok_or(INVALID_OPERATION)?;
            let avail = (buf.size as i64 - ib.offset).max(0);
            let size = if ib.size == 0 { avail } else { ib.size.min(avail) };
            if size < slot.bytes as i64 {
                return Err(INVALID_OPERATION);
            }
            block_binds.push(BlockBind { binding: slot.binding, buffer: ib.buffer, offset: ib.offset as u32, size: size as u32 });
        }
        let textures = self.draw_textures(&slots, &units)?;
        self.check_outputs(&outputs)?;
        let topology = match mode {
            POINTS => Topology::Points,
            LINES => Topology::Lines,
            LINE_STRIP | LINE_LOOP => Topology::LineStrip,
            TRIANGLE_STRIP => Topology::TriangleStrip,
            _ => Topology::Triangles,
        };
        let Some(state) = self.draw_state(topology, &target)? else { return Ok(Ret::Undefined) };
        let rewrite = mode == LINE_LOOP || mode == TRIANGLE_FAN || elems.as_ref().is_some_and(|e| e.ty == UNSIGNED_BYTE);
        let (index, first, count) = if rewrite {
            // Size the rewritten index data arithmetically and reserve it before allocating.
            let n_out: u64 = match mode {
                LINE_LOOP => count as u64 + 1,
                TRIANGLE_FAN => 3 * (count as u64).saturating_sub(2),
                _ => count as u64,
            };
            if n_out == 0 {
                return Ok(Ret::Undefined);
            }
            let wide = match &elems {
                Some(e) => e.ty == UNSIGNED_INT,
                None => first as u64 + count as u64 - 1 > 0xFFFF,
            };
            let len = n_out.checked_mul(if wide { 4 } else { 2 }).ok_or(OUT_OF_MEMORY)?;
            if len > crate::limits::MEMORY_BUDGET || n_out > u32::MAX as u64 {
                return Err(OUT_OF_MEMORY);
            }
            if self.scratch == 0 {
                self.scratch = self.new_handle();
            }
            let id = self.scratch;
            self.reserve(id, Some(len))?;
            let bytes = self.rewrite_indices(mode, first as u32, count, elems.as_ref(), restart, wide, len as usize);
            self.push(Cmd::BufferCreate { id, index: true, bytes });
            (Some(IndexSource { buffer: id, offset: 0, wide }), 0, n_out as u32)
        } else {
            match &elems {
                Some(e) => (Some(IndexSource { buffer: e.buffer, offset: e.offset as u32, wide: e.ty == UNSIGNED_INT }), 0, count as u32),
                None => (None, first as u32, count as u32),
            }
        };
        let uniforms = self.objs.programs.get_mut(&prog).and_then(|p| p.exe.as_mut()).map(|e| e.uniforms()).ok_or(INVALID_OPERATION)?;
        if target == Target::Surface {
            self.drawn = true;
        }
        self.push(Cmd::Draw(Box::new(Draw { target, state, program: prog, attrs, consts, index, first, count, instances: inst, uniforms, blocks: block_binds, textures })));
        Ok(Ret::Undefined)
    }

    /// The rewritten index data (`len` bytes, already reserved): a closed line strip for
    /// `LINE_LOOP`, a triangle list for `TRIANGLE_FAN`, 8-bit indices widened. Written straight
    /// into the output; nothing else is allocated.
    #[allow(clippy::too_many_arguments)]
    fn rewrite_indices(&self, mode: u32, first: u32, count: usize, elems: Option<&Elements>, restart: bool, wide: bool, len: usize) -> Vec<u8> {
        let data: &[u8] = elems.and_then(|e| self.objs.buffers.get(&e.buffer)).and_then(|b| b.data.as_deref()).unwrap_or(&[]);
        let src = |i: usize| -> u32 {
            match elems {
                None => first + i as u32,
                Some(e) => {
                    let size = attrib_type_size(e.ty) as usize;
                    let p = e.offset + i * size;
                    let v = data.get(p..p + size).map(index_value).unwrap_or(0);
                    if e.ty == UNSIGNED_BYTE && restart && v == 0xFF {
                        0xFFFF
                    } else {
                        v
                    }
                }
            }
        };
        let mut out = Vec::with_capacity(len);
        let mut put = |v: u32| {
            if wide {
                out.extend_from_slice(&v.to_le_bytes());
            } else {
                out.extend_from_slice(&(v as u16).to_le_bytes());
            }
        };
        match mode {
            TRIANGLE_FAN => {
                let first = src(0);
                for i in 1..count.saturating_sub(1) {
                    put(first);
                    put(src(i));
                    put(src(i + 1));
                }
            }
            _ => {
                for i in 0..count {
                    put(src(i));
                }
                if mode == LINE_LOOP {
                    put(src(0));
                }
            }
        }
        out
    }

    /// The program's attributes as arrays (checked against their buffers) and constants.
    fn draw_attributes(&self, attributes: &[invisium_glsl::Attribute], vertices: u64, inst: u32, instanced: bool) -> R<(Vec<VertexAttr>, Vec<ConstAttr>)> {
        let mut attrs = Vec::new();
        let mut consts = Vec::new();
        let mut per_vertex = false;
        for at in attributes {
            for c in 0..at.columns.max(1) {
                let loc = at.location + c;
                let Some(s) = self.attribs().get(loc as usize).copied() else { return Err(INVALID_OPERATION) };
                per_vertex |= s.divisor == 0;
                if !s.enabled {
                    let value = self.st.attrib_values[loc as usize];
                    let ok = matches!((value, at.base), (ConstValue::F(_), Scalar::F32) | (ConstValue::I(_), Scalar::I32) | (ConstValue::U(_), Scalar::U32));
                    if !ok {
                        return Err(INVALID_OPERATION);
                    }
                    consts.push(ConstAttr { location: loc, value });
                    continue;
                }
                let signed = matches!(s.ty, BYTE | SHORT | INT);
                let type_ok = match at.base {
                    Scalar::F32 => !s.integer,
                    Scalar::I32 => s.integer && signed,
                    Scalar::U32 => s.integer && !signed,
                };
                if !type_ok {
                    return Err(INVALID_OPERATION);
                }
                let buf = self.objs.buffers.get(&s.buffer).ok_or(INVALID_OPERATION)?;
                let elem = if matches!(s.ty, INT_2_10_10_10_REV | UNSIGNED_INT_2_10_10_10_REV) { 4 } else { s.size as u64 * attrib_type_size(s.ty) as u64 };
                let stride = if s.stride == 0 { elem } else { s.stride as u64 };
                let reads = if s.divisor == 0 { vertices } else { (inst as u64).div_ceil(s.divisor as u64) };
                if reads > 0 {
                    let need = s.offset.checked_add((reads - 1).checked_mul(stride).ok_or(INVALID_OPERATION)?).and_then(|v| v.checked_add(elem)).ok_or(INVALID_OPERATION)?;
                    if need > buf.size as u64 {
                        return Err(INVALID_OPERATION);
                    }
                }
                attrs.push(VertexAttr {
                    location: loc,
                    buffer: s.buffer,
                    offset: s.offset as u32,
                    stride: stride as u32,
                    ty: attr_type(s.ty),
                    size: s.size,
                    normalized: s.normalized && !s.integer,
                    integer: s.integer,
                    divisor: s.divisor,
                });
            }
        }
        if instanced && !self.is2() && !attributes.is_empty() && !per_vertex {
            return Err(INVALID_OPERATION);
        }
        Ok((attrs, consts))
    }
}
