//! Layout, input routing, painting and animation of the top bar.

use crate::draw::{DrawList, FontId, Glyph, Rect, Rgba, Stop, TextShaper, NO_RADII};
use crate::field::TextField;
use crate::theme::Theme;
use crate::{Action, ChromeState, Clipboard, CursorKind, Input, Key, Mods, MouseButton, Outcome, Security, TorIndicator};

pub(crate) mod panel;

const ICON_MORE: &str = "\u{E712}";
const TOR_W_ON: f32 = 52.0;
const TOR_W_CONNECTING: f32 = 92.0;
const TOR_W_FAILED: f32 = 84.0;

const ICON_BACK: &str = "\u{E72B}";
const ICON_FORWARD: &str = "\u{E72A}";
const ICON_REFRESH: &str = "\u{E72C}";
const ICON_CANCEL: &str = "\u{E711}";
const ICON_ADD: &str = "\u{E710}";
const ICON_CLOSE: &str = "\u{E8BB}";
const ICON_LOCK: &str = "\u{E72E}";
const ICON_SHIELD: &str = "\u{EA18}";
const ICON_GLOBE: &str = "\u{E774}";
const ICON_WARNING: &str = "\u{E7BA}";

const CARET_BLINK_MS: u32 = 530;
const SPIN_STEP_MS: u32 = 120;
const SHIELD_W: f32 = 52.0;
const PLUS_W: f32 = 28.0;
const MARGIN: f32 = 8.0;
const DRAG_THRESHOLD: f32 = 4.0;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Page,
    Bar,
    Tab(usize),
    Close(usize),
    Plus,
    Back,
    Forward,
    Reload,
    Address,
    Shield,
    Menu,
    Tor,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Drag {
    None,
    Field(usize),
    Tab { id: u64, start_x: f32, moved: bool },
}

#[derive(Clone, Copy)]
pub(crate) struct Fonts {
    pub(crate) ui: FontId,
    pub(crate) icons: FontId,
}

pub(crate) struct Geo {
    pub strip: Rect,
    pub toolbar: Rect,
    pub back: Rect,
    pub forward: Rect,
    pub reload: Rect,
    pub address: Rect,
    pub sec: Rect,
    pub shield: Rect,
    pub text: Rect,
    pub menu: Rect,
    /// Empty when the Tor indicator is off.
    pub tor: Rect,
}

pub struct Chrome {
    pub(crate) panel_open: bool,
    pub(crate) panel_scroll: f32,
    tor: TorIndicator,
    theme: Theme,
    w: f32,
    h: f32,
    pub(crate) field: TextField,
    url: String,
    focused: bool,
    window_focus: bool,
    hover: Hit,
    pub(crate) drag: Drag,
    caret_ms: u32,
    spin_ms: u32,
}

/// Width of each character, cumulative: `v[i]` is the x of the boundary before char `i`.
fn offsets(shaper: &mut dyn TextShaper, font: FontId, size: f32, text: &str) -> Vec<f32> {
    let mut v = Vec::with_capacity(text.len() + 1);
    let mut scratch = Vec::new();
    let mut x = 0.0;
    v.push(0.0);
    let mut buf = [0u8; 4];
    for c in text.chars() {
        scratch.clear();
        x += shaper.shape(font, size, c.encode_utf8(&mut buf), &mut scratch);
        v.push(x);
    }
    v
}

fn put(list: &mut DrawList, glyphs: &mut Vec<Glyph>, shaper: &mut dyn TextShaper, font: FontId, size: f32, x: f32, baseline: f32, color: Rgba, s: &str) -> f32 {
    let start = glyphs.len() as u32;
    let w = shaper.shape(font, size, s, glyphs);
    list.glyphs(x, baseline, w, size, color, (start, glyphs.len() as u32));
    w
}

fn baseline_in(shaper: &mut dyn TextShaper, font: FontId, size: f32, r: &Rect) -> f32 {
    let m = shaper.metrics(font, size);
    r.y + (r.h - (m.ascent + m.descent)) / 2.0 + m.ascent
}

/// Draw `s` centred in `r`.
fn put_centered(list: &mut DrawList, glyphs: &mut Vec<Glyph>, shaper: &mut dyn TextShaper, font: FontId, size: f32, r: &Rect, color: Rgba, s: &str) {
    let mut scratch = Vec::new();
    let w = shaper.shape(font, size, s, &mut scratch);
    let b = baseline_in(shaper, font, size, r);
    put(list, glyphs, shaper, font, size, r.x + (r.w - w) / 2.0, b, color, s);
}

