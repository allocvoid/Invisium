//! Built-in variables, constants and the signatures of built-in functions.

use crate::ir::*;
use crate::parse::Parser;
use crate::Stage;

pub fn declare_vars(p: &mut Parser) {
    let v3 = p.version == 300;
    let add = |p: &mut Parser, name: &str, ty: Ty, s: Storage, c: Option<CVal>| {
        let mut v = Parser::var(name, ty, s, 0, true);
        v.cval = c;
        let _ = p.add_var(v);
    };
    let consts: [(&str, i32, bool); 12] = [
        ("gl_MaxVertexAttribs", 16, true),
        ("gl_MaxVertexUniformVectors", 256, true),
        ("gl_MaxVaryingVectors", 15, !v3),
        ("gl_MaxVertexTextureImageUnits", 16, true),
        ("gl_MaxCombinedTextureImageUnits", 32, true),
        ("gl_MaxTextureImageUnits", 16, true),
        ("gl_MaxFragmentUniformVectors", 224, true),
        ("gl_MaxDrawBuffers", if v3 || p.exts.draw_buffers { 4 } else { 1 }, true),
        ("gl_MaxVertexOutputVectors", 16, v3),
        ("gl_MaxFragmentInputVectors", 15, v3),
        ("gl_MinProgramTexelOffset", -8, v3),
        ("gl_MaxProgramTexelOffset", 7, v3),
    ];
    for (n, v, on) in consts {
        if on {
            add(p, n, INT, Storage::Const, Some(CVal::I(v)));
        }
    }
    let vec = |n| Ty::V(K::Float, n);
    if p.stage == Stage::Vertex {
        add(p, "gl_Position", vec(4), Storage::Builtin(BV::Position), None);
        add(p, "gl_PointSize", FLOAT, Storage::Builtin(BV::PointSize), None);
        if v3 {
            add(p, "gl_VertexID", INT, Storage::Builtin(BV::VertexID), None);
            add(p, "gl_InstanceID", INT, Storage::Builtin(BV::InstanceID), None);
        }
    } else {
        add(p, "gl_FragCoord", vec(4), Storage::Builtin(BV::FragCoord), None);
        add(p, "gl_FrontFacing", BOOL, Storage::Builtin(BV::FrontFacing), None);
        add(p, "gl_PointCoord", vec(2), Storage::Builtin(BV::PointCoord), None);
        if v3 {
            add(p, "gl_FragDepth", FLOAT, Storage::Builtin(BV::FragDepth), None);
        } else {
            add(p, "gl_FragColor", vec(4), Storage::Builtin(BV::FragColor), None);
            let n = if p.exts.draw_buffers { 4 } else { 1 };
            add(p, "gl_FragData", Ty::Array(Box::new(vec(4)), n), Storage::Builtin(BV::FragData), None);
            if p.exts.frag_depth {
                add(p, "gl_FragDepthEXT", FLOAT, Storage::Builtin(BV::FragDepth), None);
            }
        }
    }
}

pub fn is_builtin(p: &Parser, name: &str) -> bool {
    sig(p, name, &[]).is_some()
}

