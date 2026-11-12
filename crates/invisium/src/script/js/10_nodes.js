
// ───────────────────────────── nodes ─────────────────────────────

function nodeList(arr) {
  hide(arr, 'item', function (i) { return this[i] === undefined ? null : this[i]; });
  return arr;
}
// A collection that can also be read by element id or name (`form.elements.q`).
function namedList(arr) {
  nodeList(arr);
  for (var i = 0; i < arr.length; i++) {
    var id = n.getAttr(arr[i], 'id'), name = n.getAttr(arr[i], 'name');
    if (id && !(id in arr)) hide(arr, id, arr[i]);
    if (name && !(name in arr)) hide(arr, name, arr[i]);
  }
  hide(arr, 'namedItem', function (key) {
    key = String(key);
    for (var i = 0; i < this.length; i++) if (n.getAttr(this[i], 'id') === key || n.getAttr(this[i], 'name') === key) return this[i];
    return null;
  });
  return arr;
}
function toNode(v) {
  return v instanceof Node ? v : n.createText(String(v));
}
// Several nodes/strings -> one node to insert (ParentNode/ChildNode methods).
function gather(items) {
  if (items.length === 1) return toNode(items[0]);
  var f = n.createFragment();
  for (var i = 0; i < items.length; i++) n.insert(f, toNode(items[i]), null);
  return f;
}
function isCharData(k) { return k === 3 || k === 8; }

class Node extends EventTarget {
  get nodeType() { return n.kind(this); }
  get nodeName() {
    switch (n.kind(this)) {
      case 1: return n.tag(this).toUpperCase();
      case 3: return '#text';
      case 8: return '#comment';
      case 9: return '#document';
      case 10: return 'html';
      default: return '#document-fragment';
    }
  }
  get parentNode() { return n.parent(this); }
  get parentElement() { var p = n.parent(this); return p && n.kind(p) === 1 ? p : null; }
  get childNodes() { return nodeList(n.children(this, false)); }
  get firstChild() { return n.first(this); }
  get lastChild() { return n.last(this); }
  get nextSibling() { return n.next(this); }
  get previousSibling() { return n.prev(this); }
  get ownerDocument() { return this === document ? null : document; }
  get isConnected() { return n.connected(this); }
  get baseURI() { return n.resolve('') || n.url(); }
  get textContent() { return n.text(this); }
  set textContent(v) { n.setText(this, v === null || v === undefined ? '' : String(v)); }
  get nodeValue() { return isCharData(n.kind(this)) ? n.text(this) : null; }
  set nodeValue(v) { if (isCharData(n.kind(this))) n.setText(this, v === null ? '' : String(v)); }
  appendChild(c) { n.insert(this, c, null); return c; }
  insertBefore(c, ref) { n.insert(this, c, ref === undefined ? null : ref); return c; }
  removeChild(c) {
    if (!(c instanceof Node) || n.parent(c) !== this) throw new DOMException('The node to be removed is not a child of this node', 'NotFoundError');
    n.remove(c);
    return c;
  }
  replaceChild(fresh, old) {
    if (!(old instanceof Node) || n.parent(old) !== this) throw new DOMException('The node to be replaced is not a child of this node', 'NotFoundError');
    var next = n.next(old);
    if (next === fresh) next = n.next(fresh);
    n.remove(old);
    n.insert(this, fresh, next);
    return old;
  }
  cloneNode(deep) {
    var c = n.clone(this, !!deep);
    if (CE.count) ceUpgradeTree(c);
    return c;
  }
  contains(o) { return o instanceof Node && n.contains(this, o); }
  hasChildNodes() { return n.first(this) !== null; }
  getRootNode() { var t = this, p; while ((p = n.parent(t))) t = p; return t; }
  isSameNode(o) { return o === this; }
  isEqualNode(o) {
    if (o === this) return true;
    if (!(o instanceof Node) || n.kind(o) !== n.kind(this)) return false;
    return n.kind(this) === 1 ? n.html(this, true) === n.html(o, true) : n.text(this) === n.text(o);
  }
  compareDocumentPosition(o) {
    if (o === this) return 0;
    if (!(o instanceof Node)) return 1;
    var a = [], b = [], t;
    for (t = this; t; t = n.parent(t)) a.unshift(t);
    for (t = o; t; t = n.parent(t)) b.unshift(t);
    if (a[0] !== b[0]) return 1 | 32 | 2;
    var i = 0;
    while (i < a.length && i < b.length && a[i] === b[i]) i++;
    if (i === a.length) return 16 | 4;   // o is inside this
    if (i === b.length) return 8 | 2;    // o contains this
    for (t = n.next(a[i]); t; t = n.next(t)) if (t === b[i]) return 4;
    return 2;
  }
  normalize() {
    var c = n.first(this);
    while (c) {
      var next = n.next(c);
      if (n.kind(c) === 3) {
        if (n.text(c) === '') n.remove(c);
        else if (next && n.kind(next) === 3) { n.setText(c, n.text(c) + n.text(next)); n.remove(next); continue; }
      } else if (n.kind(c) === 1) c.normalize();
      c = next;
    }
  }
  lookupNamespaceURI() { return null; }
  isDefaultNamespace() { return true; }
}
(function () {
  var consts = {
    ELEMENT_NODE: 1, ATTRIBUTE_NODE: 2, TEXT_NODE: 3, CDATA_SECTION_NODE: 4, PROCESSING_INSTRUCTION_NODE: 7,
    COMMENT_NODE: 8, DOCUMENT_NODE: 9, DOCUMENT_TYPE_NODE: 10, DOCUMENT_FRAGMENT_NODE: 11,
    DOCUMENT_POSITION_DISCONNECTED: 1, DOCUMENT_POSITION_PRECEDING: 2, DOCUMENT_POSITION_FOLLOWING: 4,
    DOCUMENT_POSITION_CONTAINS: 8, DOCUMENT_POSITION_CONTAINED_BY: 16, DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC: 32
  };
  for (var k in consts) { Node[k] = consts[k]; Node.prototype[k] = consts[k]; }
})();

