//! Native primitives behind `<canvas>`: creating a context and running its calls.
//!
//! Both kinds of context (2D in `invisium-canvas`, WebGL in `invisium-gl`) are driven through
//! one generic entry each, `call(op, args)`, so this file knows nothing about individual
//! methods: it turns script values into arguments and results back into script values. The
//! only knowledge it adds is CSS: colours and fonts are parsed here, with the page's fonts.

use super::natives::{arg, def, node, Cx, NRes, Native};
use crate::css::properties::{self, PropId, Value as CssValue};
use crate::css::tokenizer::{tokenize_value, Cur};
use crate::css::values::{parse_color, CssColor, Len};
use crate::dom::NodeId;
use crate::page::canvas::{CanvasCtx, CanvasSlot, MAX_CANVASES, MAX_GL_CONTEXTS};
use invisium_canvas as c2d;
use invisium_draw::gl::{Target, TexFormat};
use invisium_gl as gl;
use invisium_js::{ClassId, Runtime, Thrown, Value};
use std::rc::Rc;

/// Slots of `HostState::canvas_refs` (filled once by `canvasSetup`).
const REF_F32: usize = 0;
const REF_I32: usize = 1;
const REF_U32: usize = 2;
const REF_U8: usize = 3;
const REF_U8C: usize = 4;
const REF_IMAGE_DATA: usize = 5;
const REF_DOM_EXCEPTION: usize = 6;
const REF_GL_PROTOS: usize = 7;
const REF_C2D_PROTOS: usize = REF_GL_PROTOS + GL_KINDS.len();

const GL_KINDS: [gl::Kind; 12] = [
    gl::Kind::Buffer,
    gl::Kind::Texture,
    gl::Kind::Renderbuffer,
    gl::Kind::Framebuffer,
    gl::Kind::Shader,
    gl::Kind::Program,
    gl::Kind::UniformLocation,
    gl::Kind::VertexArray,
    gl::Kind::Sampler,
    gl::Kind::Query,
    gl::Kind::Sync,
    gl::Kind::TransformFeedback,
];
const C2D_KINDS: [c2d::Kind; 3] = [c2d::Kind::Gradient, c2d::Kind::Pattern, c2d::Kind::Path];

/// Host-object id of a context's object: the canvas element, the kind and the handle.
fn pack(node: NodeId, kind: usize, handle: u32) -> u64 {
    ((node as u64) << 40) | ((kind as u64) << 32) | handle as u64
}
fn unpack(id: u64) -> (NodeId, usize, u32) {
    ((id >> 40) as NodeId, ((id >> 32) & 0xff) as usize, id as u32)
}

/// One script argument, taken out of the engine.
enum A {
    Undef,
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Bytes(gl::Elem, Vec<u8>),
    Nums(Vec<f64>),
    GlObj(gl::Kind, u32),
    C2dObj(c2d::Kind, u32),
    Pixels(u32, u32, Vec<u8>),
    Color([u8; 4]),
    Font(invisium_draw::FontId, f32, String),
}

fn elem_kind(rt: &mut Runtime, v: Value) -> gl::Elem {
    let ctor = rt.get(v, "constructor").unwrap_or(Value::Undefined);
    let name = rt.get(ctor, "name").unwrap_or(Value::Undefined);
    match rt.to_string_lossy(name).as_str() {
        "Int8Array" => gl::Elem::I8,
        "Uint8Array" => gl::Elem::U8,
        "Uint8ClampedArray" => gl::Elem::U8Clamped,
        "Int16Array" => gl::Elem::I16,
        "Uint16Array" => gl::Elem::U16,
        "Int32Array" => gl::Elem::I32,
        "Uint32Array" => gl::Elem::U32,
        "Float32Array" => gl::Elem::F32,
        "Float64Array" => gl::Elem::F64,
        _ => gl::Elem::Raw,
    }
}

/// The picture of an element used as a pixel source (`texImage2D`, `drawImage`): another
/// canvas. Images and video frames are not available to scripts yet.
fn node_pixels(cx: &Cx, source: NodeId) -> Option<(u32, u32, Vec<u8>)> {
    let mut st = cx.st.borrow_mut();
    let dev = st.page.gl_device.clone();
    let slot = st.page.canvases.get_mut(&source)?;
    match &mut slot.ctx {
        CanvasCtx::D2(c) => {
            let (w, h) = c.size();
            let mut px = Vec::new();
            c.read_rgba(&mut px);
            Some((w, h, px))
        }
        CanvasCtx::Gl(c) => {
            let dev = dev?;
            let mut d = dev.borrow_mut();
            c.flush(&mut *d);
            let (w, h) = c.drawing_buffer_size();
            let (format, bytes) = d.read_pixels(c.id(), &Target::Surface, 0, [0, 0, w as i32, h as i32])?;
            if format != TexFormat::Rgba8 || bytes.len() != w as usize * h as usize * 4 {
                return None;
            }
            // the device's row 0 is the bottom row; pixel sources are top row first
            let row = w as usize * 4;
            let mut px = Vec::with_capacity(bytes.len());
            for y in (0..h as usize).rev() {
                px.extend_from_slice(&bytes[y * row..(y + 1) * row]);
            }
            Some((w, h, px))
        }
    }
}

