// Streams: ReadableStream with default readers, and small WritableStream / TransformStream
// versions on top of promise chains. No byte (BYOB) readers and no size-based backpressure:
// the queue counts chunks. The network stack delivers a response body whole, so the stream of
// a response has one chunk.

function capability() {
  var cap = {};
  cap.promise = new Promise(function (res, rej) { cap.resolve = res; cap.reject = rej; });
  cap.promise.catch(function () {});   // a stream nobody watches must not report its error as unhandled
  return cap;
}
function streamClose(s) {
  s._state = 'closed';
  var r;
  while ((r = s._reads.shift())) r.resolve({ value: undefined, done: true });
  if (s._closed) s._closed.resolve(undefined);
}
function streamError(s, e) {
  if (s._state !== 'readable') return;
  s._state = 'errored'; s._err = e; s._queue.length = 0;
  var r;
  while ((r = s._reads.shift())) r.reject(e);
  if (s._closed) s._closed.reject(e);
}
function streamPull(s) {
  if (s._pulling || !s._started || s._closing || s._state !== 'readable' || typeof s._src.pull !== 'function') return;
  if (!s._reads.length && s._queue.length >= s._hwm) return;
  s._pulling = true;
  var p;
  try { p = Promise.resolve(s._src.pull(s._ctrl)); } catch (e) { p = Promise.reject(e); }
  p.then(function () { s._pulling = false; if (s._reads.length) streamPull(s); }, function (e) { s._pulling = false; streamError(s, e); });
}
function streamCancel(s, reason) {
  if (s._state === 'closed') return Promise.resolve(undefined);
  if (s._state === 'errored') return Promise.reject(s._err);
  s._queue.length = 0;
  streamClose(s);
  try {
    return Promise.resolve(typeof s._src.cancel === 'function' ? s._src.cancel(reason) : undefined).then(function () {});
  } catch (e) { return Promise.reject(e); }
}

class ReadableStreamDefaultController {
  constructor(stream) { hide(this, '_s', stream); }
  get desiredSize() {
    var s = this._s;
    return s._state === 'errored' ? null : s._state === 'closed' ? 0 : s._hwm - s._queue.length;
  }
  enqueue(chunk) {
    var s = this._s;
    if (s._state !== 'readable' || s._closing) throw new TypeError('The stream cannot take more chunks');
    if (s._reads.length) s._reads.shift().resolve({ value: chunk, done: false });
    else s._queue.push(chunk);
  }
  close() {
    var s = this._s;
    if (s._state !== 'readable' || s._closing) throw new TypeError('The stream is already closed');
    s._closing = true;
    if (!s._queue.length) streamClose(s);
  }
  error(e) { streamError(this._s, e); }
}

