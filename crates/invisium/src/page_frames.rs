//! Embedded frames (`<iframe>`): a document inside a document.
//!
//! Each frame is a `Page` of its own, owned by the page whose element shows it. The embedder
//! fetches a frame's document like any other resource (`take_frame_requests`, answered with
//! `frame_loaded` or `frame_failed`); everything else happens here: the frame is laid out at
//! the size of its element, its picture is spliced into this page's display list where the
//! element is painted, input over the element goes to it, and its scripts and timers run
//! with this page's. A frame's scripts run in an engine of their own, so documents can only
//! talk to each other with messages, whatever their origins.

use super::{Page, PageAction, PageConfig};
use crate::atom::{a, Atom};
use crate::dom::{NodeId, NO_NODE};
use crate::draw::{Affine, ImageId, Item, Rect, NO_RADII};
use crate::layout::NO_BOX;
use crate::paint::PaintImage;
use crate::text::FontSystem;
use std::sync::atomic::{AtomicU32, Ordering};

/// Image ids from here up are not images: the painter draws a frame's element as an image
/// with id `FRAME_IMAGE_BASE + node`, and `splice_frames` replaces that item with the
/// frame's own display list.
pub(crate) const FRAME_IMAGE_BASE: ImageId = 0xF000_0000;
/// Frames one document may hold, and how deep they may nest.
const MAX_FRAMES: usize = 32;
const MAX_FRAME_DEPTH: u8 = 4;

static NEXT_PAGE_ID: AtomicU32 = AtomicU32::new(1);

pub(crate) fn next_page_id() -> u32 {
    NEXT_PAGE_ID.fetch_add(1, Ordering::Relaxed)
}

/// A frame's document, for the embedder to fetch.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameRequest {
    /// The `<iframe>` element in the page that reported the request.
    pub node: NodeId,
    pub url: String,
    /// A form posted inside the frame: its body and content type (`None` = the default).
    pub body: Option<(Vec<u8>, Option<String>)>,
}

/// A message from one document's scripts to another document.
pub(crate) struct FrameMsg {
    /// 0: the document this one is framed in, 1: the top document, 2: the document in the
    /// frame element `node`.
    pub to: u8,
    pub node: NodeId,
    /// `{"d": data}` as JSON text.
    pub json: String,
    /// The sender's origin.
    pub origin: String,
    /// The origin the receiver must have (`*` = any, `/` = the sender's).
    pub target: String,
}

pub(crate) struct Frame {
    pub page: Box<Page>,
    /// The element's `load` event has been sent for this document.
    pub load_told: bool,
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn without_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or("")
}

impl Page {
    /// A number that identifies this page object among all pages of the process (frames
    /// included), for routing what the embedder fetched to the page that asked.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// True for a page that is shown inside another page's `<iframe>`.
    pub fn is_frame(&self) -> bool {
        self.frame_depth > 0
    }

    /// Run `f` on this page and on every page framed in it, at any depth.
    pub fn for_each_page(&mut self, f: &mut dyn FnMut(&mut Page)) {
        f(self);
        for fr in self.frames.values_mut() {
            fr.page.for_each_page(f);
        }
    }

    /// The page with this id: this one or one framed in it.
    pub fn page_by_id(&mut self, id: u32) -> Option<&mut Page> {
        if self.id == id {
            return Some(self);
        }
        self.frames.values_mut().find_map(|fr| fr.page.page_by_id(id))
    }

    /// An `<iframe>` was found, inserted or changed: ask for its document if it has a new one.
    pub(crate) fn discover_frame(&mut self, n: NodeId) {
        if self.frame_depth >= MAX_FRAME_DEPTH {
            return;
        }
        let room = self.frames.contains_key(&n) || self.frames.len() + self.frame_requests.len() < MAX_FRAMES;
        if let Some(doc) = self.doc.attr(n, Atom::new("srcdoc")).map(|s| s.to_string()) {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            doc.hash(&mut h);
            let key = format!("srcdoc:{:x}", h.finish());
            if !room || self.frame_srcs.get(&n) == Some(&key) {
                return;
            }
            self.frame_srcs.insert(n, key);
            // an inline document belongs to the page that wrote it
            let (url, local) = (self.url.clone(), self.served_local);
            self.install_frame(n, &url, &doc, local);
            return;
        }
        let Some(src) = self.doc.attr(n, a::SRC).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else { return };
        let Some(abs) = self.resolve(&src).filter(|u| is_http(u)) else { return };
        // a document may not frame itself, nor a document it is framed in
        let me = without_fragment(&abs);
        if me == without_fragment(&self.url) || self.ancestors.iter().any(|u| without_fragment(u) == me) {
            return;
        }
        if !room || self.frame_srcs.get(&n) == Some(&abs) {
            return;
        }
        self.frame_srcs.insert(n, abs.clone());
        self.frame_requests.retain(|r| r.node != n);
        self.frame_requests.push(FrameRequest { node: n, url: abs, body: None });
    }

