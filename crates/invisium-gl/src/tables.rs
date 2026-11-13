//! Method tables (name -> op) of both context interfaces, and the extension objects.

use crate::{ExtensionInfo, Version};
use std::sync::OnceLock;

macro_rules! gl_methods {
    (v1: [$($a:ident = $an:expr),* $(,)?] v2: [$($b:ident = $bn:expr),* $(,)?]) => {
        /// Op numbers, named after the methods.
        #[allow(non_upper_case_globals, dead_code)]
        pub mod op {
            $(pub const $a: u16 = $an;)*
            $(pub const $b: u16 = $bn;)*
            /// `WEBGL_lose_context` methods (not context methods).
            pub const loseContext: u16 = 900;
            pub const restoreContext: u16 = 901;
        }
        pub static WEBGL1: &[(&str, u16)] = &[$((stringify!($a), $an)),*];
        pub static WEBGL2_ONLY: &[(&str, u16)] = &[$((stringify!($b), $bn)),*];
    };
}

gl_methods! {
v1: [
    getContextAttributes = 1, isContextLost = 2, getSupportedExtensions = 3, getExtension = 4,
    activeTexture = 5, attachShader = 6, bindAttribLocation = 7, bindBuffer = 8, bindFramebuffer = 9,
    bindRenderbuffer = 10, bindTexture = 11, blendColor = 12, blendEquation = 13, blendEquationSeparate = 14,
    blendFunc = 15, blendFuncSeparate = 16, checkFramebufferStatus = 17, clear = 18, clearColor = 19,
    clearDepth = 20, clearStencil = 21, colorMask = 22, compileShader = 23, copyTexImage2D = 24,
    copyTexSubImage2D = 25, createBuffer = 26, createFramebuffer = 27, createProgram = 28,
    createRenderbuffer = 29, createShader = 30, createTexture = 31, cullFace = 32, deleteBuffer = 33,
    deleteFramebuffer = 34, deleteProgram = 35, deleteRenderbuffer = 36, deleteShader = 37,
    deleteTexture = 38, depthFunc = 39, depthMask = 40, depthRange = 41, detachShader = 42, disable = 43,
    disableVertexAttribArray = 44, drawArrays = 45, drawElements = 46, enable = 47,
    enableVertexAttribArray = 48, finish = 49, flush = 50, framebufferRenderbuffer = 51,
    framebufferTexture2D = 52, frontFace = 53, generateMipmap = 54, getActiveAttrib = 55,
    getActiveUniform = 56, getAttachedShaders = 57, getAttribLocation = 58, getBufferParameter = 59,
    getParameter = 60, getError = 61, getFramebufferAttachmentParameter = 62, getProgramParameter = 63,
    getProgramInfoLog = 64, getRenderbufferParameter = 65, getShaderParameter = 66,
    getShaderPrecisionFormat = 67, getShaderInfoLog = 68, getShaderSource = 69, getTexParameter = 70,
    getUniform = 71, getUniformLocation = 72, getVertexAttrib = 73, getVertexAttribOffset = 74, hint = 75,
    isBuffer = 76, isEnabled = 77, isFramebuffer = 78, isProgram = 79, isRenderbuffer = 80, isShader = 81,
    isTexture = 82, lineWidth = 83, linkProgram = 84, pixelStorei = 85, polygonOffset = 86,
    renderbufferStorage = 87, sampleCoverage = 88, scissor = 89, shaderSource = 90, stencilFunc = 91,
    stencilFuncSeparate = 92, stencilMask = 93, stencilMaskSeparate = 94, stencilOp = 95,
    stencilOpSeparate = 96, texParameterf = 97, texParameteri = 98, uniform1f = 99, uniform2f = 100,
    uniform3f = 101, uniform4f = 102, uniform1i = 103, uniform2i = 104, uniform3i = 105, uniform4i = 106,
    useProgram = 107, validateProgram = 108, vertexAttrib1f = 109, vertexAttrib2f = 110,
    vertexAttrib3f = 111, vertexAttrib4f = 112, vertexAttrib1fv = 113, vertexAttrib2fv = 114,
    vertexAttrib3fv = 115, vertexAttrib4fv = 116, vertexAttribPointer = 117, viewport = 118,
    makeXRCompatible = 119, bufferData = 120, bufferSubData = 121, compressedTexImage2D = 122,
    compressedTexSubImage2D = 123, readPixels = 124, texImage2D = 125, texSubImage2D = 126,
    uniform1fv = 127, uniform2fv = 128, uniform3fv = 129, uniform4fv = 130, uniform1iv = 131,
    uniform2iv = 132, uniform3iv = 133, uniform4iv = 134, uniformMatrix2fv = 135, uniformMatrix3fv = 136,
    uniformMatrix4fv = 137,
]
v2: [
    copyBufferSubData = 138, getBufferSubData = 139, blitFramebuffer = 140, framebufferTextureLayer = 141,
    invalidateFramebuffer = 142, invalidateSubFramebuffer = 143, readBuffer = 144,
    getInternalformatParameter = 145, renderbufferStorageMultisample = 146, texStorage2D = 147,
    texStorage3D = 148, texImage3D = 149, texSubImage3D = 150, copyTexSubImage3D = 151,
    compressedTexImage3D = 152, compressedTexSubImage3D = 153, getFragDataLocation = 154,
    uniform1ui = 155, uniform2ui = 156, uniform3ui = 157, uniform4ui = 158, uniform1uiv = 159,
    uniform2uiv = 160, uniform3uiv = 161, uniform4uiv = 162, uniformMatrix3x2fv = 163,
    uniformMatrix4x2fv = 164, uniformMatrix2x3fv = 165, uniformMatrix4x3fv = 166, uniformMatrix2x4fv = 167,
    uniformMatrix3x4fv = 168, vertexAttribI4i = 169, vertexAttribI4iv = 170, vertexAttribI4ui = 171,
    vertexAttribI4uiv = 172, vertexAttribIPointer = 173, vertexAttribDivisor = 174,
    drawArraysInstanced = 175, drawElementsInstanced = 176, drawRangeElements = 177, drawBuffers = 178,
    clearBufferfv = 179, clearBufferiv = 180, clearBufferuiv = 181, clearBufferfi = 182, createQuery = 183,
    deleteQuery = 184, isQuery = 185, beginQuery = 186, endQuery = 187, getQuery = 188,
    getQueryParameter = 189, createSampler = 190, deleteSampler = 191, isSampler = 192, bindSampler = 193,
    samplerParameteri = 194, samplerParameterf = 195, getSamplerParameter = 196, fenceSync = 197,
    isSync = 198, deleteSync = 199, clientWaitSync = 200, waitSync = 201, getSyncParameter = 202,
    createTransformFeedback = 203, deleteTransformFeedback = 204, isTransformFeedback = 205,
    bindTransformFeedback = 206, beginTransformFeedback = 207, endTransformFeedback = 208,
    transformFeedbackVaryings = 209, getTransformFeedbackVarying = 210, pauseTransformFeedback = 211,
    resumeTransformFeedback = 212, bindBufferBase = 213, bindBufferRange = 214, getIndexedParameter = 215,
    getUniformIndices = 216, getActiveUniforms = 217, getUniformBlockIndex = 218,
    getActiveUniformBlockParameter = 219, getActiveUniformBlockName = 220, uniformBlockBinding = 221,
    createVertexArray = 222, deleteVertexArray = 223, isVertexArray = 224, bindVertexArray = 225,
]
}