fn take(rt: &mut Runtime, cx: &Cx, canvas: NodeId, v: Value) -> A {
    match v {
        Value::Undefined => A::Undef,
        Value::Null => A::Null,
        Value::Bool(b) => A::Bool(b),
        Value::Number(n) => A::Num(n),
        Value::String(_) => A::Str(rt.to_string_lossy(v)),
        Value::Object(_) => {
            if let Some((class, id)) = rt.host_id(v) {
                if class == cx.gl_class || class == cx.c2d_class {
                    let (owner, kind, handle) = unpack(id);
                    // an object of another context is no object of this one
                    let handle = if owner == canvas { handle } else { u32::MAX };
                    return if class == cx.gl_class {
                        GL_KINDS.get(kind).map_or(A::Undef, |k| A::GlObj(*k, handle))
                    } else {
                        C2D_KINDS.get(kind).map_or(A::Undef, |k| A::C2dObj(*k, handle))
                    };
                }
                if class == cx.class {
                    return match node_pixels(cx, id as NodeId) {
                        Some((w, h, px)) => A::Pixels(w, h, px),
                        None => A::Null,
                    };
                }
                return A::Undef;
            }
            if let Some(bytes) = rt.buffer_bytes(v) {
                return A::Bytes(elem_kind(rt, v), bytes);
            }
            let len = rt.get(v, "length").unwrap_or(Value::Undefined);
            if let Value::Number(n) = len {
                let n = (n.max(0.0) as u32).min(1 << 22);
                let mut out = Vec::with_capacity(n as usize);
                for i in 0..n {
                    let item = rt.get_index(v, i).unwrap_or(Value::Undefined);
                    out.push(rt.to_number(item));
                }
                return A::Nums(out);
            }
            // an ImageData: { data, width, height }
            let data = rt.get(v, "data").unwrap_or(Value::Undefined);
            if let Some(px) = rt.buffer_bytes(data) {
                let wv = rt.get(v, "width").unwrap_or(Value::Undefined);
                let hv = rt.get(v, "height").unwrap_or(Value::Undefined);
                let (w, h) = (rt.to_number(wv), rt.to_number(hv));
                if w >= 1.0 && h >= 1.0 && (w * h * 4.0) as usize == px.len() {
                    return A::Pixels(w as u32, h as u32, px);
                }
            }
            A::Undef
        }
        _ => A::Undef,
    }
}

/// The 2D ops whose argument is CSS text this file has to parse first.
struct CssOps {
    /// (op, index of the colour argument)
    colors: Vec<(u16, usize)>,
    font: u16,
}

fn css_ops() -> &'static CssOps {
    static OPS: std::sync::OnceLock<CssOps> = std::sync::OnceLock::new();
    OPS.get_or_init(|| {
        let mut colors = Vec::new();
        let mut font = u16::MAX;
        for &(name, _, set) in c2d::properties() {
            match name {
                "fillStyle" | "strokeStyle" | "shadowColor" => colors.push((set, 0)),
                "font" => font = set,
                _ => {}
            }
        }
        for &(name, op) in c2d::object_methods(c2d::Kind::Gradient) {
            if name == "addColorStop" {
                colors.push((op, 2));
            }
        }
        CssOps { colors, font }
    })
}

fn css_color(cx: &Cx, canvas: NodeId, text: &str) -> Option<[u8; 4]> {
    let toks = tokenize_value(text);
    let mut cur = Cur::new(&toks);
    let c = parse_color(&mut cur)?;
    if !cur.at_end() {
        return None;
    }
    Some(match c {
        CssColor::Rgba(c) => [c.r, c.g, c.b, c.a],
        CssColor::Current => {
            // the canvas element's own colour: its style has to be up to date for that
            let mut st = cx.st.borrow_mut();
            let st = &mut *st;
            st.page.force_layout(&mut st.fx);
            let c = st.page.color_of(canvas);
            [c.r, c.g, c.b, c.a]
        }
    })
}