    /// Frame documents asked for since the last call. The embedder fetches each one as a
    /// frame of this page and answers with `frame_loaded` or `frame_failed`.
    pub fn take_frame_requests(&mut self) -> Vec<FrameRequest> {
        std::mem::take(&mut self.frame_requests)
    }

    /// A frame's document arrived. `url` is its final address; `served_local` is what the
    /// network stack said about the response (it decides what the frame may reach).
    pub fn frame_loaded(&mut self, node: NodeId, url: &str, html: &str, served_local: bool) {
        self.install_frame(node, url, html, served_local);
    }

    /// A frame's document could not be loaded (or was refused): the frame stays empty.
    pub fn frame_failed(&mut self, node: NodeId) {
        if self.script.is_some() && self.doc.is_valid(node) {
            // pages wait for the element's load event either way
            self.script(|e| e.resource_done(node, true));
        }
    }

    fn install_frame(&mut self, node: NodeId, url: &str, html: &str, served_local: bool) {
        if !self.doc.is_valid(node) || self.doc.tag(node) != a::IFRAME {
            return;
        }
        // `sandbox` without `allow-scripts` turns the frame's scripts off
        let sandbox = self.doc.attr(node, Atom::new("sandbox"));
        let scripting = self.cfg.scripting && sandbox.map_or(true, |s| s.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("allow-scripts")));
        let cfg = PageConfig { scripting, ..self.cfg.clone() };
        // before this page has a layout, the element's attributes are the best guess
        let attr = |name, default: f32| self.doc.attr(node, name).and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| *v >= 1.0).unwrap_or(default);
        let guess = (attr(a::WIDTH, 300.0), attr(a::HEIGHT, 150.0));
        let size = self.frame_rect(node).map_or(guess, |r| (r.w.max(1.0), r.h.max(1.0)));
        let mut child = Page::new(url, html, self.join.clone(), size, cfg);
        child.frame_depth = self.frame_depth + 1;
        child.ancestors = self.ancestors.clone();
        child.ancestors.push(self.url.clone());
        child.served_local = served_local;
        if let Some(f) = &self.fonts {
            child.set_fonts(f.clone());
        }
        if scripting {
            if let Some(c) = &self.cookie_access {
                child.set_cookie_access(c.clone());
            }
            child.script_stack = self.script_stack;
            if let Some(d) = &self.gl_device {
                child.set_gl_device(d.clone());
            }
        }
        if let Some(old) = self.frames.insert(node, Frame { page: Box::new(child), load_told: false }) {
            // the embedder releases the pictures of the document that was replaced
            self.stale_images.extend(old.page.image_ids());
        }
        self.images.insert(node, PaintImage { id: FRAME_IMAGE_BASE + node, width: 0.0, height: 0.0 });
        if self.focus_frame == node {
            self.focus_frame = NO_NODE;
        }
        self.dirty_paint = true;
    }

    /// The content box of a frame's element, in document coordinates.
    pub(crate) fn frame_rect(&self, node: NodeId) -> Option<Rect> {
        if !self.laid_out {
            return None;
        }
        let b = self.tree.box_for_node(node);
        if b == NO_BOX {
            return None;
        }
        let (x, y) = self.tree.abs_pos(b);
        let bx = &self.tree.boxes[b as usize];
        let bw = &self.styles.style(bx.style).border_width;
        Some(Rect::new(
            x + bw[3] + bx.pad[3],
            y + bw[0] + bx.pad[0],
            (bx.w - bw[1] - bw[3] - bx.pad[1] - bx.pad[3]).max(0.0),
            (bx.h - bw[0] - bw[2] - bx.pad[0] - bx.pad[2]).max(0.0),
        ))
    }

    /// True if a frame has work for `update`.
    pub(crate) fn frames_need_update(&self) -> bool {
        self.frames.values().any(|f| f.page.needs_update())
    }

    /// Give every frame the size of its element and bring it up to date. Returns true if a
    /// frame's picture changed.
    pub(crate) fn update_frames(&mut self, fonts: &mut FontSystem) -> bool {
        let mut changed = false;
        let nodes: Vec<NodeId> = self.frames.keys().copied().collect();
        for n in nodes {
            let size = self.frame_rect(n).map(|r| (r.w, r.h));
            let Some(f) = self.frames.get_mut(&n) else { continue };
            if let Some((w, h)) = size.filter(|s| s.0 >= 1.0 && s.1 >= 1.0) {
                if f.page.viewport() != (w, h) {
                    f.page.set_viewport(w, h);
                }
            }
            changed |= f.page.update(fonts);
        }
        changed
    }

    /// Replace each frame's placeholder in the display list with the frame's own picture,
    /// clipped to the element and moved to where it is (minus the frame's own scrolling).
    pub(crate) fn splice_frames(&mut self) {
        self.glyph_buf.clear();
        self.glyph_buf.extend_from_slice(&self.tree.glyphs);
        let old = std::mem::take(&mut self.list.items);
        let mut items = Vec::with_capacity(old.len() + 64);
        for it in old {
            let (rect, radii, node) = match it {
                Item::Image { rect, image, radii, .. } if image >= FRAME_IMAGE_BASE => (rect, radii, image - FRAME_IMAGE_BASE),
                other => {
                    items.push(other);
                    continue;
                }
            };
            let Some(f) = self.frames.get(&node) else { continue };
            let child = &f.page;
            let (sx, sy) = child.scroll();
            let g0 = self.glyph_buf.len() as u32;
            self.glyph_buf.extend_from_slice(child.glyphs());
            let s0 = self.list.stops.len() as u32;
            self.list.stops.extend_from_slice(&child.list.stops);
            items.push(Item::PushClip { rect, radii });
            let canvas = child.canvas_color();
            if canvas.a != 0 {
                items.push(Item::Rect { rect, color: canvas, radii: NO_RADII });
            }
            // what scrolls with the frame's document, and what is fixed to the frame's viewport
            let scrolled: Affine = [1.0, 0.0, 0.0, 1.0, rect.x - sx, rect.y - sy];
            let fixed: Affine = [1.0, 0.0, 0.0, 1.0, rect.x, rect.y];
            items.push(Item::PushTransform { m: scrolled });
            for ci in &child.list.items {
                match ci {
                    Item::PushFixed => {
                        items.push(Item::PopTransform);
                        items.push(Item::PushTransform { m: fixed });
                    }
                    Item::PopFixed => {
                        items.push(Item::PopTransform);
                        items.push(Item::PushTransform { m: scrolled });
                    }
                    Item::Glyphs { x, baseline, w, size, color, glyphs } => {
                        items.push(Item::Glyphs { x: *x, baseline: *baseline, w: *w, size: *size, color: *color, glyphs: (glyphs.0 + g0, glyphs.1 + g0) });
                    }
                    Item::LinearGradient { rect, radii, angle, stops, repeating } => {
                        items.push(Item::LinearGradient { rect: *rect, radii: *radii, angle: *angle, stops: (stops.0 + s0, stops.1 + s0), repeating: *repeating });
                    }
                    Item::RadialGradient { rect, radii, center, radius, stops, repeating } => {
                        items.push(Item::RadialGradient { rect: *rect, radii: *radii, center: *center, radius: *radius, stops: (stops.0 + s0, stops.1 + s0), repeating: *repeating });
                    }
                    other => items.push(other.clone()),
                }
            }
            items.push(Item::PopTransform);
            items.push(Item::PopClip);
        }
        self.list.items = items;
    }

    /// The frame under a point of the viewport, with the point in the frame's own viewport.
    fn frame_at(&self, x: f32, y: f32) -> Option<(NodeId, f32, f32)> {
        if self.frames.is_empty() {
            return None;
        }
        let b = self.hit(x, y);
        if b == NO_BOX {
            return None;
        }
        let node = self.tree.boxes[b as usize].node;
        if !self.frames.contains_key(&node) {
            return None;
        }
        let r = self.frame_rect(node)?;
        let (px, py) = (x + self.scroll.0 - r.x, y + self.scroll.1 - r.y);
        (px >= 0.0 && py >= 0.0 && px < r.w && py < r.h).then_some((node, px, py))
    }

    /// What an action taken inside a frame means for this page: following a link there loads
    /// another document into the frame; only a new tab is the embedder's business.
    fn frame_action(&mut self, node: NodeId, action: PageAction) -> PageAction {
        match action {
            PageAction::Navigate { url, new_tab: false } => {
                self.navigate_frame(node, url, None);
                PageAction::Redraw
            }
            PageAction::Submit { url, body, content_type } => {
                self.navigate_frame(node, url, body.map(|b| (b, content_type)));
                PageAction::Redraw
            }
            // the embedder cannot address a control inside a frame yet
            PageAction::PickFiles { .. } => PageAction::None,
            other => other,
        }
    }

    fn navigate_frame(&mut self, node: NodeId, url: String, body: Option<(Vec<u8>, Option<String>)>) {
        if !is_http(&url) {
            return;
        }
        self.frame_srcs.insert(node, url.clone());
        self.frame_requests.retain(|r| r.node != node);
        self.frame_requests.push(FrameRequest { node, url, body });
    }

    fn focused_frame(&mut self) -> Option<&mut Page> {
        if self.focus_frame == NO_NODE {
            return None;
        }
        let n = self.focus_frame;
        self.frames.get_mut(&n).map(|f| &mut *f.page)
    }

    // ───────────── input: a frame under the pointer or with the focus gets it ─────────────

    /// Pointer moved to viewport coordinates `(x, y)`.
    pub fn mouse_move(&mut self, x: f32, y: f32) -> super::Hover {
        let over = self.frame_at(x, y);
        let mut left = false;
        if self.hover_frame != NO_NODE && over.map(|o| o.0) != Some(self.hover_frame) {
            let n = self.hover_frame;
            left = self.frames.get_mut(&n).map_or(false, |f| f.page.mouse_leave());
        }
        self.hover_frame = over.map_or(NO_NODE, |o| o.0);
        let mut out = self.mouse_move_own(x, y);
        out.changed |= left;
        if let Some((n, fx, fy)) = over {
            if let Some(f) = self.frames.get_mut(&n) {
                let h = f.page.mouse_move(fx, fy);
                out.link = h.link;
                out.pointer = h.pointer;
                out.changed |= h.changed;
            }
        }
        out
    }

    /// A primary-button click at viewport coordinates. `new_tab` = ctrl/middle click.
    pub fn click(&mut self, x: f32, y: f32, new_tab: bool) -> PageAction {
        if let Some((n, fx, fy)) = self.frame_at(x, y) {
            self.blur_own();
            if self.focus_frame != n {
                if let Some(f) = self.focused_frame() {
                    f.blur();
                }
            }
            self.focus_frame = n;
            let action = self.frames.get_mut(&n).map_or(PageAction::None, |f| f.page.click(fx, fy, new_tab));
            return self.frame_action(n, action);
        }
        if let Some(f) = self.focused_frame() {
            f.blur();
        }
        self.focus_frame = NO_NODE;
        self.click_own(x, y, new_tab)
    }

    /// Scroll the frame under a point of the viewport, if there is one and it can still move
    /// that way. Returns true if a frame scrolled (the page itself then stays put).
    pub fn scroll_frame_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        let Some((n, fx, fy)) = self.frame_at(x, y) else { return false };
        let Some(f) = self.frames.get_mut(&n) else { return false };
        let moved = f.page.scroll_frame_at(fx, fy, dx, dy) || f.page.scroll_by(dx, dy);
        if moved {
            self.dirty_paint = true;
        }
        moved
    }

    pub fn has_text_focus(&self) -> bool {
        if self.focus_frame != NO_NODE {
            return self.frames.get(&self.focus_frame).map_or(false, |f| f.page.has_text_focus());
        }
        self.has_text_focus_own()
    }

    pub fn set_caret(&mut self, on: bool) {
        match self.focused_frame() {
            Some(f) => f.set_caret(on),
            None => self.set_caret_own(on),
        }
    }

    pub fn text_input(&mut self, text: &str) -> PageAction {
        let n = self.focus_frame;
        match self.focused_frame() {
            Some(f) => {
                let a = f.text_input(text);
                self.frame_action(n, a)
            }
            None => self.text_input_own(text),
        }
    }

    pub fn backspace(&mut self) -> PageAction {
        let n = self.focus_frame;
        match self.focused_frame() {
            Some(f) => {
                let a = f.backspace();
                self.frame_action(n, a)
            }
            None => self.backspace_own(),
        }
    }

    pub fn enter(&mut self) -> PageAction {
        let n = self.focus_frame;
        match self.focused_frame() {
            Some(f) => {
                let a = f.enter();
                self.frame_action(n, a)
            }
            None => self.enter_own(),
        }
    }

    pub fn blur(&mut self) {
        if let Some(f) = self.focused_frame() {
            f.blur();
        }
        self.focus_frame = NO_NODE;
        self.blur_own();
    }

    // ───────────── scripts and time ─────────────

    /// `scheme://host[:port]` of this document's address.
    pub(crate) fn origin(&self) -> String {
        let u = &self.url;
        match u.find("://") {
            Some(i) => {
                let rest = &u[i + 3..];
                let end = rest.find(|c| c == '/' || c == '?' || c == '#').unwrap_or(rest.len());
                // user information is no part of an origin
                let host = rest[..end].rsplit('@').next().unwrap_or("");
                format!("{}://{}", &u[..i], host)
            }
            None => "null".to_string(),
        }
    }

    /// Hand a message to this document's scripts if it is meant for this origin.
    fn deliver_message(&mut self, through: NodeId, m: &FrameMsg) -> bool {
        let mine = self.origin();
        let meant = m.target == "*" || m.target.trim_end_matches('/') == mine || (m.target == "/" && m.origin == mine);
        if !meant || self.script.is_none() {
            return false;
        }
        self.script(|e| e.frame_message(&m.json, &m.origin, through));
        true
    }

    /// Run the frames' script work that is due, follow what they asked for, carry messages
    /// between the documents, and tell this page's scripts about frames that finished
    /// loading. Returns true if anything ran.
    pub(crate) fn pump_frames(&mut self, now: f64) -> bool {
        let mut ran = false;
        // messages from this document to the documents in its frames
        if self.outbox.iter().any(|m| m.to == 2) {
            let (down, keep): (Vec<FrameMsg>, Vec<FrameMsg>) = std::mem::take(&mut self.outbox).into_iter().partition(|m| m.to == 2);
            self.outbox = keep;
            for m in down {
                if let Some(f) = self.frames.get_mut(&m.node) {
                    ran |= f.page.deliver_message(NO_NODE, &m);
                }
            }
        }
        if self.frame_depth == 0 {
            // a top document has nobody above it
            self.outbox.clear();
        }
        if self.frames.is_empty() {
            return ran;
        }
        let nodes: Vec<NodeId> = self.frames.keys().copied().collect();
        for n in nodes {
            // messages from the frame's document to this one, or on their way to the top
            let up = match self.frames.get_mut(&n) {
                Some(f) => std::mem::take(&mut f.page.outbox),
                None => continue,
            };
            for m in up {
                if m.to == 1 && self.frame_depth > 0 {
                    if self.outbox.len() < 4096 {
                        self.outbox.push(m);
                    }
                } else if m.to <= 1 {
                    ran |= self.deliver_message(n, &m);
                }
            }
            let (action, loaded) = {
                let Some(f) = self.frames.get_mut(&n) else { continue };
                for _ in 0..4 {
                    if !f.page.pump(now) {
                        break;
                    }
                    ran = true;
                }
                let quiet = f.page.lifecycle == 2 || (f.page.script.is_none() && f.page.pending_scripts() == 0);
                let loaded = quiet && !f.load_told && f.page.pending_stylesheets() == 0;
                if loaded {
                    f.load_told = true;
                }
                (f.page.take_action(), loaded)
            };
            if let Some(a) = action {
                let a = self.frame_action(n, a);
                if !matches!(a, PageAction::None | PageAction::Redraw) && self.pending_action.is_none() {
                    self.pending_action = Some(a);
                }
                ran = true;
            }
            if loaded && self.script.is_some() && self.doc.is_valid(n) {
                self.script(|e| e.resource_done(n, true));
                ran = true;
            }
        }
        ran
    }

    /// A frame of animation for every frame's document. Returns true if any of them ran.
    pub(crate) fn frame_frames(&mut self, now: f64) -> bool {
        let mut ran = false;
        for f in self.frames.values_mut() {
            if f.page.wants_frame() {
                ran |= f.page.frame(now);
            }
        }
        if ran {
            self.dirty_paint = true;
        }
        ran
    }

    pub(crate) fn frames_want_frame(&self) -> bool {
        self.frames.values().any(|f| f.page.wants_frame())
    }

    /// The earliest timer of any frame's scripts.
    pub(crate) fn frames_next_timer(&self) -> Option<f64> {
        self.frames.values().filter_map(|f| f.page.next_timer_ms()).fold(None, |m, t| Some(m.map_or(t, |m: f64| m.min(t))))
    }

    /// Frames whose document is still on its way.
    pub fn frames_loading(&self) -> usize {
        self.frame_requests.len() + self.frames.values().map(|f| f.page.frames_loading()).sum::<usize>()
    }
}
