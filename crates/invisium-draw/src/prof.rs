//! A stopwatch for finding where time goes. Named spans add up per thread and are handed
//! out on request. It is off unless the environment variable `INVIS_PROFILE` is set; a span
//! then costs one thread-local read.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// What one name has cost so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Total {
    pub count: u32,
    pub time: Duration,
    /// The longest single span.
    pub longest: Duration,
}

thread_local! {
    static ON: bool = std::env::var_os("INVIS_PROFILE").is_some();
    static TOTALS: RefCell<BTreeMap<&'static str, Total>> = const { RefCell::new(BTreeMap::new()) };
}

/// True if spans are being measured on this thread.
pub fn enabled() -> bool {
    ON.with(|on| *on)
}

/// Time from now until the returned value is dropped counts for `name`. Spans may nest; each
/// name gets the whole time of its own spans, so a name can include what other names cost.
pub fn span(name: &'static str) -> Span {
    Span { name, start: if enabled() { Some(Instant::now()) } else { None } }
}

pub struct Span {
    name: &'static str,
    start: Option<Instant>,
}

impl Drop for Span {
    fn drop(&mut self) {
        let Some(start) = self.start else { return };
        let took = start.elapsed();
        TOTALS.with(|t| {
            let mut t = t.borrow_mut();
            let e = t.entry(self.name).or_default();
            e.count += 1;
            e.time += took;
            e.longest = e.longest.max(took);
        });
    }
}

/// The totals since the last call, most expensive first.
pub fn take() -> Vec<(&'static str, Total)> {
    let mut all: Vec<(&'static str, Total)> = TOTALS.with(|t| std::mem::take(&mut *t.borrow_mut())).into_iter().collect();
    all.sort_by(|a, b| b.1.time.cmp(&a.1.time));
    all
}

/// The totals since the last call as text, one line per name (empty if nothing was measured).
pub fn report() -> String {
    let mut out = String::new();
    for (name, t) in take() {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        out.push_str(&format!("  {name:<28} {:>9.1} ms in {:>6} spans, longest {:>7.1} ms\n", ms(t.time), t.count, ms(t.longest)));
    }
    out
}
