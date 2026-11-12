//! Native primitives for the DOM tree: navigation, mutation, attributes, text, selectors, markup.
//! The DOM classes scripts see are defined in `dom.js` on top of these.
//!
//! Borrow rule: a native converts its arguments first (conversion can run script), then borrows
//! the shared state, and releases the borrow before calling back into the runtime.

use super::host::{CeEvent, ConsoleLevel, HostState, Shared};
use super::serialize::serialize;
use crate::atom::{a, Atom};
use crate::css::selector::{matches, parse_selector_list, MatchCtx, PseudoElement, Selector};
use crate::css::tokenizer::{split_commas, tokenize_value};
use crate::dom::{NodeData, NodeId, DOCUMENT_NODE, NO_NODE};
use invisium_js::{ClassId, Runtime, Thrown, Value};
use std::rc::Rc;

pub(crate) struct Cx {
    pub st: Shared,
    pub class: ClassId,
}

pub(crate) type NRes = Result<Value, Thrown>;
pub(crate) type Native = fn(&mut Runtime, &Cx, &[Value]) -> NRes;

pub(crate) fn def(rt: &mut Runtime, obj: Value, cx: &Rc<Cx>, name: &str, arity: u32, f: Native) {
    let cx = cx.clone();
    let func = rt.function(name, arity, Rc::new(move |rt: &mut Runtime, _this: Value, args: &[Value]| f(rt, &cx, args)));
    // installed from outside any native call: keep the function alive until it is stored
    let root = rt.root(func);
    let _ = rt.set(obj, name, func);
    rt.unroot(root);
}

#[inline]
pub(crate) fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).copied().unwrap_or(Value::Undefined)
}

pub(crate) fn err(rt: &mut Runtime, class: &str, msg: &str) -> Thrown {
    rt.throw_error(class, msg)
}

/// The node behind a wrapper, or a TypeError.
pub(crate) fn node(rt: &mut Runtime, cx: &Cx, v: Value) -> Result<NodeId, Thrown> {
    if let Some((c, id)) = rt.host_id(v) {
        if c == cx.class && cx.st.borrow().page.doc.is_valid(id as NodeId) {
            return Ok(id as NodeId);
        }
    }
    Err(err(rt, "TypeError", "argument is not a Node"))
}

/// Like `node`, but `null`/`undefined` give `NO_NODE`.
pub(crate) fn opt_node(rt: &mut Runtime, cx: &Cx, v: Value) -> Result<NodeId, Thrown> {
    match v {
        Value::Null | Value::Undefined => Ok(NO_NODE),
        _ => node(rt, cx, v),
    }
}

fn proto_for(st: &HostState, id: NodeId) -> Option<Value> {
    let key = match &st.page.doc.node(id).data {
        NodeData::Element(e) => {
            if e.tag == st.fragment_tag {
                "#fragment"
            } else if let Some(p) = st.protos.get(&e.tag) {
                return Some(*p);
            } else if e.tag.as_str().contains('-') {
                "#custom"
            } else {
                "#element"
            }
        }
        NodeData::Text(_) => "#text",
        NodeData::Comment(_) => "#comment",
        NodeData::Document => "#document",
        NodeData::Doctype => "#doctype",
    };
    st.protos.get(&Atom::new(key)).copied()
}

/// The JS wrapper for a node (`null` for `NO_NODE`). Wrappers are created once, given the
/// prototype registered for their kind, and rooted for the page's lifetime so that expando
/// properties and listeners stored on them survive garbage collection.
pub(crate) fn wrap(rt: &mut Runtime, cx: &Cx, id: NodeId) -> Value {
    if id == NO_NODE {
        return Value::Null;
    }
    let fresh = {
        let mut st = cx.st.borrow_mut();
        let i = id as usize;
        if !st.page.doc.is_valid(id) {
            return Value::Null;
        }
        if st.wrapped.len() <= i {
            let n = st.page.doc.nodes.len().max(i + 1);
            st.wrapped.resize(n, false);
        }
        if st.wrapped[i] {
            None
        } else {
            st.wrapped[i] = true;
            Some(proto_for(&st, id))
        }
    };
    match fresh {
        None => rt.host_object(cx.class, id as u64),
        Some(proto) => {
            let v = match proto {
                Some(p) => rt.host_object_with_proto(cx.class, id as u64, p),
                None => rt.host_object(cx.class, id as u64),
            };
            rt.root(v);
            v
        }
    }
}

