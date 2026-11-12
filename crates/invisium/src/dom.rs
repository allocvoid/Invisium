//! Arena DOM. Every node lives in one `Vec`; links are 4-byte indices instead of pointers,
//! so a node costs ~40 bytes plus its payload and the whole tree frees in one deallocation.

use crate::atom::{a, Atom};

pub type NodeId = u32;
pub const NO_NODE: NodeId = u32::MAX;

pub struct Attr {
    pub name: Atom,
    pub value: Box<str>,
}

pub struct Element {
    pub tag: Atom,
    pub id: Atom,
    pub classes: Vec<Atom>,
    pub attrs: Vec<Attr>,
}

impl Element {
    pub fn attr(&self, name: Atom) -> Option<&str> {
        self.attrs.iter().find(|x| x.name == name).map(|x| &*x.value)
    }
    pub fn has_attr(&self, name: Atom) -> bool {
        self.attrs.iter().any(|x| x.name == name)
    }
    pub fn has_class(&self, c: Atom) -> bool {
        self.classes.contains(&c)
    }
    pub fn set_attr(&mut self, name: Atom, value: &str) {
        if name == a::ID {
            self.id = if value.is_empty() { Atom::NONE } else { Atom::new(value) };
        } else if name == a::CLASS {
            self.classes = value.split_ascii_whitespace().map(Atom::new).collect();
        }
        if let Some(x) = self.attrs.iter_mut().find(|x| x.name == name) {
            x.value = value.into();
        } else {
            self.attrs.push(Attr { name, value: value.into() });
        }
    }
    pub fn remove_attr(&mut self, name: Atom) {
        if name == a::ID {
            self.id = Atom::NONE;
        } else if name == a::CLASS {
            self.classes.clear();
        }
        self.attrs.retain(|x| x.name != name);
    }
}

pub enum NodeData {
    Document,
    Doctype,
    Element(Box<Element>),
    Text(String),
    /// Only created by scripts (the parser drops comments); never styled or laid out.
    Comment(String),
}

pub struct Node {
    pub parent: NodeId,
    pub first_child: NodeId,
    pub last_child: NodeId,
    pub prev: NodeId,
    pub next: NodeId,
    pub data: NodeData,
}

pub struct Document {
    pub nodes: Vec<Node>,
    /// True when the page has no doctype (affects a few layout quirks).
    pub quirks: bool,
}

