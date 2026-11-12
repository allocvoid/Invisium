
// ───────────────────────────── document ─────────────────────────────

var readyState = 'loading';

function createEventByName(name) {
  var table = {
    event: Event, events: Event, htmlevents: Event, customevent: CustomEvent, uievent: UIEvent, uievents: UIEvent,
    mouseevent: MouseEvent, mouseevents: MouseEvent, keyboardevent: KeyboardEvent, focusevent: FocusEvent,
    messageevent: MessageEvent, hashchangeevent: HashChangeEvent, popstateevent: PopStateEvent, errorevent: ErrorEvent
  };
  var C = table[String(name).toLowerCase()];
  if (!C) throw new DOMException('Unknown event interface', 'NotSupportedError');
  return new C('');
}

// Tree walking shared by TreeWalker and NodeIterator.
function nextInTree(root, node) {
  var c = n.first(node);
  if (c) return c;
  while (node && node !== root) {
    var s = n.next(node);
    if (s) return s;
    node = n.parent(node);
  }
  return null;
}
function shows(what, node) { return (what >>> 0) === 0xFFFFFFFF || ((what >> (n.kind(node) - 1)) & 1) === 1; }
function accepts(filter, node) {
  if (!filter) return 1;
  var r = typeof filter === 'function' ? filter(node) : filter.acceptNode(node);
  return r;
}
class TreeWalker {
  constructor(root, what, filter) { this.root = root; this.whatToShow = what === undefined ? 0xFFFFFFFF : what; this.filter = filter || null; this.currentNode = root; }
  nextNode() {
    var t = this.currentNode;
    while ((t = nextInTree(this.root, t))) {
      if (shows(this.whatToShow, t) && accepts(this.filter, t) === 1) { this.currentNode = t; return t; }
    }
    return null;
  }
  firstChild() { return this.nextNode(); }
  parentNode() {
    var t = this.currentNode;
    while (t && t !== this.root) {
      t = n.parent(t);
      if (t && shows(this.whatToShow, t) && accepts(this.filter, t) === 1) { this.currentNode = t; return t; }
    }
    return null;
  }
  nextSibling() {
    for (var t = n.next(this.currentNode); t; t = n.next(t)) {
      if (shows(this.whatToShow, t) && accepts(this.filter, t) === 1) { this.currentNode = t; return t; }
    }
    return null;
  }
}
var NodeFilter = {
  FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3, SHOW_ALL: 0xFFFFFFFF, SHOW_ELEMENT: 1, SHOW_TEXT: 4,
  SHOW_COMMENT: 128, SHOW_DOCUMENT: 256, SHOW_DOCUMENT_FRAGMENT: 1024
};

// A minimal Range: enough for libraries that build fragments or measure nodes with it.
class Range {
  constructor() { this.startContainer = document; this.endContainer = document; this.startOffset = 0; this.endOffset = 0; this.collapsed = true; hide(this, '_node', null); }
  get commonAncestorContainer() { return this._node || document; }
  setStart(node, off) { this.startContainer = node; this.startOffset = off; }
  setEnd(node, off) { this.endContainer = node; this.endOffset = off; }
  setStartBefore(node) { this._node = node; }
  setStartAfter(node) { this._node = node; }
  setEndBefore(node) { this._node = node; }
  setEndAfter(node) { this._node = node; }
  selectNode(node) { this._node = node; this.collapsed = false; }
  selectNodeContents(node) { this._node = node; this.collapsed = false; }
  collapse() { this.collapsed = true; }
  cloneRange() { var r = new Range(); r._node = this._node; return r; }
  detach() {}
  createContextualFragment(html) { return n.parse(this._node && n.kind(this._node) === 1 ? this._node : document.body || document.documentElement, String(html)); }
  getBoundingClientRect() { return this._node && n.kind(this._node) === 1 ? this._node.getBoundingClientRect() : new DOMRect(0, 0, 0, 0); }
  getClientRects() { return []; }
  deleteContents() { var node = this._node, c; if (node) while ((c = n.first(node))) n.remove(c); }
  toString() { return this._node ? n.text(this._node) || '' : ''; }
}
var SELECTION = {
  rangeCount: 0, isCollapsed: true, anchorNode: null, focusNode: null, anchorOffset: 0, focusOffset: 0, type: 'None',
  getRangeAt: function () { return new Range(); }, removeAllRanges: function () {}, addRange: function () {},
  collapse: function () {}, toString: function () { return ''; }, empty: function () {}, selectAllChildren: function () {}
};

