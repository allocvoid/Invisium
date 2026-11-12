
// ───────────────────────────── frames, observers, animations ─────────────────────────────

// requestAnimationFrame: callbacks wait here until the page is about to draw a frame.
var frameCallbacks = [], frameSeq = 0;
function requestFrame(cb) {
  if (typeof cb !== 'function') throw new TypeError('requestAnimationFrame: the callback is not a function');
  frameCallbacks.push([++frameSeq, cb]);
  n.wantFrame();
  return frameSeq;
}
function cancelFrame(id) {
  for (var i = 0; i < frameCallbacks.length; i++) {
    if (frameCallbacks[i][0] === id) { frameCallbacks.splice(i, 1); return; }
  }
}

// Size and visibility observers are checked once per frame, after the frame callbacks.
var resizeObservers = [], intersectionObservers = [];
function observersChanged() { n.observing(resizeObservers.length + intersectionObservers.length > 0); }
function dropFrom(list, item) { var i = list.indexOf(item); if (i >= 0) list.splice(i, 1); }

class ResizeObserverEntry {
  constructor(target, w, h) {
    this.target = target;
    this.contentRect = new DOMRect(0, 0, w, h);
    var size = [{ inlineSize: w, blockSize: h }];
    this.borderBoxSize = size; this.contentBoxSize = size; this.devicePixelContentBoxSize = size;
  }
}
class ResizeObserver {
  constructor(cb) {
    if (typeof cb !== 'function') throw new TypeError('ResizeObserver: the callback is not a function');
    hide(this, '_cb', cb); hide(this, '_targets', []);
  }
  observe(target) {
    if (!(target instanceof Element)) throw new TypeError('ResizeObserver.observe: the target is not an Element');
    for (var i = 0; i < this._targets.length; i++) if (this._targets[i].el === target) return;
    this._targets.push({ el: target, w: -1, h: -1 });
    if (resizeObservers.indexOf(this) < 0) resizeObservers.push(this);
    observersChanged();
  }
  unobserve(target) {
    for (var i = 0; i < this._targets.length; i++) if (this._targets[i].el === target) { this._targets.splice(i, 1); break; }
    if (!this._targets.length) { dropFrom(resizeObservers, this); observersChanged(); }
  }
  disconnect() { this._targets.length = 0; dropFrom(resizeObservers, this); observersChanged(); }
}
function checkResizeObservers() {
  var list = resizeObservers.slice();
  for (var i = 0; i < list.length; i++) {
    var o = list[i], entries = [];
    for (var j = 0; j < o._targets.length; j++) {
      var t = o._targets[j], r = n.rect(t.el);
      if (r[2] !== t.w || r[3] !== t.h) { t.w = r[2]; t.h = r[3]; entries.push(new ResizeObserverEntry(t.el, r[2], r[3])); }
    }
    if (entries.length) { try { o._cb.call(o, entries, o); } catch (e) { reportError(e); } }
  }
}

