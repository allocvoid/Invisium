//! Op numbers and the method / property tables.

use crate::Kind;

// Methods of CanvasRenderingContext2D.
pub const SAVE: u16 = 1;
pub const RESTORE: u16 = 2;
pub const RESET: u16 = 3;
pub const IS_CONTEXT_LOST: u16 = 4;
pub const SCALE: u16 = 5;
pub const ROTATE: u16 = 6;
pub const TRANSLATE: u16 = 7;
pub const TRANSFORM: u16 = 8;
pub const GET_TRANSFORM: u16 = 9;
pub const SET_TRANSFORM: u16 = 10;
pub const RESET_TRANSFORM: u16 = 11;
pub const CREATE_LINEAR_GRADIENT: u16 = 12;
pub const CREATE_RADIAL_GRADIENT: u16 = 13;
pub const CREATE_CONIC_GRADIENT: u16 = 14;
pub const CREATE_PATTERN: u16 = 15;
pub const CLEAR_RECT: u16 = 16;
pub const FILL_RECT: u16 = 17;
pub const STROKE_RECT: u16 = 18;
pub const BEGIN_PATH: u16 = 19;
pub const FILL: u16 = 20;
pub const STROKE: u16 = 21;
pub const CLIP: u16 = 22;
pub const IS_POINT_IN_PATH: u16 = 23;
pub const IS_POINT_IN_STROKE: u16 = 24;
pub const DRAW_FOCUS_IF_NEEDED: u16 = 25;
pub const FILL_TEXT: u16 = 26;
pub const STROKE_TEXT: u16 = 27;
pub const MEASURE_TEXT: u16 = 28;
pub const DRAW_IMAGE: u16 = 29;
pub const CREATE_IMAGE_DATA: u16 = 30;
pub const GET_IMAGE_DATA: u16 = 31;
pub const PUT_IMAGE_DATA: u16 = 32;
pub const SET_LINE_DASH: u16 = 33;
pub const GET_LINE_DASH: u16 = 34;
pub const CLOSE_PATH: u16 = 35;
pub const MOVE_TO: u16 = 36;
pub const LINE_TO: u16 = 37;
pub const QUADRATIC_CURVE_TO: u16 = 38;
pub const BEZIER_CURVE_TO: u16 = 39;
pub const ARC_TO: u16 = 40;
pub const RECT: u16 = 41;
pub const ROUND_RECT: u16 = 42;
pub const ARC: u16 = 43;
pub const ELLIPSE: u16 = 44;
pub const GET_CONTEXT_ATTRIBUTES: u16 = 45;

static METHODS: [(&str, u16); 45] = [
    ("save", SAVE),
    ("restore", RESTORE),
    ("reset", RESET),
    ("isContextLost", IS_CONTEXT_LOST),
    ("scale", SCALE),
    ("rotate", ROTATE),
    ("translate", TRANSLATE),
    ("transform", TRANSFORM),
    ("getTransform", GET_TRANSFORM),
    ("setTransform", SET_TRANSFORM),
    ("resetTransform", RESET_TRANSFORM),
    ("createLinearGradient", CREATE_LINEAR_GRADIENT),
    ("createRadialGradient", CREATE_RADIAL_GRADIENT),
    ("createConicGradient", CREATE_CONIC_GRADIENT),
    ("createPattern", CREATE_PATTERN),
    ("clearRect", CLEAR_RECT),
    ("fillRect", FILL_RECT),
    ("strokeRect", STROKE_RECT),
    ("beginPath", BEGIN_PATH),
    ("fill", FILL),
    ("stroke", STROKE),
    ("clip", CLIP),
    ("isPointInPath", IS_POINT_IN_PATH),
    ("isPointInStroke", IS_POINT_IN_STROKE),
    ("drawFocusIfNeeded", DRAW_FOCUS_IF_NEEDED),
    ("fillText", FILL_TEXT),
    ("strokeText", STROKE_TEXT),
    ("measureText", MEASURE_TEXT),
    ("drawImage", DRAW_IMAGE),
    ("createImageData", CREATE_IMAGE_DATA),
    ("getImageData", GET_IMAGE_DATA),
    ("putImageData", PUT_IMAGE_DATA),
    ("setLineDash", SET_LINE_DASH),
    ("getLineDash", GET_LINE_DASH),
    ("closePath", CLOSE_PATH),
    ("moveTo", MOVE_TO),
    ("lineTo", LINE_TO),
    ("quadraticCurveTo", QUADRATIC_CURVE_TO),
    ("bezierCurveTo", BEZIER_CURVE_TO),
    ("arcTo", ARC_TO),
    ("rect", RECT),
    ("roundRect", ROUND_RECT),
    ("arc", ARC),
    ("ellipse", ELLIPSE),
    ("getContextAttributes", GET_CONTEXT_ATTRIBUTES),
];

