//! `Page` and scripts: when scripts run, what they see, and what the page redoes afterwards.
//!
//! The document is parsed completely before any script runs, so scripts execute in document
//! order once their sources are available (`async` and script-inserted scripts run when they
//! arrive). `document.write` is therefore not supported. While a script runs, the engine holds
//! the document; when it returns, `apply_effects` picks up everything the script changed.

use super::{CookieAccess, FetchResult, ModuleEntry, Page, PageAction, PendingScript, ResKind, ScriptFetch};
use crate::atom::{a, Atom};
use crate::dom::{NodeId, NO_NODE};
use crate::script::{ConsoleMessage, Effects, EventInit, ScriptEngine, INLINE_MODULE};

/// Scripts this engine runs: classic JavaScript. Modules are skipped (and `nomodule` fallbacks
/// run), as are data blocks such as JSON or templates.
fn is_classic(ty: Option<&str>) -> bool {
    let Some(t) = ty else { return true };
    let t = t.trim().to_ascii_lowercase();
    let t = t.split(';').next().unwrap_or("").trim();
    t.is_empty()
        || matches!(
            t,
            "text/javascript" | "application/javascript" | "text/ecmascript" | "application/ecmascript" | "application/x-javascript" | "text/jscript" | "text/x-javascript"
        )
}

impl Page {
    pub(super) fn discover_script(&mut self, n: NodeId, parser: bool) {
        if self.seen_scripts.contains(&n) {
            return;
        }
        if self.doc.attr(n, a::TYPE).map_or(false, |t| t.trim().eq_ignore_ascii_case("module")) {
            if parser || !self.script.as_ref().map_or(false, |e| e.state().borrow().inert_scripts.contains(&n)) {
                self.discover_module(n);
            }
            return;
        }
        // modules are supported, so fallbacks meant for browsers without them stay off
        if !is_classic(self.doc.attr(n, a::TYPE)) || self.doc.attr(n, Atom::new("nomodule")).is_some() {
            return;
        }
        if !parser && self.script.as_ref().map_or(false, |e| e.state().borrow().inert_scripts.contains(&n)) {
            return;
        }
        let pending = match self.doc.attr(n, a::SRC).map(|s| s.to_string()) {
            Some(src) => {
                let Some(abs) = self.resolve(&src).filter(|_| !src.trim().is_empty()) else { return };
                let id = self.request(abs.clone(), ResKind::Script);
                PendingScript { node: n, url: abs, source: None, req: Some(id), failed: false }
            }
            None => {
                let text = self.doc.text_content(n);
                if text.trim().is_empty() {
                    return; // may still get a `src` or text later
                }
                PendingScript { node: n, url: self.url.clone(), source: Some(text), req: None, failed: false }
            }
        };
        self.seen_scripts.insert(n);
        let is_async = pending.req.is_some() && self.doc.attr(n, Atom::new("async")).is_some();
        if parser && !is_async {
            self.scripts.push_back(pending);
        } else {
            self.loose_scripts.push(pending);
        }
    }

    fn discover_module(&mut self, n: NodeId) {
        match self.doc.attr(n, a::SRC).map(|s| s.to_string()) {
            Some(src) => {
                let Some(abs) = self.resolve(&src).filter(|_| !src.trim().is_empty()) else { return };
                self.want_module(abs.clone());
                self.module_queue.push_back(ModuleEntry { key: abs, inline: None });
            }
            None => {
                let text = self.doc.text_content(n);
                if text.trim().is_empty() {
                    return;
                }
                self.module_seq += 1;
                let key = format!("{INLINE_MODULE}{}", self.module_seq);
                self.module_queue.push_back(ModuleEntry { key, inline: Some(text) });
            }
        }
        self.seen_scripts.insert(n);
    }

    /// Make sure the source of the module at `url` is on its way (once).
    fn want_module(&mut self, url: String) {
        if self.module_fetching.contains(&url) || self.module_failed.contains(&url) || self.script.as_ref().map_or(false, |e| e.has_module(&url)) {
            return;
        }
        let id = self.request(url.clone(), ResKind::Script);
        self.module_reqs.insert(id, url.clone());
        self.module_fetching.insert(url);
    }