// "10px 20%" and the like: [top, right, bottom, left], each [amount, is a percentage].
function parseRootMargin(text) {
  var parts = String(text === undefined || text === null ? '' : text).trim().split(/\s+/).filter(function (p) { return p; });
  if (parts.length > 4) throw new SyntaxError('IntersectionObserver: rootMargin has too many values');
  var vals = parts.map(function (p) {
    var m = /^(-?\d*\.?\d+)(px|%)?$/.exec(p);
    if (!m || (m[2] === undefined && +m[1] !== 0)) throw new SyntaxError('IntersectionObserver: rootMargin must be in pixels or percent');
    return [+m[1], m[2] === '%'];
  });
  if (!vals.length) vals = [[0, false]];
  var top = vals[0], right = vals.length > 1 ? vals[1] : top, bottom = vals.length > 2 ? vals[2] : top, left = vals.length > 3 ? vals[3] : right;
  return [top, right, bottom, left];
}
class IntersectionObserverEntry {
  constructor(o) { hide(this, '_o', o); }
  get time() { return this._o.time; }
  get target() { return this._o.target; }
  get rootBounds() { return this._o.rootBounds; }
  get boundingClientRect() { return this._o.boundingClientRect; }
  get intersectionRect() { return this._o.intersectionRect; }
  get isIntersecting() { return this._o.isIntersecting; }
  get intersectionRatio() { return this._o.intersectionRatio; }
  get isVisible() { return false; }
}
class IntersectionObserver {
  constructor(cb, opts) {
    if (typeof cb !== 'function') throw new TypeError('IntersectionObserver: the callback is not a function');
    opts = opts || {};
    hide(this, '_cb', cb); hide(this, '_targets', []); hide(this, '_queue', []);
    var m = parseRootMargin(opts.rootMargin);
    hide(this, '_margin', m);
    this.root = opts.root instanceof Element ? opts.root : null;
    this.rootMargin = m.map(function (v) { return v[0] + (v[1] ? '%' : 'px'); }).join(' ');
    var t = opts.threshold === undefined || opts.threshold === null ? [0] : (Array.isArray(opts.threshold) ? opts.threshold.slice() : [opts.threshold]);
    t = t.map(Number);
    for (var i = 0; i < t.length; i++) if (!(t[i] >= 0 && t[i] <= 1)) throw new RangeError('IntersectionObserver: thresholds must be between 0 and 1');
    if (!t.length) t = [0];
    t.sort(function (a, b) { return a - b; });
    this.thresholds = t;
  }
  observe(target) {
    if (!(target instanceof Element)) throw new TypeError('IntersectionObserver.observe: the target is not an Element');
    for (var i = 0; i < this._targets.length; i++) if (this._targets[i].el === target) return;
    this._targets.push({ el: target, index: -1, was: false });
    if (intersectionObservers.indexOf(this) < 0) intersectionObservers.push(this);
    observersChanged();
  }
  unobserve(target) {
    for (var i = 0; i < this._targets.length; i++) if (this._targets[i].el === target) { this._targets.splice(i, 1); break; }
    if (!this._targets.length) { dropFrom(intersectionObservers, this); observersChanged(); }
  }
  disconnect() { this._targets.length = 0; this._queue.length = 0; dropFrom(intersectionObservers, this); observersChanged(); }
  takeRecords() { var q = this._queue.slice(); this._queue.length = 0; return q; }
}
function checkIntersectionObservers(now) {
  var list = intersectionObservers.slice();
  if (!list.length) return;
  var v = n.viewport();
  for (var i = 0; i < list.length; i++) {
    var o = list[i], m = o._margin;
    var rr = o.root ? n.rect(o.root) : [0, 0, v[0], v[1]];
    var amount = function (k, basis) { return m[k][1] ? m[k][0] * basis / 100 : m[k][0]; };
    var left = rr[0] - amount(3, rr[2]), top = rr[1] - amount(0, rr[3]);
    var right = rr[0] + rr[2] + amount(1, rr[2]), bottom = rr[1] + rr[3] + amount(2, rr[3]);
    for (var j = 0; j < o._targets.length; j++) {
      var t = o._targets[j], r = n.rect(t.el);
      var there = n.connected(t.el) && !(r[0] === 0 && r[1] === 0 && r[2] === 0 && r[3] === 0);
      var x0 = Math.max(r[0], left), y0 = Math.max(r[1], top), x1 = Math.min(r[0] + r[2], right), y1 = Math.min(r[1] + r[3], bottom);
      var touching = there && x1 >= x0 && y1 >= y0;
      var iw = touching ? x1 - x0 : 0, ih = touching ? y1 - y0 : 0, area = r[2] * r[3];
      var ratio = area > 0 ? Math.min(1, (iw * ih) / area) : (touching ? 1 : 0);
      var th = o.thresholds, index = 0;
      while (index < th.length && th[index] <= ratio) index++;
      // touching the root counts once the smallest threshold is met
      var is = touching && ratio >= th[0];
      if (!touching) index = 0;
      if (index === t.index && is === t.was) continue;
      t.index = index; t.was = is;
      o._queue.push(new IntersectionObserverEntry({
        time: now, target: t.el, isIntersecting: is, intersectionRatio: touching ? ratio : 0,
        rootBounds: new DOMRect(left, top, right - left, bottom - top),
        boundingClientRect: new DOMRect(r[0], r[1], r[2], r[3]),
        intersectionRect: touching ? new DOMRect(x0, y0, iw, ih) : new DOMRect(0, 0, 0, 0)
      }));
    }
    if (o._queue.length) {
      var entries = o.takeRecords();
      try { o._cb.call(o, entries, o); } catch (e) { reportError(e); }
    }
  }
}

