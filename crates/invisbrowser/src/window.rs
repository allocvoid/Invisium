//! The window: winit events in, frames out. Draws only when something changed.

use crate::app::Browser;
use crate::loader::{NetEvent, Sink};
use invis_chrome::{Action, Chrome, ChromeState, CursorKind, Input, Key, Mods, MouseButton, Theme};
use invisium::draw::{DrawList, Glyph, Layer, Rect, Rgba, NO_RADII};
use invisium::page::{PageAction, PointerButton, PointerKind};
use invisium_gfx::{GpuOptions, Renderer};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton as WButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WKey, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

pub enum UserEvent {
    Net(NetEvent),
}

struct Clip(Option<arboard::Clipboard>);

impl invis_chrome::Clipboard for Clip {
    fn get(&mut self) -> Option<String> {
        self.0.as_mut()?.get_text().ok()
    }
    fn set(&mut self, text: &str) {
        if let Some(c) = self.0.as_mut() {
            let _ = c.set_text(text.to_string());
        }
    }
}

const BACKDROP: Rgba = Rgba::hex(0x0a0f1a);

/// Shortest time between two frames while something animates. Presenting waits for the display
/// anyway; this only keeps the loop from spinning when it does not (hidden window, no vsync).
const FRAME_GAP: Duration = Duration::from_millis(8);
/// The gap after a frame that drew nothing (nothing waited for the display): about 60 a second.
const IDLE_FRAME_GAP: Duration = Duration::from_millis(16);
/// How quickly a wheel or key scroll closes in on its target (time constant).
const SCROLL_EASE_MS: f32 = 55.0;

/// Counters printed once a second when `INVIS_LOOP_DEBUG` is set: what wakes the event loop
/// and where its time goes.
#[derive(Default)]
struct LoopStats {
    on: bool,
    since: Option<Instant>,
    wakes: u32,
    timer_wakes: u32,
    ticks: u32,
    tick_us: u64,
    frames: u32,
    frame_us: u64,
    user_events: u32,
    window_events: u32,
    /// about_to_wait outcomes: frames wanted, busy, page timer, idle
    branch: [u32; 4],
    timer_ms: u64,
}

