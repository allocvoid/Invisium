//! State shared between a page and the native functions its scripts call.
//!
//! While script runs, the page lends its document (and the layout derived from it) to this
//! struct; natives mutate it here and record what the page has to redo afterwards in `Effects`.

use crate::atom::Atom;
use crate::css::selector::Selector;
use crate::dom::{NodeId, NO_NODE};
use crate::layout::{BoxId, BoxKind, NO_BOX};
use crate::page::UrlJoin;
use invisium_js::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub(crate) type Shared = Rc<RefCell<HostState>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConsoleLevel {
    Log,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct ConsoleMessage {
    pub level: ConsoleLevel,
    pub text: String,
}

/// Something a custom element must be told about (its lifecycle callbacks).
pub(crate) enum CeEvent {
    Connected(NodeId),
    Disconnected(NodeId),
    /// (element, attribute, old value, new value)
    Attr(NodeId, Atom, Option<String>, Option<String>),
}

pub(crate) struct Timer {
    pub id: u32,
    pub due: f64,
    pub interval: Option<f64>,
}

/// A navigation requested by script.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NavRequest {
    pub url: String,
    pub replace: bool,
}

/// What the page must do once the current script entry returns.
#[derive(Default)]
pub(crate) struct Effects {
    pub dirty_style: bool,
    pub dirty_layout: bool,
    pub dirty_paint: bool,
    /// Subtree roots attached to the document by script.
    pub inserted: Vec<NodeId>,
    pub attr_changed: Vec<(NodeId, Atom)>,
    /// Elements whose text content changed (`<style>`, `<title>`, ...).
    pub text_changed: Vec<NodeId>,
    /// A `<style>` / `<link>` element was added, removed or changed.
    pub sheets_changed: bool,
    pub navigate: Option<NavRequest>,
    /// `form.submit()` / `requestSubmit()`: (form, submitter or `NO_NODE`).
    pub submit: Option<(NodeId, NodeId)>,
    /// A file control whose dialog a script asked for (during a user-input event).
    pub pick_files: Option<NodeId>,
    pub reload: bool,
    pub scroll_to: Option<(f32, f32)>,
    pub focus: Option<NodeId>,
    /// `history.pushState` / `replaceState`: the document's address changed without a load.
    pub url_changed: Option<String>,
    /// Requests started by `fetch` / `XMLHttpRequest`, and ids of ones that were aborted.
    pub fetches: Vec<crate::page::ScriptFetch>,
    pub fetch_aborts: Vec<u32>,
}

pub(crate) struct HostState {
    /// The page itself, while script runs (a hollow stand-in otherwise). Natives read and
    /// change it directly, and can bring its layout up to date.
    pub page: crate::page::Page,
    pub scripting: bool,
    pub fragment_tag: Atom,
    // ── script-side state ──
    pub fx: Effects,
    pub timers: Vec<Timer>,
    /// The page clock when script was entered, and when that was: script sees time move on
    /// while it runs (code that waits for a few milliseconds to pass must not spin forever).
    pub now_ms: f64,
    pub clock_at: std::time::Instant,
    pub console: Vec<ConsoleMessage>,
    pub current_script: NodeId,
    /// A user-input event (click, key press) is being dispatched right now.
    pub user_gesture: bool,
    /// A script has work for the next frame (animation-frame callbacks).
    pub frame_wanted: bool,
    /// Size or visibility observers exist; `observers_due`: they have a target that has not
    /// been looked at yet.
    pub observing: bool,
    pub observers_due: bool,
    /// `wrapped[node]`: a JS wrapper exists (and is rooted) for this node.
    pub wrapped: Vec<bool>,
    /// Prototype objects registered by the bootstrap script, by tag or by `#kind`.
    pub protos: HashMap<Atom, Value>,
    pub cookies: Vec<(String, String)>,
    /// The embedder's cookie jar for `document.cookie` (read, write); the per-page jar above
    /// is only used when there is none.
    pub cookie_access: Option<crate::page::CookieAccess>,
    pub next_fetch: u32,
    /// Script requests that have not finished.
    pub live_fetches: usize,
    /// Custom elements: set once the page defines one; from then on tree and attribute changes
    /// to elements with a dash in their name are queued here and delivered to `ce_hook`
    /// before the native that caused them returns.
    pub ce_active: bool,
    pub ce_queue: Vec<CeEvent>,
    pub ce_hook: Value,
    /// `<script>` elements that must never execute (made by `innerHTML` or `cloneNode`).
    pub inert_scripts: HashSet<NodeId>,
    // ── caches, dropped when the tree or the layout changes ──
    pub sel_cache: HashMap<String, Rc<Vec<Selector>>>,
    pub sib: Option<Rc<(Vec<u32>, Vec<u32>)>>,
    pub box_index: Option<Vec<BoxId>>,
}

