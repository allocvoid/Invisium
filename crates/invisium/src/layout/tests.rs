//! End-to-end layout tests: HTML + CSS in, box geometry out.

use super::*;
use crate::css::parser::{parse_stylesheet, MediaEnv, MediaQueryList, Origin};
use crate::dom::NO_NODE;
use crate::html;
use crate::style::{StyleInput, Stylist};

struct Doc {
    doc: Document,
    styles: StyleStore,
    tree: LayoutTree,
    fonts: FontSystem,
}

fn lay(body: &str, css: &str, vw: f32) -> Doc {
    let src = format!("<!doctype html><html><head></head><body>{body}</body></html>");
    let doc = html::parse(&src, false);
    let mut stylist = Stylist::new();
    let resolve = |u: &str| Some(u.to_string());
    let base = "html,body{margin:0;padding:0} body{font:16px/20px Arial}";
    stylist.add_sheet(Rc::new(parse_stylesheet(&format!("{base}{css}"), Origin::Author, &resolve)), MediaQueryList::default());
    let input = StyleInput {
        doc: &doc,
        env: MediaEnv { width: vw, height: 600.0, dark: false },
        hover: &[],
        active: &[],
        focus: NO_NODE,
        target: NO_NODE,
        resolve: &resolve,
        scripting: false,
    };
    let mut styles = stylist.compute(&input);
    let mut fonts = FontSystem::new();
    let images = HashMap::new();
    let tree = layout_document(LayoutInput { doc: &doc, styles: &mut styles, fonts: &mut fonts, viewport_w: vw, viewport_h: 600.0, image_sizes: &images });
    Doc { doc, styles, tree, fonts }
}

