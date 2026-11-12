use crate::draw::{DrawList, FontId, FontMetrics, Glyph, Item, Rect, TextShaper};
use crate::*;

const ADV: f32 = 7.0;

/// Fixed advance per character; the glyph id is the character's code point.
struct MockShaper;
impl TextShaper for MockShaper {
    fn font(&mut self, _family: &str, _weight: u16, _italic: bool) -> FontId {
        0
    }
    fn metrics(&mut self, _font: FontId, size: f32) -> FontMetrics {
        FontMetrics { ascent: size * 0.8, descent: size * 0.2, line_gap: 0.0, x_height: size * 0.5 }
    }
    fn shape(&mut self, _font: FontId, _size: f32, text: &str, out: &mut Vec<Glyph>) -> f32 {
        let mut n = 0.0;
        for c in text.chars() {
            out.push(Glyph { face: 0, id: (c as u32 as u16).max(1), adv: ADV });
            n += ADV;
        }
        n
    }
}

#[derive(Default)]
struct MockClipboard(Option<String>);
impl Clipboard for MockClipboard {
    fn get(&mut self) -> Option<String> {
        self.0.clone()
    }
    fn set(&mut self, text: &str) {
        self.0 = Some(text.to_string());
    }
}

fn tab(id: u64, title: &str) -> TabInfo {
    TabInfo { id, title: title.into(), url: format!("https://example.com/{id}"), loading: false, security: Security::Secure, blocked: 0 }
}

fn state(n: usize) -> ChromeState {
    ChromeState { tabs: (0..n).map(|i| tab(i as u64 + 1, &format!("A very long page title number {i} that cannot fit"))).collect(), ..Default::default() }
}

struct Rig {
    c: Chrome,
    s: MockShaper,
    cb: MockClipboard,
    st: ChromeState,
}

impl Rig {
    fn new(w: f32, h: f32, tabs: usize) -> Rig {
        let mut c = Chrome::new(Theme::invis());
        c.layout(w, h);
        Rig { c, s: MockShaper, cb: MockClipboard::default(), st: state(tabs) }
    }
    fn send(&mut self, i: Input) -> Outcome {
        self.c.handle(&i, &self.st, &mut self.s, &mut self.cb)
    }
    fn down(&mut self, x: f32, y: f32) -> Outcome {
        self.down_with(x, y, MouseButton::Left, 1)
    }
    fn down_with(&mut self, x: f32, y: f32, button: MouseButton, clicks: u8) -> Outcome {
        self.send(Input::MouseDown { x, y, button, clicks })
    }
    fn up(&mut self, x: f32, y: f32) -> Outcome {
        self.send(Input::MouseUp { x, y, button: MouseButton::Left })
    }
    fn key(&mut self, key: Key, mods: Mods) -> Outcome {
        self.send(Input::Key { key, mods })
    }
    fn ctrl(&mut self, c: char) -> Outcome {
        self.key(Key::Char(c), Mods { ctrl: true, ..Default::default() })
    }
    fn plain(&mut self, key: Key) -> Outcome {
        self.key(key, Mods::default())
    }
    fn shift(&mut self, key: Key) -> Outcome {
        self.key(key, Mods { shift: true, ..Default::default() })
    }
    fn ctrl_key(&mut self, key: Key) -> Outcome {
        self.key(key, Mods { ctrl: true, ..Default::default() })
    }
    fn text(&mut self, s: &str) -> Outcome {
        self.send(Input::Text(s.into()))
    }
    fn field(&self) -> &str {
        &self.c.field.text
    }
    fn sel(&self) -> (usize, usize) {
        (self.c.field.anchor, self.c.field.caret)
    }
    fn paint(&mut self) -> (DrawList, Vec<Glyph>) {
        let mut l = DrawList::new();
        let mut g = Vec::new();
        self.c.paint(&self.st, &mut self.s, &mut l, &mut g);
        (l, g)
    }
}

