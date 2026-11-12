(function (n, g) {
'use strict';
// The DOM that page scripts see, built on the native primitives in `n`.
// This file and the ones after it are concatenated into one function body.

var defineProp = Object.defineProperty;
var L = Symbol('listeners');
var H = Symbol('handlers');
var hasOwn = function (o, k) { return Object.prototype.hasOwnProperty.call(o, k); };

function hide(obj, name, value) {
  defineProp(obj, name, { value: value, writable: true, configurable: true, enumerable: false });
}
function getter(obj, name, get, set) {
  defineProp(obj, name, { get: get, set: set, configurable: true, enumerable: true });
}
function methods(proto, table) {
  for (var k in table) if (hasOwn(table, k)) hide(proto, k, table[k]);
}

var DOM_CODES = {
  IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5,
  NoModificationAllowedError: 7, NotFoundError: 8, NotSupportedError: 9, InvalidStateError: 11,
  SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14, InvalidAccessError: 15,
  SecurityError: 18, NetworkError: 19, AbortError: 20, QuotaExceededError: 22, TimeoutError: 23,
  DataCloneError: 25
};
function DOMException(message, name) {
  var e = new Error(message === undefined ? '' : String(message));
  e.name = name === undefined ? 'Error' : String(name);
  e.code = DOM_CODES[e.name] || 0;
  Object.setPrototypeOf(e, DOMException.prototype);
  return e;
}
DOMException.prototype = Object.create(Error.prototype);
hide(DOMException.prototype, 'constructor', DOMException);

function describe(e) {
  try {
    if (e && typeof e === 'object' && (e.message !== undefined || e.name !== undefined)) {
      return (e.name || 'Error') + ': ' + e.message;
    }
    return String(e);
  } catch (x) {
    return 'unprintable error';
  }
}
// The innermost frames of an error's stack, for the console: where it was thrown.
function whereThrown(e) {
  try {
    if (!e || typeof e.stack !== 'string') return '';
    var lines = e.stack.split(String.fromCharCode(10)), out = [];
    for (var i = 0; i < lines.length && out.length < 3; i++) {
      var l = lines[i].trim();
      if (l.slice(0, 3) === 'at ') out.push(l.slice(3));
    }
    return out.length ? ' [at ' + out.join(' < ') + ']' : '';
  } catch (x) {
    return '';
  }
}
function reportError(e) {
  n.log(3, 'Uncaught ' + describe(e) + whereThrown(e));
  try {
    if (typeof g.onerror === 'function') g.onerror(describe(e), '', 0, 0, e);
  } catch (x) {}
}

// ───────────────────────────── events ─────────────────────────────

class Event {
  constructor(type, init) {
    init = init || {};
    this.type = String(type);
    this.bubbles = !!init.bubbles;
    this.cancelable = !!init.cancelable;
    this.composed = !!init.composed;
    this.target = null;
    this.currentTarget = null;
    this.eventPhase = 0;
    this.defaultPrevented = false;
    this.isTrusted = false;
    this.timeStamp = n.now();
    hide(this, '_stop', false);
    hide(this, '_stopNow', false);
    hide(this, '_passive', false);
    hide(this, '_path', null);
  }
  preventDefault() { if (this.cancelable && !this._passive) this.defaultPrevented = true; }
  stopPropagation() { this._stop = true; }
  stopImmediatePropagation() { this._stop = true; this._stopNow = true; }
  composedPath() { return this._path ? this._path.slice() : []; }
  initEvent(type, bubbles, cancelable) {
    this.type = String(type); this.bubbles = !!bubbles; this.cancelable = !!cancelable;
  }
  get srcElement() { return this.target; }
  get returnValue() { return !this.defaultPrevented; }
  set returnValue(v) { if (v === false) this.preventDefault(); }
  get cancelBubble() { return this._stop; }
  set cancelBubble(v) { if (v) this._stop = true; }
}
Event.NONE = 0; Event.CAPTURING_PHASE = 1; Event.AT_TARGET = 2; Event.BUBBLING_PHASE = 3;

function copyInit(ev, init, defaults) {
  for (var k in defaults) if (hasOwn(defaults, k)) ev[k] = init && init[k] !== undefined ? init[k] : defaults[k];
}

class CustomEvent extends Event {
  constructor(type, init) { super(type, init); this.detail = init && init.detail !== undefined ? init.detail : null; }
  initCustomEvent(type, bubbles, cancelable, detail) { this.initEvent(type, bubbles, cancelable); this.detail = detail; }
}
class UIEvent extends Event {
  constructor(type, init) { super(type, init); this.view = g; this.detail = (init && init.detail) || 0; }
}
var MODS = { ctrlKey: false, shiftKey: false, altKey: false, metaKey: false };
class MouseEvent extends UIEvent {
  constructor(type, init) {
    super(type, init);
    copyInit(this, init, MODS);
    copyInit(this, init, { clientX: 0, clientY: 0, screenX: 0, screenY: 0, button: 0, buttons: 0, relatedTarget: null, movementX: 0, movementY: 0 });
    var v = n.viewport();
    this.pageX = this.clientX + v[2]; this.pageY = this.clientY + v[3];
    this.x = this.clientX; this.y = this.clientY;
    this.offsetX = 0; this.offsetY = 0; this.which = this.button + 1;
  }
  getModifierState() { return false; }
}
class PointerEvent extends MouseEvent {
  constructor(type, init) {
    super(type, init);
    copyInit(this, init, { pointerId: 1, pointerType: 'mouse', isPrimary: true, width: 1, height: 1, pressure: 0 });
  }
}
class WheelEvent extends MouseEvent {
  constructor(type, init) { super(type, init); copyInit(this, init, { deltaX: 0, deltaY: 0, deltaZ: 0, deltaMode: 0 }); }
}
class KeyboardEvent extends UIEvent {
  constructor(type, init) {
    super(type, init);
    copyInit(this, init, MODS);
    copyInit(this, init, { key: '', code: '', keyCode: 0, which: 0, charCode: 0, repeat: false, location: 0, isComposing: false });
    if (!this.which) this.which = this.keyCode;
  }
  getModifierState() { return false; }
}
class FocusEvent extends UIEvent {
  constructor(type, init) { super(type, init); this.relatedTarget = (init && init.relatedTarget) || null; }
}
class InputEvent extends UIEvent {
  constructor(type, init) { super(type, init); copyInit(this, init, { data: null, inputType: '', isComposing: false }); }
}
class SubmitEvent extends Event {
  constructor(type, init) { super(type, init); this.submitter = (init && init.submitter) || null; }
}
class ErrorEvent extends Event {
  constructor(type, init) { super(type, init); copyInit(this, init, { message: '', filename: '', lineno: 0, colno: 0, error: null }); }
}
class HashChangeEvent extends Event {
  constructor(type, init) { super(type, init); copyInit(this, init, { oldURL: '', newURL: '' }); }
}
class PopStateEvent extends Event {
  constructor(type, init) { super(type, init); this.state = init && init.state !== undefined ? init.state : null; }
}
class ProgressEvent extends Event {
  constructor(type, init) { super(type, init); copyInit(this, init, { lengthComputable: false, loaded: 0, total: 0 }); }
}
class MessageEvent extends Event {
  constructor(type, init) { super(type, init); copyInit(this, init, { data: null, origin: '', lastEventId: '', source: null, ports: [] }); }
}
class TransitionEvent extends Event {
  constructor(type, init) { super(type, init); copyInit(this, init, { propertyName: '', elapsedTime: 0, pseudoElement: '' }); }
}
class AnimationEvent extends Event {
  constructor(type, init) { super(type, init); copyInit(this, init, { animationName: '', elapsedTime: 0, pseudoElement: '' }); }
}
class TouchEvent extends UIEvent {
  constructor(type, init) { super(type, init); copyInit(this, init, { touches: [], targetTouches: [], changedTouches: [] }); }
}

class EventTarget {
  addEventListener(type, cb, opts) {
    if (cb === null || cb === undefined) return;
    type = String(type);
    var isObj = opts !== null && typeof opts === 'object';
    var capture = isObj ? !!opts.capture : !!opts;
    var map = this[L];
    if (!map) { map = Object.create(null); hide(this, L, map); }
    var list = map[type] || (map[type] = []);
    for (var i = 0; i < list.length; i++) if (list[i].cb === cb && list[i].capture === capture) return;
    var entry = { cb: cb, capture: capture, once: isObj && !!opts.once, passive: isObj && !!opts.passive, removed: false };
    list.push(entry);
    if (isObj && opts.signal && typeof opts.signal.addEventListener === 'function') {
      var self = this;
      opts.signal.addEventListener('abort', function () { self.removeEventListener(type, cb, capture); });
    }
  }
  removeEventListener(type, cb, opts) {
    var map = this[L];
    var list = map && map[String(type)];
    if (!list) return;
    var capture = opts !== null && typeof opts === 'object' ? !!opts.capture : !!opts;
    for (var i = 0; i < list.length; i++) {
      if (list[i].cb === cb && list[i].capture === capture) { list[i].removed = true; list.splice(i, 1); return; }
    }
  }
  dispatchEvent(ev) {
    if (!ev || typeof ev.type !== 'string') throw new TypeError('dispatchEvent needs an Event');
    return dispatch(this, ev);
  }
}

function eventParent(t) {
  if (t === g) return null;
  if (t === document) return g;
  if (t instanceof Node) return n.parent(t);
  return null;
}

function invoke(target, ev, phase) {
  ev.currentTarget = target;
  ev.eventPhase = phase;
  var map = target[L];
  var list = map && map[ev.type];
  if (list && list.length) {
    var copy = list.slice();
    for (var i = 0; i < copy.length; i++) {
      var l = copy[i];
      if (l.removed) continue;
      if (phase === 1 && !l.capture) continue;
      if (phase === 3 && l.capture) continue;
      if (l.once) { l.removed = true; var at = list.indexOf(l); if (at >= 0) list.splice(at, 1); }
      ev._passive = l.passive;
      try {
        if (typeof l.cb === 'function') l.cb.call(target, ev);
        else if (l.cb && typeof l.cb.handleEvent === 'function') l.cb.handleEvent(ev);
      } catch (e) { reportError(e); }
      ev._passive = false;
      if (ev._stopNow) return;
    }
  }
  // window.onerror has its own signature and is called by the error reporter instead
  if (phase !== 1 && !(target === g && ev.type === 'error')) {
    var h = target['on' + ev.type];
    if (typeof h === 'function') {
      try { if (h.call(target, ev) === false) ev.preventDefault(); } catch (e) { reportError(e); }
    }
  }
}

function dispatch(target, ev) {
  ev.target = target;
  var path = [];
  // `load` on a document resource does not reach the window
  for (var p = eventParent(target); p; p = eventParent(p)) {
    if (p === g && ev.type === 'load') break;
    path.push(p);
  }
  ev._path = [target].concat(path);
  ev._stop = false; ev._stopNow = false;
  for (var i = path.length - 1; i >= 0 && !ev._stop; i--) invoke(path[i], ev, 1);
  if (!ev._stop) invoke(target, ev, 2);
  if (ev.bubbles) for (var j = 0; j < path.length && !ev._stop; j++) invoke(path[j], ev, 3);
  ev.currentTarget = null;
  ev.eventPhase = 0;
  return !ev.defaultPrevented;
}

var EVENT_CLASSES = {
  click: MouseEvent, dblclick: MouseEvent, mousedown: MouseEvent, mouseup: MouseEvent, mousemove: MouseEvent,
  mouseover: MouseEvent, mouseout: MouseEvent, mouseenter: MouseEvent, mouseleave: MouseEvent,
  contextmenu: MouseEvent, auxclick: MouseEvent, wheel: WheelEvent,
  pointerdown: PointerEvent, pointerup: PointerEvent, pointermove: PointerEvent, pointerover: PointerEvent,
  pointerout: PointerEvent, pointerenter: PointerEvent, pointerleave: PointerEvent,
  keydown: KeyboardEvent, keyup: KeyboardEvent, keypress: KeyboardEvent,
  focus: FocusEvent, blur: FocusEvent, focusin: FocusEvent, focusout: FocusEvent,
  input: InputEvent, beforeinput: InputEvent, submit: SubmitEvent,
  hashchange: HashChangeEvent, popstate: PopStateEvent
};
var NO_BUBBLE = { focus: 1, blur: 1, load: 1, unload: 1, mouseenter: 1, mouseleave: 1, pointerenter: 1, pointerleave: 1, scroll: 0, resize: 1, error: 1 };
var NO_CANCEL = { input: 1, change: 1, focus: 1, blur: 1, focusin: 1, focusout: 1, load: 1, scroll: 1, resize: 1, DOMContentLoaded: 1, mouseenter: 1, mouseleave: 1, hashchange: 1, popstate: 1 };

// Build and dispatch an event the browser itself raised (input, lifecycle). Returns false if
// a listener cancelled it.
function trusted(target, type, init) {
  init = init || {};
  if (init.bubbles === undefined) init.bubbles = !NO_BUBBLE[type];
  if (init.cancelable === undefined) init.cancelable = !NO_CANCEL[type];
  var C = EVENT_CLASSES[type] || Event;
  var ev = new C(type, init);
  ev.isTrusted = true;
  return dispatch(target, ev);
}
