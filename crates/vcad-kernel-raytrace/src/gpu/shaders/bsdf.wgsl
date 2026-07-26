//! Shared BSDF: the single shading model used by BOTH the GPU viewport path
//! tracer and (as a port of `pathtrace.rs`) the CPU photoreal renderer.
//!
//! This file is prepended to every shader that needs to shade, so there is
//! exactly one copy of the BSDF in the codebase. `tests/bsdf_parity.rs`
//! compiles it standalone against a harness entry point and checks it against
//! the Rust reference in `pathtrace.rs`.

const PI: f32 = 3.14159265359;


// Layout must match GpuMaterial in buffers.rs, which in turn mirrors
// pathtrace::Pbr field-for-field so both renderers shade identically.
struct GpuMaterial {
    color: vec4<f32>,
    metallic: f32,
    roughness: f32,
    clearcoat: f32,
    clearcoat_roughness: f32,
    ior: f32,
    // Signed: positive smears the highlight along the local +X (tangent) axis,
    // negative along +Y. See `mat_alpha_tb`.
    anisotropy: f32,
    _pad0: f32,
    _pad1: f32,
}

// ─── unified BSDF ─────────────────────────────────────────────────────────
//
// A direct port of `pathtrace.rs`'s layered metallic-roughness BSDF: Lambert
// diffuse + GGX specular with VNDF sampling + a GGX clearcoat layer. This is
// the SINGLE shading model shared by the GPU viewport and the CPU photoreal
// renderer, so the same document shades the same way in both.
//
// The invariant that matters: the PDF returned by `bsdf_sample` must exactly
// equal the PDF `bsdf_eval` reports for the same pair of directions. If they
// disagree, MIS silently produces an energy-wrong image that still looks
// plausible. `tests/bsdf_parity.rs` pins both that identity and agreement with
// the Rust reference, evaluated on the GPU itself.
//
// All vectors are in the local shading frame (+Z = normal) and point away from
// the surface. The CPU reference computes in f64 and this in f32, so agreement
// holds to f32 tolerance, not bit-exactly.

fn max3(v: vec3<f32>) -> f32 {
    return max(max(v.x, v.y), v.z);
}

// GGX alpha for the base specular lobe, ignoring anisotropy.
fn mat_alpha(m: GpuMaterial) -> f32 {
    return max(m.roughness * m.roughness, 1e-4);
}

// GGX alphas along the tangent and bitangent for the base specular lobe.
//
// Standard Disney/glTF construction: an aspect ratio of
// sqrt(1 - 0.9·|anisotropy|) splits the isotropic alpha into a stretched and
// a squeezed axis while keeping their product roughly fixed. At anisotropy 0
// the aspect is exactly 1, so both alphas equal mat_alpha and every
// anisotropic path below reduces bit-identically to the isotropic one.
fn mat_alpha_tb(m: GpuMaterial) -> vec2<f32> {
    let a = mat_alpha(m);
    let aniso = clamp(m.anisotropy, -1.0, 1.0);
    if aniso == 0.0 {
        return vec2<f32>(a, a);
    }
    let aspect = sqrt(1.0 - 0.9 * abs(aniso));
    let wide = min(a / aspect, 1.0);
    let narrow = a * aspect;
    if aniso > 0.0 {
        return vec2<f32>(wide, narrow);
    }
    return vec2<f32>(narrow, wide);
}

// GGX alpha for the clearcoat lobe.
fn mat_coat_alpha(m: GpuMaterial) -> f32 {
    return max(m.clearcoat_roughness * m.clearcoat_roughness, 1e-4);
}

// Specular reflectance at normal incidence: from IOR, blended toward the base
// color by metallic.
fn mat_f0(m: GpuMaterial) -> vec3<f32> {
    let r = (m.ior - 1.0) / (m.ior + 1.0);
    let d = r * r;
    return mix(vec3<f32>(d, d, d), m.color.rgb, m.metallic);
}

// Diffuse albedo (metals have none).
fn mat_diffuse_albedo(m: GpuMaterial) -> vec3<f32> {
    return m.color.rgb * (1.0 - m.metallic);
}

// GGX / Trowbridge-Reitz normal distribution. Takes alpha directly (already
// squared roughness) — NOT perceptual roughness.
fn d_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / max(PI * d * d, 1e-9);
}

// Anisotropic GGX normal distribution. Reduces exactly to `d_ggx` when at==ab.
fn d_ggx_aniso(wh: vec3<f32>, at: f32, ab: f32) -> f32 {
    if at == ab {
        return d_ggx(max(wh.z, 0.0), at);
    }
    let d = (wh.x / at) * (wh.x / at) + (wh.y / ab) * (wh.y / ab) + wh.z * wh.z;
    return 1.0 / max(PI * at * ab * d * d, 1e-9);
}

// Smith height-correlated visibility term, already divided by 4·NoL·NoV.
fn v_smith(n_dot_v: f32, n_dot_l: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let gv = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
    let gl = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-9);
}