/// Deliver queued custom-element events to the script-side registry. Called by every native
/// that can change the tree or an attribute, after it has released the shared state, so the
/// callbacks run before control returns to the script that made the change.
fn flush_ce(rt: &mut Runtime, cx: &Cx) {
    for _ in 0..64 {
        let (events, hook) = {
            let mut st = cx.st.borrow_mut();
            if st.ce_queue.is_empty() {
                return;
            }
            (std::mem::take(&mut st.ce_queue), st.ce_hook)
        };
        let mut flat: Vec<Value> = Vec::with_capacity(events.len() * 5);
        for ev in events {
            let opt = |rt: &mut Runtime, v: Option<String>| match v {
                Some(s) => rt.string(&s),
                None => Value::Null,
            };
            match ev {
                CeEvent::Connected(n) | CeEvent::Disconnected(n) => {
                    let kind = if matches!(ev, CeEvent::Connected(_)) { 0.0 } else { 1.0 };
                    flat.push(Value::Number(kind));
                    flat.push(wrap(rt, cx, n));
                    flat.extend([Value::Null, Value::Null, Value::Null]);
                }
                CeEvent::Attr(n, name, old, new) => {
                    flat.push(Value::Number(2.0));
                    flat.push(wrap(rt, cx, n));
                    flat.push(rt.string(name.as_str()));
                    let o = opt(rt, old);
                    flat.push(o);
                    let nv = opt(rt, new);
                    flat.push(nv);
                }
            }
        }
        let list = rt.array(&flat);
        let _ = rt.call(hook, Value::Undefined, &[list]);
    }
}

/// ceEnable(hook): the page defined its first custom element; start reporting.
fn n_ce_enable(_rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let hook = arg(args, 0);
    let mut st = cx.st.borrow_mut();
    if !st.ce_active {
        _rt.root(hook);
        st.ce_hook = hook;
        st.ce_active = true;
    }
    Ok(Value::Undefined)
}

fn wrap_list(rt: &mut Runtime, cx: &Cx, ids: &[NodeId]) -> Value {
    let vals: Vec<Value> = ids.iter().map(|&n| wrap(rt, cx, n)).collect();
    rt.array(&vals)
}

fn can_have_children(st: &HostState, n: NodeId) -> bool {
    matches!(st.page.doc.node(n).data, NodeData::Element(_) | NodeData::Document)
}

/// Bookkeeping after `n` was attached under a connected parent.
/// Does the subtree at `n` contain an element that owns a style sheet?
fn has_sheet_owner(st: &HostState, n: NodeId) -> bool {
    std::iter::once(n).chain(st.page.doc.descendants(n)).any(|d| matches!(st.page.doc.tag(d), a::STYLE | a::LINK))
}

fn note_inserted(st: &mut HostState, parent: NodeId, n: NodeId) {
    st.tree_changed();
    st.fx.inserted.push(n);
    st.queue_ce_subtree(n, true);
    if st.page.doc.tag(parent) == a::STYLE || has_sheet_owner(st, n) {
        st.fx.sheets_changed = true;
    }
    if matches!(st.page.doc.tag(parent), a::STYLE | a::TITLE | a::SCRIPT | a::TEXTAREA | a::OPTION) {
        st.fx.text_changed.push(parent);
    }
}