class ReadableStream {
  constructor(source, strategy) {
    source = source === undefined || source === null ? {} : source;
    hide(this, '_src', source); hide(this, '_queue', []); hide(this, '_reads', []);
    hide(this, '_state', 'readable'); hide(this, '_err', undefined); hide(this, '_closing', false);
    hide(this, '_pulling', false); hide(this, '_started', false); hide(this, '_reader', null); hide(this, '_closed', null);
    var hwm = strategy && strategy.highWaterMark !== undefined ? Number(strategy.highWaterMark) : 1;
    if (!(hwm >= 0)) throw new RangeError('Invalid highWaterMark');
    hide(this, '_hwm', hwm);
    hide(this, '_ctrl', new ReadableStreamDefaultController(this));
    var s = this;
    var started = typeof source.start === 'function' ? source.start(this._ctrl) : undefined;   // a throw here fails the constructor
    Promise.resolve(started).then(function () { s._started = true; streamPull(s); }, function (e) { streamError(s, e); });
  }
  get locked() { return this._reader !== null; }
  getReader(opts) {
    if (opts && opts.mode !== undefined) throw new TypeError('Byte stream readers are not supported');
    return new ReadableStreamDefaultReader(this);
  }
  cancel(reason) {
    if (this._reader) return Promise.reject(new TypeError('The stream is locked to a reader'));
    return streamCancel(this, reason);
  }
  tee() {
    var reader = this.getReader(), ctrls = [];
    var each = function (f) { ctrls.forEach(function (c) { try { f(c); } catch (e) {} }); };
    var pump = function () {
      return reader.read().then(function (r) {
        if (r.done) each(function (c) { c.close(); });
        else each(function (c) { c.enqueue(r.value); });
      }, function (e) { each(function (c) { c.error(e); }); });
    };
    var branch = function () { return new ReadableStream({ start: function (c) { ctrls.push(c); }, pull: pump }, { highWaterMark: 0 }); };
    return [branch(), branch()];
  }
  pipeTo(dest, opts) {
    if (!(dest instanceof WritableStream)) return Promise.reject(new TypeError('pipeTo needs a WritableStream'));
    var reader, writer;
    try { reader = this.getReader(); writer = dest.getWriter(); } catch (e) { return Promise.reject(e); }
    var step = function () {
      return reader.read().then(function (r) {
        if (r.done) return opts && opts.preventClose ? undefined : writer.close();
        return writer.write(r.value).then(step);
      });
    };
    return step().then(function () { reader.releaseLock(); writer.releaseLock(); }, function (e) {
      reader.cancel(e).catch(function () {});
      writer.abort(e).catch(function () {});
      reader.releaseLock(); writer.releaseLock();
      throw e;
    });
  }
  pipeThrough(pair, opts) {
    this.pipeTo(pair.writable, opts).catch(function () {});
    return pair.readable;
  }
  values() {
    var reader = this.getReader();
    var it = {
      next: function () { return reader.read().then(function (r) { if (r.done) reader.releaseLock(); return r; }); },
      return: function (v) {
        var p = reader.cancel(v);
        reader.releaseLock();
        return p.then(function () { return { value: v, done: true }; });
      }
    };
    it[Symbol.asyncIterator] = function () { return it; };
    return it;
  }
  [Symbol.asyncIterator]() { return this.values(); }
}

class ReadableStreamDefaultReader {
  constructor(stream) {
    if (!(stream instanceof ReadableStream)) throw new TypeError('The argument is not a ReadableStream');
    if (stream._reader) throw new TypeError('The stream is locked to a reader');
    hide(this, '_s', stream);
    hide(this, '_cap', capability());
    stream._reader = this; stream._closed = this._cap;
    if (stream._state === 'closed') this._cap.resolve(undefined);
    else if (stream._state === 'errored') this._cap.reject(stream._err);
  }
  get closed() { return this._cap.promise; }
  read() {
    var s = this._s;
    if (!s) return Promise.reject(new TypeError('The reader has been released'));
    if (s._queue.length) {
      var v = s._queue.shift();
      if (s._closing && !s._queue.length) streamClose(s); else streamPull(s);
      return Promise.resolve({ value: v, done: false });
    }
    if (s._state === 'closed') return Promise.resolve({ value: undefined, done: true });
    if (s._state === 'errored') return Promise.reject(s._err);
    return new Promise(function (resolve, reject) { s._reads.push({ resolve: resolve, reject: reject }); streamPull(s); });
  }
  cancel(reason) {
    return this._s ? streamCancel(this._s, reason) : Promise.reject(new TypeError('The reader has been released'));
  }
  releaseLock() {
    var s = this._s, r;
    if (!s) return;
    while ((r = s._reads.shift())) r.reject(new TypeError('The reader was released'));
    s._reader = null; s._closed = null; this._s = null;
  }
}

