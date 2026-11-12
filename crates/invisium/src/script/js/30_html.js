
// ───────────────────────────── HTML elements ─────────────────────────────

function reflectStr(proto, prop, attr) {
  getter(proto, prop, function () { return n.getAttr(this, attr) || ''; }, function (v) { n.setAttr(this, attr, String(v)); });
}
function reflectBool(proto, prop, attr) {
  getter(proto, prop, function () { return n.hasAttr(this, attr); }, function (v) { if (v) n.setAttr(this, attr, ''); else n.removeAttr(this, attr); });
}
function reflectUrl(proto, prop, attr) {
  getter(proto, prop, function () {
    var v = n.getAttr(this, attr);
    if (v === null) return '';
    return n.resolve(v) || v;
  }, function (v) { n.setAttr(this, attr, String(v)); });
}
function reflectNum(proto, prop, attr, dflt) {
  getter(proto, prop, function () {
    var v = parseInt(n.getAttr(this, attr), 10);
    return isFinite(v) ? v : dflt;
  }, function (v) { n.setAttr(this, attr, String(v)); });
}
function strs(proto, names) {
  names = names.split(' ');
  for (var i = 0; i < names.length; i++) reflectStr(proto, names[i], names[i].toLowerCase());
}
function bools(proto, names) {
  names = names.split(' ');
  for (var i = 0; i < names.length; i++) reflectBool(proto, names[i], names[i].toLowerCase());
}

var EVENT_PROPS = ('abort blur change click close contextmenu dblclick error focus focusin focusout input invalid keydown ' +
  'keypress keyup load mousedown mouseenter mouseleave mousemove mouseout mouseover mouseup pointerdown pointermove ' +
  'pointerup reset resize scroll select submit touchend touchmove touchstart transitionend animationend wheel ' +
  'readystatechange').split(' ');
function handlerProps(proto) {
  function install(name) {
    getter(proto, 'on' + name, function () {
      var h = this[H];
      if (h && name in h) return h[name];
      var src = n.kind(this) === 1 ? n.getAttr(this, 'on' + name) : null;
      if (src === null) return null;
      var f = null;
      try { f = new Function('event', src); } catch (e) { reportError(e); }
      if (!h) { h = Object.create(null); hide(this, H, h); }
      h[name] = f;
      return f;
    }, function (v) {
      var h = this[H];
      if (!h) { h = Object.create(null); hide(this, H, h); }
      h[name] = typeof v === 'function' ? v : null;
    });
  }
  for (var i = 0; i < EVENT_PROPS.length; i++) install(EVENT_PROPS[i]);
}

// What a click does when no listener cancels it (script-initiated clicks; the engine handles
// real pointer clicks itself).
function activate(el) {
  var tag = n.tag(el), type = (n.getAttr(el, 'type') || '').toLowerCase();
  if (n.hasAttr(el, 'disabled')) return;
  if (tag === 'a' || tag === 'area') {
    var href = n.getAttr(el, 'href');
    if (href !== null) n.navigate(href, false);
  } else if (tag === 'input' && type === 'checkbox') {
    el.checked = !el.checked;
    trusted(el, 'input'); trusted(el, 'change');
  } else if (tag === 'input' && type === 'radio') {
    if (!el.checked) { el.checked = true; trusted(el, 'input'); trusted(el, 'change'); }
  } else if ((tag === 'input' && (type === 'submit' || type === 'image')) || (tag === 'button' && (type === '' || type === 'submit'))) {
    var f = el.form;
    if (f) f.requestSubmit(el);
  } else if (tag === 'label') {
    var c = el.control;
    if (c) c.click();
  }
}
function setFocus(el) {
  var prev = n.active();
  if (prev === el) return;
  if (prev) {
    n.focus(null);
    trusted(prev, 'blur', { relatedTarget: el });
    trusted(prev, 'focusout', { relatedTarget: el });
  }
  if (el) {
    n.focus(el);
    trusted(el, 'focus', { relatedTarget: prev });
    trusted(el, 'focusin', { relatedTarget: prev });
  }
}


// ───────────────────────────── custom elements ─────────────────────────────
// The registry and the upgrade algorithm live here; the engine reports tree and attribute
// changes of dash-named elements through `ceReact` once the first element is defined.

