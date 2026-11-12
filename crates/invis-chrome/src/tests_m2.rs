//! Milestone 2: privacy panel, Tor indicator, status bubble, menu button, internal pages.

use crate::draw::{DrawList, FontId, FontMetrics, Glyph, Item, Rect, TextShaper};
use crate::pages::{self, SettingsRow, SettingsSection};
use crate::*;

struct Shaper;
impl TextShaper for Shaper {
    fn font(&mut self, _f: &str, _w: u16, _i: bool) -> FontId {
        0
    }
    fn metrics(&mut self, _f: FontId, size: f32) -> FontMetrics {
        FontMetrics { ascent: size * 0.8, descent: size * 0.2, line_gap: 0.0, x_height: size * 0.5 }
    }
    fn shape(&mut self, _f: FontId, _s: f32, text: &str, out: &mut Vec<Glyph>) -> f32 {
        for c in text.chars() {
            out.push(Glyph { face: 0, id: (c as u32 as u16).max(1), adv: 7.0 });
        }
        7.0 * text.chars().count() as f32
    }
}

#[derive(Default)]
struct Clip;
impl Clipboard for Clip {
    fn get(&mut self) -> Option<String> {
        None
    }
    fn set(&mut self, _t: &str) {}
}

fn toggles() -> Vec<Toggle> {
    (0..12)
        .map(|i| Toggle { id: format!("t{i}"), label: format!("Toggle number {i}"), detail: "A description that is long enough to need eliding in the panel".into(), on: i % 2 == 0, section: format!("Section {}", i / 4), enabled: i != 1 })
        .collect()
}

struct Rig {
    c: Chrome,
    st: ChromeState,
}

