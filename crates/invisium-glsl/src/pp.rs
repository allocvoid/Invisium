//! Preprocessor: directives, conditional compilation and macro expansion.

use crate::lex::{lex, Tok, T};
use crate::{err, Error, Stage};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

/// Most tokens macro expansion may produce, in total.
const EXPANSION_BUDGET: u64 = 2_000_000;
/// Deepest nesting of macro calls inside macro arguments.
const MAX_ARG_DEPTH: u32 = 64;

#[derive(Clone, Copy, Default, Debug)]
pub struct Exts {
    pub derivatives: bool,
    pub lod: bool,
    pub frag_depth: bool,
    pub draw_buffers: bool,
}

pub struct PpOut {
    pub toks: Vec<Tok>,
    pub version: u32,
    pub exts: Exts,
}

struct Macro {
    id: u32,
    params: Option<Vec<Rc<str>>>,
    body: Vec<Tok>,
    builtin: bool,
}

struct Cond {
    outer: bool,
    active: bool,
    taken: bool,
    seen_else: bool,
    line: u32,
}

struct Pp {
    macros: HashMap<Rc<str>, Macro>,
    next_id: u32,
    budget: u64,
    version: u32,
    delta: i64,
    exts: Exts,
}

fn int_tok(v: u64, line: u32) -> Tok {
    Tok { t: T::Int(v, false), line, bol: false, sp: true, hs: None }
}

pub fn preprocess(src: &str, stage: Stage, webgl2: bool) -> Result<PpOut, Error> {
    let toks = lex(src);
    let mut lines: Vec<Vec<Tok>> = Vec::new();
    for t in toks {
        if t.bol || lines.is_empty() {
            lines.push(Vec::new());
        }
        if let Some(l) = lines.last_mut() {
            l.push(t);
        }
    }
    let mut pp = Pp { macros: HashMap::new(), next_id: 1, budget: 0, version: 100, delta: 0, exts: Exts::default() };
    let mut first = 0;
    if let Some(l) = lines.first() {
        if l.len() >= 2 && l[0].is("#") && l[1].id() == Some("version") {
            let line = l[0].line;
            let v = match l.get(2).map(|t| &t.t) {
                Some(T::Int(100, false)) if l.len() == 3 => 100,
                Some(T::Int(300, false)) if l.len() == 4 && l[3].id() == Some("es") => 300,
                _ => return err(line, "'#version' : unsupported version (use 100 or 300 es)"),
            };
            if v == 300 && !webgl2 {
                return err(line, "'#version' : version 300 es is only available in WebGL 2 contexts");
            }
            pp.version = v;
            first = 1;
        }
    }
    let ver = pp.version as u64;
    pp.builtin("GL_ES", 1);
    pp.builtin("__VERSION__", ver);
    if stage == Stage::Fragment || pp.version == 300 {
        pp.builtin("GL_FRAGMENT_PRECISION_HIGH", 1);
    }
    if pp.version == 100 {
        for e in ["GL_OES_standard_derivatives", "GL_EXT_shader_texture_lod", "GL_EXT_frag_depth", "GL_EXT_draw_buffers"] {
            pp.builtin(e, 1);
        }
    }

    let mut text: Vec<Tok> = Vec::new();
    let mut out: Vec<Tok> = Vec::new();
    let mut conds: Vec<Cond> = Vec::new();
    for mut line in lines.into_iter().skip(first) {
        for t in line.iter_mut() {
            t.line = (t.line as i64 + pp.delta).clamp(0, u32::MAX as i64) as u32;
        }
        let active = conds.last().map_or(true, |c| c.active);
        if line[0].is("#") {
            if active && !text.is_empty() {
                out.extend(pp.expand(std::mem::take(&mut text), 0)?);
            }
            pp.directive(&line, &mut conds, active)?;
        } else if active {
            text.extend(line);
        }
    }
    if !text.is_empty() {
        out.extend(pp.expand(text, 0)?);
    }
    if let Some(c) = conds.last() {
        return err(c.line, "unterminated conditional directive (missing #endif)");
    }
    for t in out.iter_mut() {
        t.hs = None;
        match &t.t {
            T::Bad(m) => return err(t.line, *m),
            T::P("#") | T::P("##") => return err(t.line, format!("unexpected '{}'", t.t)),
            _ => {}
        }
    }
    Ok(PpOut { toks: out, version: pp.version, exts: pp.exts })
}

