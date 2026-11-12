
// ───────────────────────────── window ─────────────────────────────

// timers: callbacks live here, deadlines live in the engine
var timers = Object.create(null), nextTimer = 1;
function addTimer(cb, delay, repeat, args) {
  if (typeof cb === 'string') { try { cb = new Function(cb); } catch (e) { reportError(e); return 0; } }
  if (typeof cb !== 'function') return 0;
  var id = nextTimer++;
  timers[id] = { cb: cb, args: args, repeat: repeat };
  n.timer(id, +delay || 0, repeat);
  return id;
}
function clearTimer(id) {
  if (typeof id === 'number' && timers[id]) { delete timers[id]; n.clearTimer(id); }
}
function fireTimer(id) {
  var t = timers[id];
  if (!t) return;
  if (!t.repeat) delete timers[id];
  try { t.cb.apply(g, t.args); } catch (e) { reportError(e); }
}
function rest(args, from) { return Array.prototype.slice.call(args, from); }

function inspect(v, depth) {
  if (typeof v === 'string') return depth ? JSON.stringify(v) : v;
  if (v === null || v === undefined || typeof v === 'number' || typeof v === 'boolean') return String(v);
  if (typeof v === 'function') return 'function ' + (v.name || '') + '()';
  if (typeof v === 'symbol') return v.toString();
  try {
    if (v instanceof Error) return describe(v);
    if (v instanceof Node) return n.kind(v) === 1 ? '<' + n.tag(v) + '>' : v.nodeName;
    if (depth > 2) return Array.isArray(v) ? '[…]' : '{…}';
    var parts = [], i;
    if (Array.isArray(v)) {
      for (i = 0; i < v.length && i < 50; i++) parts.push(inspect(v[i], depth + 1));
      return '[' + parts.join(', ') + (v.length > 50 ? ', …' : '') + ']';
    }
    var keys = Object.keys(v);
    for (i = 0; i < keys.length && i < 30; i++) parts.push(keys[i] + ': ' + inspect(v[keys[i]], depth + 1));
    return '{' + parts.join(', ') + (keys.length > 30 ? ', …' : '') + '}';
  } catch (e) {
    return '[object]';
  }
}
function logger(level) {
  return function () {
    var parts = [];
    for (var i = 0; i < arguments.length; i++) parts.push(inspect(arguments[i], 0));
    n.log(level, parts.join(' '));
  };
}
var console = {
  log: logger(0), debug: logger(0), info: logger(1), warn: logger(2), error: logger(3), trace: logger(0),
  dir: logger(0), table: logger(0), group: logger(0), groupCollapsed: logger(0), groupEnd: function () {},
  time: function () {}, timeEnd: function () {}, timeLog: function () {}, count: function () {}, countReset: function () {},
  clear: function () {},
  assert: function (ok) { if (!ok) logger(3).apply(null, ['Assertion failed:'].concat(rest(arguments, 1))); }
};

// Session-only storage: nothing is written to disk.
class Storage {
  constructor() { hide(this, '_m', Object.create(null)); hide(this, '_k', []); }
  get length() { return this._k.length; }
  key(i) { return i >= 0 && i < this._k.length ? this._k[i] : null; }
  getItem(k) { k = String(k); return k in this._m ? this._m[k] : null; }
  setItem(k, v) { k = String(k); if (!(k in this._m)) this._k.push(k); this._m[k] = String(v); }
  removeItem(k) { k = String(k); if (k in this._m) { delete this._m[k]; this._k.splice(this._k.indexOf(k), 1); } }
  clear() { this._m = Object.create(null); this._k = []; }
}
function makeStorage() {
  var s = new Storage();
  if (typeof Proxy !== 'function') return s;
  return new Proxy(s, {
    get: function (t, k) { if (typeof k !== 'string' || k in t) { var v = t[k]; return typeof v === 'function' ? v.bind(t) : v; } var r = t.getItem(k); return r === null ? undefined : r; },
    set: function (t, k, v) { t.setItem(k, v); return true; },
    has: function (t, k) { return k in t || t.getItem(k) !== null; },
    deleteProperty: function (t, k) { t.removeItem(k); return true; },
    ownKeys: function (t) { return t._k.slice(); },
    getOwnPropertyDescriptor: function (t, k) { var r = typeof k === 'string' ? t.getItem(k) : null; return r === null ? undefined : { value: r, writable: true, enumerable: true, configurable: true }; }
  });
}