/// Parse a `font` shorthand and resolve it against the page's fonts.
fn css_font(cx: &Cx, canvas: NodeId, text: &str) -> Option<(invisium_draw::FontId, f32)> {
    let toks = tokenize_value(text);
    let mut decls = Vec::new();
    let no_urls = |_: &str| None;
    properties::parse_value(PropId::Font, &toks, false, &no_urls, &mut decls);
    let (mut italic, mut weight, mut size, mut families) = (false, 400.0f32, None, None);
    for d in decls {
        match (d.prop, d.value) {
            (PropId::FontStyle, CssValue::Kw(k)) => italic = matches!(k, properties::Kw::Italic | properties::Kw::Oblique),
            (PropId::FontWeight, CssValue::Num(n)) => weight = n,
            (PropId::FontSize, CssValue::Len(l)) => size = Some(l),
            (PropId::FontFamily, CssValue::Families(f)) => families = Some(f),
            _ => {}
        }
    }
    // lengths relative to the element's font need its style
    if matches!(size, Some(Len::Em(_)) | Some(Len::Pct(_))) {
        let mut st = cx.st.borrow_mut();
        let st = &mut *st;
        st.page.force_layout(&mut st.fx);
    }
    let st = cx.st.borrow();
    let em = st.page.styles.get(canvas).map_or(16.0, |s| s.font_size);
    let px = match size? {
        Len::Px(v) => v,
        Len::Em(v) => v * em,
        Len::Rem(v) => v * 16.0,
        Len::Pct(v) => v / 100.0 * em,
        _ => return None,
    };
    if !(px.is_finite() && px >= 0.0) {
        return None;
    }
    let fonts = st.page.fonts.clone()?;
    let mut fonts = fonts.try_borrow_mut().ok()?;
    fonts.set_scope(st.page.font_scope);
    Some((fonts.resolve(&families?, weight.clamp(1.0, 1000.0) as u16, italic), px))
}

struct FontBridge<'a>(&'a mut crate::text::FontSystem);

impl c2d::Fonts for FontBridge<'_> {
    fn metrics(&mut self, font: invisium_draw::FontId, size: f32) -> invisium_draw::FontMetrics {
        invisium_draw::TextShaper::metrics(self.0, font, size)
    }
    fn shape(&mut self, font: invisium_draw::FontId, size: f32, text: &str, out: &mut Vec<invisium_draw::Glyph>) -> f32 {
        invisium_draw::TextShaper::shape(self.0, font, size, text, out)
    }
    fn rasterize(&mut self, face: u16, glyph: u16, px_size: f32, subpx: u8) -> Option<invisium_draw::GlyphBitmap> {
        invisium_draw::GlyphSource::rasterize(&*self.0, face, glyph, px_size, subpx)
    }
}

fn reference(cx: &Cx, slot: usize) -> Value {
    cx.st.borrow().canvas_refs.get(slot).copied().unwrap_or(Value::Undefined)
}

fn typed(rt: &mut Runtime, cx: &Cx, slot: usize, bytes: &[u8]) -> Value {
    let buf = rt.array_buffer(bytes);
    let ctor = reference(cx, slot);
    rt.construct(ctor, &[buf]).unwrap_or(Value::Null)
}

fn num_array(rt: &mut Runtime, items: impl Iterator<Item = f64>) -> Value {
    let vals: Vec<Value> = items.map(Value::Number).collect();
    rt.array(&vals)
}

fn obj_with(rt: &mut Runtime, fields: &[(&str, Value)]) -> Value {
    let o = rt.object();
    for (k, v) in fields {
        let _ = rt.set(o, k, *v);
    }
    o
}

fn gl_object(rt: &mut Runtime, cx: &Cx, canvas: NodeId, kind: gl::Kind, handle: u32) -> Value {
    let Some(k) = GL_KINDS.iter().position(|x| *x == kind) else { return Value::Null };
    match reference(cx, REF_GL_PROTOS + k) {
        p @ Value::Object(_) => rt.host_object_with_proto(cx.gl_class, pack(canvas, k, handle), p),
        _ => rt.host_object(cx.gl_class, pack(canvas, k, handle)),
    }
}