/// Bookkeeping before `n` is detached from a connected parent.
fn note_removing(st: &mut HostState, n: NodeId) {
    let parent = st.page.doc.parent(n);
    if parent == NO_NODE || !st.page.doc.is_connected(parent) {
        return;
    }
    st.tree_changed();
    if st.page.doc.tag(parent) == a::STYLE || has_sheet_owner(st, n) {
        st.fx.sheets_changed = true;
    }
    st.queue_ce_subtree(n, false);
    if matches!(st.page.doc.tag(parent), a::STYLE | a::TITLE | a::SCRIPT | a::TEXTAREA | a::OPTION) {
        st.fx.text_changed.push(parent);
    }
    if st.page.focus != NO_NODE && st.page.doc.contains(n, st.page.focus) {
        st.fx.focus = Some(NO_NODE);
    }
}

fn n_kind(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let st = cx.st.borrow();
    let k = match &st.page.doc.node(n).data {
        NodeData::Element(e) if e.tag == st.fragment_tag => 11,
        NodeData::Element(_) => 1,
        NodeData::Text(_) => 3,
        NodeData::Comment(_) => 8,
        NodeData::Document => 9,
        NodeData::Doctype => 10,
    };
    Ok(Value::Number(k as f64))
}

fn n_tag(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let tag = cx.st.borrow().page.doc.tag(n);
    Ok(rt.string(tag.as_str()))
}

fn link(rt: &mut Runtime, cx: &Cx, args: &[Value], pick: fn(&crate::dom::Node) -> NodeId) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let target = pick(cx.st.borrow().page.doc.node(n));
    Ok(wrap(rt, cx, target))
}

fn n_parent(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    link(rt, cx, args, |n| n.parent)
}
fn n_first(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    link(rt, cx, args, |n| n.first_child)
}
fn n_last(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    link(rt, cx, args, |n| n.last_child)
}
fn n_next(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    link(rt, cx, args, |n| n.next)
}
fn n_prev(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    link(rt, cx, args, |n| n.prev)
}

/// children(node, elementsOnly)
fn n_children(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let only_elements = rt.to_boolean(arg(args, 1));
    let ids: Vec<NodeId> = {
        let st = cx.st.borrow();
        st.page.doc.children(n).filter(|&c| !only_elements || st.page.doc.is_element(c)).collect()
    };
    Ok(wrap_list(rt, cx, &ids))
}

fn n_connected(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    Ok(Value::Bool(cx.st.borrow().page.doc.is_connected(n)))
}

fn n_contains(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let anc = node(rt, cx, arg(args, 0))?;
    let n = opt_node(rt, cx, arg(args, 1))?;
    Ok(Value::Bool(n != NO_NODE && cx.st.borrow().page.doc.contains(anc, n)))
}

/// insert(parent, child, before|null): the pre-insert algorithm, including fragments.
fn n_insert(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let parent = node(rt, cx, arg(args, 0))?;
    let child = node(rt, cx, arg(args, 1))?;
    let mut before = opt_node(rt, cx, arg(args, 2))?;
    let problem = {
        let mut st = cx.st.borrow_mut();
        let st = &mut *st;
        if !can_have_children(st, parent) {
            Some(("HierarchyRequestError", "this node cannot have children"))
        } else if st.page.doc.contains(child, parent) {
            Some(("HierarchyRequestError", "the new child contains the parent"))
        } else if matches!(st.page.doc.node(child).data, NodeData::Document) {
            Some(("HierarchyRequestError", "a document cannot be inserted"))
        } else if before != NO_NODE && st.page.doc.parent(before) != parent {
            Some(("NotFoundError", "the reference node is not a child of this node"))
        } else {
            if before == child {
                before = st.page.doc.node(child).next;
            }
            let connected = st.page.doc.is_connected(parent);
            let moving: Vec<NodeId> = if st.is_fragment(child) { st.page.doc.children(child).collect() } else { vec![child] };
            for m in moving {
                note_removing(st, m);
                st.page.doc.insert_before(parent, m, before);
                if connected {
                    note_inserted(st, parent, m);
                }
            }
            None
        }
    };
    match problem {
        Some((name, msg)) => Err(err(rt, "Error", &format!("{name}: {msg}"))),
        None => {
            flush_ce(rt, cx);
            Ok(arg(args, 1))
        }
    }
}