// Λ-style stretched length used by the anisotropic Smith terms.
fn aniso_stretched(w: vec3<f32>, at: f32, ab: f32) -> f32 {
    return sqrt((at * w.x) * (at * w.x) + (ab * w.y) * (ab * w.y) + w.z * w.z);
}

// Anisotropic Smith height-correlated visibility term (already divided by
// 4·NoL·NoV). Reduces exactly to `v_smith` when at==ab.
fn v_smith_aniso(wo: vec3<f32>, wi: vec3<f32>, at: f32, ab: f32) -> f32 {
    if at == ab {
        return v_smith(wo.z, wi.z, at);
    }
    let gv = wi.z * aniso_stretched(wo, at, ab);
    let gl = wo.z * aniso_stretched(wi, at, ab);
    return 0.5 / max(gv + gl, 1e-9);
}

// Smith G1 masking term for the anisotropic GGX distribution. The at==ab
// branch is the isotropic expression evaluated exactly as it was before
// anisotropy existed, so isotropic renders are bit-unchanged.
fn g1_smith_aniso(w: vec3<f32>, at: f32, ab: f32) -> f32 {
    let z = max(w.z, 1e-6);
    var lambda: f32;
    if at == ab {
        let a2 = at * at;
        lambda = (sqrt(1.0 + a2 * (1.0 - z * z) / (z * z)) - 1.0) * 0.5;
    } else {
        lambda = (aniso_stretched(vec3<f32>(w.x, w.y, z), at, ab) / z - 1.0) * 0.5;
    }
    return 1.0 / (1.0 + lambda);
}

// Schlick Fresnel.
fn fresnel(f0: vec3<f32>, cos_theta: f32) -> vec3<f32> {
    let m = pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
    return f0 + (vec3<f32>(1.0) - f0) * m;
}

// Cosine-weighted hemisphere sample in the local frame (+Z = normal).
fn cosine_hemisphere_local(r1: f32, r2: f32) -> vec3<f32> {
    let r = sqrt(r1);
    let phi = 2.0 * PI * r2;
    return vec3<f32>(r * cos(phi), r * sin(phi), sqrt(max(1.0 - r1, 0.0)));
}

// Sample the GGX visible-normal distribution (Heitz 2018). Returns the
// sampled half-vector in the local frame.
// Anisotropic by construction: the stretch step that maps to the
// hemisphere-configured space takes each axis' alpha separately, so passing
// at != ab needs no other change.
fn sample_vndf(wo: vec3<f32>, at: f32, ab: f32, r1: f32, r2: f32) -> vec3<f32> {
    let a = at;
    let b = ab;
    // Stretch the view direction into the hemisphere-configured space.
    let vh = normalize(vec3<f32>(a * wo.x, b * wo.y, wo.z));
    let lensq = vh.x * vh.x + vh.y * vh.y;
    var t1: vec3<f32>;
    if lensq > 0.0 {
        t1 = vec3<f32>(-vh.y, vh.x, 0.0) / sqrt(lensq);
    } else {
        t1 = vec3<f32>(1.0, 0.0, 0.0);
    }
    let t2 = cross(vh, t1);

    let r = sqrt(r1);
    let phi = 2.0 * PI * r2;
    let p1 = r * cos(phi);
    let p2r = r * sin(phi);
    let s = 0.5 * (1.0 + vh.z);
    let p2 = (1.0 - s) * sqrt(max(1.0 - p1 * p1, 0.0)) + s * p2r;

    let nh = t1 * p1 + t2 * p2 + vh * sqrt(max(1.0 - p1 * p1 - p2 * p2, 0.0));
    return normalize(vec3<f32>(a * nh.x, b * nh.y, max(nh.z, 1e-9)));
}

// PDF of the VNDF sampling strategy, in solid angle around wi.
fn vndf_pdf(wo: vec3<f32>, wh: vec3<f32>, at: f32, ab: f32) -> f32 {
    let n_dot_v = max(wo.z, 1e-6);
    let d = d_ggx_aniso(wh, at, ab);
    let g1 = g1_smith_aniso(wo, at, ab);
    let o_dot_h = max(dot(wo, wh), 1e-9);
    return d * g1 * o_dot_h / n_dot_v / (4.0 * o_dot_h);
}

// Relative sampling weights of the three lobes (diffuse, specular, coat).
fn lobe_weights(m: GpuMaterial) -> vec3<f32> {
    let diff = max(max3(mat_diffuse_albedo(m)), 0.0);
    let spec = max(max3(mat_f0(m)), 0.0) + 0.08;
    let coat = m.clearcoat * 0.25;
    let total = max(diff + spec + coat, 1e-6);
    return vec3<f32>(diff, spec, coat) / total;
}

struct BsdfEval {
    // f * cos(theta_i), NOT the bare BSDF.
    value: vec3<f32>,
    pdf: f32,
}

