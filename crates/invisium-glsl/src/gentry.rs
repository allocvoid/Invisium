//! The `vs_main` / `fs_main` entry points: copy stage inputs into the private globals, run
//! the shader's `main`, and gather the outputs (with GL's clip space mapped to WebGPU's).

use crate::gen::{bv_name, flatten, Gen};
use crate::ir::*;
use crate::layout::wty;
use crate::Stage;

impl Gen<'_> {
    fn has_bv(&self, b: BV) -> bool {
        self.m.vars.iter().any(|v| v.storage == Storage::Builtin(b))
    }

    fn interp(var: &Var, leaf: &Ty) -> &'static str {
        if leaf.is_int() || var.interp.flat {
            " @interpolate(flat)"
        } else if var.interp.centroid {
            " @interpolate(perspective, centroid)"
        } else {
            ""
        }
    }

    pub fn entry(&mut self) {
        let m = self.m;
        let c = self.c;
        let main = format!("f{}_main();", m.main);
        let mut ios = Vec::new();
        let mut copies = Vec::new();
        for (v, loc) in &c.varyings {
            let var = &m.vars[*v as usize];
            let name = self.vname(*v);
            for (k, (suf, t)) in flatten(&var.ty).into_iter().enumerate() {
                let l = loc + k as u32;
                let dir = if c.stage == Stage::Vertex { "o" } else { "i" };
                ios.push(format!("@location({l}){} {dir}{l}: {}", Self::interp(var, &t), wty(m, &t)));
                copies.push(if c.stage == Stage::Vertex { format!("o.o{l} = {name}{suf};") } else { format!("{name}{suf} = i{l};") });
            }
        }
        if c.stage == Stage::Vertex {
            self.line("struct VsOut {");
            self.line("  @builtin(position) pos: vec4<f32>,");
            for s in &ios {
                self.line(&format!("  {s},"));
            }
            self.line("}");
            let mut params = vec!["@builtin(vertex_index) vid: u32".to_string(), "@builtin(instance_index) iid: u32".to_string()];
            let mut fills = Vec::new();
            for (v, loc) in &c.attrs {
                let var = &m.vars[*v as usize];
                let name = self.vname(*v);
                match var.ty {
                    Ty::M(cols, r) => {
                        let mut parts = Vec::new();
                        for j in 0..cols as u32 {
                            params.push(format!("@location({}) i{}: vec{r}<f32>", loc + j, loc + j));
                            parts.push(format!("i{}", loc + j));
                        }
                        fills.push(format!("{name} = {}({});", wty(m, &var.ty), parts.join(", ")));
                    }
                    _ => {
                        params.push(format!("@location({loc}) i{loc}: {}", wty(m, &var.ty)));
                        fills.push(format!("{name} = i{loc};"));
                    }
                }
            }
            self.line(&format!("@vertex fn vs_main({}) -> VsOut {{", params.join(", ")));
            self.ind += 1;
            if self.has_bv(BV::VertexID) {
                self.line(&format!("{} = i32(vid);", bv_name(BV::VertexID)));
                self.line(&format!("{} = i32(iid);", bv_name(BV::InstanceID)));
            }
            fills.iter().for_each(|f| self.line(f));
            self.line("init_globals();");
            self.line(&main);
            self.line("var o: VsOut;");
            self.line(&format!("let p = {};", bv_name(BV::Position)));
            self.line("o.pos = vec4<f32>(p.x, -p.y, (p.z + p.w) * 0.5f, p.w);");
            copies.iter().for_each(|f| self.line(f));
            self.line("return o;");
            self.ind -= 1;
            self.line("}");
            return;
        }
        let has_out = !c.outputs.is_empty() || c.writes_depth;
        if has_out {
            self.line("struct FsOut {");
            for (_, l, t) in &c.outputs {
                self.line(&format!("  @location({l}) c{l}: {t},"));
            }
            if c.writes_depth {
                self.line("  @builtin(frag_depth) depth: f32,");
            }
            self.line("}");
        }
        let mut params = vec!["@builtin(position) fc: vec4<f32>".to_string(), "@builtin(front_facing) ff: bool".to_string()];
        params.extend(ios);
        let ret = if has_out { " -> FsOut" } else { "" };
        self.line(&format!("@fragment fn fs_main({}){ret} {{", params.join(", ")));
        self.ind += 1;
        self.line(&format!("{} = fc;", bv_name(BV::FragCoord)));
        self.line(&format!("{} = ff;", bv_name(BV::FrontFacing)));
        self.line(&format!("{} = vec2<f32>(0.5f);", bv_name(BV::PointCoord)));
        if self.has_bv(BV::FragDepth) {
            self.line(&format!("{} = fc.z;", bv_name(BV::FragDepth)));
        }
        copies.iter().for_each(|f| self.line(f));
        self.line("init_globals();");
        self.line(&main);
        if has_out {
            self.line("var o: FsOut;");
            for (src, l, _) in &c.outputs {
                self.line(&format!("o.c{l} = {src};"));
            }
            if c.writes_depth {
                self.line(&format!("o.depth = {};", bv_name(BV::FragDepth)));
            }
            self.line("return o;");
        }
        self.ind -= 1;
        self.line("}");
    }
}
