// <canvas>: getContext and the context classes.
//
// The contexts themselves live in native code (a 2D rasteriser, a WebGL state machine). Each
// has one generic entry point, so the classes here are built from tables the natives hand
// over: a method is "call op N with these arguments", a property is a pair of ops.

function ctxMethod(op) {
  return function () { return n.canvasCall(this._c, op, arguments); };
}
function objMethod(op) {
  return function () { return n.canvasObjCall(this, op, arguments); };
}
function noNew(name) {
  return function () { throw new TypeError("Failed to construct '" + name + "': Illegal constructor"); };
}

// Objects a WebGL context hands out. Scripts never construct them.
var GL_OBJECT_NAMES = ['WebGLBuffer', 'WebGLTexture', 'WebGLRenderbuffer', 'WebGLFramebuffer', 'WebGLShader', 'WebGLProgram',
  'WebGLUniformLocation', 'WebGLVertexArrayObject', 'WebGLSampler', 'WebGLQuery', 'WebGLSync', 'WebGLTransformFeedback'];
var glObjects = {};
GL_OBJECT_NAMES.forEach(function (name) {
  var C = noNew(name);
  Object.defineProperty(C, 'name', { value: name });
  Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  glObjects[name] = C;
});
var WebGLActiveInfo = noNew('WebGLActiveInfo');
var WebGLShaderPrecisionFormat = noNew('WebGLShaderPrecisionFormat');

var CanvasGradient = noNew('CanvasGradient');
var CanvasPattern = noNew('CanvasPattern');
Object.defineProperty(CanvasGradient.prototype, Symbol.toStringTag, { value: 'CanvasGradient', configurable: true });
Object.defineProperty(CanvasPattern.prototype, Symbol.toStringTag, { value: 'CanvasPattern', configurable: true });

class ImageData {
  constructor(a, b, c) {
    var data, width, height;
    if (typeof a === 'number') {
      width = a >>> 0; height = b >>> 0;
      if (!width || !height) throw new DOMException('The source width or height is zero.', 'IndexSizeError');
      data = new Uint8ClampedArray(width * height * 4);
    } else {
      data = a; width = b >>> 0;
      if (!(data instanceof Uint8ClampedArray)) throw new TypeError("Failed to construct 'ImageData': parameter 1 is not of type 'Uint8ClampedArray'");
      if (!width || data.length % 4 || (data.length / 4) % width) throw new DOMException('The input data length is not a multiple of (4 * width).', 'InvalidStateError');
      height = c === undefined ? data.length / 4 / width : c >>> 0;
      if (height * width * 4 !== data.length) throw new DOMException('The input data length is not equal to (4 * width * height).', 'IndexSizeError');
    }
    Object.defineProperty(this, 'data', { value: data, enumerable: true });
    Object.defineProperty(this, 'width', { value: width, enumerable: true });
    Object.defineProperty(this, 'height', { value: height, enumerable: true });
    Object.defineProperty(this, 'colorSpace', { value: 'srgb', enumerable: true });
  }
}

var CanvasRenderingContext2D = noNew('CanvasRenderingContext2D');
var WebGLRenderingContext = noNew('WebGLRenderingContext');
var WebGL2RenderingContext = noNew('WebGL2RenderingContext');
[[CanvasRenderingContext2D, 'CanvasRenderingContext2D'], [WebGLRenderingContext, 'WebGLRenderingContext'], [WebGL2RenderingContext, 'WebGL2RenderingContext']].forEach(function (p) {
  Object.defineProperty(p[0], 'name', { value: p[1] });
  Object.defineProperty(p[0].prototype, Symbol.toStringTag, { value: p[1], configurable: true });
  Object.defineProperty(p[0].prototype, 'canvas', { get: function () { return this._c; }, enumerable: true, configurable: true });
});

n.canvasSetup([Float32Array, Int32Array, Uint32Array, Uint8Array, Uint8ClampedArray, ImageData, DOMException].concat(
  GL_OBJECT_NAMES.map(function (name) { return glObjects[name].prototype; }),
  [CanvasGradient.prototype, CanvasPattern.prototype, null]));

var canvasBuilt = {};

// A DOMMatrix-like argument ({a..f} or {m11..m42}) as six numbers.
function matrixArgs(m) {
  var pick = function (x, y, d) { return m[x] !== undefined ? +m[x] : m[y] !== undefined ? +m[y] : d; };
  return [pick('a', 'm11', 1), pick('b', 'm12', 0), pick('c', 'm21', 0), pick('d', 'm22', 1), pick('e', 'm41', 0), pick('f', 'm42', 0)];
}

