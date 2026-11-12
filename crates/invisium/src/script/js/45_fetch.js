
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
function fetchDone(id, ok, status, url, flat, text) {
  var cb = pendingFetches[id];
  if (!cb) return;
  delete pendingFetches[id];
  try { cb(ok, status, url, flat, text); } catch (e) { reportError(e); }
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

// A request body as [text or null, default Content-Type or null].
function encodeBody(body) {
  if (body === null || body === undefined) return [null, null];
  if (typeof body === 'string') return [body, 'text/plain;charset=UTF-8'];
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
    hide(this, '_text', body === null || body === undefined ? '' : String(body));
  }
  get ok() { return this.status >= 200 && this.status < 300; }
  get body() { return null; }
  text() {
    if (this.bodyUsed) return Promise.reject(new TypeError('The body has already been read'));
    this.bodyUsed = true;
    return Promise.resolve(this._text);
  }
  json() { return this.text().then(function (t) { return JSON.parse(t); }); }
  arrayBuffer() { return unsupportedBody('Response.arrayBuffer'); }
  blob() { return unsupportedBody('Response.blob'); }
  formData() { return unsupportedBody('Response.formData'); }
  clone() {
    var r = new Response(this._text, { status: this.status, statusText: this.statusText, headers: this.headers });
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
    var enc = encodeBody(req._body);
    if (enc[0] !== null && (req.method === 'GET' || req.method === 'HEAD')) throw new TypeError('A GET or HEAD request cannot have a body');
    var headers = new Headers(req.headers);
    if (enc[1] && !headers.has('content-type')) headers.set('content-type', enc[1]);
    var signal = req.signal;
    if (signal && signal.aborted) { reject(signal.reason); return; }
    var cred = req.credentials === 'include' ? 2 : req.credentials === 'omit' ? 0 : 1;
    var id = startFetch(req.method, req.url, headers, enc[0], req.mode === 'same-origin', cred, function (ok, status, url, flat, text) {
      if (!ok) { reject(new TypeError('Failed to fetch')); return; }
      var r = new Response(text, { status: status, statusText: STATUS_TEXT[status] || '', headers: headersFromFlat(flat) });
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
    this._resp = null; this._sent = false; this._id = 0;
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
    var enc = m === 'GET' || m === 'HEAD' ? [null, null] : encodeBody(body);
    if (enc[1] && !this._headers.has('content-type')) this._headers.set('content-type', enc[1]);
    try {
      this._id = startFetch(m, this._url, this._headers, enc[0], false, this.withCredentials ? 2 : 1, function (ok, status, url, flat, text) {
        self._done(ok, status, url, flat, text);
      });
    } catch (e) {
      g.setTimeout(function () { self._fail('error'); }, 0);
      return;
    }
    this._fire('loadstart');
    if (this.timeout > 0) this._timer = g.setTimeout(function () { cancelFetch(self._id); self._fail('timeout'); }, this.timeout);
  }
  _state(s) { this.readyState = s; dispatch(this, new Event('readystatechange')); }
  _fire(type) { dispatch(this, new ProgressEvent(type, { loaded: this.responseText.length })); }
  _done(ok, status, url, flat, text) {
    if (this._timer) { g.clearTimeout(this._timer); this._timer = 0; }
    if (!ok) { this._fail('error'); return; }
    this.status = status; this.statusText = STATUS_TEXT[status] || ''; this.responseURL = url;
    this._resp = headersFromFlat(flat);
    this._state(2);
    this.responseText = text;
    this._state(3);
    this._state(4);
    this._fire('load');
    this._fire('loadend');
  }
  _fail(type) {
    if (this._timer) { g.clearTimeout(this._timer); this._timer = 0; }
    this._id = 0; this.status = 0; this.statusText = ''; this.responseText = ''; this._resp = null;
    this._state(4);
    this._fire(type);
    this._fire('loadend');
  }
  abort() {
    if (this._sent && this.readyState !== 4 && this._id) { cancelFetch(this._id); this._fail('abort'); }
    this.readyState = 0;
  }
  get response() {
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