impl LoopStats {
    fn report(&mut self, extra: &str) {
        if !self.on {
            return;
        }
        let now = Instant::now();
        let since = *self.since.get_or_insert(now);
        if now.duration_since(since) < Duration::from_secs(1) {
            return;
        }
        eprintln!(
            "[loop] wakes {} (timer {}) ticks {} ({} ms) frames {} ({} ms) user-events {} window-events {} wait[frames {} busy {} timer {} idle {}] next-timer {} ms {}",
            self.wakes,
            self.timer_wakes,
            self.ticks,
            self.tick_us / 1000,
            self.frames,
            self.frame_us / 1000,
            self.user_events,
            self.window_events,
            self.branch[0],
            self.branch[1],
            self.branch[2],
            self.branch[3],
            self.timer_ms,
            extra
        );
        if invisium::draw::prof::enabled() {
            // where the main thread's time went in this second (see `INVIS_PROFILE`)
            let lines: Vec<String> = invisium::draw::prof::report().lines().take(14).map(str::to_string).collect();
            eprintln!("{}", lines.join("
"));
        }
        *self = LoopStats { on: true, since: Some(now), ..LoopStats::default() };
    }
}

struct App {
    stats: LoopStats,
    browser: Browser,
    chrome: Chrome,
    state: ChromeState,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    chrome_list: DrawList,
    chrome_glyphs: Vec<Glyph>,
    under: DrawList,
    over: DrawList,
    clip: Clip,
    mods: Mods,
    cursor: (f32, f32),
    /// The buttons that went down on the page and are still held (see `press_bit`), and the
    /// page they went down on.
    page_press: u8,
    page_press_id: u32,
    /// Where the page is drawn, in logical pixels.
    page_rect: Rect,
    last_click: (Instant, f32, f32, u8),
    /// Debugging aid (`INVIS_TEST_INPUT`): pointer input still to be played back.
    test_input: Vec<TestStep>,
    last_tick: Instant,
    /// When the last frame was started (frames are never started less than `FRAME_GAP` apart).
    last_frame: Instant,
    /// Something asked for a redraw since the last frame (as opposed to a frame that only
    /// exists because the page is animating, which is skipped if nothing changed).
    redraw_wanted: std::cell::Cell<bool>,
    /// The last frame drew something (presenting paces the loop; without it we pace ourselves).
    presented: bool,
    /// When the animation loop last asked for a frame (it asks at most once per gap, also
    /// when the system delivers no frame, as for a minimised window).
    last_request: Instant,
    /// The window is minimised or fully covered: pages get no frames (timers still run).
    hidden: bool,
    /// The pending redraw was asked for by the animation loop only (a redraw the system asks
    /// for, after the window was covered or restored, always draws).
    auto_frame: std::cell::Cell<bool>,
    /// Where a wheel or key scroll is heading: (tab, navigation, target y in page pixels).
    scroll_goal: Option<(u64, u64, f32)>,
    caret_elapsed: u32,
    caret_on: bool,
    shown_address: String,
    shown_tab: u64,
    start_url: Option<String>,
    quit: bool,
}

impl App {
    fn redraw_soon(&self) {
        self.redraw_wanted.set(true);
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// Ask for a frame only because the page is animating.
    fn next_animation_frame(&self) {
        self.auto_frame.set(true);
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn zoom(&self) -> f32 {
        self.browser.tabs.get(self.browser.active).map_or(1.0, |t| t.zoom)
    }

    /// Rebuild what the chrome shows from the browser state.
    fn sync(&mut self) {
        self.state = self.browser.chrome_state();
        if let Some(tab) = self.browser.tabs.get(self.browser.active) {
            if tab.id != self.shown_tab || tab.url != self.shown_address {
                self.shown_tab = tab.id;
                self.shown_address = tab.url.clone();
                let shown = if tab.url == "invis://newtab" { "" } else { tab.url.as_str() };
                self.chrome.set_address(shown);
            }
            if let Some(w) = &self.window {
                w.set_title(&format!("{} - InvisBrowser", tab.title));
            }
        }
        self.redraw_soon();
    }

    fn page_point(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        if self.page_rect.contains(x, y) {
            let z = self.zoom();
            Some(((x - self.page_rect.x) / z, (y - self.page_rect.y) / z))
        } else {
            None
        }
    }

    fn set_cursor(&self, kind: CursorKind) {
        if let Some(w) = &self.window {
            w.set_cursor(match kind {
                CursorKind::Pointer => CursorIcon::Pointer,
                CursorKind::Text => CursorIcon::Text,
                CursorKind::Default => CursorIcon::Default,
            });
        }
    }

    /// Give the chrome first refusal on an input event. Returns true if it took it.
    fn chrome_input(&mut self, input: Input) -> bool {
        let out = self.chrome.handle(&input, &self.state, &mut *self.browser.fonts.borrow_mut(), &mut self.clip);
        if let Some(c) = out.cursor {
            self.set_cursor(c);
        }
        let had_actions = !out.actions.is_empty();
        for a in out.actions {
            self.action(a);
        }
        if had_actions {
            self.sync();
        } else if out.redraw {
            self.redraw_soon();
        }
        out.consumed
    }

    fn action(&mut self, a: Action) {
        let active = self.browser.active;
        match a {
            Action::NewTab => {
                self.browser.active = self.browser.new_tab("invis://newtab");
                self.chrome.focus_address(true);
            }
            Action::CloseTab(id) => {
                if !self.browser.close_tab(id) {
                    self.quit = true;
                }
            }
            Action::SelectTab(id) => self.browser.select_tab(id),
            Action::MoveTab { .. } => {}
            Action::Navigate(text) => self.browser.navigate(active, &text, true, None),
            Action::Back => self.browser.back(active),
            Action::Forward => self.browser.forward(active),
            Action::Reload => self.browser.reload(active),
            Action::Stop => self.browser.stop(active),
            Action::Toggle(id) => {
                self.browser.settings.toggle(&id);
                self.browser.settings.save();
                self.browser.apply_settings();
            }
            Action::OpenInternal(name) => self.browser.navigate(active, &format!("invis://{name}"), true, None),
            Action::NewIdentity => self.browser.net.new_identity(),
            Action::ZoomIn | Action::ZoomOut | Action::ZoomReset => {
                if let Some(t) = self.browser.tabs.get_mut(active) {
                    t.zoom = match a {
                        Action::ZoomIn => (t.zoom + 0.1).min(3.0),
                        Action::ZoomOut => (t.zoom - 0.1).max(0.3),
                        _ => 1.0,
                    };
                }
            }
            Action::FocusPage => {}
        }
    }

    fn page_action(&mut self, a: PageAction) {
        let active = self.browser.active;
        match a {
            PageAction::None => {}
            PageAction::Redraw => self.redraw_soon(),
            PageAction::Navigate { url, new_tab } => {
                let from = self.browser.shown_url(active);
                self.browser.navigate_from_page(active, &from, &url, None, new_tab);
                self.sync();
            }
            PageAction::Submit { url, body, content_type } => {
                let from = self.browser.shown_url(active);
                self.browser.navigate_from_page(active, &from, &url, body.map(|b| (b, content_type)), false);
                self.sync();
            }
            PageAction::PickFiles { control, multiple, accept } => self.pick_files(control, multiple, &accept),
        }
    }

    /// The page asked for files (the user activated a file control): show the system dialog,
    /// read what was chosen and hand it to the page. Only files picked here ever reach a page.
    fn pick_files(&mut self, control: invisium::dom::NodeId, multiple: bool, accept: &str) {
        let owner = self.window.as_ref().map_or(0, |w| native_handle(w));
        let paths = crate::filedialog::pick_files(owner, multiple, accept);
        if paths.is_empty() {
            return;
        }
        let mut files = Vec::new();
        let mut total: u64 = 0;
        for path in paths {
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            total += meta.len();
            if total > MAX_PICKED_BYTES {
                self.browser.status = Some("That selection is too large to load (limit 1 GB)".into());
                break;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0.0, |d| d.as_millis() as f64);
            files.push(invisium::page::PickedFile { mime: mime_of(&name).to_string(), name, last_modified_ms: modified, bytes });
        }
        if files.is_empty() {
            return;
        }
        if let Some(page) = self.browser.active_page() {
            page.files_chosen(control, files);
        }
        self.browser.tick();
        self.sync();
        self.redraw_soon();
    }

    /// Play back the steps of `INVIS_TEST_INPUT` that are due, through the same code a real
    /// mouse goes through. Returns when the next one is due.
    fn play_test_input(&mut self) -> Option<Instant> {
        while let Some(step) = self.test_input.first().copied() {
            if Instant::now() < step.at {
                return Some(step.at);
            }
            self.test_input.remove(0);
            let z = self.zoom();
            let (x, y) = (self.page_rect.x + step.x * z, self.page_rect.y + step.y * z);
            match step.what {
                'm' => self.mouse_move(x, y),
                'd' => self.mouse_down(WButton::Left),
                'w' => self.wheel(step.x),
                _ => self.mouse_up(MouseButton::Left),
            }
            if std::env::var_os("INVIS_TEST_QUIET").is_none() {
                let title = self.browser.tabs.get(self.browser.active).map_or(String::new(), |t| t.title.clone());
                let scroll = self.browser.active_page().map(|p| p.scroll());
                eprintln!("[test] {} at page {},{} (viewport {:?}, scroll {scroll:?}, title {title:?})", step.what, step.x, step.y, self.browser.viewport);
            }
        }
        None
    }

    /// A press on the page ends without its release: the page it went to is no longer the one
    /// in front (another tab), or the window lost the pointer (another window took over).
    fn cancel_page_press(&mut self) {
        if std::mem::take(&mut self.page_press) == 0 {
            return;
        }
        let id = self.page_press_id;
        let page = self.browser.tabs.iter_mut().filter_map(|t| t.page.as_mut()).map(|lp| &mut lp.page).find(|p| p.id() == id);
        if page.map_or(false, |p| p.release_all()) {
            self.redraw_soon();
        }
    }

    /// True while a press on the page is going on and that page is still the one in front.
    fn page_pressed(&mut self) -> bool {
        if self.page_press == 0 {
            return false;
        }
        let id = self.page_press_id;
        if self.browser.active_page().map(|p| p.id()) != Some(id) {
            self.cancel_page_press();
        }
        self.page_press != 0
    }

    /// The pointer is over the window but not over the page.
    fn page_left(&mut self) {
        if self.browser.active_page().map_or(false, |p| p.mouse_leave()) {
            self.redraw_soon();
        }
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        // while a button that went down on the page is held, the page keeps the pointer
        let dragging = self.page_pressed();
        if !dragging && self.chrome_input(Input::MouseMove { x, y }) {
            if self.browser.status.take().is_some() {
                self.sync();
            }
            self.page_left();
            return;
        }
        let point = match self.page_point(x, y) {
            None if dragging => {
                let z = self.zoom();
                Some(((x - self.page_rect.x) / z, (y - self.page_rect.y) / z))
            }
            p => p,
        };
        let Some((px, py)) = point else {
            self.set_cursor(CursorKind::Default);
            self.page_left();
            return;
        };
        let mods = self.mods;
        let Some(page) = self.browser.active_page() else { return };
        page.set_modifiers(mods.ctrl, mods.shift, mods.alt);
        let h = page.mouse_move(px, py);
        self.set_cursor(match h.pointer {
            Some(PointerKind::Pointer) => CursorKind::Pointer,
            Some(PointerKind::Text) => CursorKind::Text,
            _ => CursorKind::Default,
        });
        if h.link != self.browser.status {
            self.browser.status = h.link;
            self.sync();
        } else if h.changed {
            self.redraw_soon();
        }
    }

    fn mouse_down(&mut self, button: WButton) {
        let (x, y) = self.cursor;
        let b = match button {
            WButton::Left => MouseButton::Left,
            WButton::Middle => MouseButton::Middle,
            WButton::Right => MouseButton::Right,
            WButton::Back => {
                let a = self.browser.active;
                self.browser.back(a);
                self.sync();
                return;
            }
            WButton::Forward => {
                let a = self.browser.active;
                self.browser.forward(a);
                self.sync();
                return;
            }
            _ => return,
        };
        let now = Instant::now();
        let (t, lx, ly, n) = self.last_click;
        let clicks = if b == MouseButton::Left && now.duration_since(t) < Duration::from_millis(450) && (x - lx).abs() < 4.0 && (y - ly).abs() < 4.0 { (n % 3) + 1 } else { 1 };
        if b == MouseButton::Left {
            self.last_click = (now, x, y, clicks);
        }
        if self.chrome_input(Input::MouseDown { x, y, button: b, clicks }) {
            if let Some(p) = self.browser.active_page() {
                p.blur();
            }
            return;
        }
        // the page hears the button go down now; what a click does happens when it comes up
        let Some((px, py)) = self.page_point(x, y) else { return };
        let pb = page_button(b);
        let mods = self.mods;
        self.page_pressed();
        let Some(page) = self.browser.active_page() else { return };
        page.set_modifiers(mods.ctrl, mods.shift, mods.alt);
        page.press(px, py, pb);
        let id = page.id();
        self.page_press |= press_bit(pb);
        self.page_press_id = id;
        self.redraw_soon();
    }

    fn mouse_up(&mut self, b: MouseButton) {
        let (x, y) = self.cursor;
        self.chrome_input(Input::MouseUp { x, y, button: b });
        let pb = page_button(b);
        if !self.page_pressed() || self.page_press & press_bit(pb) == 0 {
            return;
        }
        self.page_press &= !press_bit(pb);
        let point = self.page_point(x, y);
        let mods = self.mods;
        let Some(page) = self.browser.active_page() else { return };
        page.set_modifiers(mods.ctrl, mods.shift, mods.alt);
        match point {
            Some((px, py)) => {
                let a = page.release(px, py, pb, mods.ctrl);
                self.caret_on = true;
                self.caret_elapsed = 0;
                self.page_action(a);
            }
            None => {
                page.release_outside(pb);
            }
        }
        self.redraw_soon();
    }

    fn wheel(&mut self, dy_px: f32) {
        let (x, y) = self.cursor;
        if self.chrome_input(Input::Wheel { x, y, dy: dy_px }) {
            return;
        }
        let z = self.zoom();
        // a frame under the pointer scrolls first; the page moves once the frame cannot
        if let Some((px, py)) = self.page_point(x, y) {
            let in_frame = self.browser.active_page().map_or(false, |p| p.scroll_frame_at(px, py, 0.0, dy_px / z));
            if in_frame {
                self.redraw_soon();
                return;
            }
        }
        self.scroll_smoothly(dy_px / z);
    }

    /// Scroll the page by `dy` page pixels, gliding there over the next few frames. Returns
    /// false if the page cannot scroll that way.
    fn scroll_smoothly(&mut self, dy: f32) -> bool {
        let Some(tab) = self.browser.tabs.get(self.browser.active) else { return false };
        let key = (tab.id, tab.nav);
        let Some(page) = self.browser.active_page() else { return false };
        let (_, sy) = page.scroll();
        let max = (page.content_size().1 - page.viewport().1).max(0.0);
        // further input adds to where the page is already heading
        let from = match self.scroll_goal {
            Some((t, n, goal)) if (t, n) == key => goal,
            _ => sy,
        };
        let goal = (from + dy).clamp(0.0, max);
        if (goal - sy).abs() < 0.5 {
            self.scroll_goal = None;
            return false;
        }
        self.scroll_goal = Some((key.0, key.1, goal));
        self.redraw_soon();
        true
    }

    /// Move a gliding scroll one frame along. `dt_ms` is the time since the previous frame.
    fn step_scroll(&mut self, dt_ms: f32) {
        let Some((t, n, goal)) = self.scroll_goal else { return };
        let same = self.browser.tabs.get(self.browser.active).map_or(false, |tab| (tab.id, tab.nav) == (t, n));
        let Some(page) = self.browser.active_page().filter(|_| same) else {
            self.scroll_goal = None;
            return;
        };
        let (sx, sy) = page.scroll();
        let left = goal - sy;
        let k = 1.0 - (-dt_ms.clamp(1.0, 100.0) / SCROLL_EASE_MS).exp();
        let mut step = left * k;
        if step.abs() < 1.0 {
            step = left.signum() * left.abs().min(1.0);
        }
        let done = (left - step).abs() < 0.5;
        let moved = page.scroll_to(sx, if done { goal } else { sy + step });
        if done || !moved {
            self.scroll_goal = None;
        }
    }

    fn key(&mut self, key: &WKey, text: Option<&str>) {
        let mapped = match key {
            WKey::Named(n) => match n {
                NamedKey::Enter => Some(Key::Enter),
                NamedKey::Escape => Some(Key::Escape),
                NamedKey::Backspace => Some(Key::Backspace),
                NamedKey::Delete => Some(Key::Delete),
                NamedKey::Tab => Some(Key::Tab),
                NamedKey::ArrowLeft => Some(Key::Left),
                NamedKey::ArrowRight => Some(Key::Right),
                NamedKey::ArrowUp => Some(Key::Up),
                NamedKey::ArrowDown => Some(Key::Down),
                NamedKey::Home => Some(Key::Home),
                NamedKey::End => Some(Key::End),
                NamedKey::PageUp => Some(Key::PageUp),
                NamedKey::PageDown => Some(Key::PageDown),
                NamedKey::F5 => Some(Key::F5),
                NamedKey::F6 => Some(Key::F6),
                _ => None,
            },
            // character keys reach the chrome as shortcuts only when a modifier is held;
            // plain typing arrives as text below
            WKey::Character(s) if self.mods.ctrl || self.mods.alt => s.chars().next().map(|c| Key::Char(c.to_ascii_lowercase())),
            _ => None,
        };
        if let Some(k) = mapped.clone() {
            if self.chrome_input(Input::Key { key: k, mods: self.mods }) {
                return;
            }
        }
        let typing = !self.mods.ctrl && !self.mods.alt;
        if typing {
            if let Some(t) = text.filter(|t| !t.is_empty() && !t.chars().any(|c| c.is_control())) {
                if self.chrome_input(Input::Text(t.to_string())) {
                    return;
                }
                let has_focus = self.browser.active_page().map_or(false, |p| p.has_text_focus());
                if has_focus {
                    let a = self.browser.active_page().map(|p| p.text_input(t)).unwrap_or(PageAction::None);
                    self.caret_on = true;
                    self.caret_elapsed = 0;
                    self.page_action(a);
                    return;
                }
            }
        }
        // keys the chrome did not want go to the page
        let z = self.zoom();
        let view_h = self.page_rect.h / z;
        let Some(page) = self.browser.active_page() else { return };
        let text_focus = page.has_text_focus();
        let action = match mapped {
            Some(Key::Backspace) if text_focus => page.backspace(),
            Some(Key::Enter) if text_focus => page.enter(),
            Some(Key::Escape) if text_focus => {
                page.blur();
                PageAction::Redraw
            }
            Some(k) if !text_focus => {
                let (_, sy) = page.scroll();
                let _ = sy;
                let glide = match k {
                    Key::Down => Some(48.0),
                    Key::Up => Some(-48.0),
                    Key::PageDown => Some(view_h * 0.9),
                    Key::PageUp => Some(-view_h * 0.9),
                    _ => None,
                };
                let moved = match k {
                    Key::Home => page.scroll_to(0.0, 0.0),
                    Key::End => page.scroll_to(0.0, f32::MAX),
                    _ => false,
                };
                if let Some(dy) = glide {
                    self.scroll_smoothly(dy);
                } else if moved {
                    self.scroll_goal = None;
                }
                if moved {
                    PageAction::Redraw
                } else {
                    PageAction::None
                }
            }
            _ => {
                // space scrolls a page that is not being typed into
                if typing && !text_focus && text == Some(" ") {
                    self.scroll_smoothly(view_h * 0.9);
                }
                PageAction::None
            }
        };
        self.page_action(action);
    }

    fn tick(&mut self) {
        let now = Instant::now();
        self.stats.ticks += 1;
        self.tick_inner(now);
        self.stats.tick_us += now.elapsed().as_micros() as u64;
    }

    fn tick_inner(&mut self, now: Instant) {
        let dt = now.duration_since(self.last_tick).as_millis().min(1000) as u32;
        self.last_tick = now;
        if self.chrome.tick(dt, &self.state) {
            self.redraw_soon();
        }
        if self.browser.tick() {
            self.sync();
        }
        self.caret_elapsed += dt;
        if self.caret_elapsed >= 530 {
            self.caret_elapsed = 0;
            self.caret_on = !self.caret_on;
            let on = self.caret_on;
            if let Some(p) = self.browser.active_page() {
                if p.has_text_focus() {
                    p.set_caret(on);
                    self.redraw_soon();
                }
            }
        }
        // the Tor indicator changes without any input while bootstrapping
        if self.browser.settings.uses_tor() {
            let s = self.browser.chrome_state();
            if s.tor != self.state.tor {
                self.sync();
            }
        }
    }

    /// Nothing of the window can be seen: it is minimised (which some systems only show as a
    /// window of no size) or fully covered. Pages get no frames then and their timers run at
    /// most once a second, as in other browsers.
    fn out_of_sight(&self) -> bool {
        let Some(w) = self.window.as_ref() else { return true };
        let size = w.inner_size();
        self.hidden || w.is_minimized() == Some(true) || size.width == 0 || size.height == 0
    }

    /// True if the next frame should follow this one without waiting for input.
    fn wants_frames(&self) -> bool {
        if self.out_of_sight() {
            return false;
        }
        self.scroll_goal.is_some() || self.browser.active_wants_frame()
    }

    fn animating(&mut self) -> bool {
        let loading = self.browser.tabs.iter().any(|t| t.loading());
        let typing = self.chrome.address_focused() || self.browser.active_page().map_or(false, |p| p.has_text_focus());
        let tor_boot = self.browser.settings.uses_tor() && self.state.tor != invis_chrome::TorIndicator::On;
        loading || typing || tor_boot
    }

    fn frame(&mut self) {
        let Some(window) = self.window.clone() else { return };
        // timers and loading go on while the page animates (the loop only comes here then)
        let started = Instant::now();
        let dt_ms = started.duration_since(self.last_frame).as_secs_f32() * 1000.0;
        self.last_frame = started;
        self.presented = false;
        if self.wants_frames() {
            self.tick();
        }
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 || self.renderer.is_none() {
            return;
        }
        // only a frame the animation loop asked for may turn out to have nothing to draw
        let auto = self.auto_frame.replace(false);
        let mut changed = self.redraw_wanted.replace(false) || !auto;
        let scale = window.scale_factor() as f32;
        let (lw, lh) = (size.width as f32 / scale, size.height as f32 / scale);
        let content = self.chrome.layout(lw, lh);
        let zoom = self.zoom();

        // Page viewport in CSS pixels. With letterboxing it is rounded down to 100px steps so
        // the size pages can observe is shared by many users.
        let (mut vw, mut vh) = ((content.w / zoom).max(1.0), (content.h / zoom).max(1.0));
        if self.browser.settings.letterbox {
            if vw >= 400.0 {
                vw = (vw / 100.0).floor() * 100.0;
            }
            if vh >= 300.0 {
                vh = (vh / 100.0).floor() * 100.0;
            }
        }
        self.page_rect = Rect::new(content.x + ((content.w - vw * zoom) / 2.0).floor(), content.y, vw * zoom, vh * zoom);
        self.browser.viewport = (vw, vh);

        // release textures of pages that went away
        if let Some(r) = self.renderer.as_mut() {
            for id in self.browser.dropped_images.drain(..) {
                r.drop_image(id);
            }
        }

        // this frame's share of a gliding scroll, then what the page itself has queued for it
        changed |= self.scroll_goal.is_some();
        self.step_scroll(dt_ms);
        let (tab_before, title_before) = self.browser.tabs.get(self.browser.active).map_or((0, String::new()), |t| (t.nav, t.title.clone()));
        self.browser.frame_active();
        if self.browser.tabs.get(self.browser.active).map_or(false, |t| t.nav != tab_before || t.title != title_before) {
            self.sync();
        }

        let active = self.browser.active;
        let mut canvas = Rgba::WHITE;
        let mut scroll = (0.0, 0.0);
        let mut content_h = 0.0;
        {
            let Browser { tabs, fonts, .. } = &mut self.browser;
            if let Some(lp) = tabs.get_mut(active).and_then(|t| t.page.as_mut()) {
                lp.page.set_viewport(vw, vh);
                changed |= lp.page.update(&mut fonts.borrow_mut());
                if let Some(r) = self.renderer.as_mut() {
                    let mut upload = |w: u32, h: u32, px: &[u8]| r.create_image(w, h, px);
                    if crate::loader::rasterize_inline_svgs(&mut lp.page, scale * zoom, &mut upload) {
                        changed |= lp.page.update(&mut fonts.borrow_mut());
                    }
                    if crate::loader::upload_canvases(&mut lp.page, &mut upload) {
                        changed |= lp.page.update(&mut fonts.borrow_mut());
                    }
                    // an inline SVG that was drawn again leaves its old texture behind
                    for id in lp.page.take_stale_images() {
                        r.drop_image(id);
                    }
                }
                canvas = lp.page.canvas_color();
                scroll = lp.page.scroll();
                content_h = lp.page.content_size().1;
            }
        }
        // layout can reveal more resources (background images)
        if active < self.browser.tabs.len() {
            self.browser.pump(active);
        }

        // a frame that exists only because the page is animating, and in which nothing moved
        // (an idle animation-frame loop, an animation still in its delay), draws nothing
        if !changed && !self.redraw_wanted.get() {
            return;
        }
        self.redraw_wanted.set(false);
        self.presented = true;

        self.chrome_list.clear();
        self.chrome_glyphs.clear();
        self.chrome.paint(&self.state, &mut *self.browser.fonts.borrow_mut(), &mut self.chrome_list, &mut self.chrome_glyphs);

        let pr = Rect::new(self.page_rect.x * scale, self.page_rect.y * scale, self.page_rect.w * scale, self.page_rect.h * scale);
        self.under.clear();
        self.under.rect(pr, canvas);
        self.over.clear();
        if content_h > vh + 1.0 {
            // overlay scrollbar
            let track = pr.h;
            let thumb = (track * vh / content_h).max(24.0 * scale);
            let pos = (track - thumb) * (scroll.1 / (content_h - vh)).clamp(0.0, 1.0);
            let w = 6.0 * scale;
            self.over.rounded_rect(Rect::new(pr.right() - w - 2.0 * scale, pr.y + pos, w, thumb), Rgba::new(128, 136, 150, 150), [w / 2.0; 4]);
        }
        let _ = NO_RADII;

        let full = Rect::new(0.0, 0.0, size.width as f32, size.height as f32);
        let no_glyphs: [Glyph; 0] = [];
        let page = self.browser.tabs.get(active).and_then(|t| t.page.as_ref()).map(|lp| &lp.page);
        let mut layers: Vec<Layer> = Vec::with_capacity(4);
        layers.push(Layer { list: &self.under, glyphs: &no_glyphs, viewport: full, scroll: (0.0, 0.0), scale: 1.0 });
        if let Some(p) = page {
            layers.push(Layer { list: p.display_list(), glyphs: p.glyphs(), viewport: pr, scroll, scale: scale * zoom });
        }
        layers.push(Layer { list: &self.over, glyphs: &no_glyphs, viewport: full, scroll: (0.0, 0.0), scale: 1.0 });
        layers.push(Layer { list: &self.chrome_list, glyphs: &self.chrome_glyphs, viewport: full, scroll: (0.0, 0.0), scale });
        if let Some(r) = self.renderer.as_mut() {
            if let Err(e) = r.render(&layers, &*self.browser.fonts.borrow(), BACKDROP) {
                eprintln!("render: {e}");
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("InvisBrowser")
            .with_inner_size(LogicalSize::new(1280.0, 820.0))
            .with_min_inner_size(LogicalSize::new(420.0, 320.0))
            .with_theme(Some(winit::window::Theme::Dark));
        let window = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("could not create a window: {e}");
                el.exit();
                return;
            }
        };
        let size = window.inner_size();
        let opts = GpuOptions { low_power: self.browser.settings.low_power_gpu, vsync: true, max_atlas_size: 4096 };
        match pollster::block_on(Renderer::new(window.clone(), size.width.max(1), size.height.max(1), opts)) {
            Ok(r) => {
                self.browser.gpu_info = r.stats().adapter;
                // pages that run scripts draw their WebGL canvases with the renderer's device
                let dev = r.gl_device();
                for tab in &mut self.browser.tabs {
                    let pages = tab.incoming.iter_mut().map(|(lp, _)| lp).chain(tab.page.iter_mut());
                    for lp in pages.filter(|lp| !lp.internal) {
                        lp.page.for_each_page(&mut |p| p.set_gl_device(dev.clone()));
                    }
                }
                self.browser.gl_device = Some(dev);
                self.renderer = Some(r);
            }
            Err(e) => {
                eprintln!("could not start the GPU renderer: {e}");
                el.exit();
                return;
            }
        }
        self.window = Some(window);
        let start = self.start_url.take().unwrap_or_else(|| "invis://newtab".into());
        self.browser.active = self.browser.new_tab(&start);
        if start == "invis://newtab" {
            self.chrome.focus_address(true);
        }
        self.sync();
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: UserEvent) {
        self.stats.user_events += 1;
        let UserEvent::Net(ev) = event;
        let App { browser, renderer, .. } = self;
        let mut upload = |w: u32, h: u32, rgba: &[u8]| match renderer.as_mut() {
            Some(r) => r.create_image(w, h, rgba),
            None => 0,
        };
        browser.on_net(ev, &mut upload);
        // a result can unblock scripts or complete the load: let them run now, not at the next timer
        browser.tick();
        self.sync();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        self.stats.window_events += 1;
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(size.width.max(1), size.height.max(1));
                }
                self.redraw_soon();
            }
            WindowEvent::ScaleFactorChanged { .. } => self.redraw_soon(),
            WindowEvent::RedrawRequested => {
                let t = Instant::now();
                self.frame();
                self.stats.frames += 1;
                self.stats.frame_us += t.elapsed().as_micros() as u64;
            }
            WindowEvent::Focused(f) => {
                if !f {
                    // another window took over: a press on the page ends here
                    self.cancel_page_press();
                }
                self.chrome_input(Input::WindowFocus(f));
                self.redraw_soon();
            }
            WindowEvent::Occluded(covered) => {
                self.hidden = covered;
                if !covered {
                    self.redraw_soon();
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                self.mods = Mods { ctrl: s.control_key(), shift: s.shift_key(), alt: s.alt_key() };
                let m = self.mods;
                if let Some(p) = self.browser.active_page() {
                    p.set_modifiers(m.ctrl, m.shift, m.alt);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor() as f32);
                self.mouse_move(position.x as f32 / scale, position.y as f32 / scale);
            }
            WindowEvent::CursorLeft { .. } => {
                self.chrome_input(Input::MouseLeave);
                // a page that holds the pointer for a press keeps it beyond the window's edge
                let changed = !self.page_pressed() && self.browser.active_page().map_or(false, |p| p.mouse_leave());
                if self.browser.status.take().is_some() {
                    self.sync();
                } else if changed {
                    self.redraw_soon();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => match state {
                ElementState::Pressed => self.mouse_down(button),
                ElementState::Released => {
                    let b = match button {
                        WButton::Left => Some(MouseButton::Left),
                        WButton::Middle => Some(MouseButton::Middle),
                        WButton::Right => Some(MouseButton::Right),
                        _ => None,
                    };
                    if let Some(b) = b {
                        self.mouse_up(b);
                    }
                }
            },
            WindowEvent::MouseWheel { delta, .. } => {
                let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor() as f32);
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * 60.0,
                    MouseScrollDelta::PixelDelta(p) => -(p.y as f32) / scale,
                };
                self.wheel(dy);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    let text = event.text.as_ref().map(|t| t.to_string());
                    self.key(&event.logical_key, text.as_deref());
                }
            }
            _ => {}
        }
        if self.quit {
            el.exit();
        }
    }

    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        self.stats.wakes += 1;
        if matches!(cause, StartCause::ResumeTimeReached { .. }) {
            self.stats.timer_wakes += 1;
            self.tick();
            if self.wants_frames() {
                self.next_animation_frame();
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        // Sleep until input arrives unless something on screen is still changing.
        // A page timer also wakes the loop, but never more often than every 4 ms (once a
        // second while the window cannot be seen).
        let floor = if self.out_of_sight() { 1000 } else { 4 };
        let timer = self.browser.next_script_wake().map(|ms| ms.max(floor));
        if self.stats.on {
            self.stats.timer_ms = timer.unwrap_or(0);
            let branch = if self.wants_frames() {
                0
            } else if self.animating() {
                1
            } else if timer.is_some() {
                2
            } else {
                3
            };
            self.stats.branch[branch] += 1;
            let extra = format!("out-of-sight {} page-wants-frame {}", self.out_of_sight(), self.browser.active_wants_frame());
            self.stats.report(&extra);
        }
        if let Some(due) = self.play_test_input() {
            // a step of the test input is waiting: come back for it even if nothing else happens
            el.set_control_flow(ControlFlow::WaitUntil(due.min(Instant::now() + Duration::from_millis(50))));
            if !self.wants_frames() {
                return;
            }
        }
        if self.wants_frames() {
            // something on the page is moving: the next frame follows as soon as the gap allows
            // (a frame that drew waited for the display; one that did not is paced here)
            let gap = if self.presented { FRAME_GAP } else { IDLE_FRAME_GAP };
            let now = Instant::now();
            let next = self.last_frame.max(self.last_request) + gap;
            if now >= next {
                self.last_request = now;
                self.next_animation_frame();
                el.set_control_flow(ControlFlow::WaitUntil(now + gap));
            } else {
                el.set_control_flow(ControlFlow::WaitUntil(next));
            }
            return;
        }
        if self.animating() {
            let ms = timer.map_or(120, |t| t.min(120));
            el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(ms)));
        } else if let Some(ms) = timer {
            el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(ms)));
        } else {
            self.last_tick = Instant::now();
            el.set_control_flow(ControlFlow::Wait);
        }
    }
}

