//! `Canvas2d::call`: argument checking and dispatch.

use crate::draw::matrix_of;
use crate::geom::{M, P};
use crate::paint::{GradKind, Gradient};
use crate::path::Path;
use crate::budget::{grad_bytes, path_obj_bytes, state_extra, Charge, GRAD_OBJ, STOP_BYTES};
use crate::path::{MAX_SEGS, SEG_BYTES};
use crate::state::{num, Objects, State};
use crate::tables::*;
use crate::{svgpath, Arg, Canvas2d, Fonts, Kind, Ret};
use std::cell::RefCell;
use std::rc::Rc;

/// Deepest `save` nesting; further saves are ignored.
const MAX_SAVE: usize = 4096;

pub fn throw(name: &'static str, msg: &str) -> Ret {
    Ret::Throw(name, msg.to_string())
}

fn nums<const N: usize>(a: &[Arg], from: usize) -> Option<[f64; N]> {
    let mut out = [0.0; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = num(a.get(from + i));
        if !o.is_finite() {
            return None;
        }
    }
    Some(out)
}

/// The fill rule argument: `Ok(evenodd)` or a `TypeError`.
fn rule(a: Option<&Arg>) -> Result<bool, Ret> {
    match a {
        None | Some(Arg::Undefined) => Ok(false),
        Some(Arg::Str("nonzero")) => Ok(false),
        Some(Arg::Str("evenodd")) => Ok(true),
        _ => Err(throw("TypeError", "invalid CanvasFillRule")),
    }
}

fn path_arg(a: &[Arg]) -> Option<u32> {
    match a.first() {
        Some(Arg::Object(Kind::Path, h)) => Some(*h),
        _ => None,
    }
}

/// One `CanvasPath` method (`op` without the `Path2D` offset) on `path`, whose points are
/// mapped through `m`; `inv` maps stored points back to user space.
pub fn build(path: &mut Path, m: &M, inv: Option<M>, op: u16, a: &[Arg]) -> Ret {
    match op {
        CLOSE_PATH => path.close(),
        MOVE_TO => {
            if let Some([x, y]) = nums(a, 0) {
                path.move_to(m, x, y)
            }
        }
        LINE_TO => {
            if let Some([x, y]) = nums(a, 0) {
                path.line_to(m, x, y)
            }
        }
        QUADRATIC_CURVE_TO => {
            if let Some([cx, cy, x, y]) = nums(a, 0) {
                path.quad_to(m, cx, cy, x, y)
            }
        }
        BEZIER_CURVE_TO => {
            if let Some(c) = nums::<6>(a, 0) {
                path.cubic_to(m, c)
            }
        }
        ARC_TO => {
            if let Some([x1, y1, x2, y2, r]) = nums(a, 0) {
                if r < 0.0 {
                    return throw("IndexSizeError", "negative radius");
                }
                path.arc_to(m, inv, P::new(x1, y1), P::new(x2, y2), r);
            }
        }
        RECT => {
            if let Some([x, y, w, h]) = nums(a, 0) {
                path.rect(m, x, y, w, h)
            }
        }
        ROUND_RECT => {
            let Some([x, y, w, h]) = nums(a, 0) else { return Ret::Undefined };
            let list: Vec<f64> = match a.get(4) {
                None | Some(Arg::Undefined) => vec![0.0],
                Some(Arg::Nums(v)) if v.is_empty() || v.len() > 4 => {
                    return throw("RangeError", "roundRect needs 1 to 4 radii")
                }
                Some(Arg::Nums(v)) => v.to_vec(),
                Some(other) => vec![num(Some(other))],
            };
            if list.iter().any(|v| !v.is_finite()) {
                return Ret::Undefined;
            }
            if list.iter().any(|v| *v < 0.0) {
                return throw("RangeError", "negative radius");
            }
            let r = match list.len() {
                1 => [list[0]; 4],
                2 => [list[0], list[1], list[0], list[1]],
                3 => [list[0], list[1], list[2], list[1]],
                _ => [list[0], list[1], list[2], list[3]],
            };
            path.round_rect(m, x, y, w, h, r);
        }
        ARC => {
            let Some([x, y, r, a0, a1]) = nums(a, 0) else { return Ret::Undefined };
            if r < 0.0 {
                return throw("IndexSizeError", "negative radius");
            }
            let ccw = truthy(a.get(5));
            path.ellipse(m, P::new(x, y), r, r, 0.0, a0, a1, ccw);
        }
        ELLIPSE => {
            let Some([x, y, rx, ry, rot, a0, a1]) = nums(a, 0) else { return Ret::Undefined };
            if rx < 0.0 || ry < 0.0 {
                return throw("IndexSizeError", "negative radius");
            }
            let ccw = truthy(a.get(7));
            path.ellipse(m, P::new(x, y), rx, ry, rot, a0, a1, ccw);
        }
        _ => {}
    }
    Ret::Undefined
}