// Properties: index `i` is read with op `PROP_BASE + 2 i` and written with `PROP_BASE + 2 i + 1`.
pub const PROP_BASE: u16 = 100;
pub const PROP_NAMES: [&str; 27] = [
    "globalAlpha",
    "globalCompositeOperation",
    "imageSmoothingEnabled",
    "imageSmoothingQuality",
    "strokeStyle",
    "fillStyle",
    "shadowOffsetX",
    "shadowOffsetY",
    "shadowBlur",
    "shadowColor",
    "filter",
    "lineWidth",
    "lineCap",
    "lineJoin",
    "miterLimit",
    "lineDashOffset",
    "font",
    "textAlign",
    "textBaseline",
    "direction",
    "letterSpacing",
    "fontKerning",
    "fontStretch",
    "fontVariantCaps",
    "textRendering",
    "wordSpacing",
    "lang",
];

const fn build_props() -> [(&'static str, u16, u16); 27] {
    let mut out = [("", 0u16, 0u16); 27];
    let mut i = 0;
    while i < 27 {
        out[i] = (PROP_NAMES[i], PROP_BASE + 2 * i as u16, PROP_BASE + 2 * i as u16 + 1);
        i += 1;
    }
    out
}
static PROPS: [(&str, u16, u16); 27] = build_props();

// Methods of the objects (`Path2D` ops are shared by its constructor and building methods).
pub const ADD_COLOR_STOP: u16 = 200;
pub const PATTERN_SET_TRANSFORM: u16 = 201;
pub const PATH2D_NEW: u16 = 202;
pub const PATH2D_ADD_PATH: u16 = 203;
/// `Path2D` building ops: `PATH2D_OFFSET + CLOSE_PATH` ..= `PATH2D_OFFSET + ELLIPSE`.
pub const PATH2D_OFFSET: u16 = 210;

static GRADIENT_METHODS: [(&str, u16); 1] = [("addColorStop", ADD_COLOR_STOP)];
static PATTERN_METHODS: [(&str, u16); 1] = [("setTransform", PATTERN_SET_TRANSFORM)];
static PATH_METHODS: [(&str, u16); 12] = [
    ("Path2D", PATH2D_NEW),
    ("addPath", PATH2D_ADD_PATH),
    ("closePath", PATH2D_OFFSET + CLOSE_PATH),
    ("moveTo", PATH2D_OFFSET + MOVE_TO),
    ("lineTo", PATH2D_OFFSET + LINE_TO),
    ("quadraticCurveTo", PATH2D_OFFSET + QUADRATIC_CURVE_TO),
    ("bezierCurveTo", PATH2D_OFFSET + BEZIER_CURVE_TO),
    ("arcTo", PATH2D_OFFSET + ARC_TO),
    ("rect", PATH2D_OFFSET + RECT),
    ("roundRect", PATH2D_OFFSET + ROUND_RECT),
    ("arc", PATH2D_OFFSET + ARC),
    ("ellipse", PATH2D_OFFSET + ELLIPSE),
];

pub fn methods() -> &'static [(&'static str, u16)] {
    &METHODS
}

pub fn properties() -> &'static [(&'static str, u16, u16)] {
    &PROPS
}

pub fn object_methods(kind: Kind) -> &'static [(&'static str, u16)] {
    match kind {
        Kind::Gradient => &GRADIENT_METHODS,
        Kind::Pattern => &PATTERN_METHODS,
        Kind::Path => &PATH_METHODS,
    }
}