var CE = { defs: Object.create(null), ctors: new Map(), waiting: Object.create(null), count: 0 };
var CE_STATE = Symbol('customElementState');
var CE_USED = {};
var RESERVED_NAMES = ['annotation-xml', 'color-profile', 'font-face', 'font-face-src', 'font-face-uri', 'font-face-format', 'font-face-name', 'missing-glyph'];

function validCustomName(name) {
  if (name.length < 2 || name.indexOf('-') < 1 || RESERVED_NAMES.indexOf(name) >= 0) return false;
  var c = name.charCodeAt(0);
  if (c < 97 || c > 122) return false;
  for (var i = 0; i < name.length; i++) {
    c = name.charCodeAt(i);
    if ((c >= 65 && c <= 90) || c <= 32 || c === 47 || c === 61 || c === 62) return false;
  }
  return true;
}
function ceCall(fn, el, a, b, c) {
  try { fn.call(el, a, b, c, null); } catch (e) { reportError(e); }
}
// Turn an existing element into an instance of its definition by running the constructor
// with the element on the definition's construction stack.
function ceUpgrade(el, def) {
  if (el[CE_STATE] !== undefined) return;
  hide(el, CE_STATE, 'failed');
  def.stack.push(el);
  var ok = false;
  try {
    var made = Reflect.construct(def.ctor, [], def.ctor);
    if (made !== el) throw new TypeError("The custom element constructor for '" + def.name + "' did not produce the element being upgraded");
    ok = true;
  } catch (e) { reportError(e); }
  def.stack.pop();
  if (!ok) return;
  el[CE_STATE] = 'custom';
  if (def.attrChanged && def.observed.length) {
    var names = n.attrNames(el);
    for (var i = 0; i < names.length; i++) {
      if (def.observed.indexOf(names[i]) >= 0) ceCall(def.attrChanged, el, names[i], null, n.getAttr(el, names[i]));
    }
  }
  if (def.connected && n.connected(el)) ceCall(def.connected, el);
}
function ceUpgradeTree(root) {
  if (!CE.count) return;
  var list = n.byTag(root, '*');
  if (n.kind(root) === 1) list.unshift(root);
  for (var i = 0; i < list.length; i++) {
    var def = CE.defs[n.tag(list[i])];
    if (def) ceUpgrade(list[i], def);
  }
}
// From the engine: [kind, element, attribute, old, new, ...]; kind 0 = connected,
// 1 = disconnected, 2 = attribute changed.
function ceReact(ev) {
  for (var i = 0; i + 4 < ev.length; i += 5) {
    var el = ev[i + 1], def = CE.defs[n.tag(el)];
    if (!def) continue;
    var state = el[CE_STATE];
    if (ev[i] === 0) {
      if (state === 'custom') { if (def.connected) ceCall(def.connected, el); }
      else if (state === undefined) ceUpgrade(el, def);
    } else if (state === 'custom') {
      if (ev[i] === 1) { if (def.disconnected) ceCall(def.disconnected, el); }
      else if (def.attrChanged && def.observed.indexOf(ev[i + 2]) >= 0) ceCall(def.attrChanged, el, ev[i + 2], ev[i + 3], ev[i + 4]);
    }
  }
}

class CustomElementRegistry {
  define(name, ctor, options) {
    name = String(name);
    if (typeof ctor !== 'function') throw new TypeError('The custom element constructor is not a function');
    if (!validCustomName(name)) throw new DOMException("'" + name + "' is not a valid custom element name", 'SyntaxError');
    if (CE.defs[name]) throw new DOMException("The name '" + name + "' has already been used with this registry", 'NotSupportedError');
    if (CE.ctors.has(ctor)) throw new DOMException('This constructor has already been used with this registry', 'NotSupportedError');
    var proto = ctor.prototype;
    if (proto === null || typeof proto !== 'object') throw new TypeError('The custom element constructor has no prototype object');
    var callback = function (v) { return typeof v === 'function' ? v : null; };
    var attrChanged = callback(proto.attributeChangedCallback), observed = [];
    if (attrChanged && ctor.observedAttributes) observed = Array.from(ctor.observedAttributes).map(String);
    var def = {
      name: name, ctor: ctor, stack: [], observed: observed, attrChanged: attrChanged,
      connected: callback(proto.connectedCallback), disconnected: callback(proto.disconnectedCallback),
      // customized built-ins (`extends`) are registered but never matched: not supported yet
      builtin: !!(options && options.extends)
    };
    CE.ctors.set(ctor, def);
    if (!def.builtin) CE.defs[name] = def;
    CE.count++;
    if (CE.count === 1) n.ceEnable(ceReact);
    if (!def.builtin) {
      var existing = n.byTag(document, name);
      for (var i = 0; i < existing.length; i++) ceUpgrade(existing[i], def);
    }
    var waiting = CE.waiting[name];
    if (waiting) {
      delete CE.waiting[name];
      for (var j = 0; j < waiting.length; j++) waiting[j](ctor);
    }
  }
  get(name) { var d = CE.defs[String(name)]; return d ? d.ctor : undefined; }
  getName(ctor) { var d = CE.ctors.get(ctor); return d ? d.name : null; }
  whenDefined(name) {
    name = String(name);
    if (!validCustomName(name)) return Promise.reject(new DOMException("'" + name + "' is not a valid custom element name", 'SyntaxError'));
    if (CE.defs[name]) return Promise.resolve(CE.defs[name].ctor);
    return new Promise(function (resolve) { (CE.waiting[name] || (CE.waiting[name] = [])).push(resolve); });
  }
  upgrade(root) { if (root instanceof Node) ceUpgradeTree(root); }
}
var customElements = new CustomElementRegistry();