    /// Register module sources that arrived, ask for what they import, and run the module
    /// scripts and dynamic imports whose whole graph is here. Returns true if anything ran.
    fn process_modules(&mut self) -> bool {
        let idle = self.module_queue.is_empty() && self.module_arrived.is_empty() && self.imports.is_empty();
        if idle && self.script.is_none() {
            return false;
        }
        let mut ran = false;
        for _ in 0..10_000 {
            let Some((url, source)) = self.module_arrived.pop() else { break };
            self.module_fetching.remove(&url);
            match source {
                Some(text) => {
                    for dep in self.script(|e| e.add_module(&url, &text)).unwrap_or_default() {
                        self.want_module(dep);
                    }
                }
                None => {
                    self.module_failed.insert(url);
                }
            }
        }
        for i in 0..self.module_queue.len() {
            if let Some(text) = self.module_queue[i].inline.take() {
                let key = self.module_queue[i].key.clone();
                for dep in self.script(|e| e.add_module(&key, &text)).unwrap_or_default() {
                    self.want_module(dep);
                }
            }
        }
        if self.script.is_some() {
            for (id, url) in self.script(|e| e.take_import_requests()).unwrap_or_default() {
                self.want_module(url.clone());
                self.imports.push((id, url));
            }
        }
        // a graph is complete when no module source is still on its way
        if !self.module_fetching.is_empty() {
            return ran;
        }
        if self.scripts.is_empty() {
            while let Some(entry) = self.module_queue.pop_front() {
                self.script(|e| e.run_module(&entry.key));
                ran = true;
                if !self.module_fetching.is_empty() {
                    break; // it asked for more (a dynamic import): later entries wait their turn
                }
            }
        }
        for (id, url) in std::mem::take(&mut self.imports) {
            let present = self.script.as_ref().map_or(false, |e| e.has_module(&url));
            let result = if present { Ok(()) } else { Err(format!("Failed to fetch dynamically imported module: {url}")) };
            self.script(|e| e.finish_import(id, result));
            ran = true;
        }
        ran
    }

    /// An external script (or module source) arrived.
    pub fn script_loaded(&mut self, id: u32, source: &str) {
        if let Some(url) = self.module_reqs.remove(&id) {
            self.module_arrived.push((url, Some(source.to_string())));
            return;
        }
        for s in self.scripts.iter_mut().chain(self.loose_scripts.iter_mut()) {
            if s.req == Some(id) {
                s.source = Some(source.to_string());
            }
        }
    }

    pub(super) fn script_failed(&mut self, id: u32) {
        if let Some(url) = self.module_reqs.remove(&id) {
            self.module_arrived.push((url, None));
            return;
        }
        for s in self.scripts.iter_mut().chain(self.loose_scripts.iter_mut()) {
            if s.req == Some(id) {
                s.failed = true;
            }
        }
    }

    /// Scripts that have been found but have not run yet.
    pub fn pending_scripts(&self) -> usize {
        self.scripts.len() + self.loose_scripts.len() + self.module_queue.len()
    }

    /// True once a script has run on this page (events are only dispatched to script then).
    pub fn has_script(&self) -> bool {
        self.script.is_some()
    }

    /// Run `f` with the script engine. For the duration the whole page lives in the script
    /// host (natives work on it directly and can redo its layout); `self` is a hollow stand-in
    /// that nothing touches. `None` if scripting is off or the engine could not start.
    pub(super) fn script<R>(&mut self, f: impl FnOnce(&mut ScriptEngine) -> R) -> Option<R> {
        if !self.cfg.scripting {
            return None;
        }
        let mut eng = match self.script.take() {
            Some(e) => e,
            None => Box::new(ScriptEngine::new(self.join.clone(), self.script_stack)),
        };
        {
            let mut st = eng.state().borrow_mut();
            st.now_ms = self.clock_ms;
            st.live_fetches = self.live_fetches.len();
            st.box_index = None;
            if st.cookie_access.is_none() {
                st.cookie_access.clone_from(&self.cookie_access);
            }
            std::mem::swap(self, &mut st.page);
        }
        eng.boot();
        let r = if eng.usable() { Some(f(&mut eng)) } else { None };
        let fx = {
            let mut st = eng.state().borrow_mut();
            std::mem::swap(self, &mut st.page);
            st.box_index = None;
            if self.console.len() < 5000 {
                self.console.append(&mut st.console);
            } else {
                st.console.clear();
            }
            std::mem::take(&mut st.fx)
        };
        self.script = Some(eng);
        self.apply_effects(fx);
        r
    }