type Sig = Result<(&'static str, Ty), String>;

const NAMES: &[&str] = &[
    "radians", "degrees", "sin", "cos", "tan", "asin", "acos", "exp", "log", "exp2", "log2", "sqrt", "inversesqrt",
    "floor", "ceil", "fract", "normalize", "sinh", "cosh", "tanh", "asinh", "acosh", "atanh", "trunc", "round",
    "roundEven", "dFdx", "dFdy", "fwidth", "atan", "pow", "abs", "sign", "mod", "min", "max", "clamp", "mix", "step",
    "smoothstep", "length", "distance", "dot", "cross", "faceforward", "reflect", "refract", "matrixCompMult",
    "transpose", "inverse", "determinant", "outerProduct", "lessThan", "lessThanEqual", "greaterThan",
    "greaterThanEqual", "equal", "notEqual", "any", "all", "not", "isnan", "isinf", "floatBitsToInt",
    "floatBitsToUint", "intBitsToFloat", "uintBitsToFloat", "packSnorm2x16", "unpackSnorm2x16", "packUnorm2x16",
    "unpackUnorm2x16", "packHalf2x16", "unpackHalf2x16", "modf", "texture2D", "texture2DProj", "texture2DLod",
    "texture2DProjLod", "textureCube", "textureCubeLod", "texture2DLodEXT", "texture2DProjLodEXT",
    "textureCubeLodEXT", "texture", "textureProj", "textureLod", "textureProjLod", "texelFetch", "textureSize",
    "textureGrad", "textureOffset", "textureProjOffset", "textureLodOffset", "textureProjLodOffset",
    "texelFetchOffset", "textureGradOffset", "textureProjGrad", "textureProjGradOffset", "texture2DGradEXT",
];

const V300: &[&str] = &[
    "sinh", "cosh", "tanh", "asinh", "acosh", "atanh", "trunc", "round", "roundEven", "transpose", "inverse",
    "determinant", "outerProduct", "isnan", "isinf", "floatBitsToInt", "floatBitsToUint", "intBitsToFloat",
    "uintBitsToFloat", "packSnorm2x16", "unpackSnorm2x16", "packUnorm2x16", "unpackUnorm2x16", "packHalf2x16",
    "unpackHalf2x16", "modf", "texture", "textureProj", "textureLod", "textureProjLod", "texelFetch", "textureSize",
    "textureGrad", "textureOffset", "textureProjOffset", "textureLodOffset", "textureProjLodOffset",
    "texelFetchOffset", "textureGradOffset", "textureProjGrad", "textureProjGradOffset", "texture2DGradEXT",
];

const TEX100: &[&str] = &[
    "texture2D", "texture2DProj", "texture2DLod", "texture2DProjLod", "textureCube", "textureCubeLod",
    "texture2DLodEXT", "texture2DProjLodEXT", "textureCubeLodEXT",
];

/// Type and canonical name of a built-in function call. `None`: no built-in of that name exists
/// in this shader; `Some(Err)`: it exists but not for these arguments.
pub fn sig(p: &Parser, name: &str, a: &[Ty]) -> Option<Sig> {
    let canon = NAMES.iter().copied().find(|n| *n == name)?;
    let v3 = p.version == 300;
    let frag = p.stage == Stage::Fragment;
    let tex = canon.starts_with("tex");
    if (V300.contains(&canon) && !v3) || (TEX100.contains(&canon) && v3) {
        return None;
    }
    if canon.contains("Grad") || canon.contains("Offset") {
        return Some(Err("this texture function is not supported yet".into()));
    }
    match canon {
        "dFdx" | "dFdy" | "fwidth" if !frag || !(v3 || p.exts.derivatives) => return None,
        "texture2DLod" | "texture2DProjLod" | "textureCubeLod" if frag => return None,
        "texture2DLodEXT" | "texture2DProjLodEXT" | "textureCubeLodEXT" if !frag || !p.exts.lod => return None,
        _ => {}
    }
    if a.is_empty() {
        return Some(Err("no matching overloaded function found".into()));
    }
    let r = if tex { texture(p, canon, a) } else { math(p, canon, a) };
    Some(r.map(|t| (canon, t)).ok_or_else(|| "no matching overloaded function found".to_string()))
}

fn gen_f(t: &Ty) -> bool {
    t.is_float()
}

fn math(p: &Parser, n: &str, a: &[Ty]) -> Option<Ty> {
    let v3 = p.version == 300;
    let same = |k: usize| a.len() == k && a.iter().all(|t| *t == a[0]);
    let f1 = a.len() == 1 && gen_f(&a[0]);
    let bvec = |t: &Ty| Ty::vec(K::Bool, t.size());
    match n {
        "radians" | "degrees" | "sin" | "cos" | "tan" | "asin" | "acos" | "exp" | "log" | "exp2" | "log2" | "sqrt"
        | "inversesqrt" | "floor" | "ceil" | "fract" | "normalize" | "sinh" | "cosh" | "tanh" | "asinh" | "acosh"
        | "atanh" | "trunc" | "round" | "roundEven" | "dFdx" | "dFdy" | "fwidth" => f1.then(|| a[0].clone()),
        "atan" => ((a.len() == 1 || same(2)) && gen_f(&a[0])).then(|| a[0].clone()),
        "pow" => (same(2) && gen_f(&a[0])).then(|| a[0].clone()),
        "abs" | "sign" => (a.len() == 1 && (gen_f(&a[0]) || (v3 && matches!(a[0].kind(), Some(K::Int))) && a[0].is_sv())).then(|| a[0].clone()),
        "mod" => (a.len() == 2 && gen_f(&a[0]) && (a[1] == a[0] || a[1] == FLOAT)).then(|| a[0].clone()),
        "min" | "max" | "clamp" => {
            let k = a[0].kind()?;
            let ok_k = k == K::Float || (v3 && matches!(k, K::Int | K::Uint));
            let want = if n == "clamp" { 3 } else { 2 };
            let ok = a.len() == want && a[0].is_sv() && (a[1..].iter().all(|t| *t == a[0]) || a[1..].iter().all(|t| *t == Ty::S(k)));
            (ok_k && ok).then(|| a[0].clone())
        }
        "mix" => {
            let ok = a.len() == 3 && gen_f(&a[0]) && a[1] == a[0];
            let third = a.get(2).is_some_and(|t| *t == a[0] || *t == FLOAT || (v3 && *t == bvec(&a[0])));
            (ok && third).then(|| a[0].clone())
        }
        "step" => (a.len() == 2 && gen_f(&a[1]) && (a[0] == a[1] || a[0] == FLOAT)).then(|| a[1].clone()),
        "smoothstep" => {
            (a.len() == 3 && gen_f(&a[2]) && ((a[0] == a[2] && a[1] == a[2]) || (a[0] == FLOAT && a[1] == FLOAT))).then(|| a[2].clone())
        }
        "length" => f1.then_some(FLOAT),
        "distance" | "dot" => (same(2) && gen_f(&a[0])).then_some(FLOAT),
        "cross" => (same(2) && a[0] == Ty::V(K::Float, 3)).then(|| a[0].clone()),
        "faceforward" => (same(3) && gen_f(&a[0])).then(|| a[0].clone()),
        "reflect" => (same(2) && gen_f(&a[0])).then(|| a[0].clone()),
        "refract" => (a.len() == 3 && gen_f(&a[0]) && a[1] == a[0] && a[2] == FLOAT).then(|| a[0].clone()),
        "matrixCompMult" => (same(2) && a[0].is_mat()).then(|| a[0].clone()),
        "transpose" | "inverse" => (a.len() == 1 && a[0].is_mat()).then(|| a[0].clone()),
        "determinant" => (a.len() == 1 && a[0].is_mat()).then_some(FLOAT),
        "outerProduct" => match (a.len(), &a[0], a.get(1)) {
            (2, Ty::V(K::Float, c), Some(Ty::V(K::Float, r))) if c == r => Some(Ty::M(*r, *c)),
            _ => None,
        },
        "lessThan" | "lessThanEqual" | "greaterThan" | "greaterThanEqual" => {
            (same(2) && a[0].is_vector() && a[0].is_numeric()).then(|| bvec(&a[0]))
        }
        "equal" | "notEqual" => (same(2) && a[0].is_vector()).then(|| bvec(&a[0])),
        "any" | "all" => (a.len() == 1 && matches!(a[0], Ty::V(K::Bool, _))).then_some(BOOL),
        "not" => (a.len() == 1 && matches!(a[0], Ty::V(K::Bool, _))).then(|| a[0].clone()),
        "isnan" | "isinf" => f1.then(|| bvec(&a[0])),
        "floatBitsToInt" => f1.then(|| Ty::vec(K::Int, a[0].size())),
        "floatBitsToUint" => f1.then(|| Ty::vec(K::Uint, a[0].size())),
        "intBitsToFloat" => (a.len() == 1 && matches!(a[0], Ty::S(K::Int) | Ty::V(K::Int, _))).then(|| Ty::vec(K::Float, a[0].size())),
        "uintBitsToFloat" => (a.len() == 1 && matches!(a[0], Ty::S(K::Uint) | Ty::V(K::Uint, _))).then(|| Ty::vec(K::Float, a[0].size())),
        "packSnorm2x16" | "packUnorm2x16" | "packHalf2x16" => (a.len() == 1 && a[0] == Ty::V(K::Float, 2)).then_some(UINT),
        "unpackSnorm2x16" | "unpackUnorm2x16" | "unpackHalf2x16" => (a.len() == 1 && a[0] == UINT).then_some(Ty::V(K::Float, 2)),
        "modf" => (same(2) && gen_f(&a[0])).then(|| a[0].clone()),
        _ => None,
    }
}

fn texture(p: &Parser, n: &str, a: &[Ty]) -> Option<Ty> {
    let Ty::Sampler(s) = a[0] else { return None };
    let frag = p.stage == Stage::Fragment;
    let vec4 = Ty::V(K::Float, 4);
    let fv = |k: u8| Ty::vec(K::Float, k);
    let iv = |k: u8| Ty::vec(K::Int, k);
    let rest = &a[1..];
    if n == "textureSize" {
        let k = if matches!(s.kind(), invisium_draw::gl::TexKind::D2 | invisium_draw::gl::TexKind::Cube) { 2 } else { 3 };
        return (rest.len() == 1 && rest[0] == INT).then(|| iv(k));
    }
    // Coordinate size of the plain lookup for the four float sampler kinds.
    let coord = match s {
        Samp::S2D => 2,
        Samp::S3D | Samp::Cube | Samp::S2DArray => 3,
        _ => return None,
    };
    let bias_ok = |extra: &[Ty]| extra.is_empty() || (frag && extra.len() == 1 && extra[0] == FLOAT);
    match n {
        "texture2D" | "texture" | "textureCube" => {
            let want = match n {
                "texture2D" => s == Samp::S2D,
                "textureCube" => s == Samp::Cube,
                _ => true,
            };
            (want && rest.first() == Some(&fv(coord)) && bias_ok(&rest[1..])).then_some(vec4)
        }
        "texture2DProj" | "textureProj" => {
            let c = rest.first()?;
            let ok = match s {
                Samp::S2D => *c == fv(3) || *c == fv(4),
                Samp::S3D => *c == fv(4) && n == "textureProj",
                _ => false,
            };
            (ok && bias_ok(&rest[1..])).then_some(vec4)
        }
        "texture2DLod" | "texture2DLodEXT" | "textureLod" | "textureCubeLod" | "textureCubeLodEXT" => {
            let want = match n {
                "textureLod" => true,
                "textureCubeLod" | "textureCubeLodEXT" => s == Samp::Cube,
                _ => s == Samp::S2D,
            };
            (want && rest.len() == 2 && rest[0] == fv(coord) && rest[1] == FLOAT).then_some(vec4)
        }
        "texture2DProjLod" | "texture2DProjLodEXT" | "textureProjLod" => {
            let ok = match s {
                Samp::S2D => rest.first() == Some(&fv(3)) || rest.first() == Some(&fv(4)),
                Samp::S3D => rest.first() == Some(&fv(4)) && n == "textureProjLod",
                _ => false,
            };
            (ok && rest.len() == 2 && rest[1] == FLOAT).then_some(vec4)
        }
        "texelFetch" => (s != Samp::Cube && rest.len() == 2 && rest[0] == iv(coord) && rest[1] == INT).then_some(vec4),
        _ => None,
    }
}
