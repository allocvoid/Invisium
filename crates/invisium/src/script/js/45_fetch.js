
// ───────────────────────────── network requests from script ─────────────────────────────
// The engine only carries these requests; the network stack enforces the cross-origin rules.
// A failure is always reported to script as the same opaque error, whatever its cause.

var pendingFetches = Object.create(null);
function startFetch(method, url, headers, body, sameOrigin, credentials, done) {
  var flat = [];
  headers.forEach(function (v, k) { flat.push(k, v); });
  var id = n.fetch(method, url, flat, body, sameOrigin, credentials);
  pendingFetches[id] = done;
  return id;
}
// The body arrives as an ArrayBuffer with the charset label of its Content-Type (or null).
function fetchDone(id, ok, status, url, flat, buf, charset) {
  var cb = pendingFetches[id];
  if (!cb) return;
  delete pendingFetches[id];
  try { cb(ok, status, url, flat, buf, charset); } catch (e) { reportError(e); }
}
function cancelFetch(id) {
  if (pendingFetches[id]) { delete pendingFetches[id]; n.abortFetch(id); }
}

var STATUS_TEXT = {
  200: 'OK', 201: 'Created', 202: 'Accepted', 204: 'No Content', 206: 'Partial Content', 301: 'Moved Permanently',
  302: 'Found', 303: 'See Other', 304: 'Not Modified', 307: 'Temporary Redirect', 308: 'Permanent Redirect',
  400: 'Bad Request', 401: 'Unauthorized', 403: 'Forbidden', 404: 'Not Found', 405: 'Method Not Allowed',
  409: 'Conflict', 410: 'Gone', 413: 'Payload Too Large', 415: 'Unsupported Media Type', 422: 'Unprocessable Entity',
  429: 'Too Many Requests', 500: 'Internal Server Error', 501: 'Not Implemented', 502: 'Bad Gateway',
  503: 'Service Unavailable', 504: 'Gateway Timeout'
};
var METHODS = ['GET', 'POST', 'HEAD', 'PUT', 'DELETE', 'PATCH', 'OPTIONS'];
function normMethod(m) {
  var u = String(m).toUpperCase();
  if (METHODS.indexOf(u) < 0) throw new TypeError("'" + m + "' is not a supported HTTP method");
  return u;
}

class Headers {
  constructor(init) {
    hide(this, '_h', []);
    if (init === undefined || init === null) return;
    var self = this;
    if (init instanceof Headers) init.forEach(function (v, k) { self.append(k, v); });
    else if (Array.isArray(init)) { for (var i = 0; i < init.length; i++) this.append(init[i][0], init[i][1]); }
    else { for (var k in init) if (hasOwn(init, k)) this.append(k, init[k]); }
  }
  append(k, v) { this._h.push([String(k).toLowerCase(), String(v).trim()]); }
  set(k, v) { this.delete(k); this.append(k, v); }
  delete(k) { k = String(k).toLowerCase(); this._h = this._h.filter(function (p) { return p[0] !== k; }); }
  get(k) {
    k = String(k).toLowerCase();
    var out = null;
    for (var i = 0; i < this._h.length; i++) if (this._h[i][0] === k) out = out === null ? this._h[i][1] : out + ', ' + this._h[i][1];
    return out;
  }
  has(k) { return this.get(k) !== null; }
  getSetCookie() { return []; }
  _pairs() {
    var names = [], i;
    for (i = 0; i < this._h.length; i++) if (names.indexOf(this._h[i][0]) < 0) names.push(this._h[i][0]);
    names.sort();
    var out = [];
    for (i = 0; i < names.length; i++) out.push([names[i], this.get(names[i])]);
    return out;
  }
  forEach(cb, self) { var p = this._pairs(); for (var i = 0; i < p.length; i++) cb.call(self, p[i][1], p[i][0], this); }
  keys() { return this._pairs().map(function (p) { return p[0]; })[Symbol.iterator](); }
  values() { return this._pairs().map(function (p) { return p[1]; })[Symbol.iterator](); }
  entries() { return this._pairs()[Symbol.iterator](); }
  [Symbol.iterator]() { return this.entries(); }
}
function headersFromFlat(flat) {
  var h = new Headers();
  for (var i = 0; i + 1 < flat.length; i += 2) h.append(flat[i], flat[i + 1]);
  return h;
}

