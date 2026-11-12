//! Animations on a page: CSS transitions and animations start and stop when styles change;
//! every animation moves along with the page clock, once per frame.

use super::Page;
use crate::anim::interp::{self, can_blend, computed_value, TRANSITIONABLE};
use crate::anim::{Direction, Easing, Fill, Keyframe, Phase, Source, Timeline, Timing};
use crate::atom::Atom;
use crate::css::properties::{longhands, PropId, TransProp, UrlResolver, Value};
use crate::dom::{Document, NodeId, NO_NODE};
use crate::style::{cyc, ComputedStyle, Display, Overrides, StyleStore, Stylist, NO_STYLE};
use std::collections::{HashMap, HashSet};

/// At most this many animations exist on a page at once (new ones are refused beyond it).
const MAX_ANIMATIONS: usize = 20_000;

struct Sync<'a> {
    styles: &'a StyleStore,
    old: &'a StyleStore,
    doc: &'a Document,
    stylist: &'a Stylist,
    now: f64,
    resolve: UrlResolver<'a>,
}

/// True if an ancestor of `node` has a running animation of `prop` (the element then follows
/// its ancestor's animated value and must not start a transition of its own every frame).
fn ancestor_animates(tl: &Timeline, doc: &Document, node: NodeId, prop: PropId, now: f64) -> bool {
    if !tl.anims.iter().any(|a| a.node != node && a.props.contains(&prop) && a.running(now)) {
        return false;
    }
    let mut p = doc.parent(node);
    while p != NO_NODE {
        if tl.anims.iter().any(|a| a.node == p && a.props.contains(&prop) && a.running(now)) {
            return true;
        }
        p = doc.parent(p);
    }
    false
}

/// Start transitions for values of `node` that differ between its old and new style.
fn start_transitions(tl: &mut Timeline, node: NodeId, ob: &ComputedStyle, st: &ComputedStyle, cx: &Sync) -> bool {
    // (property, duration, delay, timing function): later list entries win
    let mut wanted: Vec<(PropId, f32, f32, Easing)> = Vec::new();
    if let Some(tr) = st.transitions.as_ref().filter(|t| t.any()) {
        let all = [TransProp::All];
        let props: &[TransProp] = if tr.props.is_empty() { &all } else { &tr.props };
        for (i, tp) in props.iter().enumerate() {
            let (dur, delay) = (cyc(&tr.durations, i, 0.0), cyc(&tr.delays, i, 0.0));
            let ease = cyc(&tr.easings, i, Easing::EASE);
            let mut add = |p: PropId| {
                if TRANSITIONABLE.contains(&p) {
                    wanted.retain(|w| w.0 != p);
                    wanted.push((p, dur, delay, ease.clone()));
                }
            };
            match tp {
                TransProp::All => TRANSITIONABLE.iter().for_each(|p| add(*p)),
                TransProp::Prop(p) if p.is_shorthand() => longhands(*p).iter().for_each(|l| add(*l)),
                TransProp::Prop(p) => add(*p),
                TransProp::None | TransProp::Other => {}
            }
        }
    }
    let running: Vec<(u32, PropId)> = tl
        .anims
        .iter()
        .filter(|a| a.node == node)
        .filter_map(|a| match a.source {
            Source::Transition(p) => Some((a.id, p)),
            _ => None,
        })
        .collect();
    if wanted.is_empty() && running.is_empty() {
        return false;
    }
    let mut changed = false;
    for (p, dur, delay, ease) in &wanted {
        let Some(new_v) = computed_value(st, *p) else { continue };
        let mut dur = *dur as f64;
        let existing = running.iter().find(|(_, q)| q == p).map(|(id, _)| *id);
        let from = match existing.and_then(|id| tl.get(id).map(|a| (id, a.clone()))) {
            Some((id, a)) => {
                if a.target.as_ref() == Some(&new_v) {
                    continue; // already on its way there
                }
                let mut out = Vec::new();
                a.sample(cx.now, &|_| None, &mut out);
                // turning back half-way takes as long as it took to get here
                let back_to_start = a.keyframes.first().and_then(|k| k.values.first()).map_or(false, |(_, v)| *v == new_v);
                if back_to_start {
                    dur *= a.progress(cx.now).unwrap_or(1.0).clamp(0.0, 1.0) as f64;
                }
                tl.remove(id);
                changed = true;
                out.pop().map(|(_, v)| v)
            }
            None => None,
        };
        let old_v = match from.or_else(|| computed_value(ob, *p)) {
            Some(v) => v,
            None => continue,
        };
        if old_v == new_v || dur + (*delay as f64).max(0.0) <= 0.0 || !can_blend(&old_v, &new_v) {
            continue;
        }
        // a value that only changed because an ancestor's animated value is passed down must
        // not start a transition of its own (it would trail behind, every frame)
        if interp::inherited(*p) && ancestor_animates(tl, cx.doc, node, *p, cx.now) {
            let passed_down = cx.styles.get(cx.doc.parent(node)).and_then(|ps| computed_value(ps, *p));
            if passed_down.as_ref() == Some(&new_v) {
                continue;
            }
        }
        if tl.anims.len() >= MAX_ANIMATIONS {
            break;
        }
        let frames = vec![
            Keyframe { offset: 0.0, easing: None, values: vec![(*p, old_v)] },
            Keyframe { offset: 1.0, easing: None, values: vec![(*p, new_v.clone())] },
        ];
        let timing = Timing { duration: dur, delay: *delay as f64, easing: ease.clone(), fill: Fill::Backwards, ..Timing::default() };
        let a = tl.add(node, Source::Transition(*p), frames, timing);
        a.target = Some(new_v);
        a.play(cx.now);
        changed = true;
    }
    // a transition whose property is no longer listed ends at once if its goal moved
    for (id, p) in running {
        if wanted.iter().any(|w| w.0 == p) {
            continue;
        }
        let stale = tl.get(id).map_or(false, |a| a.target != computed_value(st, p));
        if stale {
            tl.remove(id);
            changed = true;
        }
    }
    changed
}