impl HostState {
    /// Script is entered at page time `ms`.
    pub fn set_now(&mut self, ms: f64) {
        self.now_ms = ms;
        self.clock_at = std::time::Instant::now();
    }

    /// The page clock as script sees it right now, in milliseconds.
    pub fn live_now(&self) -> f64 {
        self.now_ms + self.clock_at.elapsed().as_secs_f64() * 1000.0
    }

    pub fn new(join: UrlJoin, scripting: bool) -> HostState {
        HostState {
            page: crate::page::Page::hollow(join),
            scripting,
            fragment_tag: Atom::new("#document-fragment"),
            fx: Effects::default(),
            timers: Vec::new(),
            now_ms: 0.0,
            clock_at: std::time::Instant::now(),
            console: Vec::new(),
            current_script: NO_NODE,
            user_gesture: false,
            frame_wanted: false,
            observing: false,
            observers_due: false,
            wrapped: Vec::new(),
            protos: HashMap::new(),
            cookies: Vec::new(),
            cookie_access: None,
            next_fetch: 0,
            live_fetches: 0,
            ce_active: false,
            ce_queue: Vec::new(),
            ce_hook: Value::Undefined,
            inert_scripts: HashSet::new(),
            sel_cache: HashMap::new(),
            sib: None,
            box_index: None,
        }
    }

    pub fn log(&mut self, level: ConsoleLevel, text: String) {
        if self.console.len() < 2000 {
            self.console.push(ConsoleMessage { level, text });
        }
    }

    /// The tree structure changed: style must be recomputed and structural caches are stale.
    pub fn tree_changed(&mut self) {
        self.fx.dirty_style = true;
        self.sib = None;
    }

    /// Could this node be a custom element (an element whose name contains a dash)?
    pub fn is_custom_candidate(&self, n: NodeId) -> bool {
        match self.page.doc.element(n) {
            Some(e) => e.tag != self.fragment_tag && e.tag.as_str().contains('-'),
            None => false,
        }
    }

    /// Queue a lifecycle event for every custom-element candidate in the subtree at `root`.
    pub fn queue_ce_subtree(&mut self, root: NodeId, connected: bool) {
        if !self.ce_active {
            return;
        }
        let nodes: Vec<NodeId> = std::iter::once(root).chain(self.page.doc.descendants(root)).filter(|&n| self.is_custom_candidate(n)).collect();
        for n in nodes {
            self.ce_queue.push(if connected { CeEvent::Connected(n) } else { CeEvent::Disconnected(n) });
        }
    }

    pub fn is_fragment(&self, n: NodeId) -> bool {
        self.page.doc.tag(n) == self.fragment_tag
    }

    /// Sibling indices for selector matching, computed on demand.
    pub fn sibling_indices(&mut self) -> Rc<(Vec<u32>, Vec<u32>)> {
        if let Some(s) = &self.sib {
            if s.0.len() == self.page.doc.nodes.len() {
                return s.clone();
            }
        }
        let s = Rc::new(crate::style::sibling_indices(&self.page.doc));
        self.sib = Some(s.clone());
        s
    }

    /// The principal box of a node in the lent layout tree (`NO_BOX` if it has none).
    pub fn box_of(&mut self, node: NodeId) -> BoxId {
        if self.box_index.is_none() {
            let mut idx = vec![NO_BOX; self.page.doc.nodes.len()];
            for (i, b) in self.page.tree.boxes.iter().enumerate() {
                let n = b.node as usize;
                let principal = b.flags & (crate::layout::F_ANON | crate::layout::F_BEFORE | crate::layout::F_AFTER) == 0 && b.kind != BoxKind::Marker;
                if principal && n < idx.len() && idx[n] == NO_BOX {
                    idx[n] = i as BoxId;
                }
            }
            self.box_index = Some(idx);
        }
        self.box_index.as_ref().and_then(|v| v.get(node as usize).copied()).unwrap_or(NO_BOX)
    }

    pub fn resolve(&self, reference: &str) -> Option<String> {
        self.page.resolve(reference)
    }
}