/// One step of scripted pointer input: at a time, move to a point of the page (`m`), or press
/// (`d`) or release (`u`) the primary button there.
#[derive(Clone, Copy)]
struct TestStep {
    at: Instant,
    what: char,
    x: f32,
    y: f32,
}

/// Debugging aid: `INVIS_TEST_INPUT="2000 m 100 40; 2100 d; 2200 u; 3000 w 600"` plays pointer input back
/// through the window's own handlers, each step that many milliseconds after start. Lets the
/// path from the mouse to the page be checked without a hand on the mouse.
fn test_steps(start: Instant) -> Vec<TestStep> {
    let Ok(text) = std::env::var("INVIS_TEST_INPUT") else { return Vec::new() };
    let mut out = Vec::new();
    let (mut x, mut y) = (0.0, 0.0);
    for step in text.split(';') {
        let mut parts = step.split_whitespace();
        let (Some(ms), Some(what)) = (parts.next().and_then(|v| v.parse::<u64>().ok()), parts.next().and_then(|v| v.chars().next())) else { continue };
        if what == 'w' {
            // the wheel: scroll by that many pixels
            let dy = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
            out.push(TestStep { at: start + Duration::from_millis(ms), what, x: dy, y: 0.0 });
            continue;
        }
        if what == 'm' {
            x = parts.next().and_then(|v| v.parse().ok()).unwrap_or(x);
            y = parts.next().and_then(|v| v.parse().ok()).unwrap_or(y);
        }
        out.push(TestStep { at: start + Duration::from_millis(ms), what, x, y });
    }
    out
}