// ChildNode / ParentNode mixins
var childNode = {
  remove: function () { n.remove(this); },
  before: function () { var p = n.parent(this); if (p) n.insert(p, gather(arguments), this); },
  after: function () {
    var p = n.parent(this);
    if (!p) return;
    var next = n.next(this);
    var skip = Array.prototype.slice.call(arguments);
    while (next && skip.indexOf(next) >= 0) next = n.next(next);
    n.insert(p, gather(arguments), next);
  },
  replaceWith: function () {
    var p = n.parent(this);
    if (!p) return;
    var next = n.next(this);
    var skip = Array.prototype.slice.call(arguments);
    while (next && skip.indexOf(next) >= 0) next = n.next(next);
    n.remove(this);
    if (arguments.length) n.insert(p, gather(arguments), next);
  }
};
var siblingElements = {
  nextElementSibling: function () { var t = n.next(this); while (t && n.kind(t) !== 1) t = n.next(t); return t; },
  previousElementSibling: function () { var t = n.prev(this); while (t && n.kind(t) !== 1) t = n.prev(t); return t; }
};
var parentNode = {
  append: function () { if (arguments.length) n.insert(this, gather(arguments), null); },
  prepend: function () { if (arguments.length) n.insert(this, gather(arguments), n.first(this)); },
  replaceChildren: function () {
    var c;
    while ((c = n.first(this))) n.remove(c);
    if (arguments.length) n.insert(this, gather(arguments), null);
  },
  querySelector: function (s) { return n.query(this, String(s), 0); },
  querySelectorAll: function (s) { return nodeList(n.query(this, String(s), 1)); }
};
var parentGetters = {
  children: function () { return nodeList(n.children(this, true)); },
  firstElementChild: function () { var t = n.first(this); while (t && n.kind(t) !== 1) t = n.next(t); return t; },
  lastElementChild: function () { var t = n.last(this); while (t && n.kind(t) !== 1) t = n.prev(t); return t; },
  childElementCount: function () { return n.children(this, true).length; }
};
function mixGetters(proto, table) {
  for (var k in table) if (hasOwn(table, k)) getter(proto, k, table[k]);
}

class CharacterData extends Node {
  get data() { return n.text(this); }
  set data(v) { n.setText(this, v === null ? '' : String(v)); }
  get length() { return n.text(this).length; }
  appendData(s) { n.setText(this, n.text(this) + String(s)); }
  insertData(at, s) { var d = n.text(this); n.setText(this, d.slice(0, at) + String(s) + d.slice(at)); }
  deleteData(at, count) { var d = n.text(this); n.setText(this, d.slice(0, at) + d.slice(at + count)); }
  replaceData(at, count, s) { var d = n.text(this); n.setText(this, d.slice(0, at) + String(s) + d.slice(at + count)); }
  substringData(at, count) { return n.text(this).substr(at, count); }
}
methods(CharacterData.prototype, childNode);
mixGetters(CharacterData.prototype, siblingElements);

