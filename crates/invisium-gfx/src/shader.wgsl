// Invisium uber-shader: every item is an instanced quad shaded analytically.

struct Globals {
    size: vec2<f32>,
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var<storage, read> stops: array<vec2<u32>>;
@group(0) @binding(3) var samp_clamp: sampler;
@group(0) @binding(4) var samp_repeat: sampler;
@group(1) @binding(0) var img: texture_2d<f32>;

struct VIn {
    @builtin(vertex_index) vi: u32,
    @location(0) quad: vec4<f32>,
    @location(1) shape: vec4<f32>,
    @location(2) radii: vec4<f32>,
    @location(3) clip: vec4<f32>,
    @location(4) clip_radii: vec4<f32>,
    @location(5) colors: vec4<u32>,
    @location(6) p0: vec4<f32>,
    @location(7) p1: vec4<f32>,
    @location(8) info: vec4<u32>,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) shape: vec4<f32>,
    @location(1) @interpolate(flat) radii: vec4<f32>,
    @location(2) @interpolate(flat) clip: vec4<f32>,
    @location(3) @interpolate(flat) clip_radii: vec4<f32>,
    @location(4) @interpolate(flat) colors: vec4<u32>,
    @location(5) @interpolate(flat) p0: vec4<f32>,
    @location(6) @interpolate(flat) p1: vec4<f32>,
    @location(7) @interpolate(flat) info: vec4<u32>,
};

@vertex
fn vs(v: VIn) -> VOut {
    let c = vec2<f32>(f32(v.vi & 1u), f32(v.vi >> 1u));
    let p = v.quad.xy + c * v.quad.zw;
    var o: VOut;
    o.pos = vec4<f32>(p.x / globals.size.x * 2.0 - 1.0, 1.0 - p.y / globals.size.y * 2.0, 0.0, 1.0);
    o.shape = v.shape;
    o.radii = v.radii;
    o.clip = v.clip;
    o.clip_radii = v.clip_radii;
    o.colors = v.colors;
    o.p0 = v.p0;
    o.p1 = v.p1;
    o.info = v.info;
    return o;
}

// Signed distance to a box of half-size `half` centred at the origin; radii tl, tr, br, bl.
fn sd_rbox(p: vec2<f32>, half: vec2<f32>, r: vec4<f32>) -> f32 {
    var rr = select(r.w, r.z, p.x > 0.0);
    if (p.y < 0.0) {
        rr = select(r.x, r.y, p.x > 0.0);
    }
    let q = abs(p) - half + vec2<f32>(rr);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - rr;
}

fn rbox_dist(p: vec2<f32>, b: vec4<f32>, r: vec4<f32>) -> f32 {
    let half = b.zw * 0.5;
    return sd_rbox(p - (b.xy + half), half, r);
}

fn rrect_cov(p: vec2<f32>, b: vec4<f32>, r: vec4<f32>) -> f32 {
    return clamp(0.5 - rbox_dist(p, b, r), 0.0, 1.0);
}

fn erf_approx(x: f32) -> f32 {
    let s = sign(x);
    let a = abs(x);
    var t = 1.0 + (0.278393 + (0.230389 + 0.078108 * a * a) * a) * a;
    t = t * t;
    t = t * t;
    return s - s / t;
}

fn blur_cov(p: vec2<f32>, b: vec4<f32>, r: vec4<f32>, sigma: f32) -> f32 {
    if (sigma < 0.01) {
        return rrect_cov(p, b, r);
    }
    let d = rbox_dist(p, b, r);
    return clamp(0.5 - 0.5 * erf_approx(d / (sigma * 1.4142135)), 0.0, 1.0);
}

fn premul(c: u32) -> vec4<f32> {
    let v = unpack4x8unorm(c);
    return vec4<f32>(v.rgb * v.a, v.a);
}

