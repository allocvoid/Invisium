//! Buffer objects.

use crate::args::*;
use crate::cmd::{Cmd, GlDevice};
use crate::consts1::*;
use crate::consts2::*;
use crate::objects::Buffer;
use crate::{Arg, Context, Kind, Ret};

impl Context {
    pub(crate) fn buffer_slot(&mut self, target: u32) -> R<&mut u32> {
        let webgl2 = self.is2();
        let s = &mut self.st;
        Ok(match target {
            ARRAY_BUFFER => &mut s.array_buffer,
            ELEMENT_ARRAY_BUFFER => &mut s.element_buffer,
            COPY_READ_BUFFER if webgl2 => &mut s.copy_read,
            COPY_WRITE_BUFFER if webgl2 => &mut s.copy_write,
            PIXEL_PACK_BUFFER if webgl2 => &mut s.pixel_pack,
            PIXEL_UNPACK_BUFFER if webgl2 => &mut s.pixel_unpack,
            UNIFORM_BUFFER if webgl2 => &mut s.uniform_buffer,
            TRANSFORM_FEEDBACK_BUFFER if webgl2 => &mut s.tf_buffer,
            _ => return Err(INVALID_ENUM),
        })
    }

    /// The buffer bound to `target`: `INVALID_ENUM` for a bad target, `INVALID_OPERATION` if
    /// nothing is bound.
    fn bound_buffer(&mut self, target: u32) -> R<u32> {
        match *self.buffer_slot(target)? {
            0 => Err(INVALID_OPERATION),
            b => Ok(b),
        }
    }

    /// WebGL's rule: a buffer keeps the kind (element array or other) of its first binding.
    fn check_kind(&mut self, target: u32, h: u32) -> R<()> {
        let b = self.objs.buffers.get_mut(&h).ok_or(INVALID_OPERATION)?;
        if target == COPY_READ_BUFFER || target == COPY_WRITE_BUFFER {
            b.elem.get_or_insert(false);
            return Ok(());
        }
        let want = target == ELEMENT_ARRAY_BUFFER;
        match b.elem {
            Some(e) if e != want => Err(INVALID_OPERATION),
            _ => {
                b.elem = Some(want);
                Ok(())
            }
        }
    }

    /// `len` bytes of buffer `h` from `offset`: from the CPU copy if there is one, otherwise
    /// read back from the device after flushing (`None` if the device cannot).
    pub(crate) fn buffer_bytes(&mut self, dev: &mut dyn GlDevice, h: u32, offset: usize, len: usize) -> Option<Vec<u8>> {
        let b = self.objs.buffers.get(&h)?;
        if let Some(d) = &b.data {
            return d.get(offset..offset + len).map(|s| s.to_vec());
        }
        if len == 0 || !b.on_dev {
            return Some(vec![0; len]);
        }
        self.flush(dev);
        dev.read_buffer(self.id, h, offset as u32, len as u32).filter(|v| v.len() == len)
    }

    pub(crate) fn buffer_call(&mut self, dev: &mut dyn GlDevice, name: &str, a: &[Arg]) -> R<Ret> {
        match name {
            "createBuffer" => {
                let h = self.new_handle();
                self.objs.buffers.insert(h, Buffer { size: 0, usage: STATIC_DRAW, elem: None, data: None, on_dev: false, scan: None });
                Ok(Ret::Object(Kind::Buffer, h))
            }
            "isBuffer" => Ok(Ret::Bool(match obj(a, 0, Kind::Buffer) {
                Ok(Some(h)) => self.objs.buffers.get(&h).map(|b| b.elem.is_some()).unwrap_or(false),
                _ => false,
            })),
            "deleteBuffer" => {
                let Some(h) = obj(a, 0, Kind::Buffer)? else { return Ok(Ret::Undefined) };
                if let Some(b) = self.objs.buffers.remove(&h) {
                    let s = &mut self.st;
                    for slot in [&mut s.array_buffer, &mut s.element_buffer, &mut s.copy_read, &mut s.copy_write, &mut s.pixel_pack, &mut s.pixel_unpack, &mut s.uniform_buffer, &mut s.tf_buffer] {
                        if *slot == h {
                            *slot = 0;
                        }
                    }
                    for ib in s.uniform_bindings.iter_mut().chain(s.tf_bindings.iter_mut()) {
                        if ib.buffer == h {
                            *ib = Default::default();
                        }
                    }
                    self.unbind_buffer_from_vaos(h);
                    if b.on_dev {
                        self.push(Cmd::BufferDelete { id: h });
                    }
                    self.release(h);
                }
                Ok(Ret::Undefined)
            }
            "bindBuffer" => {
                let target = uint(a, 0)?;
                let b = obj(a, 1, Kind::Buffer)?;
                self.buffer_slot(target)?;
                if let Some(h) = b {
                    self.check_kind(target, h)?;
                }
                *self.buffer_slot(target)? = b.unwrap_or(0);
                Ok(Ret::Undefined)
            }
            "bindBufferBase" | "bindBufferRange" => self.bind_indexed(name == "bindBufferRange", a),
            "bufferData" => self.buffer_data(a),
            "bufferSubData" => self.buffer_sub_data(a),
            "getBufferParameter" => {
                let (target, pname) = (uint(a, 0)?, uint(a, 1)?);
                let h = *self.buffer_slot(target)?;
                if pname != BUFFER_SIZE && pname != BUFFER_USAGE {
                    return Err(INVALID_ENUM);
                }
                let b = self.objs.buffers.get(&h).ok_or(INVALID_OPERATION)?;
                Ok(Ret::Num(if pname == BUFFER_SIZE { b.size as f64 } else { b.usage as f64 }))
            }
            "getBufferSubData" => self.get_buffer_sub_data(dev, a),
            "copyBufferSubData" => self.copy_buffer_sub_data(dev, a),
            _ => Ok(Ret::Undefined),
        }
    }