class Text extends CharacterData {
  get wholeText() { return n.text(this); }
  splitText(at) {
    var d = n.text(this);
    var rest = n.createText(d.slice(at));
    n.setText(this, d.slice(0, at));
    var p = n.parent(this);
    if (p) n.insert(p, rest, n.next(this));
    return rest;
  }
}
class Comment extends CharacterData {}
// never created by this engine's parser, but libraries patch their prototypes by name
class CDATASection extends Text {}
class ProcessingInstruction extends CharacterData {}
class DocumentType extends Node {
  get name() { return 'html'; }
  get publicId() { return ''; }
  get systemId() { return ''; }
}
methods(DocumentType.prototype, childNode);

class DocumentFragment extends Node {
  getElementById(id) {
    id = String(id);
    var all = n.byTag(this, '*');
    for (var i = 0; i < all.length; i++) if (n.getAttr(all[i], 'id') === id) return all[i];
    return null;
  }
}
methods(DocumentFragment.prototype, parentNode);
mixGetters(DocumentFragment.prototype, parentGetters);

// A shadow root is a fragment that hangs off its host element. The layout engine does not
// draw shadow trees yet: the root keeps its nodes for script (queries, events), nothing more.
class ShadowRoot extends DocumentFragment {
  get innerHTML() {
    var s = '';
    for (var c = this.firstChild; c; c = c.nextSibling) s += c.nodeType === 1 ? c.outerHTML : c.nodeType === 3 ? c.data : '';
    return s;
  }
  set innerHTML(v) {
    while (this.firstChild) this.removeChild(this.firstChild);
    var tmp = n.createElement('div');
    tmp.innerHTML = v;
    while (tmp.firstChild) this.appendChild(tmp.firstChild);
  }
  get activeElement() { return null; }
  get adoptedStyleSheets() { return []; }
  set adoptedStyleSheets(v) {}
  getRootNode() { return this; }
}

// ───────────────────────────── DOMTokenList ─────────────────────────────

function splitTokens(s) {
  var out = [], cur = '';
  for (var i = 0; i < s.length; i++) {
    var c = s.charCodeAt(i);
    if (c === 32 || c === 9 || c === 10 || c === 12 || c === 13) { if (cur) { out.push(cur); cur = ''; } }
    else cur += s[i];
  }
  if (cur) out.push(cur);
  return out;
}

class DOMTokenList {
  constructor(el, attr) { hide(this, '_el', el); hide(this, '_attr', attr); }
  _get() {
    var list = splitTokens(n.getAttr(this._el, this._attr) || ''), out = [];
    for (var i = 0; i < list.length; i++) if (out.indexOf(list[i]) < 0) out.push(list[i]);
    return out;
  }
  _set(list) { n.setAttr(this._el, this._attr, list.join(' ')); }
  get length() { return this._get().length; }
  get value() { return n.getAttr(this._el, this._attr) || ''; }
  set value(v) { n.setAttr(this._el, this._attr, String(v)); }
  item(i) { var l = this._get(); return i >= 0 && i < l.length ? l[i] : null; }
  contains(t) { return this._get().indexOf(String(t)) >= 0; }
  add() {
    var l = this._get(), changed = false;
    for (var i = 0; i < arguments.length; i++) {
      var t = String(arguments[i]);
      if (t && l.indexOf(t) < 0) { l.push(t); changed = true; }
    }
    if (changed) this._set(l);
  }
  remove() {
    var l = this._get(), changed = false;
    for (var i = 0; i < arguments.length; i++) {
      var at = l.indexOf(String(arguments[i]));
      if (at >= 0) { l.splice(at, 1); changed = true; }
    }
    if (changed) this._set(l);
  }
  toggle(t, force) {
    t = String(t);
    var l = this._get(), at = l.indexOf(t);
    if (at >= 0) {
      if (force === true) return true;
      l.splice(at, 1); this._set(l); return false;
    }
    if (force === false) return false;
    l.push(t); this._set(l); return true;
  }
  replace(a, b) {
    var l = this._get(), at = l.indexOf(String(a));
    if (at < 0) return false;
    b = String(b);
    if (l.indexOf(b) >= 0) l.splice(at, 1); else l[at] = b;
    this._set(l);
    return true;
  }
  supports() { return true; }
  forEach(cb, self) { var l = this._get(); for (var i = 0; i < l.length; i++) cb.call(self, l[i], i, this); }
  keys() { var l = this._get(), k = []; for (var i = 0; i < l.length; i++) k.push(i); return k[Symbol.iterator](); }
  values() { return this._get()[Symbol.iterator](); }
  entries() { var l = this._get(), e = []; for (var i = 0; i < l.length; i++) e.push([i, l[i]]); return e[Symbol.iterator](); }
  [Symbol.iterator]() { return this._get()[Symbol.iterator](); }
  toString() { return this.value; }
}