class Document extends Node {
  getAnimations() { return animationsOf(null, true); }
  get timeline() { return documentTimeline; }
  get documentElement() { return n.special(0); }
  get head() { return n.special(1); }
  get body() { return n.special(2); }
  set body(v) { var old = n.special(2), root = n.special(0); if (old) n.remove(old); if (root) n.insert(root, v, null); }
  get title() { return n.title(); }
  set title(v) { n.setTitle(String(v)); }
  get URL() { return n.url(); }
  get documentURI() { return n.url(); }
  get location() { return location; }
  set location(v) { n.navigate(String(v), false); }
  get domain() { return location.hostname; }
  set domain(v) {}
  get referrer() { return ''; }
  get cookie() { return n.cookie(); }
  set cookie(v) { n.setCookie(String(v)); }
  get readyState() { return readyState; }
  get characterSet() { return 'UTF-8'; }
  get charset() { return 'UTF-8'; }
  get inputEncoding() { return 'UTF-8'; }
  get contentType() { return 'text/html'; }
  get compatMode() { return 'CSS1Compat'; }
  get doctype() { var c = n.first(this); return c && n.kind(c) === 10 ? c : null; }
  get defaultView() { return g; }
  get hidden() { return false; }
  get visibilityState() { return 'visible'; }
  get activeElement() { return n.active() || n.special(2); }
  get currentScript() { return n.currentScript(); }
  get scripts() { return nodeList(n.byTag(this, 'script')); }
  get images() { return nodeList(n.byTag(this, 'img')); }
  get forms() { return namedList(n.byTag(this, 'form')); }
  get links() { return nodeList(n.query(this, 'a[href], area[href]', 1)); }
  get styleSheets() { return []; }
  get scrollingElement() { return n.special(0); }
  get fullscreenElement() { return null; }
  get pictureInPictureElement() { return null; }
  get designMode() { return 'off'; }
  get implementation() { return IMPLEMENTATION; }
  get fonts() {
    var ready = typeof Promise === 'function' ? Promise.resolve() : { then: function (f) { g.setTimeout(f, 0); } };
    return { ready: ready, status: 'loaded', check: function () { return true; }, load: function () { return ready; }, addEventListener: function () {}, removeEventListener: function () {}, forEach: function () {} };
  }

  createElement(name) {
    var s = String(name), def = CE.count ? CE.defs[s.toLowerCase()] : undefined;
    if (!def) return n.createElement(s);
    // a defined custom element is constructed right away
    try {
      var el = new def.ctor();
      if (!(el instanceof Node) || n.kind(el) !== 1) throw new TypeError("The constructor of '" + def.name + "' did not return an element");
      return el;
    } catch (e) {
      reportError(e);
      var failed = n.createElement(s);
      hide(failed, CE_STATE, 'failed');
      return failed;
    }
  }
  createElementNS(ns, name) { var s = String(name), at = s.indexOf(':'); return n.createElement(at >= 0 ? s.slice(at + 1) : s); }
  createTextNode(data) { return n.createText(String(data)); }
  createComment(data) { return n.createComment(String(data)); }
  createDocumentFragment() { return n.createFragment(); }
  createEvent(name) { return createEventByName(name); }
  createRange() { return new Range(); }
  createTreeWalker(root, what, filter) { return new TreeWalker(root, what, filter); }
  createNodeIterator(root, what, filter) { return new TreeWalker(root, what, filter); }
  createAttribute(name) { return { name: String(name).toLowerCase(), value: '' }; }
  importNode(node, deep) {
    var c = n.clone(node, !!deep);
    if (CE.count) ceUpgradeTree(c);
    return c;
  }
  adoptNode(node) { if (n.parent(node)) n.remove(node); return node; }