fn n_remove(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    {
        let mut st = cx.st.borrow_mut();
        note_removing(&mut st, n);
        st.page.doc.detach(n);
    }
    flush_ce(rt, cx);
    Ok(Value::Undefined)
}

fn n_create_element(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let name = rt.to_string_lossy(arg(args, 0));
    let ok = !name.is_empty() && !name.chars().any(|c| c.is_whitespace() || matches!(c, '<' | '>' | '/' | '=' | '"' | '\'' | '\0'));
    if !ok {
        return Err(err(rt, "Error", "InvalidCharacterError: invalid element name"));
    }
    let id = cx.st.borrow_mut().page.doc.create_element(Atom::lower(&name), Vec::new());
    Ok(wrap(rt, cx, id))
}

fn n_create_text(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let data = rt.to_string_lossy(arg(args, 0));
    let id = cx.st.borrow_mut().page.doc.create_text(data);
    Ok(wrap(rt, cx, id))
}

fn n_create_comment(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let data = rt.to_string_lossy(arg(args, 0));
    let id = cx.st.borrow_mut().page.doc.create_comment(data);
    Ok(wrap(rt, cx, id))
}

fn n_create_fragment(rt: &mut Runtime, cx: &Cx, _args: &[Value]) -> NRes {
    let id = {
        let mut st = cx.st.borrow_mut();
        let tag = st.fragment_tag;
        st.page.doc.create_element(tag, Vec::new())
    };
    Ok(wrap(rt, cx, id))
}

/// clone(node, deep)
fn n_clone(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let deep = rt.to_boolean(arg(args, 1));
    let id = {
        let mut st = cx.st.borrow_mut();
        if n == DOCUMENT_NODE {
            NO_NODE
        } else {
            let copy = if deep { st.page.doc.clone_subtree(n) } else { st.page.doc.clone_node(n) };
            // cloned scripts never run a second time
            let scripts: Vec<NodeId> = std::iter::once(copy).chain(st.page.doc.descendants(copy)).filter(|&d| st.page.doc.tag(d) == a::SCRIPT).collect();
            st.inert_scripts.extend(scripts);
            copy
        }
    };
    Ok(wrap(rt, cx, id))
}

fn n_text(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let s = {
        let st = cx.st.borrow();
        match &st.page.doc.node(n).data {
            NodeData::Text(t) | NodeData::Comment(t) => Some(t.clone()),
            NodeData::Element(_) => Some(st.page.doc.text_content(n)),
            _ => None,
        }
    };
    Ok(match s {
        Some(s) => rt.string(&s),
        None => Value::Null,
    })
}

/// setText(node, string): `textContent` setter, also `data` for character-data nodes.
fn n_set_text(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let text = match arg(args, 1) {
        Value::Null | Value::Undefined => String::new(),
        v => rt.to_string_lossy(v),
    };
    {
    let mut st = cx.st.borrow_mut();
    let st = &mut *st;
    let connected = st.page.doc.is_connected(n);
    if st.page.doc.set_char_data(n, &text) {
        if connected {
            st.fx.dirty_layout = true;
            let p = st.page.doc.parent(n);
            if matches!(st.page.doc.tag(p), a::STYLE | a::TITLE | a::SCRIPT | a::TEXTAREA | a::OPTION) {
                st.fx.text_changed.push(p);
            }
            if st.page.doc.tag(p) == a::STYLE {
                st.fx.sheets_changed = true;
            }
        }
    } else if can_have_children(st, n) && n != DOCUMENT_NODE {
        if connected {
            let kids: Vec<NodeId> = st.page.doc.children(n).collect();
            for k in kids {
                note_removing(st, k);
            }
        }
        st.page.doc.remove_children(n);
        if !text.is_empty() {
            let t = st.page.doc.create_text(text);
            st.page.doc.append_child(n, t);
        }
        if connected {
            st.tree_changed();
            st.fx.text_changed.push(n);
            if st.page.doc.tag(n) == a::STYLE {
                st.fx.sheets_changed = true;
            }
        }
    }
    }
    flush_ce(rt, cx);
    Ok(Value::Undefined)
}

