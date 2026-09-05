//! Device-side per-pixel history and à-trous denoise.
//!
//! A host driving `render_resident_linear` gets one raw sample per pass and
//! has to do three things with it on the CPU: fold it into a running mean,
//! filter that mean, and tonemap the result. At 512x288 the filter alone costs
//! an order of magnitude more than the trace it is cleaning up. All three fit
//! in compute passes over buffers that already live on the device.
//!
//! Four entry points, run in this order once per pass:
//!
//! * `accumulate` — fold `raw` into `mean`/`stats`, honouring the caller's
//!   per-pixel keep mask. A zero mask entry restarts that pixel's history.
//! * `demodulate` — divide the mean by the albedo guide and prefilter the
//!   variance, writing `scratch_src`.
//! * `atrous` — one 5x5 B3-spline wavelet iteration at `params.stride`,
//!   reading `scratch_src` and writing `scratch_dst`. Dispatched once per
//!   iteration with the two scratch buffers swapped between them.
//! * `resolve` — re-modulate, blend by history length, tonemap, and store
//!   into the caller's texture.
//!
//! This is a port of `pathtrace::denoise`, filter weight for filter weight, so
//! a one-sample history denoises to what the CPU would have produced from the
//! same `Film`. `tests/gpu_history.rs` pins that.

const PI: f32 = 3.14159265359;

// Floor on the demodulation divisor. Mirrors `pathtrace::DEMOD_FLOOR`.
const DEMOD_FLOOR: f32 = 0.05;

// 5x5 separable B3-spline kernel, [1 4 6 4 1] / 16.
const B3_0: f32 = 0.0625;
const B3_1: f32 = 0.25;
const B3_2: f32 = 0.375;