fn centre(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

fn run_text(glyphs: &[Glyph], span: (u32, u32)) -> String {
    glyphs[span.0 as usize..span.1 as usize].iter().map(|g| char::from_u32(g.id as u32).unwrap()).collect()
}

fn find_run(l: &DrawList, g: &[Glyph], first: char) -> Option<crate::draw::Rgba> {
    l.items.iter().find_map(|i| match i {
        Item::Glyphs { glyphs, color, .. } if g[glyphs.0 as usize].id == first as u32 as u16 => Some(*color),
        _ => None,
    })
}

// ------------------------------------------------------------------ A3 layout

#[test]
fn content_rect_fills_below_toolbar() {
    for (w, h) in [(1280.0, 800.0), (480.0, 600.0)] {
        let mut c = Chrome::new(Theme::invis());
        let r = c.layout(w, h);
        assert_eq!(r, c.content_rect());
        let t = Theme::invis();
        assert_eq!(r.x, 0.0);
        assert_eq!(r.y, t.tab_strip_h + t.toolbar_h);
        assert_eq!(r.w, w);
        assert_eq!(r.bottom(), h);
    }
}

#[test]
fn tabs_stay_inside_strip_and_plus_visible() {
    for (w, h) in [(1280.0, 800.0), (480.0, 600.0)] {
        for n in [1usize, 8, 40] {
            let mut rig = Rig::new(w, h, n);
            let strip = rig.c.geo().strip;
            let (rects, plus) = rig.c.tab_rects(n);
            assert_eq!(rects.len(), n);
            for (i, r) in rects.iter().enumerate() {
                assert!(r.w > 0.0 && r.x >= 0.0 && r.right() <= strip.right() && r.y >= 0.0 && r.bottom() <= strip.bottom(), "tab {i} of {n} at {w}");
                if i > 0 {
                    assert!(r.x >= rects[i - 1].right(), "tabs overlap");
                }
            }
            assert!(plus.w > 0.0 && plus.x >= rects[n - 1].right() && plus.right() <= strip.right() && plus.bottom() <= strip.bottom());
            // Clicking the plus button always works.
            let (px, py) = centre(plus);
            assert_eq!(rig.down(px, py).actions, vec![Action::NewTab]);

            // Painted titles stay inside their tab and are elided.
            let (l, g) = rig.paint();
            let mut elided = false;
            for it in &l.items {
                if let Item::Glyphs { x, baseline, w: gw, glyphs, .. } = it {
                    if *baseline < strip.bottom() {
                        let s = run_text(&g, *glyphs);
                        elided |= s.ends_with('…');
                        let inside = rects.iter().chain(std::iter::once(&plus)).any(|r| *x >= r.x && x + gw <= r.right());
                        assert!(inside, "text run {s:?} escapes its tab");
                    }
                }
            }
            if n == 8 {
                assert!(elided, "long titles are elided with an ellipsis");
            }
        }
    }
}

#[test]
fn close_buttons_hit_test() {
    let mut rig = Rig::new(1280.0, 800.0, 8);
    let (rects, _) = rig.c.tab_rects(8);
    for i in 0..8 {
        let close = rig.c.close_rect(&rects[i], i == rig.st.active).expect("close visible");
        assert!(close.x >= rects[i].x && close.right() <= rects[i].right());
        let (cx, cy) = centre(close);
        assert_eq!(rig.down(cx, cy).actions, vec![Action::CloseTab(i as u64 + 1)]);
        // The left part of the tab selects it instead.
        assert_eq!(rig.down(rects[i].x + 4.0, cy).actions, vec![Action::SelectTab(i as u64 + 1)]);
    }
    // The first pixel just left of the close button still selects.
    let close = rig.c.close_rect(&rects[0], true).unwrap();
    assert_eq!(rig.down(close.x - 1.0, close.y + 5.0).actions, vec![Action::SelectTab(1)]);
}

// ------------------------------------------------------------------ A4 interaction

#[test]
fn tab_mouse_interactions() {
    let mut rig = Rig::new(1280.0, 800.0, 3);
    let (rects, _) = rig.c.tab_rects(3);
    let (x, y) = centre(rects[1]);
    let o = rig.down(x - 20.0, y);
    assert_eq!(o.actions, vec![Action::SelectTab(2)]);
    assert!(o.consumed);
    rig.up(x - 20.0, y);
    assert_eq!(rig.down_with(x - 20.0, y, MouseButton::Middle, 1).actions, vec![Action::CloseTab(2)]);
    assert!(rig.down_with(x - 20.0, y, MouseButton::Right, 1).actions.is_empty());
}

#[test]
fn dragging_a_tab_emits_move() {
    let mut rig = Rig::new(1280.0, 800.0, 4);
    let (rects, _) = rig.c.tab_rects(4);
    let y = centre(rects[0]).1;
    rig.down(rects[0].x + 10.0, y);
    let o = rig.send(Input::MouseMove { x: rects[2].x + 10.0, y });
    assert_eq!(o.actions, vec![Action::MoveTab { id: 1, to: 2 }]);
    assert!(rig.up(0.0, y).consumed);
}

#[test]
fn back_forward_disabled_state() {
    let mut rig = Rig::new(1280.0, 800.0, 1);
    let g = rig.c.geo();
    let (bx, by) = centre(g.back);
    let (fx, fy) = centre(g.forward);
    let o = rig.down(bx, by);
    assert!(o.actions.is_empty() && o.consumed);
    assert!(rig.down(fx, fy).actions.is_empty());
    let th = Theme::invis();
    let (l, gl) = rig.paint();
    assert_eq!(find_run(&l, &gl, '\u{E72B}'), Some(th.text_disabled));
    assert_eq!(find_run(&l, &gl, '\u{E72A}'), Some(th.text_disabled));

    rig.st.can_back = true;
    rig.st.can_forward = true;
    assert_eq!(rig.down(bx, by).actions, vec![Action::Back]);
    assert_eq!(rig.down(fx, fy).actions, vec![Action::Forward]);
    let (l, gl) = rig.paint();
    assert_eq!(find_run(&l, &gl, '\u{E72B}'), Some(th.text_secondary));
}

#[test]
fn reload_becomes_stop_while_loading() {
    let mut rig = Rig::new(1280.0, 800.0, 1);
    let (rx, ry) = centre(rig.c.geo().reload);
    assert_eq!(rig.down(rx, ry).actions, vec![Action::Reload]);
    let (l, g) = rig.paint();
    assert!(find_run(&l, &g, '\u{E72C}').is_some() && find_run(&l, &g, '\u{E711}').is_none());
    rig.st.tabs[0].loading = true;
    assert_eq!(rig.down(rx, ry).actions, vec![Action::Stop]);
    let (l, g) = rig.paint();
    assert!(find_run(&l, &g, '\u{E711}').is_some() && find_run(&l, &g, '\u{E72C}').is_none());
}

#[test]
fn keyboard_shortcuts() {
    let mut rig = Rig::new(1280.0, 800.0, 4);
    rig.st.active = 1;
    assert_eq!(rig.ctrl('t').actions, vec![Action::NewTab]);
    assert_eq!(rig.ctrl('w').actions, vec![Action::CloseTab(2)]);
    assert_eq!(rig.ctrl_key(Key::Tab).actions, vec![Action::SelectTab(3)]);
    let both = Mods { ctrl: true, shift: true, alt: false };
    assert_eq!(rig.key(Key::Tab, both).actions, vec![Action::SelectTab(1)]);
    rig.st.active = 3;
    assert_eq!(rig.ctrl_key(Key::Tab).actions, vec![Action::SelectTab(1)]);
    rig.st.active = 0;
    assert_eq!(rig.key(Key::Tab, both).actions, vec![Action::SelectTab(4)]);
    assert_eq!(rig.ctrl('1').actions, vec![Action::SelectTab(1)]);
    assert_eq!(rig.ctrl('3').actions, vec![Action::SelectTab(3)]);
    assert_eq!(rig.ctrl('9').actions, vec![Action::SelectTab(4)]);
    assert!(rig.ctrl('7').actions.is_empty());
    assert_eq!(rig.ctrl('r').actions, vec![Action::Reload]);
    assert_eq!(rig.plain(Key::F5).actions, vec![Action::Reload]);
    let alt = Mods { alt: true, ..Default::default() };
    assert_eq!(rig.key(Key::Left, alt).actions, vec![Action::Back]);
    assert_eq!(rig.key(Key::Right, alt).actions, vec![Action::Forward]);
    assert_eq!(rig.ctrl('+').actions, vec![Action::ZoomIn]);
    assert_eq!(rig.ctrl('=').actions, vec![Action::ZoomIn]);
    assert_eq!(rig.ctrl('-').actions, vec![Action::ZoomOut]);
    assert_eq!(rig.ctrl('0').actions, vec![Action::ZoomReset]);
    for o in [rig.ctrl('t'), rig.ctrl('r'), rig.plain(Key::F5)] {
        assert!(o.consumed);
    }

    // Focus shortcuts select the whole address.
    rig.c.set_address("https://example.com/");
    for i in 0..3 {
        let o = match i {
            0 => rig.ctrl('l'),
            1 => rig.plain(Key::F6),
            _ => rig.key(Key::Char('d'), alt),
        };
        assert!(o.consumed);
        assert!(rig.c.address_focused());
        assert_eq!(rig.sel(), (0, 20));
        rig.c.blur(true);
    }
}

#[test]
fn page_area_events_belong_to_the_page() {
    let mut rig = Rig::new(1280.0, 800.0, 2);
    let y = rig.c.content_rect().y + 10.0;
    assert!(!rig.send(Input::MouseMove { x: 100.0, y }).consumed);
    assert!(!rig.down(100.0, y).consumed);
    assert!(!rig.up(100.0, y).consumed);
    assert!(!rig.send(Input::Wheel { x: 100.0, y, dy: 10.0 }).consumed);
    assert!(!rig.plain(Key::Down).consumed);
    assert!(!rig.text("x").consumed);
    assert!(!rig.ctrl('a').consumed);
    assert_eq!(rig.send(Input::MouseMove { x: 100.0, y }).cursor, None);
    // Chrome area is consumed.
    assert!(rig.down(5.0, 20.0).consumed);
}

#[test]
fn clicking_the_page_blurs_the_address_bar() {
    let mut rig = Rig::new(1280.0, 800.0, 1);
    rig.c.set_address("https://a.test/");
    rig.c.focus_address(true);
    rig.text("typed");
    let o = rig.down(100.0, 400.0);
    assert!(!o.consumed);
    assert_eq!(o.actions, vec![Action::FocusPage]);
    assert!(!rig.c.address_focused());
    assert_eq!(rig.field(), "https://a.test/");
}

// ------------------------------------------------------------------ A5 text field

fn focused_rig(url: &str) -> Rig {
    let mut rig = Rig::new(1280.0, 800.0, 1);
    rig.c.set_address(url);
    rig.c.focus_address(false);
    rig
}

#[test]
fn typing_and_deleting() {
    let mut rig = focused_rig("");
    assert!(rig.text("hello").consumed);
    assert_eq!(rig.field(), "hello");
    rig.plain(Key::Backspace);
    assert_eq!(rig.field(), "hell");
    rig.plain(Key::Home);
    rig.plain(Key::Delete);
    assert_eq!(rig.field(), "ell");
    rig.text("t\n\tx");
    assert_eq!(rig.field(), "t  xell");
    rig.plain(Key::End);
    assert_eq!(rig.sel(), (7, 7));
    // Key::Char without ctrl is not text (the app sends Input::Text for that).
    rig.key(Key::Char('z'), Mods::default());
    assert_eq!(rig.field(), "t  xell");
    // unfocused: Text is not consumed and does not change the field
    rig.c.blur(false);
    assert!(!rig.text("q").consumed);
    assert_eq!(rig.field(), "t  xell");
}

#[test]
fn caret_movement_and_selection() {
    let mut rig = focused_rig("foo bar  baz.qux");
    assert_eq!(rig.sel(), (16, 16));
    rig.plain(Key::Left);
    assert_eq!(rig.sel(), (15, 15));
    rig.plain(Key::Home);
    assert_eq!(rig.sel(), (0, 0));
    rig.ctrl_key(Key::Right);
    assert_eq!(rig.sel(), (4, 4)); // after "foo "
    rig.ctrl_key(Key::Right);
    assert_eq!(rig.sel(), (9, 9)); // after "bar  "
    rig.ctrl_key(Key::Left);
    assert_eq!(rig.sel(), (4, 4));
    rig.plain(Key::End);
    rig.ctrl_key(Key::Left);
    assert_eq!(rig.sel(), (13, 13)); // before "qux"
    rig.plain(Key::Home);
    rig.shift(Key::Right);
    rig.shift(Key::Right);
    assert_eq!(rig.sel(), (0, 2));
    rig.plain(Key::Right);
    assert_eq!(rig.sel(), (2, 2)); // collapse to the selection end
    rig.shift(Key::End);
    assert_eq!(rig.sel(), (2, 16));
    rig.text("X");
    assert_eq!(rig.field(), "foX");
    rig.key(Key::Left, Mods { ctrl: true, shift: true, alt: false });
    assert_eq!(rig.sel(), (3, 0));
    rig.plain(Key::Backspace);
    assert_eq!(rig.field(), "");
}

#[test]
fn ctrl_backspace_deletes_a_word() {
    let mut rig = focused_rig("one two");
    rig.key(Key::Backspace, Mods { ctrl: true, ..Default::default() });
    assert_eq!(rig.field(), "one ");
}

#[test]
fn clipboard_operations() {
    let mut rig = focused_rig("hello world");
    assert!(rig.ctrl('a').consumed);
    assert_eq!(rig.sel(), (0, 11));
    rig.ctrl('c');
    assert_eq!(rig.cb.0.as_deref(), Some("hello world"));
    rig.plain(Key::Home);
    rig.shift(Key::Right);
    rig.shift(Key::Right);
    rig.shift(Key::Right);
    rig.shift(Key::Right);
    rig.shift(Key::Right);
    rig.ctrl('x');
    assert_eq!(rig.cb.0.as_deref(), Some("hello"));
    assert_eq!(rig.field(), " world");
    rig.plain(Key::End);
    rig.ctrl('v');
    assert_eq!(rig.field(), " worldhello");
    rig.ctrl('a');
    rig.cb.0 = Some("multi\nline".into());
    rig.ctrl('v');
    assert_eq!(rig.field(), "multi line");
    rig.cb.0 = None;
    rig.ctrl('v');
    assert_eq!(rig.field(), "multi line");
}

#[test]
fn mouse_click_places_caret_between_glyphs() {
    let mut rig = focused_rig("abcdef");
    let tx = rig.c.geo().text.x;
    let y = rig.c.geo().text.y + 10.0;
    // Boundary i sits at tx + 7*i; a click within 3.5px of it picks it.
    rig.down(tx + 7.0 * 3.0 + 2.0, y);
    assert_eq!(rig.sel(), (3, 3));
    rig.up(tx, y);
    rig.down(tx + 7.0 * 3.0 + 5.0, y);
    assert_eq!(rig.sel(), (4, 4));
    rig.up(tx, y);
    rig.down(tx - 5.0, y);
    assert_eq!(rig.sel(), (0, 0));
    rig.up(tx, y);
    rig.down(tx + 600.0, y);
    assert_eq!(rig.sel(), (6, 6));
}

#[test]
fn first_click_selects_all_then_drag_selects() {
    let mut rig = Rig::new(1280.0, 800.0, 1);
    rig.c.set_address("abcdef");
    let tx = rig.c.geo().text.x;
    let y = rig.c.geo().text.y + 10.0;
    rig.down(tx + 14.0, y);
    assert!(rig.c.address_focused());
    assert_eq!(rig.sel(), (0, 6));
    rig.up(tx + 14.0, y);
    rig.down(tx + 7.0, y);
    let o = rig.send(Input::MouseMove { x: tx + 7.0 * 4.0, y });
    assert!(o.consumed);
    assert_eq!(rig.sel(), (1, 4));
    rig.up(tx + 28.0, y);
    assert!(matches!(rig.c.drag, crate::chrome::Drag::None));
}

#[test]
fn double_and_triple_click() {
    let mut rig = focused_rig("foo bar.baz");
    let tx = rig.c.geo().text.x;
    let y = rig.c.geo().text.y + 10.0;
    rig.down_with(tx + 7.0 * 5.0, y, MouseButton::Left, 2);
    assert_eq!(rig.sel(), (4, 7));
    rig.up(tx, y);
    rig.down_with(tx + 7.0 * 5.0, y, MouseButton::Left, 3);
    assert_eq!(rig.sel(), (0, 11));
}

#[test]
fn enter_navigates_and_escape_restores() {
    let mut rig = focused_rig("https://a.test/");
    rig.c.focus_address(true);
    rig.text("rust lang");
    let o = rig.plain(Key::Enter);
    assert_eq!(o.actions, vec![Action::Navigate("rust lang".into()), Action::FocusPage]);
    assert!(o.consumed && !rig.c.address_focused());

    rig.c.set_address("https://b.test/");
    rig.c.focus_address(true);
    rig.text("garbage");
    let o = rig.plain(Key::Escape);
    assert_eq!(o.actions, vec![Action::FocusPage]);
    assert!(!rig.c.address_focused());
    assert_eq!(rig.field(), "https://b.test/");
    // Escape with nothing focused belongs to the page.
    assert!(!rig.plain(Key::Escape).consumed);
}

#[test]
fn long_text_scrolls_to_keep_caret_visible() {
    let long = "x".repeat(300);
    let mut rig = focused_rig(&long);
    let tw = rig.c.geo().text.w;
    rig.text("y");
    let caret_x = 301.0 * ADV;
    assert!(rig.c.field.scroll > 0.0);
    assert!(caret_x - rig.c.field.scroll <= tw && caret_x - rig.c.field.scroll >= 0.0);
    rig.plain(Key::Home);
    assert_eq!(rig.c.field.scroll, 0.0);
    rig.plain(Key::End);
    assert!(rig.c.field.scroll > 0.0);
    // The painted text run starts left of the field and stays inside the window.
    let (l, _) = rig.paint();
    let g = rig.c.geo();
    for it in &l.items {
        if let Item::Glyphs { x, w, baseline, .. } = it {
            if *baseline > g.toolbar.y {
                assert!(*x >= 0.0 && x + w <= 1280.0);
            }
        }
    }
}

// ------------------------------------------------------------------ A6 paint

fn colours(l: &DrawList) -> Vec<crate::draw::Rgba> {
    let mut v: Vec<_> = l.stops.iter().map(|s| s.color).collect();
    for it in &l.items {
        match it {
            Item::Rect { color, .. } | Item::Glyphs { color, .. } => v.push(*color),
            Item::Border { colors, .. } => v.extend_from_slice(colors),
            Item::Shadow { color, .. } => v.push(*color),
            _ => {}
        }
    }
    v
}

fn busy_rig() -> Rig {
    let mut rig = Rig::new(1280.0, 800.0, 5);
    rig.st.tabs[1].loading = true;
    rig.st.tabs[2].security = Security::Insecure;
    rig.st.tabs[3].security = Security::Onion;
    rig.st.tabs[0].blocked = 12;
    rig.st.progress = Some(0.4);
    rig.st.can_back = true;
    rig.c.set_address("https://example.com/path");
    rig.c.focus_address(true);
    rig.send(Input::MouseMove { x: 40.0, y: 20.0 });
    rig
}

#[test]
fn paint_is_well_formed() {
    let mut rig = busy_rig();
    let (l, g) = rig.paint();
    assert!(!l.is_empty());
    let win = Rect::new(0.0, 0.0, 1280.0, 800.0);
    let inside = |r: &Rect| r.x >= win.x && r.y >= win.y && r.right() <= win.right() && r.bottom() <= win.bottom();
    let mut depth = 0i32;
    for it in &l.items {
        match it {
            Item::Rect { rect, .. } | Item::Border { rect, .. } | Item::PushClip { rect, .. } | Item::LinearGradient { rect, .. } => assert!(inside(rect), "{it:?}"),
            Item::Glyphs { x, baseline, w, glyphs, .. } => {
                assert!(glyphs.0 < glyphs.1 && glyphs.1 as usize <= g.len());
                assert!(*x >= 0.0 && x + w <= win.w && *baseline <= win.h);
            }
            _ => {}
        }
        match it {
            Item::PushClip { .. } => depth += 1,
            Item::PopClip => {
                depth -= 1;
                assert!(depth >= 0);
            }
            _ => {}
        }
    }
    assert_eq!(depth, 0);
    let palette = Theme::invis().palette();
    for c in colours(&l) {
        assert!(palette.contains(&c), "colour {c:?} is not in the theme palette");
    }
    // Progress line is a gradient 40% of the window wide.
    assert!(l.items.iter().any(|i| matches!(i, Item::LinearGradient { rect, .. } if (rect.w - 512.0).abs() < 0.01)));
}

#[test]
fn paint_is_deterministic_and_narrow_window_is_valid() {
    let mut rig = busy_rig();
    let a = rig.paint();
    let b = rig.paint();
    assert_eq!(a.0.items, b.0.items);
    assert_eq!(a.0.stops, b.0.stops);
    assert_eq!(a.1, b.1);

    let mut narrow = Rig::new(480.0, 600.0, 40);
    narrow.st.progress = Some(1.0);
    narrow.c.set_address(&"https://example.com/".repeat(10));
    let (l, g) = narrow.paint();
    for it in &l.items {
        if let Item::Glyphs { x, w, glyphs, .. } = it {
            assert!(glyphs.1 as usize <= g.len() && *x >= 0.0 && x + w <= 480.0);
        }
    }
}

#[test]
fn tick_animates_caret_and_spinner() {
    let mut rig = busy_rig();
    let st = rig.st.clone();
    assert!(!rig.c.tick(10, &st));
    assert!(rig.c.tick(600, &st)); // caret blink phase and spinner both move
    rig.c.blur(false);
    rig.st.tabs[1].loading = false;
    rig.st.progress = None;
    let st = rig.st.clone();
    assert!(!rig.c.tick(1000, &st));
}