impl Pp {
    fn builtin(&mut self, name: &str, v: u64) {
        let id = self.next_id;
        self.next_id += 1;
        self.macros.insert(Rc::from(name), Macro { id, params: None, body: vec![int_tok(v, 0)], builtin: true });
    }

    fn directive(&mut self, l: &[Tok], conds: &mut Vec<Cond>, active: bool) -> Result<(), Error> {
        let line = l[0].line;
        let Some(name) = l.get(1) else { return Ok(()) };
        let name = match &name.t {
            T::Id(s) => s.clone(),
            _ if !active => return Ok(()),
            _ => return err(line, format!("invalid directive '#{}'", name.t)),
        };
        let rest = &l[2.min(l.len())..];
        match &*name {
            "if" | "ifdef" | "ifndef" => {
                if !active {
                    conds.push(Cond { outer: false, active: false, taken: true, seen_else: false, line });
                    return Ok(());
                }
                let v = if &*name == "if" {
                    self.eval(rest, line)?
                } else {
                    let id = match rest {
                        [t] => t.id().ok_or_else(|| Error::new(line, format!("'#{name}' : expected a macro name")))?,
                        _ => return err(line, format!("'#{name}' : expected one macro name")),
                    };
                    self.macros.contains_key(id) == (&*name == "ifdef")
                };
                conds.push(Cond { outer: true, active: v, taken: v, seen_else: false, line });
            }
            "elif" => {
                let Some(c) = conds.last_mut() else { return err(line, "'#elif' without '#if'") };
                if c.seen_else {
                    return err(line, "'#elif' after '#else'");
                }
                if c.outer && !c.taken {
                    let v = self.eval(rest, line)?;
                    c.active = v;
                    c.taken = v;
                } else {
                    c.active = false;
                }
            }
            "else" => {
                let Some(c) = conds.last_mut() else { return err(line, "'#else' without '#if'") };
                if c.seen_else {
                    return err(line, "'#else' after '#else'");
                }
                c.seen_else = true;
                c.active = c.outer && !c.taken;
                c.taken = true;
            }
            "endif" => {
                if conds.pop().is_none() {
                    return err(line, "'#endif' without '#if'");
                }
            }
            _ if !active => {}
            "define" => self.define(rest, line)?,
            "undef" => {
                let id = match rest {
                    [t] => t.id().ok_or_else(|| Error::new(line, "'#undef' : expected a macro name"))?,
                    _ => return err(line, "'#undef' : expected one macro name"),
                };
                if id.starts_with("GL_") || id == "__LINE__" || id == "__FILE__" || self.macros.get(id).is_some_and(|m| m.builtin) {
                    return err(line, format!("'#undef' : cannot undefine the predefined macro '{id}'"));
                }
                self.macros.remove(id);
            }
            "error" => {
                let msg: Vec<String> = rest.iter().map(|t| t.t.to_string()).collect();
                return err(line, format!("'#error' : {}", msg.join(" ")));
            }
            "pragma" => {}
            "extension" => self.extension(rest, line)?,
            "version" => return err(line, "'#version' : must occur on the first line, before anything else"),
            "line" => {
                let toks = self.expand(rest.to_vec(), 0)?;
                let n = match toks.first().map(|t| &t.t) {
                    Some(T::Int(n, _)) if toks.len() <= 2 => *n as i64,
                    _ => return err(line, "'#line' : expected a line number"),
                };
                let phys = line as i64 - self.delta;
                self.delta = n - (phys + 1);
            }
            _ => return err(line, format!("invalid directive '#{name}'")),
        }
        Ok(())
    }

