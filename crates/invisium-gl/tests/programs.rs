//! P2: shaders and programs.

mod common;
use common::*;
use invisium_gl::cmd::Cmd;
use invisium_gl::*;

pub const VS: &str = "attribute vec3 aPos;\nattribute vec2 aUv;\nuniform mat4 uMvp;\nvarying vec2 vUv;\nvoid main() { vUv = aUv; gl_Position = uMvp * vec4(aPos, 1.0); }\n";
pub const FS: &str = "precision mediump float;\nuniform vec4 uColor;\nuniform sampler2D uTex;\nvarying vec2 vUv;\nvoid main() { gl_FragColor = texture2D(uTex, vUv) * uColor; }\n";
const FS_STRUCT: &str = "precision mediump float;\nstruct L { vec3 color; float power; };\nuniform L uLight;\nuniform float uW[4];\nvoid main() { gl_FragColor = vec4(uLight.color * uLight.power, uW[0] + uW[3]); }\n";

fn p(g: &mut Gl, prog: Arg<'static>, name: &str) -> Ret {
    let k = g.k(name);
    g.call("getProgramParameter", &[prog, k])
}

#[test]
fn compile_link_and_reflection() {
    let mut g = Gl::new(Version::WebGl1);
    let good = g.shader("VERTEX_SHADER", VS);
    let st = g.k("COMPILE_STATUS");
    assert_eq!(g.call("getShaderParameter", &[good, st]), Ret::Bool(true));
    assert_eq!(g.call("getShaderParameter", &[good, g.k("SHADER_TYPE")]), Ret::Num(g.kv("VERTEX_SHADER")));
    assert_eq!(g.call("getShaderSource", &[good]), Ret::Str(VS.into()));
    let bad = g.shader("FRAGMENT_SHADER", "void main() { nope; }");
    assert_eq!(g.call("getShaderParameter", &[bad, st]), Ret::Bool(false));
    let want = invisium_glsl::compile(invisium_glsl::Stage::Fragment, "void main() { nope; }", false).err().unwrap();
    assert_eq!(g.call("getShaderInfoLog", &[bad]), Ret::Str(want));
    // Link failures: a missing stage, a shader that did not compile.
    let pr = g.obj("createProgram");
    g.call("attachShader", &[pr, good]);
    g.call("linkProgram", &[pr]);
    assert_eq!(p(&mut g, pr, "LINK_STATUS"), Ret::Bool(false));
    g.call("attachShader", &[pr, bad]);
    g.call("linkProgram", &[pr]);
    assert_eq!(p(&mut g, pr, "LINK_STATUS"), Ret::Bool(false));
    assert!(matches!(g.call("getProgramInfoLog", &[pr]), Ret::Str(s) if !s.is_empty()));
    // A translator link error: a varying the vertex shader does not write.
    let fs_mismatch = "precision mediump float;\nvarying vec3 vOther;\nvoid main() { gl_FragColor = vec4(vOther, 1.0); }\n";
    let mm = g.shader("FRAGMENT_SHADER", fs_mismatch);
    g.call("detachShader", &[pr, bad]);
    g.call("attachShader", &[pr, mm]);
    g.cmds();
    g.call("linkProgram", &[pr]);
    assert_eq!(p(&mut g, pr, "LINK_STATUS"), Ret::Bool(false));
    assert!(g.cmds().is_empty());
    // Success: one ProgramCreate whose description is the translator's.
    let f = g.shader("FRAGMENT_SHADER", FS);
    g.call("detachShader", &[pr, mm]);
    g.call("attachShader", &[pr, f]);
    g.call("linkProgram", &[pr]);
    g.no_err();
    assert_eq!(p(&mut g, pr, "LINK_STATUS"), Ret::Bool(true));
    let cmds = g.cmds();
    let Arg::Object(_, id) = pr else { panic!() };
    let r = reflect(VS, FS, false);
    let creates: Vec<_> = cmds.iter().filter_map(|c| if let Cmd::ProgramCreate { id, desc } = c { Some((*id, desc.clone())) } else { None }).collect();
    assert_eq!(creates.len(), 1);
    assert_eq!(creates[0].0, id);
    let d = &creates[0].1;
    assert_eq!((&d.vertex_wgsl, &d.fragment_wgsl, d.uniform_bytes), (&r.vertex_wgsl, &r.fragment_wgsl, r.uniform_bytes));
    assert_eq!(d.textures, r.textures);
    assert_eq!(d.attributes.len(), r.attributes.len());
    assert_eq!(d.outputs.len(), r.outputs.len());
    assert_eq!(p(&mut g, pr, "ATTACHED_SHADERS"), Ret::Num(2.0));
    assert_eq!(p(&mut g, pr, "ACTIVE_ATTRIBUTES"), Ret::Num(2.0));
    assert_eq!(p(&mut g, pr, "ACTIVE_UNIFORMS"), Ret::Num(r.uniforms.len() as f64));
    assert_eq!(p(&mut g, pr, "DELETE_STATUS"), Ret::Bool(false));
    assert_eq!(p(&mut g, pr, "VALIDATE_STATUS"), Ret::Bool(false));
    g.call("validateProgram", &[pr]);
    assert_eq!(p(&mut g, pr, "VALIDATE_STATUS"), Ret::Bool(true));
    for (i, a) in r.attributes.iter().enumerate() {
        assert_eq!(g.call("getActiveAttrib", &[pr, n(i as f64)]), Ret::ActiveInfo { name: a.name.clone(), size: 1, gl_type: a.gl_type });
        assert_eq!(g.call("getAttribLocation", &[pr, Arg::Str(&a.name)]), Ret::Num(a.location as f64));
    }
    for (i, u) in r.uniforms.iter().enumerate() {
        assert_eq!(g.call("getActiveUniform", &[pr, n(i as f64)]), Ret::ActiveInfo { name: u.name.clone(), size: u.size as i32, gl_type: u.gl_type });
    }
    g.call("getActiveUniform", &[pr, n(99.0)]);
    g.expect("INVALID_VALUE");
    assert_eq!(g.call("getAttribLocation", &[pr, Arg::Str("nope")]), Ret::Num(-1.0));
    let Ret::Objects(Kind::Shader, list) = g.call("getAttachedShaders", &[pr]) else { panic!() };
    assert_eq!(list.len(), 2);
    // bindAttribLocation takes effect at the next link.
    g.call("bindAttribLocation", &[pr, n(7.0), Arg::Str("aUv")]);
    assert_ne!(g.call("getAttribLocation", &[pr, Arg::Str("aUv")]), Ret::Num(7.0));
    g.call("bindAttribLocation", &[pr, n(0.0), Arg::Str("gl_Position")]);
    g.expect("INVALID_OPERATION");
    // Relinking: a new ProgramCreate under the same id; old locations are rejected.
    let old = g.loc(pr, "uColor");
    g.call("useProgram", &[pr]);
    g.call("linkProgram", &[pr]);
    g.no_err();
    assert_eq!(g.call("getAttribLocation", &[pr, Arg::Str("aUv")]), Ret::Num(7.0));
    assert!(g.cmds().iter().any(|c| matches!(c, Cmd::ProgramCreate { id: i, .. } if *i == id)));
    g.call("uniform4f", &[old, n(1.0), n(1.0), n(1.0), n(1.0)]);
    g.expect("INVALID_OPERATION");
    // Deleting attached shaders and a program in use.
    g.call("deleteShader", &[good]);
    assert_eq!(g.call("getShaderParameter", &[good, g.k("DELETE_STATUS")]), Ret::Null, "a deleted shader is no longer usable");
    g.expect("INVALID_VALUE");
    assert_eq!(g.call("isShader", &[good]), Ret::Bool(false));
    g.call("deleteProgram", &[pr]);
    assert_eq!(g.param("CURRENT_PROGRAM"), Ret::Object(Kind::Program, id), "still in use");
    assert_eq!(g.call("isProgram", &[pr]), Ret::Bool(false));
    assert!(!g.cmds().iter().any(|c| matches!(c, Cmd::ProgramDelete { .. })));
    g.call("useProgram", &[Arg::Null]);
    assert_eq!(g.cmds(), vec![Cmd::ProgramDelete { id }]);
    g.call("useProgram", &[pr]);
    g.expect("INVALID_VALUE");
}