  getElementById(id) { return n.byId(String(id)); }
  getElementsByTagName(name) { return nodeList(n.byTag(this, String(name))); }
  getElementsByTagNameNS(ns, name) { return nodeList(n.byTag(this, String(name))); }
  getElementsByClassName(names) { return nodeList(n.byClass(this, String(names))); }
  getElementsByName(name) {
    var all = n.byTag(this, '*'), out = [];
    name = String(name);
    for (var i = 0; i < all.length; i++) if (n.getAttr(all[i], 'name') === name) out.push(all[i]);
    return nodeList(out);
  }
  elementFromPoint() { return null; }
  elementsFromPoint() { return []; }
  getSelection() { return SELECTION; }
  hasFocus() { return true; }
  execCommand() { return false; }
  queryCommandSupported() { return false; }
  write() { n.log(2, 'document.write is not supported; the call was ignored'); }
  writeln() { n.log(2, 'document.write is not supported; the call was ignored'); }
  open() { return this; }
  close() {}
  exitFullscreen() {}
}
methods(Document.prototype, parentNode);
mixGetters(Document.prototype, parentGetters);
handlerProps(Document.prototype);
n.setProto('#document', Document.prototype);

// A document that is not the page: what `createHTMLDocument` and `DOMParser` return. It is a
// detached subtree, so nothing in it is rendered, loaded or run, and writing to it can never
// touch the page.
class InertDocument extends DocumentFragment {
  get nodeType() { return 9; }
  get nodeName() { return '#document'; }
  get documentElement() { return n.children(this, true)[0] || null; }
  get head() { return n.query(this, 'head', 0); }
  get body() { return n.query(this, 'body', 0); }
  get title() { var t = n.query(this, 'title', 0); return t ? n.text(t) : ''; }
  set title(v) {
    var t = n.query(this, 'title', 0), h = this.head;
    if (!t && h) { t = n.createElement('title'); n.insert(h, t, null); }
    if (t) n.setText(t, String(v));
  }
  get defaultView() { return null; }
  get location() { return null; }
  get URL() { return 'about:blank'; }
  get documentURI() { return 'about:blank'; }
  get readyState() { return 'complete'; }
  get cookie() { return ''; }
  set cookie(v) {}
  get compatMode() { return 'CSS1Compat'; }
  get contentType() { return 'text/html'; }
  get characterSet() { return 'UTF-8'; }
  get implementation() { return IMPLEMENTATION; }
  get ownerDocument() { return null; }
  get activeElement() { return this.body; }
  get scripts() { return nodeList(n.byTag(this, 'script')); }
  get images() { return nodeList(n.byTag(this, 'img')); }
  get forms() { return nodeList(n.byTag(this, 'form')); }
  get links() { return nodeList(n.query(this, 'a[href], area[href]', 1)); }
  get styleSheets() { return []; }
  getElementsByTagName(name) { return nodeList(n.byTag(this, String(name))); }
  getElementsByClassName(names) { return nodeList(n.byClass(this, String(names))); }
  write() {}
  writeln() {}
  open() { return this; }
  close() {}
}
(function () {
  // creating nodes does not depend on which document asks
  var shared = ['createElement', 'createElementNS', 'createTextNode', 'createComment', 'createDocumentFragment',
    'createEvent', 'createRange', 'createTreeWalker', 'createNodeIterator', 'createAttribute', 'importNode',
    'adoptNode', 'getElementsByName', 'getSelection', 'hasFocus', 'execCommand', 'elementFromPoint'];
  for (var i = 0; i < shared.length; i++) hide(InertDocument.prototype, shared[i], Document.prototype[shared[i]]);
})();
function inertDocument(title) {
  var doc = n.createFragment();
  Object.setPrototypeOf(doc, InertDocument.prototype);
  var root = n.createElement('html'), head = n.createElement('head'), body = n.createElement('body');
  n.insert(doc, root, null);
  n.insert(root, head, null);
  n.insert(root, body, null);
  if (title !== undefined && title !== null) doc.title = title;
  return doc;
}
var IMPLEMENTATION = {
  hasFeature: function () { return true; },
  createHTMLDocument: function (title) { return inertDocument(title === undefined ? '' : title); },
  createDocument: function () { return inertDocument(null); },
  createDocumentType: function () { return null; }
};
class DOMParser {
  parseFromString(text) {
    var doc = inertDocument(null), body = doc.body;
    n.insert(body, n.parse(body, String(text)), null);
    return doc;
  }
}
class XMLSerializer {
  serializeToString(node) { return n.kind(node) === 1 ? n.html(node, true) : n.html(node, false); }
}
n.setProto('#custom', HTMLElement.prototype);