// Evaluate the full BSDF and its sampling PDF for a given in/out pair.
fn bsdf_eval(m: GpuMaterial, wo: vec3<f32>, wi: vec3<f32>) -> BsdfEval {
    var out: BsdfEval;
    out.value = vec3<f32>(0.0);
    out.pdf = 0.0;
    if wi.z <= 0.0 || wo.z <= 0.0 {
        return out;
    }
    let n_dot_l = wi.z;
    let n_dot_v = wo.z;
    let wh = normalize(wo + wi);
    let n_dot_h = max(wh.z, 0.0);
    let o_dot_h = max(dot(wo, wh), 0.0);

    let w = lobe_weights(m);

    // Diffuse.
    let diffuse = mat_diffuse_albedo(m) * (n_dot_l / PI);
    let pdf_d = n_dot_l / PI;

    // Base specular. Anisotropy stretches the lobe along the local x axis,
    // which the integrator aligns with the surface tangent dP/du.
    let ab_pair = mat_alpha_tb(m);
    let d = d_ggx_aniso(wh, ab_pair.x, ab_pair.y);
    let vis = v_smith_aniso(wo, wi, ab_pair.x, ab_pair.y);
    let f = fresnel(mat_f0(m), o_dot_h);
    let spec = f * (d * vis * n_dot_l);
    let pdf_s = vndf_pdf(wo, wh, ab_pair.x, ab_pair.y);

    // Clearcoat: a thin dielectric layer over everything else. It is a
    // separate isotropic film — the grain lives in the substrate beneath it,
    // not in the lacquer — so it never takes the anisotropy.
    var coat = vec3<f32>(0.0);
    var pdf_c = 0.0;
    var coat_atten = 1.0;
    if m.clearcoat > 0.0 {
        let ca = mat_coat_alpha(m);
        let cd = d_ggx(n_dot_h, ca);
        let cv = v_smith(n_dot_v, n_dot_l, ca);
        let cf = fresnel(vec3<f32>(0.04), o_dot_h).x * m.clearcoat;
        let c = cd * cv * n_dot_l * cf;
        coat = vec3<f32>(c, c, c);
        pdf_c = vndf_pdf(wo, wh, ca, ca);
        // Energy removed from the layers beneath.
        coat_atten = 1.0 - cf;
    }

    let under = diffuse + spec;
    out.value = under * coat_atten + coat;
    out.pdf = max(w.x * pdf_d + w.y * pdf_s + w.z * pdf_c, 0.0);
    return out;
}

struct BsdfSample {
    wi: vec3<f32>,
    value: vec3<f32>,
    pdf: f32,
    ok: bool,
}

// Importance-sample the BSDF. `r_lobe` picks the lobe; `r1`/`r2` drive it.
fn bsdf_sample(m: GpuMaterial, wo: vec3<f32>, r_lobe: f32, r1: f32, r2: f32) -> BsdfSample {
    var out: BsdfSample;
    out.wi = vec3<f32>(0.0, 0.0, 1.0);
    out.value = vec3<f32>(0.0);
    out.pdf = 0.0;
    out.ok = false;
    if wo.z <= 0.0 {
        return out;
    }
    let w = lobe_weights(m);

    var wi: vec3<f32>;
    if r_lobe < w.x {
        wi = cosine_hemisphere_local(r1, r2);
    } else if r_lobe < w.x + w.y {
        let ab_pair = mat_alpha_tb(m);
        let wh = sample_vndf(wo, ab_pair.x, ab_pair.y, r1, r2);
        wi = reflect(-wo, wh);
        if wi.z <= 0.0 {
            return out;
        }
    } else {
        let ca = mat_coat_alpha(m);
        let wh = sample_vndf(wo, ca, ca, r1, r2);
        wi = reflect(-wo, wh);
        if wi.z <= 0.0 {
            return out;
        }
    }

    let e = bsdf_eval(m, wo, wi);
    if e.pdf <= 1e-9 {
        return out;
    }
    out.wi = wi;
    out.value = e.value;
    out.pdf = e.pdf;
    out.ok = true;
    return out;
}

// Power heuristic (beta = 2) for multiple importance sampling.
fn power_heuristic(a: f32, b: f32) -> f32 {
    let a2 = a * a;
    let b2 = b * b;
    if a2 + b2 <= 0.0 {
        return 0.0;
    }
    return a2 / (a2 + b2);
}

// Build an orthonormal basis around `n`. Mirrors `pathtrace::onb` so both
// paths agree on the tangent frame. Columns are (tangent, bitangent, normal).
fn onb(n: vec3<f32>) -> mat3x3<f32> {
    var sign_v = 1.0;
    if n.z < 0.0 {
        sign_v = -1.0;
    }
    let a = -1.0 / (sign_v + n.z);
    let b = n.x * n.y * a;
    let t = vec3<f32>(1.0 + sign_v * n.x * n.x * a, sign_v * b, -sign_v * n.x);
    let bt = vec3<f32>(b, sign_v + n.y * n.y * a, -n.y);
    return mat3x3<f32>(t, bt, n);
}

fn to_local(frame: mat3x3<f32>, w: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(w, frame[0]), dot(w, frame[1]), dot(w, frame[2]));
}

fn to_world(frame: mat3x3<f32>, w: vec3<f32>) -> vec3<f32> {
    return frame[0] * w.x + frame[1] * w.y + frame[2] * w.z;
}