fn gl_ret(rt: &mut Runtime, cx: &Cx, canvas: NodeId, list: Value, r: gl::Ret) -> Value {
    match r {
        gl::Ret::Undefined | gl::Ret::Extension(_) => Value::Undefined,
        gl::Ret::Null => Value::Null,
        gl::Ret::Bool(b) => Value::Bool(b),
        gl::Ret::Num(n) => Value::Number(n),
        gl::Ret::Str(s) => rt.string(&s),
        gl::Ret::Object(kind, handle) => gl_object(rt, cx, canvas, kind, handle),
        gl::Ret::F32s(v) => {
            let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            typed(rt, cx, REF_F32, &bytes)
        }
        gl::Ret::I32s(v) => {
            let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            typed(rt, cx, REF_I32, &bytes)
        }
        gl::Ret::U32s(v) => {
            let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            typed(rt, cx, REF_U32, &bytes)
        }
        gl::Ret::Bools(v) => {
            let vals: Vec<Value> = v.into_iter().map(Value::Bool).collect();
            rt.array(&vals)
        }
        gl::Ret::Strs(v) => {
            let vals: Vec<Value> = v.iter().map(|s| rt.string(s)).collect();
            rt.array(&vals)
        }
        gl::Ret::Objects(kind, v) => {
            let vals: Vec<Value> = v.into_iter().map(|h| gl_object(rt, cx, canvas, kind, h)).collect();
            rt.array(&vals)
        }
        gl::Ret::ActiveInfo { name, size, gl_type } => {
            let n = rt.string(&name);
            obj_with(rt, &[("name", n), ("size", Value::Number(size as f64)), ("type", Value::Number(gl_type as f64))])
        }
        gl::Ret::Precision { range_min, range_max, precision } => obj_with(
            rt,
            &[("rangeMin", Value::Number(range_min as f64)), ("rangeMax", Value::Number(range_max as f64)), ("precision", Value::Number(precision as f64))],
        ),
        gl::Ret::Attributes(a) => {
            let power = rt.string("default");
            obj_with(
                rt,
                &[
                    ("alpha", Value::Bool(a.alpha)),
                    ("depth", Value::Bool(a.depth)),
                    ("stencil", Value::Bool(a.stencil)),
                    ("antialias", Value::Bool(a.antialias)),
                    ("premultipliedAlpha", Value::Bool(a.premultiplied_alpha)),
                    ("preserveDrawingBuffer", Value::Bool(a.preserve_drawing_buffer)),
                    ("powerPreference", power),
                    ("failIfMajorPerformanceCaveat", Value::Bool(false)),
                    ("desynchronized", Value::Bool(false)),
                    ("xrCompatible", Value::Bool(false)),
                ],
            )
        }
        gl::Ret::Fill { arg: at, offset, bytes } => {
            // copy into the view the script passed: new Uint8Array(buffer, start, n).set(bytes)
            let target = rt.get_index(list, at as u32).unwrap_or(Value::Undefined);
            let (buffer, base) = match rt.get(target, "buffer") {
                Ok(b @ Value::Object(_)) => {
                    let off = rt.get(target, "byteOffset").unwrap_or(Value::Undefined);
                    (b, rt.to_number(off).max(0.0))
                }
                _ => (target, 0.0),
            };
            let u8 = reference(cx, REF_U8);
            let start = Value::Number(base + offset as f64);
            let len = Value::Number(bytes.len() as f64);
            if let Ok(view) = rt.construct(u8, &[buffer, start, len]) {
                let src = typed(rt, cx, REF_U8, &bytes);
                if let Ok(set) = rt.get(view, "set") {
                    let _ = rt.call(set, view, &[src]);
                }
            }
            Value::Undefined
        }
    }
}

