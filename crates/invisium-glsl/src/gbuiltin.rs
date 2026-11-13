//! Built-in functions in WGSL, with GLSL's semantics where the two languages differ.

use crate::gen::Gen;
use crate::ir::*;
use crate::layout::{mangle, wty};
use crate::Stage;

const INV4: &str = "fn h_inverse_m44(m: mat4x4<f32>) -> mat4x4<f32> {
  let a00 = m[0][0]; let a01 = m[0][1]; let a02 = m[0][2]; let a03 = m[0][3];
  let a10 = m[1][0]; let a11 = m[1][1]; let a12 = m[1][2]; let a13 = m[1][3];
  let a20 = m[2][0]; let a21 = m[2][1]; let a22 = m[2][2]; let a23 = m[2][3];
  let a30 = m[3][0]; let a31 = m[3][1]; let a32 = m[3][2]; let a33 = m[3][3];
  let b00 = a00 * a11 - a01 * a10; let b01 = a00 * a12 - a02 * a10;
  let b02 = a00 * a13 - a03 * a10; let b03 = a01 * a12 - a02 * a11;
  let b04 = a01 * a13 - a03 * a11; let b05 = a02 * a13 - a03 * a12;
  let b06 = a20 * a31 - a21 * a30; let b07 = a20 * a32 - a22 * a30;
  let b08 = a20 * a33 - a23 * a30; let b09 = a21 * a32 - a22 * a31;
  let b10 = a21 * a33 - a23 * a31; let b11 = a22 * a33 - a23 * a32;
  let det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
  return mat4x4<f32>(
    a11 * b11 - a12 * b10 + a13 * b09, a02 * b10 - a01 * b11 - a03 * b09,
    a31 * b05 - a32 * b04 + a33 * b03, a22 * b04 - a21 * b05 - a23 * b03,
    a12 * b08 - a10 * b11 - a13 * b07, a00 * b11 - a02 * b08 + a03 * b07,
    a32 * b02 - a30 * b05 - a33 * b01, a20 * b05 - a22 * b02 + a23 * b01,
    a10 * b10 - a11 * b08 + a13 * b06, a01 * b08 - a00 * b10 - a03 * b06,
    a30 * b04 - a31 * b02 + a33 * b00, a21 * b02 - a20 * b04 - a23 * b00,
    a11 * b07 - a10 * b09 - a12 * b06, a00 * b09 - a01 * b07 + a02 * b06,
    a31 * b01 - a30 * b03 - a32 * b00, a20 * b03 - a21 * b01 + a22 * b00) * (1.0f / det);
}
";

const INV3: &str = "fn h_inverse_m33(m: mat3x3<f32>) -> mat3x3<f32> {
  let r0 = cross(m[1], m[2]); let r1 = cross(m[2], m[0]); let r2 = cross(m[0], m[1]);
  return transpose(mat3x3<f32>(r0, r1, r2)) * (1.0f / dot(m[0], r0));
}
";

const INV2: &str = "fn h_inverse_m22(m: mat2x2<f32>) -> mat2x2<f32> {
  let d = m[0][0] * m[1][1] - m[1][0] * m[0][1];
  return mat2x2<f32>(m[1][1], -m[0][1], -m[1][0], m[0][0]) * (1.0f / d);
}
";

fn direct(n: &str) -> Option<&'static str> {
    Some(match n {
        "radians" => "radians",
        "degrees" => "degrees",
        "sin" => "sin",
        "cos" => "cos",
        "tan" => "tan",
        "asin" => "asin",
        "acos" => "acos",
        "sinh" => "sinh",
        "cosh" => "cosh",
        "tanh" => "tanh",
        "asinh" => "asinh",
        "acosh" => "acosh",
        "atanh" => "atanh",
        "pow" => "pow",
        "exp" => "exp",
        "log" => "log",
        "exp2" => "exp2",
        "log2" => "log2",
        "sqrt" => "sqrt",
        "inversesqrt" => "inverseSqrt",
        "abs" => "abs",
        "sign" => "sign",
        "floor" => "floor",
        "trunc" => "trunc",
        "round" | "roundEven" => "round",
        "ceil" => "ceil",
        "fract" => "fract",
        "cross" => "cross",
        "transpose" => "transpose",
        "determinant" => "determinant",
        "dFdx" => "dpdx",
        "dFdy" => "dpdy",
        "fwidth" => "fwidth",
        "any" => "any",
        "all" => "all",
        "packSnorm2x16" => "pack2x16snorm",
        "unpackSnorm2x16" => "unpack2x16snorm",
        "packUnorm2x16" => "pack2x16unorm",
        "unpackUnorm2x16" => "unpack2x16unorm",
        "packHalf2x16" => "pack2x16float",
        "unpackHalf2x16" => "unpack2x16float",
        _ => return None,
    })
}