pub const DOCUMENT_NODE: NodeId = 0;

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub fn new() -> Document {
        let mut d = Document { nodes: Vec::with_capacity(256), quirks: true };
        d.push(NodeData::Document);
        d
    }

    fn push(&mut self, data: NodeData) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.nodes.push(Node {
            parent: NO_NODE,
            first_child: NO_NODE,
            last_child: NO_NODE,
            prev: NO_NODE,
            next: NO_NODE,
            data,
        });
        id
    }

    pub fn create_element(&mut self, tag: Atom, attrs: Vec<(Atom, String)>) -> NodeId {
        let mut el = Element { tag, id: Atom::NONE, classes: Vec::new(), attrs: Vec::with_capacity(attrs.len()) };
        for (name, value) in attrs {
            if el.has_attr(name) {
                continue; // first occurrence wins, per the HTML parser spec
            }
            if name == a::ID && !value.is_empty() {
                el.id = Atom::new(&value);
            } else if name == a::CLASS {
                el.classes = value.split_ascii_whitespace().map(Atom::new).collect();
            }
            el.attrs.push(Attr { name, value: value.into_boxed_str() });
        }
        self.push(NodeData::Element(Box::new(el)))
    }

    pub fn create_text(&mut self, text: String) -> NodeId {
        self.push(NodeData::Text(text))
    }

    pub fn create_doctype(&mut self) -> NodeId {
        self.push(NodeData::Doctype)
    }

    pub fn create_comment(&mut self, text: String) -> NodeId {
        self.push(NodeData::Comment(text))
    }

    /// Character data of a text or comment node.
    pub fn char_data(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id as usize].data {
            NodeData::Text(t) | NodeData::Comment(t) => Some(t),
            _ => None,
        }
    }

    /// Replace the character data of a text or comment node. Returns false for other nodes.
    pub fn set_char_data(&mut self, id: NodeId, text: &str) -> bool {
        match &mut self.nodes[id as usize].data {
            NodeData::Text(t) | NodeData::Comment(t) => {
                t.clear();
                t.push_str(text);
                true
            }
            _ => false,
        }
    }

    #[inline]
    pub fn is_valid(&self, id: NodeId) -> bool {
        (id as usize) < self.nodes.len()
    }

    /// True if `node` is `ancestor` or lies below it.
    pub fn contains(&self, ancestor: NodeId, node: NodeId) -> bool {
        let mut n = node;
        while n != NO_NODE {
            if n == ancestor {
                return true;
            }
            n = self.nodes[n as usize].parent;
        }
        false
    }

    /// True if the node is attached to the document tree.
    pub fn is_connected(&self, node: NodeId) -> bool {
        self.contains(DOCUMENT_NODE, node)
    }

    /// Detach every child of `parent`.
    pub fn remove_children(&mut self, parent: NodeId) {
        let mut c = self.nodes[parent as usize].first_child;
        while c != NO_NODE {
            let next = self.nodes[c as usize].next;
            let n = &mut self.nodes[c as usize];
            n.parent = NO_NODE;
            n.prev = NO_NODE;
            n.next = NO_NODE;
            c = next;
        }
        let p = &mut self.nodes[parent as usize];
        p.first_child = NO_NODE;
        p.last_child = NO_NODE;
    }

    /// Shallow copy of one node (no children), detached.
    pub fn clone_node(&mut self, id: NodeId) -> NodeId {
        let data = match &self.nodes[id as usize].data {
            NodeData::Document => NodeData::Document,
            NodeData::Doctype => NodeData::Doctype,
            NodeData::Text(t) => NodeData::Text(t.clone()),
            NodeData::Comment(t) => NodeData::Comment(t.clone()),
            NodeData::Element(e) => NodeData::Element(Box::new(Element {
                tag: e.tag,
                id: e.id,
                classes: e.classes.clone(),
                attrs: e.attrs.iter().map(|x| Attr { name: x.name, value: x.value.clone() }).collect(),
            })),
        };
        self.push(data)
    }

    /// Deep copy of a subtree within this document, detached. Iterative: depth is page-controlled.
    pub fn clone_subtree(&mut self, root: NodeId) -> NodeId {
        let new_root = self.clone_node(root);
        let mut stack: Vec<(NodeId, NodeId)> = vec![(root, new_root)];
        while let Some((src, dst)) = stack.pop() {
            let mut c = self.nodes[src as usize].first_child;
            while c != NO_NODE {
                let copy = self.clone_node(c);
                self.append_child(dst, copy);
                if self.nodes[c as usize].first_child != NO_NODE {
                    stack.push((c, copy));
                }
                c = self.nodes[c as usize].next;
            }
        }
        new_root
    }

    /// Deep copy of a subtree of another document into this one, detached.
    pub fn import_subtree(&mut self, from: &Document, root: NodeId) -> NodeId {
        fn copy_one(to: &mut Document, from: &Document, id: NodeId) -> NodeId {
            match &from.nodes[id as usize].data {
                NodeData::Text(t) => to.create_text(t.clone()),
                NodeData::Comment(t) => to.create_comment(t.clone()),
                NodeData::Element(e) => to.create_element(e.tag, e.attrs.iter().map(|x| (x.name, x.value.to_string())).collect()),
                NodeData::Doctype => to.create_doctype(),
                NodeData::Document => to.create_text(String::new()),
            }
        }
        let new_root = copy_one(self, from, root);
        let mut stack: Vec<(NodeId, NodeId)> = vec![(root, new_root)];
        while let Some((src, dst)) = stack.pop() {
            let mut c = from.nodes[src as usize].first_child;
            while c != NO_NODE {
                let copy = copy_one(self, from, c);
                self.append_child(dst, copy);
                if from.nodes[c as usize].first_child != NO_NODE {
                    stack.push((c, copy));
                }
                c = from.nodes[c as usize].next;
            }
        }
        new_root
    }

    #[inline]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    #[inline]
    pub fn element(&self, id: NodeId) -> Option<&Element> {
        match &self.nodes[id as usize].data {
            NodeData::Element(e) => Some(e),
            _ => None,
        }
    }

    #[inline]
    pub fn element_mut(&mut self, id: NodeId) -> Option<&mut Element> {
        match &mut self.nodes[id as usize].data {
            NodeData::Element(e) => Some(e),
            _ => None,
        }
    }

    #[inline]
    pub fn tag(&self, id: NodeId) -> Atom {
        match &self.nodes[id as usize].data {
            NodeData::Element(e) => e.tag,
            _ => Atom::NONE,
        }
    }

    #[inline]
    pub fn is_element(&self, id: NodeId) -> bool {
        matches!(self.nodes[id as usize].data, NodeData::Element(_))
    }

    pub fn text(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id as usize].data {
            NodeData::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn attr(&self, id: NodeId, name: Atom) -> Option<&str> {
        self.element(id).and_then(|e| e.attr(name))
    }

    #[inline]
    pub fn parent(&self, id: NodeId) -> NodeId {
        self.nodes[id as usize].parent
    }

    pub fn detach(&mut self, id: NodeId) {
        let (parent, prev, next) = {
            let n = &self.nodes[id as usize];
            (n.parent, n.prev, n.next)
        };
        if parent == NO_NODE {
            return;
        }
        if prev != NO_NODE {
            self.nodes[prev as usize].next = next;
        } else {
            self.nodes[parent as usize].first_child = next;
        }
        if next != NO_NODE {
            self.nodes[next as usize].prev = prev;
        } else {
            self.nodes[parent as usize].last_child = prev;
        }
        let n = &mut self.nodes[id as usize];
        n.parent = NO_NODE;
        n.prev = NO_NODE;
        n.next = NO_NODE;
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.detach(child);
        let last = self.nodes[parent as usize].last_child;
        {
            let c = &mut self.nodes[child as usize];
            c.parent = parent;
            c.prev = last;
            c.next = NO_NODE;
        }
        if last != NO_NODE {
            self.nodes[last as usize].next = child;
        } else {
            self.nodes[parent as usize].first_child = child;
        }
        self.nodes[parent as usize].last_child = child;
    }

    /// Insert `child` into `parent` immediately before `before` (which must be a child of `parent`).
    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, before: NodeId) {
        if before == NO_NODE {
            return self.append_child(parent, child);
        }
        self.detach(child);
        let prev = self.nodes[before as usize].prev;
        {
            let c = &mut self.nodes[child as usize];
            c.parent = parent;
            c.prev = prev;
            c.next = before;
        }
        self.nodes[before as usize].prev = child;
        if prev != NO_NODE {
            self.nodes[prev as usize].next = child;
        } else {
            self.nodes[parent as usize].first_child = child;
        }
    }

    /// Append text to `parent`, merging with a trailing text node when possible.
    pub fn append_text(&mut self, parent: NodeId, text: &str) {
        let last = self.nodes[parent as usize].last_child;
        if last != NO_NODE {
            if let NodeData::Text(t) = &mut self.nodes[last as usize].data {
                t.push_str(text);
                return;
            }
        }
        let n = self.create_text(text.to_owned());
        self.append_child(parent, n);
    }

    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children { doc: self, next: self.nodes[id as usize].first_child }
    }

    /// Pre-order traversal of the subtree below `root` (excluding `root`).
    pub fn descendants(&self, root: NodeId) -> Descendants<'_> {
        Descendants { doc: self, root, next: self.nodes[root as usize].first_child }
    }

    pub fn ancestors(&self, id: NodeId) -> Ancestors<'_> {
        Ancestors { doc: self, next: self.nodes[id as usize].parent }
    }

    pub fn prev_element(&self, id: NodeId) -> NodeId {
        let mut p = self.nodes[id as usize].prev;
        while p != NO_NODE {
            if self.is_element(p) {
                return p;
            }
            p = self.nodes[p as usize].prev;
        }
        NO_NODE
    }

    pub fn next_element(&self, id: NodeId) -> NodeId {
        let mut p = self.nodes[id as usize].next;
        while p != NO_NODE {
            if self.is_element(p) {
                return p;
            }
            p = self.nodes[p as usize].next;
        }
        NO_NODE
    }

    pub fn document_element(&self) -> NodeId {
        self.children(DOCUMENT_NODE).find(|&c| self.tag(c) == a::HTML).unwrap_or(NO_NODE)
    }

    pub fn find_child(&self, parent: NodeId, tag: Atom) -> NodeId {
        if parent == NO_NODE {
            return NO_NODE;
        }
        self.children(parent).find(|&c| self.tag(c) == tag).unwrap_or(NO_NODE)
    }

    pub fn head(&self) -> NodeId {
        self.find_child(self.document_element(), a::HEAD)
    }

    pub fn body(&self) -> NodeId {
        self.find_child(self.document_element(), a::BODY)
    }

    pub fn first_by_tag(&self, root: NodeId, tag: Atom) -> NodeId {
        self.descendants(root).find(|&n| self.tag(n) == tag).unwrap_or(NO_NODE)
    }

    pub fn element_by_id(&self, id: &str) -> NodeId {
        if id.is_empty() {
            return NO_NODE;
        }
        let atom = Atom::new(id);
        self.descendants(DOCUMENT_NODE)
            .find(|&n| self.element(n).map_or(false, |e| e.id == atom))
            .unwrap_or(NO_NODE)
    }

    /// Concatenated text of all descendant text nodes.
    pub fn text_content(&self, id: NodeId) -> String {
        let mut out = String::new();
        if let Some(t) = self.text(id) {
            out.push_str(t);
        }
        for n in self.descendants(id) {
            if let Some(t) = self.text(n) {
                out.push_str(t);
            }
        }
        out
    }

    /// Nearest ancestor-or-self element with the given tag.
    pub fn closest(&self, id: NodeId, tag: Atom) -> NodeId {
        let mut n = id;
        while n != NO_NODE {
            if self.tag(n) == tag {
                return n;
            }
            n = self.nodes[n as usize].parent;
        }
        NO_NODE
    }

    /// Approximate heap bytes used by this document.
    pub fn memory_bytes(&self) -> usize {
        let mut total = self.nodes.capacity() * std::mem::size_of::<Node>();
        for n in &self.nodes {
            match &n.data {
                NodeData::Text(t) | NodeData::Comment(t) => total += t.capacity(),
                NodeData::Element(e) => {
                    total += std::mem::size_of::<Element>()
                        + e.classes.capacity() * 4
                        + e.attrs.capacity() * std::mem::size_of::<Attr>()
                        + e.attrs.iter().map(|x| x.value.len()).sum::<usize>();
                }
                _ => {}
            }
        }
        total
    }

    pub fn shrink(&mut self) {
        self.nodes.shrink_to_fit();
        for n in &mut self.nodes {
            match &mut n.data {
                NodeData::Text(t) => t.shrink_to_fit(),
                NodeData::Element(e) => {
                    e.attrs.shrink_to_fit();
                    e.classes.shrink_to_fit();
                }
                _ => {}
            }
        }
    }
}

pub struct Children<'a> {
    doc: &'a Document,
    next: NodeId,
}

impl Iterator for Children<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        if self.next == NO_NODE {
            return None;
        }
        let cur = self.next;
        self.next = self.doc.nodes[cur as usize].next;
        Some(cur)
    }
}

pub struct Descendants<'a> {
    doc: &'a Document,
    root: NodeId,
    next: NodeId,
}

impl Iterator for Descendants<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        if self.next == NO_NODE {
            return None;
        }
        let cur = self.next;
        let n = &self.doc.nodes[cur as usize];
        if n.first_child != NO_NODE {
            self.next = n.first_child;
        } else {
            let mut up = cur;
            self.next = NO_NODE;
            while up != self.root && up != NO_NODE {
                let un = &self.doc.nodes[up as usize];
                if un.next != NO_NODE {
                    self.next = un.next;
                    break;
                }
                up = un.parent;
            }
        }
        Some(cur)
    }
}

pub struct Ancestors<'a> {
    doc: &'a Document,
    next: NodeId,
}

impl Iterator for Ancestors<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        if self.next == NO_NODE {
            return None;
        }
        let cur = self.next;
        self.next = self.doc.nodes[cur as usize].parent;
        Some(cur)
    }
}