fn c2d_ret(rt: &mut Runtime, cx: &Cx, canvas: NodeId, r: c2d::Ret) -> NRes {
    Ok(match r {
        c2d::Ret::Undefined => Value::Undefined,
        c2d::Ret::Null => Value::Null,
        c2d::Ret::Bool(b) => Value::Bool(b),
        c2d::Ret::Num(n) => Value::Number(n),
        c2d::Ret::Str(s) => rt.string(&s),
        c2d::Ret::Nums(v) => num_array(rt, v.into_iter()),
        c2d::Ret::Object(kind, handle) => {
            let Some(k) = C2D_KINDS.iter().position(|x| *x == kind) else { return Ok(Value::Null) };
            match reference(cx, REF_C2D_PROTOS + k) {
                p @ Value::Object(_) => rt.host_object_with_proto(cx.c2d_class, pack(canvas, k, handle), p),
                _ => rt.host_object(cx.c2d_class, pack(canvas, k, handle)),
            }
        }
        c2d::Ret::Pixels { width, height, rgba } => {
            let data = typed(rt, cx, REF_U8C, &rgba);
            let ctor = reference(cx, REF_IMAGE_DATA);
            rt.construct(ctor, &[data, Value::Number(width as f64), Value::Number(height as f64)]).unwrap_or(Value::Null)
        }
        c2d::Ret::Text(m) => obj_with(
            rt,
            &[
                ("width", Value::Number(m.width)),
                ("actualBoundingBoxLeft", Value::Number(m.actual_left)),
                ("actualBoundingBoxRight", Value::Number(m.actual_right)),
                ("fontBoundingBoxAscent", Value::Number(m.font_ascent)),
                ("fontBoundingBoxDescent", Value::Number(m.font_descent)),
                ("actualBoundingBoxAscent", Value::Number(m.actual_ascent)),
                ("actualBoundingBoxDescent", Value::Number(m.actual_descent)),
                ("emHeightAscent", Value::Number(m.em_ascent)),
                ("emHeightDescent", Value::Number(m.em_descent)),
                ("hangingBaseline", Value::Number(m.hanging_baseline)),
                ("alphabeticBaseline", Value::Number(m.alphabetic_baseline)),
                ("ideographicBaseline", Value::Number(m.ideographic_baseline)),
            ],
        ),
        c2d::Ret::Matrix(m) => {
            let n = |i: usize| Value::Number(m[i]);
            obj_with(
                rt,
                &[
                    ("a", n(0)), ("b", n(1)), ("c", n(2)), ("d", n(3)), ("e", n(4)), ("f", n(5)),
                    ("m11", n(0)), ("m12", n(1)), ("m21", n(2)), ("m22", n(3)), ("m41", n(4)), ("m42", n(5)),
                    ("is2D", Value::Bool(true)),
                    ("isIdentity", Value::Bool(m == [1.0, 0.0, 0.0, 1.0, 0.0, 0.0])),
                ],
            )
        }
        c2d::Ret::Throw(name, message) => {
            if name == "TypeError" {
                return Err(super::natives::err(rt, "TypeError", &message));
            }
            let ctor = reference(cx, REF_DOM_EXCEPTION);
            let (m, n) = (rt.string(&message), rt.string(name));
            let e = rt.construct(ctor, &[m, n]).unwrap_or(Value::Undefined);
            return Err(Thrown(e));
        }
    })
}

/// What kind of context a canvas has: 1 = 2D, 2 = WebGL 1, 3 = WebGL 2, 0 = none.
fn context_kind(slot: Option<&CanvasSlot>) -> u8 {
    match slot.map(|s| &s.ctx) {
        None => 0,
        Some(CanvasCtx::D2(_)) => 1,
        Some(CanvasCtx::Gl(c)) => match c.version() {
            gl::Version::WebGl1 => 2,
            gl::Version::WebGl2 => 3,
        },
    }
}

/// `canvasContext(canvas, type, alpha, depth, stencil, antialias, premultipliedAlpha,
/// preserveDrawingBuffer)` → 1 (2D), 2 (WebGL 1), 3 (WebGL 2) or null. A canvas keeps the
/// kind of context it was first asked for.
fn n_canvas_context(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let canvas = node(rt, cx, arg(args, 0))?;
    let want = match rt.to_string_lossy(arg(args, 1)).as_str() {
        "2d" => 1u8,
        "webgl" | "experimental-webgl" => 2,
        "webgl2" => 3,
        _ => return Ok(Value::Null),
    };
    let flag = |rt: &mut Runtime, i: usize, default: bool| match arg(args, i) {
        Value::Undefined => default,
        v => rt.to_boolean(v),
    };
    let d = gl::Attributes::default();
    let attrs = gl::Attributes {
        alpha: flag(rt, 2, d.alpha),
        depth: flag(rt, 3, d.depth),
        stencil: flag(rt, 4, d.stencil),
        antialias: flag(rt, 5, d.antialias),
        premultiplied_alpha: flag(rt, 6, d.premultiplied_alpha),
        preserve_drawing_buffer: flag(rt, 7, d.preserve_drawing_buffer),
    };
    let mut st = cx.st.borrow_mut();
    let page = &mut st.page;
    if page.doc.tag(canvas) != crate::atom::a::CANVAS {
        return Ok(Value::Null);
    }
    let have = context_kind(page.canvases.get(&canvas));
    if have != 0 {
        return Ok(if have == want { Value::Number(have as f64) } else { Value::Null });
    }
    if page.canvases.len() >= MAX_CANVASES {
        return Ok(Value::Null);
    }
    let size = page.canvas_attr_size(canvas);
    if want == 1 {
        let ctx = CanvasCtx::D2(Box::new(c2d::Canvas2d::new(size.0, size.1)));
        page.canvases.insert(canvas, CanvasSlot { ctx, size, upload: true });
        return Ok(Value::Number(1.0));
    }
    let Some(dev) = page.gl_device.clone() else { return Ok(Value::Null) };
    let live = page.canvases.values().filter(|s| matches!(s.ctx, CanvasCtx::Gl(_))).count();
    if live >= MAX_GL_CONTEXTS {
        // Too many contexts: the oldest one goes, as in other browsers. Pages make throwaway
        // canvases just to ask whether WebGL exists, so one that is not in the document is
        // taken first.
        let in_document = |mut n: NodeId| {
            while n != crate::dom::NO_NODE && n != crate::dom::DOCUMENT_NODE {
                n = page.doc.parent(n);
            }
            n == crate::dom::DOCUMENT_NODE
        };
        let victim = page
            .canvases
            .iter()
            .filter_map(|(n, s)| match &s.ctx {
                CanvasCtx::Gl(c) => Some((in_document(*n), c.id(), *n)),
                _ => None,
            })
            .min();
        let Some((_, _, node)) = victim else { return Ok(Value::Null) };
        if let Some(CanvasSlot { ctx: CanvasCtx::Gl(mut c), .. }) = page.canvases.remove(&node) {
            c.destroy(&mut *dev.borrow_mut());
        }
        page.images.remove(&node);
        page.dirty_paint = true;
    }
    let Some(id) = dev.borrow_mut().create() else { return Ok(Value::Null) };
    let version = if want == 3 { gl::Version::WebGl2 } else { gl::Version::WebGl1 };
    let ctx = CanvasCtx::Gl(Box::new(gl::Context::new(id, version, attrs, size.0, size.1)));
    page.canvases.insert(canvas, CanvasSlot { ctx, size, upload: false });
    page.attach_gl_image(canvas, id, size);
    Ok(Value::Number(want as f64))
}