/// Bring the CSS animations of `node` in line with its `animation-*` properties.
fn sync_css_animations(tl: &mut Timeline, node: NodeId, st: &ComputedStyle, cx: &Sync) -> bool {
    let mut changed = false;
    let mut desired: Vec<(Atom, u32)> = Vec::new();
    if let Some(an) = &st.animations {
        for (i, name) in an.names.iter().enumerate() {
            if *name == Atom::NONE {
                continue;
            }
            let key = (*name, i as u32);
            desired.push(key);
            let paused = cyc(&an.paused, i, false);
            if let Some(a) = tl.anims.iter_mut().find(|a| a.node == node && a.source == Source::Css(key.0, key.1)) {
                if paused && !a.paused() {
                    a.hold = Some(a.time(cx.now).unwrap_or(0.0));
                    a.start = None;
                    changed = true;
                } else if !paused && a.paused() {
                    let at = a.hold.take().unwrap_or(0.0);
                    a.start = Some(cx.now - at / a.rate);
                    changed = true;
                }
                continue;
            }
            if tl.anims.len() >= MAX_ANIMATIONS {
                continue;
            }
            let ease = cyc(&an.easings, i, Easing::EASE);
            let Some(frames) = cx.stylist.css_keyframes(*name, st, &ease, cx.resolve) else { continue };
            if frames.is_empty() {
                continue;
            }
            let timing = Timing {
                duration: cyc(&an.durations, i, 0.0) as f64,
                delay: cyc(&an.delays, i, 0.0) as f64,
                iterations: cyc(&an.iterations, i, 1.0) as f64,
                direction: cyc(&an.directions, i, Direction::Normal),
                fill: cyc(&an.fills, i, Fill::None),
                easing: Easing::Linear,
                ..Timing::default()
            };
            let a = tl.add(node, Source::Css(key.0, key.1), frames, timing);
            if paused {
                a.hold = Some(0.0);
            } else {
                a.play(cx.now);
            }
            changed = true;
        }
    }
    let before = tl.anims.len();
    tl.anims.retain(|a| a.node != node || !matches!(&a.source, Source::Css(n, i) if !desired.contains(&(*n, *i))));
    changed | (tl.anims.len() != before)
}