class HTMLElement extends Element {
  // Only custom elements are constructed from script. During an upgrade the element already
  // exists and waits on its definition's construction stack; otherwise a new one is made.
  constructor() {
    var def = CE.ctors.get(new.target);
    if (!def) throw new TypeError('Illegal constructor');
    var el, top = def.stack.length - 1;
    if (top >= 0) {
      el = def.stack[top];
      if (el === CE_USED) throw new DOMException("The constructor of '" + def.name + "' ran twice for one element", 'InvalidStateError');
      def.stack[top] = CE_USED;
    } else {
      el = n.createElement(def.name);
      hide(el, CE_STATE, 'custom');
    }
    Object.setPrototypeOf(el, new.target.prototype);
    return el;
  }
  get style() { return this[STYLE] || (hide(this, STYLE, new CSSStyleDeclaration(this)), this[STYLE]); }
  set style(v) { n.setAttr(this, 'style', String(v)); }
  get dataset() { return this[DATA] || (hide(this, DATA, makeDataset(this)), this[DATA]); }
  get innerText() { return n.text(this); }
  set innerText(v) { n.setText(this, v === null || v === undefined ? '' : String(v)); }
  get outerText() { return n.text(this); }
  get tabIndex() { var v = parseInt(n.getAttr(this, 'tabindex'), 10); return isFinite(v) ? v : -1; }
  set tabIndex(v) { n.setAttr(this, 'tabindex', String(v)); }
  get isContentEditable() { return false; }
  get contentEditable() { return n.getAttr(this, 'contenteditable') || 'inherit'; }
  set contentEditable(v) { n.setAttr(this, 'contenteditable', String(v)); }
  click() {
    var ev = new MouseEvent('click', { bubbles: true, cancelable: true });
    if (dispatch(this, ev)) activate(this);
  }
  focus() { setFocus(this); }
  blur() { if (n.active() === this) setFocus(null); }
  attachInternals() { return {}; }
}
strs(HTMLElement.prototype, 'title lang dir accessKey role');
bools(HTMLElement.prototype, 'hidden draggable inert');
handlerProps(HTMLElement.prototype);

class SVGElement extends Element {
  get style() { return this[STYLE] || (hide(this, STYLE, new CSSStyleDeclaration(this)), this[STYLE]); }
  get dataset() { return this[DATA] || (hide(this, DATA, makeDataset(this)), this[DATA]); }
  get ownerSVGElement() { return this.closest('svg'); }
  getBBox() { var r = n.rect(this); return new DOMRect(0, 0, r[2], r[3]); }
  focus() {}
  blur() {}
}
handlerProps(SVGElement.prototype);

function formOf(el) { return n.query(el, 'form', 3); }
function controlsOf(form) { return n.query(form, 'input, select, textarea, button, fieldset, output', 1); }
function labelsOf(el) {
  var id = n.getAttr(el, 'id'), all = n.byTag(document, 'label'), out = [];
  for (var i = 0; i < all.length; i++) {
    if ((id && n.getAttr(all[i], 'for') === id) || n.contains(all[i], el)) out.push(all[i]);
  }
  return nodeList(out);
}
var VALIDITY = { valid: true, valueMissing: false, typeMismatch: false, patternMismatch: false, tooLong: false, tooShort: false, rangeUnderflow: false, rangeOverflow: false, stepMismatch: false, badInput: false, customError: false };
function formControl(C) {
  var p = C.prototype;
  getter(p, 'form', function () { return formOf(this); });
  getter(p, 'labels', function () { return labelsOf(this); });
  getter(p, 'validity', function () { return VALIDITY; });
  getter(p, 'validationMessage', function () { return ''; });
  getter(p, 'willValidate', function () { return !n.hasAttr(this, 'disabled'); });
  methods(p, { checkValidity: function () { return true; }, reportValidity: function () { return true; }, setCustomValidity: function () {} });
  strs(p, 'name');
  bools(p, 'disabled required autofocus');
}

