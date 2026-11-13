//! The context's single memory budget and what it counts.
//!
//! Counted (as allocated capacity): the bitmap, the current path, `Path2D` objects, gradients
//! and patterns (with their stops and pixels), clip masks, the current drawing state and the
//! save stack (with dash arrays and strings). Gradients, patterns and clip masks may be shared
//! by several states; their bytes are held by a [`Charge`] inside the shared data, so they stay
//! counted until the last holder lets go, whatever happened to the script's handle.
//! Transient working memory of a single call is not counted.

use crate::paint::{Gradient, C4};
use crate::path::{Path, SEG_BYTES};
use crate::raster::Mask;
use crate::state::{Pattern, State};
use crate::Canvas2d;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::mem::size_of;
use std::rc::Rc;

/// Most bytes one context may hold.
pub const BUDGET: usize = 512 << 20;
/// Bytes counted per gradient stop.
pub const STOP_BYTES: usize = size_of::<(f64, C4)>();
/// Fixed cost of one gradient: the shared allocation with its reference counts (its handle's
/// map slot is counted with the map, see [`map_bytes`]).
pub const GRAD_OBJ: usize = size_of::<RefCell<Gradient>>() + 2 * size_of::<usize>();
/// Fixed cost of one pattern, besides its pixels.
pub const PAT_OBJ: usize = size_of::<RefCell<Pattern>>() + 2 * size_of::<usize>();

/// Bytes a handle map holds: its slots (entry plus control byte) at its current capacity.
pub fn map_bytes<V>(m: &HashMap<u32, V>) -> usize {
    slots_bytes::<V>(m.capacity())
}

/// Bytes of a map with room for `cap` entries (buckets are about 8/7 of the capacity).
pub fn slots_bytes<V>(cap: usize) -> usize {
    cap * (size_of::<(u32, V)>() + 1) * 8 / 7
}

/// Extra bytes one more insertion may make the map allocate (it may double).
pub fn grow_bytes<V>(m: &HashMap<u32, V>) -> usize {
    if m.len() < m.capacity() {
        0
    } else {
        slots_bytes::<V>(2 * m.capacity() + 4)
    }
}

/// Bytes added to a shared counter for as long as this value lives. A clone charges again.
#[derive(Debug)]
pub struct Charge {
    bytes: usize,
    counter: Rc<Cell<usize>>,
}

impl Charge {
    pub fn new(bytes: usize, counter: &Rc<Cell<usize>>) -> Charge {
        counter.set(counter.get() + bytes);
        Charge { bytes, counter: counter.clone() }
    }
    /// Change the charged amount.
    pub fn set(&mut self, bytes: usize) {
        self.counter.set(self.counter.get().saturating_sub(self.bytes) + bytes);
        self.bytes = bytes;
    }
}

impl Clone for Charge {
    fn clone(&self) -> Charge {
        Charge::new(self.bytes, &self.counter)
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.counter.set(self.counter.get().saturating_sub(self.bytes));
    }
}

/// A clip mask, charged while it lives.
pub struct ClipMask {
    pub mask: Mask,
    _charge: Charge,
}

impl ClipMask {
    pub fn new(mask: Mask, counter: &Rc<Cell<usize>>) -> ClipMask {
        let c = Charge::new(mask.data.capacity(), counter);
        ClipMask { mask, _charge: c }
    }
}

/// Bytes one drawing state holds outside its own struct (its clip is charged separately).
pub fn state_extra(s: &State) -> usize {
    s.dash.capacity() * size_of::<f64>() + s.font_css.capacity() + s.filter.capacity() + s.lang.capacity()
}

/// Bytes one `Path2D` counts for besides its map slot.
pub fn path_obj_bytes(p: &Path) -> usize {
    p.segs.capacity() * SEG_BYTES
}

/// Bytes a gradient counts for.
pub fn grad_bytes(g: &Gradient) -> usize {
    GRAD_OBJ + g.stops.capacity() * STOP_BYTES
}

impl Canvas2d {
    /// Everything the budget counts, in bytes.
    pub(crate) fn used_bytes(&self) -> usize {
        self.bmp.px.capacity()
            + self.path.segs.capacity() * SEG_BYTES
            + self.objs.path_bytes
            + self.objs.map_bytes()
            + self.held.get()
            + self.stack.capacity() * size_of::<State>()
            + self.stack_extra
            + size_of::<State>()
            + state_extra(&self.st)
    }
    /// Bytes still free under the budget.
    pub(crate) fn room(&self) -> usize {
        BUDGET.saturating_sub(self.used_bytes())
    }
}