impl Doc {
    /// Absolute border-box rectangle of the element with this id: (x, y, w, h).
    fn rect(&self, id: &str) -> (f32, f32, f32, f32) {
        let node = self.doc.element_by_id(id);
        assert_ne!(node, NO_NODE, "no element #{id}");
        let b = self.tree.box_for_node(node);
        assert_ne!(b, NO_BOX, "no box for #{id}");
        let (x, y) = self.tree.abs_pos(b);
        let bx = &self.tree.boxes[b as usize];
        (x, y, bx.w, bx.h)
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.6
}

macro_rules! assert_rect {
    ($d:expr, $id:expr, $x:expr, $y:expr, $w:expr, $h:expr) => {{
        let (x, y, w, h) = $d.rect($id);
        assert!(
            close(x, $x as f32) && close(y, $y as f32) && close(w, $w as f32) && close(h, $h as f32),
            "#{}: got ({x}, {y}, {w}, {h}), expected ({}, {}, {}, {})",
            $id,
            $x,
            $y,
            $w,
            $h
        );
    }};
}

#[test]
fn block_flow_and_box_model() {
    let d = lay(
        "<div id=a></div><div id=b></div><div id=c><div id=d></div></div>",
        "#a{height:50px} #b{height:30px;margin:10px 20px;padding:5px;border:2px solid} #c{width:50%;padding:10px;box-sizing:border-box} #d{height:10px;margin:0 auto;width:100px}",
        800.0,
    );
    assert_rect!(d, "a", 0, 0, 800, 50);
    assert_rect!(d, "b", 20, 60, 760, 44);
    assert_rect!(d, "c", 0, 114, 400, 30);
    assert_rect!(d, "d", 150, 124, 100, 10);
    assert!(close(d.tree.height, 144.0), "document height {}", d.tree.height);
}

#[test]
fn margins_collapse() {
    let d = lay(
        "<div id=w><p id=p1>a</p><p id=p2>b</p></div><div id=after></div>",
        "p{margin:16px 0;height:20px} #after{height:5px}",
        800.0,
    );
    // the first paragraph's top margin collapses through the wrapper
    assert_rect!(d, "w", 0, 16, 800, 56);
    assert_rect!(d, "p1", 0, 16, 800, 20);
    assert_rect!(d, "p2", 0, 52, 800, 20);
    assert_rect!(d, "after", 0, 88, 800, 5);
}

#[test]
fn inline_wrapping_and_alignment() {
    let d = lay(
        "<div id=n>aaaa bbbb cccc dddd eeee ffff gggg hhhh</div><div id=c><span id=s>hi</span></div><div id=one>x</div>",
        "#n{width:100px} #c{text-align:center;width:200px}",
        800.0,
    );
    let (_, _, w, h) = d.rect("n");
    assert!(close(w, 100.0));
    assert!(h >= 60.0 && (h / 20.0).fract().abs() < 0.01, "wrapped height {h} should be a whole number of 20px lines");
    assert_rect!(d, "one", 0, h + 20.0, 800, 20);
    // centred text: the fragment sits in the middle of the 200px box
    let c = d.tree.box_for_node(d.doc.element_by_id("c"));
    let cb = &d.tree.boxes[c as usize];
    let f = d.tree.fragments[cb.r0 as usize];
    assert!(close(f.x + f.w / 2.0, 100.0), "fragment centre {}", f.x + f.w / 2.0);
}

#[test]
fn flex_row_and_column() {
    let d = lay(
        "<div id=f><div id=a>a</div><div id=b>b</div><div id=c>c</div></div>\
         <div id=g><div id=g1></div><div id=g2></div></div>\
         <div id=col><div id=c1></div><div id=c2></div></div>",
        "#f{display:flex;width:300px;height:60px;align-items:center;gap:0} #f>div{flex:1}\
         #g{display:flex;justify-content:space-between;width:300px} #g>div{width:50px;height:10px}\
         #col{display:flex;flex-direction:column;width:200px;height:100px} #c1{height:30px} #c2{flex:1}",
        800.0,
    );
    assert_rect!(d, "a", 0, 20, 100, 20);
    assert_rect!(d, "b", 100, 20, 100, 20);
    assert_rect!(d, "c", 200, 20, 100, 20);
    assert_rect!(d, "g1", 0, 60, 50, 10);
    assert_rect!(d, "g2", 250, 60, 50, 10);
    assert_rect!(d, "c1", 0, 70, 200, 30);
    assert_rect!(d, "c2", 0, 100, 200, 70);
}

#[test]
fn flex_row_min_height_gives_align_items_room() {
    // a full-height hero: auto height, min-height, content centred on the cross axis
    let d = lay(
        "<section id=hero><div id=c></div></section><section id=tall><div id=e></div></section>",
        "section{display:flex;align-items:center} #hero{min-height:400px} #c{width:100px;height:100px}
         #tall{min-height:50px;align-items:flex-end} #e{width:10px;height:80px}",
        800.0,
    );
    assert_rect!(d, "hero", 0, 0, 800, 400);
    assert_rect!(d, "c", 0, 150, 100, 100);
    // content taller than the minimum: the minimum changes nothing
    assert_rect!(d, "tall", 0, 400, 800, 80);
    assert_rect!(d, "e", 0, 400, 10, 80);
}

#[test]
fn flex_wrap_and_shrink() {
    let d = lay(
        "<div id=w><div id=a></div><div id=b></div><div id=c></div></div><div id=s><div id=s1></div><div id=s2></div></div>",
        "#w{display:flex;flex-wrap:wrap;width:250px;gap:10px} #w>div{width:120px;height:20px}\
         #s{display:flex;width:100px} #s>div{width:80px;height:10px}",
        800.0,
    );
    assert_rect!(d, "a", 0, 0, 120, 20);
    assert_rect!(d, "b", 130, 0, 120, 20);
    assert_rect!(d, "c", 0, 30, 120, 20);
    // two 80px items shrink equally into 100px
    assert_rect!(d, "s1", 0, 50, 50, 10);
    assert_rect!(d, "s2", 50, 50, 50, 10);
}

#[test]
fn positioned_boxes() {
    let d = lay(
        "<div id=rel><div id=abs></div><div id=fill></div></div><div id=fixed></div>",
        "#rel{position:relative;width:200px;height:100px;margin:10px} #abs{position:absolute;right:0;bottom:0;width:30px;height:20px}\
         #fill{position:absolute;top:5px;left:5px;right:5px;bottom:5px} #fixed{position:fixed;top:0;right:0;width:40px;height:40px}",
        800.0,
    );
    assert_rect!(d, "rel", 10, 10, 200, 100);
    assert_rect!(d, "abs", 180, 90, 30, 20);
    assert_rect!(d, "fill", 15, 15, 190, 90);
    assert_rect!(d, "fixed", 760, 0, 40, 40);
}

#[test]
fn tables() {
    let d = lay(
        "<table id=t cellspacing=0 cellpadding=0><tr><td id=a><div style='width:100px;height:20px'></div></td><td id=b><div style='width:50px;height:40px'></div></td></tr>\
         <tr><td id=c colspan=2><div style='height:10px'></div></td></tr></table>",
        "",
        800.0,
    );
    assert_rect!(d, "t", 0, 0, 150, 50);
    assert_rect!(d, "a", 0, 0, 100, 40);
    assert_rect!(d, "b", 100, 0, 50, 40);
    assert_rect!(d, "c", 0, 40, 150, 10);
}

#[test]
fn grid_tracks() {
    let d = lay(
        "<div id=g><div id=a></div><div id=b></div><div id=c></div><div id=d></div></div>",
        "#g{display:grid;grid-template-columns:100px 1fr 2fr;gap:10px;width:420px} #g>div{height:20px} #d{grid-column:span 2}",
        800.0,
    );
    assert_rect!(d, "a", 0, 0, 100, 20);
    assert_rect!(d, "b", 110, 0, 100, 20);
    assert_rect!(d, "c", 220, 0, 200, 20);
    assert_rect!(d, "d", 0, 30, 210, 20);
}

#[test]
fn floats() {
    let d = lay(
        "<div id=w><div id=l></div><div id=r></div><p id=p>text</p><div id=clear></div></div>",
        "#w{width:300px} #l{float:left;width:100px;height:50px} #r{float:right;width:60px;height:30px} p{margin:0} #clear{clear:both;height:5px}",
        800.0,
    );
    assert_rect!(d, "l", 0, 0, 100, 50);
    assert_rect!(d, "r", 240, 0, 60, 30);
    // the paragraph's line box starts to the right of the left float
    let p = d.tree.box_for_node(d.doc.element_by_id("p"));
    let pb = &d.tree.boxes[p as usize];
    let f = d.tree.fragments[pb.r0 as usize];
    assert!(close(f.x, 100.0), "text starts at {}", f.x);
    assert_rect!(d, "clear", 0, 50, 300, 5);
}

#[test]
fn nested_inline_blocks_keep_their_own_fragments() {
    // Regression: an inline-block lays itself out while its parent is still building lines, and
    // its text fragments used to land inside the parent's fragment range (painted twice).
    let d = lay(
        "<div id=o>before <span id=i>inner text</span> after<br>second <span id=j>more</span> line</div>",
        "#i,#j{display:inline-block;padding:4px}",
        800.0,
    );
    let o = d.tree.box_for_node(d.doc.element_by_id("o"));
    let ob = &d.tree.boxes[o as usize];
    for f in &d.tree.fragments[ob.r0 as usize..ob.r1 as usize] {
        if f.kind == FragKind::Text {
            // every text fragment of the outer block belongs to a text box that is its direct child
            assert_eq!(d.tree.boxes[f.box_id as usize].parent, o, "foreign fragment in the outer block's range");
        }
    }
    let atomics = d.tree.fragments[ob.r0 as usize..ob.r1 as usize].iter().filter(|f| f.kind == FragKind::Atomic).count();
    assert_eq!(atomics, 2);
    for id in ["i", "j"] {
        let b = &d.tree.boxes[d.tree.box_for_node(d.doc.element_by_id(id)) as usize];
        assert_eq!(b.r1 - b.r0, 1, "#{id} has exactly its own text fragment");
    }
}

#[test]
fn form_controls_in_flex_rows_keep_their_height() {
    // Regression: a text input stretched horizontally by flex must not grow taller in proportion.
    let d = lay(
        "<form id=f><input id=q><button id=b>Go</button></form><input id=wide>",
        "#f{display:flex;width:500px} #q{flex:1;padding:12px 16px;border:1px solid;font-size:14px} #b{padding:4px 20px;font-size:14px;border:0}\
         #wide{display:block;width:100%;padding:12px 16px;border:1px solid;font-size:14px;box-sizing:border-box}",
        800.0,
    );
    // a width in CSS does not make a text field taller either
    let (_, _, ww, wh) = d.rect("wide");
    assert!(close(ww, 800.0) && wh > 36.0 && wh < 52.0, "wide input is {ww} x {wh}");
    let (_, _, qw, qh) = d.rect("q");
    assert!(qw > 300.0, "input grew to fill the row: {qw}");
    assert!(qh > 36.0 && qh < 52.0, "input height stays one line plus padding: {qh}");
    let (_, _, _, bh) = d.rect("b");
    assert!(close(bh, qh), "button stretches to the row height");
    // the button label is centred vertically in the stretched button
    let b = &d.tree.boxes[d.tree.box_for_node(d.doc.element_by_id("b")) as usize];
    let f = d.tree.fragments[b.r0 as usize];
    assert!(close(f.y + f.h / 2.0, bh / 2.0), "label centre {} vs button centre {}", f.y + f.h / 2.0, bh / 2.0);
}

#[test]
fn paints_a_display_list() {
    use crate::draw::Item;
    use crate::paint::{paint, PaintInput};
    let mut d = lay(
        "<div id=card><b>Hello</b> world<div id=pill></div></div>",
        "#card{background:#1f2937;border:1px solid #374151;border-radius:16px;padding:24px;color:#f3f4f6}\
         #pill{width:36px;height:20px;background:linear-gradient(135deg,#2563eb,#1e40af);border-radius:100px}",
        400.0,
    );
    let (images, bg, forms) = (HashMap::new(), HashMap::new(), HashMap::new());
    let out = paint(
        &mut d.tree,
        PaintInput {
            doc: &d.doc,
            styles: &d.styles,
            fonts: &mut d.fonts,
            images: &images,
            bg_images: &bg,
            form_values: &forms,
            focus: NO_NODE,
            caret_on: false,
            viewport_w: 400.0,
            viewport_h: 600.0,
        },
    );
    let items = &out.list.items;
    assert!(items.iter().any(|i| matches!(i, Item::Rect { radii, .. } if radii[0] == 16.0)), "rounded card background");
    assert!(items.iter().any(|i| matches!(i, Item::Border { .. })));
    assert!(items.iter().any(|i| matches!(i, Item::LinearGradient { angle, .. } if *angle == 135.0)));
    let glyph_runs: Vec<_> = items.iter().filter(|i| matches!(i, Item::Glyphs { .. })).collect();
    assert!(glyph_runs.len() >= 2, "bold and regular runs");
    for i in items {
        if let Item::Glyphs { glyphs, .. } = i {
            assert!(glyphs.1 as usize <= d.tree.glyphs.len());
        }
    }
}