// The same answers for every user: nothing here is read from the machine.
var navigator = {
  userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) Gecko/20100101 Firefox/128.0',
  appCodeName: 'Mozilla', appName: 'Netscape', appVersion: '5.0 (Windows)', product: 'Gecko', productSub: '20100101',
  vendor: '', vendorSub: '', platform: 'Win32', oscpu: 'Windows NT 10.0; Win64; x64',
  language: 'en-US', languages: ['en-US', 'en'], onLine: true, cookieEnabled: true, doNotTrack: '1',
  globalPrivacyControl: true, hardwareConcurrency: 4, maxTouchPoints: 0, webdriver: false, pdfViewerEnabled: false,
  plugins: [], mimeTypes: [],
  javaEnabled: function () { return false; },
  sendBeacon: function () { return false; },
  vibrate: function () { return false; }
};

function scrollArgs(x, y) {
  if (x !== null && typeof x === 'object') return [x.left, x.top];
  return [x, y];
}
function mediaQueryList(query) {
  query = String(query);
  var mql = new EventTarget();
  getter(mql, 'matches', function () { return n.media(query); });
  mql.media = query;
  mql.onchange = null;
  mql.addListener = function (cb) { this.addEventListener('change', cb); };
  mql.removeListener = function (cb) { this.removeEventListener('change', cb); };
  return mql;
}

// Observers. Mutation and resize notifications are not delivered yet; intersection observers
// report every target as visible once, which is what lazy-loading code needs to proceed.
class MutationObserver {
  constructor(cb) { hide(this, '_cb', cb); }
  observe() {}
  disconnect() {}
  takeRecords() { return []; }
}
class IntersectionObserver {
  constructor(cb, opts) { hide(this, '_cb', cb); this.root = (opts && opts.root) || null; this.rootMargin = '0px'; this.thresholds = [0]; }
  observe(target) {
    var self = this;
    if (!(target instanceof Element)) throw new TypeError('IntersectionObserver.observe: the target is not an Element');
    g.setTimeout(function () {
      try {
        var r = target.getBoundingClientRect();
        self._cb([{ target: target, isIntersecting: true, intersectionRatio: 1, boundingClientRect: r, intersectionRect: r, rootBounds: null, time: n.now() }], self);
      } catch (e) { reportError(e); }
    }, 0);
  }
  unobserve() {}
  disconnect() {}
  takeRecords() { return []; }
}
class ResizeObserver {
  constructor(cb) { hide(this, '_cb', cb); }
  observe(target) {
    var self = this;
    if (!(target instanceof Element)) throw new TypeError('ResizeObserver.observe: the target is not an Element');
    g.setTimeout(function () {
      try {
        var r = target.getBoundingClientRect();
        var size = [{ inlineSize: r.width, blockSize: r.height }];
        self._cb([{ target: target, contentRect: r, borderBoxSize: size, contentBoxSize: size }], self);
      } catch (e) { reportError(e); }
    }, 0);
  }
  unobserve() {}
  disconnect() {}
}
class PerformanceObserver {
  constructor() {}
  observe() {}
  disconnect() {}
  takeRecords() { return []; }
}
hide(PerformanceObserver, 'supportedEntryTypes', []);

class AbortSignal extends EventTarget {
  constructor() { super(); this.aborted = false; this.reason = undefined; this.onabort = null; }
  throwIfAborted() { if (this.aborted) throw this.reason; }
}
class AbortController {
  constructor() { this.signal = new AbortSignal(); }
  abort(reason) {
    var s = this.signal;
    if (s.aborted) return;
    s.aborted = true;
    s.reason = reason === undefined ? new DOMException('The operation was aborted', 'AbortError') : reason;
    dispatch(s, new Event('abort'));
  }
}