class HTMLAnchorElement extends HTMLElement {
  get text() { return n.text(this); }
  set text(v) { n.setText(this, String(v)); }
  get relList() { return new DOMTokenList(this, 'rel'); }
  toString() { return this.href; }
}
reflectUrl(HTMLAnchorElement.prototype, 'href', 'href');
strs(HTMLAnchorElement.prototype, 'target rel download hreflang type ping referrerPolicy');
class HTMLAreaElement extends HTMLAnchorElement {}

class HTMLImageElement extends HTMLElement {
  get complete() { return true; }
  get naturalWidth() { return Math.round(n.rect(this)[2]); }
  get naturalHeight() { return Math.round(n.rect(this)[3]); }
  get currentSrc() { return this.src; }
  get width() { var v = parseInt(n.getAttr(this, 'width'), 10); return isFinite(v) ? v : Math.round(n.rect(this)[2]); }
  set width(v) { n.setAttr(this, 'width', String(v)); }
  get height() { var v = parseInt(n.getAttr(this, 'height'), 10); return isFinite(v) ? v : Math.round(n.rect(this)[3]); }
  set height(v) { n.setAttr(this, 'height', String(v)); }
  get x() { return Math.round(n.rect(this)[0]); }
  get y() { return Math.round(n.rect(this)[1]); }
}
reflectUrl(HTMLImageElement.prototype, 'src', 'src');
strs(HTMLImageElement.prototype, 'alt srcset sizes crossOrigin useMap loading decoding referrerPolicy name');
bools(HTMLImageElement.prototype, 'isMap');

class HTMLInputElement extends HTMLElement {
  get type() { var t = (n.getAttr(this, 'type') || 'text').toLowerCase(); return t; }
  set type(v) { n.setAttr(this, 'type', String(v)); }
  get value() {
    var t = this.type;
    if (t === 'checkbox' || t === 'radio') { var v = n.getAttr(this, 'value'); return v === null ? 'on' : v; }
    return n.value(this);
  }
  set value(v) {
    var t = this.type;
    v = v === null ? '' : String(v);
    if (t === 'checkbox' || t === 'radio' || t === 'hidden' || t === 'submit' || t === 'button' || t === 'reset' || t === 'image') n.setAttr(this, 'value', v);
    else n.setValue(this, v);
  }
  get defaultValue() { return n.getAttr(this, 'value') || ''; }
  set defaultValue(v) { n.setAttr(this, 'value', String(v)); }
  get valueAsNumber() { var v = parseFloat(this.value); return v; }
  set valueAsNumber(v) { this.value = String(v); }
  get files() { return null; }
  get list() { return null; }
  get selectionStart() { return this.value.length; }
  set selectionStart(v) {}
  get selectionEnd() { return this.value.length; }
  set selectionEnd(v) {}
  select() {}
  setSelectionRange() {}
  setRangeText(t) { this.value = String(t); }
  stepUp(k) { this.value = String((parseFloat(this.value) || 0) + (k === undefined ? 1 : k) * (parseFloat(n.getAttr(this, 'step')) || 1)); }
  stepDown(k) { this.stepUp(-(k === undefined ? 1 : k)); }
}
formControl(HTMLInputElement);
strs(HTMLInputElement.prototype, 'placeholder min max step pattern accept autocomplete alt inputMode dirName');
reflectUrl(HTMLInputElement.prototype, 'src', 'src');
bools(HTMLInputElement.prototype, 'checked readOnly multiple');
reflectBool(HTMLInputElement.prototype, 'defaultChecked', 'checked');
reflectNum(HTMLInputElement.prototype, 'maxLength', 'maxlength', -1);
reflectNum(HTMLInputElement.prototype, 'minLength', 'minlength', -1);
reflectNum(HTMLInputElement.prototype, 'size', 'size', 20);
hide(HTMLInputElement.prototype, 'indeterminate', false);