/// Run `op` with `first` (an object argument, for methods of gradients and paths) followed by
/// the items of the array-like `list`.
fn run_call(rt: &mut Runtime, cx: &Cx, canvas: NodeId, op: u16, first: Option<A>, list: Value) -> NRes {
    let kind = context_kind(cx.st.borrow().page.canvases.get(&canvas));
    if kind == 0 {
        return Ok(Value::Undefined);
    }
    let len_v = rt.get(list, "length").unwrap_or(Value::Undefined);
    let len = (rt.to_number(len_v).max(0.0) as u32).min(32);
    let mut owned: Vec<A> = Vec::with_capacity(len as usize + 1);
    owned.extend(first);
    for i in 0..len {
        let v = rt.get_index(list, i).unwrap_or(Value::Undefined);
        owned.push(take(rt, cx, canvas, v));
    }
    if kind == 1 {
        // CSS text becomes parsed values; an assignment that does not parse is dropped
        let ops = css_ops();
        if let Some(&(_, at)) = ops.colors.iter().find(|c| c.0 == op) {
            if let Some(A::Str(text)) = owned.get(at) {
                match css_color(cx, canvas, text) {
                    Some(c) => owned[at] = A::Color(c),
                    None => return Ok(Value::Undefined),
                }
            }
        }
        if op == ops.font {
            let Some(A::Str(text)) = owned.first() else { return Ok(Value::Undefined) };
            let text = text.trim().to_string();
            match css_font(cx, canvas, &text) {
                Some((font, size)) => owned[0] = A::Font(font, size, text),
                None => return Ok(Value::Undefined),
            }
        }
        let args: Vec<c2d::Arg> = owned
            .iter()
            .map(|a| match a {
                A::Undef | A::GlObj(..) | A::Bytes(..) => c2d::Arg::Undefined,
                A::Null => c2d::Arg::Null,
                A::Bool(b) => c2d::Arg::Bool(*b),
                A::Num(n) => c2d::Arg::Num(*n),
                A::Str(s) => c2d::Arg::Str(s),
                A::Nums(v) => c2d::Arg::Nums(v),
                A::C2dObj(k, h) => c2d::Arg::Object(*k, *h),
                A::Pixels(w, h, px) => c2d::Arg::Pixels { width: *w, height: *h, rgba: px },
                A::Color(c) => c2d::Arg::Color(*c),
                A::Font(f, size, css) => c2d::Arg::Font { font: *f, size: *size, css },
            })
            .collect();
        let ret = {
            let mut st = cx.st.borrow_mut();
            let Some(fonts) = st.page.fonts.clone() else { return Ok(Value::Undefined) };
            let Ok(mut fonts) = fonts.try_borrow_mut() else { return Ok(Value::Undefined) };
            fonts.set_scope(st.page.font_scope);
            match st.page.canvases.get_mut(&canvas).map(|s| &mut s.ctx) {
                Some(CanvasCtx::D2(c)) => c.call(&mut FontBridge(&mut fonts), op, &args),
                _ => c2d::Ret::Undefined,
            }
        };
        return c2d_ret(rt, cx, canvas, ret);
    }
    let args: Vec<gl::Arg> = owned
        .iter()
        .map(|a| match a {
            A::Undef | A::C2dObj(..) | A::Color(_) | A::Font(..) => gl::Arg::Undefined,
            A::Null => gl::Arg::Null,
            A::Bool(b) => gl::Arg::Bool(*b),
            A::Num(n) => gl::Arg::Num(*n),
            A::Str(s) => gl::Arg::Str(s),
            A::Bytes(e, b) => gl::Arg::Bytes(*e, b),
            A::Nums(v) => gl::Arg::Nums(v),
            A::GlObj(k, h) => gl::Arg::Object(*k, *h),
            A::Pixels(w, h, px) => gl::Arg::Pixels { width: *w, height: *h, rgba: px },
        })
        .collect();
    dump_shader(kind, op, &args);
    // with profiling on, every WebGL method is timed under its own name
    let _t = crate::draw::prof::enabled().then(|| {
        let version = if kind == 3 { gl::Version::WebGl2 } else { gl::Version::WebGl1 };
        let name = gl::methods(version).iter().find(|m| m.1 == op).map_or("gl: ?", |m| m.0);
        crate::draw::prof::span(name)
    });
    let ret = {
        let mut st = cx.st.borrow_mut();
        let Some(dev) = st.page.gl_device.clone() else { return Ok(Value::Undefined) };
        let Ok(mut dev) = dev.try_borrow_mut() else { return Ok(Value::Undefined) };
        match st.page.canvases.get_mut(&canvas).map(|s| &mut s.ctx) {
            Some(CanvasCtx::Gl(c)) => c.call(&mut *dev, op, &args),
            _ => gl::Ret::Undefined,
        }
    };
    Ok(gl_ret(rt, cx, canvas, list, ret))
}