    fn bind_indexed(&mut self, range: bool, a: &[Arg]) -> R<Ret> {
        let (target, index) = (uint(a, 0)?, uint(a, 1)?);
        let b = obj(a, 2, Kind::Buffer)?;
        let (offset, size) = if range { (long(a, 3)?, long(a, 4)?) } else { (0, 0) };
        let count = match target {
            UNIFORM_BUFFER => self.st.uniform_bindings.len(),
            TRANSFORM_FEEDBACK_BUFFER => self.st.tf_bindings.len(),
            _ => return Err(INVALID_ENUM),
        };
        if index as usize >= count {
            return Err(INVALID_VALUE);
        }
        if range && b.is_some() {
            let align = if target == UNIFORM_BUFFER { crate::limits::UNIFORM_BUFFER_OFFSET_ALIGNMENT as i64 } else { 4 };
            if offset < 0 || size <= 0 || offset % align != 0 || (target == TRANSFORM_FEEDBACK_BUFFER && size % 4 != 0) {
                return Err(INVALID_VALUE);
            }
        }
        if let Some(h) = b {
            self.check_kind(target, h)?;
        }
        let ib = crate::state::IndexedBuffer { buffer: b.unwrap_or(0), offset, size };
        if target == UNIFORM_BUFFER {
            self.st.uniform_bindings[index as usize] = ib;
        } else {
            self.st.tf_bindings[index as usize] = ib;
        }
        *self.buffer_slot(target)? = b.unwrap_or(0);
        Ok(Ret::Undefined)
    }

    fn usage_ok(&self, u: u32) -> bool {
        match u {
            STREAM_DRAW | STATIC_DRAW | DYNAMIC_DRAW => true,
            STREAM_READ | STREAM_COPY | STATIC_READ | STATIC_COPY | DYNAMIC_READ | DYNAMIC_COPY => self.is2(),
            _ => false,
        }
    }

