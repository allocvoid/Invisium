//! Privacy-panel popover, Tor indicator, menu button and status bubble.

use super::*;
use crate::draw::Item;

const PANEL_W: f32 = 320.0;
const PAD: f32 = 12.0;
const ROW_SECTION_H: f32 = 30.0;
const ROW_TOGGLE_H: f32 = 52.0;
const FOOTER_H: f32 = 92.0;
const STATUS_H: f32 = 24.0;
const ICON_SETTINGS: &str = "\u{E713}";

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum RowKind {
    Section(usize),
    Toggle(usize),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PRow {
    pub kind: RowKind,
    /// Offset from the top of the scrolled content.
    pub y: f32,
    pub h: f32,
}

pub(crate) struct PanelGeo {
    pub panel: Rect,
    pub body: Rect,
    pub new_identity: Rect,
    pub settings: Rect,
    pub content_h: f32,
}

impl PanelGeo {
    pub fn max_scroll(&self) -> f32 {
        (self.content_h - self.body.h).max(0.0)
    }
}

/// Section names in order of first appearance, and the rows grouped under them.
fn sections(state: &ChromeState) -> Vec<(String, Vec<usize>)> {
    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, t) in state.toggles.iter().enumerate() {
        match out.iter_mut().find(|(s, _)| *s == t.section) {
            Some((_, v)) => v.push(i),
            None => out.push((t.section.clone(), vec![i])),
        }
    }
    out
}

impl Chrome {
    pub(crate) fn panel_rows(&self, state: &ChromeState) -> (Vec<PRow>, f32) {
        let mut rows = Vec::new();
        let mut y = 4.0;
        for (si, (_, idxs)) in sections(state).iter().enumerate() {
            rows.push(PRow { kind: RowKind::Section(si), y, h: ROW_SECTION_H });
            y += ROW_SECTION_H;
            for &i in idxs {
                rows.push(PRow { kind: RowKind::Toggle(i), y, h: ROW_TOGGLE_H });
                y += ROW_TOGGLE_H;
            }
        }
        (rows, y + 4.0)
    }

    pub(crate) fn panel_geo(&self, state: &ChromeState) -> PanelGeo {
        let g = self.geo();
        let w = PANEL_W.min((self.w - 2.0 * MARGIN).max(0.0));
        let max_x = (self.w - MARGIN - w).max(0.0);
        let x = (g.shield.right() - w).max(MARGIN.min(max_x)).min(max_x);
        let y = g.toolbar.bottom() + 4.0;
        let (_, content_h) = self.panel_rows(state);
        let max_h = (self.h - y - MARGIN).max(FOOTER_H);
        let h = (content_h + FOOTER_H).min(max_h);
        let panel = Rect::new(x, y, w, h);
        let body = Rect::new(x, y, w, (h - FOOTER_H).max(0.0));
        let fy = y + h - FOOTER_H;
        let new_identity = Rect::new(x + PAD, fy + PAD, (w - 2.0 * PAD).max(0.0), 32.0);
        let settings = Rect::new(x + PAD, new_identity.bottom() + 4.0, (w - 2.0 * PAD).max(0.0), 32.0);
        PanelGeo { panel, body, new_identity, settings, content_h }
    }

    /// Route an event to the open panel. Returns true when the panel took it.
    pub(crate) fn panel_input(&mut self, input: &Input, state: &ChromeState, o: &mut Outcome) -> bool {
        if !self.panel_open {
            return false;
        }
        let pg = self.panel_geo(state);
        self.panel_scroll = self.panel_scroll.clamp(0.0, pg.max_scroll());
        match input {
            Input::Key { key: Key::Escape, .. } => {
                self.close_panel();
            }
            Input::MouseDown { x, y, button, .. } => {
                if pg.panel.contains(*x, *y) {
                    if *button == MouseButton::Left {
                        self.panel_click(*x, *y, state, &pg, o);
                    }
                } else {
                    self.close_panel();
                }
            }
            Input::MouseUp { x, y, .. } if pg.panel.contains(*x, *y) => {}
            Input::Wheel { x, y, dy } if pg.panel.contains(*x, *y) => {
                let d = if dy.is_finite() { *dy } else { 0.0 };
                self.panel_scroll = (self.panel_scroll + d).clamp(0.0, pg.max_scroll());
            }
            Input::MouseMove { x, y } if pg.panel.contains(*x, *y) => {
                let over = pg.new_identity.contains(*x, *y) || pg.settings.contains(*x, *y) || self.toggle_at(*x, *y, state, &pg).is_some();
                o.cursor = Some(if over { CursorKind::Pointer } else { CursorKind::Default });
            }
            _ => return false,
        }
        o.consumed = true;
        o.redraw = true;
        true
    }