pub fn truthy(a: Option<&Arg>) -> bool {
    match a {
        None | Some(Arg::Undefined) | Some(Arg::Null) => false,
        Some(Arg::Bool(b)) => *b,
        Some(Arg::Num(v)) => *v != 0.0 && !v.is_nan(),
        Some(Arg::Str(s)) => !s.is_empty(),
        _ => true,
    }
}

fn new_object_ok(objs: &Objects) -> Result<(), Ret> {
    if objs.full() {
        Err(throw("NotSupportedError", "too many canvas objects"))
    } else {
        Ok(())
    }
}

impl Canvas2d {
    pub(crate) fn dispatch(&mut self, fonts: &mut dyn Fonts, op: u16, a: &[Arg]) -> Ret {
        if (PROP_BASE..PROP_BASE + 2 * PROP_NAMES.len() as u16).contains(&op) {
            let i = ((op - PROP_BASE) / 2) as usize;
            if (op - PROP_BASE).is_multiple_of(2) {
                return self.st.get_prop(i);
            }
            self.st.set_prop(i, a.first(), &self.objs);
            return Ret::Undefined;
        }
        let m = self.st.m;
        match op {
            SAVE => {
                let (len, cap) = (self.stack.len(), self.stack.capacity());
                let grow = if len == cap { len.max(4).min(MAX_SAVE.saturating_sub(len)) } else { 0 };
                let need = state_extra(&self.st) + grow * std::mem::size_of::<State>();
                if len < MAX_SAVE && need <= self.room() {
                    self.stack.reserve_exact(grow);
                    let saved = self.st.clone();
                    self.stack_extra += state_extra(&saved);
                    self.stack.push(saved);
                }
            }
            RESTORE => {
                if let Some(s) = self.stack.pop() {
                    self.stack_extra = self.stack_extra.saturating_sub(state_extra(&s));
                    self.st = s;
                }
            }
            RESET => {
                for p in self.bmp.px.iter_mut() {
                    *p = 0;
                }
                self.st = State::default();
                self.stack = Vec::new();
                self.stack_extra = 0;
                self.path.clear();
                self.dirty = true;
            }
            IS_CONTEXT_LOST => return Ret::Bool(false),
            GET_CONTEXT_ATTRIBUTES => {
                return Ret::Str(
                    "{\"alpha\":true,\"colorSpace\":\"srgb\",\"desynchronized\":false,\"willReadFrequently\":false}"
                        .into(),
                )
            }
            SCALE => {
                if let Some([x, y]) = nums(a, 0) {
                    self.st.m = m.concat(&M::scale(x, y));
                }
            }
            ROTATE => {
                if let Some([r]) = nums(a, 0) {
                    self.st.m = m.concat(&M::rotate(r));
                }
            }
            TRANSLATE => {
                if let Some([x, y]) = nums(a, 0) {
                    self.st.m = m.concat(&M::translate(x, y));
                }
            }
            TRANSFORM => {
                if let Some(v) = nums::<6>(a, 0) {
                    self.st.m = m.concat(&M(v));
                }
            }
            GET_TRANSFORM => return Ret::Matrix(m.0),
            SET_TRANSFORM => {
                if a.iter().all(|x| matches!(x, Arg::Undefined)) {
                    self.st.m = M::ID;
                } else if let Some(v) = nums::<6>(a, 0) {
                    self.st.m = M(v);
                }
            }
            RESET_TRANSFORM => self.st.m = M::ID,
            CREATE_LINEAR_GRADIENT | CREATE_RADIAL_GRADIENT | CREATE_CONIC_GRADIENT => {
                return self.create_gradient(op, a)
            }
            CREATE_PATTERN => return self.create_pattern(a),
            CLEAR_RECT => {
                if let Some([x, y, w, h]) = nums(a, 0) {
                    self.clear_rect(x, y, w, h);
                }
            }
            FILL_RECT => {
                if let Some([x, y, w, h]) = nums(a, 0) {
                    if w != 0.0 && h != 0.0 {
                        let p = self.rect_path(x, y, w, h);
                        self.fill_flat(p.flatten(crate::draw::TOL), false);
                    }
                }
            }
            STROKE_RECT => {
                if let Some([x, y, w, h]) = nums(a, 0) {
                    if w != 0.0 || h != 0.0 {
                        let p = self.rect_path(x, y, w, h);
                        self.stroke_flat(p.flatten(crate::draw::TOL));
                    }
                }
            }
            BEGIN_PATH => self.path.clear(),
            FILL | CLIP => {
                let obj = path_arg(a);
                let r = match rule(a.get(obj.is_some() as usize)) {
                    Ok(r) => r,
                    Err(e) => return e,
                };
                if obj.is_none() && matches!(a.first(), Some(Arg::Object(..))) {
                    return throw("TypeError", "not a Path2D");
                }
                if op == FILL {
                    if let Some(p) = self.flat(obj) {
                        self.fill_flat(p, r);
                    }
                } else if obj.is_none() || self.objs.paths.contains_key(&obj.unwrap_or(0)) {
                    self.clip_obj(obj, r);
                }
            }
            STROKE => {
                if let Some(p) = self.flat(path_arg(a)) {
                    self.stroke_flat(p);
                }
            }
            IS_POINT_IN_PATH | IS_POINT_IN_STROKE => {
                let obj = path_arg(a);
                let k = obj.is_some() as usize;
                let (x, y) = (num(a.get(k)), num(a.get(k + 1)));
                let Some(p) = self.flat(obj) else { return Ret::Bool(false) };
                if op == IS_POINT_IN_PATH {
                    let r = match rule(a.get(k + 2)) {
                        Ok(r) => r,
                        Err(e) => return e,
                    };
                    return Ret::Bool(self.point_in_path(p, x, y, r));
                }
                return Ret::Bool(self.point_in_stroke(p, x, y));
            }
            DRAW_FOCUS_IF_NEEDED => {}
            FILL_TEXT | STROKE_TEXT => return self.fill_text(fonts, a, op == STROKE_TEXT),
            MEASURE_TEXT => return self.measure_text(fonts, a),
            DRAW_IMAGE => return self.draw_image(a),
            CREATE_IMAGE_DATA => return self.create_image_data(a),
            GET_IMAGE_DATA => return self.get_image_data(a),
            PUT_IMAGE_DATA => return self.put_image_data(a),
            SET_LINE_DASH => match a.first() {
                Some(Arg::Nums(v)) => {
                    let n = if v.len() % 2 == 1 { 2 * v.len() } else { v.len() };
                    let grow = (n * 8).saturating_sub(self.st.dash.capacity() * 8);
                    if v.len() <= 4096 && v.iter().all(|x| x.is_finite() && *x >= 0.0) && grow <= self.room() {
                        let mut d = v.to_vec();
                        if d.len() % 2 == 1 {
                            d.extend_from_slice(v);
                        }
                        d.shrink_to_fit();
                        self.st.dash = d;
                    }
                }
                _ => return throw("TypeError", "setLineDash needs a sequence"),
            },
            GET_LINE_DASH => return Ret::Nums(self.st.dash.clone()),
            CLOSE_PATH..=ELLIPSE => {
                let inv = m.invert();
                self.path.limit = self.path.segs.capacity() + self.room() / SEG_BYTES;
                let mut path = std::mem::take(&mut self.path);
                let r = build(&mut path, &m, inv, op, a);
                self.path = path;
                return r;
            }
            ADD_COLOR_STOP => return self.add_color_stop(a),
            PATTERN_SET_TRANSFORM => {
                if let Some(Arg::Object(Kind::Pattern, h)) = a.first() {
                    if let Some(p) = self.objs.pats.get(h) {
                        if a.len() <= 1 || a[1..].iter().all(|x| matches!(x, Arg::Undefined)) {
                            p.borrow_mut().m = M::ID;
                        } else if let Some(v) = nums::<6>(a, 1) {
                            p.borrow_mut().m = M(v);
                        }
                    }
                }
            }
            PATH2D_NEW => {
                if let Err(e) = new_object_ok(&self.objs) {
                    return e;
                }
                let room = self.room();
                let fixed = self.objs.grow_bytes(Kind::Path);
                if room <= fixed {
                    return throw("NotSupportedError", "canvas memory budget exhausted");
                }
                let limit = (room - fixed) / SEG_BYTES;
                let mut p = match a.first() {
                    Some(Arg::Object(Kind::Path, h)) => match self.objs.paths.get(h) {
                        Some(src) if src.len() > limit => {
                            return throw("NotSupportedError", "canvas memory budget exhausted")
                        }
                        Some(src) => src.clone(),
                        None => Path::new(),
                    },
                    Some(Arg::Str(s)) => svgpath::parse(s, limit),
                    None | Some(Arg::Undefined) => Path::new(),
                    Some(other) => svgpath::parse(&crate::state::text_of(Some(other)), limit),
                };
                p.limit = MAX_SEGS;
                let h = self.objs.handle();
                self.objs.insert_path(h, p);
                return Ret::Object(Kind::Path, h);
            }
            PATH2D_ADD_PATH => {
                let (Some(Arg::Object(Kind::Path, dst)), Some(Arg::Object(Kind::Path, src))) = (a.first(), a.get(1))
                else {
                    return throw("TypeError", "addPath needs a Path2D");
                };
                let tm = if a.len() <= 2 || a[2..].iter().all(|x| matches!(x, Arg::Undefined)) {
                    Some(M::ID)
                } else {
                    a.get(2..8).and_then(|s| {
                        let v: Vec<f64> = s.iter().map(|x| num(Some(x))).collect();
                        matrix_of(&v)
                    })
                };
                let Some(tm) = tm else { return Ret::Undefined };
                let room = self.room() / SEG_BYTES;
                if !self.objs.paths.contains_key(dst) {
                    return Ret::Undefined;
                }
                let Some(src) = self.objs.paths.get(src).cloned() else { return Ret::Undefined };
                if let Some(d) = self.objs.paths.get_mut(dst) {
                    let before = path_obj_bytes(d);
                    d.limit = d.segs.capacity() + room;
                    d.add_path(&src, &tm);
                    d.limit = MAX_SEGS;
                    self.objs.path_bytes += path_obj_bytes(d).saturating_sub(before);
                }
            }
            o if o > PATH2D_OFFSET && o <= PATH2D_OFFSET + ELLIPSE => {
                let Some(Arg::Object(Kind::Path, h)) = a.first() else {
                    return throw("TypeError", "not a Path2D");
                };
                let room = self.room() / SEG_BYTES;
                let Some(p) = self.objs.paths.get_mut(h) else { return Ret::Undefined };
                let before = path_obj_bytes(p);
                p.limit = p.segs.capacity() + room;
                let r = build(p, &M::ID, Some(M::ID), o - PATH2D_OFFSET, &a[1..]);
                p.limit = MAX_SEGS;
                // Path building never shrinks a path's allocation.
                self.objs.path_bytes += path_obj_bytes(p).saturating_sub(before);
                return r;
            }
            _ => {}
        }
        Ret::Undefined
    }