var document = n.document();

// ───────────────────────────── URL ─────────────────────────────

// Split an absolute, normalised URL (as the engine's resolver returns it) into its parts.
function splitUrl(href) {
  var u = { href: href, protocol: '', username: '', password: '', hostname: '', port: '', pathname: '', search: '', hash: '' };
  var rest = href, at = rest.indexOf('#');
  if (at >= 0) { u.hash = rest.slice(at); rest = rest.slice(0, at); }
  at = rest.indexOf('?');
  if (at >= 0) { u.search = rest.slice(at); rest = rest.slice(0, at); }
  at = rest.indexOf(':');
  if (at < 0) { u.pathname = rest; return u; }
  u.protocol = rest.slice(0, at + 1);
  rest = rest.slice(at + 1);
  if (rest.slice(0, 2) === '//') {
    rest = rest.slice(2);
    var slash = rest.indexOf('/');
    var auth = slash >= 0 ? rest.slice(0, slash) : rest;
    u.pathname = slash >= 0 ? rest.slice(slash) : '/';
    at = auth.lastIndexOf('@');
    if (at >= 0) {
      var cred = auth.slice(0, at), colon = cred.indexOf(':');
      u.username = colon >= 0 ? cred.slice(0, colon) : cred;
      u.password = colon >= 0 ? cred.slice(colon + 1) : '';
      auth = auth.slice(at + 1);
    }
    var pc = auth.lastIndexOf(':');
    if (pc >= 0 && auth.indexOf(']', pc) < 0) { u.port = auth.slice(pc + 1); auth = auth.slice(0, pc); }
    u.hostname = auth;
  } else {
    u.pathname = rest;
  }
  if (u.search === '?') u.search = '';
  if (u.hash === '#') u.hash = '';
  return u;
}
function hostOf(u) { return u.hostname + (u.port ? ':' + u.port : ''); }
function originOf(u) {
  return (u.protocol === 'http:' || u.protocol === 'https:' || u.protocol === 'ws:' || u.protocol === 'wss:' || u.protocol === 'ftp:') ? u.protocol + '//' + hostOf(u) : 'null';
}
function joinUrl(u) {
  var s = u.protocol;
  if (u.hostname || u.protocol === 'file:') {
    s += '//';
    if (u.username) s += u.username + (u.password ? ':' + u.password : '') + '@';
    s += hostOf(u);
  }
  return s + u.pathname + u.search + u.hash;
}
function formDecode(s) {
  try { return decodeURIComponent(s.split('+').join(' ')); } catch (e) { return s; }
}
function formEncode(s) {
  return encodeURIComponent(s).split('%20').join('+');
}