fn n_get_attr(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let name = rt.to_string_lossy(arg(args, 1));
    let v = cx.st.borrow().page.doc.attr(n, Atom::lower(&name)).map(|s| s.to_string());
    Ok(match v {
        Some(s) => rt.string(&s),
        None => Value::Null,
    })
}

fn attr_effects(st: &mut HostState, n: NodeId, name: Atom) {
    if !st.page.doc.is_connected(n) {
        return;
    }
    st.fx.dirty_style = true;
    let tag = st.page.doc.tag(n);
    if matches!(tag, a::LINK | a::STYLE) {
        st.fx.sheets_changed = true;
    }
    if matches!(name, a::SRC | a::SRCSET | a::HREF | a::VALUE | a::CHECKED) || tag == a::BASE {
        st.fx.attr_changed.push((n, name));
    }
}

fn n_set_attr(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let name = rt.to_string_lossy(arg(args, 1));
    let value = rt.to_string_lossy(arg(args, 2));
    if name.is_empty() || name.chars().any(|c| c.is_whitespace() || matches!(c, '<' | '>' | '/' | '=' | '"' | '\'' | '\0')) {
        return Err(err(rt, "Error", "InvalidCharacterError: invalid attribute name"));
    }
    let atom = Atom::lower(&name);
    {
        let mut st = cx.st.borrow_mut();
        let st = &mut *st;
        let watch = st.ce_active && st.is_custom_candidate(n);
        let old = if watch { st.page.doc.attr(n, atom).map(|v| v.to_string()) } else { None };
        let changed = match st.page.doc.element_mut(n) {
            Some(el) => {
                if el.attr(atom) == Some(value.as_str()) {
                    false
                } else {
                    el.set_attr(atom, &value);
                    true
                }
            }
            None => false,
        };
        if changed {
            attr_effects(st, n, atom);
        }
        // a custom element hears about every `setAttribute`, even one that sets the same value
        if watch {
            st.ce_queue.push(CeEvent::Attr(n, atom, old, Some(value)));
        }
    }
    flush_ce(rt, cx);
    Ok(Value::Undefined)
}

fn n_remove_attr(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let name = rt.to_string_lossy(arg(args, 1));
    let atom = Atom::lower(&name);
    {
        let mut st = cx.st.borrow_mut();
        let st = &mut *st;
        let watch = st.ce_active && st.is_custom_candidate(n);
        let old = if watch { st.page.doc.attr(n, atom).map(|v| v.to_string()) } else { None };
        let changed = match st.page.doc.element_mut(n) {
            Some(el) if el.has_attr(atom) => {
                el.remove_attr(atom);
                true
            }
            _ => false,
        };
        if changed {
            attr_effects(st, n, atom);
            if watch {
                st.ce_queue.push(CeEvent::Attr(n, atom, old, None));
            }
        }
    }
    flush_ce(rt, cx);
    Ok(Value::Undefined)
}

fn n_has_attr(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let name = rt.to_string_lossy(arg(args, 1));
    let has = cx.st.borrow().page.doc.element(n).map_or(false, |e| e.has_attr(Atom::lower(&name)));
    Ok(Value::Bool(has))
}

fn n_attr_names(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let names: Vec<&'static str> = cx.st.borrow().page.doc.element(n).map(|e| e.attrs.iter().map(|x| x.name.as_str()).collect()).unwrap_or_default();
    let vals: Vec<Value> = names.iter().map(|s| rt.string(s)).collect();
    Ok(rt.array(&vals))
}