/// Shorten `text` with a trailing "…" so it fits in `max_w`; empty if even the ellipsis does not.
fn elide(shaper: &mut dyn TextShaper, font: FontId, size: f32, text: &str, max_w: f32) -> String {
    let t: Vec<char> = text.chars().filter(|c| !c.is_control()).take(256).collect();
    let mut scratch = Vec::new();
    let full: String = t.iter().collect();
    if shaper.shape(font, size, &full, &mut scratch) <= max_w {
        return full;
    }
    let build = |k: usize| -> String {
        let mut s: String = t[..k].iter().collect();
        s = s.trim_end().to_string();
        s.push('…');
        s
    };
    let (mut lo, mut hi) = (0usize, t.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        scratch.clear();
        if shaper.shape(font, size, &build(mid), &mut scratch) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let s = build(lo);
    scratch.clear();
    if shaper.shape(font, size, &s, &mut scratch) <= max_w {
        s
    } else {
        String::new()
    }
}

impl Chrome {
    pub fn new(theme: Theme) -> Chrome {
        Chrome {
            panel_open: false,
            panel_scroll: 0.0,
            tor: TorIndicator::Off,
            theme,
            w: 0.0,
            h: 0.0,
            field: TextField::default(),
            url: String::new(),
            focused: false,
            window_focus: true,
            hover: Hit::Page,
            drag: Drag::None,
            caret_ms: 0,
            spin_ms: 0,
        }
    }

    /// Window size in logical pixels. Returns the rectangle left for page content.
    pub fn layout(&mut self, width: f32, height: f32) -> Rect {
        self.w = if width.is_finite() { width.max(0.0) } else { 0.0 };
        self.h = if height.is_finite() { height.max(0.0) } else { 0.0 };
        self.content_rect()
    }

    fn content_y(&self) -> f32 {
        self.theme.tab_strip_h + self.theme.toolbar_h
    }

    pub fn content_rect(&self) -> Rect {
        let y = self.content_y().min(self.h);
        Rect::new(0.0, y, self.w, (self.h - y).max(0.0))
    }

    pub(crate) fn geo(&self) -> Geo {
        let t = &self.theme;
        let strip = Rect::new(0.0, 0.0, self.w, t.tab_strip_h);
        let toolbar = Rect::new(0.0, t.tab_strip_h, self.w, t.toolbar_h);
        let by = toolbar.y + (toolbar.h - t.button) / 2.0;
        let b = t.button;
        let back = Rect::new(MARGIN, by, b, b);
        let forward = Rect::new(back.right() + 4.0, by, b, b);
        let reload = Rect::new(forward.right() + 4.0, by, b, b);
        let menu = Rect::new((self.w - MARGIN - b).max(0.0), by, b, b);
        let tor_w = match self.tor {
            TorIndicator::Off => 0.0,
            TorIndicator::On => TOR_W_ON,
            TorIndicator::Connecting(_) => TOR_W_CONNECTING,
            TorIndicator::Failed => TOR_W_FAILED,
        };
        let tor = if tor_w > 0.0 { Rect::new(menu.x - 8.0 - tor_w, toolbar.y + (toolbar.h - 24.0) / 2.0, tor_w, 24.0) } else { Rect::new(menu.x - 8.0, by, 0.0, 0.0) };
        let ax = reload.right() + MARGIN;
        let aw = (tor.x - MARGIN - ax).max(0.0);
        let ay = toolbar.y + (toolbar.h - t.field_h) / 2.0;
        let address = Rect::new(ax, ay, aw, t.field_h);
        let sec = Rect::new(ax + 8.0, ay, 20.0, t.field_h);
        let sw = SHIELD_W.min(aw);
        let shield = Rect::new(ax + aw - sw, ay, sw, t.field_h);
        let tx = ax + 32.0;
        let text = Rect::new(tx, ay, (shield.x - 4.0 - tx).max(0.0), t.field_h);
        Geo { strip, toolbar, back, forward, reload, address, sec, shield, text, menu, tor }
    }

    /// Rectangles of `n` tabs and of the "+" button; all inside the strip.
    pub(crate) fn tab_rects(&self, n: usize) -> (Vec<Rect>, Rect) {
        let t = &self.theme;
        let avail = (self.w - 2.0 * MARGIN - PLUS_W - 4.0).max(0.0);
        let y = t.tab_strip_h - t.tab_h;
        let mut tw = 0.0;
        if n > 0 {
            tw = ((avail - t.tab_gap * (n as f32 - 1.0)) / n as f32).clamp(0.0, t.tab_max_w);
        }
        let rects: Vec<Rect> = (0..n).map(|i| Rect::new(MARGIN + i as f32 * (tw + t.tab_gap), y, tw, t.tab_h)).collect();
        let px = match rects.last() {
            Some(r) => r.right() + 4.0,
            None => MARGIN,
        };
        let plus = Rect::new(px, (t.tab_strip_h - PLUS_W) / 2.0, PLUS_W, PLUS_W);
        (rects, plus)
    }

    pub(crate) fn close_rect(&self, tab: &Rect, active: bool) -> Option<Rect> {
        if tab.w >= 64.0 || (active && tab.w >= 40.0) {
            Some(Rect::new(tab.right() - 26.0, tab.y + (tab.h - 20.0) / 2.0, 20.0, 20.0))
        } else {
            None
        }
    }

    fn fonts(&self, shaper: &mut dyn TextShaper) -> Fonts {
        Fonts { ui: shaper.font(&self.theme.font_ui, 400, false), icons: shaper.font(&self.theme.font_icons, 400, false) }
    }

    fn hit(&self, x: f32, y: f32, state: &ChromeState) -> Hit {
        if !(y < self.content_y()) {
            return Hit::Page;
        }
        if y < self.theme.tab_strip_h {
            let (rects, plus) = self.tab_rects(state.tabs.len());
            if plus.contains(x, y) {
                return Hit::Plus;
            }
            for (i, r) in rects.iter().enumerate() {
                if r.contains(x, y) {
                    let on_close = self.close_rect(r, i == state.active).map_or(false, |c| c.contains(x, y));
                    return if on_close { Hit::Close(i) } else { Hit::Tab(i) };
                }
            }
            return Hit::Bar;
        }
        let g = self.geo();
        if g.back.contains(x, y) {
            Hit::Back
        } else if g.forward.contains(x, y) {
            Hit::Forward
        } else if g.reload.contains(x, y) {
            Hit::Reload
        } else if g.menu.contains(x, y) {
            Hit::Menu
        } else if g.tor.contains(x, y) {
            Hit::Tor
        } else if g.shield.contains(x, y) {
            Hit::Shield
        } else if g.address.contains(x, y) {
            Hit::Address
        } else {
            Hit::Bar
        }
    }

    // ---------------------------------------------------------------- public state API

    pub fn set_address(&mut self, url: &str) {
        self.url = url.chars().filter(|c| !c.is_control()).take(2048).collect();
        if !self.focused {
            self.field.set_text(&self.url);
        }
    }

    pub fn focus_address(&mut self, select_all: bool) {
        self.focused = true;
        self.caret_ms = 0;
        if select_all {
            self.field.select_all();
        } else {
            self.field.end(false);
        }
    }

    pub fn address_focused(&self) -> bool {
        self.focused
    }

    /// Privacy panel popover.
    pub fn panel_open(&self) -> bool {
        self.panel_open
    }

    pub fn close_panel(&mut self) {
        self.panel_open = false;
        self.panel_scroll = 0.0;
    }

    pub(crate) fn blur(&mut self, restore: bool) {
        self.focused = false;
        self.drag = Drag::None;
        if restore {
            self.field.set_text(&self.url);
        } else {
            self.field.anchor = self.field.caret;
            self.field.scroll = 0.0;
        }
    }

    // ---------------------------------------------------------------- text field helpers

    fn hit_index(&self, x: f32, shaper: &mut dyn TextShaper) -> usize {
        let f = self.fonts(shaper);
        let offs = offsets(shaper, f.ui, self.theme.text_size, &self.field.text);
        let local = x - self.geo().text.x + self.field.scroll;
        let i = offs.iter().position(|&o| o >= local).unwrap_or(offs.len() - 1);
        if i > 0 && local - offs[i - 1] < offs[i] - local {
            i - 1
        } else {
            i
        }
    }

    /// Scroll the field so the caret is visible.
    fn fix_scroll(&mut self, shaper: &mut dyn TextShaper) {
        let f = self.fonts(shaper);
        let offs = offsets(shaper, f.ui, self.theme.text_size, &self.field.text);
        let width = (self.geo().text.w - 2.0).max(0.0);
        let total = offs[offs.len() - 1];
        let cx = offs[self.field.caret.min(offs.len() - 1)];
        let mut s = self.field.scroll;
        if cx - s > width {
            s = cx - width;
        }
        if cx < s {
            s = cx;
        }
        self.field.scroll = s.min((total - width).max(0.0)).max(0.0);
    }

    fn edited(&mut self, shaper: &mut dyn TextShaper, o: &mut Outcome) {
        self.fix_scroll(shaper);
        self.caret_ms = 0;
        o.consumed = true;
        o.redraw = true;
    }

    // ---------------------------------------------------------------- input

    pub fn handle(&mut self, input: &Input, state: &ChromeState, shaper: &mut dyn TextShaper, clipboard: &mut dyn Clipboard) -> Outcome {
        let mut o = Outcome::default();
        self.tor = state.tor.clone();
        if self.panel_input(input, state, &mut o) {
            return o;
        }
        match input {
            Input::MouseMove { x, y } => self.on_move(*x, *y, state, shaper, &mut o),
            Input::MouseDown { x, y, button, clicks } => self.on_down(*x, *y, *button, *clicks, state, shaper, &mut o),
            Input::MouseUp { x, y, .. } => {
                if self.drag != Drag::None {
                    self.drag = Drag::None;
                    o.consumed = true;
                } else {
                    o.consumed = *y < self.content_y();
                }
                let _ = x;
            }
            Input::MouseLeave => {
                if self.hover != Hit::Page {
                    self.hover = Hit::Page;
                    o.redraw = true;
                }
            }
            Input::Wheel { y, .. } => o.consumed = *y < self.content_y(),
            Input::Key { key, mods } => self.on_key(key, *mods, state, shaper, clipboard, &mut o),
            Input::Text(s) => {
                if self.focused {
                    self.field.insert(s);
                    self.edited(shaper, &mut o);
                }
            }
            Input::WindowFocus(f) => {
                self.window_focus = *f;
                if !*f {
                    self.drag = Drag::None;
                }
                o.redraw = true;
            }
        }
        o
    }

    fn on_move(&mut self, x: f32, y: f32, state: &ChromeState, shaper: &mut dyn TextShaper, o: &mut Outcome) {
        match self.drag {
            Drag::Field(anchor) => {
                let idx = self.hit_index(x, shaper);
                self.field.anchor = anchor;
                self.field.caret = idx;
                self.fix_scroll(shaper);
                o.consumed = true;
                o.redraw = true;
                o.cursor = Some(CursorKind::Text);
                return;
            }
            Drag::Tab { id, start_x, moved } => {
                let moved = moved || (x - start_x).abs() > DRAG_THRESHOLD;
                self.drag = Drag::Tab { id, start_x, moved };
                if moved {
                    if let Some(cur) = state.tabs.iter().position(|t| t.id == id) {
                        let (rects, _) = self.tab_rects(state.tabs.len());
                        let step = rects.first().map_or(0.0, |r| r.w + self.theme.tab_gap);
                        if step > 0.0 {
                            let to = (((x - MARGIN) / step).floor() as isize).clamp(0, state.tabs.len() as isize - 1) as usize;
                            if to != cur {
                                o.actions.push(Action::MoveTab { id, to });
                                o.redraw = true;
                            }
                        }
                    }
                }
                o.consumed = true;
                o.cursor = Some(CursorKind::Default);
                return;
            }
            Drag::None => {}
        }
        let hit = self.hit(x, y, state);
        let hover = if matches!(hit, Hit::Bar) { Hit::Page } else { hit };
        if hover != self.hover {
            self.hover = hover;
            o.redraw = true;
        }
        o.consumed = hit != Hit::Page;
        o.cursor = match hit {
            Hit::Page => None,
            Hit::Address => Some(CursorKind::Text),
            Hit::Tab(_) | Hit::Close(_) | Hit::Plus | Hit::Reload | Hit::Menu | Hit::Shield => Some(CursorKind::Pointer),
            Hit::Back => Some(if state.can_back { CursorKind::Pointer } else { CursorKind::Default }),
            Hit::Forward => Some(if state.can_forward { CursorKind::Pointer } else { CursorKind::Default }),
            Hit::Bar | Hit::Tor => Some(CursorKind::Default),
        };
    }

    #[allow(clippy::too_many_arguments)]
    fn on_down(&mut self, x: f32, y: f32, button: MouseButton, clicks: u8, state: &ChromeState, shaper: &mut dyn TextShaper, o: &mut Outcome) {
        let hit = self.hit(x, y, state);
        if self.focused && hit != Hit::Address {
            self.blur(true);
            o.redraw = true;
            if hit == Hit::Page {
                o.actions.push(Action::FocusPage);
            }
        }
        o.consumed = hit != Hit::Page;
        o.redraw = true;
        let left = button == MouseButton::Left;
        match hit {
            Hit::Page | Hit::Bar | Hit::Tor => {}
            Hit::Shield => {
                if left {
                    self.panel_open = true;
                    self.panel_scroll = 0.0;
                }
            }
            Hit::Menu => {
                if left {
                    o.actions.push(Action::OpenInternal("settings"));
                }
            }
            Hit::Tab(i) => {
                if let Some(t) = state.tabs.get(i) {
                    match button {
                        MouseButton::Left => {
                            o.actions.push(Action::SelectTab(t.id));
                            self.drag = Drag::Tab { id: t.id, start_x: x, moved: false };
                        }
                        MouseButton::Middle => o.actions.push(Action::CloseTab(t.id)),
                        MouseButton::Right => {}
                    }
                }
            }
            Hit::Close(i) => {
                if let (true, Some(t)) = (left, state.tabs.get(i)) {
                    o.actions.push(Action::CloseTab(t.id));
                }
            }
            Hit::Plus => {
                if left {
                    o.actions.push(Action::NewTab);
                }
            }
            Hit::Back => {
                if left && state.can_back {
                    o.actions.push(Action::Back);
                }
            }
            Hit::Forward => {
                if left && state.can_forward {
                    o.actions.push(Action::Forward);
                }
            }
            Hit::Reload => {
                if left {
                    let loading = state.tabs.get(state.active).map_or(false, |t| t.loading);
                    o.actions.push(if loading { Action::Stop } else { Action::Reload });
                }
            }
            Hit::Address => {
                if !left {
                    return;
                }
                if !self.focused {
                    self.focus_address(true);
                    return;
                }
                self.caret_ms = 0;
                let idx = self.hit_index(x, shaper);
                match clicks {
                    0 | 1 => {
                        self.field.move_to(idx, false);
                        self.drag = Drag::Field(self.field.anchor);
                    }
                    2 => {
                        let (a, b) = self.field.word_range(idx);
                        self.field.anchor = a;
                        self.field.caret = b;
                    }
                    _ => self.field.select_all(),
                }
                self.fix_scroll(shaper);
            }
        }
    }

    fn on_key(&mut self, key: &Key, mods: Mods, state: &ChromeState, shaper: &mut dyn TextShaper, clipboard: &mut dyn Clipboard, o: &mut Outcome) {
        let n = state.tabs.len();
        let active_id = state.tabs.get(state.active).map(|t| t.id);
        let act = |a: Action, o: &mut Outcome| {
            o.actions.push(a);
            o.consumed = true;
        };
        match key {
            Key::Char(c) if mods.ctrl && !mods.alt => match c.to_ascii_lowercase() {
                't' => act(Action::NewTab, o),
                'w' => {
                    if let Some(id) = active_id {
                        act(Action::CloseTab(id), o);
                    }
                    o.consumed = true;
                }
                'l' => {
                    self.focus_address(true);
                    o.consumed = true;
                    o.redraw = true;
                }
                'r' => act(Action::Reload, o),
                '+' | '=' => act(Action::ZoomIn, o),
                '-' | '_' => act(Action::ZoomOut, o),
                '0' => act(Action::ZoomReset, o),
                d @ '1'..='9' => {
                    let idx = if d == '9' { n.wrapping_sub(1) } else { d as usize - '1' as usize };
                    if let Some(t) = state.tabs.get(idx) {
                        act(Action::SelectTab(t.id), o);
                    }
                    o.consumed = true;
                }
                'a' | 'c' | 'x' | 'v' if self.focused => self.clipboard_key(c.to_ascii_lowercase(), shaper, clipboard, o),
                _ => {}
            },
            Key::Tab if mods.ctrl => {
                if n > 0 {
                    let i = if mods.shift { (state.active + n - 1) % n } else { (state.active + 1) % n };
                    act(Action::SelectTab(state.tabs[i].id), o);
                }
                o.consumed = true;
            }
            Key::F6 => {
                self.focus_address(true);
                o.consumed = true;
                o.redraw = true;
            }
            Key::F5 => act(Action::Reload, o),
            Key::Char(c) if mods.alt && !mods.ctrl && c.eq_ignore_ascii_case(&'d') => {
                self.focus_address(true);
                o.consumed = true;
                o.redraw = true;
            }
            Key::Left if mods.alt => act(Action::Back, o),
            Key::Right if mods.alt => act(Action::Forward, o),
            Key::Escape => {
                if self.focused {
                    self.blur(true);
                    act(Action::FocusPage, o);
                    o.redraw = true;
                }
            }
            _ if self.focused => self.field_key(key, mods, shaper, o),
            _ => {}
        }
    }

    fn clipboard_key(&mut self, c: char, shaper: &mut dyn TextShaper, clipboard: &mut dyn Clipboard, o: &mut Outcome) {
        match c {
            'a' => self.field.select_all(),
            'c' => {
                if let Some(s) = self.field.selected_text() {
                    clipboard.set(&s);
                }
            }
            'x' => {
                if let Some(s) = self.field.selected_text() {
                    clipboard.set(&s);
                    self.field.delete_selection();
                }
            }
            _ => {
                if let Some(s) = clipboard.get() {
                    self.field.insert(&s);
                }
            }
        }
        self.edited(shaper, o);
    }

    fn field_key(&mut self, key: &Key, mods: Mods, shaper: &mut dyn TextShaper, o: &mut Outcome) {
        let (ext, word) = (mods.shift, mods.ctrl);
        match key {
            Key::Left => self.field.left(ext, word),
            Key::Right => self.field.right(ext, word),
            Key::Home | Key::Up | Key::PageUp => self.field.home(ext),
            Key::End | Key::Down | Key::PageDown => self.field.end(ext),
            Key::Backspace => self.field.backspace(word),
            Key::Delete => self.field.delete(word),
            Key::Enter => {
                let text = self.field.text.trim().to_string();
                if !text.is_empty() {
                    o.actions.push(Action::Navigate(text));
                }
                o.actions.push(Action::FocusPage);
                self.blur(false);
                o.consumed = true;
                o.redraw = true;
                return;
            }
            _ => {}
        }
        self.edited(shaper, o);
    }

    // ---------------------------------------------------------------- animation

    /// Advance time (caret blink, spinner). Returns true if a redraw is needed.
    pub fn tick(&mut self, dt_ms: u32, state: &ChromeState) -> bool {
        let mut redraw = false;
        if self.focused {
            let before = self.caret_ms / CARET_BLINK_MS % 2;
            self.caret_ms = self.caret_ms.saturating_add(dt_ms);
            redraw |= self.caret_ms / CARET_BLINK_MS % 2 != before;
        }
        if state.progress.is_some() || state.tabs.iter().any(|t| t.loading) {
            let before = self.spin_ms / SPIN_STEP_MS;
            self.spin_ms = self.spin_ms.saturating_add(dt_ms);
            redraw |= self.spin_ms / SPIN_STEP_MS != before;
        }
        redraw
    }

    // ---------------------------------------------------------------- painting

    /// Paint the whole chrome in logical pixels. Appends to `list` and `glyphs`.
    pub fn paint(&mut self, state: &ChromeState, shaper: &mut dyn TextShaper, list: &mut DrawList, glyphs: &mut Vec<Glyph>) {
        self.tor = state.tor.clone();
        let f = self.fonts(shaper);
        let g = self.geo();
        let t = self.theme.clone();
        list.rect(g.strip, t.bg_deep);
        self.paint_tabs(state, &t, f, shaper, list, glyphs);
        list.rect(g.toolbar, t.bg_body);
        list.rect(Rect::new(0.0, g.toolbar.bottom() - 1.0, g.toolbar.w, 1.0), t.border);

        let loading = state.tabs.get(state.active).map_or(false, |x| x.loading);
        let buttons = [
            (g.back, ICON_BACK, state.can_back, Hit::Back),
            (g.forward, ICON_FORWARD, state.can_forward, Hit::Forward),
            (g.reload, if loading { ICON_CANCEL } else { ICON_REFRESH }, true, Hit::Reload),
        ];
        for (r, icon, enabled, which) in buttons {
            if enabled && self.hover == which {
                list.rounded_rect(r, t.hover_overlay, [t.radius_xs; 4]);
            }
            let c = if enabled { t.text_secondary } else { t.text_disabled };
            put_centered(list, glyphs, shaper, f.icons, t.icon_size + 2.0, &r, c, icon);
        }
        self.paint_address(state, &t, f, &g, shaper, list, glyphs);

        if let Some(p) = state.progress {
            let w = g.toolbar.w * if p.is_finite() { p.clamp(0.0, 1.0) } else { 0.0 };
            let bar = Rect::new(0.0, g.toolbar.bottom() - t.progress_h, w, t.progress_h);
            if !bar.is_empty() {
                let stops = list.add_stops(&[Stop { offset: 0.0, color: t.blue_600 }, Stop { offset: 1.0, color: t.blue_400 }]);
                list.push(crate::draw::Item::LinearGradient { rect: bar, radii: NO_RADII, angle: 90.0, stops, repeating: false });
            }
        }
        self.paint_toolbar_extras(state, &t, f, &g, shaper, list, glyphs);
        self.paint_overlays(state, &t, f, &g, shaper, list, glyphs);
    }

    fn paint_tabs(&self, state: &ChromeState, t: &Theme, f: Fonts, shaper: &mut dyn TextShaper, list: &mut DrawList, glyphs: &mut Vec<Glyph>) {
        let (rects, plus) = self.tab_rects(state.tabs.len());
        for (i, (tab, r)) in state.tabs.iter().zip(rects.iter()).enumerate() {
            if r.is_empty() {
                continue;
            }
            let active = i == state.active;
            let hovered = matches!(self.hover, Hit::Tab(j) | Hit::Close(j) if j == i);
            let bg = if active {
                t.bg_body
            } else if hovered {
                t.bg_card
            } else {
                Rgba::TRANSPARENT
            };
            list.rounded_rect(*r, bg, [t.radius_sm - 2.0, t.radius_sm - 2.0, 0.0, 0.0]);
            list.push_clip(*r, NO_RADII);
            let fg = if active {
                t.text_primary
            } else if hovered {
                t.text_secondary
            } else {
                t.text_muted
            };
            let mut tx = r.x + 8.0;
            if r.w >= 72.0 {
                let icon = Rect::new(r.x + 10.0, r.y, 14.0, r.h);
                if tab.loading {
                    let phase = (self.spin_ms / SPIN_STEP_MS % 4) as usize;
                    let c = [t.blue_400, t.blue_500, t.blue_600, t.blue_700][phase];
                    list.rounded_rect(Rect::new(icon.x + 3.0, r.y + (r.h - 8.0) / 2.0, 8.0, 8.0), c, [4.0; 4]);
                } else {
                    let c = if matches!(tab.security, Security::Tor | Security::Onion) { t.blue_400 } else { t.text_muted };
                    put_centered(list, glyphs, shaper, f.icons, t.icon_size, &icon, c, ICON_GLOBE);
                }
                tx = r.x + 30.0;
            }
            let close = self.close_rect(r, active);
            let right = close.map_or(r.right() - 8.0, |c| c.x - 4.0);
            if right - tx >= 10.0 {
                let label = if !tab.title.trim().is_empty() {
                    &tab.title
                } else if !tab.url.is_empty() {
                    &tab.url
                } else {
                    "New Tab"
                };
                let s = elide(shaper, f.ui, t.text_size, label, right - tx);
                if !s.is_empty() {
                    let b = baseline_in(shaper, f.ui, t.text_size, r);
                    put(list, glyphs, shaper, f.ui, t.text_size, tx, b, fg, &s);
                }
            }
            if let Some(c) = close {
                let ch = self.hover == Hit::Close(i);
                if ch {
                    list.rounded_rect(c, t.hover_overlay, [4.0; 4]);
                }
                put_centered(list, glyphs, shaper, f.icons, 10.0, &c, if ch { t.text_primary } else { t.text_muted }, ICON_CLOSE);
            }
            list.pop_clip();
        }
        if self.hover == Hit::Plus {
            list.rounded_rect(plus, t.hover_overlay, [t.radius_xs; 4]);
        }
        put_centered(list, glyphs, shaper, f.icons, t.icon_size, &plus, t.text_secondary, ICON_ADD);
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_address(&self, state: &ChromeState, t: &Theme, f: Fonts, g: &Geo, shaper: &mut dyn TextShaper, list: &mut DrawList, glyphs: &mut Vec<Glyph>) {
        let a = g.address;
        if a.is_empty() {
            return;
        }
        let radii = [t.radius_sm; 4];
        if self.focused {
            list.stroke(a.inflate(3.0), 3.0, t.border_light, [t.radius_sm + 3.0; 4]);
        }
        list.rounded_rect(a, t.bg_deep, radii);
        list.stroke(a, 1.0, if self.focused { t.blue_500 } else { t.border }, radii);

        let active = state.tabs.get(state.active);
        let (icon, color) = match active.map(|x| &x.security) {
            Some(Security::Secure) => (ICON_LOCK, t.success),
            Some(Security::Insecure) => (ICON_WARNING, t.warning),
            Some(Security::Tor) | Some(Security::Onion) => (ICON_GLOBE, t.blue_400),
            _ => (ICON_GLOBE, t.text_muted),
        };
        put_centered(list, glyphs, shaper, f.icons, t.icon_size, &g.sec, color, icon);

        let blocked = active.map_or(0, |x| x.blocked);
        let sc = if blocked > 0 { t.blue_400 } else { t.text_muted };
        let ir = Rect::new(g.shield.x + 6.0, g.shield.y, 16.0, g.shield.h);
        put_centered(list, glyphs, shaper, f.icons, t.icon_size, &ir, sc, ICON_SHIELD);
        if blocked > 0 && g.shield.w >= SHIELD_W {
            let b = baseline_in(shaper, f.ui, t.small_size, &g.shield);
            put(list, glyphs, shaper, f.ui, t.small_size, g.shield.x + 26.0, b, sc, &blocked.min(999).to_string());
        }

        let tr = g.text;
        if tr.is_empty() {
            return;
        }
        let size = t.text_size;
        let base = baseline_in(shaper, f.ui, size, &tr);
        if self.field.text.is_empty() {
            if !self.focused {
                put(list, glyphs, shaper, f.ui, size, tr.x, base, t.text_muted, "Search or enter address");
            }
        } else {
            let offs = offsets(shaper, f.ui, size, &self.field.text);
            let n = offs.len() - 1;
            let scroll = self.field.scroll;
            list.push_clip(tr, NO_RADII);
            if let (true, Some((s0, s1))) = (self.focused, self.field.selection()) {
                let x0 = (tr.x + offs[s0] - scroll).max(tr.x);
                let x1 = (tr.x + offs[s1] - scroll).min(tr.right());
                let m = shaper.metrics(f.ui, size);
                let sel = Rect::new(x0, base - m.ascent, x1 - x0, m.ascent + m.descent);
                list.rect(sel, t.blue_700);
            }
            let i0 = (0..n).find(|&i| offs[i + 1] > scroll).unwrap_or(n);
            let i1 = (i0..n).find(|&i| offs[i] >= scroll + tr.w).unwrap_or(n);
            if i1 > i0 {
                let visible: String = self.field.text.chars().skip(i0).take(i1 - i0).collect();
                put(list, glyphs, shaper, f.ui, size, tr.x + offs[i0] - scroll, base, t.text_primary, &visible);
            }
            list.pop_clip();
        }
        if self.focused && self.window_focus && self.caret_ms / CARET_BLINK_MS % 2 == 0 {
            let offs = offsets(shaper, f.ui, size, &self.field.text);
            let cx = tr.x + offs[self.field.caret.min(offs.len() - 1)] - self.field.scroll;
            let m = shaper.metrics(f.ui, size);
            let caret = Rect::new(cx.clamp(tr.x, tr.right()), base - m.ascent, 1.5, m.ascent + m.descent);
            list.rect(caret, t.text_primary);
        }
    }
}