struct HistoryParams {
    width: u32,
    height: u32,
    // History length at which the filter has fully faded out. A pixel with
    // this many samples is left exactly as the mean found it.
    count_cutoff: u32,
    // Number of à-trous iterations the host is about to dispatch. Zero means
    // "no filtering at all", and `resolve` then tonemaps the bare mean.
    iters: u32,
    sigma_lum: f32,
    sigma_depth: f32,
    sigma_normal: f32,
    exposure: f32,
    // Tap spacing for this à-trous iteration: 1, 2, 4, ...
    stride: u32,
    // `resolve` only: non-zero when the final iteration left its result in
    // `scratch_dst` rather than `scratch_src`.
    src_is_b: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> params: HistoryParams;
// This pass's own raw linear sample: the ray tracer's accumulation buffer
// after a `raw_sample` pass, which is (radiance, coverage).
@group(0) @binding(1) var<storage, read> raw: array<vec4<f32>>;
// The resident depth/normal buffer's three planes. Plane 1 is
// (face-forwarded normal, distance from the eye) and plane 2 is
// (denoise albedo, 0), both in the CPU `Film`'s conventions — background
// depth is 0, not MAX_T.
@group(0) @binding(2) var<storage, read> guides: array<vec4<f32>>;
// Running mean: (linear radiance, coverage).
@group(0) @binding(3) var<storage, read_write> mean: array<vec4<f32>>;
// (count, luminance sum, luminance-squared sum, variance of the mean).
@group(0) @binding(4) var<storage, read_write> stats: array<vec4<f32>>;
// One entry per pixel: 0 restarts that pixel's history, non-zero keeps it.
@group(0) @binding(5) var<storage, read> keep: array<u32>;
// (illumination, variance) ping-pong for the wavelet iterations.
@group(0) @binding(6) var<storage, read_write> scratch_src: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read_write> scratch_dst: array<vec4<f32>>;
@group(0) @binding(8) var out_tex: texture_storage_2d<rgba8unorm, write>;

fn luminance(c: vec3<f32>) -> f32 {
    return 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
}

fn n_pixels() -> u32 {
    return params.width * params.height;
}

fn guide_depth(i: u32) -> f32 {
    return guides[n_pixels() + i].w;
}

fn guide_normal(i: u32) -> vec3<f32> {
    return guides[n_pixels() + i].xyz;
}

fn guide_albedo(i: u32) -> vec3<f32> {
    return guides[2u * n_pixels() + i].xyz;
}

// The demodulation divisor, per channel and floored, as `pathtrace::denoise`
// applies it on the way in and the way out.
fn demod_albedo(i: u32) -> vec3<f32> {
    return max(guide_albedo(i), vec3<f32>(DEMOD_FLOOR));
}

// The scalar the demodulated variance is divided by: the luminance of the
// floored albedo, itself floored.
fn demod_lum(i: u32) -> f32 {
    return max(luminance(demod_albedo(i)), DEMOD_FLOOR);
}

fn in_bounds(gid: vec3<u32>) -> bool {
    return gid.x < params.width && gid.y < params.height;
}

fn flat_index(gid: vec3<u32>) -> u32 {
    return gid.y * params.width + gid.x;
}

// ─── pass 1: fold this sample into the history ────────────────────────────

@compute @workgroup_size(8, 8)
fn accumulate(@builtin(global_invocation_id) gid: vec3<u32>) {
    if !in_bounds(gid) {
        return;
    }
    let i = flat_index(gid);
    let c = raw[i];
    let l = luminance(c.rgb);

    var m = mean[i];
    var st = stats[i];
    if keep[i] == 0u {
        m = vec4<f32>(0.0);
        st = vec4<f32>(0.0);
    }

    let n = st.x + 1.0;
    // Welford-style running mean, so the history never holds a sum that can
    // lose the low bits of a long accumulation.
    m = m + (c - m) / n;
    let lsum = st.y + l;
    let lsum2 = st.z + l * l;

    // Variance of the *mean*, matching `pathtrace::trace_pixel`: sample
    // variance over n, with a single sample falling back to its own magnitude
    // because it says nothing about its own spread.
    var v: f32;
    if n > 1.5 {
        let inv = 1.0 / n;
        let mu = lsum * inv;
        let sample_var = max(lsum2 * inv - mu * mu, 0.0) * n / (n - 1.0);
        v = sample_var * inv;
    } else {
        v = lsum * lsum;
    }

    mean[i] = m;
    stats[i] = vec4<f32>(n, lsum, lsum2, v);
}

// ─── pass 2: demodulate and prefilter the variance ────────────────────────

@compute @workgroup_size(8, 8)
fn demodulate(@builtin(global_invocation_id) gid: vec3<u32>) {
    if !in_bounds(gid) {
        return;
    }
    let i = flat_index(gid);
    let illum = mean[i].rgb / demod_albedo(i);

    // A 3x3 box over the demodulated variance. The per-pixel estimate is
    // itself noisy at low sample counts, and a noisy error bar makes the
    // luminance weight jitter between "trust" and "reject" pixel to pixel.
    // Background taps are excluded, exactly as the CPU filter excludes them.
    var s = 0.0;
    var k = 0.0;
    let x = i32(gid.x);
    let y = i32(gid.y);
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        let qy = y + dy;
        if qy < 0 || qy >= i32(params.height) {
            continue;
        }
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let qx = x + dx;
            if qx < 0 || qx >= i32(params.width) {
                continue;
            }
            let q = u32(qy) * params.width + u32(qx);
            if guide_depth(q) <= 0.0 {
                continue;
            }
            let lq = demod_lum(q);
            s = s + stats[q].w / (lq * lq);
            k = k + 1.0;
        }
    }

    let own = stats[i].w / (demod_lum(i) * demod_lum(i));
    var v = own;
    if k > 0.0 {
        v = s / k;
    }
    scratch_src[i] = vec4<f32>(illum, v);
}

// ─── pass 3: one à-trous wavelet iteration ────────────────────────────────

fn b3(k: i32) -> f32 {
    if k == 0 || k == 4 {
        return B3_0;
    }
    if k == 1 || k == 3 {
        return B3_1;
    }
    return B3_2;
}

