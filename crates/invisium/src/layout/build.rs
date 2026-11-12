//! Box-tree construction: DOM + computed styles -> boxes, including anonymous-box fix-ups.

use super::*;
use crate::atom::Atom;
use crate::css::values::ContentItem;
use crate::dom::{NodeData, NO_NODE};
use crate::style::{anonymous_style, ListStyle, WhiteSpace, NO_STYLE};

fn push(l: &mut Layout, b: LBox) -> BoxId {
    l.tree.boxes.push(b);
    (l.tree.boxes.len() - 1) as BoxId
}

fn append(l: &mut Layout, parent: BoxId, child: BoxId) {
    let last = l.tree.boxes[parent as usize].last_child;
    {
        let c = &mut l.tree.boxes[child as usize];
        c.parent = parent;
        c.next = NO_BOX;
    }
    if last == NO_BOX {
        l.tree.boxes[parent as usize].first_child = child;
    } else {
        l.tree.boxes[last as usize].next = child;
    }
    l.tree.boxes[parent as usize].last_child = child;
}

fn set_children(l: &mut Layout, parent: BoxId, kids: &[BoxId]) {
    {
        let p = &mut l.tree.boxes[parent as usize];
        p.first_child = NO_BOX;
        p.last_child = NO_BOX;
    }
    for &k in kids {
        append(l, parent, k);
    }
}

fn anon_style(l: &mut Layout, parent_style: StyleId, display: Display) -> StyleId {
    if let Some(&s) = l.anon_styles.get(&(parent_style, display)) {
        return s;
    }
    let st = anonymous_style(l.styles.style(parent_style), display);
    let id = l.styles.push(st);
    l.anon_styles.insert((parent_style, display), id);
    id
}

fn is_replaced(tag: Atom) -> bool {
    matches!(
        tag,
        a::IMG | a::INPUT | a::TEXTAREA | a::SELECT | a::IFRAME | a::VIDEO | a::CANVAS | a::EMBED | a::OBJECT
            | a::SVG | a::AUDIO
    )
}

/// True if this text box holds only collapsible white space.
fn is_blank_text(l: &Layout, id: BoxId) -> bool {
    let b = l.bx(id);
    if b.kind != BoxKind::Text {
        return false;
    }
    let st = l.styles.style(b.style);
    if matches!(st.white_space, WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::PreLine) {
        return false;
    }
    let text: &str = if b.flags & F_GEN_TEXT != 0 { &l.tree.gen_text[b.aux as usize] } else { l.doc.text(b.node).unwrap_or("") };
    text.bytes().all(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0c))
}

