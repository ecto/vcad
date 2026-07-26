// Lat-long (equirectangular) HDR environment, ported from `pathtrace::EnvMap`.
//
// Row 0 is the zenith (+Z), the last row is nadir; column 0 is phi = rotation.
// Nearest-texel lookups throughout, deliberately: the CDF is piecewise-constant
// per texel, so nearest sampling makes the radiance and the PDF describe
// exactly the same function — which is what MIS requires.
//
// The pixel and CDF data share ONE storage buffer (`env_data`), declared by the
// host shader rather than here, so this file can be composed into both the
// renderer and the parity harness at whatever binding each has spare. The
// layout, in f32s:
//
//   [0                    .. 3*w*h)                  pixels, RGB per texel
//   [3*w*h                .. 3*w*h + h*(w+1))        per-row conditional CDFs
//   [3*w*h + h*(w+1)      .. + (h+1))                marginal CDF over rows
//
// Every function takes the map's scalars explicitly instead of reading a
// uniform, so the harness can drive them without a RenderState.

const ENV_MODE_GRADIENT: u32 = 0u;
const ENV_MODE_IMAGE: u32 = 1u;

fn env_cond_base(w: u32, h: u32) -> u32 {
    return 3u * w * h;
}

fn env_marg_base(w: u32, h: u32) -> u32 {
    return 3u * w * h + h * (w + 1u);
}

// Relative luminance — the scalar the CDF is built over.
fn env_luminance(c: vec3<f32>) -> f32 {
    return 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
}

fn env_texel(i: u32, j: u32, w: u32) -> vec3<f32> {
    let o = 3u * (j * w + i);
    return vec3<f32>(env_data[o], env_data[o + 1u], env_data[o + 2u]);
}

// Image coordinates in [0,1)^2 for a world direction.
fn env_uv(d: vec3<f32>, rotation: f32) -> vec2<f32> {
    let eps = 1e-9;
    let theta = acos(clamp(d.z, -1.0, 1.0));
    let v = clamp(theta / PI, 0.0, 1.0 - eps);
    // rem_euclid over TAU.
    let tau = 2.0 * PI;
    var phi = atan2(d.y, d.x) - rotation;
    phi = phi - tau * floor(phi / tau);
    let u = clamp(phi / tau, 0.0, 1.0 - eps);
    return vec2<f32>(u, v);
}

// World direction for image coordinates in [0,1]^2.
fn env_direction(u: f32, v: f32, rotation: f32) -> vec3<f32> {
    let phi = u * 2.0 * PI + rotation;
    let theta = v * PI;
    let st = sin(theta);
    return vec3<f32>(st * cos(phi), st * sin(phi), cos(theta));
}

fn env_texel_index(uv: vec2<f32>, w: u32, h: u32) -> vec2<u32> {
    let i = min(u32(uv.x * f32(w)), w - 1u);
    let j = min(u32(uv.y * f32(h)), h - 1u);
    return vec2<u32>(i, j);
}

// Incoming radiance from direction `d`.
fn env_image_radiance(d: vec3<f32>, w: u32, h: u32, intensity: f32, rotation: f32) -> vec3<f32> {
    let uv = env_uv(d, rotation);
    let ij = env_texel_index(uv, w, h);
    return env_texel(ij.x, ij.y, w) * intensity;
}

// Solid-angle PDF of the environment sampling strategy for direction `d`.
//
// The uv-space density converts with dω = 2·π²·sin(θ)·du·dv, since u spans 2π
// of azimuth and v spans π of polar angle.
fn env_image_pdf(d: vec3<f32>, w: u32, h: u32, rotation: f32, marg_int: f32) -> f32 {
    if marg_int <= 0.0 {
        return 0.0;
    }
    let uv = env_uv(d, rotation);
    let ij = env_texel_index(uv, w, h);
    let sin_bin = sin(PI * (f32(ij.y) + 0.5) / f32(h));
    let f = max(env_luminance(env_texel(ij.x, ij.y, w)), 0.0) * sin_bin;
    if f <= 0.0 {
        return 0.0;
    }
    let pdf_uv = f / marg_int;
    // Actual sin(theta) of this direction, not the bin's — the Jacobian is a
    // property of the point, while the bin weight is importance.
    let sin_t = sqrt(max(1.0 - d.z * d.z, 0.0));
    if sin_t <= 1e-9 {
        return 0.0;
    }
    return pdf_uv / (2.0 * PI * PI * sin_t);
}

struct Sample1d {
    index: u32,
    frac: f32,
}

// Binary search a normalised CDF slice starting at `base` with `n` bins
// (so `n + 1` entries). Mirrors `pathtrace::sample_1d`.
fn env_sample_1d(base: u32, n: u32, u: f32) -> Sample1d {
    var lo = 0u;
    var hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2u;
        if env_data[base + mid + 1u] <= u {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    let i = min(lo, n - 1u);
    let a = env_data[base + i];
    let b = env_data[base + i + 1u];
    var d = 0.5;
    if b > a {
        d = (u - a) / (b - a);
    }
    var out: Sample1d;
    out.index = i;
    out.frac = clamp(d, 0.0, 1.0);
    return out;
}

struct EnvSample {
    dir: vec3<f32>,
    radiance: vec3<f32>,
    pdf: f32,
    ok: bool,
}

// Importance-sample a direction. PDF is measured in solid angle.
fn env_image_sample(
    r1: f32,
    r2: f32,
    w: u32,
    h: u32,
    intensity: f32,
    rotation: f32,
    marg_int: f32,
) -> EnvSample {
    var out: EnvSample;
    out.dir = vec3<f32>(0.0, 0.0, 1.0);
    out.radiance = vec3<f32>(0.0);
    out.pdf = 0.0;
    out.ok = false;
    if marg_int <= 0.0 {
        return out;
    }

    let mrow = env_sample_1d(env_marg_base(w, h), h, r2);
    let j = mrow.index;
    let ccol = env_sample_1d(env_cond_base(w, h) + j * (w + 1u), w, r1);
    let i = ccol.index;

    let u = (f32(i) + ccol.frac) / f32(w);
    let v = (f32(j) + mrow.frac) / f32(h);
    let d = env_direction(u, v, rotation);

    // Recompute through the pdf so the MIS partner and this sample agree
    // bit-for-bit on the density.
    let pdf = env_image_pdf(d, w, h, rotation, marg_int);
    if pdf <= 0.0 {
        return out;
    }

    out.dir = d;
    out.radiance = env_texel(i, j, w) * intensity;
    out.pdf = pdf;
    out.ok = true;
    return out;
}