// The page is about to draw a frame.
function runFrame(now) {
  var list = frameCallbacks;
  frameCallbacks = [];
  for (var i = 0; i < list.length; i++) {
    try { list[i][1].call(g, now); } catch (e) { reportError(e); }
  }
  checkResizeObservers();
  checkIntersectionObservers(now);
}

// ── Web Animations: `Element.animate`, `Animation`, `KeyframeEffect` ──

// backgroundColor → background-color, webkitTransform → -webkit-transform
function animProp(name) {
  if (name === 'cssFloat') return 'float';
  if (name === 'cssOffset') return 'offset';
  if (name.slice(0, 2) === '--') return name;
  var s = name.replace(/[A-Z]/g, function (c) { return '-' + c.toLowerCase(); });
  return /^(webkit|moz|ms)-/.test(s) ? '-' + s : s;
}
var NOT_PROPS = { offset: 1, easing: 1, composite: 1 };

// Either form of keyframes → [{ offset: number|null, easing: string|null, props: {css-name: text} }].
function normalizeKeyframes(kf) {
  var out = [], i, k;
  if (kf === null || kf === undefined) return out;
  if (typeof kf !== 'object') throw new TypeError('Keyframes must be an object or an array');
  if (Array.isArray(kf) || typeof kf[Symbol.iterator] === 'function') {
    var arr = Array.isArray(kf) ? kf : Array.from(kf);
    for (i = 0; i < arr.length; i++) {
      var f = arr[i];
      if (f === null || f === undefined) continue;
      if (typeof f !== 'object') throw new TypeError('A keyframe must be an object');
      var props = {};
      for (k in f) if (!NOT_PROPS[k] && f[k] !== undefined && f[k] !== null) props[animProp(k)] = String(f[k]);
      var off = f.offset === undefined || f.offset === null ? null : Number(f.offset);
      if (off !== null && !(off >= 0 && off <= 1)) throw new TypeError('Keyframe offsets must be between 0 and 1');
      out.push({ offset: off, easing: f.easing === undefined || f.easing === null ? null : String(f.easing), props: props });
    }
  } else {
    // { opacity: [0, 1], transform: [...] }: each property spreads its values evenly
    var byOffset = [];
    var slot = function (o) {
      for (var q = 0; q < byOffset.length; q++) if (byOffset[q].offset === o) return byOffset[q];
      var s = { offset: o, easing: null, props: {} };
      byOffset.push(s);
      return s;
    };
    var counts = 0;
    for (k in kf) {
      if (NOT_PROPS[k]) continue;
      var vals = Array.isArray(kf[k]) ? kf[k] : [kf[k]];
      counts = Math.max(counts, vals.length);
      for (i = 0; i < vals.length; i++) {
        if (vals[i] === null || vals[i] === undefined) continue;
        var at = vals.length === 1 ? 1 : i / (vals.length - 1);
        slot(at).props[animProp(k)] = String(vals[i]);
      }
    }
    byOffset.sort(function (a, b) { return a.offset - b.offset; });
    var offs = kf.offset === undefined || kf.offset === null ? [] : (Array.isArray(kf.offset) ? kf.offset : [kf.offset]);
    var eases = kf.easing === undefined || kf.easing === null ? [] : (Array.isArray(kf.easing) ? kf.easing : [kf.easing]);
    for (i = 0; i < byOffset.length; i++) {
      if (i < offs.length && offs[i] !== null && offs[i] !== undefined && byOffset.length === counts) byOffset[i].offset = Number(offs[i]);
      if (eases.length) byOffset[i].easing = String(eases[i % eases.length]);
    }
    out = byOffset;
  }
  // offsets that were left out are spread evenly between their neighbours
  var len = out.length;
  if (len) {
    if (out[len - 1].offset === null) out[len - 1].offset = 1;
    if (len > 1 && out[0].offset === null) out[0].offset = 0;
    i = 1;
    while (i < len) {
      if (out[i].offset === null) {
        var a = i - 1, b = i;
        while (out[b].offset === null) b++;
        for (var c = i; c < b; c++) out[c].offset = out[a].offset + (out[b].offset - out[a].offset) * (c - a) / (b - a);
        i = b;
      } else i++;
    }
  }
  return out;
}

