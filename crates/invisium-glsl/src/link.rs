//! Linking: match the two stages, lay out the default uniform block, assign locations and
//! texture slots, build the reflection data, and generate both WGSL modules.

use crate::gen::{generate, Ctx};
use crate::ir::*;
use crate::layout::{col_stride, gl_type, round_up, slayout, stride, struct_offsets, wty};
use crate::{Attribute, Base, Output, Place, Program, Scalar, Stage, TexSlot, Uniform};
use std::collections::HashMap;

/// Largest default uniform block (WebGPU's guaranteed uniform binding size).
const MAX_UNIFORM_BYTES: u32 = 65536;
const MAX_ATTRIBS: u32 = 16;
const MAX_VARYING_LOCATIONS: u32 = 15;
const MAX_TEXTURE_SLOTS: u32 = 16;
const MAX_OUTPUTS: u32 = 8;
const MAX_UNIFORM_ENTRIES: usize = 16384;

fn lerr(line: u32, msg: impl std::fmt::Display) -> String {
    if line > 0 {
        format!("ERROR: 0:{line}: {msg}\n")
    } else {
        format!("ERROR: {msg}\n")
    }
}

pub struct Usage {
    pub funcs: Vec<bool>,
    pub vars: Vec<bool>,
}

fn walk_e(e: &Expr, u: &mut Usage, work: &mut Vec<FuncId>) {
    match &e.k {
        EK::Var(v) => u.vars[*v as usize] = true,
        EK::Call(f, _) => work.push(*f),
        _ => {}
    }
    for c in e.k.children() {
        walk_e(c, u, work);
    }
}

fn walk_s(s: &Stmt, u: &mut Usage, work: &mut Vec<FuncId>) {
    let e = |x: &Expr, u: &mut Usage, w: &mut Vec<FuncId>| walk_e(x, u, w);
    match s {
        Stmt::Decl(_, Some(x)) | Stmt::Expr(x) | Stmt::Return(Some(x)) => e(x, u, work),
        Stmt::Block(v) => v.iter().for_each(|s| walk_s(s, u, work)),
        Stmt::If(chain, b) => {
            for (c, a) in chain {
                e(c, u, work);
                walk_s(a, u, work);
            }
            if let Some(b) = b {
                walk_s(b, u, work);
            }
        }
        Stmt::For { init, cond, step, body } => {
            init.iter().for_each(|s| walk_s(s, u, work));
            for x in cond.iter().chain(step.iter()) {
                e(x, u, work);
            }
            walk_s(body, u, work);
        }
        Stmt::While(c, b) | Stmt::DoWhile(b, c) => {
            e(c, u, work);
            walk_s(b, u, work);
        }
        Stmt::Switch(c, cases) => {
            e(c, u, work);
            cases.iter().flat_map(|c| c.body.iter()).for_each(|s| walk_s(s, u, work));
        }
        _ => {}
    }
}

/// Functions reachable from `main` and the global initialisers, and the variables they use.
pub fn usage(m: &Module) -> Usage {
    let mut u = Usage { funcs: vec![false; m.funcs.len()], vars: vec![false; m.vars.len()] };
    let mut work = vec![m.main];
    for (_, e) in &m.inits {
        walk_e(e, &mut u, &mut work);
    }
    while let Some(f) = work.pop() {
        if std::mem::replace(&mut u.funcs[f as usize], true) {
            continue;
        }
        for s in m.funcs[f as usize].body.iter().flatten() {
            walk_s(s, &mut u, &mut work);
        }
    }
    u
}

/// Structural type equality across the two modules. Structure pairs are memoised, so the
/// work is linear in the number of structure definitions.
fn ty_eq(ma: &Module, a: &Ty, mb: &Module, b: &Ty, memo: &mut HashMap<(u32, u32), bool>) -> bool {
    match (a, b) {
        (Ty::Struct(x), Ty::Struct(y)) => {
            if let Some(r) = memo.get(&(*x, *y)) {
                return *r;
            }
            let (sx, sy) = (&ma.structs[*x as usize], &mb.structs[*y as usize]);
            let r = sx.name == sy.name
                && sx.fields.len() == sy.fields.len()
                && sx.fields.iter().zip(&sy.fields).all(|(f, g)| f.0 == g.0 && ty_eq(ma, &f.1, mb, &g.1, memo));
            memo.insert((*x, *y), r);
            r
        }
        (Ty::Array(x, n), Ty::Array(y, k)) => n == k && ty_eq(ma, x, mb, y, memo),
        _ => a == b,
    }
}