/// Parsed selector list for `text`, or `None` if any part of it is invalid.
fn selectors(st: &mut HostState, text: &str) -> Option<Rc<Vec<Selector>>> {
    if let Some(s) = st.sel_cache.get(text) {
        return Some(s.clone());
    }
    let toks = tokenize_value(text);
    let parts = split_commas(&toks).len();
    let list = parse_selector_list(&toks, None);
    if list.is_empty() || list.len() != parts {
        return None;
    }
    if st.sel_cache.len() >= 512 {
        st.sel_cache.clear();
    }
    let rc = Rc::new(list);
    st.sel_cache.insert(text.to_string(), rc.clone());
    Some(rc)
}

/// Elements under `root` (or `root` itself when `include_root`) matching `sels`, in tree order.
fn select(st: &mut HostState, sels: &[Selector], root: NodeId, include_root: bool, first_only: bool, up: bool) -> Vec<NodeId> {
    let sib = st.sibling_indices();
    let ctx = MatchCtx { doc: &st.page.doc, elem_index: &sib.0, elem_count: &sib.1, hover: &[], active: &[], focus: st.page.focus, target: NO_NODE };
    let hit = |n: NodeId| st.page.doc.is_element(n) && sels.iter().any(|s| s.pseudo_element == PseudoElement::None && matches(s, n, &ctx));
    let mut out = Vec::new();
    if up {
        // closest(): the node and its ancestors
        let mut n = root;
        while n != NO_NODE {
            if hit(n) {
                out.push(n);
                break;
            }
            n = st.page.doc.parent(n);
        }
        return out;
    }
    if include_root {
        if hit(root) {
            out.push(root);
        }
        return out;
    }
    for n in st.page.doc.descendants(root) {
        if hit(n) {
            out.push(n);
            if first_only {
                break;
            }
        }
    }
    out
}

/// query(root, selector, mode): mode 0 = first, 1 = all, 2 = matches(root), 3 = closest(root)
fn n_query(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let root = node(rt, cx, arg(args, 0))?;
    let text = rt.to_string_lossy(arg(args, 1));
    let mode = rt.to_number(arg(args, 2)) as i32;
    let found = {
        let mut st = cx.st.borrow_mut();
        let st = &mut *st;
        selectors(st, &text).map(|sels| select(st, &sels, root, mode == 2, mode == 0, mode == 3))
    };
    let Some(found) = found else {
        return Err(err(rt, "SyntaxError", &format!("'{text}' is not a valid selector")));
    };
    Ok(match mode {
        1 => wrap_list(rt, cx, &found),
        2 => Value::Bool(!found.is_empty()),
        _ => wrap(rt, cx, found.first().copied().unwrap_or(NO_NODE)),
    })
}

fn n_by_id(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let id = rt.to_string_lossy(arg(args, 0));
    let n = cx.st.borrow().page.doc.element_by_id(&id);
    Ok(wrap(rt, cx, n))
}

/// byTag(root, name): `*` matches every element.
fn n_by_tag(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let root = node(rt, cx, arg(args, 0))?;
    let name = rt.to_string_lossy(arg(args, 1));
    let ids: Vec<NodeId> = {
        let st = cx.st.borrow();
        let all = name == "*";
        let tag = Atom::lower(&name);
        st.page.doc.descendants(root).filter(|&n| st.page.doc.is_element(n) && (all || st.page.doc.tag(n) == tag)).collect()
    };
    Ok(wrap_list(rt, cx, &ids))
}

/// byClass(root, "a b"): elements carrying every listed class.
fn n_by_class(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let root = node(rt, cx, arg(args, 0))?;
    let names = rt.to_string_lossy(arg(args, 1));
    let classes: Vec<Atom> = names.split_ascii_whitespace().map(Atom::new).collect();
    let ids: Vec<NodeId> = {
        let st = cx.st.borrow();
        if classes.is_empty() {
            Vec::new()
        } else {
            st.page.doc.descendants(root).filter(|&n| st.page.doc.element(n).map_or(false, |e| classes.iter().all(|c| e.has_class(*c)))).collect()
        }
    };
    Ok(wrap_list(rt, cx, &ids))
}

