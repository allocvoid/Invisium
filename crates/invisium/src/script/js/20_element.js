
// ───────────────────────────── inline style ─────────────────────────────

// "a: b; c: d !important" -> [[name, value, important], ...]. Semicolons and colons inside
// parentheses or quotes (data: URLs) do not split.
function parseDecls(text) {
  var out = [], depth = 0, quote = '', start = 0, colon = -1, i, c;
  function flush(end) {
    if (colon > start) {
      var name = text.slice(start, colon).trim();
      var value = text.slice(colon + 1, end).trim();
      var important = false;
      var low = value.toLowerCase();
      if (low.length >= 10 && low.slice(low.length - 10) === '!important') {
        important = true;
        value = value.slice(0, value.length - 10).trim();
      }
      if (name.slice(0, 2) !== '--') name = name.toLowerCase();
      if (name && value) out.push([name, value, important]);
    }
    start = end + 1; colon = -1;
  }
  for (i = 0; i < text.length; i++) {
    c = text[i];
    if (quote) { if (c === '\\') i++; else if (c === quote) quote = ''; continue; }
    if (c === '"' || c === "'") quote = c;
    else if (c === '(') depth++;
    else if (c === ')') { if (depth > 0) depth--; }
    else if (c === ':' && depth === 0 && colon < 0) colon = i;
    else if (c === ';' && depth === 0) flush(i);
  }
  flush(text.length);
  return out;
}
function serializeDecls(list) {
  var s = '';
  for (var i = 0; i < list.length; i++) {
    if (i) s += ' ';
    s += list[i][0] + ': ' + list[i][1] + (list[i][2] ? ' !important' : '') + ';';
  }
  return s;
}
function cssName(prop) {
  // camelCase / cssFloat -> kebab-case
  if (prop === 'cssFloat') return 'float';
  if (prop.indexOf('-') >= 0) return prop.slice(0, 2) === '--' ? prop : prop.toLowerCase();
  var out = '';
  for (var i = 0; i < prop.length; i++) {
    var c = prop[i];
    if (c >= 'A' && c <= 'Z') out += '-' + c.toLowerCase(); else out += c;
  }
  if (out.slice(0, 7) === 'webkit-' || out.slice(0, 4) === 'moz-' || out.slice(0, 3) === 'ms-') out = '-' + out;
  return out;
}

class CSSStyleDeclaration {
  constructor(el) { hide(this, '_el', el); }
  _list() { return parseDecls(n.getAttr(this._el, 'style') || ''); }
  _store(list) {
    if (list.length) n.setAttr(this._el, 'style', serializeDecls(list));
    else n.removeAttr(this._el, 'style');
  }
  getPropertyValue(name) {
    name = cssName(String(name));
    var l = this._list();
    for (var i = l.length - 1; i >= 0; i--) if (l[i][0] === name) return l[i][1];
    return '';
  }
  getPropertyPriority(name) {
    name = cssName(String(name));
    var l = this._list();
    for (var i = l.length - 1; i >= 0; i--) if (l[i][0] === name) return l[i][2] ? 'important' : '';
    return '';
  }
  setProperty(name, value, priority) {
    name = cssName(String(name));
    if (value === null || value === undefined || value === '') { this.removeProperty(name); return; }
    value = String(value);
    var l = this._list(), important = String(priority || '').toLowerCase() === 'important';
    for (var i = 0; i < l.length; i++) {
      if (l[i][0] === name) { l[i][1] = value; l[i][2] = important; this._store(l); return; }
    }
    l.push([name, value, important]);
    this._store(l);
  }
  removeProperty(name) {
    name = cssName(String(name));
    var l = this._list(), old = '', kept = [];
    for (var i = 0; i < l.length; i++) { if (l[i][0] === name) old = l[i][1]; else kept.push(l[i]); }
    if (kept.length !== l.length) this._store(kept);
    return old;
  }
  get cssText() { return serializeDecls(this._list()); }
  set cssText(v) { this._store(parseDecls(String(v))); }
  get length() { return this._list().length; }
  item(i) { var l = this._list(); return i >= 0 && i < l.length ? l[i][0] : ''; }
  get parentRule() { return null; }
}