    fn define(&mut self, l: &[Tok], line: u32) -> Result<(), Error> {
        let Some(name) = l.first().and_then(|t| t.id()) else { return err(line, "'#define' : expected a macro name") };
        let name: Rc<str> = Rc::from(name);
        if name.starts_with("GL_") || &*name == "defined" || &*name == "__LINE__" || &*name == "__FILE__" {
            return err(line, format!("'#define' : '{name}' is a reserved macro name"));
        }
        let mut i = 1;
        let mut params = None;
        if l.len() > 1 && l[1].is("(") && !l[1].sp {
            let mut ps: Vec<Rc<str>> = Vec::new();
            i = 2;
            if l.get(2).is_some_and(|t| t.is(")")) {
                i = 3;
            } else {
                loop {
                    let p = l.get(i).and_then(|t| t.id()).ok_or_else(|| Error::new(line, "'#define' : expected a parameter name"))?;
                    if ps.iter().any(|q| &**q == p) {
                        return err(line, format!("'#define' : duplicate macro parameter '{p}'"));
                    }
                    ps.push(Rc::from(p));
                    i += 1;
                    match l.get(i) {
                        Some(t) if t.is(",") => i += 1,
                        Some(t) if t.is(")") => {
                            i += 1;
                            break;
                        }
                        _ => return err(line, "'#define' : expected ',' or ')' in the parameter list"),
                    }
                }
            }
            params = Some(ps);
        }
        let body: Vec<Tok> = l[i..].iter().map(|t| Tok { bol: false, hs: None, ..t.clone() }).collect();
        if let Some(old) = self.macros.get(&name) {
            let same = !old.builtin
                && old.params == params
                && old.body.len() == body.len()
                && old.body.iter().zip(&body).all(|(a, b)| a.t == b.t);
            if !same {
                return err(line, format!("'#define' : macro '{name}' redefined"));
            }
            return Ok(());
        }
        let id = self.next_id;
        self.next_id += 1;
        self.macros.insert(name, Macro { id, params, body, builtin: false });
        Ok(())
    }

    fn extension(&mut self, l: &[Tok], line: u32) -> Result<(), Error> {
        let (name, beh) = match l {
            [n, c, b] if c.is(":") => match (n.id(), b.id()) {
                (Some(n), Some(b)) => (n, b),
                _ => return err(line, "'#extension' : expected 'name : behavior'"),
            },
            _ => return err(line, "'#extension' : expected 'name : behavior'"),
        };
        if !matches!(beh, "require" | "enable" | "warn" | "disable") {
            return err(line, format!("'#extension' : invalid behavior '{beh}'"));
        }
        let on = beh != "disable";
        let known = match name {
            "all" => {
                if matches!(beh, "require" | "enable") {
                    return err(line, "'#extension' : 'all' only accepts warn or disable");
                }
                return Ok(());
            }
            "GL_OES_EGL_image_external" | "GL_OES_EGL_image_external_essl3" | "GL_NV_EGL_stream_consumer_external" => {
                return err(line, format!("'#extension' : extension '{name}' is not supported"))
            }
            "GL_OES_standard_derivatives" => &mut self.exts.derivatives,
            "GL_EXT_shader_texture_lod" => &mut self.exts.lod,
            "GL_EXT_frag_depth" => &mut self.exts.frag_depth,
            "GL_EXT_draw_buffers" => &mut self.exts.draw_buffers,
            _ => {
                if beh == "require" {
                    return err(line, format!("'#extension' : extension '{name}' is not supported"));
                }
                return Ok(());
            }
        };
        if self.version == 300 {
            if beh == "require" {
                return err(line, format!("'#extension' : extension '{name}' is not supported in version 300"));
            }
            return Ok(());
        }
        *known = on;
        Ok(())
    }

    fn eval(&mut self, toks: &[Tok], line: u32) -> Result<bool, Error> {
        let mut v = Vec::with_capacity(toks.len());
        let mut i = 0;
        while i < toks.len() {
            if toks[i].id() == Some("defined") {
                let (name, next) = if toks.get(i + 1).is_some_and(|t| t.is("(")) {
                    match (toks.get(i + 2).and_then(|t| t.id()), toks.get(i + 3)) {
                        (Some(n), Some(c)) if c.is(")") => (n, i + 4),
                        _ => return err(line, "'defined' : expected '(name)'"),
                    }
                } else {
                    match toks.get(i + 1).and_then(|t| t.id()) {
                        Some(n) => (n, i + 2),
                        None => return err(line, "'defined' : expected a macro name"),
                    }
                };
                v.push(int_tok(self.macros.contains_key(name) as u64, line));
                i = next;
            } else {
                v.push(toks[i].clone());
                i += 1;
            }
        }
        let ex = self.expand(v, 0)?;
        Ok(crate::ppexpr::eval(&ex, line)? != 0)
    }