class URLSearchParams {
  constructor(init) {
    hide(this, '_list', []);
    hide(this, '_url', null);
    if (init === undefined || init === null) return;
    if (typeof init === 'string') this._parse(init);
    else if (init instanceof URLSearchParams) this._list = init._list.slice();
    else if (typeof init[Symbol.iterator] === 'function') { for (var pair of init) this._list.push([String(pair[0]), String(pair[1])]); }
    else { for (var k in init) if (hasOwn(init, k)) this._list.push([k, String(init[k])]); }
  }
  _parse(s) {
    if (s[0] === '?') s = s.slice(1);
    var parts = s.split('&');
    for (var i = 0; i < parts.length; i++) {
      if (!parts[i]) continue;
      var eq = parts[i].indexOf('=');
      this._list.push(eq < 0 ? [formDecode(parts[i]), ''] : [formDecode(parts[i].slice(0, eq)), formDecode(parts[i].slice(eq + 1))]);
    }
  }
  _changed() { if (this._url) { var s = this.toString(); this._url._u.search = s ? '?' + s : ''; } }
  append(k, v) { this._list.push([String(k), String(v)]); this._changed(); }
  delete(k) { k = String(k); this._list = this._list.filter(function (p) { return p[0] !== k; }); this._changed(); }
  get(k) { k = String(k); for (var i = 0; i < this._list.length; i++) if (this._list[i][0] === k) return this._list[i][1]; return null; }
  getAll(k) { k = String(k); var out = []; for (var i = 0; i < this._list.length; i++) if (this._list[i][0] === k) out.push(this._list[i][1]); return out; }
  has(k) { return this.get(k) !== null; }
  set(k, v) {
    k = String(k); v = String(v);
    var done = false, out = [];
    for (var i = 0; i < this._list.length; i++) {
      if (this._list[i][0] !== k) out.push(this._list[i]);
      else if (!done) { out.push([k, v]); done = true; }
    }
    if (!done) out.push([k, v]);
    this._list = out;
    this._changed();
  }
  sort() { this._list.sort(function (a, b) { return a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0; }); this._changed(); }
  forEach(cb, self) { for (var i = 0; i < this._list.length; i++) cb.call(self, this._list[i][1], this._list[i][0], this); }
  keys() { return this._list.map(function (p) { return p[0]; })[Symbol.iterator](); }
  values() { return this._list.map(function (p) { return p[1]; })[Symbol.iterator](); }
  entries() { return this._list.map(function (p) { return [p[0], p[1]]; })[Symbol.iterator](); }
  [Symbol.iterator]() { return this.entries(); }
  get size() { return this._list.length; }
  toString() {
    var s = '';
    for (var i = 0; i < this._list.length; i++) s += (i ? '&' : '') + formEncode(this._list[i][0]) + '=' + formEncode(this._list[i][1]);
    return s;
  }
}

