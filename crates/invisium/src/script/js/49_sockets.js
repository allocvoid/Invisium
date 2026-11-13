// ───────────────────────────── WebSocket ─────────────────────────────
// The connection itself belongs to the embedder's network stack; this is the script-facing
// object. Natives: n.wsOpen, n.wsSend, n.wsClose, n.wsBuffered. Events come in through
// socketEvent (see the entry points at the end of the bootstrap).

class CloseEvent extends Event {
  constructor(type, init) { super(type, init); copyInit(this, init, { wasClean: false, code: 0, reason: '' }); }
}

var SOCKETS = {};
var WS_TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

class WebSocket extends EventTarget {
  constructor(url, protocols) {
    super();
    if (arguments.length < 1) throw new TypeError("Failed to construct 'WebSocket': 1 argument required");
    var abs = parseUrl(String(url), location.href);
    if (abs === null) throw new DOMException("The URL '" + url + "' is invalid.", 'SyntaxError');
    // http(s) addresses mean the matching ws(s) ones
    abs = abs.replace(/^http(s?):/i, 'ws$1:');
    if (!/^wss?:/i.test(abs)) throw new DOMException("The URL's scheme must be either 'http', 'https', 'ws', or 'wss'.", 'SyntaxError');
    if (abs.indexOf('#') >= 0) throw new DOMException('The URL contains a fragment identifier.', 'SyntaxError');
    var list = protocols === undefined ? [] : typeof protocols === 'string' ? [protocols] : Array.prototype.slice.call(protocols).map(String);
    for (var i = 0; i < list.length; i++) {
      if (!WS_TOKEN.test(list[i])) throw new DOMException("The subprotocol '" + list[i] + "' is invalid.", 'SyntaxError');
      if (list.indexOf(list[i]) !== i) throw new DOMException("The subprotocol '" + list[i] + "' is duplicated.", 'SyntaxError');
    }
    this.url = abs; this.readyState = 0; this.protocol = ''; this.extensions = ''; this.binaryType = 'blob';
    this.onopen = null; this.onmessage = null; this.onerror = null; this.onclose = null;
    hide(this, '_queued', 0);
    var id = n.wsOpen(abs, list.join(','));
    hide(this, '_id', id);
    if (id) SOCKETS[id] = this;
    else {
      // the document may not have another socket: it fails like a refused connection
      var self = this;
      g.setTimeout(function () { socketClosed(self, 1006, '', false); }, 0);
    }
  }
  get bufferedAmount() { return this.readyState === 1 ? n.wsBuffered(this._id) : this._queued; }
  send(data) {
    if (arguments.length < 1) throw new TypeError("Failed to execute 'send' on 'WebSocket': 1 argument required");
    if (this.readyState === 0) throw new DOMException("Failed to execute 'send' on 'WebSocket': Still in CONNECTING state.", 'InvalidStateError');
    var out;
    if (data instanceof Blob) out = data._buf !== null ? data._buf : blobBuffer(data);
    else if (data instanceof ArrayBuffer || ArrayBuffer.isView(data)) out = data;
    else out = String(data);
    if (this.readyState !== 1) {
      // closing or closed: the data is dropped, but counts as buffered
      this._queued += typeof out === 'string' ? n.encode(out).byteLength : out.byteLength;
      return;
    }
    n.wsSend(this._id, out);
  }
  close(code, reason) {
    if (code !== undefined) {
      code = Number(code);
      if (!(code === 1000 || (code >= 3000 && code <= 4999))) throw new DOMException('The close code must be either 1000, or between 3000 and 4999. ' + code + ' is neither.', 'InvalidAccessError');
    }
    reason = reason === undefined ? '' : String(reason);
    if (n.encode(reason).byteLength > 123) throw new DOMException('The close reason must not be greater than 123 UTF-8 bytes.', 'SyntaxError');
    if (this.readyState >= 2) return;
    this.readyState = 2;
    if (this._id) n.wsClose(this._id, code, reason);
  }
}
WebSocket.CONNECTING = 0; WebSocket.OPEN = 1; WebSocket.CLOSING = 2; WebSocket.CLOSED = 3;
WebSocket.prototype.CONNECTING = 0; WebSocket.prototype.OPEN = 1; WebSocket.prototype.CLOSING = 2; WebSocket.prototype.CLOSED = 3;

function socketOrigin(ws) {
  var m = /^(wss?:\/\/[^\/?#]+)/i.exec(ws.url);
  return m ? m[1].toLowerCase() : '';
}

function socketClosed(ws, code, reason, clean) {
  if (ws.readyState === 3) return;
  var opened = ws.readyState !== 0;
  ws.readyState = 3;
  if (ws._id) delete SOCKETS[ws._id];
  // a connection that never opened, or broke off, is an error first
  if (!clean || !opened) dispatch(ws, new Event('error'));
  dispatch(ws, new CloseEvent('close', { wasClean: !!clean, code: code, reason: reason }));
}

// From the engine: kind 0 = open (a = subprotocol), 1 = text (a), 2 = binary (a = ArrayBuffer),
// 3 = closed (a = reason, b = code, c = clean).
function socketEvent(id, kind, a, b, c) {
  var ws = SOCKETS[id];
  if (!ws) return;
  if (kind === 0) {
    if (ws.readyState !== 0) return;
    ws.readyState = 1; ws.protocol = a;
    dispatch(ws, new Event('open'));
  } else if (kind === 1 || kind === 2) {
    // what the server sent before it saw our close still arrives
    if (ws.readyState !== 1 && ws.readyState !== 2) return;
    var data = kind === 1 ? a : ws.binaryType === 'arraybuffer' ? a : new Blob([a]);
    dispatch(ws, new MessageEvent('message', { data: data, origin: socketOrigin(ws) }));
  } else {
    socketClosed(ws, b, a, c);
  }
}