    /// Expand every macro in `input` (a run of text lines, or a directive's operands).
    fn expand(&mut self, input: Vec<Tok>, depth: u32) -> Result<Vec<Tok>, Error> {
        if depth > MAX_ARG_DEPTH {
            let line = input.first().map_or(0, |t| t.line);
            return err(line, "macro arguments nested too deeply");
        }
        let mut q: VecDeque<Tok> = input.into();
        let mut out = Vec::with_capacity(q.len());
        while let Some(t) = q.pop_front() {
            let Some(name) = t.id() else {
                out.push(t);
                continue;
            };
            match name {
                "__LINE__" => {
                    out.push(Tok { t: T::Int(t.line as u64, false), ..t });
                    continue;
                }
                "__FILE__" => {
                    out.push(Tok { t: T::Int(0, false), ..t });
                    continue;
                }
                _ => {}
            }
            let Some(m) = self.macros.get(name) else {
                out.push(t);
                continue;
            };
            if t.hs.as_ref().is_some_and(|h| h.contains(&m.id)) {
                out.push(t);
                continue;
            }
            let mut hs: Vec<u32> = t.hs.as_ref().map_or(Vec::new(), |h| (**h).clone());
            hs.push(m.id);
            let hs = Rc::new(hs);
            let line = t.line;
            let name: Rc<str> = Rc::from(name);
            let result: Vec<Tok> = match &m.params {
                None => m.body.iter().map(|b| Tok { line, hs: Some(hs.clone()), ..b.clone() }).collect(),
                Some(params) => {
                    if !q.front().is_some_and(|n| n.is("(")) {
                        out.push(t);
                        continue;
                    }
                    q.pop_front();
                    let params = params.clone();
                    let body = m.body.clone();
                    let mut args: Vec<Vec<Tok>> = vec![Vec::new()];
                    let mut level = 0u32;
                    loop {
                        let Some(a) = q.pop_front() else {
                            return err(line, format!("unterminated argument list invoking macro '{name}'"));
                        };
                        if a.is("(") {
                            level += 1;
                        } else if a.is(")") {
                            if level == 0 {
                                break;
                            }
                            level -= 1;
                        } else if a.is(",") && level == 0 {
                            args.push(Vec::new());
                            continue;
                        }
                        if let Some(last) = args.last_mut() {
                            last.push(a);
                        }
                    }
                    if params.is_empty() && args.len() == 1 && args[0].is_empty() {
                        args.clear();
                    }
                    if args.len() != params.len() {
                        return err(line, format!("macro '{name}' expects {} arguments, got {}", params.len(), args.len()));
                    }
                    let mut expanded = Vec::with_capacity(args.len());
                    for a in args {
                        expanded.push(self.expand(a, depth + 1)?);
                    }
                    let mut r = Vec::new();
                    for b in &body {
                        match b.id().and_then(|n| params.iter().position(|p| &**p == n)) {
                            Some(k) => {
                                self.charge(expanded[k].len(), line)?;
                                r.extend(expanded[k].iter().map(|a| Tok { line, hs: Some(hs.clone()), ..a.clone() }));
                            }
                            None => r.push(Tok { line, hs: Some(hs.clone()), ..b.clone() }),
                        }
                    }
                    r
                }
            };
            self.charge(result.len(), line)?;
            for r in result.into_iter().rev() {
                q.push_front(r);
            }
        }
        Ok(out)
    }

    fn charge(&mut self, n: usize, line: u32) -> Result<(), Error> {
        self.budget += n as u64 + 1;
        if self.budget > EXPANSION_BUDGET {
            return err(line, "macro expansion too large");
        }
        Ok(())
    }
}