// What `getComputedStyle` returns: read-only, backed by the engine's computed values.
class ComputedStyle {
  constructor(el) { hide(this, '_el', el); }
  getPropertyValue(name) { return n.computed(this._el, cssName(String(name))); }
  getPropertyPriority() { return ''; }
  setProperty() {}
  removeProperty() { return ''; }
  get cssText() { return ''; }
  get length() { return 0; }
  item() { return ''; }
}

var CSS_PROPS = ('align-content align-items align-self animation animation-delay animation-direction animation-duration ' +
  'animation-fill-mode animation-iteration-count animation-name animation-play-state animation-timing-function appearance ' +
  'aspect-ratio backdrop-filter backface-visibility background background-attachment background-clip background-color ' +
  'background-image background-origin background-position background-repeat background-size border border-bottom ' +
  'border-bottom-color border-bottom-left-radius border-bottom-right-radius border-bottom-style border-bottom-width ' +
  'border-collapse border-color border-image border-left border-left-color border-left-style border-left-width ' +
  'border-radius border-right border-right-color border-right-style border-right-width border-spacing border-style ' +
  'border-top border-top-color border-top-left-radius border-top-right-radius border-top-style border-top-width ' +
  'border-width bottom box-shadow box-sizing caret-color clear clip clip-path color column-count column-gap columns ' +
  'contain content cursor direction display fill filter flex flex-basis flex-direction flex-flow flex-grow flex-shrink ' +
  'flex-wrap float font font-family font-size font-style font-variant font-weight gap grid grid-area grid-auto-columns ' +
  'grid-auto-flow grid-auto-rows grid-column grid-column-end grid-column-start grid-gap grid-row grid-row-end ' +
  'grid-row-start grid-template grid-template-areas grid-template-columns grid-template-rows height inset isolation ' +
  'justify-content justify-items justify-self left letter-spacing line-height list-style list-style-image ' +
  'list-style-position list-style-type margin margin-bottom margin-left margin-right margin-top mask mask-image ' +
  'max-height max-width min-height min-width mix-blend-mode object-fit object-position opacity order outline ' +
  'outline-color outline-offset outline-style outline-width overflow overflow-wrap overflow-x overflow-y ' +
  'overscroll-behavior padding padding-bottom padding-left padding-right padding-top perspective place-content ' +
  'place-items place-self pointer-events position resize right rotate row-gap scale scroll-behavior stroke ' +
  'stroke-width tab-size table-layout text-align text-decoration text-decoration-color text-decoration-line ' +
  'text-indent text-overflow text-shadow text-transform top touch-action transform transform-origin transform-style ' +
  'transition transition-delay transition-duration transition-property transition-timing-function translate ' +
  'user-select vertical-align visibility white-space width will-change word-break word-spacing word-wrap ' +
  'writing-mode z-index zoom -webkit-transform -webkit-transition -webkit-user-select -webkit-appearance ' +
  '-webkit-line-clamp -webkit-box-orient -webkit-overflow-scrolling -webkit-tap-highlight-color').split(' ');

function camel(name) {
  var out = '', up = false;
  for (var i = 0; i < name.length; i++) {
    var c = name[i];
    if (c === '-') { if (i > 0) up = true; continue; }
    out += up ? c.toUpperCase() : c;
    up = false;
  }
  return out;
}
(function () {
  function install(proto, name) {
    var get = function () { return this.getPropertyValue(name); };
    var set = function (v) { this.setProperty(name, v); };
    var c = camel(name);
    getter(proto, c, get, set);
    if (c !== name) getter(proto, name, get, set);
  }
  for (var i = 0; i < CSS_PROPS.length; i++) {
    install(CSSStyleDeclaration.prototype, CSS_PROPS[i]);
    install(ComputedStyle.prototype, CSS_PROPS[i]);
  }
  getter(CSSStyleDeclaration.prototype, 'cssFloat', function () { return this.getPropertyValue('float'); }, function (v) { this.setProperty('float', v); });
  getter(ComputedStyle.prototype, 'cssFloat', function () { return this.getPropertyValue('float'); });
})();

