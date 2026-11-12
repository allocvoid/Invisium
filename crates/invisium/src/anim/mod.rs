//! Animations: CSS transitions, CSS `@keyframes` animations and script animations
//! (`Element.animate`) share one model. An animation is a list of keyframes on one element plus
//! timing; sampling it at a time gives values that are laid over the element's style.
//!
//! Times are milliseconds on the page clock (the one passed to `Page::pump`).

pub mod easing;
pub mod interp;

use crate::atom::Atom;
use crate::css::properties::{PropId, Value};
use crate::dom::NodeId;
pub use easing::Easing;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fill {
    None,
    Forwards,
    Backwards,
    Both,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Normal,
    Reverse,
    Alternate,
    AlternateReverse,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Timing {
    pub delay: f64,
    pub end_delay: f64,
    /// Length of one iteration.
    pub duration: f64,
    /// May be infinite.
    pub iterations: f64,
    pub iteration_start: f64,
    pub direction: Direction,
    pub fill: Fill,
    pub easing: Easing,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            delay: 0.0,
            end_delay: 0.0,
            duration: 0.0,
            iterations: 1.0,
            iteration_start: 0.0,
            direction: Direction::Normal,
            fill: Fill::None,
            easing: Easing::Linear,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Keyframe {
    /// 0..=1
    pub offset: f32,
    /// Timing function towards the next keyframe (`None` = linear).
    pub easing: Option<Easing>,
    pub values: Vec<(PropId, Value)>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Source {
    /// A CSS transition of this property.
    Transition(PropId),
    /// A CSS animation: its `@keyframes` name and its position in `animation-name`.
    Css(Atom, u32),
    /// `Element.animate`.
    Script,
}

#[derive(Clone, Debug)]
pub struct Animation {
    pub id: u32,
    pub node: NodeId,
    pub source: Source,
    /// Sorted by offset.
    pub keyframes: Vec<Keyframe>,
    /// Every property some keyframe sets.
    pub props: Vec<PropId>,
    pub timing: Timing,
    /// Page time at which the animation's own time was zero (while it runs).
    pub start: Option<f64>,
    /// The animation's own time while it is paused or has finished.
    pub hold: Option<f64>,
    pub rate: f64,
    /// Script sees it as finished (set once, by the page).
    pub finish_told: bool,
    pub start_told: bool,
    /// For a transition: the value it is heading to.
    pub target: Option<Value>,
}

/// Where an animation's own time is relative to its active interval.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Cancelled or never started: no time at all.
    Idle,
    Before,
    Active,
    After,
}

impl Animation {
    pub fn new(id: u32, node: NodeId, source: Source, mut keyframes: Vec<Keyframe>, timing: Timing) -> Animation {
        keyframes.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap_or(std::cmp::Ordering::Equal));
        let mut props: Vec<PropId> = Vec::new();
        for k in &keyframes {
            for (p, _) in &k.values {
                if !props.contains(p) {
                    props.push(*p);
                }
            }
        }
        Animation { id, node, source, keyframes, props, timing, start: None, hold: None, rate: 1.0, finish_told: false, start_told: false, target: None }
    }

    /// Start (or restart) playing from the animation's own time zero at page time `now`.
    pub fn play(&mut self, now: f64) {
        self.start = Some(now);
        self.hold = None;
    }

    /// The animation's own time at page time `now`.
    pub fn time(&self, now: f64) -> Option<f64> {
        if let Some(h) = self.hold {
            return Some(h);
        }
        self.start.map(|s| (now - s) * self.rate)
    }

    pub fn active_duration(&self) -> f64 {
        if self.timing.duration <= 0.0 || self.timing.iterations <= 0.0 {
            0.0
        } else {
            self.timing.duration * self.timing.iterations
        }
    }

    /// The time at which everything is over, including the delays.
    pub fn end_time(&self) -> f64 {
        (self.timing.delay + self.active_duration() + self.timing.end_delay).max(0.0)
    }

    pub fn phase(&self, now: f64) -> Phase {
        let Some(t) = self.time(now) else { return Phase::Idle };
        let d = self.timing.delay;
        let backwards = self.rate < 0.0;
        if t < d || (backwards && t == d) {
            Phase::Before
        } else if t > d + self.active_duration() || (!backwards && t == d + self.active_duration()) {
            Phase::After
        } else {
            Phase::Active
        }
    }

    /// True once the animation's time has reached its end in the direction it is playing.
    pub fn finished(&self, now: f64) -> bool {
        match self.time(now) {
            None => false,
            // a time that is not a number can never reach the end: treat it as over
            Some(t) if t.is_nan() => true,
            Some(t) => {
                if self.rate >= 0.0 {
                    t >= self.end_time()
                } else {
                    t <= 0.0
                }
            }
        }
    }

    pub fn paused(&self) -> bool {
        self.hold.is_some() && self.start.is_none()
    }

    /// Whether a frame is needed to move this animation along.
    pub fn running(&self, now: f64) -> bool {
        self.start.is_some() && self.hold.is_none() && !self.finished(now)
    }

    /// How far through the current iteration the animation is, after direction and the timing
    /// function. `None` when the animation contributes nothing at this time.
    pub fn progress(&self, now: f64) -> Option<f32> {
        let phase = self.phase(now);
        let t = self.time(now)?;
        let tm = &self.timing;
        let active = match phase {
            Phase::Idle => return None,
            Phase::Before => {
                if !matches!(tm.fill, Fill::Backwards | Fill::Both) {
                    return None;
                }
                0.0
            }
            Phase::After => {
                if !matches!(tm.fill, Fill::Forwards | Fill::Both) {
                    return None;
                }
                self.active_duration()
            }
            Phase::Active => t - tm.delay,
        };
        let overall = if tm.duration <= 0.0 {
            if phase == Phase::Before {
                0.0
            } else {
                tm.iterations
            }
        } else {
            active / tm.duration
        } + tm.iteration_start;
        let overall = if overall.is_finite() { overall } else { 0.0 };
        let at_end = phase == Phase::After || (phase == Phase::Active && active >= self.active_duration() && self.active_duration() > 0.0);
        let mut iteration = overall.floor();
        let mut simple = overall - iteration;
        if simple == 0.0 && at_end && overall > 0.0 {
            simple = 1.0;
            iteration -= 1.0;
        }
        let odd = (iteration as i64).rem_euclid(2) == 1;
        let forwards = match tm.direction {
            Direction::Normal => true,
            Direction::Reverse => false,
            Direction::Alternate => !odd,
            Direction::AlternateReverse => odd,
        };
        let directed = if forwards { simple } else { 1.0 - simple };
        Some(tm.easing.at(directed as f32))
    }

    /// The value of every animated property at `now`. `under` gives the element's own value for
    /// properties that a keyframe at the start or end leaves out.
    pub fn sample(&self, now: f64, under: &dyn Fn(PropId) -> Option<Value>, out: &mut Vec<(PropId, Value)>) {
        let Some(p) = self.progress(now) else { return };
        for &prop in &self.props {
            // keyframes that mention this property, with implied ends
            let mut frames: Vec<(f32, Option<&Easing>, std::borrow::Cow<Value>)> = Vec::new();
            for k in &self.keyframes {
                if let Some((_, v)) = k.values.iter().rev().find(|(q, _)| *q == prop) {
                    frames.push((k.offset, k.easing.as_ref(), std::borrow::Cow::Borrowed(v)));
                }
            }
            if frames.is_empty() {
                continue;
            }
            if frames[0].0 > 0.0 {
                match under(prop) {
                    Some(v) => frames.insert(0, (0.0, None, std::borrow::Cow::Owned(v))),
                    None => continue,
                }
            }
            if frames[frames.len() - 1].0 < 1.0 {
                match under(prop) {
                    Some(v) => frames.push((1.0, None, std::borrow::Cow::Owned(v))),
                    None => continue,
                }
            }
            if frames.len() == 1 {
                out.push((prop, frames[0].2.clone().into_owned()));
                continue;
            }
            let mut i = 0;
            while i + 2 < frames.len() && p >= frames[i + 1].0 {
                i += 1;
            }
            let (o0, ease, v0) = (&frames[i].0, frames[i].1, &frames[i].2);
            let (o1, v1) = (&frames[i + 1].0, &frames[i + 1].2);
            let mut t = if o1 > o0 { (p - o0) / (o1 - o0) } else if p < *o0 { 0.0 } else { 1.0 };
            if let Some(e) = ease {
                t = e.at(t);
            }
            out.push((prop, interp::interpolate(prop, v0, v1, t)));
        }
    }
}

/// Every animation of a page.
#[derive(Default)]
pub struct Timeline {
    pub anims: Vec<Animation>,
    next_id: u32,
}

impl Timeline {
    pub fn is_empty(&self) -> bool {
        self.anims.is_empty()
    }

    pub fn add(&mut self, node: NodeId, source: Source, keyframes: Vec<Keyframe>, timing: Timing) -> &mut Animation {
        self.next_id += 1;
        self.anims.push(Animation::new(self.next_id, node, source, keyframes, timing));
        let last = self.anims.len() - 1;
        &mut self.anims[last]
    }

    pub fn get(&mut self, id: u32) -> Option<&mut Animation> {
        self.anims.iter_mut().find(|a| a.id == id)
    }

    pub fn remove(&mut self, id: u32) -> Option<Animation> {
        let i = self.anims.iter().position(|a| a.id == id)?;
        Some(self.anims.remove(i))
    }

    /// True if any animation needs frames to keep moving.
    pub fn any_running(&self, now: f64) -> bool {
        self.anims.iter().any(|a| a.running(now))
    }

    /// The values laid over each element's style at `now`, in composite order: transitions,
    /// then CSS animations, then script animations (later ones win), by element.
    pub fn sample(&self, now: f64, under: &dyn Fn(NodeId, PropId) -> Option<Value>) -> Vec<(NodeId, Vec<(PropId, Value)>)> {
        let rank = |s: &Source| match s {
            Source::Transition(_) => 0,
            Source::Css(..) => 1,
            Source::Script => 2,
        };
        let mut order: Vec<&Animation> = self.anims.iter().collect();
        order.sort_by_key(|a| (rank(&a.source), a.id));
        let mut out: Vec<(NodeId, Vec<(PropId, Value)>)> = Vec::new();
        let mut slot_of: std::collections::HashMap<NodeId, usize> = std::collections::HashMap::new();
        let mut tmp: Vec<(PropId, Value)> = Vec::new();
        for a in order {
            tmp.clear();
            let node = a.node;
            a.sample(now, &|p| under(node, p), &mut tmp);
            if tmp.is_empty() {
                continue;
            }
            let slot = *slot_of.entry(node).or_insert_with(|| {
                out.push((node, Vec::new()));
                out.len() - 1
            });
            for (p, v) in tmp.drain(..) {
                let list = &mut out[slot].1;
                match list.iter_mut().find(|(q, _)| *q == p) {
                    Some(e) => e.1 = v,
                    None => list.push((p, v)),
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::values::Len;

    fn fade(timing: Timing) -> Animation {
        let frames = vec![
            Keyframe { offset: 0.0, easing: None, values: vec![(PropId::Opacity, Value::Num(0.0))] },
            Keyframe { offset: 1.0, easing: None, values: vec![(PropId::Opacity, Value::Num(1.0))] },
        ];
        let mut a = Animation::new(1, 7, Source::Script, frames, timing);
        a.play(1000.0);
        a
    }

    fn opacity(a: &Animation, now: f64) -> Option<f32> {
        let mut out = Vec::new();
        a.sample(now, &|_| None, &mut out);
        out.first().map(|(_, v)| match v {
            Value::Num(n) => *n,
            _ => f32::NAN,
        })
    }

    #[test]
    fn timing_phases_and_fill() {
        let a = fade(Timing { duration: 200.0, delay: 100.0, ..Timing::default() });
        // before the delay is over and after the end there is nothing without a fill
        assert_eq!(opacity(&a, 1050.0), None);
        assert_eq!(a.phase(1050.0), Phase::Before);
        assert!((opacity(&a, 1200.0).unwrap() - 0.5).abs() < 1e-4);
        assert_eq!(opacity(&a, 1300.0), None);
        assert!(a.finished(1300.0) && !a.finished(1299.0) && a.running(1299.0));
        let a = fade(Timing { duration: 200.0, delay: 100.0, fill: Fill::Both, ..Timing::default() });
        assert_eq!(opacity(&a, 1050.0), Some(0.0));
        assert_eq!(opacity(&a, 5000.0), Some(1.0));
    }

    #[test]
    fn iterations_and_direction() {
        let a = fade(Timing { duration: 100.0, iterations: f64::INFINITY, direction: Direction::Alternate, ..Timing::default() });
        assert!((opacity(&a, 1025.0).unwrap() - 0.25).abs() < 1e-4);
        assert!((opacity(&a, 1125.0).unwrap() - 0.75).abs() < 1e-4); // second iteration runs backwards
        assert!((opacity(&a, 1225.0).unwrap() - 0.25).abs() < 1e-4);
        assert!(!a.finished(1e9) && a.running(1e9));
        let a = fade(Timing { duration: 100.0, iterations: 2.0, direction: Direction::Reverse, fill: Fill::Forwards, ..Timing::default() });
        assert!((opacity(&a, 1010.0).unwrap() - 0.9).abs() < 1e-4);
        assert_eq!(opacity(&a, 1200.0), Some(0.0)); // ends where a reversed iteration ends
    }

    #[test]
    fn keyframes_with_implied_ends_and_their_own_easing() {
        // one keyframe in the middle: the ends come from the element's own value
        let frames = vec![Keyframe { offset: 0.5, easing: None, values: vec![(PropId::Width, Value::Len(Len::Px(100.0)))] }];
        let mut a = Animation::new(1, 7, Source::Css(Atom::NONE, 0), frames, Timing { duration: 100.0, ..Timing::default() });
        a.play(0.0);
        let mut out = Vec::new();
        a.sample(25.0, &|_| Some(Value::Len(Len::Px(0.0))), &mut out);
        assert_eq!(out, vec![(PropId::Width, Value::Len(Len::Px(50.0)))]);
        out.clear();
        a.sample(75.0, &|_| Some(Value::Len(Len::Px(0.0))), &mut out);
        assert_eq!(out, vec![(PropId::Width, Value::Len(Len::Px(50.0)))]);
        // a step between two keyframes holds the first value
        let frames = vec![
            Keyframe { offset: 0.0, easing: Easing::parse_str("step-end"), values: vec![(PropId::Opacity, Value::Num(0.2))] },
            Keyframe { offset: 1.0, easing: None, values: vec![(PropId::Opacity, Value::Num(1.0))] },
        ];
        let mut a = Animation::new(2, 7, Source::Script, frames, Timing { duration: 100.0, ..Timing::default() });
        a.play(0.0);
        assert_eq!(opacity(&a, 90.0), Some(0.2));
    }

    #[test]
    fn later_sources_win_per_property() {
        let mut tl = Timeline::default();
        let frame = |v: f32| vec![Keyframe { offset: 0.0, easing: None, values: vec![(PropId::Opacity, Value::Num(v))] }, Keyframe { offset: 1.0, easing: None, values: vec![(PropId::Opacity, Value::Num(v))] }];
        tl.add(3, Source::Script, frame(0.9), Timing { duration: 100.0, ..Timing::default() }).play(0.0);
        tl.add(3, Source::Transition(PropId::Opacity), frame(0.1), Timing { duration: 100.0, ..Timing::default() }).play(0.0);
        let got = tl.sample(50.0, &|_, _| None);
        assert_eq!(got, vec![(3, vec![(PropId::Opacity, Value::Num(0.9))])]);
        assert!(tl.any_running(50.0) && !tl.any_running(100.0));
    }
}