#[test]
fn uniform_locations() {
    let mut g = Gl::new(Version::WebGl1);
    let pr = g.program(VS, FS_STRUCT);
    let r = reflect(VS, FS_STRUCT, false);
    for name in ["uMvp", "uW", "uW[0]", "uW[2]", "uLight.color", "uLight.power"] {
        assert!(matches!(g.call("getUniformLocation", &[pr, Arg::Str(name)]), Ret::Object(Kind::UniformLocation, _)), "{name}");
    }
    let h = |a: Arg| if let Arg::Object(_, h) = a { h } else { 0 };
    assert_eq!(h(g.loc(pr, "uW")), h(g.loc(pr, "uW[0]")));
    assert_ne!(h(g.loc(pr, "uW[2]")), h(g.loc(pr, "uW[0]")));
    for name in ["uW[4]", "nope", "uLight", "gl_FragColor"] {
        assert_eq!(g.call("getUniformLocation", &[pr, Arg::Str(name)]), Ret::Null, "{name}");
    }
    assert!(r.uniforms.iter().any(|u| u.name == "uLight.color"));
    // A location of another program.
    let other = g.program(VS, FS);
    let l = g.loc(pr, "uW[1]");
    g.call("useProgram", &[other]);
    g.call("uniform1f", &[l, n(1.0)]);
    g.expect("INVALID_OPERATION");
    g.call("uniform1f", &[Arg::Null, n(1.0)]);
    g.no_err();
}