class HTMLTextAreaElement extends HTMLElement {
  get type() { return 'textarea'; }
  get value() { return n.value(this); }
  set value(v) { n.setValue(this, v === null ? '' : String(v)); }
  get defaultValue() { return n.text(this); }
  set defaultValue(v) { n.setText(this, String(v)); }
  get textLength() { return this.value.length; }
  get selectionStart() { return this.value.length; }
  set selectionStart(v) {}
  get selectionEnd() { return this.value.length; }
  set selectionEnd(v) {}
  select() {}
  setSelectionRange() {}
}
formControl(HTMLTextAreaElement);
strs(HTMLTextAreaElement.prototype, 'placeholder wrap autocomplete');
bools(HTMLTextAreaElement.prototype, 'readOnly');
reflectNum(HTMLTextAreaElement.prototype, 'rows', 'rows', 2);
reflectNum(HTMLTextAreaElement.prototype, 'cols', 'cols', 20);
reflectNum(HTMLTextAreaElement.prototype, 'maxLength', 'maxlength', -1);

class HTMLButtonElement extends HTMLElement {
  get type() { var t = (n.getAttr(this, 'type') || '').toLowerCase(); return t === 'button' || t === 'reset' ? t : 'submit'; }
  set type(v) { n.setAttr(this, 'type', String(v)); }
}
formControl(HTMLButtonElement);
strs(HTMLButtonElement.prototype, 'value formAction formMethod formTarget');

class HTMLOptionElement extends HTMLElement {
  get value() { var v = n.getAttr(this, 'value'); return v === null ? n.text(this).trim() : v; }
  set value(v) { n.setAttr(this, 'value', String(v)); }
  get text() { return n.text(this).trim(); }
  set text(v) { n.setText(this, String(v)); }
  get label() { var v = n.getAttr(this, 'label'); return v === null ? this.text : v; }
  set label(v) { n.setAttr(this, 'label', String(v)); }
  get index() { var s = n.query(this, 'select', 3); return s ? n.byTag(s, 'option').indexOf(this) : 0; }
  get form() { return formOf(this); }
}
bools(HTMLOptionElement.prototype, 'selected disabled');
reflectBool(HTMLOptionElement.prototype, 'defaultSelected', 'selected');

class HTMLSelectElement extends HTMLElement {
  get type() { return n.hasAttr(this, 'multiple') ? 'select-multiple' : 'select-one'; }
  get options() { return nodeList(n.byTag(this, 'option')); }
  get length() { return n.byTag(this, 'option').length; }
  get selectedOptions() { var o = n.byTag(this, 'option'), out = []; for (var i = 0; i < o.length; i++) if (o[i].selected) out.push(o[i]); return nodeList(out); }
  get selectedIndex() {
    var o = n.byTag(this, 'option');
    for (var i = 0; i < o.length; i++) if (n.hasAttr(o[i], 'selected')) return i;
    return o.length && !n.hasAttr(this, 'multiple') ? 0 : -1;
  }
  set selectedIndex(v) {
    var o = n.byTag(this, 'option');
    for (var i = 0; i < o.length; i++) { if (i === v) n.setAttr(o[i], 'selected', ''); else n.removeAttr(o[i], 'selected'); }
  }
  get value() { var o = n.byTag(this, 'option'), i = this.selectedIndex; return i >= 0 && o[i] ? o[i].value : ''; }
  set value(v) {
    var o = n.byTag(this, 'option'), hit = -1;
    v = String(v);
    for (var i = 0; i < o.length; i++) if (o[i].value === v) { hit = i; break; }
    this.selectedIndex = hit;
  }
  item(i) { return n.byTag(this, 'option')[i] || null; }
  add(opt, before) { n.insert(this, opt, before instanceof Node ? before : null); }
}
formControl(HTMLSelectElement);
bools(HTMLSelectElement.prototype, 'multiple');
reflectNum(HTMLSelectElement.prototype, 'size', 'size', 0);