/// Debugging aid: with `INVIS_GL_DUMP=<directory>` set, every source a page hands to
/// `shaderSource` is written there as `NNNN.glsl` (the engine is its own corpus collector for
/// the shader translator).
fn dump_shader(kind: u8, op: u16, args: &[gl::Arg]) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static DIR: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let Some(dir) = DIR.get_or_init(|| std::env::var_os("INVIS_GL_DUMP").map(std::path::PathBuf::from)) else { return };
    let version = if kind == 3 { gl::Version::WebGl2 } else { gl::Version::WebGl1 };
    if gl::method_op(version, "shaderSource") != Some(op) {
        return;
    }
    if let Some(gl::Arg::Str(src)) = args.get(1) {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let _ = std::fs::write(dir.join(format!("{n:04}.glsl")), src);
    }
}

/// `canvasCall(canvas, op, args)`: one method or property access of the canvas's context.
fn n_canvas_call(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let canvas = node(rt, cx, arg(args, 0))?;
    let op = rt.to_number(arg(args, 1)) as u16;
    run_call(rt, cx, canvas, op, None, arg(args, 2))
}

/// `canvasObjCall(object, op, args)`: a method of a gradient, pattern or path. The object
/// itself becomes the first argument.
fn n_canvas_obj_call(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let Some((class, id)) = rt.host_id(arg(args, 0)) else { return Ok(Value::Undefined) };
    if class != cx.c2d_class {
        return Ok(Value::Undefined);
    }
    let (canvas, kind, handle) = unpack(id);
    let Some(kind) = C2D_KINDS.get(kind) else { return Ok(Value::Undefined) };
    let op = rt.to_number(arg(args, 1)) as u16;
    run_call(rt, cx, canvas, op, Some(A::C2dObj(*kind, handle)), arg(args, 2))
}

fn flat_table(rt: &mut Runtime, items: &[(&str, u16)]) -> Value {
    let mut vals = Vec::with_capacity(items.len() * 2);
    for (name, op) in items {
        vals.push(rt.string(name));
        vals.push(Value::Number(*op as f64));
    }
    rt.array(&vals)
}

/// `canvasTables(kind)`: the names scripts see. For 2D: [methods, properties (name, read op,
/// write op), gradient methods, pattern methods, path methods]; for WebGL (2 or 3):
/// [methods, constants].
fn n_canvas_tables(rt: &mut Runtime, _cx: &Cx, args: &[Value]) -> NRes {
    let kind = rt.to_number(arg(args, 0)) as u8;
    if kind == 1 {
        let methods = flat_table(rt, c2d::methods());
        let mut props = Vec::new();
        for (name, get, set) in c2d::properties() {
            props.push(rt.string(name));
            props.push(Value::Number(*get as f64));
            props.push(Value::Number(*set as f64));
        }
        let props = rt.array(&props);
        let grad = flat_table(rt, c2d::object_methods(c2d::Kind::Gradient));
        let pattern = flat_table(rt, c2d::object_methods(c2d::Kind::Pattern));
        let path = flat_table(rt, c2d::object_methods(c2d::Kind::Path));
        return Ok(rt.array(&[methods, props, grad, pattern, path]));
    }
    let version = if kind == 3 { gl::Version::WebGl2 } else { gl::Version::WebGl1 };
    let methods = flat_table(rt, gl::methods(version));
    let mut consts = Vec::new();
    for (name, value) in gl::constants(version) {
        consts.push(rt.string(name));
        // the one negative constant of the interface does not fit the unsigned table
        consts.push(Value::Number(if *name == "TIMEOUT_IGNORED" { -1.0 } else { *value as f64 }));
    }
    let consts = rt.array(&consts);
    Ok(rt.array(&[methods, consts]))
}

