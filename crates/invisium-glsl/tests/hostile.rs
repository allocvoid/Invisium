//! G7: hostile input returns `Err` quickly and never panics.

mod common;
use invisium_glsl::{compile, Stage};
use std::time::{Duration, Instant};

fn hostile(src: &str, what: &str) {
    let t = Instant::now();
    let r = compile(Stage::Fragment, src, true);
    let dt = t.elapsed();
    assert!(r.is_err(), "{what}: expected an error");
    assert!(r.as_ref().err().is_some_and(|e| e.starts_with("ERROR: ")), "{what}: {:?}", r.err());
    assert!(dt < Duration::from_secs(2), "{what}: took {dt:?}");
}

#[test]
fn random_bytes() {
    let mut seed = 0x1234_5678_9abc_def0u64;
    for round in 0..40 {
        let len = 1 + (round * 997) % 20000;
        let mut bytes = Vec::with_capacity(len);
        for _ in 0..len {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            bytes.push((seed >> 24) as u8);
        }
        let src = String::from_utf8_lossy(&bytes);
        hostile(&src, &format!("random bytes round {round}"));
    }
}

#[test]
fn random_tokens() {
    let words = ["(", ")", "{", "}", "[", "]", ";", "float", "vec4", "x", "=", "+", "1.0", "#define", "#if", "\n", "main", "void", ",", ".", "?", ":", "struct", "for", "return"];
    let mut seed = 42u64;
    for round in 0..40 {
        let mut s = String::new();
        for _ in 0..2000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            s.push_str(words[(seed >> 33) as usize % words.len()]);
            s.push(' ');
        }
        let _ = compile(Stage::Fragment, &s, true);
        let _ = compile(Stage::Vertex, &s, false);
        let _ = round;
    }
}

#[test]
fn one_mib_of_open_parens() {
    hostile(&"(".repeat(1 << 20), "1 MiB of (");
    let src = format!("void main() {{ float x = {}; }}", "(".repeat((1 << 20) - 40));
    hostile(&src, "1 MiB of ( in an expression");
}

#[test]
fn exponential_macro_chain() {
    let mut src = String::from("#define A0 x\n");
    for k in 1..=40 {
        src.push_str(&format!("#define A{k} A{} A{}\n", k - 1, k - 1));
    }
    src.push_str("A40\n");
    hostile(&src, "2^40 macro chain");
    let mut src = String::from("#define F0(x) x x\n");
    for k in 1..=40 {
        src.push_str(&format!("#define F{k}(x) F{}(F{}(x))\n", k - 1, k - 1));
    }
    src.push_str("F40(y)\n");
    hostile(&src, "2^40 function-like chain");
}

#[test]
fn deep_nesting() {
    let blocks = format!("void main() {{ {}{} }}", "{".repeat(10_000), "}".repeat(10_000));
    hostile(&blocks, "10,000 nested blocks");
    let parens = format!("void main() {{ float x = {}1.0{}; }}", "(".repeat(10_000), ")".repeat(10_000));
    hostile(&parens, "10,000 nested parentheses");
    let unary = format!("void main() {{ float x = {}1.0; }}", "-".repeat(10_000));
    hostile(&unary, "10,000 unary minus");
    let calls = format!("void main() {{ float x = {}1.0{}; }}", "sin(".repeat(10_000), ")".repeat(10_000));
    hostile(&calls, "10,000 nested calls");
    let chain = format!("void main() {{ float x = 1.0{}; }}", " + 1.0".repeat(10_000));
    hostile(&chain, "10,000-term sum");
    let ifs = format!("void main() {{ float x = 0.0; {} }}", "if (x > 0.0) ".repeat(10_000) + "x = 1.0;");
    hostile(&ifs, "10,000 nested ifs");
    let args = format!("#define F(x) x\n{}1{}", "F(".repeat(10_000), ")".repeat(10_000));
    hostile(&args, "10,000 nested macro arguments");
}

#[test]
fn huge_arrays() {
    hostile("float a[1000000000];\nvoid main() {}", "float a[1000000000]");
    hostile("void main() { float a[100000]; }", "local array over the limit");
    hostile("struct S { float x[65536]; }; uniform S s[65536]; void main() { gl_FragColor = vec4(s[0].x[0]); }", "huge uniform");
    hostile("void main() { float a[-1]; }", "negative size");
}

#[test]
fn diamond_structure_chain() {
    let mut src = String::from("struct S0 { float a; };\n");
    for k in 1..=40 {
        src.push_str(&format!("struct S{k} {{ S{p} a; S{p} b; }};\n", p = k - 1));
    }
    src.push_str("uniform S40 u;\nvoid main() { gl_FragColor = vec4(u.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a.a); }\n");
    hostile(&src, "diamond structure chain");
    let e = compile(Stage::Fragment, &src, true).err().unwrap();
    assert!(e.contains("too large"), "{e}");
}