class HTMLFormElement extends HTMLElement {
  get elements() { return namedList(controlsOf(this)); }
  get length() { return controlsOf(this).length; }
  submit() { n.submit(this, null); }
  requestSubmit(submitter) {
    var ev = new SubmitEvent('submit', { bubbles: true, cancelable: true, submitter: submitter || null });
    if (dispatch(this, ev)) n.submit(this, submitter || null);
  }
  reset() {
    var c = controlsOf(this);
    for (var i = 0; i < c.length; i++) {
      var t = n.tag(c[i]);
      if (t === 'input' || t === 'textarea') n.setValue(c[i], t === 'textarea' ? n.text(c[i]) : (n.getAttr(c[i], 'value') || ''));
    }
    trusted(this, 'reset');
  }
  checkValidity() { return true; }
  reportValidity() { return true; }
}
reflectUrl(HTMLFormElement.prototype, 'action', 'action');
strs(HTMLFormElement.prototype, 'method name target enctype encoding acceptCharset autocomplete');
bools(HTMLFormElement.prototype, 'noValidate');

class HTMLLabelElement extends HTMLElement {
  get control() {
    var id = n.getAttr(this, 'for');
    if (id) return n.byId(id);
    return n.query(this, 'input, select, textarea, button', 0);
  }
  get form() { return formOf(this); }
}
reflectStr(HTMLLabelElement.prototype, 'htmlFor', 'for');

class HTMLScriptElement extends HTMLElement {
  get text() { return n.text(this); }
  set text(v) { n.setText(this, String(v)); }
}
reflectUrl(HTMLScriptElement.prototype, 'src', 'src');
strs(HTMLScriptElement.prototype, 'type charset crossOrigin integrity referrerPolicy event htmlFor');
bools(HTMLScriptElement.prototype, 'async defer noModule');

class HTMLLinkElement extends HTMLElement {
  get relList() { return new DOMTokenList(this, 'rel'); }
  get sheet() { return null; }
}
reflectUrl(HTMLLinkElement.prototype, 'href', 'href');
strs(HTMLLinkElement.prototype, 'rel media type as crossOrigin integrity hreflang sizes referrerPolicy');
bools(HTMLLinkElement.prototype, 'disabled');

var SHEET = Symbol('sheet');
// Just enough CSSOM for libraries that add rules one by one: rules live in the element's text.
class CSSStyleSheet {
  constructor(owner) { hide(this, '_owner', owner); hide(this, '_rules', []); this.disabled = false; }
  get ownerNode() { return this._owner; }
  get cssRules() { return this._rules; }
  get rules() { return this._rules; }
  _flush() {
    var s = '';
    for (var i = 0; i < this._rules.length; i++) s += this._rules[i].cssText + '\n';
    n.setText(this._owner, s);
  }
  insertRule(text, index) {
    index = index === undefined ? 0 : index;
    if (index < 0 || index > this._rules.length) throw new DOMException('Index out of range', 'IndexSizeError');
    this._rules.splice(index, 0, { cssText: String(text), type: 1, style: {}, selectorText: '' });
    this._flush();
    return index;
  }
  deleteRule(index) {
    if (index < 0 || index >= this._rules.length) throw new DOMException('Index out of range', 'IndexSizeError');
    this._rules.splice(index, 1);
    this._flush();
  }
  addRule(sel, style, index) { return this.insertRule(sel + '{' + style + '}', index === undefined ? this._rules.length : index); }
  removeRule(index) { this.deleteRule(index === undefined ? 0 : index); }
  replaceSync(text) { this._rules.length = 0; this._rules.push({ cssText: String(text), type: 1 }); this._flush(); }
}
class HTMLStyleElement extends HTMLElement {
  get sheet() {
    if (!this[SHEET]) {
      var sh = new CSSStyleSheet(this), text = n.text(this);
      if (text) sh._rules.push({ cssText: text, type: 1 });
      hide(this, SHEET, sh);
    }
    return this[SHEET];
  }
}
strs(HTMLStyleElement.prototype, 'media type');
bools(HTMLStyleElement.prototype, 'disabled');

class HTMLMetaElement extends HTMLElement {}
strs(HTMLMetaElement.prototype, 'name content charset');
reflectStr(HTMLMetaElement.prototype, 'httpEquiv', 'http-equiv');
class HTMLBaseElement extends HTMLElement {}
reflectUrl(HTMLBaseElement.prototype, 'href', 'href');
class HTMLTitleElement extends HTMLElement {
  get text() { return n.text(this); }
  set text(v) { n.setText(this, String(v)); }
}
class HTMLIFrameElement extends HTMLElement {
  get contentWindow() { return null; }
  get contentDocument() { return null; }
}
reflectUrl(HTMLIFrameElement.prototype, 'src', 'src');
strs(HTMLIFrameElement.prototype, 'name width height allow loading srcdoc referrerPolicy');