// `CSS.supports`: answered from the list of properties the engine parses. Saying "no" to
// everything makes sites run heavy fallbacks (a custom-properties ponyfill, for one).
var CSS_UNSUPPORTED = ['backdrop-filter', 'filter', 'mask', 'mask-image', 'clip-path', 'mix-blend-mode', 'writing-mode',
  'columns', 'column-count', 'animation', 'transition', 'perspective', 'rotate', 'scale', 'zoom'];
function cssSupports(a, b) {
  var prop;
  if (b === undefined) {
    var t = String(a).trim();
    var low = t.toLowerCase();
    if (low.slice(0, 3) === 'not' || low.indexOf(') and ') >= 0 || low.indexOf(') or ') >= 0 || low.slice(0, 9) === 'selector(') return false;
    if (t[0] === '(' && t[t.length - 1] === ')') t = t.slice(1, t.length - 1);
    var colon = t.indexOf(':');
    if (colon < 1) return false;
    prop = t.slice(0, colon).trim();
    if (!t.slice(colon + 1).trim()) return false;
  } else {
    prop = String(a).trim();
    if (!String(b).trim()) return false;
  }
  if (prop.slice(0, 2) === '--') return true;
  prop = prop.toLowerCase();
  return CSS_PROPS.indexOf(prop) >= 0 && CSS_UNSUPPORTED.indexOf(prop) < 0;
}
function cssEscape(value) {
  var s = String(value), out = '';
  for (var i = 0; i < s.length; i++) {
    var c = s.charCodeAt(i), ch = s[i];
    if (c === 0) out += '\uFFFD';
    else if ((c >= 1 && c <= 31) || c === 127 || (c >= 48 && c <= 57 && (i === 0 || (i === 1 && s[0] === '-')))) out += '\\' + c.toString(16) + ' ';
    else if (c >= 128 || ch === '-' || ch === '_' || (c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122)) out += ch;
    else out += '\\' + ch;
  }
  return out;
}

var B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
function btoa(s) {
  s = String(s);
  var out = '', i, a, b, c;
  for (i = 0; i < s.length; i += 3) {
    a = s.charCodeAt(i); b = s.charCodeAt(i + 1); c = s.charCodeAt(i + 2);
    if (a > 255 || b > 255 || c > 255) throw new DOMException('The string contains characters outside of the Latin1 range', 'InvalidCharacterError');
    out += B64[a >> 2] + B64[((a & 3) << 4) | ((b || 0) >> 4)];
    out += i + 1 < s.length ? B64[((b & 15) << 2) | ((c || 0) >> 6)] : '=';
    out += i + 2 < s.length ? B64[c & 63] : '=';
  }
  return out;
}
function atob(s) {
  s = String(s);
  var out = '', bits = 0, acc = 0, i, v;
  for (i = 0; i < s.length; i++) {
    var ch = s[i];
    if (ch === '=' || ch === ' ' || ch === '\n' || ch === '\r' || ch === '\t') continue;
    v = B64.indexOf(ch);
    if (v < 0) throw new DOMException('The string is not correctly encoded', 'InvalidCharacterError');
    acc = (acc << 6) | v; bits += 6;
    if (bits >= 8) { bits -= 8; out += String.fromCharCode((acc >> bits) & 255); }
  }
  return out;
}