fn gradient(t_in: f32, start: u32, count: u32, repeating: bool) -> vec4<f32> {
    if (count == 0u) {
        return vec4<f32>(0.0);
    }
    let first = bitcast<f32>(stops[start].x);
    let last = bitcast<f32>(stops[start + count - 1u].x);
    var t = t_in;
    if (repeating && last - first > 1e-5) {
        let period = last - first;
        t = first + fract((t - first) / period) * period;
    }
    if (t <= first) {
        return premul(stops[start].y);
    }
    if (t >= last) {
        return premul(stops[start + count - 1u].y);
    }
    var prev_o = first;
    var prev_c = premul(stops[start].y);
    for (var i = 1u; i < count; i = i + 1u) {
        let s = stops[start + i];
        let o = bitcast<f32>(s.x);
        let c = premul(s.y);
        if (t <= o) {
            let span = o - prev_o;
            if (span <= 1e-6) {
                return c;
            }
            return mix(prev_c, c, (t - prev_o) / span);
        }
        prev_o = o;
        prev_c = c;
    }
    return prev_c;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    let p = in.pos.xy;
    let kind = in.info.x;
    let flags = in.info.w;
    var col = vec4<f32>(0.0);
    if (kind == 0u) {
        col = premul(in.colors.x) * rrect_cov(p, in.shape, in.radii);
    } else if (kind == 1u) {
        let w = in.p0; // top, right, bottom, left
        let s = in.shape;
        let outer = rrect_cov(p, s, in.radii);
        let ib = vec4<f32>(s.x + w.w, s.y + w.x, s.z - w.w - w.y, s.w - w.x - w.z);
        var inner = 0.0;
        if (ib.z > 0.0 && ib.w > 0.0) {
            inner = rrect_cov(p, ib, in.p1);
        }
        let cov = max(outer - inner, 0.0);
        // Pick the side whose normalised distance is smallest: sides meet on the corner diagonals.
        let dt = select(1e9, (p.y - s.y) / w.x, w.x > 0.0);
        let dr = select(1e9, (s.x + s.z - p.x) / w.y, w.y > 0.0);
        let db = select(1e9, (s.y + s.w - p.y) / w.z, w.z > 0.0);
        let dl = select(1e9, (p.x - s.x) / w.w, w.w > 0.0);
        var c = in.colors.x;
        var best = dt;
        if (dr < best) { best = dr; c = in.colors.y; }
        if (db < best) { best = db; c = in.colors.z; }
        if (dl < best) { best = dl; c = in.colors.w; }
        col = premul(c) * cov;
    } else if (kind == 2u) {
        let sigma = bitcast<f32>(in.info.y);
        col = premul(in.colors.x) * blur_cov(p, in.shape, in.radii, sigma) * (1.0 - rrect_cov(p, in.p0, in.p1));
    } else if (kind == 3u) {
        let sigma = bitcast<f32>(in.info.y);
        var hole = 0.0;
        if (in.shape.z > 0.0 && in.shape.w > 0.0) {
            hole = blur_cov(p, in.shape, in.radii, sigma);
        }
        col = premul(in.colors.x) * rrect_cov(p, in.p0, in.p1) * (1.0 - hole);
    } else if (kind == 4u) {
        let t = dot(p - in.p0.xy, in.p0.zw);
        col = gradient(t, in.info.y, in.info.z, (flags & 1u) != 0u) * rrect_cov(p, in.shape, in.radii);
    } else if (kind == 5u) {
        let t = length((p - in.p0.xy) * in.p0.zw);
        col = gradient(t, in.info.y, in.info.z, (flags & 1u) != 0u) * rrect_cov(p, in.shape, in.radii);
    } else if (kind == 6u) {
        let rel = clamp(floor(p - in.shape.xy), vec2<f32>(0.0), max(in.shape.zw - 1.0, vec2<f32>(0.0)));
        let a = textureLoad(atlas, vec2<i32>(in.p1.xy + rel), 0).r;
        col = premul(in.colors.x) * a;
    } else if (kind == 7u) {
        let local = clamp((p - in.shape.xy) / in.shape.zw, vec2<f32>(0.0), vec2<f32>(1.0));
        let uv = mix(in.p0.xy, in.p0.zw, local);
        var s: vec4<f32>;
        if ((flags & 1u) != 0u) {
            s = textureSampleLevel(img, samp_repeat, uv, 0.0);
        } else {
            s = textureSampleLevel(img, samp_clamp, uv, 0.0);
        }
        col = s * in.p1.x * rrect_cov(p, in.shape, in.radii);
    }
    if ((flags & 2u) != 0u) {
        col = col * rrect_cov(p, in.clip, in.clip_radii);
    }
    return col;
}