class FormData {
  constructor(form) {
    hide(this, '_e', []);
    if (!(form instanceof Node)) return;
    var c = controlsOf(form);
    for (var i = 0; i < c.length; i++) {
      var el = c[i], name = n.getAttr(el, 'name'), tag = n.tag(el), type = (n.getAttr(el, 'type') || '').toLowerCase();
      if (!name || n.hasAttr(el, 'disabled')) continue;
      if (tag === 'input') {
        if (type === 'checkbox' || type === 'radio') { if (n.hasAttr(el, 'checked')) this.append(name, el.value); }
        else if (type !== 'submit' && type !== 'button' && type !== 'reset' && type !== 'image' && type !== 'file') this.append(name, el.value);
      } else if (tag === 'select' || tag === 'textarea') this.append(name, el.value);
    }
  }
  append(k, v) { this._e.push([String(k), String(v)]); }
  set(k, v) { this.delete(k); this.append(k, v); }
  delete(k) { k = String(k); this._e = this._e.filter(function (p) { return p[0] !== k; }); }
  get(k) { k = String(k); for (var i = 0; i < this._e.length; i++) if (this._e[i][0] === k) return this._e[i][1]; return null; }
  getAll(k) { k = String(k); var out = []; for (var i = 0; i < this._e.length; i++) if (this._e[i][0] === k) out.push(this._e[i][1]); return out; }
  has(k) { return this.get(k) !== null; }
  forEach(cb, self) { for (var i = 0; i < this._e.length; i++) cb.call(self, this._e[i][1], this._e[i][0], this); }
  keys() { return this._e.map(function (p) { return p[0]; })[Symbol.iterator](); }
  values() { return this._e.map(function (p) { return p[1]; })[Symbol.iterator](); }
  entries() { return this._e.map(function (p) { return [p[0], p[1]]; })[Symbol.iterator](); }
  [Symbol.iterator]() { return this.entries(); }
}

// Binary data. A Blob owns one ArrayBuffer that nothing else can reach, so it is immutable.
function blobPart(p) {
  if (typeof p === 'string') return new Uint8Array(n.encode(p));
  if (p instanceof Blob) return new Uint8Array(p._buf);
  if (p instanceof ArrayBuffer) return new Uint8Array(p);
  if (ArrayBuffer.isView(p)) return new Uint8Array(p.buffer, p.byteOffset, p.byteLength);
  return new Uint8Array(n.encode(String(p)));
}
class Blob {
  constructor(parts, opts) {
    var chunks = [], total = 0, i;
    if (parts !== undefined && parts !== null) {
      if (typeof parts !== 'object' || typeof parts[Symbol.iterator] !== 'function') throw new TypeError("Failed to construct 'Blob': the parts are not a sequence");
      for (var p of parts) { var c = blobPart(p); chunks.push(c); total += c.byteLength; }
    }
    var out = new Uint8Array(total), at = 0;
    for (i = 0; i < chunks.length; i++) { out.set(chunks[i], at); at += chunks[i].byteLength; }
    hide(this, '_buf', out.buffer);
    var type = opts && opts.type !== undefined ? String(opts.type) : '';
    this.type = /[^\x20-\x7e]/.test(type) ? '' : type.toLowerCase();
  }
  get size() { return this._buf.byteLength; }
  slice(start, end, type) {
    var len = this._buf.byteLength;
    return new Blob([this._buf.slice(start === undefined ? 0 : start, end === undefined ? len : end)], { type: type });
  }
  text() { return Promise.resolve(n.decode(this._buf, 'utf-8')); }
  arrayBuffer() { return Promise.resolve(this._buf.slice(0)); }
  bytes() { return Promise.resolve(new Uint8Array(this._buf.slice(0))); }
  stream() { var buf = this._buf; return bytesStream(function () { return new Uint8Array(buf.slice(0)); }); }
}
class File extends Blob {
  constructor(parts, name, opts) {
    super(parts, opts);
    this.name = String(name);
    this.lastModified = opts && opts.lastModified !== undefined ? Number(opts.lastModified) : Date.now();
    this.webkitRelativePath = '';
  }
}
function binaryString(buf) {
  var u = new Uint8Array(buf), s = '', i;
  for (i = 0; i < u.length; i += 4096) s += String.fromCharCode.apply(null, u.subarray(i, i + 4096));
  return s;
}
class FileReader extends EventTarget {
  constructor() {
    super();
    this.readyState = 0; this.result = null; this.error = null;
    this.onloadstart = null; this.onprogress = null; this.onload = null; this.onabort = null; this.onerror = null; this.onloadend = null;
  }
  _read(blob, make) {
    if (!(blob instanceof Blob)) throw new TypeError('FileReader: the argument is not a Blob');
    if (this.readyState === 1) throw new DOMException('The reader is busy', 'InvalidStateError');
    var self = this;
    this.readyState = 1; this.result = null;
    g.setTimeout(function () {
      if (self.readyState !== 1) return;
      self.result = make(blob._buf, blob.type);
      self.readyState = 2;
      var done = { lengthComputable: true, loaded: blob.size, total: blob.size };
      dispatch(self, new ProgressEvent('load', done));
      dispatch(self, new ProgressEvent('loadend', done));
    }, 0);
  }
  readAsText(blob) { this._read(blob, function (buf) { return n.decode(buf, 'utf-8'); }); }
  readAsArrayBuffer(blob) { this._read(blob, function (buf) { return buf.slice(0); }); }
  readAsBinaryString(blob) { this._read(blob, binaryString); }
  readAsDataURL(blob) {
    this._read(blob, function (buf, type) { return 'data:' + (type || 'application/octet-stream') + ';base64,' + btoa(binaryString(buf)); });
  }
  abort() {
    if (this.readyState !== 1) return;
    this.readyState = 2; this.result = null;
    dispatch(this, new ProgressEvent('abort'));
    dispatch(this, new ProgressEvent('loadend'));
  }
}
(function () {
  var states = { EMPTY: 0, LOADING: 1, DONE: 2 };
  for (var k in states) { FileReader[k] = states[k]; FileReader.prototype[k] = states[k]; }
})();