fn base(t: &Ty) -> Base {
    match t.kind() {
        Some(K::Bool) => Base::Bool,
        Some(K::Int) => Base::I32,
        Some(K::Uint) => Base::U32,
        _ => Base::F32,
    }
}

fn scalar(t: &Ty) -> Scalar {
    match t.kind() {
        Some(K::Int) => Scalar::I32,
        Some(K::Uint) => Scalar::U32,
        _ => Scalar::F32,
    }
}

fn place(t: &Ty, offset: u32, stride: u32) -> Place {
    let (rows, cols) = match t {
        Ty::M(c, r) => (*r, *c),
        Ty::V(_, n) => (*n, 1),
        _ => (1, 1),
    };
    Place::Block { offset, stride, base: base(t), rows, cols, col_stride: col_stride(t) }
}

/// Expand a uniform into `getActiveUniform` entries; fails once there are more than
/// `MAX_UNIFORM_ENTRIES`, so the work is bounded whatever the type.
fn reflect(m: &Module, name: &str, t: &Ty, off: u32, out: &mut Vec<Uniform>) -> Result<(), String> {
    if out.len() >= MAX_UNIFORM_ENTRIES {
        return Err(lerr(0, format!("too many active uniform entries (limit {MAX_UNIFORM_ENTRIES})")));
    }
    match t {
        Ty::Struct(s) => {
            let (offs, _) = struct_offsets(m, *s);
            for (k, (f, ft)) in m.structs[*s as usize].fields.iter().enumerate() {
                reflect(m, &format!("{name}.{f}"), ft, off + offs[k], out)?;
            }
        }
        Ty::Array(e, n) if matches!(**e, Ty::Struct(_)) => {
            let st = stride(m, e);
            for i in 0..*n {
                reflect(m, &format!("{name}[{i}]"), e, off + i * st, out)?;
            }
        }
        Ty::Array(e, n) => out.push(Uniform { name: format!("{name}[0]"), gl_type: gl_type(e), size: *n, place: place(e, off, stride(m, e)) }),
        _ => out.push(Uniform { name: name.into(), gl_type: gl_type(t), size: 1, place: place(t, off, stride(m, t)) }),
    }
    Ok(())
}

fn globals(m: &Module, s: Storage) -> impl Iterator<Item = VarId> + '_ {
    m.globals.iter().copied().filter(move |v| m.vars[*v as usize].storage == s)
}

