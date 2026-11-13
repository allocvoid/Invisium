mod common;
use common::*;
use invisium_canvas::*;
use std::time::{Duration, Instant};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const SPECIAL: [f64; 12] =
    [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300, 1e9, -1e9, 0.0, -0.0, 1e-300, 65536.0, -1.0];
const STRS: [&str; 22] = [
    "", "round", "square", "bevel", "miter", "evenodd", "nonzero", "copy", "xor", "multiply", "lighter",
    "repeat", "no-repeat", "repeat-x", "top", "middle", "center", "rtl", "3px", "-1e9px", "M0 0 L10 10 A5 5 0 1 1 3 3 Z",
    "Hello, world \u{1F600}",
];
const DASH: [&[f64]; 5] = [&[], &[1.0, 2.0], &[0.0, 0.0], &[1e-9, 1e-9], &[5.0, f64::NAN]];
const PX: [u8; 36] = [7; 36];

fn arg(r: &mut Rng, handles: &[(Kind, u32)]) -> Arg<'static> {
    match r.below(20) {
        0 => Arg::Undefined,
        1 => Arg::Null,
        2 => Arg::Bool(r.below(2) == 0),
        3 => Arg::Num(SPECIAL[r.below(SPECIAL.len())]),
        4 => Arg::Str(STRS[r.below(STRS.len())]),
        5 => Arg::Nums(DASH[r.below(DASH.len())]),
        6 => Arg::Color([r.below(256) as u8, 0, 128, r.below(256) as u8]),
        7 => Arg::Font { font: 0, size: [0.0, 12.0, 5000.0, f32::NAN][r.below(4)], css: "x" },
        8 => match handles.get(r.below(handles.len() + 1)) {
            Some(&(k, h)) => Arg::Object(k, h),
            None => Arg::Object([Kind::Path, Kind::Gradient, Kind::Pattern][r.below(3)], r.below(5) as u32),
        },
        9 => Arg::Pixels { width: 3, height: 3, rgba: &PX },
        10 => Arg::Pixels { width: 1000, height: 1000, rgba: &PX },
        _ => Arg::Num(r.below(120) as f64 - 10.0 + (r.below(4) as f64) * 0.25),
    }
}

#[test]
fn c12_random_calls_never_panic() {
    let mut ops: Vec<u16> = methods().iter().map(|m| m.1).collect();
    for p in properties() {
        ops.push(p.1);
        ops.push(p.2);
    }
    for k in [Kind::Gradient, Kind::Pattern, Kind::Path] {
        ops.extend(object_methods(k).iter().map(|m| m.1));
    }
    ops.extend([0, 999, u16::MAX]);
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let mut t = T::new(64, 48);
    let mut handles: Vec<(Kind, u32)> = Vec::new();
    let start = Instant::now();
    for i in 0..100_000 {
        if i % 20_000 == 0 {
            t.c.resize([64, 1, 0, 33][r.below(4)], 48);
        }
        let op = ops[r.below(ops.len())];
        let nargs = r.below(10);
        let args: Vec<Arg> = (0..nargs).map(|_| arg(&mut r, &handles)).collect();
        if let Ret::Object(k, h) = t.c.call(&mut t.f, op, &args) {
            handles.push((k, h));
            if handles.len() > 64 {
                let (k, h) = handles.remove(0);
                t.c.release(k, h);
            }
        }
    }
    let mut out = Vec::new();
    t.c.read_rgba(&mut out);
    assert!(start.elapsed() < Duration::from_secs(120), "took {:?}", start.elapsed());
}

#[test]
fn c12_limits() {
    for (w, h) in [(100_000, 100_000), (8193, 1), (8192, 8193), (u32::MAX, u32::MAX)] {
        let mut t = T::new(w, h);
        assert_eq!(t.c.size(), (0, 0));
        t.call("fillRect", &[n(0.0), n(0.0), n(10.0), n(10.0)]);
        assert!(t.rgba().is_empty());
        t.c.resize(w, h);
        assert_eq!(t.c.size(), (0, 0));
    }

    let mut t = T::new(64, 64);
    for v in [f64::NAN, f64::INFINITY, -1e308, 1e308] {
        t.call("fillRect", &[n(v), n(0.0), n(10.0), n(10.0)]);
        t.call("moveTo", &[n(v), n(v)]);
        t.call("lineTo", &[n(1e308), n(-1e308)]);
        t.call("lineTo", &[n(10.0), n(v)]);
        t.call("arc", &[n(v), n(5.0), n(1e300), n(0.0), n(v)]);
    }
    t.set("lineWidth", n(1e300));
    t.call("stroke", &[]);
    t.call("fill", &[]);
    t.call("clip", &[]);
    t.call("scale", &[n(1e300), n(1e300)]);
    t.call("scale", &[n(1e300), n(1e300)]);
    t.call("fillRect", &[n(0.0), n(0.0), n(1.0), n(1.0)]);
    t.call("fillText", &[s("abc"), n(0.0), n(0.0)]);
    t.set("shadowBlur", n(1e300));
    t.set("shadowColor", col(0, 0, 0, 255));
    t.call("resetTransform", &[]);
    t.call("fillRect", &[n(0.0), n(0.0), n(10.0), n(10.0)]);

    // A million path segments: bounded time and memory.
    let mut t = T::new(64, 64);
    let start = Instant::now();
    let mut r = Rng(7);
    for i in 0..1_000_000 {
        let (x, y) = ((r.next() % 640) as f64 / 10.0, (r.next() % 640) as f64 / 10.0);
        t.call(if i == 0 { "moveTo" } else { "lineTo" }, &[n(x), n(y)]);
    }
    t.call("fill", &[]);
    t.call("stroke", &[]);
    t.call("isPointInPath", &[n(30.0), n(30.0)]);
    assert!(start.elapsed() < Duration::from_secs(120), "took {:?}", start.elapsed());
    assert!(t.c.memory_bytes() < 200 << 20, "{}", t.c.memory_bytes());

    // save without end.
    let mut t = T::new(16, 16);
    for _ in 0..200_000 {
        t.call("save", &[]);
    }
    assert!(t.c.memory_bytes() < 64 << 20, "{}", t.c.memory_bytes());
    for _ in 0..200_000 {
        t.call("restore", &[]);
    }
    t.call("fillRect", &[n(0.0), n(0.0), n(16.0), n(16.0)]);
    assert_eq!(t.alpha(5, 5), 255);
    // Many nested clips stay within the clip budget.
    let mut t = T::new(1024, 1024);
    for i in 0..2000 {
        t.call("save", &[]);
        t.call("rect", &[n(i as f64 * 0.1), n(0.0), n(1000.0), n(1000.0)]);
        t.call("clip", &[]);
    }
    assert!(t.c.memory_bytes() <= 512 << 20);
}