    /// The bytes of a view argument limited by WebGL 2's `srcOffset` / `length` (in elements).
    fn view_range<'a>(&self, a: &[Arg<'a>], i: usize, oi: usize) -> R<Option<&'a [u8]>> {
        match arg(a, i) {
            Arg::Bytes(e, bytes) => {
                let es = elem_size(e);
                let n = bytes.len() / es;
                let off = if self.is2() { uint(a, oi)? as usize } else { 0 };
                let len = if self.is2() { uint(a, oi + 1)? as usize } else { 0 };
                if off > n {
                    return Err(INVALID_VALUE);
                }
                let len = if len == 0 { n - off } else { len };
                if off.checked_add(len).map(|end| end > n).unwrap_or(true) {
                    return Err(INVALID_VALUE);
                }
                Ok(Some(&bytes[off * es..(off + len) * es]))
            }
            Arg::Null => Err(INVALID_VALUE),
            _ => Ok(None),
        }
    }

    fn buffer_data(&mut self, a: &[Arg]) -> R<Ret> {
        let target = uint(a, 0)?;
        self.buffer_slot(target)?;
        let usage = uint(a, 2)?;
        if !self.usage_ok(usage) {
            return Err(INVALID_ENUM);
        }
        let data = self.view_range(a, 1, 3)?;
        let bytes = match data {
            Some(d) => d.to_vec(),
            None => {
                let size = long(a, 1)?;
                if size < 0 {
                    return Err(INVALID_VALUE);
                }
                let h = self.bound_buffer(target)?;
                if size as u64 > crate::limits::MEMORY_BUDGET {
                    return Err(OUT_OF_MEMORY);
                }
                self.reserve(h, Some(size as u64))?;
                vec![0; size as usize]
            }
        };
        let h = self.bound_buffer(target)?;
        self.reserve(h, Some(bytes.len() as u64))?;
        let b = self.objs.buffers.get_mut(&h).ok_or(INVALID_OPERATION)?;
        let index = b.elem == Some(true);
        b.size = bytes.len();
        b.usage = usage;
        b.on_dev = true;
        b.data = if index { Some(bytes.clone()) } else { None };
        b.scan = None;
        self.push(Cmd::BufferCreate { id: h, index, bytes });
        Ok(Ret::Undefined)
    }

    fn buffer_sub_data(&mut self, a: &[Arg]) -> R<Ret> {
        let target = uint(a, 0)?;
        self.buffer_slot(target)?;
        let offset = long(a, 1)?;
        let data = self.view_range(a, 2, 3)?.ok_or(INVALID_OPERATION)?;
        if offset < 0 {
            return Err(INVALID_VALUE);
        }
        let h = self.bound_buffer(target)?;
        let b = self.objs.buffers.get_mut(&h).ok_or(INVALID_OPERATION)?;
        let off = offset as usize;
        if (offset as u64).saturating_add(data.len() as u64) > b.size as u64 {
            return Err(INVALID_VALUE);
        }
        if data.is_empty() {
            return Ok(Ret::Undefined);
        }
        if let Some(copy) = b.data_mut() {
            copy[off..off + data.len()].copy_from_slice(data);
        }
        self.push(Cmd::BufferWrite { id: h, offset: off as u32, bytes: data.to_vec() });
        Ok(Ret::Undefined)
    }

    fn get_buffer_sub_data(&mut self, dev: &mut dyn GlDevice, a: &[Arg]) -> R<Ret> {
        let target = uint(a, 0)?;
        self.buffer_slot(target)?;
        let src = long(a, 1)?;
        let Arg::Bytes(e, view) = arg(a, 2) else { return Err(INVALID_OPERATION) };
        let es = elem_size(e);
        let n = view.len() / es;
        let (dst_off, len) = (uint(a, 3)? as usize, uint(a, 4)? as usize);
        if src < 0 || dst_off > n {
            return Err(INVALID_VALUE);
        }
        let len = if len == 0 { n - dst_off } else { len };
        if dst_off.checked_add(len).map(|end| end > n).unwrap_or(true) {
            return Err(INVALID_VALUE);
        }
        let h = self.bound_buffer(target)?;
        let b = self.objs.buffers.get(&h).ok_or(INVALID_OPERATION)?;
        let bytes = len * es;
        if (src as u64).saturating_add(bytes as u64) > b.size as u64 {
            return Err(INVALID_VALUE);
        }
        match self.buffer_bytes(dev, h, src as usize, bytes) {
            Some(out) => Ok(Ret::Fill { arg: 2, offset: dst_off * es, bytes: out }),
            None => Ok(Ret::Undefined),
        }
    }

    fn copy_buffer_sub_data(&mut self, dev: &mut dyn GlDevice, a: &[Arg]) -> R<Ret> {
        let (rt, wt) = (uint(a, 0)?, uint(a, 1)?);
        self.buffer_slot(rt)?;
        self.buffer_slot(wt)?;
        let (ro, wo, size) = (long(a, 2)?, long(a, 3)?, long(a, 4)?);
        if ro < 0 || wo < 0 || size < 0 {
            return Err(INVALID_VALUE);
        }
        let (r, w) = (self.bound_buffer(rt)?, self.bound_buffer(wt)?);
        let (rb, wb) = (self.objs.buffers.get(&r).ok_or(INVALID_OPERATION)?, self.objs.buffers.get(&w).ok_or(INVALID_OPERATION)?);
        if rb.elem.unwrap_or(false) != wb.elem.unwrap_or(false) {
            return Err(INVALID_OPERATION);
        }
        if ro.saturating_add(size) > rb.size as i64 || wo.saturating_add(size) > wb.size as i64 {
            return Err(INVALID_VALUE);
        }
        if r == w && ro < wo + size && wo < ro + size {
            return Err(INVALID_VALUE);
        }
        let (ro, wo, size) = (ro as usize, wo as usize, size as usize);
        if size == 0 {
            return Ok(Ret::Undefined);
        }
        // Keep the destination's CPU copy (element-array buffers) right.
        if wb.data.is_some() {
            if r == w {
                if let Some(d) = self.objs.buffers.get_mut(&w).and_then(|b| b.data_mut()) {
                    d.copy_within(ro..ro + size, wo);
                }
            } else if let Some(chunk) = self.buffer_bytes(dev, r, ro, size) {
                if let Some(d) = self.objs.buffers.get_mut(&w).and_then(|b| b.data_mut()) {
                    d[wo..wo + size].copy_from_slice(&chunk);
                }
            }
        }
        self.push(Cmd::BufferCopy { src: r, src_offset: ro as u32, dst: w, dst_offset: wo as u32, size: size as u32 });
        Ok(Ret::Undefined)
    }
}