    fn toggle_at(&self, x: f32, y: f32, state: &ChromeState, pg: &PanelGeo) -> Option<usize> {
        if !pg.body.contains(x, y) {
            return None;
        }
        let (rows, _) = self.panel_rows(state);
        rows.iter().find_map(|r| match r.kind {
            RowKind::Toggle(i) if y >= pg.body.y + r.y - self.panel_scroll && y < pg.body.y + r.y + r.h - self.panel_scroll => Some(i),
            _ => None,
        })
    }

    fn panel_click(&mut self, x: f32, y: f32, state: &ChromeState, pg: &PanelGeo, o: &mut Outcome) {
        if pg.new_identity.contains(x, y) {
            o.actions.push(Action::NewIdentity);
            self.close_panel();
        } else if pg.settings.contains(x, y) {
            o.actions.push(Action::OpenInternal("settings"));
            self.close_panel();
        } else if let Some(i) = self.toggle_at(x, y, state, pg) {
            let t = &state.toggles[i];
            if t.enabled {
                o.actions.push(Action::Toggle(t.id.clone()));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_toolbar_extras(&self, _state: &ChromeState, t: &Theme, f: Fonts, g: &Geo, shaper: &mut dyn TextShaper, list: &mut DrawList, glyphs: &mut Vec<Glyph>) {
        if self.hover == Hit::Menu {
            list.rounded_rect(g.menu, t.hover_overlay, [t.radius_xs; 4]);
        }
        put_centered(list, glyphs, shaper, f.icons, t.icon_size + 2.0, &g.menu, t.text_secondary, ICON_MORE);
        if g.tor.is_empty() {
            return;
        }
        let (label, color) = match &self.tor {
            TorIndicator::Connecting(p) => (format!("Tor {}%", (*p).min(100)), t.warning),
            TorIndicator::On => ("Tor".to_string(), t.success),
            TorIndicator::Failed => ("Tor failed".to_string(), t.danger),
            TorIndicator::Off => return,
        };
        list.rounded_rect(g.tor, t.bg_card, [12.0; 4]);
        list.stroke(g.tor, 1.0, color, [12.0; 4]);
        put_centered(list, glyphs, shaper, f.ui, t.small_size, &g.tor, color, &label);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_overlays(&mut self, state: &ChromeState, t: &Theme, f: Fonts, _g: &Geo, shaper: &mut dyn TextShaper, list: &mut DrawList, glyphs: &mut Vec<Glyph>) {
        self.paint_status(state, t, f, shaper, list, glyphs);
        if self.panel_open {
            self.paint_panel(state, t, f, shaper, list, glyphs);
        }
    }

    fn paint_status(&self, state: &ChromeState, t: &Theme, f: Fonts, shaper: &mut dyn TextShaper, list: &mut DrawList, glyphs: &mut Vec<Glyph>) {
        let Some(text) = &state.status else { return };
        let c = self.content_rect();
        if c.h < STATUS_H || c.w < 40.0 {
            return;
        }
        let s = elide(shaper, f.ui, t.small_size, text, (c.w * 0.6).max(120.0).min(c.w) - 20.0);
        if s.is_empty() {
            return;
        }
        let mut scratch = Vec::new();
        let tw = shaper.shape(f.ui, t.small_size, &s, &mut scratch);
        let r = Rect::new(c.x, c.bottom() - STATUS_H, (tw + 20.0).min(c.w), STATUS_H);
        let radii = [0.0, t.radius_xs, 0.0, 0.0];
        list.rounded_rect(r, t.bg_card, radii);
        list.stroke(r, 1.0, t.border, radii);
        let b = baseline_in(shaper, f.ui, t.small_size, &r);
        put(list, glyphs, shaper, f.ui, t.small_size, r.x + 10.0, b, t.text_secondary, &s);
    }

    fn paint_panel(&mut self, state: &ChromeState, t: &Theme, f: Fonts, shaper: &mut dyn TextShaper, list: &mut DrawList, glyphs: &mut Vec<Glyph>) {
        let pg = self.panel_geo(state);
        self.panel_scroll = self.panel_scroll.clamp(0.0, pg.max_scroll());
        let p = pg.panel;
        let radii = [t.radius_sm; 4];
        list.push(Item::Shadow { rect: p, radii, dx: 0.0, dy: 8.0, blur: 24.0, spread: 0.0, color: t.blue_glow, inset: false });
        list.rounded_rect(p, t.bg_card, radii);
        list.stroke(p, 1.0, t.border, radii);

        let secs = sections(state);
        let (rows, _) = self.panel_rows(state);
        list.push_clip(pg.body, NO_RADII);
        for r in &rows {
            let rr = Rect::new(p.x, pg.body.y + r.y - self.panel_scroll, p.w, r.h);
            if !rr.intersects(&pg.body) {
                continue;
            }
            match r.kind {
                RowKind::Section(si) => {
                    let b = baseline_in(shaper, f.ui, t.small_size, &rr);
                    let name = if secs[si].0.is_empty() { "Privacy" } else { secs[si].0.as_str() };
                    put(list, glyphs, shaper, f.ui, t.small_size, p.x + PAD, b, t.text_muted, name);
                }
                RowKind::Toggle(i) => self.paint_toggle_row(&state.toggles[i], &rr, t, f, shaper, list, glyphs),
            }
        }
        list.pop_clip();

        let fy = pg.new_identity.y - PAD;
        list.rect(Rect::new(p.x, fy, p.w, 1.0), t.border);
        let bs = [crate::draw::Stop { offset: 0.0, color: t.blue_600 }, crate::draw::Stop { offset: 1.0, color: t.blue_700 }];
        let stops = list.add_stops(&bs);
        list.push(Item::LinearGradient { rect: pg.new_identity, radii: [t.radius_xs; 4], angle: 135.0, stops, repeating: false });
        put_centered(list, glyphs, shaper, f.ui, t.text_size, &pg.new_identity, t.white, "New identity");
        let icon = Rect::new(pg.settings.x, pg.settings.y, 20.0, pg.settings.h);
        put_centered(list, glyphs, shaper, f.icons, t.icon_size, &icon, t.blue_400, ICON_SETTINGS);
        let b = baseline_in(shaper, f.ui, t.text_size, &pg.settings);
        put(list, glyphs, shaper, f.ui, t.text_size, pg.settings.x + 28.0, b, t.blue_400, "Privacy settings");
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_toggle_row(&self, tg: &crate::Toggle, rr: &Rect, t: &Theme, f: Fonts, shaper: &mut dyn TextShaper, list: &mut DrawList, glyphs: &mut Vec<Glyph>) {
        let track = Rect::new(rr.right() - PAD - 36.0, rr.y + (rr.h - 20.0) / 2.0, 36.0, 20.0);
        let max_w = track.x - 12.0 - (rr.x + PAD);
        let (label_c, detail_c) = if tg.enabled { (t.text_primary, t.text_muted) } else { (t.text_disabled, t.text_disabled) };
        let label = elide(shaper, f.ui, t.text_size, &tg.label, max_w);
        let detail = elide(shaper, f.ui, t.small_size, &tg.detail, max_w);
        let top = Rect::new(rr.x, rr.y + 6.0, rr.w, 20.0);
        let bot = Rect::new(rr.x, rr.y + 26.0, rr.w, 18.0);
        let b1 = baseline_in(shaper, f.ui, t.text_size, &top);
        let b2 = baseline_in(shaper, f.ui, t.small_size, &bot);
        put(list, glyphs, shaper, f.ui, t.text_size, rr.x + PAD, b1, label_c, &label);
        put(list, glyphs, shaper, f.ui, t.small_size, rr.x + PAD, b2, detail_c, &detail);
        let on = tg.on && tg.enabled;
        list.rounded_rect(track, if on { t.blue_600 } else { t.border }, [10.0; 4]);
        let kx = if tg.on { track.x + 18.0 } else { track.x + 2.0 };
        list.rounded_rect(Rect::new(kx, track.y + 2.0, 16.0, 16.0), t.white, [8.0; 4]);
    }
}