fn sync(tl: &mut Timeline, cx: &Sync) -> bool {
    let mut changed = false;
    let first = cx.old.of_node.is_empty();
    let animated: HashSet<NodeId> = tl.anims.iter().filter(|a| a.source != Source::Script).map(|a| a.node).collect();
    let drop_css = |tl: &mut Timeline, node: NodeId| {
        let before = tl.anims.len();
        tl.anims.retain(|a| a.node != node || a.source == Source::Script);
        tl.anims.len() != before
    };
    for node in 0..cx.styles.of_node.len() as NodeId {
        let Some(st) = cx.styles.base_style(node) else {
            if animated.contains(&node) {
                changed |= drop_css(tl, node);
            }
            continue;
        };
        let has_tr = st.transitions.as_ref().map_or(false, |t| t.any());
        let has_an = st.animations.as_ref().map_or(false, |a| a.names.iter().any(|n| *n != Atom::NONE));
        if !has_tr && !has_an && !animated.contains(&node) {
            continue;
        }
        if st.display == Display::None {
            changed |= drop_css(tl, node);
            continue;
        }
        if !first {
            if let Some(ob) = cx.old.base_style(node).filter(|ob| ob.display != Display::None) {
                changed |= start_transitions(tl, node, ob, st, cx);
            }
        }
        changed |= sync_css_animations(tl, node, st, cx);
    }
    // elements that are gone
    let n = cx.styles.of_node.len() as NodeId;
    let before = tl.anims.len();
    tl.anims.retain(|a| a.source == Source::Script || a.node < n);
    changed | (tl.anims.len() != before)
}

impl Page {
    /// Styles were just recomputed (`old` is what they were): start transitions for values that
    /// changed and bring CSS animations in line with `animation-name`. Returns true if any
    /// animation was added or removed.
    pub(super) fn sync_animations(&mut self, old: &StyleStore) -> bool {
        let join = self.join.clone();
        let base = self.base_url.clone();
        let resolve = move |u: &str| join(&base, u);
        let cx = Sync { styles: &self.styles, old, doc: &self.doc, stylist: &self.stylist, now: self.clock_ms, resolve: &resolve };
        sync(&mut self.timeline, &cx)
    }

    /// Sample every animation at `now` and lay the result over the styles, redoing as little
    /// as the changed properties allow.
    pub(super) fn refresh_overrides(&mut self, now: f64) {
        let mut new: Overrides = HashMap::new();
        if !self.timeline.is_empty() {
            let styles = &self.styles;
            let under = |node: NodeId, prop: PropId| styles.base_style(node).and_then(|st| computed_value(st, prop));
            for (node, list) in self.timeline.sample(now, &under) {
                if styles.id_of(node) != NO_STYLE {
                    new.insert(node, list);
                }
            }
        }
        if new == self.anim_over {
            return;
        }
        let mut touched: Vec<NodeId> = new.iter().filter(|(n, l)| self.anim_over.get(*n) != Some(*l)).map(|(n, _)| *n).collect();
        touched.extend(self.anim_over.keys().filter(|n| !new.contains_key(*n)).copied());
        let mut full = self.dirty_style;
        let mut layout = false;
        let none: [(PropId, Value); 0] = [];
        for node in &touched {
            let now_list: &[(PropId, Value)] = new.get(node).map_or(&none, |v| v.as_slice());
            let old_list: &[(PropId, Value)] = self.anim_over.get(node).map_or(&none, |v| v.as_slice());
            let value_in = |list: &[(PropId, Value)], p: PropId| list.iter().find(|(q, _)| *q == p).map(|(_, v)| v.clone());
            for (p, _) in now_list.iter().chain(old_list.iter()) {
                if value_in(now_list, *p) != value_in(old_list, *p) {
                    full |= interp::inherited(*p);
                    layout |= !interp::paint_only(*p);
                }
            }
        }
        self.anim_over = new;
        if full {
            self.dirty_style = true;
            return;
        }
        for node in touched {
            let over: &[(PropId, Value)] = self.anim_over.get(&node).map_or(&none, |v| v.as_slice());
            let fresh = !self.styles.base.contains_key(&node);
            let had_transform = self.styles.get(node).map_or(false, |s| s.has_transform());
            if !self.styles.reanimate(&self.doc, node, over) {
                continue;
            }
            // boxes still point at the style the element shared, and a box that gains or loses
            // its transform changes how its subtree is stacked
            let has_transform = self.styles.get(node).map_or(false, |s| s.has_transform());
            layout |= fresh || had_transform != has_transform;
        }
        if layout {
            self.dirty_layout = true;
        } else {
            self.dirty_paint = true;
        }
    }