function animTiming(options, base) {
  var t = base || { duration: 0, delay: 0, endDelay: 0, iterations: 1, iterationStart: 0, direction: 'normal', fill: 'auto', easing: 'linear' };
  if (typeof options === 'number') { t.duration = options; return t; }
  if (!options || typeof options !== 'object') return t;
  var num = function (k) { if (options[k] !== undefined && options[k] !== null) { var v = Number(options[k]); if (v === v) t[k] = v; } };
  if (options.duration !== 'auto') num('duration');
  num('delay'); num('endDelay'); num('iterations'); num('iterationStart');
  if (options.direction !== undefined) t.direction = String(options.direction);
  if (options.fill !== undefined) t.fill = String(options.fill);
  if (options.easing !== undefined) t.easing = String(options.easing);
  if (t.duration < 0 || t.iterations < 0) throw new TypeError('Animation timing values must not be negative');
  return t;
}

class AnimationEffect {
  getTiming() { var t = this._timing, o = {}; for (var k in t) o[k] = t[k]; return o; }
  getComputedTiming() {
    var o = this.getTiming(), a = this._anim;
    o.activeDuration = o.duration * o.iterations;
    o.endTime = Math.max(0, o.delay + o.activeDuration + o.endDelay);
    o.localTime = a ? a.currentTime : null;
    var local = o.localTime;
    if (local === null || o.duration <= 0) { o.progress = local === null ? null : 1; o.currentIteration = local === null ? null : 0; }
    else {
      var active = Math.min(Math.max(local - o.delay, 0), o.activeDuration), overall = active / o.duration;
      o.currentIteration = Math.min(Math.floor(overall), Math.max(0, Math.ceil(o.iterations) - 1));
      o.progress = active >= o.activeDuration ? 1 : overall - Math.floor(overall);
    }
    return o;
  }
  updateTiming(t) { animTiming(t, this._timing); if (this._anim) this._anim._rebuild(); }
}
class KeyframeEffect extends AnimationEffect {
  constructor(target, keyframes, options) {
    super();
    if (target instanceof KeyframeEffect) { keyframes = target._source; options = target._timing; target = target.target; }
    hide(this, '_source', keyframes);
    hide(this, '_frames', normalizeKeyframes(keyframes));
    hide(this, '_timing', animTiming(options));
    hide(this, '_anim', null);
    this.target = target || null;
    this.pseudoElement = null;
    this.composite = 'replace';
    this.iterationComposite = 'replace';
  }
  getKeyframes() {
    return this._frames.map(function (f) {
      var o = { offset: f.offset, computedOffset: f.offset, easing: f.easing || 'linear', composite: 'auto' };
      for (var k in f.props) o[k.replace(/-([a-z])/g, function (_, c) { return c.toUpperCase(); })] = f.props[k];
      return o;
    });
  }
  setKeyframes(kf) { this._source = kf; this._frames = normalizeKeyframes(kf); if (this._anim) this._anim._rebuild(); }
}