    /// Dispatch an input event to script. Returns false if a listener cancelled it. Does nothing
    /// (and returns true) on pages where no script has run.
    pub(super) fn fire(&mut self, target: NodeId, kind: &str, init: EventInit) -> bool {
        if self.script.is_none() {
            return true;
        }
        self.script(|e| e.event(target, kind, init)).unwrap_or(true)
    }

    pub(super) fn resource_event(&mut self, node: NodeId, ok: bool) {
        self.script(|e| e.resource_done(node, ok));
    }

    /// The element an event at `node` targets (text nodes report their parent).
    pub(super) fn element_of(&self, node: NodeId) -> NodeId {
        if node != NO_NODE && !self.doc.is_element(node) {
            self.doc.parent(node)
        } else {
            node
        }
    }

    fn apply_effects(&mut self, fx: Effects) {
        self.dirty_style |= fx.dirty_style;
        self.dirty_layout |= fx.dirty_layout;
        self.dirty_paint |= fx.dirty_paint;
        let mut sheets = fx.sheets_changed;
        let mut title = false;
        for root in fx.inserted {
            if !self.doc.is_connected(root) {
                continue;
            }
            let nodes: Vec<NodeId> = std::iter::once(root).chain(self.doc.descendants(root)).collect();
            for n in nodes {
                match self.doc.tag(n) {
                    a::STYLE | a::LINK => sheets = true,
                    a::TITLE => title = true,
                    _ => {}
                }
                self.discover_node(n, false);
            }
        }
        for (n, name) in fx.attr_changed {
            match (self.doc.tag(n), name) {
                (a::IMG, a::SRC | a::SRCSET) | (a::INPUT, a::SRC) => {
                    self.images.remove(&n);
                    self.image_sizes.remove(&n);
                    self.dirty_layout = true;
                    self.discover_node(n, false);
                }
                (a::SCRIPT, a::SRC) => self.discover_node(n, false),
                (a::BASE, _) => {
                    if let Some(abs) = self.doc.attr(n, a::HREF).and_then(|h| (self.join)(&self.url, h)) {
                        self.base_url = abs;
                    }
                }
                _ => {}
            }
        }
        for n in fx.text_changed {
            match self.doc.tag(n) {
                a::STYLE => sheets = true,
                a::TITLE => title = true,
                a::SCRIPT => self.discover_node(n, false),
                _ => {}
            }
        }
        if sheets {
            self.sync_sheets();
        }
        if title {
            self.retitle();
        }
        if let Some(url) = fx.url_changed {
            if self.base_url == self.url {
                self.base_url.clone_from(&url);
            }
            self.url = url;
        }
        if let Some(pos) = fx.scroll_to {
            if self.scroll != pos {
                self.scroll = pos;
                self.clamp_scroll();
                self.script_redraw = true;
            }
        }
        if let Some(n) = fx.focus {
            // the script-side focus() already told listeners
            if self.focus != n {
                self.focus = n;
                self.caret_on = true;
                self.dirty_paint = true;
                self.focus_value = if n != NO_NODE && self.is_text_control(n) { Some(self.current_value(n)) } else { None };
                if self.stylist.has_dynamic_rules() {
                    self.dirty_style = true;
                }
            }
        }
        for f in fx.fetches {
            self.live_fetches.insert(f.id);
            self.fetches.push(f);
        }
        for id in fx.fetch_aborts {
            if self.live_fetches.remove(&id) {
                self.fetches.retain(|f| f.id != id);
                self.fetch_aborts.push(id);
            }
        }
        if let Some(nav) = fx.navigate {
            let _ = nav.replace;
            self.pending_action = Some(PageAction::Navigate { url: nav.url, new_tab: false });
        }
        if fx.reload {
            self.pending_action = Some(PageAction::Navigate { url: self.url.clone(), new_tab: false });
        }
        if let Some((form, submitter)) = fx.submit {
            let action = self.submit_now(form, submitter);
            if action != PageAction::None {
                self.pending_action = Some(action);
            }
        }
    }