function build2d() {
  var proto = CanvasRenderingContext2D.prototype;
  var t = n.canvasTables(1), i;
  var methods = t[0], props = t[1];
  for (i = 0; i < methods.length; i += 2) proto[methods[i]] = ctxMethod(methods[i + 1]);
  for (i = 0; i < props.length; i += 3) (function (name, get, set) {
    Object.defineProperty(proto, name, {
      get: function () { return n.canvasCall(this._c, get, []); },
      set: function (v) { n.canvasCall(this._c, set, [v]); },
      enumerable: true, configurable: true,
    });
  })(props[i], props[i + 1], props[i + 2]);
  for (i = 0; i < t[2].length; i += 2) CanvasGradient.prototype[t[2][i]] = objMethod(t[2][i + 1]);
  for (i = 0; i < t[3].length; i += 2) CanvasPattern.prototype[t[3][i]] = objMethod(t[3][i + 1]);
  // setTransform also takes a matrix object
  var setTransform = proto.setTransform;
  if (setTransform) proto.setTransform = function (a) {
    if (arguments.length === 1 && a && typeof a === 'object') return setTransform.apply(this, matrixArgs(a));
    return setTransform.apply(this, arguments);
  };
}

function buildGl(Class, kind) {
  var proto = Class.prototype;
  var t = n.canvasTables(kind), i;
  var methods = t[0], consts = t[1];
  for (i = 0; i < methods.length; i += 2) proto[methods[i]] = ctxMethod(methods[i + 1]);
  for (i = 0; i < consts.length; i += 2) {
    var d = { value: consts[i + 1], enumerable: true };
    Object.defineProperty(proto, consts[i], d);
    Object.defineProperty(Class, consts[i], d);
  }
  // an extension is one object per context, built from the names the native side lists
  proto.getExtension = function (name) {
    var key = String(name).toLowerCase(), cache = this._ext;
    if (key in cache) return cache[key];
    var info = n.canvasExtension(this._c, String(name)), ext = null;
    if (info) {
      ext = {};
      var canvas = this._c, j;
      for (j = 0; j < info[1].length; j += 2) Object.defineProperty(ext, info[1][j], { value: info[1][j + 1], enumerable: true });
      for (j = 0; j < info[2].length; j += 2) (function (m, op) {
        ext[m] = function () { return n.canvasCall(canvas, op, arguments); };
      })(info[2][j], info[2][j + 1]);
    }
    cache[key] = ext;
    return ext;
  };
  Object.defineProperty(proto, 'drawingBufferWidth', { get: function () { return n.canvasBufferSize(this._c)[0]; }, enumerable: true, configurable: true });
  Object.defineProperty(proto, 'drawingBufferHeight', { get: function () { return n.canvasBufferSize(this._c)[1]; }, enumerable: true, configurable: true });
  Object.defineProperty(proto, 'drawingBufferColorSpace', { value: 'srgb', writable: true, enumerable: true, configurable: true });
  Object.defineProperty(proto, 'unpackColorSpace', { value: 'srgb', writable: true, enumerable: true, configurable: true });
  proto.makeXRCompatible = function () { return Promise.resolve(); };
}

HTMLCanvasElement.prototype.getContext = function (type, attrs) {
  var a = attrs !== null && typeof attrs === 'object' ? attrs : {};
  var kind = n.canvasContext(this, String(type), a.alpha, a.depth, a.stencil, a.antialias, a.premultipliedAlpha, a.preserveDrawingBuffer);
  if (!kind) return null;
  var ctx = this._ctx;
  if (!ctx) {
    var Class = kind === 1 ? CanvasRenderingContext2D : kind === 2 ? WebGLRenderingContext : WebGL2RenderingContext;
    if (!canvasBuilt[kind]) {
      canvasBuilt[kind] = true;
      if (kind === 1) build2d(); else buildGl(Class, kind);
    }
    ctx = Object.create(Class.prototype);
    hide(ctx, '_c', this);
    if (kind !== 1) hide(ctx, '_ext', {});
    hide(this, '_ctx', ctx);
  }
  return ctx;
};

// Assigning the size clears the bitmap and resets the context, also to the same value.
['width', 'height'].forEach(function (key) {
  var d = Object.getOwnPropertyDescriptor(HTMLCanvasElement.prototype, key);
  Object.defineProperty(HTMLCanvasElement.prototype, key, {
    get: d.get,
    set: function (v) { d.set.call(this, v); n.canvasResized(this); },
    enumerable: true, configurable: true,
  });
});