#[test]
fn deep_structure_chain() {
    let mut src = String::from("struct s0{float m;};");
    for k in 1..40_000 {
        src.push_str(&format!("struct s{k}{{s{} m;}};", k - 1));
    }
    assert!(src.len() < 1 << 20);
    hostile(&src, "40,000-deep structure chain");
    let e = compile(Stage::Fragment, &src, true).err().unwrap();
    assert!(e.contains("nested too deeply"), "{e}");
}

#[test]
fn reflection_entry_explosion() {
    let fs = "precision mediump float; struct A { float x[2]; }; struct B { A a[1000]; }; uniform B b[1000];
        void main() { gl_FragColor = vec4(b[0].a[0].x[0]); }";
    let fs2 = "precision mediump float; struct A { float x; }; struct B { A a[512]; }; uniform B b[512];
        void main() { gl_FragColor = vec4(b[1].a[2].x); }";
    let vs = "attribute vec4 p; void main() { gl_Position = p; }";
    for src in [fs, fs2] {
        let t = Instant::now();
        let r = common::try_build(vs, src, false, &[]);
        assert!(t.elapsed() < Duration::from_secs(2));
        match r {
            Err(e) => assert!(e.contains("ERROR: "), "{e}"),
            Ok(p) => panic!("linked with {} uniform entries", p.uniforms.len()),
        }
    }
}

#[test]
fn long_else_if_chain_compiles() {
    for (n, side_effects) in [(10_000, false), (10_000, true), (12, false)] {
        let mut body = String::from("float r = 0.0; int i = int(u);\n");
        for k in 0..n {
            let cond = if side_effects { format!("(i += 1) == {}", k * 2) } else { format!("i == {k}") };
            body.push_str(&format!("{}if ({cond}) {{ r = {k}.0; }}\n", if k == 0 { "" } else { "else " }));
        }
        body.push_str("else { r = -1.0; }\n");
        let fs = format!("precision highp float; uniform float u; void main() {{ {body} gl_FragColor = vec4(r); }}");
        let t = Instant::now();
        let p = common::try_build("attribute vec4 p; void main() { gl_Position = p; }", &fs, false, &[])
            .unwrap_or_else(|e| panic!("{n}-long chain: {e}"));
        assert!(t.elapsed() < Duration::from_secs(2), "{n}-long chain took {:?}", t.elapsed());
        common::validate_program(&p);
    }
}

#[test]
fn nested_ifs_are_refused() {
    let src = format!("void main() {{ float x = 0.0; {}x = 1.0;{} }}", "if (x > 0.0) { ".repeat(10_000), " }".repeat(10_000));
    hostile(&src, "10,000 nested ifs with braces");
}

#[test]
fn source_over_the_cap() {
    let mut src = String::from("void main() {}\n");
    src.push_str(&" ".repeat((1 << 20) + 1 - src.len()));
    assert_eq!(src.len(), (1 << 20) + 1);
    hostile(&src, "source over 1 MiB");
}

#[test]
fn large_valid_shader_is_fast() {
    let mut src = String::from("precision highp float;\nuniform vec4 u;\n");
    let mut k = 0;
    while src.len() < 100 * 1024 {
        src.push_str(&format!(
            "vec4 f{k}(vec4 a, float b) {{ vec4 r = a * b + u; for (int i = 0; i < 4; i++) {{ r.xy += sin(r.zw) * float(i); if (r.x > b) r = r.wzyx; }} return mix(r, a, 0.5) + vec4(mod(b, 2.0)); }}\n"
        ));
        k += 1;
    }
    src.push_str("void main() { vec4 a = u;\n");
    for i in 0..k {
        src.push_str(&format!("a = f{i}(a, {i}.0);\n"));
    }
    src.push_str("gl_FragColor = a; }\n");
    let t = Instant::now();
    let fs = compile(Stage::Fragment, &src, false).expect("large shader compiles");
    let vs = compile(Stage::Vertex, "attribute vec4 p; void main() { gl_Position = p; }", false).unwrap();
    let p = invisium_glsl::link(&vs, &fs, &[]).expect("links");
    let dt = t.elapsed();
    println!("{} KiB of GLSL -> {} KiB of WGSL in {dt:?}", src.len() / 1024, p.fragment_wgsl.len() / 1024);
    common::validate(&p.fragment_wgsl);
    assert!(dt < Duration::from_millis(500), "took {dt:?}");
}