pub fn link(vm: &Module, fm: &Module, binds: &[(String, u32)]) -> Result<Program, String> {
    let (vu, fu) = (usage(vm), usage(fm));
    let mut memo = HashMap::new();
    let find = |m: &Module, s: Storage, n: &str| globals(m, s).find(|v| m.vars[*v as usize].name == n);

    // ---- uniforms ----
    for v in globals(vm, Storage::Uniform) {
        let vv = &vm.vars[v as usize];
        if let Some(f) = find(fm, Storage::Uniform, &vv.name) {
            if !ty_eq(vm, &vv.ty, fm, &fm.vars[f as usize].ty, &mut memo) {
                return Err(lerr(fm.vars[f as usize].line, format!("uniform '{}' has different types in the vertex and fragment shaders", vv.name)));
            }
        }
    }
    let mut active: Vec<(String, Option<VarId>, Option<VarId>)> = Vec::new();
    for v in globals(vm, Storage::Uniform).filter(|v| vu.vars[*v as usize]) {
        let name = vm.vars[v as usize].name.clone();
        let f = find(fm, Storage::Uniform, &name).filter(|f| fu.vars[*f as usize]);
        active.push((name, Some(v), f));
    }
    for f in globals(fm, Storage::Uniform).filter(|f| fu.vars[*f as usize]) {
        let name = &fm.vars[f as usize].name;
        if !active.iter().any(|a| a.0 == *name) {
            active.push((name.clone(), None, Some(f)));
        }
    }
    let (mut off, mut slot) = (0u32, 0u32);
    let (mut uniforms, mut textures) = (Vec::new(), Vec::new());
    let (mut vblock, mut fblock) = (Vec::new(), Vec::new());
    let (mut vsamp, mut fsamp) = (HashMap::new(), HashMap::new());
    for (name, v, f) in &active {
        let (m, id) = match (v, f) {
            (Some(v), _) => (vm, *v),
            (None, Some(f)) => (fm, *f),
            _ => continue,
        };
        let ty = &m.vars[id as usize].ty;
        let samp = match ty {
            Ty::Sampler(k) => Some((*k, 1, false)),
            Ty::Array(e, n) => match **e {
                Ty::Sampler(k) => Some((k, *n, true)),
                _ => None,
            },
            _ => None,
        };
        if let Some((k, n, arr)) = samp {
            for i in 0..n {
                let s = slot + i;
                textures.push(TexSlot { texture: 2 * s, sampler: 2 * s + 1, kind: k.kind(), sample: k.sample(), in_vertex: v.is_some(), in_fragment: f.is_some() });
            }
            let uname = if arr { format!("{name}[0]") } else { name.clone() };
            uniforms.push(Uniform { name: uname, gl_type: k.info().1, size: n, place: Place::Texture { slot } });
            if let Some(v) = v {
                vsamp.insert(*v, slot);
            }
            if let Some(f) = f {
                fsamp.insert(*f, slot);
            }
            slot += n;
            if slot > MAX_TEXTURE_SLOTS {
                return Err(lerr(0, "too many samplers"));
            }
            continue;
        }
        let (size, align) = slayout(m, ty);
        let align = if matches!(ty, Ty::Struct(_) | Ty::Array(..)) { 16 } else { align };
        off = round_up(align, off);
        if off.saturating_add(size) > MAX_UNIFORM_BYTES {
            return Err(lerr(0, format!("too many uniforms (the default block is limited to {MAX_UNIFORM_BYTES} bytes)")));
        }
        reflect(m, name, ty, off, &mut uniforms)?;
        if let Some(v) = v {
            vblock.push((*v, off));
        }
        if let Some(f) = f {
            fblock.push((*f, off));
        }
        off += size;
    }
    let ubytes = if vblock.is_empty() && fblock.is_empty() { 0 } else { round_up(16, off) };

    // ---- vertex inputs ----
    let attrs: Vec<VarId> = globals(vm, Storage::Attribute).filter(|v| vu.vars[*v as usize]).collect();
    let mut taken: Vec<Option<VarId>> = vec![None; MAX_ATTRIBS as usize];
    let mut locs: HashMap<VarId, u32> = HashMap::new();
    let occupy = |v: VarId, loc: u32, locs: &mut HashMap<VarId, u32>, taken: &mut Vec<Option<VarId>>| -> Result<(), String> {
        let var = &vm.vars[v as usize];
        let n = if let Ty::M(c, _) = var.ty { c as u32 } else { 1 };
        if loc + n > MAX_ATTRIBS {
            return Err(lerr(var.line, format!("attribute '{}' location {loc} is out of range", var.name)));
        }
        for l in loc..loc + n {
            if let Some(o) = taken[l as usize] {
                return Err(lerr(var.line, format!("attribute '{}' location {l} is already used by '{}'", var.name, vm.vars[o as usize].name)));
            }
            taken[l as usize] = Some(v);
        }
        locs.insert(v, loc);
        Ok(())
    };
    for v in &attrs {
        if let Some(l) = vm.vars[*v as usize].location {
            occupy(*v, l, &mut locs, &mut taken)?;
        }
    }
    for v in &attrs {
        let var = &vm.vars[*v as usize];
        if var.location.is_none() {
            if let Some((_, l)) = binds.iter().rev().find(|b| b.0 == var.name) {
                occupy(*v, *l, &mut locs, &mut taken)?;
            }
        }
    }
    for v in &attrs {
        if locs.contains_key(v) {
            continue;
        }
        let var = &vm.vars[*v as usize];
        let n = if let Ty::M(c, _) = var.ty { c as u32 } else { 1 };
        let free = (0..=MAX_ATTRIBS.saturating_sub(n)).find(|l| (*l..l + n).all(|k| taken[k as usize].is_none()));
        match free {
            Some(l) => occupy(*v, l, &mut locs, &mut taken)?,
            None => return Err(lerr(var.line, "too many vertex attributes")),
        }
    }
    let mut vattrs: Vec<(VarId, u32)> = attrs.iter().map(|v| (*v, locs[v])).collect();
    vattrs.sort_by_key(|a| a.1);
    let attributes = vattrs
        .iter()
        .map(|(v, l)| {
            let var = &vm.vars[*v as usize];
            let columns = if let Ty::M(c, _) = var.ty { c as u32 } else { 1 };
            Attribute { name: var.name.clone(), gl_type: gl_type(&var.ty), location: *l, columns, base: scalar(&var.ty) }
        })
        .collect();

    // ---- varyings ----
    let (mut vvary, mut fvary, mut next) = (Vec::new(), Vec::new(), 0u32);
    for f in globals(fm, Storage::VaryingIn) {
        let fv = &fm.vars[f as usize];
        let Some(v) = find(vm, Storage::VaryingOut, &fv.name) else {
            if fu.vars[f as usize] {
                return Err(lerr(fv.line, format!("varying '{}' is not declared in the vertex shader", fv.name)));
            }
            continue;
        };
        let vv = &vm.vars[v as usize];
        if !ty_eq(vm, &vv.ty, fm, &fv.ty, &mut memo) {
            return Err(lerr(fv.line, format!("varying '{}' has a different type in the vertex shader", fv.name)));
        }
        if vv.interp.flat != fv.interp.flat {
            return Err(lerr(fv.line, format!("varying '{}' has different interpolation qualifiers", fv.name)));
        }
        let n = crate::gen::flat_count(&fv.ty);
        if next + n > MAX_VARYING_LOCATIONS {
            return Err(lerr(fv.line, "too many varyings"));
        }
        vvary.push((v, next));
        fvary.push((f, next));
        next += n;
    }

    // ---- fragment outputs ----
    let mut outs: Vec<(String, u32, String)> = Vec::new();
    let mut outputs = Vec::new();
    let mut writes_depth = false;
    let mut add_out = |src: String, name: String, loc: u32, t: &Ty, line: u32, outs: &mut Vec<(String, u32, String)>| -> Result<(), String> {
        if loc >= MAX_OUTPUTS || outs.iter().any(|o| o.1 == loc) {
            return Err(lerr(line, format!("fragment output '{name}' location {loc} is out of range or already used")));
        }
        outs.push((src, loc, wty(fm, t)));
        outputs.push(Output { name, location: loc, base: scalar(t) });
        Ok(())
    };
    let user_outs: Vec<VarId> = globals(fm, Storage::FragOut).collect();
    for v in fm.globals.iter().copied() {
        let var = &fm.vars[v as usize];
        let used = fu.vars[v as usize];
        match var.storage {
            Storage::Builtin(BV::FragColor) if used => add_out("bi_FragColor".into(), "gl_FragColor".into(), 0, &var.ty, 0, &mut outs)?,
            Storage::Builtin(BV::FragData) if used => {
                if let Ty::Array(e, n) = &var.ty {
                    for i in 0..*n {
                        add_out(format!("bi_FragData[{i}]"), format!("gl_FragData[{i}]"), i, e, 0, &mut outs)?;
                    }
                }
            }
            Storage::Builtin(BV::FragDepth) if used => writes_depth = true,
            Storage::FragOut => {
                if user_outs.len() > 1 && var.location.is_none() {
                    return Err(lerr(var.line, "layout(location) is required when there is more than one output"));
                }
                let loc = var.location.unwrap_or(0);
                let src = format!("g{v}_{}", var.name);
                match &var.ty {
                    Ty::Array(e, n) => {
                        for i in 0..*n {
                            add_out(format!("{src}[{i}]"), format!("{}[{i}]", var.name), loc + i, e, var.line, &mut outs)?;
                        }
                    }
                    t => add_out(src, var.name.clone(), loc, t, var.line, &mut outs)?,
                }
            }
            _ => {}
        }
    }

    let vctx = Ctx {
        m: vm,
        stage: Stage::Vertex,
        uniforms: vblock,
        ubytes,
        samplers: vsamp,
        attrs: vattrs,
        varyings: vvary,
        outputs: Vec::new(),
        writes_depth: false,
        reachable: vu.funcs,
    };
    let fctx = Ctx {
        m: fm,
        stage: Stage::Fragment,
        uniforms: fblock,
        ubytes,
        samplers: fsamp,
        attrs: Vec::new(),
        varyings: fvary,
        outputs: outs,
        writes_depth,
        reachable: fu.funcs,
    };
    Ok(Program {
        vertex_wgsl: generate(&vctx)?,
        fragment_wgsl: generate(&fctx)?,
        uniform_bytes: ubytes,
        attributes,
        uniforms,
        textures,
        blocks: Vec::new(),
        outputs,
        writes_depth,
    })
}