// ───────────────────────────── elements ─────────────────────────────

class DOMRect {
  constructor(x, y, w, h) { this.x = +x || 0; this.y = +y || 0; this.width = +w || 0; this.height = +h || 0; }
  get top() { return this.y; }
  get left() { return this.x; }
  get right() { return this.x + this.width; }
  get bottom() { return this.y + this.height; }
  toJSON() { return { x: this.x, y: this.y, width: this.width, height: this.height, top: this.top, left: this.left, right: this.right, bottom: this.bottom }; }
}

var SVG_TAGS = { svg: 1, path: 1, g: 1, rect: 1, circle: 1, ellipse: 1, line: 1, polygon: 1, polyline: 1, use: 1, defs: 1, symbol: 1, text: 1, tspan: 1, lineargradient: 1, radialgradient: 1, stop: 1, clippath: 1, mask: 1, pattern: 1, image: 1, foreignobject: 1 };
var STYLE = Symbol('style'), CLASSES = Symbol('classList'), DATA = Symbol('dataset');

function dataName(prop) {
  var out = 'data-';
  for (var i = 0; i < prop.length; i++) {
    var c = prop[i];
    if (c >= 'A' && c <= 'Z') out += '-' + c.toLowerCase(); else out += c;
  }
  return out;
}
function makeDataset(el) {
  if (typeof Proxy === 'function') {
    return new Proxy({}, {
      get: function (t, k) { if (typeof k !== 'string') return undefined; var v = n.getAttr(el, dataName(k)); return v === null ? undefined : v; },
      set: function (t, k, v) { n.setAttr(el, dataName(String(k)), String(v)); return true; },
      has: function (t, k) { return typeof k === 'string' && n.hasAttr(el, dataName(k)); },
      deleteProperty: function (t, k) { n.removeAttr(el, dataName(String(k))); return true; },
      ownKeys: function () {
        var names = n.attrNames(el), out = [];
        for (var i = 0; i < names.length; i++) if (names[i].slice(0, 5) === 'data-') out.push(camel(names[i].slice(5)));
        return out;
      },
      getOwnPropertyDescriptor: function (t, k) {
        var v = typeof k === 'string' ? n.getAttr(el, dataName(k)) : null;
        return v === null ? undefined : { value: v, writable: true, enumerable: true, configurable: true };
      }
    });
  }
  // without Proxy: a snapshot (writes through it are not reflected)
  var names = n.attrNames(el), snap = {};
  for (var i = 0; i < names.length; i++) if (names[i].slice(0, 5) === 'data-') snap[camel(names[i].slice(5))] = n.getAttr(el, names[i]);
  return snap;
}

function adjacent(el, where, node) {
  var p;
  switch (String(where).toLowerCase()) {
    case 'beforebegin': p = n.parent(el); if (p) n.insert(p, node, el); break;
    case 'afterbegin': n.insert(el, node, n.first(el)); break;
    case 'beforeend': n.insert(el, node, null); break;
    case 'afterend': p = n.parent(el); if (p) n.insert(p, node, n.next(el)); break;
    default: throw new DOMException('Invalid position', 'SyntaxError');
  }
}
function isRootScroller(el) { var t = n.tag(el); return t === 'html' || t === 'body'; }