class HTMLCanvasElement extends HTMLElement {
  getContext() { return null; }
  toDataURL() { return 'data:,'; }
  toBlob(cb) { if (typeof cb === 'function') g.setTimeout(function () { cb(null); }, 0); }
}
reflectNum(HTMLCanvasElement.prototype, 'width', 'width', 300);
reflectNum(HTMLCanvasElement.prototype, 'height', 'height', 150);

class HTMLMediaElement extends HTMLElement {
  get paused() { return true; }
  get ended() { return false; }
  get duration() { return NaN; }
  get readyState() { return 0; }
  get networkState() { return 0; }
  get error() { return null; }
  get buffered() { return { length: 0 }; }
  play() { return typeof Promise === 'function' ? Promise.reject(new DOMException('Media playback is not supported yet', 'NotSupportedError')) : undefined; }
  pause() {}
  load() {}
  canPlayType() { return ''; }
  addTextTrack() { return {}; }
}
reflectUrl(HTMLMediaElement.prototype, 'src', 'src');
strs(HTMLMediaElement.prototype, 'preload crossOrigin');
bools(HTMLMediaElement.prototype, 'controls autoplay loop muted');
hide(HTMLMediaElement.prototype, 'currentTime', 0);
hide(HTMLMediaElement.prototype, 'volume', 1);
hide(HTMLMediaElement.prototype, 'playbackRate', 1);
class HTMLVideoElement extends HTMLMediaElement {
  get videoWidth() { return 0; }
  get videoHeight() { return 0; }
}
reflectUrl(HTMLVideoElement.prototype, 'poster', 'poster');
reflectNum(HTMLVideoElement.prototype, 'width', 'width', 0);
reflectNum(HTMLVideoElement.prototype, 'height', 'height', 0);
class HTMLAudioElement extends HTMLMediaElement {}

var CONTENT = Symbol('content');
class HTMLTemplateElement extends HTMLElement {
  // the parser leaves template children in place; they move into the content fragment on first use
  get content() {
    if (!this[CONTENT]) {
      var f = n.createFragment(), c;
      while ((c = n.first(this))) n.insert(f, c, null);
      hide(this, CONTENT, f);
    }
    return this[CONTENT];
  }
  get innerHTML() { return n.html(this.content, false); }
  set innerHTML(v) {
    var f = this.content, c;
    while ((c = n.first(f))) n.remove(c);
    n.insert(f, n.parse(this, String(v)), null);
  }
}

class HTMLTableElement extends HTMLElement {
  get rows() { return nodeList(n.byTag(this, 'tr')); }
  get tBodies() { return nodeList(n.byTag(this, 'tbody')); }
  get tHead() { return n.query(this, 'thead', 0); }
  get tFoot() { return n.query(this, 'tfoot', 0); }
  insertRow(i) {
    var body = n.query(this, 'tbody', 0) || this, row = n.createElement('tr'), rows = n.children(body, true);
    n.insert(body, row, i === undefined || i < 0 || i >= rows.length ? null : rows[i]);
    return row;
  }
  deleteRow(i) { var r = n.byTag(this, 'tr'); if (r.length) n.remove(r[i < 0 ? r.length - 1 : i]); }
}
class HTMLTableSectionElement extends HTMLElement {
  get rows() { return nodeList(n.children(this, true)); }
}
class HTMLTableRowElement extends HTMLElement {
  get cells() { return nodeList(n.children(this, true)); }
  get rowIndex() { var t = n.query(this, 'table', 3); return t ? n.byTag(t, 'tr').indexOf(this) : -1; }
  insertCell(i) {
    var cell = n.createElement('td'), cells = n.children(this, true);
    n.insert(this, cell, i === undefined || i < 0 || i >= cells.length ? null : cells[i]);
    return cell;
  }
}
class HTMLTableCellElement extends HTMLElement {
  get cellIndex() { var p = n.parent(this); return p ? n.children(p, true).indexOf(this) : -1; }
}
reflectNum(HTMLTableCellElement.prototype, 'colSpan', 'colspan', 1);
reflectNum(HTMLTableCellElement.prototype, 'rowSpan', 'rowspan', 1);

class HTMLUnknownElement extends HTMLElement {}