fn page_button(b: MouseButton) -> PointerButton {
    match b {
        MouseButton::Left => PointerButton::Primary,
        MouseButton::Middle => PointerButton::Middle,
        MouseButton::Right => PointerButton::Secondary,
    }
}

/// The bit of a button in `App::page_press`.
fn press_bit(b: PointerButton) -> u8 {
    match b {
        PointerButton::Primary => 1,
        PointerButton::Middle => 2,
        PointerButton::Secondary => 4,
    }
}

pub fn run(start_url: Option<String>) -> i32 {
    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("could not start the event loop: {e}");
            return 1;
        }
    };
    let proxy = event_loop.create_proxy();
    let sink: Sink = Arc::new(move |ev| {
        let _ = proxy.send_event(UserEvent::Net(ev));
    });
    let now = Instant::now();
    let mut app = App {
        stats: LoopStats { on: std::env::var_os("INVIS_LOOP_DEBUG").is_some(), ..LoopStats::default() },
        browser: Browser::new(sink),
        chrome: Chrome::new(Theme::invis()),
        state: ChromeState::default(),
        window: None,
        renderer: None,
        chrome_list: DrawList::new(),
        chrome_glyphs: Vec::new(),
        under: DrawList::new(),
        over: DrawList::new(),
        clip: Clip(arboard::Clipboard::new().ok()),
        mods: Mods::default(),
        cursor: (0.0, 0.0),
        page_press: 0,
        page_press_id: 0,
        page_rect: Rect::ZERO,
        last_click: (now, 0.0, 0.0, 0),
        test_input: test_steps(now),
        last_tick: now,
        last_frame: now,
        redraw_wanted: std::cell::Cell::new(true),
        presented: false,
        auto_frame: std::cell::Cell::new(false),
        last_request: now,
        hidden: false,
        scroll_goal: None,
        caret_elapsed: 0,
        caret_on: true,
        shown_address: String::new(),
        shown_tab: 0,
        start_url,
        quit: false,
    };
    match event_loop.run_app(&mut app) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("event loop error: {e}");
            1
        }
    }
}

/// Most bytes one file selection may load into memory.
const MAX_PICKED_BYTES: u64 = 1 << 30;

/// The window's native handle for system dialogs (an HWND on Windows); 0 where there is none.
fn native_handle(w: &Window) -> isize {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match w.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

/// MIME type of a picked file, from its extension ("" if unknown, as browsers report it).
fn mime_of(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|p| p.1.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "md" => "text/markdown",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        "tar" => "application/x-tar",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "exe" => "application/x-msdownload",
        "wasm" => "application/wasm",
        _ => "",
    }
}