// A body as [data, default Content-Type or null]; data is a string, an ArrayBuffer that the
// caller may keep (always a private copy), or null.
function toBody(body) {
  if (body === null || body === undefined) return [null, null];
  if (typeof body === 'string') return [body, 'text/plain;charset=UTF-8'];
  if (body instanceof ReadableStream) throw new TypeError('Streaming request bodies are not supported');
  if (body instanceof Blob) return [body._buf.slice(0), body.type || null];
  if (body instanceof ArrayBuffer) return [body.slice(0), null];
  if (ArrayBuffer.isView(body)) return [body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength), null];
  if (body instanceof URLSearchParams) return [body.toString(), 'application/x-www-form-urlencoded;charset=UTF-8'];
  if (body instanceof FormData) {
    var boundary = '----invisium', hex = '0123456789abcdef', i, s = '';
    for (i = 0; i < 24; i++) boundary += hex[Math.floor(Math.random() * 16)];
    for (i = 0; i < body._e.length; i++) {
      var name = body._e[i][0].split('"').join('%22').split('\r').join('%0D').split('\n').join('%0A');
      s += '--' + boundary + '\r\nContent-Disposition: form-data; name="' + name + '"\r\n\r\n' + body._e[i][1] + '\r\n';
    }
    return [s + '--' + boundary + '--\r\n', 'multipart/form-data; boundary=' + boundary];
  }
  return [String(body), 'text/plain;charset=UTF-8'];
}
function unsupportedBody(what) {
  return Promise.reject(new DOMException(what + ' is not supported yet', 'NotSupportedError'));
}