class URL {
  constructor(url, base) {
    var abs = n.resolve(String(url), base === undefined ? 'about:blank' : String(base));
    if (abs === null) throw new TypeError("Failed to construct 'URL': Invalid URL");
    hide(this, '_u', splitUrl(abs));
    hide(this, '_sp', null);
  }
  static canParse(url, base) { return n.resolve(String(url), base === undefined ? 'about:blank' : String(base)) !== null; }
  get href() { return joinUrl(this._u); }
  set href(v) { var abs = n.resolve(String(v), 'about:blank'); if (abs === null) throw new TypeError('Invalid URL'); this._u = splitUrl(abs); this._sp = null; }
  get origin() { return originOf(this._u); }
  get protocol() { return this._u.protocol; }
  set protocol(v) { v = String(v); this._u.protocol = v[v.length - 1] === ':' ? v : v + ':'; }
  get username() { return this._u.username; }
  set username(v) { this._u.username = String(v); }
  get password() { return this._u.password; }
  set password(v) { this._u.password = String(v); }
  get host() { return hostOf(this._u); }
  set host(v) { v = String(v); var c = v.lastIndexOf(':'); if (c >= 0) { this._u.hostname = v.slice(0, c); this._u.port = v.slice(c + 1); } else this._u.hostname = v; }
  get hostname() { return this._u.hostname; }
  set hostname(v) { this._u.hostname = String(v); }
  get port() { return this._u.port; }
  set port(v) { this._u.port = String(v); }
  get pathname() { return this._u.pathname; }
  set pathname(v) { v = String(v); this._u.pathname = v[0] === '/' ? v : '/' + v; }
  get search() { return this._u.search; }
  set search(v) { v = String(v); this._u.search = v && v[0] !== '?' ? '?' + v : (v === '?' ? '' : v); this._sp = null; }
  get hash() { return this._u.hash; }
  set hash(v) { v = String(v); this._u.hash = v && v[0] !== '#' ? '#' + v : (v === '#' ? '' : v); }
  get searchParams() {
    if (!this._sp) { var sp = new URLSearchParams(this._u.search); sp._url = this; this._sp = sp; }
    return this._sp;
  }
  toString() { return this.href; }
  toJSON() { return this.href; }
}
hide(URL, 'createObjectURL', function () { return 'blob:null/unsupported'; });
hide(URL, 'revokeObjectURL', function () {});

// <a> and <area> expose the parts of their address (scripts use a link element as a URL parser)
(function () {
  function part(name) {
    getter(HTMLAnchorElement.prototype, name, function () {
      var href = this.href;
      if (!href) return name === 'protocol' ? ':' : '';
      var u = splitUrl(href);
      return name === 'host' ? hostOf(u) : name === 'origin' ? originOf(u) : u[name];
    }, function (v) {
      if (name === 'origin') return;
      try {
        var u = new URL(this.href);
        u[name] = v;
        n.setAttr(this, 'href', u.href);
      } catch (e) {}
    });
  }
  var names = ['protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin', 'username', 'password'];
  for (var i = 0; i < names.length; i++) part(names[i]);
})();

// ───────────────────────────── location and history ─────────────────────────────

var location = {};
(function () {
  function part(name) {
    getter(location, name, function () { var u = splitUrl(n.url()); return name === 'host' ? hostOf(u) : name === 'origin' ? originOf(u) : u[name]; },
      function (v) {
        if (name === 'origin') return;
        var u = new URL(n.url());
        u[name] = v;
        if (name === 'hash') setHash(u.href); else n.navigate(u.href, false);
      });
  }
  var names = ['protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin'];
  for (var i = 0; i < names.length; i++) part(names[i]);
  getter(location, 'href', function () { return n.url(); }, function (v) { n.navigate(String(v), false); });
  getter(location, 'ancestorOrigins', function () { return []; });
  methods(location, {
    assign: function (u) { n.navigate(String(u), false); },
    replace: function (u) { n.navigate(String(u), true); },
    reload: function () { n.reload(); },
    toString: function () { return n.url(); }
  });
})();
function setHash(href) {
  var old = n.url();
  if (n.setUrl(href) && old !== href) g.setTimeout(function () { trusted(g, 'hashchange', { oldURL: old, newURL: href }); }, 0);
}

var historyState = null;
var history = {
  get length() { return 1; },
  get state() { return historyState; },
  scrollRestoration: 'auto',
  pushState: function (state, title, url) {
    if (url !== undefined && url !== null && !n.setUrl(String(url))) throw new DOMException('The URL is not same-origin with the document', 'SecurityError');
    historyState = state === undefined ? null : state;
  },
  replaceState: function (state, title, url) {
    if (url !== undefined && url !== null && !n.setUrl(String(url))) throw new DOMException('The URL is not same-origin with the document', 'SecurityError');
    historyState = state === undefined ? null : state;
  },
  back: function () {}, forward: function () {}, go: function () {}
};
