// Other documents: the page this one is framed in, and the pages in its frames.
//
// Every document has a script engine of its own, so documents never share objects. The
// window of another document is a small stand-in that can be sent messages and nothing else,
// which is what a browser allows across origins anyway.

function WindowStandIn(kind, node) {
  hide(this, '_k', kind);
  hide(this, '_n', node);
}
WindowStandIn.prototype.postMessage = function (data, target) {
  var origin = typeof target === 'string' ? target
    : (target !== null && typeof target === 'object' && typeof target.targetOrigin === 'string' ? target.targetOrigin : '*');
  // what can cross is what JSON can carry; functions and symbols cannot be cloned at all
  var json, bad = false;
  try {
    json = JSON.stringify(data === undefined ? { u: 1 } : { d: data }, function (k, v) {
      if (typeof v === 'function' || typeof v === 'symbol') bad = true;
      return v;
    });
  } catch (e) { bad = true; }
  if (bad || typeof json !== 'string') throw new DOMException('The object could not be cloned.', 'DataCloneError');
  n.framePost(this._k, this._n, json, origin);
};
WindowStandIn.prototype.focus = function () {};
WindowStandIn.prototype.blur = function () {};
WindowStandIn.prototype.close = function () {};
Object.defineProperty(WindowStandIn.prototype, 'closed', { get: function () { return false; }, configurable: true });
Object.defineProperty(WindowStandIn.prototype, 'length', { get: function () { return 0; }, configurable: true });

var outerWindows = [null, null];

// kind 0: the parent document, 1: the top document, 2: the document in frame element `node`
function otherWindow(kind, node) {
  if (kind === 2) {
    if (!node._cw) hide(node, '_cw', new WindowStandIn(2, node));
    return node._cw;
  }
  if (!outerWindows[kind]) outerWindows[kind] = new WindowStandIn(kind, null);
  return outerWindows[kind];
}

// A message from another document: its data as JSON, the sender's origin, and the frame
// element it came through (null when it came from the document this one is framed in).
function frameMessage(json, origin, node) {
  var box;
  try { box = JSON.parse(json); } catch (e) { return; }
  var ev = new MessageEvent('message', { data: box.d === undefined ? null : box.d, origin: origin, source: node ? otherWindow(2, node) : otherWindow(0, null) });
  if (box.u) Object.defineProperty(ev, 'data', { value: undefined, enumerable: true, configurable: true });
  dispatch(g, ev);
}

Object.defineProperty(HTMLIFrameElement.prototype, 'contentWindow', {
  get: function () { return otherWindow(2, this); }, enumerable: true, configurable: true,
});