class Response {
  constructor(body, init) {
    init = init || {};
    this.status = init.status === undefined ? 200 : init.status;
    this.statusText = init.statusText === undefined ? '' : String(init.statusText);
    this.headers = new Headers(init.headers);
    this.url = '';
    this.redirected = false;
    this.type = 'default';
    this.bodyUsed = false;
    // the body is either data (string, ArrayBuffer or null) or a stream the caller supplied
    var fromStream = body instanceof ReadableStream;
    var enc = fromStream ? [null, null] : toBody(body);
    hide(this, '_data', enc[0]);
    hide(this, '_stream', fromStream ? body : null);
    if (enc[1] && !this.headers.has('content-type')) this.headers.set('content-type', enc[1]);
  }
  get ok() { return this.status >= 200 && this.status < 300; }
  get body() {
    var self = this;
    if (!this._stream && this._data !== null) {
      this._stream = bytesStream(function () {
        self.bodyUsed = true;
        var d = self._data;
        return new Uint8Array(typeof d === 'string' ? n.encode(d) : d.slice(0));
      });
    }
    return this._stream;
  }
  _take() {
    if (this.bodyUsed || (this._stream && this._stream.locked)) return Promise.reject(new TypeError('The body has already been read'));
    this.bodyUsed = true;
    if (this._data === null && this._stream) return drainStream(this._stream);
    return Promise.resolve(this._data);
  }
  // `text()` is always UTF-8, whatever the Content-Type says (the Fetch standard)
  text() { return this._take().then(function (d) { return d === null ? '' : typeof d === 'string' ? d : n.decode(d, 'utf-8'); }); }
  json() { return this.text().then(function (t) { return JSON.parse(t); }); }
  arrayBuffer() {
    return this._take().then(function (d) { return d === null ? new ArrayBuffer(0) : typeof d === 'string' ? n.encode(d) : d.slice(0); });
  }
  bytes() { return this.arrayBuffer().then(function (b) { return new Uint8Array(b); }); }
  blob() {
    var type = this.headers.get('content-type') || '';
    return this.arrayBuffer().then(function (b) { return new Blob([b], { type: type }); });
  }
  formData() { return unsupportedBody('Response.formData'); }
  clone() {
    if (this.bodyUsed || (this._stream && this._stream.locked)) throw new TypeError('The body has already been read');
    var r = new Response(null, { status: this.status, statusText: this.statusText, headers: this.headers });
    if (this._data === null && this._stream) {
      var pair = this._stream.tee();
      this._stream = pair[0]; r._stream = pair[1];
    }
    r._data = this._data;   // shared: neither copy ever changes it
    r.url = this.url; r.redirected = this.redirected; r.type = this.type;
    return r;
  }
  static error() { var r = new Response(null, { status: 0 }); r.type = 'error'; return r; }
  static json(data, init) {
    init = init || {};
    var h = new Headers(init.headers);
    if (!h.has('content-type')) h.set('content-type', 'application/json');
    return new Response(JSON.stringify(data), { status: init.status, statusText: init.statusText, headers: h });
  }
}

class Request {
  constructor(input, init) {
    init = init || {};
    var from = input instanceof Request ? input : null;
    var url = from ? from.url : n.resolve(String(input));
    if (url === null) throw new TypeError("Failed to construct 'Request': Invalid URL");
    this.url = url;
    this.method = normMethod(init.method !== undefined ? init.method : from ? from.method : 'GET');
    this.headers = new Headers(init.headers !== undefined ? init.headers : from ? from.headers : undefined);
    this.mode = init.mode !== undefined ? String(init.mode) : from ? from.mode : 'cors';
    this.credentials = init.credentials !== undefined ? String(init.credentials) : from ? from.credentials : 'same-origin';
    this.signal = init.signal !== undefined ? init.signal : from ? from.signal : null;
    this.cache = 'default'; this.redirect = 'follow'; this.referrer = 'about:client'; this.destination = '';
    hide(this, '_body', init.body !== undefined ? init.body : from ? from._body : null);
  }
  clone() { return new Request(this); }
}

function fetch(input, init) {
  return new Promise(function (resolve, reject) {
    var req = new Request(input, init);
    var enc = toBody(req._body);
    if (enc[0] !== null && (req.method === 'GET' || req.method === 'HEAD')) throw new TypeError('A GET or HEAD request cannot have a body');
    var headers = new Headers(req.headers);
    if (enc[1] && !headers.has('content-type')) headers.set('content-type', enc[1]);
    var signal = req.signal;
    if (signal && signal.aborted) { reject(signal.reason); return; }
    var cred = req.credentials === 'include' ? 2 : req.credentials === 'omit' ? 0 : 1;
    var id = startFetch(req.method, req.url, headers, enc[0], req.mode === 'same-origin', cred, function (ok, status, url, flat, buf) {
      if (!ok) { reject(new TypeError('Failed to fetch')); return; }
      var r = new Response(null, { status: status, statusText: STATUS_TEXT[status] || '', headers: headersFromFlat(flat) });
      r._data = buf;
      r.url = url; r.redirected = url !== req.url; r.type = 'basic';
      resolve(r);
    });
    if (signal && typeof signal.addEventListener === 'function') {
      signal.addEventListener('abort', function () { cancelFetch(id); reject(signal.reason); });
    }
  });
}