// Animations the engine is running, by its id.
var liveAnimations = Object.create(null);
var documentTimeline = { get currentTime() { return n.now(); } };

class Animation extends EventTarget {
  constructor(effect, timeline) {
    super();
    hide(this, '_id', 0); hide(this, '_rate', 1); hide(this, '_ended', false); hide(this, '_lastTime', null);
    hide(this, '_resolve', null); hide(this, '_reject', null); hide(this, '_finished', null);
    this.id = '';
    this.onfinish = null; this.oncancel = null; this.onremove = null;
    this.effect = effect || null;
    if (effect) effect._anim = this;
    this.timeline = timeline === undefined ? documentTimeline : timeline;
  }
  // Hand the animation to the engine (again, if it has let go of it).
  _start() {
    var e = this.effect;
    if (!e || !e.target) return false;
    if (this._id && n.animation(this._id, 'state') !== undefined) return true;
    var flat = [], frames = e._frames, t = e._timing;
    for (var i = 0; i < frames.length; i++) {
      var f = frames[i], names = Object.keys(f.props);
      flat.push(f.offset, f.easing, names.length);
      for (var j = 0; j < names.length; j++) flat.push(names[j], f.props[names[j]]);
    }
    var fill = t.fill === 'auto' ? 'none' : t.fill;
    if (this._id) delete liveAnimations[this._id];
    this._id = n.animate(e.target, flat, t.duration, t.delay, t.endDelay, t.iterations, t.iterationStart, t.direction, fill, t.easing);
    if (!this._id) return false;
    liveAnimations[this._id] = this;
    if (this._rate !== 1) n.animation(this._id, 'setRate', this._rate);
    this._ended = false;
    return true;
  }
  // The effect's keyframes or timing changed: restart the engine's copy where it was.
  _rebuild() {
    if (!this._id) return;
    var at = n.animation(this._id, 'time'), state = n.animation(this._id, 'state');
    n.animation(this._id, 'cancel');
    delete liveAnimations[this._id];
    this._id = 0;
    if (state === undefined) return;
    if (this._start()) {
      if (typeof at === 'number') n.animation(this._id, 'seek', at);
      if (state === 2) n.animation(this._id, 'pause');
    }
  }
  _promise() {
    if (!this._finished) {
      var self = this;
      this._finished = new Promise(function (res, rej) { self._resolve = res; self._reject = rej; });
      this._finished.catch(function () {});
    }
    return this._finished;
  }
  // The engine reports that the animation reached its end.
  _done() {
    if (this._ended) return;
    this._ended = true;
    var end = this.effect ? this.effect.getComputedTiming().endTime : 0;
    this._lastTime = this._rate < 0 ? 0 : end;
    var fill = this.effect ? this.effect._timing.fill : 'none';
    if (fill !== 'forwards' && fill !== 'both') { if (this._id) delete liveAnimations[this._id]; }
    this._promise();
    this._resolve(this);
    var ev = new Event('finish');
    ev.currentTime = this._lastTime; ev.timelineTime = n.now();
    dispatch(this, ev);
  }
  play() {
    if (this._id && n.animation(this._id, 'state') !== undefined) n.animation(this._id, 'play');
    else if (!this._start()) return;
    if (this._ended) { this._ended = false; this._finished = null; }
  }
  pause() { if (this._start()) n.animation(this._id, 'pause'); }
  finish() {
    if (!this._start()) return;
    n.animation(this._id, 'finish');
    this._done();
  }
  cancel() {
    var was = this.playState;
    if (this._id) { n.animation(this._id, 'cancel'); delete liveAnimations[this._id]; this._id = 0; }
    this._lastTime = null;
    this._ended = false;
    if (was === 'idle') return;
    if (this._finished && this._reject) {
      this._reject(new DOMException('The animation was cancelled', 'AbortError'));
    }
    this._finished = null;
    var ev = new Event('cancel');
    ev.currentTime = null; ev.timelineTime = n.now();
    dispatch(this, ev);
  }
  reverse() {
    this._rate = -this._rate;
    if (!this._start()) return;
    n.animation(this._id, 'setRate', this._rate);
    n.animation(this._id, 'play');
    if (this._ended) { this._ended = false; this._finished = null; }
  }
  persist() {}
  commitStyles() {
    var e = this.effect;
    if (!e || !e.target) return;
    var cs = new ComputedStyle(e.target), seen = {};
    for (var i = 0; i < e._frames.length; i++) {
      for (var k in e._frames[i].props) {
        if (seen[k]) continue;
        seen[k] = true;
        var v = cs.getPropertyValue(k);
        if (v !== '') e.target.style.setProperty(k, v);
      }
    }
  }
  updatePlaybackRate(r) { this.playbackRate = r; }
  get playbackRate() { return this._rate; }
  set playbackRate(r) {
    r = Number(r);
    if (r !== r) return;
    this._rate = r;
    if (this._id && r !== 0) n.animation(this._id, 'setRate', r);
  }
  get currentTime() {
    if (this._id) { var t = n.animation(this._id, 'time'); if (typeof t === 'number') return t; }
    return this._lastTime;
  }
  set currentTime(v) {
    if (v === null || v === undefined) return;
    if (this._start()) { n.animation(this._id, 'seek', Number(v)); if (this._ended) { this._ended = false; this._finished = null; } }
  }
  get startTime() {
    if (this._id) { var s = n.animation(this._id, 'start'); if (typeof s === 'number') return s; }
    return null;
  }
  set startTime(v) {
    if (v === null || v === undefined || !this._start()) return;
    n.animation(this._id, 'seek', (n.now() - Number(v)) * this._rate);
  }
  get playState() {
    if (this._id) {
      var s = n.animation(this._id, 'state');
      if (s === 1) return 'running';
      if (s === 2) return 'paused';
      if (s === 3) return 'finished';
    }
    return this._ended ? 'finished' : 'idle';
  }
  get pending() { return false; }
  get replaceState() { return 'active'; }
  get ready() { return Promise.resolve(this); }
  get finished() { return this._promise(); }
}