    /// Move animations to `now`: send the events that are due, drop what has ended, and lay
    /// the current values over the styles. Returns true while any animation still needs frames.
    pub(super) fn tick_animations(&mut self, now: f64) -> bool {
        if self.timeline.is_empty() {
            if !self.anim_over.is_empty() {
                self.refresh_overrides(now);
            }
            return false;
        }
        let mut events: Vec<(NodeId, &'static str, String, f64)> = Vec::new();
        let mut done: Vec<u32> = Vec::new();
        for a in &mut self.timeline.anims {
            if !a.start_told && matches!(a.phase(now), Phase::Active | Phase::After) {
                a.start_told = true;
                match &a.source {
                    Source::Css(name, _) => events.push((a.node, "animationstart", name.as_str().to_string(), 0.0)),
                    Source::Transition(p) => events.push((a.node, "transitionstart", p.name().to_string(), 0.0)),
                    Source::Script => {}
                }
            }
            if !a.finish_told && a.finished(now) {
                a.finish_told = true;
                let elapsed = a.active_duration() / 1000.0;
                match &a.source {
                    Source::Transition(p) => events.push((a.node, "transitionend", p.name().to_string(), elapsed)),
                    Source::Css(name, _) => events.push((a.node, "animationend", name.as_str().to_string(), elapsed)),
                    Source::Script => done.push(a.id),
                }
            }
        }
        // what has ended and leaves nothing behind goes away
        self.timeline.anims.retain(|a| {
            !(a.finish_told
                && match a.source {
                    Source::Transition(_) => true,
                    Source::Script => !matches!(a.timing.fill, Fill::Forwards | Fill::Both),
                    Source::Css(..) => false,
                })
        });
        self.refresh_overrides(now);
        if self.script.is_some() && (!events.is_empty() || !done.is_empty()) {
            let doc_len = self.doc.nodes.len() as NodeId;
            events.retain(|e| e.0 < doc_len);
            self.script(|e| {
                for (node, kind, name, elapsed) in &events {
                    e.animation_event(*node, kind, name, *elapsed);
                }
                if !done.is_empty() {
                    e.animations_finished(&done);
                }
            });
        }
        self.animations_need_frames()
    }

    /// True if the page needs frames without any input: an animation is running, or a script
    /// asked to be called before the next frame.
    pub fn wants_frame(&self) -> bool {
        self.animations_need_frames() || self.script_frame_due()
    }

    /// True if a running animation needs frames: one on an element that is shown, or one that
    /// will end by itself (its end has to be reported even if nothing shows it).
    fn animations_need_frames(&self) -> bool {
        let now = self.clock_ms;
        self.timeline.anims.iter().any(|a| a.running(now) && (a.end_time().is_finite() || self.styles.id_of(a.node) != NO_STYLE))
    }

    /// Scripts have something to do before the next frame is drawn.
    fn script_frame_due(&self) -> bool {
        self.script.as_ref().map_or(false, |e| e.frame_wanted() || (self.geom_changed && e.observing()))
    }

    /// Call once per displayed frame, before `update`: runs the scripts that asked for this
    /// frame, then moves animations to `now_ms` (the clock of `pump`). Returns true if the
    /// picture may have changed.
    pub fn frame(&mut self, now_ms: f64) -> bool {
        self.clock_ms = self.clock_ms.max(now_ms);
        let now = self.clock_ms;
        // In the order a frame is specified to happen: animations move and send their events,
        // then scripts' frame callbacks run, then the observers. (A script that reacts to an
        // animation ending expects its frame callback to follow in the same frame.)
        self.tick_animations(now);
        let mut ran = false;
        if self.script_frame_due() {
            self.geom_changed = false;
            self.script(|e| e.run_frame(now));
            ran = true;
            // animations those callbacks started or stopped show in this very frame
            self.refresh_overrides(now);
        }
        ran || self.needs_update() || std::mem::take(&mut self.script_redraw)
    }

    // ───────────── script animations (`Element.animate`) ─────────────

    /// Create and start a script animation. Returns its id (0 if refused).
    pub(crate) fn script_animate(&mut self, node: NodeId, frames: Vec<Keyframe>, timing: Timing) -> u32 {
        if self.timeline.anims.len() >= MAX_ANIMATIONS {
            return 0;
        }
        let now = self.clock_ms;
        let a = self.timeline.add(node, Source::Script, frames, timing);
        a.play(now);
        let (id, props) = (a.id, a.props.clone());
        // older finished animations of this element that the new one covers completely can
        // never show again: forget them (they would otherwise pile up, one per hover)
        self.timeline.anims.retain(|o| {
            !(o.id != id && o.node == node && o.source == Source::Script && o.finished(now) && o.props.iter().all(|p| props.contains(p)))
        });
        id
    }

    /// Control or query a script animation. Unknown ids answer `None`.
    pub(crate) fn script_animation(&mut self, id: u32, what: &str, arg: f64) -> Option<f64> {
        let now = self.clock_ms;
        if what == "cancel" {
            return self.timeline.remove(id).map(|_| 0.0);
        }
        let a = self.timeline.get(id)?;
        let end = a.end_time();
        if matches!(what, "seek" | "setRate") && !arg.is_finite() {
            return Some(0.0);
        }
        Some(match what {
            // an animation that has run out stays at its end
            "time" => {
                let t = a.time(now).unwrap_or(0.0);
                let t = if a.paused() {
                    t
                } else if a.rate >= 0.0 {
                    t.min(end)
                } else {
                    t.max(0.0)
                };
                // whole milliseconds, like every clock script can read
                t.floor()
            }
            "start" => a.start.map_or(f64::NAN, f64::floor),
            "rate" => a.rate,
            "end" => end,
            // 0 idle, 1 running, 2 paused, 3 finished
            "state" => {
                if a.paused() {
                    2.0
                } else if a.finished(now) {
                    3.0
                } else if a.start.is_some() {
                    1.0
                } else {
                    0.0
                }
            }
            "pause" => {
                if !a.paused() {
                    a.hold = Some(a.time(now).unwrap_or(0.0));
                    a.start = None;
                }
                0.0
            }
            "play" => {
                let at = a.time(now).unwrap_or(0.0);
                let over = if a.rate >= 0.0 { at >= end || at < 0.0 } else { at <= 0.0 || at > end };
                let from = if over {
                    if a.rate >= 0.0 {
                        0.0
                    } else if end.is_finite() {
                        end
                    } else {
                        // playing an endless animation backwards from its end is not possible
                        return Some(0.0);
                    }
                } else {
                    at
                };
                a.hold = None;
                a.start = Some(now - from / a.rate);
                a.finish_told = false;
                0.0
            }
            "finish" => {
                let to = if a.rate >= 0.0 { end } else { 0.0 };
                if to.is_finite() {
                    a.hold = None;
                    a.start = Some(now - to / a.rate);
                }
                0.0
            }
            "seek" => {
                if a.paused() {
                    a.hold = Some(arg);
                } else {
                    a.start = Some(now - arg / a.rate);
                }
                a.finish_told = false;
                0.0
            }
            "setRate" => {
                if arg != 0.0 && arg.is_finite() {
                    let at = a.time(now).unwrap_or(0.0);
                    a.rate = arg;
                    if !a.paused() {
                        a.start = Some(now - at / a.rate);
                    }
                    a.finish_told = false;
                }
                0.0
            }
            _ => return None,
        })
    }
}