/// `canvasExtension(canvas, name)`: `getExtension`. → null, or [name, constants, methods].
fn n_canvas_extension(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let canvas = node(rt, cx, arg(args, 0))?;
    let name = rt.to_string_lossy(arg(args, 1));
    let found = {
        let mut st = cx.st.borrow_mut();
        let Some(dev) = st.page.gl_device.clone() else { return Ok(Value::Null) };
        let Ok(mut dev) = dev.try_borrow_mut() else { return Ok(Value::Null) };
        match st.page.canvases.get_mut(&canvas).map(|s| &mut s.ctx) {
            Some(CanvasCtx::Gl(c)) => {
                let version = c.version();
                let Some(op) = gl::method_op(version, "getExtension") else { return Ok(Value::Null) };
                match c.call(&mut *dev, op, &[gl::Arg::Str(&name)]) {
                    gl::Ret::Extension(n) => gl::extension(version, n).map(|info| (n, info)),
                    _ => None,
                }
            }
            _ => None,
        }
    };
    let Some((n, info)) = found else { return Ok(Value::Null) };
    let name = rt.string(n);
    let mut consts = Vec::new();
    for (k, v) in info.constants {
        consts.push(rt.string(k));
        consts.push(Value::Number(*v as f64));
    }
    let consts = rt.array(&consts);
    let methods = flat_table(rt, info.methods);
    Ok(rt.array(&[name, consts, methods]))
}

/// `canvasResized(canvas)`: its `width` or `height` was assigned.
fn n_canvas_resized(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let canvas = node(rt, cx, arg(args, 0))?;
    cx.st.borrow_mut().page.canvas_resized(canvas);
    Ok(Value::Undefined)
}

/// `canvasBufferSize(canvas)` → [width, height] of the bitmap or drawing buffer.
fn n_canvas_buffer_size(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let canvas = node(rt, cx, arg(args, 0))?;
    let size = {
        let st = cx.st.borrow();
        match st.page.canvases.get(&canvas).map(|s| &s.ctx) {
            Some(CanvasCtx::Gl(c)) => c.drawing_buffer_size(),
            Some(CanvasCtx::D2(c)) => c.size(),
            None => st.page.canvas_attr_size(canvas),
        }
    };
    Ok(num_array(rt, [size.0 as f64, size.1 as f64].into_iter()))
}

/// `canvasSetup(refs)`: the script-side constructors and prototypes results are built with.
fn n_canvas_setup(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let list = arg(args, 0);
    let len_v = rt.get(list, "length").unwrap_or(Value::Undefined);
    let len = (rt.to_number(len_v).max(0.0) as u32).min(64);
    let mut refs = Vec::with_capacity(len as usize);
    for i in 0..len {
        let v = rt.get_index(list, i).unwrap_or(Value::Undefined);
        rt.root(v);
        refs.push(v);
    }
    cx.st.borrow_mut().canvas_refs = refs;
    Ok(Value::Undefined)
}

pub(crate) fn classes(rt: &mut Runtime) -> (ClassId, ClassId) {
    (rt.register_plain_host_class("WebGLObject"), rt.register_plain_host_class("CanvasObject"))
}

pub(crate) fn install(rt: &mut Runtime, n: Value, cx: &Rc<Cx>) {
    let table: &[(&str, u32, Native)] = &[
        ("canvasContext", 8, n_canvas_context),
        ("canvasCall", 3, n_canvas_call),
        ("canvasObjCall", 3, n_canvas_obj_call),
        ("canvasTables", 1, n_canvas_tables),
        ("canvasExtension", 2, n_canvas_extension),
        ("canvasResized", 1, n_canvas_resized),
        ("canvasBufferSize", 1, n_canvas_buffer_size),
        ("canvasSetup", 1, n_canvas_setup),
    ];
    for &(name, arity, f) in table {
        def(rt, n, cx, name, arity, f);
    }
}