// Writes run one after another on a promise chain; the first failure makes the stream errored.
class WritableStream {
  constructor(sink) {
    sink = sink === undefined || sink === null ? {} : sink;
    hide(this, '_sink', sink); hide(this, '_writer', null); hide(this, '_state', 'writable'); hide(this, '_err', undefined);
    hide(this, '_cap', capability());
    var s = this;
    hide(this, '_ctrl', { error: function (e) { s._fail(e); } });
    hide(this, '_chain', Promise.resolve(typeof sink.start === 'function' ? sink.start(this._ctrl) : undefined));
    this._chain.catch(function (e) { s._fail(e); });
  }
  _fail(e) {
    if (this._state === 'errored' || this._state === 'closed') return;
    this._state = 'errored'; this._err = e;
    this._cap.reject(e);
  }
  _run(f) {
    var s = this;
    var p = this._chain.then(function () {
      if (s._state === 'errored') throw s._err;
      return f();
    });
    p.catch(function (e) { s._fail(e); });
    this._chain = p.catch(function () {});
    return p;
  }
  get locked() { return this._writer !== null; }
  getWriter() { return new WritableStreamDefaultWriter(this); }
  close() {
    var s = this;
    if (s._state !== 'writable') return Promise.reject(new TypeError('The stream is not writable'));
    s._state = 'closing';
    return s._run(function () { return typeof s._sink.close === 'function' ? s._sink.close() : undefined; })
      .then(function () { s._state = 'closed'; s._cap.resolve(undefined); });
  }
  abort(reason) {
    var s = this;
    if (s._state === 'closed' || s._state === 'errored') return Promise.resolve(undefined);
    s._fail(reason);
    try { return Promise.resolve(typeof s._sink.abort === 'function' ? s._sink.abort(reason) : undefined).then(function () {}); } catch (e) { return Promise.reject(e); }
  }
}
class WritableStreamDefaultWriter {
  constructor(stream) {
    if (!(stream instanceof WritableStream)) throw new TypeError('The argument is not a WritableStream');
    if (stream._writer) throw new TypeError('The stream is locked to a writer');
    hide(this, '_s', stream);
    stream._writer = this;
  }
  get closed() { return this._s ? this._s._cap.promise : Promise.reject(new TypeError('The writer has been released')); }
  get ready() { return Promise.resolve(undefined); }
  get desiredSize() { var s = this._s; return !s || s._state === 'errored' ? null : s._state === 'closed' ? 0 : 1; }
  write(chunk) {
    var s = this._s;
    if (!s) return Promise.reject(new TypeError('The writer has been released'));
    if (s._state !== 'writable') return Promise.reject(s._state === 'errored' ? s._err : new TypeError('The stream is closed'));
    return s._run(function () { return typeof s._sink.write === 'function' ? s._sink.write(chunk, s._ctrl) : undefined; });
  }
  close() { return this._s ? this._s.close() : Promise.reject(new TypeError('The writer has been released')); }
  abort(reason) { return this._s ? this._s.abort(reason) : Promise.reject(new TypeError('The writer has been released')); }
  releaseLock() { if (this._s) { this._s._writer = null; this._s = null; } }
}

class TransformStream {
  constructor(transformer) {
    var t = transformer === undefined || transformer === null ? {} : transformer;
    var out;
    this.readable = new ReadableStream({ start: function (c) { out = c; } });
    var ctrl = {
      enqueue: function (v) { out.enqueue(v); },
      error: function (e) { out.error(e); },
      terminate: function () { try { out.close(); } catch (e) {} }
    };
    Object.defineProperty(ctrl, 'desiredSize', { get: function () { return out.desiredSize; } });
    this.writable = new WritableStream({
      start: function () { return typeof t.start === 'function' ? t.start(ctrl) : undefined; },
      write: function (chunk) { return typeof t.transform === 'function' ? t.transform(chunk, ctrl) : ctrl.enqueue(chunk); },
      close: function () {
        return Promise.resolve(typeof t.flush === 'function' ? t.flush(ctrl) : undefined).then(function () { ctrl.terminate(); });
      },
      abort: function (e) { out.error(e); }
    });
    this.writable._cap.promise.catch(function (e) { out.error(e); });
  }
}
class CountQueuingStrategy {
  constructor(init) { this.highWaterMark = Number(init.highWaterMark); }
  size() { return 1; }
}
class ByteLengthQueuingStrategy {
  constructor(init) { this.highWaterMark = Number(init.highWaterMark); }
  size(chunk) { return chunk.byteLength; }
}

// Everything a stream yields, as one ArrayBuffer (chunks may be strings, buffers, views or Blobs).
function drainStream(stream) {
  var reader = stream.getReader(), chunks = [], total = 0;
  var step = function () {
    return reader.read().then(function (r) {
      if (r.done) {
        var out = new Uint8Array(total), at = 0;
        for (var i = 0; i < chunks.length; i++) { out.set(chunks[i], at); at += chunks[i].byteLength; }
        return out.buffer;
      }
      var c = blobPart(r.value);
      chunks.push(c); total += c.byteLength;
      return step();
    });
  };
  return step();
}
// A stream with the given bytes as its only chunk, made when the first read asks for it.
function bytesStream(make) {
  return new ReadableStream({
    pull: function (c) {
      var bytes = make();
      if (bytes.byteLength) c.enqueue(bytes);
      c.close();
    }
  }, { highWaterMark: 0 });
}
