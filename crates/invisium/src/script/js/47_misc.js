
// ───────────────────────────── text encoding, randomness, message channels ─────────────────────────────

var HAS_BYTES = typeof Uint8Array === 'function';

class TextEncoder {
  get encoding() { return 'utf-8'; }
  encode(input) {
    var s = input === undefined ? '' : String(input), out = [], i, c;
    for (i = 0; i < s.length; i++) {
      c = s.charCodeAt(i);
      if (c >= 0xD800 && c <= 0xDBFF && i + 1 < s.length) {
        var d = s.charCodeAt(i + 1);
        if (d >= 0xDC00 && d <= 0xDFFF) { c = 0x10000 + ((c - 0xD800) << 10) + (d - 0xDC00); i++; }
      }
      if (c >= 0xD800 && c <= 0xDFFF) c = 0xFFFD; // a lone surrogate
      if (c < 0x80) out.push(c);
      else if (c < 0x800) out.push(0xC0 | (c >> 6), 0x80 | (c & 63));
      else if (c < 0x10000) out.push(0xE0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
      else out.push(0xF0 | (c >> 18), 0x80 | ((c >> 12) & 63), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
    }
    return new Uint8Array(out);
  }
}
function bytesOf(v) {
  if (v === undefined || v === null) return new Uint8Array(0);
  if (v instanceof ArrayBuffer) return new Uint8Array(v);
  if (v.buffer instanceof ArrayBuffer) return new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
  throw new TypeError('expected an ArrayBuffer or a view on one');
}
class TextDecoder {
  constructor(label) {
    var l = label === undefined ? 'utf-8' : String(label).toLowerCase();
    if (l !== 'utf-8' && l !== 'utf8' && l !== 'unicode-1-1-utf-8') throw new RangeError("The encoding '" + label + "' is not supported");
  }
  get encoding() { return 'utf-8'; }
  decode(input) {
    var b = bytesOf(input), out = '', i = 0, n1 = b.length, c, need, min;
    if (n1 >= 3 && b[0] === 0xEF && b[1] === 0xBB && b[2] === 0xBF) i = 3;
    while (i < n1) {
      c = b[i++];
      if (c < 0x80) { out += String.fromCharCode(c); continue; }
      if (c >= 0xC2 && c < 0xE0) { need = 1; c &= 31; min = 0x80; }
      else if (c >= 0xE0 && c < 0xF0) { need = 2; c &= 15; min = 0x800; }
      else if (c >= 0xF0 && c < 0xF5) { need = 3; c &= 7; min = 0x10000; }
      else { out += '�'; continue; }
      var ok = i + need <= n1, j;
      for (j = 0; ok && j < need; j++) { var t = b[i + j]; if ((t & 0xC0) !== 0x80) ok = false; else c = (c << 6) | (t & 63); }
      if (!ok || c < min || c > 0x10FFFF || (c >= 0xD800 && c <= 0xDFFF)) { out += '�'; continue; }
      i += need;
      if (c > 0xFFFF) { c -= 0x10000; out += String.fromCharCode(0xD800 + (c >> 10), 0xDC00 + (c & 0x3FF)); }
      else out += String.fromCharCode(c);
    }
    return out;
  }
}

var crypto = {
  getRandomValues: function (target) {
    if (!HAS_BYTES || !target || !(target.buffer instanceof ArrayBuffer)) throw new TypeError('getRandomValues needs an integer typed array');
    if (target.byteLength > 65536) throw new DOMException('At most 65536 bytes may be requested', 'QuotaExceededError');
    new Uint8Array(target.buffer, target.byteOffset, target.byteLength).set(new Uint8Array(n.random(target.byteLength)));
    return target;
  },
  randomUUID: function () {
    var b = crypto.getRandomValues(new Uint8Array(16)), h = '0123456789abcdef', s = '';
    b[6] = (b[6] & 15) | 64; b[8] = (b[8] & 63) | 128;
    for (var i = 0; i < 16; i++) { if (i === 4 || i === 6 || i === 8 || i === 10) s += '-'; s += h[b[i] >> 4] + h[b[i] & 15]; }
    return s;
  }
};

class MessagePort extends EventTarget {
  constructor() { super(); this.onmessage = null; this.onmessageerror = null; hide(this, '_other', null); hide(this, '_closed', false); }
  postMessage(data) {
    var other = this._other;
    if (!other || this._closed) return;
    g.setTimeout(function () { if (!other._closed) dispatch(other, new MessageEvent('message', { data: data })); }, 0);
  }
  start() {}
  close() { this._closed = true; }
}
class MessageChannel {
  constructor() {
    this.port1 = new MessagePort(); this.port2 = new MessagePort();
    this.port1._other = this.port2; this.port2._other = this.port1;
  }
}

// legacy event initialisers still used by older libraries
methods(UIEvent.prototype, {
  initUIEvent: function (type, bubbles, cancelable, view, detail) { this.initEvent(type, bubbles, cancelable); this.detail = detail || 0; }
});
methods(MouseEvent.prototype, {
  initMouseEvent: function (type, bubbles, cancelable, view, detail, sx, sy, cx, cy, ctrl, alt, shift, meta, button, related) {
    this.initEvent(type, bubbles, cancelable);
    this.detail = detail || 0; this.screenX = sx || 0; this.screenY = sy || 0; this.clientX = cx || 0; this.clientY = cy || 0;
    this.ctrlKey = !!ctrl; this.altKey = !!alt; this.shiftKey = !!shift; this.metaKey = !!meta;
    this.button = button || 0; this.relatedTarget = related || null;
  }
});
methods(KeyboardEvent.prototype, {
  initKeyboardEvent: function (type, bubbles, cancelable, view, key) { this.initEvent(type, bubbles, cancelable); this.key = key === undefined ? '' : String(key); }
});

// Stopgaps for built-ins the JavaScript engine does not have yet. Each is replaced by the
// engine's own implementation as soon as that exists (they only install when missing).
(function () {
  if (typeof String.fromCodePoint !== 'function') hide(String, 'fromCodePoint', function () {
    var s = '';
    for (var i = 0; i < arguments.length; i++) {
      var c = +arguments[i];
      if (!(c >= 0 && c <= 0x10FFFF) || Math.floor(c) !== c) throw new RangeError('Invalid code point ' + arguments[i]);
      if (c > 0xFFFF) { c -= 0x10000; s += String.fromCharCode(0xD800 + (c >> 10), 0xDC00 + (c & 0x3FF)); }
      else s += String.fromCharCode(c);
    }
    return s;
  });
  // no Unicode normalisation tables yet: text is returned as it is
  if (typeof String.prototype.normalize !== 'function') hide(String.prototype, 'normalize', function () { return String(this); });
  if (typeof Error.captureStackTrace !== 'function') hide(Error, 'captureStackTrace', function () {});
  if (typeof g.escape !== 'function') hide(g, 'escape', function (s) {
    s = String(s);
    var keep = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789@*_+-./', h = '0123456789ABCDEF', out = '';
    for (var i = 0; i < s.length; i++) {
      var c = s.charCodeAt(i);
      if (keep.indexOf(s[i]) >= 0) out += s[i];
      else if (c < 256) out += '%' + h[c >> 4] + h[c & 15];
      else out += '%u' + h[c >> 12] + h[(c >> 8) & 15] + h[(c >> 4) & 15] + h[c & 15];
    }
    return out;
  });
  if (typeof g.unescape !== 'function') hide(g, 'unescape', function (s) {
    s = String(s);
    var out = '', i = 0;
    while (i < s.length) {
      if (s[i] === '%') {
        var u = s[i + 1] === 'u', hex = u ? s.slice(i + 2, i + 6) : s.slice(i + 1, i + 3), v = parseInt(hex, 16);
        if (hex.length === (u ? 4 : 2) && isFinite(v) && v >= 0) { out += String.fromCharCode(v); i += u ? 6 : 3; continue; }
      }
      out += s[i++];
    }
    return out;
  });
})();

// Interfaces that scripts refer to by name (for `instanceof` or to patch their prototypes).
// The objects this engine hands out for them are plain ones, so these carry no behaviour.
class Window extends EventTarget {}
class Attr {}
class NamedNodeMap {}
class Navigator {}
class Location {}
class History {}
class Screen {}
class DOMImplementation {}
class Selection {}
class MediaQueryList extends EventTarget {}
class CSSRule {}
class CSSStyleRule extends CSSRule {}
class StyleSheet {}