fn roman(mut n: u32, upper: bool) -> String {
    const T: [(u32, &str); 13] = [
        (1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"),
        (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i"),
    ];
    if n == 0 || n > 3999 {
        return n.to_string();
    }
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    if upper {
        s.to_ascii_uppercase()
    } else {
        s
    }
}

fn alpha(mut n: u32, upper: bool) -> String {
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push((if upper { b'A' } else { b'a' }) + (n % 26) as u8);
        n /= 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn list_ordinal(l: &Layout, node: NodeId) -> i32 {
    let doc = l.doc;
    let parent = doc.parent(node);
    let mut n: i32 = if parent != NO_NODE { doc.attr(parent, a::START).and_then(|v| v.trim().parse().ok()).unwrap_or(1) } else { 1 };
    if parent != NO_NODE {
        for sib in doc.children(parent) {
            if doc.tag(sib) != a::LI {
                continue;
            }
            if let Some(v) = doc.attr(sib, a::VALUE).and_then(|v| v.trim().parse::<i32>().ok()) {
                n = v;
            }
            if sib == node {
                break;
            }
            n += 1;
        }
    }
    n
}

fn marker_text(l: &Layout, node: NodeId, style: ListStyle) -> String {
    let n = || list_ordinal(l, node);
    match style {
        ListStyle::None => String::new(),
        ListStyle::Disc => "\u{2022}".into(),
        ListStyle::Circle => "\u{25e6}".into(),
        ListStyle::Square => "\u{25aa}".into(),
        ListStyle::Decimal => format!("{}.", n()),
        ListStyle::DecimalLeadingZero => format!("{:02}.", n()),
        ListStyle::LowerAlpha => format!("{}.", alpha(n().max(0) as u32, false)),
        ListStyle::UpperAlpha => format!("{}.", alpha(n().max(0) as u32, true)),
        ListStyle::LowerRoman => format!("{}.", roman(n().max(0) as u32, false)),
        ListStyle::UpperRoman => format!("{}.", roman(n().max(0) as u32, true)),
    }
}

fn box_flags(st: &ComputedStyle) -> u16 {
    let mut flags = 0;
    if st.is_out_of_flow() {
        flags |= F_OUT_OF_FLOW | F_BFC;
    } else if st.float != Float::None {
        flags |= F_FLOAT | F_BFC;
    }
    if st.position != Position::Static {
        flags |= F_POSITIONED;
    }
    if st.clips() {
        flags |= F_CLIP | F_BFC;
    }
    flags
}

fn kind_for(display: Display) -> Option<BoxKind> {
    Some(match display {
        Display::Inline => BoxKind::Inline,
        Display::Block | Display::InlineBlock | Display::ListItem => BoxKind::Block,
        Display::Flex | Display::InlineFlex => BoxKind::Flex,
        Display::Grid | Display::InlineGrid => BoxKind::Grid,
        Display::Table | Display::InlineTable => BoxKind::Table,
        Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup => BoxKind::TableRowGroup,
        Display::TableRow => BoxKind::TableRow,
        Display::TableCell => BoxKind::TableCell,
        Display::TableCaption => BoxKind::TableCaption,
        Display::TableColumn | Display::TableColumnGroup | Display::None | Display::Contents => return None,
    })
}

fn build_pseudo(l: &mut Layout, pbox: BoxId, node: NodeId, psid: StyleId, flag: u16) {
    let st = l.styles.rc(psid);
    let Some(kind) = kind_for(st.display) else { return };
    let mut text = String::new();
    if let Some(items) = &st.content {
        for it in items.iter() {
            match it {
                ContentItem::Str(s) => text.push_str(s),
                ContentItem::Attr(name) => text.push_str(l.doc.attr(node, *name).unwrap_or("")),
                ContentItem::OpenQuote => text.push('\u{201c}'),
                ContentItem::CloseQuote => text.push('\u{201d}'),
            }
        }
    }
    let mut flags = box_flags(&st) | flag;
    if st.display.is_inline_level() && kind != BoxKind::Inline {
        flags |= F_INLINE_LEVEL | F_BFC;
    }
    if matches!(kind, BoxKind::Flex | BoxKind::Grid | BoxKind::Table | BoxKind::TableCell | BoxKind::TableCaption) {
        flags |= F_BFC;
    }
    let b = push(l, new_box(kind, node, psid, flags));
    append(l, pbox, b);
    if !text.is_empty() {
        l.tree.gen_text.push(text);
        let mut t = new_box(BoxKind::Text, node, psid, F_GEN_TEXT | flag);
        t.aux = (l.tree.gen_text.len() - 1) as u32;
        let t = push(l, t);
        append(l, b, t);
    }
    finish_container(l, b);
}

fn build_children(l: &mut Layout, pbox: BoxId, node: NodeId) {
    let text_style = l.styles.id_of(node);
    if let Some(&psid) = l.styles.before.get(&node) {
        build_pseudo(l, pbox, node, psid, F_BEFORE);
    }
    let doc = l.doc;
    let mut c = doc.node(node).first_child;
    while c != NO_NODE {
        match &doc.node(c).data {
            NodeData::Text(t) => {
                if !t.is_empty() && text_style != NO_STYLE {
                    let b = push(l, new_box(BoxKind::Text, c, text_style, 0));
                    append(l, pbox, b);
                }
            }
            NodeData::Element(_) => build_element(l, pbox, c),
            _ => {}
        }
        c = doc.node(c).next;
    }
    if let Some(&psid) = l.styles.after.get(&node) {
        build_pseudo(l, pbox, node, psid, F_AFTER);
    }
}

fn build_element(l: &mut Layout, pbox: BoxId, node: NodeId) {
    let sid = l.styles.id_of(node);
    if sid == NO_STYLE {
        return;
    }
    let st = l.styles.rc(sid);
    match st.display {
        Display::None => return,
        Display::Contents => {
            build_children(l, pbox, node);
            return;
        }
        _ => {}
    }
    let tag = l.doc.tag(node);
    if tag == a::BR {
        let b = push(l, new_box(BoxKind::Break, node, sid, 0));
        append(l, pbox, b);
        return;
    }
    if tag == a::WBR {
        return;
    }
    let mut flags = box_flags(&st);
    let inline_level = st.display.is_inline_level();
    if is_replaced(tag) {
        if inline_level {
            flags |= F_INLINE_LEVEL;
        }
        let b = push(l, new_box(BoxKind::Replaced, node, sid, flags | F_BFC));
        append(l, pbox, b);
        return;
    }
    let Some(kind) = kind_for(st.display) else { return };
    if inline_level && kind != BoxKind::Inline {
        flags |= F_INLINE_LEVEL | F_BFC;
    }
    if matches!(kind, BoxKind::Flex | BoxKind::Grid | BoxKind::Table | BoxKind::TableCell | BoxKind::TableCaption) {
        flags |= F_BFC;
    }
    // flex and grid items are independent formatting contexts
    let pkind = l.bx(pbox).kind;
    if matches!(pkind, BoxKind::Flex | BoxKind::Grid) && kind == BoxKind::Block {
        flags |= F_BFC;
    }
    let mut nb = new_box(kind, node, sid, flags);
    if kind == BoxKind::TableCell {
        let span = |name: Atom| -> u32 {
            l.doc.attr(node, name).and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(1).clamp(1, 1000)
        };
        nb.aux = span(a::COLSPAN) | (span(a::ROWSPAN).min(65535) << 16);
    }
    let b = push(l, nb);
    append(l, pbox, b);

    if st.display == Display::ListItem && st.list_style != ListStyle::None {
        let text = marker_text(l, node, st.list_style);
        if !text.is_empty() {
            let font = l.fonts.resolve(&st.font_family, st.font_weight, st.italic);
            let g0 = l.tree.glyphs.len() as u32;
            let w = l.fonts.shape(font, st.font_size, &text, 0.0, &mut l.tree.glyphs);
            let mut m = new_box(BoxKind::Marker, node, sid, 0);
            m.r0 = g0;
            m.r1 = l.tree.glyphs.len() as u32;
            m.w = w;
            let m = push(l, m);
            append(l, b, m);
        }
    }
    build_children(l, b, node);
    finish_container(l, b);
}

/// Wrap `run` (children of `parent`) in a new anonymous box of `kind` and return it.
fn wrap(l: &mut Layout, parent: BoxId, run: &[BoxId], kind: BoxKind, display: Display, flags: u16) -> BoxId {
    let pstyle = l.bx(parent).style;
    let node = l.bx(parent).node;
    let sid = anon_style(l, pstyle, display);
    let anon = push(l, new_box(kind, node, sid, F_ANON | flags));
    l.tree.boxes[anon as usize].parent = parent;
    for &k in run {
        append(l, anon, k);
    }
    anon
}

/// Group consecutive children matching `pred` into anonymous wrappers; blank text between them
/// is dropped. `after` runs on each wrapper created.
fn wrap_runs(
    l: &mut Layout,
    id: BoxId,
    pred: &dyn Fn(&Layout, BoxId) -> bool,
    kind: BoxKind,
    display: Display,
    flags: u16,
    after: &dyn Fn(&mut Layout, BoxId),
) {
    let kids = l.child_ids(id);
    let mut out: Vec<BoxId> = Vec::with_capacity(kids.len());
    let mut run: Vec<BoxId> = Vec::new();
    let mut created: Vec<BoxId> = Vec::new();
    let flush = |l: &mut Layout, run: &mut Vec<BoxId>, out: &mut Vec<BoxId>, created: &mut Vec<BoxId>| {
        if run.is_empty() {
            return;
        }
        // a run made only of blank text and out-of-flow boxes needs no wrapper
        let substantive = run.iter().any(|&k| !is_blank_text(l, k) && !l.bx(k).is_neutral());
        if substantive {
            let anon = wrap(l, id, run, kind, display, flags);
            out.push(anon);
            created.push(anon);
        } else {
            for &k in run.iter() {
                if !is_blank_text(l, k) {
                    out.push(k);
                }
            }
        }
        run.clear();
    };
    for k in kids {
        if pred(l, k) || (l.bx(k).is_neutral() && !run.is_empty()) {
            run.push(k);
        } else {
            flush(l, &mut run, &mut out, &mut created);
            out.push(k);
        }
    }
    flush(l, &mut run, &mut out, &mut created);
    set_children(l, id, &out);
    for c in created {
        after(l, c);
    }
}

fn is_table_part(kind: BoxKind) -> bool {
    matches!(kind, BoxKind::TableRowGroup | BoxKind::TableRow | BoxKind::TableCell | BoxKind::TableCaption)
}

fn fix_row(l: &mut Layout, id: BoxId) {
    wrap_runs(
        l,
        id,
        &|l, k| l.bx(k).kind != BoxKind::TableCell && !l.bx(k).is_neutral(),
        BoxKind::TableCell,
        Display::TableCell,
        F_BFC,
        &|l, c| {
            l.bxm(c).aux = 1 | (1 << 16);
            normalize_block(l, c);
        },
    );
}

fn fix_row_group(l: &mut Layout, id: BoxId) {
    wrap_runs(
        l,
        id,
        &|l, k| l.bx(k).kind != BoxKind::TableRow && !l.bx(k).is_neutral(),
        BoxKind::TableRow,
        Display::TableRow,
        0,
        &|l, c| fix_row(l, c),
    );
}

fn fix_table(l: &mut Layout, id: BoxId) {
    wrap_runs(
        l,
        id,
        &|l, k| {
            let kind = l.bx(k).kind;
            !matches!(kind, BoxKind::TableRowGroup | BoxKind::TableRow | BoxKind::TableCaption) && !l.bx(k).is_neutral()
        },
        BoxKind::TableRow,
        Display::TableRow,
        0,
        &|l, c| fix_row(l, c),
    );
}

/// Decide whether a block container holds inline content or block children, wrapping stray
/// inline runs in anonymous blocks when it holds both.
fn normalize_block(l: &mut Layout, id: BoxId) {
    // stray table parts (e.g. display: table-cell without a table) get an anonymous table
    let has_stray = l.tree.children(id).any(|k| is_table_part(l.bx(k).kind));
    if has_stray {
        wrap_runs(l, id, &|l, k| is_table_part(l.bx(k).kind), BoxKind::Table, Display::Table, F_BFC, &|l, c| fix_table(l, c));
    }
    let mut has_block = false;
    let mut has_inline = false;
    for k in l.tree.children(id) {
        let b = l.bx(k);
        if b.is_neutral() {
            continue;
        }
        if b.is_inline_level() {
            has_inline = true;
        } else {
            has_block = true;
        }
    }
    if !has_block {
        if has_inline {
            l.bxm(id).flags |= F_IFC;
        }
        return;
    }
    if !has_inline {
        return;
    }
    wrap_runs(
        l,
        id,
        &|l, k| l.bx(k).is_inline_level(),
        BoxKind::Block,
        Display::Block,
        F_IFC,
        &|_, _| {},
    );
}

/// Children of a flex or grid container: each becomes an item; loose inline content (text, or
/// inline boxes hoisted out of a `display: contents` child) is wrapped in an anonymous item.
fn wrap_items(l: &mut Layout, id: BoxId) {
    wrap_runs(l, id, &|l, k| l.bx(k).is_inline_level(), BoxKind::Block, Display::Block, F_IFC | F_BFC, &|_, _| {});
    // every item establishes its own formatting context
    for k in l.child_ids(id) {
        let b = l.bxm(k);
        if !b.is_neutral() {
            b.flags |= F_BFC;
        }
    }
}

fn finish_container(l: &mut Layout, id: BoxId) {
    match l.bx(id).kind {
        BoxKind::Inline => {
            // An inline box that contains a block is promoted to a block (block-in-inline).
            let has_block = l.tree.children(id).any(|k| {
                let b = l.bx(k);
                !b.is_neutral() && !b.is_inline_level()
            });
            if has_block {
                l.bxm(id).kind = BoxKind::Block;
                normalize_block(l, id);
            }
        }
        BoxKind::Text | BoxKind::Break | BoxKind::Replaced | BoxKind::Marker => {}
        BoxKind::Flex | BoxKind::Grid => wrap_items(l, id),
        BoxKind::Table => fix_table(l, id),
        BoxKind::TableRowGroup => fix_row_group(l, id),
        BoxKind::TableRow => fix_row(l, id),
        BoxKind::Block | BoxKind::TableCell | BoxKind::TableCaption => normalize_block(l, id),
    }
}

pub(super) fn build_tree(l: &mut Layout) {
    let root_node = l.doc.document_element();
    if root_node == NO_NODE {
        return;
    }
    let sid = l.styles.id_of(root_node);
    if sid == NO_STYLE || l.styles.style(sid).display == Display::None {
        return;
    }
    l.tree.boxes.reserve(l.doc.nodes.len());
    let st = l.styles.rc(sid);
    let root = push(l, new_box(BoxKind::Block, root_node, sid, F_BFC | box_flags(&st) & F_POSITIONED));
    l.tree.root = root;
    build_children(l, root, root_node);
    finish_container(l, root);
}