class Element extends Node {
  get tagName() { return n.tag(this).toUpperCase(); }
  get localName() { return n.tag(this); }
  get prefix() { return null; }
  get namespaceURI() { return SVG_TAGS[n.tag(this)] ? 'http://www.w3.org/2000/svg' : 'http://www.w3.org/1999/xhtml'; }
  get id() { return n.getAttr(this, 'id') || ''; }
  set id(v) { n.setAttr(this, 'id', String(v)); }
  get className() { return n.getAttr(this, 'class') || ''; }
  set className(v) { n.setAttr(this, 'class', String(v)); }
  get classList() { return this[CLASSES] || (hide(this, CLASSES, new DOMTokenList(this, 'class')), this[CLASSES]); }
  set classList(v) { n.setAttr(this, 'class', String(v)); }
  get slot() { return n.getAttr(this, 'slot') || ''; }
  animate(keyframes, options) { return elementAnimate(this, keyframes, options); }
  getAnimations(options) { return animationsOf(this, !!(options && options.subtree)); }
  get shadowRoot() { var r = this._shadow; return r && r.mode === 'open' ? r : null; }
  attachShadow(init) {
    if (this._shadow) throw new DOMException('The element already has a shadow root', 'NotSupportedError');
    var root = n.createFragment();
    Object.setPrototypeOf(root, ShadowRoot.prototype);
    hide(root, 'host', this);
    hide(root, 'mode', init && init.mode === 'closed' ? 'closed' : 'open');
    hide(root, 'delegatesFocus', !!(init && init.delegatesFocus));
    hide(this, '_shadow', root);
    return root;
  }
  get assignedSlot() { return null; }

  getAttribute(name) { return n.getAttr(this, String(name)); }
  setAttribute(name, value) { n.setAttr(this, String(name), String(value)); }
  removeAttribute(name) { n.removeAttr(this, String(name)); }
  hasAttribute(name) { return n.hasAttr(this, String(name)); }
  toggleAttribute(name, force) {
    name = String(name);
    if (n.hasAttr(this, name)) { if (force === true) return true; n.removeAttr(this, name); return false; }
    if (force === false) return false;
    n.setAttr(this, name, '');
    return true;
  }
  getAttributeNames() { return n.attrNames(this); }
  hasAttributes() { return n.attrNames(this).length > 0; }
  getAttributeNS(ns, name) { return n.getAttr(this, String(name)); }
  setAttributeNS(ns, name, value) { n.setAttr(this, String(name), String(value)); }
  removeAttributeNS(ns, name) { n.removeAttr(this, String(name)); }
  hasAttributeNS(ns, name) { return n.hasAttr(this, String(name)); }
  getAttributeNode(name) {
    var v = n.getAttr(this, String(name));
    return v === null ? null : { name: String(name).toLowerCase(), localName: String(name).toLowerCase(), value: v, nodeValue: v, specified: true, ownerElement: this };
  }
  setAttributeNode(attr) {
    var old = this.getAttributeNode(attr.name);
    n.setAttr(this, String(attr.name), String(attr.value));
    return old;
  }
  removeAttributeNode(attr) {
    var old = this.getAttributeNode(attr.name);
    if (old === null) throw new DOMException('The attribute is not on this element', 'NotFoundError');
    n.removeAttr(this, String(attr.name));
    return old;
  }
  get attributes() {
    var names = n.attrNames(this), out = [], self = this;
    for (var i = 0; i < names.length; i++) {
      var v = n.getAttr(this, names[i]);
      out.push({ name: names[i], localName: names[i], nodeName: names[i], value: v, nodeValue: v, specified: true, ownerElement: this });
    }
    for (var j = 0; j < out.length; j++) if (!(out[j].name in out)) hide(out, out[j].name, out[j]);
    hide(out, 'item', function (i) { return this[i] || null; });
    hide(out, 'getNamedItem', function (name) { return self.getAttributeNode(name); });
    hide(out, 'setNamedItem', function (attr) { self.setAttribute(attr.name, attr.value); return null; });
    hide(out, 'removeNamedItem', function (name) { self.removeAttribute(name); });
    return out;
  }

