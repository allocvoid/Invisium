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
    @location(9) xf: vec4<f32>,
    @location(10) cxf: vec4<f32>,
    @location(11) xt: vec4<f32>,
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
    @location(8) @interpolate(flat) xf: vec4<f32>,
    @location(9) @interpolate(flat) cxf: vec4<f32>,
    @location(10) @interpolate(flat) xt: vec4<f32>,
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
    o.xf = v.xf;
    o.cxf = v.cxf;
    o.xt = v.xt;
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

// Coverage with `k` physical pixels per unit of `p`'s space (1 when untransformed).
fn rrect_cov(p: vec2<f32>, b: vec4<f32>, r: vec4<f32>, k: f32) -> f32 {
    return clamp(0.5 - rbox_dist(p, b, r) * k, 0.0, 1.0);
}

// Map a physical point through the linear part `m` and translation `t`.
fn map_point(m: vec4<f32>, t: vec2<f32>, p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(m.x * p.x + m.z * p.y + t.x, m.y * p.x + m.w * p.y + t.y);
}

// Physical pixels per unit of the space that `m` (physical -> space) maps into.
fn px_per_unit(m: vec4<f32>) -> f32 {
    return inverseSqrt(max(abs(m.x * m.w - m.y * m.z), 1e-12));
}

fn erf_approx(x: f32) -> f32 {
    let s = sign(x);
    let a = abs(x);
    var t = 1.0 + (0.278393 + (0.230389 + 0.078108 * a * a) * a) * a;
    t = t * t;
    t = t * t;
    return s - s / t;
}

fn blur_cov(p: vec2<f32>, b: vec4<f32>, r: vec4<f32>, sigma: f32, k: f32) -> f32 {
    if (sigma < 0.01) {
        return rrect_cov(p, b, r, k);
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
    let pp = in.pos.xy;
    let kind = in.info.x;
    let flags = in.info.w;
    // `p`: the point in the item's own space; `k`: physical pixels per unit of that space.
    var p = pp;
    var k = 1.0;
    let xformed = (flags & 4u) != 0u;
    if (xformed) {
        p = map_point(in.xf, in.xt.xy, pp);
        k = px_per_unit(in.xf);
    }
    var col = vec4<f32>(0.0);
    if (kind == 0u) {
        col = premul(in.colors.x) * rrect_cov(p, in.shape, in.radii, k);
    } else if (kind == 1u) {
        let w = in.p0; // top, right, bottom, left
        let s = in.shape;
        let outer = rrect_cov(p, s, in.radii, k);
        let ib = vec4<f32>(s.x + w.w, s.y + w.x, s.z - w.w - w.y, s.w - w.x - w.z);
        var inner = 0.0;
        if (ib.z > 0.0 && ib.w > 0.0) {
            inner = rrect_cov(p, ib, in.p1, k);
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
        col = premul(in.colors.x) * blur_cov(p, in.shape, in.radii, sigma, k) * (1.0 - rrect_cov(p, in.p0, in.p1, k));
    } else if (kind == 3u) {
        let sigma = bitcast<f32>(in.info.y);
        var hole = 0.0;
        if (in.shape.z > 0.0 && in.shape.w > 0.0) {
            hole = blur_cov(p, in.shape, in.radii, sigma, k);
        }
        col = premul(in.colors.x) * rrect_cov(p, in.p0, in.p1, k) * (1.0 - hole);
    } else if (kind == 4u) {
        let t = dot(p - in.p0.xy, in.p0.zw);
        col = gradient(t, in.info.y, in.info.z, (flags & 1u) != 0u) * rrect_cov(p, in.shape, in.radii, k);
    } else if (kind == 5u) {
        let t = length((p - in.p0.xy) * in.p0.zw);
        col = gradient(t, in.info.y, in.info.z, (flags & 1u) != 0u) * rrect_cov(p, in.shape, in.radii, k);
    } else if (kind == 6u && xformed) {
        // Transformed glyph: bilinear sample inside its atlas cell (p1 = cell x, y, w, h).
        let dims = max(in.p1.zw, vec2<f32>(1.0));
        let t = (p - in.shape.xy) / max(in.shape.zw, vec2<f32>(1e-6)) * dims;
        let tc = clamp(t, vec2<f32>(0.5), dims - 0.5);
        let uv = (in.p1.xy + tc) / vec2<f32>(textureDimensions(atlas));
        let a = textureSampleLevel(atlas, samp_clamp, uv, 0.0).r;
        col = premul(in.colors.x) * a * rrect_cov(p, in.shape, vec4<f32>(0.0), k);
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
        col = s * in.p1.x * rrect_cov(p, in.shape, in.radii, k);
    }
    if ((flags & 2u) != 0u) {
        var pc = pp;
        var kc = 1.0;
        if ((flags & 8u) != 0u) {
            pc = map_point(in.cxf, in.xt.zw, pp);
            kc = px_per_unit(in.cxf);
        }
        col = col * rrect_cov(pc, in.clip, in.clip_radii, kc);
    }
    return col;
}