pub fn methods(version: Version) -> &'static [(&'static str, u16)] {
    static ALL: OnceLock<Vec<(&'static str, u16)>> = OnceLock::new();
    match version {
        Version::WebGl1 => WEBGL1,
        Version::WebGl2 => ALL.get_or_init(|| WEBGL1.iter().chain(WEBGL2_ONLY).copied().collect()),
    }
}

pub fn constants(version: Version) -> &'static [(&'static str, u32)] {
    static ALL: OnceLock<Vec<(&'static str, u32)>> = OnceLock::new();
    match version {
        Version::WebGl1 => crate::consts1::WEBGL1,
        Version::WebGl2 => ALL.get_or_init(|| {
            crate::consts1::WEBGL1.iter().chain(crate::consts2::WEBGL2_ONLY).copied().collect()
        }),
    }
}

/// Name of an op of either interface (or of an extension-only op).
pub fn op_name(op: u16) -> Option<&'static str> {
    match op {
        op::loseContext => Some("loseContext"),
        op::restoreContext => Some("restoreContext"),
        _ => methods(Version::WebGl2).get((op as usize).wrapping_sub(1)).map(|m| m.0),
    }
}

pub const EXT_WEBGL2: &[&str] = &["EXT_color_buffer_float", "EXT_color_buffer_half_float", "WEBGL_lose_context"];
pub const EXT_WEBGL1: &[&str] = &[
    "ANGLE_instanced_arrays", "EXT_blend_minmax", "EXT_frag_depth", "EXT_shader_texture_lod",
    "OES_element_index_uint", "OES_standard_derivatives", "OES_texture_float", "OES_texture_half_float",
    "OES_vertex_array_object", "WEBGL_color_buffer_float", "WEBGL_depth_texture", "WEBGL_draw_buffers",
    "WEBGL_lose_context",
];

pub fn ext_list(version: Version) -> &'static [&'static str] {
    match version {
        Version::WebGl1 => EXT_WEBGL1,
        Version::WebGl2 => EXT_WEBGL2,
    }
}

/// The canonical name of a supported extension (case-insensitive lookup).
pub fn ext_canonical(version: Version, name: &str) -> Option<&'static str> {
    ext_list(version).iter().copied().find(|e| e.eq_ignore_ascii_case(name))
}