  get innerHTML() { return n.html(this, false); }
  set innerHTML(v) {
    var f = n.parse(this, v === null ? '' : String(v)), c;
    while ((c = n.first(this))) n.remove(c);
    n.insert(this, f, null);
  }
  get outerHTML() { return n.html(this, true); }
  set outerHTML(v) {
    var p = n.parent(this);
    if (!p) return;
    var f = n.parse(n.kind(p) === 1 ? p : this, String(v)), next = n.next(this);
    n.remove(this);
    n.insert(p, f, next);
  }
  insertAdjacentHTML(where, html) {
    var w = String(where).toLowerCase();
    var ctx = (w === 'beforebegin' || w === 'afterend') ? (this.parentElement || this) : this;
    adjacent(this, where, n.parse(ctx, String(html)));
  }
  insertAdjacentElement(where, el) { adjacent(this, where, el); return el; }
  insertAdjacentText(where, text) { adjacent(this, where, n.createText(String(text))); }

  matches(s) { return n.query(this, String(s), 2); }
  webkitMatchesSelector(s) { return n.query(this, String(s), 2); }
  closest(s) { return n.query(this, String(s), 3); }
  getElementsByTagName(name) { return nodeList(n.byTag(this, String(name))); }
  getElementsByTagNameNS(ns, name) { return nodeList(n.byTag(this, String(name))); }
  getElementsByClassName(names) { return nodeList(n.byClass(this, String(names))); }

  getBoundingClientRect() { var r = n.rect(this); return new DOMRect(r[0], r[1], r[2], r[3]); }
  getClientRects() { var r = n.rect(this); return r[2] || r[3] ? [new DOMRect(r[0], r[1], r[2], r[3])] : []; }
  get offsetParent() { return n.offsetParent(this); }
  get offsetLeft() { return Math.round(n.metrics(this)[0]); }
  get offsetTop() { return Math.round(n.metrics(this)[1]); }
  get offsetWidth() { return Math.round(n.metrics(this)[2]); }
  get offsetHeight() { return Math.round(n.metrics(this)[3]); }
  get clientLeft() { return Math.round(n.metrics(this)[4]); }
  get clientTop() { return Math.round(n.metrics(this)[5]); }
  get clientWidth() { return Math.round(n.metrics(this)[6]); }
  get clientHeight() { return Math.round(n.metrics(this)[7]); }
  get scrollWidth() { return Math.round(n.metrics(this)[8]); }
  get scrollHeight() { return Math.round(n.metrics(this)[9]); }
  get scrollTop() { return isRootScroller(this) ? n.viewport()[3] : 0; }
  set scrollTop(v) { if (isRootScroller(this)) n.scrollTo(n.viewport()[2], +v || 0); }
  get scrollLeft() { return isRootScroller(this) ? n.viewport()[2] : 0; }
  set scrollLeft(v) { if (isRootScroller(this)) n.scrollTo(+v || 0, n.viewport()[3]); }
  scrollIntoView() { var r = n.rect(this), v = n.viewport(); n.scrollTo(v[2], v[3] + r[1]); }
  scrollTo(x, y) { if (isRootScroller(this)) g.scrollTo(x, y); }
  scrollBy(x, y) { if (isRootScroller(this)) g.scrollBy(x, y); }
  scroll(x, y) { this.scrollTo(x, y); }
  // pointer events go to this element until the buttons are up (only while one is down)
  setPointerCapture() {
    if (!PTR.buttons || !this.isConnected || PTR.capture === this) return;
    PTR.capture = this;
    trusted(this, 'gotpointercapture', { clientX: PTR.x, clientY: PTR.y, buttons: PTR.buttons });
  }
  releasePointerCapture() {
    if (PTR.capture !== this) return;
    PTR.capture = null;
    trusted(this, 'lostpointercapture', { clientX: PTR.x, clientY: PTR.y, buttons: PTR.buttons });
  }
  hasPointerCapture() { return PTR.capture === this; }
  requestFullscreen() {}
}
methods(Element.prototype, childNode);
methods(Element.prototype, parentNode);
mixGetters(Element.prototype, siblingElements);
mixGetters(Element.prototype, parentGetters);
