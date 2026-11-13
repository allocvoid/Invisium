//! L2: constant and method tables.

use invisium_gl::*;
use std::collections::HashSet;

const SPOT: &[(&str, u32)] = &[
    ("DEPTH_TEST", 0x0B71), ("TEXTURE_2D", 0x0DE1), ("UNPACK_FLIP_Y_WEBGL", 0x9240), ("COLOR_BUFFER_BIT", 0x4000),
    ("DEPTH_BUFFER_BIT", 0x0100), ("STENCIL_BUFFER_BIT", 0x0400), ("TRIANGLES", 0x0004), ("ARRAY_BUFFER", 0x8892),
    ("ELEMENT_ARRAY_BUFFER", 0x8893), ("STATIC_DRAW", 0x88E4), ("BLEND", 0x0BE2), ("CULL_FACE", 0x0B44),
    ("FLOAT", 0x1406), ("UNSIGNED_BYTE", 0x1401), ("RGBA", 0x1908), ("RGB", 0x1907), ("TEXTURE_CUBE_MAP", 0x8513),
    ("TEXTURE0", 0x84C0), ("TEXTURE31", 0x84DF), ("FRAMEBUFFER", 0x8D40), ("RENDERBUFFER", 0x8D41),
    ("COLOR_ATTACHMENT0", 0x8CE0), ("FRAMEBUFFER_COMPLETE", 0x8CD5), ("INVALID_ENUM", 0x0500),
    ("CONTEXT_LOST_WEBGL", 0x9242), ("VERTEX_SHADER", 0x8B31), ("FRAGMENT_SHADER", 0x8B30), ("LINK_STATUS", 0x8B82),
    ("MAX_TEXTURE_SIZE", 0x0D33), ("DEPTH_STENCIL", 0x84F9), ("UNSIGNED_SHORT_5_6_5", 0x8363),
    ("STENCIL_INDEX8", 0x8D48), ("SAMPLER_CUBE", 0x8B60), ("BROWSER_DEFAULT_WEBGL", 0x9244),
];

const SPOT2: &[(&str, u32)] = &[
    ("RGBA16F", 0x881A), ("MAX_SAMPLES", 0x8D57), ("TEXTURE_3D", 0x806F), ("RGBA8", 0x8058), ("R8", 0x8229),
    ("HALF_FLOAT", 0x140B), ("UNIFORM_BUFFER", 0x8A11), ("READ_FRAMEBUFFER", 0x8CA8), ("DRAW_FRAMEBUFFER", 0x8CA9),
    ("TEXTURE_2D_ARRAY", 0x8C1A), ("SRGB8_ALPHA8", 0x8C43), ("DEPTH24_STENCIL8", 0x88F0), ("RGBA32F", 0x8814),
    ("COPY_READ_BUFFER", 0x8F36), ("SYNC_GPU_COMMANDS_COMPLETE", 0x9117), ("MAX_CLIENT_WAIT_TIMEOUT_WEBGL", 0x9247),
    ("RGBA8UI", 0x8D7C), ("TRANSFORM_FEEDBACK", 0x8E22), ("COLOR_ATTACHMENT15", 0x8CEF), ("DRAW_BUFFER15", 0x8834),
    ("UNSIGNED_INT_2_10_10_10_REV", 0x8368), ("R11F_G11F_B10F", 0x8C3A), ("RED_INTEGER", 0x8D94),
    ("TEXTURE_IMMUTABLE_LEVELS", 0x82DF), ("INVALID_INDEX", 0xFFFF_FFFF),
];

fn get(v: Version, name: &str) -> Option<u32> {
    constants(v).iter().find(|c| c.0 == name).map(|c| c.1)
}

#[test]
fn constants_spot_check() {
    assert!(SPOT.len() + SPOT2.len() >= 40);
    for &(name, val) in SPOT {
        assert_eq!(get(Version::WebGl1, name), Some(val), "{name} (WebGL 1)");
        assert_eq!(get(Version::WebGl2, name), Some(val), "{name} (WebGL 2)");
    }
    for &(name, val) in SPOT2 {
        assert_eq!(get(Version::WebGl2, name), Some(val), "{name} (WebGL 2)");
        assert_eq!(get(Version::WebGl1, name), None, "{name} must not be in WebGL 1");
    }
}

#[test]
fn constants_complete_and_unique() {
    for v in [Version::WebGl1, Version::WebGl2] {
        let names: HashSet<_> = constants(v).iter().map(|c| c.0).collect();
        assert_eq!(names.len(), constants(v).len(), "duplicate constant names");
    }
    let one = constants(Version::WebGl1);
    assert!(one.iter().all(|c| get(Version::WebGl2, c.0) == Some(c.1)));
    let two = constants(Version::WebGl2).len();
    assert_eq!((one.len(), two - one.len()), (296, 263));
}

#[test]
fn methods_tables() {
    let one = methods(Version::WebGl1);
    let two = methods(Version::WebGl2);
    // IDL method names (overloads counted once): 137 for WebGL 1, 225 for WebGL 2.
    assert_eq!(one.len(), 137);
    assert_eq!(two.len(), 225);
    for t in [one, two] {
        let ops: HashSet<_> = t.iter().map(|m| m.1).collect();
        let names: HashSet<_> = t.iter().map(|m| m.0).collect();
        assert_eq!(ops.len(), t.len(), "op numbers are unique");
        assert_eq!(names.len(), t.len(), "names are unique");
        assert!(!ops.contains(&0));
    }
    for m in one {
        assert_eq!(method_op(Version::WebGl2, m.0), Some(m.1));
    }
    for name in ["texStorage2D", "blitFramebuffer", "bindBufferBase", "clearBufferfv", "createVertexArray", "drawBuffers", "texImage3D", "getBufferSubData"] {
        assert!(method_op(Version::WebGl1, name).is_none(), "{name} must not be in WebGL 1");
        assert!(method_op(Version::WebGl2, name).is_some(), "{name} must be in WebGL 2");
    }
    for name in ["texImage2D", "readPixels", "getParameter", "uniformMatrix4fv", "makeXRCompatible", "getContextAttributes"] {
        assert!(method_op(Version::WebGl1, name).is_some(), "{name}");
    }
}

#[test]
fn extensions() {
    assert!(extension(Version::WebGl1, "webgl_LOSE_context").is_some());
    assert!(extension(Version::WebGl1, "WEBGL_debug_renderer_info").is_none());
    assert!(extension(Version::WebGl2, "OES_texture_float").is_none());
    let ang = extension(Version::WebGl1, "ANGLE_instanced_arrays").unwrap();
    let op = ang.methods.iter().find(|m| m.0 == "drawArraysInstancedANGLE").unwrap().1;
    assert_eq!(Some(op), method_op(Version::WebGl2, "drawArraysInstanced"));
    let db = extension(Version::WebGl1, "WEBGL_draw_buffers").unwrap();
    assert!(db.constants.contains(&("MAX_DRAW_BUFFERS_WEBGL", 0x8824)));
}
