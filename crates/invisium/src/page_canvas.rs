//! `<canvas>`: the contexts scripts draw with, and getting their pictures on screen.
//!
//! A canvas has no bitmap until a script asks for a context. A 2D context draws into a bitmap
//! on the CPU (`invisium-canvas`); when it changed, the embedder uploads it and attaches it to
//! the element like any other image. A WebGL context (`invisium-gl`) describes its drawing as
//! commands for the embedder's GPU device, which also owns the picture: the element is painted
//! with the image id the device hands out, and presenting only means "show the new frame".

use super::Page;
use crate::atom::a;
use crate::dom::NodeId;
use crate::paint::PaintImage;
use invisium_canvas::Canvas2d;
use invisium_draw::gl::GlDevice;
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) enum CanvasCtx {
    D2(Box<Canvas2d>),
    Gl(Box<invisium_gl::Context>),
}

pub(crate) struct CanvasSlot {
    pub ctx: CanvasCtx,
    /// Size of the bitmap or drawing buffer in pixels.
    pub size: (u32, u32),
    /// 2D only: the bitmap changed and the embedder has not fetched it yet.
    pub upload: bool,
}

/// Contexts one page may hold. Browsers cap WebGL contexts at 16; 2D ones are cheaper.
pub(crate) const MAX_GL_CONTEXTS: usize = 16;
pub(crate) const MAX_CANVASES: usize = 256;

/// A canvas dimension from its attribute: a non-negative integer, else the default.
fn dimension(v: Option<&str>, default: u32) -> u32 {
    match v.map(str::trim).and_then(|s| s.parse::<u32>().ok()) {
        Some(n) => n.min(32_767),
        None => default,
    }
}

impl Page {
    /// The GPU device WebGL contexts of this page draw with. Without one, `getContext('webgl')`
    /// answers null and pages fall back to whatever they do without WebGL.
    pub fn set_gl_device(&mut self, dev: Rc<RefCell<dyn GlDevice>>) {
        self.gl_device = Some(dev);
    }

    /// The bitmap size a canvas element asks for (300 x 150 unless its attributes say else).
    pub(crate) fn canvas_attr_size(&self, node: NodeId) -> (u32, u32) {
        (dimension(self.doc.attr(node, a::WIDTH), 300), dimension(self.doc.attr(node, a::HEIGHT), 150))
    }

    /// A canvas's `width` or `height` changed: its context starts over with a blank bitmap.
    pub(crate) fn canvas_resized(&mut self, node: NodeId) {
        let size = self.canvas_attr_size(node);
        let Some(slot) = self.canvases.get_mut(&node) else {
            self.dirty_layout = true;
            return;
        };
        slot.size = size;
        match &mut slot.ctx {
            CanvasCtx::D2(c) => {
                c.resize(size.0, size.1);
                slot.upload = true;
            }
            CanvasCtx::Gl(c) => {
                c.resize(size.0, size.1);
                if let Some(img) = self.images.get_mut(&node) {
                    img.width = size.0 as f32;
                    img.height = size.1 as f32;
                }
            }
        }
        self.dirty_layout = true;
    }

    /// Show what scripts drew since the last call. Returns true if a canvas changed on screen.
    pub(crate) fn present_canvases(&mut self) -> bool {
        if self.canvases.is_empty() {
            return false;
        }
        let dev = self.gl_device.clone();
        let mut changed = false;
        for slot in self.canvases.values_mut() {
            match &mut slot.ctx {
                CanvasCtx::Gl(c) => {
                    if let Some(d) = &dev {
                        changed |= c.present(&mut *d.borrow_mut());
                    }
                }
                CanvasCtx::D2(c) => {
                    if c.take_dirty() {
                        slot.upload = true;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.dirty_paint = true;
        }
        changed
    }

    /// 2D canvases whose bitmap changed since the last call: the element, the bitmap's size and
    /// its pixels (straight-alpha RGBA8, top row first). The embedder uploads each one and
    /// attaches it with `set_node_image`.
    pub fn take_canvas_uploads(&mut self) -> Vec<(NodeId, u32, u32, Vec<u8>)> {
        let mut out = Vec::new();
        for (&node, slot) in self.canvases.iter_mut() {
            if !slot.upload {
                continue;
            }
            slot.upload = false;
            if let CanvasCtx::D2(c) = &slot.ctx {
                let (w, h) = c.size();
                if w == 0 || h == 0 {
                    continue;
                }
                let mut px = Vec::new();
                c.read_rgba(&mut px);
                out.push((node, w, h, px));
            }
        }
        out
    }

    /// Number of canvases with a context, and the CPU bytes they hold.
    pub fn canvas_stats(&self) -> (usize, usize) {
        let bytes = self
            .canvases
            .values()
            .map(|s| match &s.ctx {
                CanvasCtx::D2(c) => c.memory_bytes(),
                CanvasCtx::Gl(c) => c.memory_bytes(),
            })
            .sum();
        (self.canvases.len(), bytes)
    }

    /// The image ids of this page's WebGL canvases: they belong to the GPU device, so the
    /// embedder must not release them like images it uploaded itself.
    pub fn gl_image_ids(&self) -> Vec<crate::draw::ImageId> {
        let Some(dev) = &self.gl_device else { return Vec::new() };
        let dev = dev.borrow();
        self.canvases
            .values()
            .filter_map(|s| match &s.ctx {
                CanvasCtx::Gl(c) => Some(dev.image(c.id())),
                _ => None,
            })
            .collect()
    }

    /// Give a canvas the image its WebGL context presents into.
    pub(crate) fn attach_gl_image(&mut self, node: NodeId, ctx: u32, size: (u32, u32)) {
        let Some(dev) = &self.gl_device else { return };
        let id = dev.borrow().image(ctx);
        self.images.insert(node, PaintImage { id, width: size.0 as f32, height: size.1 as f32 });
        self.dirty_paint = true;
    }

    /// Free the GPU side of every WebGL context (the page is going away).
    pub(crate) fn drop_canvases(&mut self) {
        let Some(dev) = self.gl_device.clone() else {
            self.canvases.clear();
            return;
        };
        let slots: Vec<CanvasSlot> = self.canvases.drain().map(|(_, s)| s).collect();
        let borrowed = dev.try_borrow_mut();
        if let Ok(mut d) = borrowed {
            for slot in slots {
                if let CanvasCtx::Gl(mut c) = slot.ctx {
                    c.destroy(&mut *d);
                }
            }
        };
    }
}