/// html(node, outer)
fn n_html(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let n = node(rt, cx, arg(args, 0))?;
    let outer = rt.to_boolean(arg(args, 1));
    let s = {
        let st = cx.st.borrow();
        serialize(&st.page.doc, n, !outer, st.scripting)
    };
    Ok(rt.string(&s))
}

/// parse(contextNode, markup): a fragment holding the parsed nodes. Scripts in it never run.
fn n_parse(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let ctx_node = node(rt, cx, arg(args, 0))?;
    let markup = rt.to_string_lossy(arg(args, 1));
    let frag = {
        let mut st = cx.st.borrow_mut();
        let st = &mut *st;
        let ctx_tag = match st.page.doc.tag(ctx_node) {
            t if t.is_none() || t == st.fragment_tag => a::DIV,
            t => t,
        };
        let nodes = crate::html::parse_fragment(&mut st.page.doc, ctx_tag, &markup, st.scripting);
        let frag = st.page.doc.create_element(st.fragment_tag, Vec::new());
        for n in nodes {
            st.page.doc.append_child(frag, n);
        }
        let scripts: Vec<NodeId> = st.page.doc.descendants(frag).filter(|&d| st.page.doc.tag(d) == a::SCRIPT).collect();
        st.inert_scripts.extend(scripts);
        frag
    };
    Ok(wrap(rt, cx, frag))
}

/// setProto(key, prototype): the bootstrap registers the prototype for a tag or a `#kind`.
fn n_set_proto(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let key = rt.to_string_lossy(arg(args, 0));
    let proto = arg(args, 1);
    rt.root(proto);
    cx.st.borrow_mut().protos.insert(Atom::new(&key), proto);
    Ok(Value::Undefined)
}

/// log(level, text)
fn n_log(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let level = match rt.to_number(arg(args, 0)) as i32 {
        1 => ConsoleLevel::Info,
        2 => ConsoleLevel::Warn,
        3 => ConsoleLevel::Error,
        _ => ConsoleLevel::Log,
    };
    let text = rt.to_string_lossy(arg(args, 1));
    cx.st.borrow_mut().log(level, text);
    Ok(Value::Undefined)
}

pub(crate) fn install(rt: &mut Runtime, n: Value, cx: &Rc<Cx>) {
    let table: &[(&str, u32, Native)] = &[
        ("kind", 1, n_kind),
        ("tag", 1, n_tag),
        ("parent", 1, n_parent),
        ("first", 1, n_first),
        ("last", 1, n_last),
        ("next", 1, n_next),
        ("prev", 1, n_prev),
        ("children", 2, n_children),
        ("connected", 1, n_connected),
        ("contains", 2, n_contains),
        ("insert", 3, n_insert),
        ("remove", 1, n_remove),
        ("createElement", 1, n_create_element),
        ("createText", 1, n_create_text),
        ("createComment", 1, n_create_comment),
        ("createFragment", 0, n_create_fragment),
        ("clone", 2, n_clone),
        ("text", 1, n_text),
        ("setText", 2, n_set_text),
        ("getAttr", 2, n_get_attr),
        ("setAttr", 3, n_set_attr),
        ("removeAttr", 2, n_remove_attr),
        ("hasAttr", 2, n_has_attr),
        ("attrNames", 1, n_attr_names),
        ("query", 3, n_query),
        ("byId", 1, n_by_id),
        ("byTag", 2, n_by_tag),
        ("byClass", 2, n_by_class),
        ("html", 2, n_html),
        ("parse", 2, n_parse),
        ("setProto", 2, n_set_proto),
        ("ceEnable", 1, n_ce_enable),
        ("log", 2, n_log),
    ];
    for &(name, arity, f) in table {
        def(rt, n, cx, name, arity, f);
    }
}