impl Rig {
    fn new(h: f32) -> Rig {
        let mut c = Chrome::new(Theme::invis());
        c.layout(1280.0, h);
        let tab = TabInfo { id: 1, title: "T".into(), url: "https://a.test/".into(), loading: false, security: Security::Secure, blocked: 3 };
        Rig { c, st: ChromeState { tabs: vec![tab], toggles: toggles(), ..Default::default() } }
    }
    fn send(&mut self, i: Input) -> Outcome {
        self.c.handle(&i, &self.st, &mut Shaper, &mut Clip)
    }
    fn click(&mut self, x: f32, y: f32) -> Outcome {
        let o = self.send(Input::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
        self.send(Input::MouseUp { x, y, button: MouseButton::Left });
        o
    }
    fn open_panel(&mut self) {
        let (x, y) = centre(self.c.geo().shield);
        assert!(self.click(x, y).consumed);
        assert!(self.c.panel_open());
    }
    fn paint(&mut self) -> (DrawList, Vec<Glyph>) {
        let mut l = DrawList::new();
        let mut g = Vec::new();
        self.c.paint(&self.st, &mut Shaper, &mut l, &mut g);
        (l, g)
    }
    fn row_centre(&self, toggle: usize) -> (f32, f32) {
        let pg = self.c.panel_geo(&self.st);
        let (rows, _) = self.c.panel_rows(&self.st);
        let r = rows.iter().find(|r| r.kind == crate::chrome::panel::RowKind::Toggle(toggle)).unwrap();
        (pg.body.x + pg.body.w / 2.0, pg.body.y + r.y - self.c.panel_scroll + r.h / 2.0)
    }
}

fn centre(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

fn texts(l: &DrawList, g: &[Glyph]) -> Vec<String> {
    l.items
        .iter()
        .filter_map(|i| match i {
            Item::Glyphs { glyphs, .. } => Some(g[glyphs.0 as usize..glyphs.1 as usize].iter().map(|x| char::from_u32(x.id as u32).unwrap()).collect()),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------ B2 panel

#[test]
fn shield_opens_panel_and_toggle_rows_emit() {
    let mut rig = Rig::new(800.0);
    assert!(!rig.c.panel_open());
    rig.open_panel();
    let (x, y) = rig.row_centre(0);
    assert_eq!(rig.click(x, y).actions, vec![Action::Toggle("t0".into())]);
    assert!(rig.c.panel_open(), "toggling keeps the panel open");
    let (x, y) = rig.row_centre(1);
    let o = rig.click(x, y);
    assert!(o.actions.is_empty() && o.consumed, "disabled toggles emit nothing");
}

#[test]
fn wheel_scroll_keeps_rows_inside_the_clip() {
    let mut rig = Rig::new(500.0);
    rig.open_panel();
    let pg = rig.c.panel_geo(&rig.st);
    assert!(pg.max_scroll() > 0.0, "content taller than the window scrolls");
    assert!(pg.panel.bottom() <= 500.0);
    let o = rig.send(Input::Wheel { x: pg.body.x + 10.0, y: pg.body.y + 10.0, dy: 100000.0 });
    assert!(o.consumed);
    assert_eq!(rig.c.panel_scroll, pg.max_scroll());
    rig.send(Input::Wheel { x: pg.body.x + 10.0, y: pg.body.y + 10.0, dy: -100000.0 });
    assert_eq!(rig.c.panel_scroll, 0.0);
    rig.send(Input::Wheel { x: pg.body.x + 10.0, y: pg.body.y + 10.0, dy: 100000.0 });
    // The last row ends exactly at the bottom of the body and can be clicked.
    let (x, y) = rig.row_centre(11);
    assert!(y < pg.body.bottom() && y > pg.body.y);
    assert_eq!(rig.click(x, y).actions, vec![Action::Toggle("t11".into())]);

    let (l, _) = rig.paint();
    let mut depth = 0;
    for it in &l.items {
        match it {
            Item::PushClip { rect, .. } => {
                depth += 1;
                assert!(rect.x >= 0.0 && rect.bottom() <= 500.0 && rect.right() <= 1280.0);
            }
            Item::PopClip => depth -= 1,
            _ => {}
        }
        assert!(depth >= 0);
    }
    assert_eq!(depth, 0);
    // Wheel outside the panel goes through to normal routing (page area is not consumed).
    assert!(!rig.send(Input::Wheel { x: 10.0, y: 300.0, dy: 5.0 }).consumed);
}

#[test]
fn outside_click_and_escape_close() {
    let mut rig = Rig::new(800.0);
    rig.open_panel();
    let o = rig.click(20.0, 400.0);
    assert!(o.consumed && o.actions.is_empty());
    assert!(!rig.c.panel_open());

    rig.open_panel();
    let o = rig.send(Input::Key { key: Key::Escape, mods: Mods::default() });
    assert!(o.consumed && !rig.c.panel_open());

    rig.open_panel();
    let (x, y) = centre(rig.c.geo().shield);
    rig.click(x, y);
    assert!(!rig.c.panel_open(), "shield toggles the panel");

    rig.open_panel();
    rig.c.close_panel();
    assert!(!rig.c.panel_open());
}

#[test]
fn new_identity_and_settings_buttons() {
    let mut rig = Rig::new(800.0);
    rig.open_panel();
    let pg = rig.c.panel_geo(&rig.st);
    let (x, y) = centre(pg.new_identity);
    assert_eq!(rig.click(x, y).actions, vec![Action::NewIdentity]);
    assert!(!rig.c.panel_open());
    rig.open_panel();
    let (x, y) = centre(pg.settings);
    assert_eq!(rig.click(x, y).actions, vec![Action::OpenInternal("settings")]);
    assert!(!rig.c.panel_open());
}

#[test]
fn menu_button_opens_settings() {
    let mut rig = Rig::new(800.0);
    let (x, y) = centre(rig.c.geo().menu);
    assert_eq!(rig.click(x, y).actions, vec![Action::OpenInternal("settings")]);
}

// ------------------------------------------------------------------ indicators and bubble

#[test]
fn tor_indicator_states() {
    let mut rig = Rig::new(800.0);
    let off_w = rig.c.geo().address.w;
    let cases = [(TorIndicator::Connecting(45), "Tor 45%"), (TorIndicator::On, "Tor"), (TorIndicator::Failed, "Tor failed")];
    for (tor, label) in cases {
        rig.st.tor = tor;
        let (l, g) = rig.paint();
        assert!(texts(&l, &g).iter().any(|t| t == label), "{label}");
        let geo = rig.c.geo();
        assert!(!geo.tor.is_empty() && geo.tor.right() < geo.menu.x && geo.address.right() < geo.tor.x);
        assert!(geo.address.w < off_w);
    }
    rig.st.tor = TorIndicator::Off;
    let (l, g) = rig.paint();
    assert!(!texts(&l, &g).iter().any(|t| t.starts_with("Tor")));
    assert_eq!(rig.c.geo().address.w, off_w);
}

#[test]
fn status_bubble_is_bottom_left() {
    let mut rig = Rig::new(800.0);
    rig.st.status = Some("https://example.com/a/very/long/link/".repeat(20));
    let (l, g) = rig.paint();
    assert!(texts(&l, &g).iter().any(|t| t.starts_with("https://example") && t.ends_with('…')));
    let bubble = l.items.iter().find_map(|i| match i {
        Item::Rect { rect, .. } if rect.x == 0.0 && rect.bottom() == 800.0 => Some(*rect),
        _ => None,
    });
    let b = bubble.expect("bubble rect");
    assert!(b.w <= 1280.0 * 0.6 + 20.0 && b.h == 24.0);
    rig.st.status = None;
    let (l2, _) = rig.paint();
    assert!(l2.items.len() < l.items.len());
}

#[test]
fn m2_paint_uses_only_palette_colours_inside_window() {
    let mut rig = Rig::new(500.0);
    rig.st.tor = TorIndicator::Connecting(10);
    rig.st.status = Some("https://x.test/".into());
    rig.paint(); // the app paints before it routes input; geometry follows the last painted state
    rig.open_panel();
    let (l, g) = rig.paint();
    let palette = Theme::invis().palette();
    let mut used: Vec<_> = l.stops.iter().map(|s| s.color).collect();
    for it in &l.items {
        match it {
            Item::Rect { rect, color, .. } => {
                used.push(*color);
                assert!(rect.x >= 0.0 && rect.right() <= 1280.0 && rect.bottom() <= 500.0, "{it:?}");
            }
            Item::Border { rect, colors, .. } => {
                used.extend_from_slice(colors);
                assert!(rect.right() <= 1280.0 && rect.bottom() <= 500.0);
            }
            Item::Shadow { color, .. } | Item::Glyphs { color, .. } => used.push(*color),
            _ => {}
        }
        if let Item::Glyphs { glyphs, .. } = it {
            assert!(glyphs.1 as usize <= g.len());
        }
    }
    assert!(used.iter().all(|c| palette.contains(c)));
}

// ------------------------------------------------------------------ B3/B4 pages

const NASTY: &str = "<script>alert(\"x\")</script>'&";

fn all_pages() -> Vec<String> {
    let sections = vec![SettingsSection {
        title: NASTY.into(),
        blurb: NASTY.into(),
        rows: vec![
            SettingsRow::Toggle { id: "a b&c".into(), label: NASTY.into(), detail: NASTY.into(), on: true },
            SettingsRow::Choice { id: "theme\"".into(), label: NASTY.into(), detail: NASTY.into(), options: vec![("x y".into(), NASTY.into()), ("z".into(), "Z".into())], selected: "z".into() },
            SettingsRow::Text { id: NASTY.into(), label: NASTY.into(), detail: NASTY.into(), value: NASTY.into(), placeholder: NASTY.into() },
            SettingsRow::Link { label: NASTY.into(), detail: NASTY.into(), href: "invis://about".into() },
            SettingsRow::Link { label: "bad".into(), detail: "".into(), href: "https://evil.test/\"onclick=\"x".into() },
        ],
    }];
    vec![
        pages::new_tab("invis://search", true, &[(NASTY.to_string(), NASTY.to_string())]),
        pages::new_tab("invis://search", false, &[]),
        pages::settings(&sections),
        pages::about(&[(NASTY.to_string(), NASTY.to_string())]),
        pages::error(NASTY, NASTY, NASTY, Some("https://x.test/?a=\"b\"&c=<d>")),
        pages::error("Offline", "Nothing", "invis://x", None),
    ]
}

fn attr_values<'a>(html: &'a str, attr: &str) -> Vec<&'a str> {
    let pat = format!("{attr}=\"");
    html.split(&pat).skip(1).map(|r| &r[..r.find('"').unwrap()]).collect()
}

#[test]
fn pages_are_safe_complete_documents() {
    for html in all_pages() {
        assert!(html.starts_with("<!DOCTYPE html>") && html.trim_end().ends_with("</html>"));
        assert!(html.contains("<style>") && html.contains(pages::stylesheet()));
        let lower = html.to_ascii_lowercase();
        assert!(!lower.contains("<script"));
        for bad in ["src=", "url(", "@import", "<link", "<iframe", "<img", "javascript:"] {
            assert!(!lower.contains(bad), "{bad}");
        }
        for h in attr_values(&html, "href") {
            assert!(h.starts_with("invis://"), "href {h}");
        }
        for a in attr_values(&html, "action") {
            assert!(a.starts_with("invis://"), "action {a}");
        }
        assert!(!html.contains("<form") || html.contains("method=\"get\""));
        // Attribute values never contain a raw quote or angle bracket from input.
        assert!(!html.contains("\"onclick=\""));
    }
}

#[test]
fn pages_escape_interpolations() {
    let pages = all_pages();
    assert!(pages[0].contains("&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt;&#39;&amp;"));
    assert!(pages[2].contains("value=\"&lt;script&gt;"));
    assert!(pages[4].contains("invis://retry?url=https%3A%2F%2Fx.test%2F%3Fa%3D%22b%22%26c%3D%3Cd%3E"));
    assert!(pages[4].contains("<title>&lt;script&gt;"));
}

#[test]
fn settings_renders_one_row_per_input_row_with_hrefs() {
    let html = &all_pages()[2];
    assert_eq!(html.matches("class=\"row\"").count(), 5);
    assert!(html.contains("href=\"invis://settings/toggle?id=a%20b%26c\""));
    assert!(html.contains("toggle-track active"));
    assert!(html.contains("href=\"invis://settings/set?id=theme%22&amp;value=x%20y\""));
    assert!(html.contains("class=\"choice active\" href=\"invis://settings/set?id=theme%22&amp;value=z\""));
    assert!(html.contains("<form action=\"invis://settings/set\" method=\"get\">"));
    assert!(html.contains("href=\"invis://about\"><div class=\"row-text\"><div class=\"row-label\">&lt;script"));
    // A non-internal link is replaced, not passed through.
    assert!(!html.contains("evil.test"));
    let empty = pages::settings(&[]);
    assert_eq!(empty.matches("class=\"row\"").count(), 0);
}

#[test]
fn new_tab_has_search_form() {
    let html = pages::new_tab("invis://search", false, &[]);
    assert!(html.contains("<form class=\"search\" action=\"invis://search\" method=\"get\">"));
    assert!(html.contains("name=\"q\""));
}

#[test]
fn stylesheet_uses_only_supported_features() {
    let css = pages::stylesheet().to_ascii_lowercase();
    for bad in ["@keyframes", "animation", "filter", "backdrop-filter", "@import", "url(", "http"] {
        assert!(!css.contains(bad), "{bad}");
    }
    for (i, _) in css.match_indices("transform:") {
        assert!(css[i..].split(';').next().unwrap().contains("translate"));
    }
    assert!(css.contains("var(--") && css.contains("--blue-500: #3b82f6"));
}