    fn run_pending(&mut self, s: PendingScript) {
        if s.failed {
            if self.script.is_some() {
                self.resource_event(s.node, false);
            }
            return;
        }
        let Some(src) = s.source else { return };
        let external = s.req.is_some();
        self.script(|e| {
            e.run_script(&src, &s.url, s.node);
            if external {
                e.resource_done(s.node, true);
            }
        });
    }

    /// Advance script work: run scripts whose sources have arrived, timers that are due, and
    /// the document lifecycle events. `now_ms` is a monotonic clock in milliseconds. Returns
    /// true if anything ran (the page may then need `update`, and `take_action` may have work).
    pub fn pump(&mut self, now_ms: f64) -> bool {
        self.clock_ms = now_ms;
        if !self.cfg.scripting {
            return false;
        }
        let mut ran = std::mem::take(&mut self.script_redraw);
        // document order: an external script that has not arrived holds back those after it
        for _ in 0..64 {
            if !matches!(self.scripts.front(), Some(s) if s.source.is_some() || s.failed) {
                break;
            }
            if let Some(s) = self.scripts.pop_front() {
                self.run_pending(s);
                ran = true;
            }
        }
        let mut i = 0;
        while i < self.loose_scripts.len() {
            if self.loose_scripts[i].source.is_some() || self.loose_scripts[i].failed {
                let s = self.loose_scripts.remove(i);
                self.run_pending(s);
                ran = true;
            } else {
                i += 1;
            }
        }
        ran |= self.process_modules();
        if self.script.is_none() {
            return ran;
        }
        if self.lifecycle == 0 && self.scripts.is_empty() && self.module_queue.is_empty() {
            self.lifecycle = 1;
            self.script(|e| e.ready("interactive"));
            ran = true;
        }
        if self.lifecycle == 1 && self.scripts.is_empty() && self.pending_stylesheets() == 0 {
            self.lifecycle = 2;
            self.script(|e| e.ready("complete"));
            ran = true;
        }
        let due = self.script.as_ref().and_then(|e| e.next_timer()).map_or(false, |d| d <= now_ms);
        if due {
            ran |= self.script(|e| e.run_timers(now_ms)).unwrap_or(0) > 0;
        }
        ran | std::mem::take(&mut self.script_redraw)
    }

    /// Requests started by script since the last call. Each must be answered with `fetch_done`.
    pub fn take_fetches(&mut self) -> Vec<ScriptFetch> {
        std::mem::take(&mut self.fetches)
    }

    /// Requests that script gave up on since the last call (the embedder may cancel them).
    pub fn take_fetch_aborts(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.fetch_aborts)
    }

    /// Script requests still waiting for `fetch_done`.
    pub fn pending_fetches(&self) -> usize {
        self.live_fetches.len()
    }

    /// The outcome of a script request. `None` means it failed for any reason: blocked by the
    /// cross-origin rules, by a protection, or by the network. Script cannot tell which.
    pub fn fetch_done(&mut self, id: u32, result: Option<FetchResult>) {
        if !self.live_fetches.remove(&id) {
            return; // aborted, or not ours
        }
        self.script(|e| e.fetch_done(id, result.as_ref()));
    }

    /// Give `document.cookie` access to the embedder's cookie jar. Without this, scripts see
    /// a jar that lives only in the page.
    pub fn set_cookie_access(&mut self, access: CookieAccess) {
        self.cookie_access = Some(access);
    }

    /// Run `source` as if the page had contained it. For diagnostics (`--eval` in screenshot
    /// mode): whatever it logs shows up in `take_console`.
    pub fn eval(&mut self, source: &str) {
        self.script(|e| e.run_script(source, "eval", NO_NODE));
    }

    /// When the next timer is due, on the clock passed to `pump`.
    pub fn next_timer_ms(&self) -> Option<f64> {
        self.script.as_ref().and_then(|e| e.next_timer())
    }

    /// Console output since the last call.
    pub fn take_console(&mut self) -> Vec<ConsoleMessage> {
        std::mem::take(&mut self.console)
    }

    /// A navigation or form submission that a script asked for.
    pub fn take_action(&mut self) -> Option<PageAction> {
        self.pending_action.take()
    }
}