    fn create_gradient(&mut self, op: u16, a: &[Arg]) -> Ret {
        if let Err(e) = new_object_ok(&self.objs) {
            return e;
        }
        if GRAD_OBJ + self.objs.grow_bytes(Kind::Gradient) > self.room() {
            return throw("NotSupportedError", "canvas memory budget exhausted");
        }
        let kind = match op {
            CREATE_LINEAR_GRADIENT => {
                let Some([x0, y0, x1, y1]) = nums(a, 0) else { return throw("TypeError", "non-finite") };
                GradKind::Linear { p0: P::new(x0, y0), p1: P::new(x1, y1) }
            }
            CREATE_RADIAL_GRADIENT => {
                let Some([x0, y0, r0, x1, y1, r1]) = nums(a, 0) else { return throw("TypeError", "non-finite") };
                if r0 < 0.0 || r1 < 0.0 {
                    return throw("IndexSizeError", "negative radius");
                }
                GradKind::Radial { c0: P::new(x0, y0), r0, c1: P::new(x1, y1), r1 }
            }
            _ => {
                let Some([angle, x, y]) = nums(a, 0) else { return throw("TypeError", "non-finite") };
                GradKind::Conic { angle, c: P::new(x, y) }
            }
        };
        let h = self.objs.handle();
        let mut g = Gradient::new(kind);
        g.charge = Some(Charge::new(GRAD_OBJ, &self.held));
        self.objs.grads.insert(h, Rc::new(RefCell::new(g)));
        Ret::Object(Kind::Gradient, h)
    }

    fn add_color_stop(&mut self, a: &[Arg]) -> Ret {
        let Some(Arg::Object(Kind::Gradient, h)) = a.first() else {
            return throw("TypeError", "not a CanvasGradient");
        };
        let off = num(a.get(1));
        if !off.is_finite() {
            return throw("TypeError", "non-finite offset");
        }
        if !(0.0..=1.0).contains(&off) {
            return throw("IndexSizeError", "offset out of range");
        }
        let Some(Arg::Color(c)) = a.get(2) else {
            return throw("SyntaxError", "invalid colour");
        };
        if STOP_BYTES > self.room() {
            return Ret::Undefined;
        }
        if let Some(g) = self.objs.grads.get(h) {
            let mut g = g.borrow_mut();
            g.add_stop(off, *c);
            let b = grad_bytes(&g);
            if let Some(ch) = g.charge.as_mut() {
                ch.set(b);
            }
        }
        Ret::Undefined
    }
}
