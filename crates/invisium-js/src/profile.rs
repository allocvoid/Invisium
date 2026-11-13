//! Where script time goes: an opt-in sampler. Every 1024th safepoint (calls and backward
//! jumps) the time since the previous sample is added to the function that is running then.
//! Natives have no safepoints, so their time lands on the script function that called them.
//! Off by default; it then costs one branch per safepoint.

use crate::bytecode::FuncCode;
use crate::Runtime;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub(crate) struct Profile {
    last: Instant,
    /// Keyed by the address of the function's code.
    funcs: HashMap<usize, Entry>,
    gc: Duration,
    gc_runs: u32,
}

struct Entry {
    name: String,
    time: Duration,
    samples: u32,
}

/// One row of a profile.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileRow {
    /// `name (script:line)`, or a row in brackets for the engine's own work.
    pub what: String,
    pub ms: f64,
    pub samples: u32,
}

fn describe(code: &FuncCode) -> String {
    let name = String::from_utf16_lossy(&code.name);
    let name = if name.is_empty() { "(anonymous)".to_string() } else { name };
    let script: &str = &code.script;
    let tail = script.rsplit('/').next().unwrap_or(script);
    let line = code.lines.first().map_or(0, |l| l.1.line);
    let col = code.lines.first().map_or(0, |l| l.1.col);
    format!("{name} ({tail}:{line}:{col})")
}

impl Runtime {
    /// Start or stop measuring where script time goes (see `take_profile`).
    pub fn set_profile(&mut self, on: bool) {
        self.prof = on.then(|| Box::new(Profile { last: Instant::now(), funcs: HashMap::new(), gc: Duration::ZERO, gc_runs: 0 }));
    }

    /// What was measured since profiling started or since the last call, most expensive
    /// first. Empty when profiling is off.
    pub fn take_profile(&mut self) -> Vec<ProfileRow> {
        let Some(p) = self.prof.as_mut() else { return Vec::new() };
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let mut rows: Vec<ProfileRow> =
            p.funcs.drain().map(|(_, e)| ProfileRow { what: e.name, ms: ms(e.time), samples: e.samples }).collect();
        if p.gc_runs > 0 {
            rows.push(ProfileRow { what: "[garbage collection]".into(), ms: ms(p.gc), samples: p.gc_runs });
        }
        p.gc = Duration::ZERO;
        p.gc_runs = 0;
        p.last = Instant::now();
        rows.sort_by(|a, b| b.ms.partial_cmp(&a.ms).unwrap_or(std::cmp::Ordering::Equal));
        rows
    }

    /// Script is entered from outside: time spent elsewhere since the last sample is nobody's.
    pub(crate) fn profile_enter(&mut self) {
        if let Some(p) = self.prof.as_mut() {
            p.last = Instant::now();
        }
    }

    #[cold]
    pub(crate) fn profile_sample(&mut self) {
        let Some(p) = self.prof.as_mut() else { return };
        let now = Instant::now();
        let took = now - p.last;
        p.last = now;
        let Some(f) = self.frames.last() else { return };
        let code: &FuncCode = &f.code;
        let e = p.funcs.entry(code as *const FuncCode as usize).or_insert_with(|| Entry { name: describe(code), time: Duration::ZERO, samples: 0 });
        e.time += took;
        e.samples += 1;
    }

    /// A collection ran from `start` until now: its time is the collector's, not the function's
    /// that happened to trigger it.
    pub(crate) fn profile_gc(&mut self, start: Instant) {
        if let Some(p) = self.prof.as_mut() {
            let now = Instant::now();
            p.gc += now - start;
            p.gc_runs += 1;
            p.last += now - start;
        }
    }
}