@compute @workgroup_size(8, 8)
fn atrous(@builtin(global_invocation_id) gid: vec3<u32>) {
    if !in_bounds(gid) {
        return;
    }
    let p = flat_index(gid);
    let z_p = guide_depth(p);
    let centre = scratch_src[p];

    // Background is analytic and noise-free: pass it through, and never let it
    // bleed onto a surface below.
    if z_p <= 0.0 {
        scratch_dst[p] = centre;
        return;
    }
    // A pixel with a long enough history is already clean; leave it alone.
    // This is what makes the filter cost fall away as the frame converges.
    if stats[p].x >= f32(params.count_cutoff) {
        scratch_dst[p] = centre;
        return;
    }

    let stride = i32(max(params.stride, 1u));
    let sigma_n2 = max(params.sigma_normal, 1e-4) * max(params.sigma_normal, 1e-4);
    let sigma_l = max(params.sigma_lum, 1e-6);
    let sigma_z = max(params.sigma_depth, 1e-6) * f32(stride);

    let n_p = guide_normal(p);
    let c_p = centre.xyz;
    let l_p = luminance(c_p);
    // The estimator's own error bar sets how much luminance disagreement
    // counts as signal rather than noise, so a firefly — which has an enormous
    // error bar — stops protecting itself and gets filtered.
    let l_tol = sigma_l * sqrt(max(centre.w, 0.0)) + 1e-4;

    var sum = vec3<f32>(0.0);
    var vsum = 0.0;
    var wsum = 0.0;

    let x = i32(gid.x);
    let y = i32(gid.y);
    for (var ky = 0; ky < 5; ky = ky + 1) {
        let qy = y + (ky - 2) * stride;
        if qy < 0 || qy >= i32(params.height) {
            continue;
        }
        for (var kx = 0; kx < 5; kx = kx + 1) {
            let qx = x + (kx - 2) * stride;
            if qx < 0 || qx >= i32(params.width) {
                continue;
            }
            let q = u32(qy) * params.width + u32(qx);
            let z_q = guide_depth(q);
            if z_q <= 0.0 {
                continue;
            }

            let dn = n_p - guide_normal(q);
            let w_n = exp(-dot(dn, dn) / sigma_n2);
            let w_z = exp(-abs(z_p - z_q) / (sigma_z * z_p));
            let tap = scratch_src[q];
            let w_l = exp(-abs(l_p - luminance(tap.xyz)) / l_tol);

            let weight = b3(kx) * b3(ky) * w_n * w_z * w_l;
            if weight <= 0.0 {
                continue;
            }
            sum = sum + tap.xyz * weight;
            // The variance of a weighted mean of independent estimates carries
            // the squared weights.
            vsum = vsum + weight * weight * tap.w;
            wsum = wsum + weight;
        }
    }

    if wsum > 0.0 {
        scratch_dst[p] = vec4<f32>(sum / wsum, vsum / (wsum * wsum));
    } else {
        scratch_dst[p] = centre;
    }
}

// ─── pass 4: re-modulate, tonemap, present ────────────────────────────────

// ACES filmic tonemap (Narkowicz fit), matching `pathtrace::tonemap_aces`.
fn tonemap_aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Linear to sRGB transfer, matching `pathtrace::linear_to_srgb` — the exact
// curve, not the 1/2.2 approximation the main pass uses, so a frame that came
// out of here is the frame `Film::to_srgb8` would have written.
fn linear_to_srgb1(x: f32) -> f32 {
    if x <= 0.0031308 {
        return 12.92 * x;
    }
    return 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(linear_to_srgb1(c.x), linear_to_srgb1(c.y), linear_to_srgb1(c.z));
}

@compute @workgroup_size(8, 8)
fn resolve(@builtin(global_invocation_id) gid: vec3<u32>) {
    if !in_bounds(gid) {
        return;
    }
    let i = flat_index(gid);
    let m = mean[i];
    var rgb = m.rgb;

    if params.iters > 0u && guide_depth(i) > 0.0 {
        var filt: vec4<f32>;
        if params.src_is_b != 0u {
            filt = scratch_dst[i];
        } else {
            filt = scratch_src[i];
        }
        let remod = filt.xyz * demod_albedo(i);
        // The filter fades out as the history grows: full strength on the
        // first sample, nothing at all once the pixel has `count_cutoff` of
        // them and the temporal mean is doing the work.
        let cnt = stats[i].x;
        let span = max(f32(params.count_cutoff) - 1.0, 1e-6);
        let strength = clamp((f32(params.count_cutoff) - cnt) / span, 0.0, 1.0);
        rgb = mix(rgb, remod, strength);
    }

    let mapped = linear_to_srgb(tonemap_aces(rgb * params.exposure));
    textureStore(out_tex, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<f32>(mapped, m.w));
}