function elementAnimate(el, keyframes, options) {
  var effect = new KeyframeEffect(el, keyframes, options);
  var anim = new Animation(effect);
  if (options && typeof options === 'object' && options.id !== undefined) anim.id = String(options.id);
  anim.play();
  return anim;
}
function animationsOf(el, subtree) {
  var out = [];
  for (var id in liveAnimations) {
    var a = liveAnimations[id], t = a.effect && a.effect.target;
    if (!t || n.animation(a._id, 'state') === undefined) continue;
    if (!el || t === el || (subtree && el.contains(t))) out.push(a);
  }
  return out;
}
// Script animations that reached their end (their engine ids).
function animationsDone(ids) {
  for (var i = 0; i < ids.length; i++) {
    var a = liveAnimations[ids[i]];
    if (a) { try { a._done(); } catch (e) { reportError(e); } }
  }
}
// A CSS transition or animation started or ended.
function cssAnimationEvent(target, type, name, elapsed) {
  var init = { bubbles: true, cancelable: false, elapsedTime: elapsed, pseudoElement: '' };
  if (type.slice(0, 10) === 'transition') init.propertyName = name; else init.animationName = name;
  var C = type.slice(0, 10) === 'transition' ? TransitionEvent : AnimationEvent;
  var ev = new C(type, init);
  ev.isTrusted = true;
  dispatch(target, ev);
}