(function () {
  var ET = EventTarget.prototype;
  // The global object is a Window, which is an event target. Its event methods sit on
  // `Window.prototype` (libraries patch them there); unqualified calls have no `this`.
  methods(Window.prototype, {
    addEventListener: function (t, c, o) { return ET.addEventListener.call(this === undefined || this === null ? g : this, t, c, o); },
    removeEventListener: function (t, c, o) { return ET.removeEventListener.call(this === undefined || this === null ? g : this, t, c, o); },
    dispatchEvent: function (e) { return dispatch(this === undefined || this === null ? g : this, e); }
  });
  Object.setPrototypeOf(g, Window.prototype);

  var view = function (i) { return function () { return n.viewport()[i]; }; };
  getter(g, 'innerWidth', view(0)); getter(g, 'innerHeight', view(1));
  getter(g, 'outerWidth', view(0)); getter(g, 'outerHeight', view(1));
  getter(g, 'scrollX', view(2)); getter(g, 'scrollY', view(3));
  getter(g, 'pageXOffset', view(2)); getter(g, 'pageYOffset', view(3));
  getter(g, 'location', function () { return location; }, function (v) { n.navigate(String(v), false); });
  getter(g, 'document', function () { return document; });
  getter(g, 'origin', function () { return location.origin; });

  var screen = { colorDepth: 24, pixelDepth: 24, availLeft: 0, availTop: 0, orientation: { type: 'landscape-primary', angle: 0, addEventListener: function () {} } };
  getter(screen, 'width', view(0)); getter(screen, 'height', view(1));
  getter(screen, 'availWidth', view(0)); getter(screen, 'availHeight', view(1));

  var performance = {
    timeOrigin: 0,
    now: function () { return n.now(); },
    mark: function () {}, measure: function () {}, clearMarks: function () {}, clearMeasures: function () {},
    getEntries: function () { return []; }, getEntriesByName: function () { return []; }, getEntriesByType: function () { return []; },
    timing: { navigationStart: 0, domContentLoadedEventEnd: 0, loadEventEnd: 0 },
    navigation: { type: 0, redirectCount: 0 }
  };

  var globals = {
    window: g, self: g, top: g, parent: g, frames: g, frameElement: null, opener: null, closed: false, length: 0, name: '',
    devicePixelRatio: 1, screenX: 0, screenY: 0, screenLeft: 0, screenTop: 0, isSecureContext: true,
    history: history, navigator: navigator, clientInformation: navigator, screen: screen, performance: performance,
    console: console, localStorage: makeStorage(), sessionStorage: makeStorage(), customElements: customElements,
    setTimeout: function (cb, delay) { return addTimer(cb, delay, false, rest(arguments, 2)); },
    setInterval: function (cb, delay) { return addTimer(cb, delay, true, rest(arguments, 2)); },
    clearTimeout: clearTimer, clearInterval: clearTimer,
    requestAnimationFrame: function (cb) { return addTimer(function () { cb(n.now()); }, 16, false, []); },
    cancelAnimationFrame: clearTimer,
    requestIdleCallback: function (cb) { return addTimer(function () { cb({ didTimeout: false, timeRemaining: function () { return 10; } }); }, 1, false, []); },
    cancelIdleCallback: clearTimer,
    getComputedStyle: function (el) { return new ComputedStyle(el); },
    matchMedia: mediaQueryList,
    scrollTo: function (x, y) { var a = scrollArgs(x, y), v = n.viewport(); n.scrollTo(a[0] === undefined ? v[2] : +a[0], a[1] === undefined ? v[3] : +a[1]); },
    scrollBy: function (x, y) { var a = scrollArgs(x, y), v = n.viewport(); n.scrollTo(v[2] + (+a[0] || 0), v[3] + (+a[1] || 0)); },
    alert: function (m) { n.log(1, 'alert: ' + String(m)); },
    confirm: function () { return false; },
    prompt: function () { return null; },
    print: function () {}, stop: function () {}, focus: function () {}, blur: function () {}, close: function () {},
    open: function () { return null; },
    postMessage: function (data) { g.setTimeout(function () { dispatch(g, new MessageEvent('message', { data: data, origin: location.origin, source: g })); }, 0); },
    getSelection: function () { return SELECTION; },
    fetch: fetch,
    reportError: reportError,
    btoa: btoa, atob: atob,
    CSS: { supports: cssSupports, escape: cssEscape },
    crypto: crypto,
    Image: function (w, h) { var img = n.createElement('img'); if (w !== undefined) n.setAttr(img, 'width', String(w)); if (h !== undefined) n.setAttr(img, 'height', String(h)); return img; },
    Option: function (text, value) { var o = n.createElement('option'); if (text !== undefined) n.setText(o, String(text)); if (value !== undefined) n.setAttr(o, 'value', String(value)); return o; },
    Audio: function (src) { var a = n.createElement('audio'); if (src !== undefined) n.setAttr(a, 'src', String(src)); return a; }
  };
  globals.scroll = globals.scrollTo;
  if (typeof g.queueMicrotask !== 'function') globals.queueMicrotask = function (cb) { addTimer(cb, 0, false, []); };

  var classes = {
    Event: Event, CustomEvent: CustomEvent, UIEvent: UIEvent, MouseEvent: MouseEvent, PointerEvent: PointerEvent,
    WheelEvent: WheelEvent, KeyboardEvent: KeyboardEvent, FocusEvent: FocusEvent, InputEvent: InputEvent,
    SubmitEvent: SubmitEvent, ErrorEvent: ErrorEvent, HashChangeEvent: HashChangeEvent, PopStateEvent: PopStateEvent,
    ProgressEvent: ProgressEvent, MessageEvent: MessageEvent, TransitionEvent: TransitionEvent,
    AnimationEvent: AnimationEvent, TouchEvent: TouchEvent, EventTarget: EventTarget, DOMException: DOMException,
    Node: Node, CharacterData: CharacterData, Text: Text, Comment: Comment, DocumentType: DocumentType,
    DocumentFragment: DocumentFragment, ShadowRoot: ShadowRoot, Document: Document, HTMLDocument: Document, Element: Element,
    HTMLElement: HTMLElement, SVGElement: SVGElement, HTMLUnknownElement: HTMLUnknownElement,
    HTMLAnchorElement: HTMLAnchorElement, HTMLAreaElement: HTMLAreaElement, HTMLImageElement: HTMLImageElement,
    HTMLInputElement: HTMLInputElement, HTMLTextAreaElement: HTMLTextAreaElement, HTMLButtonElement: HTMLButtonElement,
    HTMLOptionElement: HTMLOptionElement, HTMLSelectElement: HTMLSelectElement, HTMLFormElement: HTMLFormElement,
    HTMLLabelElement: HTMLLabelElement, HTMLScriptElement: HTMLScriptElement, HTMLLinkElement: HTMLLinkElement,
    HTMLStyleElement: HTMLStyleElement, HTMLMetaElement: HTMLMetaElement, HTMLBaseElement: HTMLBaseElement,
    HTMLTitleElement: HTMLTitleElement, HTMLIFrameElement: HTMLIFrameElement, HTMLCanvasElement: HTMLCanvasElement,
    HTMLMediaElement: HTMLMediaElement, HTMLVideoElement: HTMLVideoElement, HTMLAudioElement: HTMLAudioElement,
    HTMLTemplateElement: HTMLTemplateElement, HTMLTableElement: HTMLTableElement,
    HTMLTableSectionElement: HTMLTableSectionElement, HTMLTableRowElement: HTMLTableRowElement,
    HTMLTableCellElement: HTMLTableCellElement, DOMTokenList: DOMTokenList, DOMRect: DOMRect, DOMRectReadOnly: DOMRect,
    CSSStyleDeclaration: CSSStyleDeclaration, CSSStyleSheet: CSSStyleSheet, Storage: Storage, URL: URL,
    URLSearchParams: URLSearchParams, Range: Range, TreeWalker: TreeWalker, NodeFilter: NodeFilter,
    MutationObserver: MutationObserver, IntersectionObserver: IntersectionObserver, ResizeObserver: ResizeObserver,
    PerformanceObserver: PerformanceObserver, AbortController: AbortController, AbortSignal: AbortSignal,
    XMLHttpRequest: XMLHttpRequest, Headers: Headers, Request: Request, Response: Response, FormData: FormData,
    DOMParser: DOMParser, XMLSerializer: XMLSerializer, MessageChannel: MessageChannel, MessagePort: MessagePort,
    CDATASection: CDATASection, ProcessingInstruction: ProcessingInstruction, Window: Window,
    Attr: Attr, NamedNodeMap: NamedNodeMap, Navigator: Navigator, Location: Location, History: History,
    Screen: Screen, DOMImplementation: DOMImplementation, Selection: Selection, MediaQueryList: MediaQueryList,
    CSSRule: CSSRule, CSSStyleRule: CSSStyleRule, StyleSheet: StyleSheet, HTMLSlotElement: HTML_CLASSES.HTMLSlotElement,
    CustomElementRegistry: CustomElementRegistry, Blob: Blob, File: File, FileReader: FileReader,
    ReadableStream: ReadableStream, ReadableStreamDefaultReader: ReadableStreamDefaultReader,
    ReadableStreamDefaultController: ReadableStreamDefaultController, WritableStream: WritableStream,
    WritableStreamDefaultWriter: WritableStreamDefaultWriter, TransformStream: TransformStream,
    CountQueuingStrategy: CountQueuingStrategy, ByteLengthQueuingStrategy: ByteLengthQueuingStrategy,
    IDBFactory: IDBFactory, IDBRequest: IDBRequest, IDBOpenDBRequest: IDBOpenDBRequest, IDBDatabase: IDBDatabase,
    IDBObjectStore: IDBObjectStore, IDBIndex: IDBIndex, IDBCursor: IDBCursor, IDBCursorWithValue: IDBCursorWithValue,
    IDBTransaction: IDBTransaction, IDBKeyRange: IDBKeyRange, IDBVersionChangeEvent: IDBVersionChangeEvent
  };
  if (HAS_BYTES) { classes.TextEncoder = TextEncoder; classes.TextDecoder = TextDecoder; }
  globals.indexedDB = new IDBFactory();
  // `Object.prototype.toString.call(x)` has to name the interface: libraries tell DOM objects from
  // plain ones this way (Vue wraps whatever answers "[object Object]" in a Proxy, and a Proxy is
  // not a node any more when it reaches a native method).
  var tag = function (name, C) {
    if (typeof C !== 'function' || !C.prototype || Object.prototype.hasOwnProperty.call(C.prototype, Symbol.toStringTag)) return;
    Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  };
  var k;
  for (k in classes) tag(k, classes[k]);
  for (k in HTML_CLASSES) tag(k, HTML_CLASSES[k]);
  for (k in globals) hide(g, k, globals[k]);
  for (k in classes) hide(g, k, classes[k]);
  for (k in HTML_CLASSES) hide(g, k, HTML_CLASSES[k]);
  hide(g, 'NodeList', Array);
  hide(g, 'HTMLCollection', Array);
})();