var TAG_CLASSES = {
  a: HTMLAnchorElement, area: HTMLAreaElement, img: HTMLImageElement, input: HTMLInputElement,
  textarea: HTMLTextAreaElement, button: HTMLButtonElement, option: HTMLOptionElement, select: HTMLSelectElement,
  form: HTMLFormElement, label: HTMLLabelElement, script: HTMLScriptElement, link: HTMLLinkElement,
  style: HTMLStyleElement, meta: HTMLMetaElement, base: HTMLBaseElement, title: HTMLTitleElement,
  iframe: HTMLIFrameElement, canvas: HTMLCanvasElement, video: HTMLVideoElement, audio: HTMLAudioElement,
  template: HTMLTemplateElement, table: HTMLTableElement, thead: HTMLTableSectionElement,
  tbody: HTMLTableSectionElement, tfoot: HTMLTableSectionElement, tr: HTMLTableRowElement,
  td: HTMLTableCellElement, th: HTMLTableCellElement
};
// Plain element kinds that only need a distinct constructor for `instanceof` and `constructor.name`.
var SIMPLE = {
  HTMLDivElement: 'div', HTMLSpanElement: 'span', HTMLParagraphElement: 'p', HTMLBodyElement: 'body',
  HTMLHeadElement: 'head', HTMLHtmlElement: 'html', HTMLBRElement: 'br', HTMLHRElement: 'hr',
  HTMLUListElement: 'ul', HTMLOListElement: 'ol', HTMLLIElement: 'li', HTMLPreElement: 'pre',
  HTMLHeadingElement: 'h1 h2 h3 h4 h5 h6', HTMLQuoteElement: 'blockquote q', HTMLDListElement: 'dl',
  HTMLFieldSetElement: 'fieldset', HTMLLegendElement: 'legend', HTMLDetailsElement: 'details',
  HTMLDialogElement: 'dialog', HTMLPictureElement: 'picture', HTMLSourceElement: 'source',
  HTMLOptGroupElement: 'optgroup', HTMLTableCaptionElement: 'caption', HTMLTableColElement: 'col colgroup',
  HTMLEmbedElement: 'embed', HTMLObjectElement: 'object', HTMLProgressElement: 'progress',
  HTMLMeterElement: 'meter', HTMLOutputElement: 'output', HTMLDataElement: 'data', HTMLTimeElement: 'time',
  HTMLSlotElement: 'slot', HTMLMenuElement: 'menu', HTMLMapElement: 'map', HTMLTrackElement: 'track'
};
var HTML_CLASSES = {};
(function () {
  function simple(name) {
    var C = class extends HTMLElement {};
    defineProp(C, 'name', { value: name, configurable: true });
    return C;
  }
  for (var name in SIMPLE) {
    var C = simple(name), tags = SIMPLE[name].split(' ');
    HTML_CLASSES[name] = C;
    for (var i = 0; i < tags.length; i++) TAG_CLASSES[tags[i]] = C;
  }
  // inline and sectioning elements without an interface of their own are plain HTMLElements
  var plain = ('abbr address article aside b bdi bdo cite code dd dfn dt em figcaption figure footer header hgroup i kbd ' +
    'main mark nav noscript rp rt ruby s samp section small strong sub summary sup u var wbr center font big tt nobr strike').split(' ');
  for (var j = 0; j < plain.length; j++) TAG_CLASSES[plain[j]] = HTMLElement;
  for (var svg in SVG_TAGS) TAG_CLASSES[svg] = SVGElement;
  for (var tag in TAG_CLASSES) n.setProto(tag, TAG_CLASSES[tag].prototype);
  bools(HTML_CLASSES.HTMLDetailsElement.prototype, 'open');
  bools(HTML_CLASSES.HTMLDialogElement.prototype, 'open');
  methods(HTML_CLASSES.HTMLDialogElement.prototype, {
    show: function () { n.setAttr(this, 'open', ''); },
    showModal: function () { n.setAttr(this, 'open', ''); },
    close: function () { n.removeAttr(this, 'open'); trusted(this, 'close'); }
  });
  strs(HTML_CLASSES.HTMLSourceElement.prototype, 'srcset sizes media type');
  reflectUrl(HTML_CLASSES.HTMLSourceElement.prototype, 'src', 'src');
  n.setProto('#element', HTMLUnknownElement.prototype);
  n.setProto('#text', Text.prototype);
  n.setProto('#comment', Comment.prototype);
  n.setProto('#doctype', DocumentType.prototype);
  n.setProto('#fragment', DocumentFragment.prototype);
})();