impl Gen<'_> {
    pub fn builtin(&mut self, n: &str, args: &[Expr], ret: &Ty) -> String {
        if n.starts_with("tex") {
            return self.texture(n, args);
        }
        if n == "modf" {
            let x = self.expr(&args[0]);
            let lv = self.lvalue(&args[1]);
            let t = self.fresh();
            self.line(&format!("let {t} = modf({x});"));
            self.store(&lv, &format!("{t}.whole"));
            return format!("{t}.fract");
        }
        let refs: Vec<&Expr> = args.iter().collect();
        let v = self.operands(&refs);
        let t0 = &args[0].ty;
        let scalar = t0.is_scalar();
        let in2 = |s: &str| format!("vec2<f32>({s}, 0.0f)");
        if let Some(d) = direct(n) {
            return format!("{d}({})", v.join(", "));
        }
        match n {
            "atan" if v.len() == 2 => format!("atan2({}, {})", v[0], v[1]),
            "atan" => format!("atan({})", v[0]),
            "min" | "max" | "clamp" | "step" | "smoothstep" | "mix" => {
                if n == "mix" && args[2].ty.is_bool() {
                    return format!("select({}, {}, {})", v[0], v[1], v[2]);
                }
                let parts: Vec<String> = v
                    .iter()
                    .zip(args)
                    .map(|(s, a)| match (ret, &a.ty) {
                        (Ty::V(k, m), Ty::S(_)) => format!("vec{m}<{}>({s})", k.wgsl()),
                        _ => s.clone(),
                    })
                    .collect();
                format!("{n}({})", parts.join(", "))
            }
            "mod" => {
                let name = format!("h_mod_{}", mangle(t0));
                let w = wty(self.m, t0);
                self.helper(&name, || format!("fn {name}(x: {w}, y: {w}) -> {w} {{ return x - y * floor(x / y); }}\n"));
                let y = if args[1].ty != *t0 { format!("{w}({})", v[1]) } else { v[1].clone() };
                format!("{name}({}, {y})", v[0])
            }
            "length" if scalar => format!("abs({})", v[0]),
            "distance" if scalar => format!("abs({} - {})", v[0], v[1]),
            "dot" if scalar => format!("({} * {})", v[0], v[1]),
            "normalize" if scalar => format!("sign({})", v[0]),
            "faceforward" if scalar => format!("faceForward({}, {}, {}).x", in2(&v[0]), in2(&v[1]), in2(&v[2])),
            "reflect" if scalar => format!("reflect({}, {}).x", in2(&v[0]), in2(&v[1])),
            "refract" if scalar => format!("refract({}, {}, {}).x", in2(&v[0]), in2(&v[1]), v[2]),
            "length" | "distance" | "dot" | "normalize" | "reflect" | "refract" => format!("{n}({})", v.join(", ")),
            "faceforward" => format!("faceForward({})", v.join(", ")),
            "matrixCompMult" | "outerProduct" | "inverse" => {
                let name = format!("h_{n}_{}", mangle(ret));
                let w = wty(self.m, ret);
                let Ty::M(c, _) = *ret else { return "0".into() };
                match n {
                    "inverse" => {
                        let code = match c {
                            2 => INV2,
                            3 => INV3,
                            _ => INV4,
                        };
                        self.helper(&name, || code.to_string());
                        format!("{name}({})", v[0])
                    }
                    "matrixCompMult" => {
                        let cols: Vec<String> = (0..c).map(|j| format!("a[{j}] * b[{j}]")).collect();
                        self.helper(&name, || format!("fn {name}(a: {w}, b: {w}) -> {w} {{ return {w}({}); }}\n", cols.join(", ")));
                        format!("{name}({}, {})", v[0], v[1])
                    }
                    _ => {
                        let (wc, wr) = (wty(self.m, t0), wty(self.m, &args[1].ty));
                        let cols: Vec<String> = (0..c).map(|j| format!("c * r[{j}]")).collect();
                        self.helper(&name, || format!("fn {name}(c: {wc}, r: {wr}) -> {w} {{ return {w}({}); }}\n", cols.join(", ")));
                        format!("{name}({}, {})", v[0], v[1])
                    }
                }
            }
            "lessThan" => format!("({} < {})", v[0], v[1]),
            "lessThanEqual" => format!("({} <= {})", v[0], v[1]),
            "greaterThan" => format!("({} > {})", v[0], v[1]),
            "greaterThanEqual" => format!("({} >= {})", v[0], v[1]),
            "equal" => format!("({} == {})", v[0], v[1]),
            "notEqual" => format!("({} != {})", v[0], v[1]),
            "not" => format!("(!{})", v[0]),
            "isnan" | "isinf" => {
                let cmp = if n == "isnan" { ">" } else { "==" };
                match t0 {
                    Ty::V(_, m) => format!(
                        "((bitcast<vec{m}<u32>>({}) & vec{m}<u32>(0x7fffffffu)) {cmp} vec{m}<u32>(0x7f800000u))",
                        v[0]
                    ),
                    _ => format!("((bitcast<u32>({}) & 0x7fffffffu) {cmp} 0x7f800000u)", v[0]),
                }
            }
            "floatBitsToInt" | "floatBitsToUint" | "intBitsToFloat" | "uintBitsToFloat" => {
                format!("bitcast<{}>({})", wty(self.m, ret), v[0])
            }
            _ => format!("{n}({})", v.join(", ")),
        }
    }

    fn texture(&mut self, n: &str, args: &[Expr]) -> String {
        let (t, s) = self.tex_pair(&args[0]);
        let Ty::Sampler(sk) = args[0].ty else { return "vec4<f32>()".into() };
        let refs: Vec<&Expr> = args[1..].iter().collect();
        let v = self.operands(&refs);
        let arr = sk == Samp::S2DArray;
        let proj = n.contains("Proj");
        let mut coord = v[0].clone();
        if proj {
            let p = self.hoist(&coord, &args[1].ty);
            coord = match (&args[1].ty, sk) {
                (Ty::V(_, 3), _) => format!("({p}.xy / {p}.z)"),
                (_, Samp::S3D) => format!("({p}.xyz / {p}.w)"),
                _ => format!("({p}.xy / {p}.w)"),
            };
        }
        match n {
            "texelFetch" => {
                if arr {
                    let p = self.hoist(&coord, &args[1].ty);
                    return format!("textureLoad({t}, {p}.xy, {p}.z, {})", v[1]);
                }
                return format!("textureLoad({t}, {coord}, {})", v[1]);
            }
            "textureSize" => {
                return match sk.kind() {
                    invisium_draw::gl::TexKind::D2 | invisium_draw::gl::TexKind::Cube => {
                        format!("vec2<i32>(textureDimensions({t}, {}))", v[0])
                    }
                    invisium_draw::gl::TexKind::D3 => format!("vec3<i32>(textureDimensions({t}, {}))", v[0]),
                    invisium_draw::gl::TexKind::Array => {
                        format!("vec3<i32>(vec2<i32>(textureDimensions({t}, {})), i32(textureNumLayers({t})))", v[0])
                    }
                };
            }
            _ => {}
        }
        let (c, layer) = if arr {
            let p = self.hoist(&coord, &args[1].ty);
            (format!("{p}.xy"), format!(", i32(floor({p}.z + 0.5f))"))
        } else {
            (coord, String::new())
        };
        let vert = self.c.stage == Stage::Vertex;
        if n.contains("Lod") {
            return format!("textureSampleLevel({t}, {s}, {c}{layer}, {})", v[1]);
        }
        match v.get(1) {
            _ if vert => format!("textureSampleLevel({t}, {s}, {c}{layer}, 0.0f)"),
            Some(b) => format!("textureSampleBias({t}, {s}, {c}{layer}, {b})"),
            None => format!("textureSample({t}, {s}, {c}{layer})"),
        }
    }
}