// ───────────────────────────── entry points for the engine ─────────────────────────────

return {
  // an input event raised by the user; returns false if a listener cancelled it
  event: function (target, type, init) { return trusted(target === null ? g : target, type, init); },
  timer: fireTimer,
  // a script request finished: (id, ok, status, url, [name, value, ...], text)
  fetched: fetchDone,
  focus: function (prev, next) {
    if (prev) { trusted(prev, 'blur', { relatedTarget: next }); trusted(prev, 'focusout', { relatedTarget: next }); }
    if (next) { trusted(next, 'focus', { relatedTarget: prev }); trusted(next, 'focusin', { relatedTarget: prev }); }
  },
  // document lifecycle: 'interactive' | 'complete'
  ready: function (state) {
    readyState = state;
    trusted(document, 'readystatechange', { bubbles: false, cancelable: false });
    if (state === 'interactive') {
      trusted(document, 'DOMContentLoaded', { bubbles: true, cancelable: false });
    } else {
      var b = n.special(2);
      if (b && typeof g.onload !== 'function' && n.getAttr(b, 'onload') !== null) g.onload = b.onload;
      trusted(g, 'load', { bubbles: false, cancelable: false });
      trusted(g, 'pageshow', { bubbles: false, cancelable: false });
    }
  },
  // a resource element finished loading (or failed)
  loaded: function (node, ok) { trusted(node, ok ? 'load' : 'error', { bubbles: false, cancelable: false }); },
  uncaught: function (message) {
    try { if (typeof g.onerror === 'function') g.onerror(message, '', 0, 0, null); } catch (e) {}
    try { trusted(g, 'error', { message: message, bubbles: false, cancelable: true }); } catch (e) {}
  }
};
})