class XMLHttpRequest extends EventTarget {
  constructor() {
    super();
    this.readyState = 0; this.status = 0; this.statusText = ''; this.responseText = ''; this.responseURL = '';
    this.responseType = ''; this.withCredentials = false; this.timeout = 0;
    this.upload = new EventTarget();
    this.onreadystatechange = null; this.onload = null; this.onerror = null; this.onloadend = null;
    this.onabort = null; this.ontimeout = null; this.onprogress = null; this.onloadstart = null;
    hide(this, '_method', 'GET'); hide(this, '_url', ''); hide(this, '_headers', new Headers());
    hide(this, '_resp', null); hide(this, '_id', 0); hide(this, '_sent', false); hide(this, '_timer', 0);
    hide(this, '_buf', null);
  }
  open(method, url, async) {
    if (async === false) {
      n.log(2, 'synchronous XMLHttpRequest is not supported');
      throw new DOMException('Synchronous XMLHttpRequest is not supported', 'NotSupportedError');
    }
    var abs = n.resolve(String(url));
    if (abs === null) throw new DOMException('Invalid URL', 'SyntaxError');
    this._method = normMethod(method);
    this._url = abs;
    this._headers = new Headers();
    this._resp = null; this._sent = false; this._id = 0; this._buf = null;
    this.status = 0; this.statusText = ''; this.responseText = ''; this.responseURL = '';
    this._state(1);
  }
  setRequestHeader(k, v) {
    if (this.readyState !== 1 || this._sent) throw new DOMException('The request is not open', 'InvalidStateError');
    this._headers.append(k, v);
  }
  send(body) {
    if (this.readyState !== 1 || this._sent) throw new DOMException('The request is not open', 'InvalidStateError');
    this._sent = true;
    var m = this._method, self = this;
    var enc = m === 'GET' || m === 'HEAD' ? [null, null] : toBody(body);
    if (enc[1] && !this._headers.has('content-type')) this._headers.set('content-type', enc[1]);
    try {
      this._id = startFetch(m, this._url, this._headers, enc[0], false, this.withCredentials ? 2 : 1, function (ok, status, url, flat, buf, charset) {
        self._done(ok, status, url, flat, buf, charset);
      });
    } catch (e) {
      g.setTimeout(function () { self._fail('error'); }, 0);
      return;
    }
    this._fire('loadstart');
    if (this.timeout > 0) this._timer = g.setTimeout(function () { cancelFetch(self._id); self._fail('timeout'); }, this.timeout);
  }
  _state(s) { this.readyState = s; dispatch(this, new Event('readystatechange')); }
  _fire(type) { dispatch(this, new ProgressEvent(type, { loaded: this._buf ? this._buf.byteLength : this.responseText.length })); }
  _done(ok, status, url, flat, buf, charset) {
    if (this._timer) { g.clearTimeout(this._timer); this._timer = 0; }
    if (!ok) { this._fail('error'); return; }
    this.status = status; this.statusText = STATUS_TEXT[status] || ''; this.responseURL = url;
    this._resp = headersFromFlat(flat);
    this._state(2);
    // binary response types keep the bytes; the others keep only the decoded text
    if (this.responseType === 'arraybuffer' || this.responseType === 'blob') this._buf = buf;
    else this.responseText = n.decode(buf, charset || 'utf-8');
    this._state(3);
    this._state(4);
    this._fire('load');
    this._fire('loadend');
  }
  _fail(type) {
    if (this._timer) { g.clearTimeout(this._timer); this._timer = 0; }
    this._id = 0; this.status = 0; this.statusText = ''; this.responseText = ''; this._resp = null; this._buf = null;
    this._state(4);
    this._fire(type);
    this._fire('loadend');
  }
  abort() {
    if (this._sent && this.readyState !== 4 && this._id) { cancelFetch(this._id); this._fail('abort'); }
    this.readyState = 0;
  }
  get response() {
    if (this.responseType === 'arraybuffer') return this.readyState === 4 ? this._buf : null;
    if (this.responseType === 'blob') {
      if (this.readyState !== 4 || !this._buf) return null;
      return new Blob([this._buf], { type: (this._resp && this._resp.get('content-type')) || '' });
    }
    if (this.responseType === 'json') {
      if (this.readyState !== 4) return null;
      try { return JSON.parse(this.responseText); } catch (e) { return null; }
    }
    return this.responseText;
  }
  get responseXML() { return null; }
  getResponseHeader(k) { return this._resp ? this._resp.get(k) : null; }
  getAllResponseHeaders() {
    var s = '';
    if (this._resp) this._resp.forEach(function (v, k) { s += k + ': ' + v + '\r\n'; });
    return s;
  }
  overrideMimeType() {}
}
(function () {
  var states = { UNSENT: 0, OPENED: 1, HEADERS_RECEIVED: 2, LOADING: 3, DONE: 4 };
  for (var k in states) { XMLHttpRequest[k] = states[k]; XMLHttpRequest.prototype[k] = states[k]; }
})();