const DRAW_BUFFERS_CONSTS: &[(&str, u32)] = &[
    ("COLOR_ATTACHMENT0_WEBGL", 0x8CE0), ("COLOR_ATTACHMENT1_WEBGL", 0x8CE1), ("COLOR_ATTACHMENT2_WEBGL", 0x8CE2),
    ("COLOR_ATTACHMENT3_WEBGL", 0x8CE3), ("COLOR_ATTACHMENT4_WEBGL", 0x8CE4), ("COLOR_ATTACHMENT5_WEBGL", 0x8CE5),
    ("COLOR_ATTACHMENT6_WEBGL", 0x8CE6), ("COLOR_ATTACHMENT7_WEBGL", 0x8CE7), ("COLOR_ATTACHMENT8_WEBGL", 0x8CE8),
    ("COLOR_ATTACHMENT9_WEBGL", 0x8CE9), ("COLOR_ATTACHMENT10_WEBGL", 0x8CEA), ("COLOR_ATTACHMENT11_WEBGL", 0x8CEB),
    ("COLOR_ATTACHMENT12_WEBGL", 0x8CEC), ("COLOR_ATTACHMENT13_WEBGL", 0x8CED), ("COLOR_ATTACHMENT14_WEBGL", 0x8CEE),
    ("COLOR_ATTACHMENT15_WEBGL", 0x8CEF),
    ("DRAW_BUFFER0_WEBGL", 0x8825), ("DRAW_BUFFER1_WEBGL", 0x8826), ("DRAW_BUFFER2_WEBGL", 0x8827),
    ("DRAW_BUFFER3_WEBGL", 0x8828), ("DRAW_BUFFER4_WEBGL", 0x8829), ("DRAW_BUFFER5_WEBGL", 0x882A),
    ("DRAW_BUFFER6_WEBGL", 0x882B), ("DRAW_BUFFER7_WEBGL", 0x882C), ("DRAW_BUFFER8_WEBGL", 0x882D),
    ("DRAW_BUFFER9_WEBGL", 0x882E), ("DRAW_BUFFER10_WEBGL", 0x882F), ("DRAW_BUFFER11_WEBGL", 0x8830),
    ("DRAW_BUFFER12_WEBGL", 0x8831), ("DRAW_BUFFER13_WEBGL", 0x8832), ("DRAW_BUFFER14_WEBGL", 0x8833),
    ("DRAW_BUFFER15_WEBGL", 0x8834), ("MAX_COLOR_ATTACHMENTS_WEBGL", 0x8CDF), ("MAX_DRAW_BUFFERS_WEBGL", 0x8824),
];

pub fn extension(version: Version, name: &str) -> Option<ExtensionInfo> {
    let name = ext_canonical(version, name)?;
    type Lists = (&'static [(&'static str, u32)], &'static [(&'static str, u16)]);
    let (constants, methods): Lists = match name {
        "ANGLE_instanced_arrays" => (
            &[("VERTEX_ATTRIB_ARRAY_DIVISOR_ANGLE", 0x88FE)],
            &[
                ("drawArraysInstancedANGLE", op::drawArraysInstanced),
                ("drawElementsInstancedANGLE", op::drawElementsInstanced),
                ("vertexAttribDivisorANGLE", op::vertexAttribDivisor),
            ],
        ),
        "EXT_blend_minmax" => (&[("MIN_EXT", 0x8007), ("MAX_EXT", 0x8008)], &[]),
        "OES_standard_derivatives" => (&[("FRAGMENT_SHADER_DERIVATIVE_HINT_OES", 0x8B8B)], &[]),
        "OES_texture_half_float" => (&[("HALF_FLOAT_OES", 0x8D61)], &[]),
        "OES_vertex_array_object" => (
            &[("VERTEX_ARRAY_BINDING_OES", 0x85B5)],
            &[
                ("createVertexArrayOES", op::createVertexArray),
                ("deleteVertexArrayOES", op::deleteVertexArray),
                ("isVertexArrayOES", op::isVertexArray),
                ("bindVertexArrayOES", op::bindVertexArray),
            ],
        ),
        "WEBGL_color_buffer_float" => (
            &[("RGBA32F_EXT", 0x8814), ("FRAMEBUFFER_ATTACHMENT_COMPONENT_TYPE_EXT", 0x8211), ("UNSIGNED_NORMALIZED_EXT", 0x8C17)],
            &[],
        ),
        "EXT_color_buffer_half_float" => (
            &[
                ("RGBA16F_EXT", 0x881A), ("RGB16F_EXT", 0x881B),
                ("FRAMEBUFFER_ATTACHMENT_COMPONENT_TYPE_EXT", 0x8211), ("UNSIGNED_NORMALIZED_EXT", 0x8C17),
            ],
            &[],
        ),
        "WEBGL_depth_texture" => (&[("UNSIGNED_INT_24_8_WEBGL", 0x84FA)], &[]),
        "WEBGL_draw_buffers" => (DRAW_BUFFERS_CONSTS, &[("drawBuffersWEBGL", op::drawBuffers)]),
        "WEBGL_lose_context" => (&[], &[("loseContext", op::loseContext), ("restoreContext", op::restoreContext)]),
        _ => (&[], &[]),
    };
    Some(ExtensionInfo { constants, methods })
}
