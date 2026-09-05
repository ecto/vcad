// Test harness for the shared BSDF. Not part of the render pipeline.
//
// Composed with `bsdf.wgsl` (which supplies PI, GpuMaterial, and the BSDF
// itself) and driven by `tests/bsdf_parity.rs`. Evaluates and samples the BSDF
// on the GPU so the port can be checked against the Rust reference in
// `pathtrace.rs`, and so the sample-PDF/eval-PDF identity that MIS depends on
// is pinned on the hardware that actually runs it.

struct ParityInput {
    material: GpuMaterial,
    // Local-frame outgoing direction (+Z = normal); .w unused.
    wo: vec4<f32>,
    // Local-frame incoming direction to evaluate against; .w unused.
    wi: vec4<f32>,
    // Sampling randoms: x = lobe pick, y/z = lobe params; .w unused.
    rnd: vec4<f32>,
}

struct ParityOutput {
    // bsdf_eval(m, wo, wi, eta, lambda_nm): .xyz = f*cos, .w = pdf. The
    // wavelength is 0 — the RGB sentinel — because nothing in this harness
    // is spectral.
    eval: vec4<f32>,
    // bsdf_sample(...): .xyz = sampled wi, .w = returned pdf.
    sample_dir: vec4<f32>,
    // .xyz = sampled f*cos, .w = 1 if the sample succeeded else 0.
    sample_value: vec4<f32>,
    // .x = bsdf_eval's pdf re-evaluated at the SAMPLED direction. Must equal
    // sample_dir.w or MIS is silently energy-wrong.
    resampled: vec4<f32>,
}

@group(0) @binding(0) var<storage, read> parity_in: array<ParityInput>;
@group(0) @binding(1) var<storage, read_write> parity_out: array<ParityOutput>;

@compute @workgroup_size(64)
fn bsdf_parity(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let i = global_id.x;
    if i >= arrayLength(&parity_in) {
        return;
    }
    let inp = parity_in[i];
    let m = inp.material;
    let wo = inp.wo.xyz;

    var out: ParityOutput;

    // These materials are opaque, so the dielectric lobe is off and `eta` is
    // whatever an unused parameter wants to be. `tests/gpu_bsdf.rs` over in
    // kosm-render is where transmission's own parity is checked.
    let eta = 1.0;

    let e = bsdf_eval(m, wo, inp.wi.xyz, eta, 0.0);
    out.eval = vec4<f32>(e.value, e.pdf);

    let s = bsdf_sample(m, wo, eta, 0.0, inp.rnd.x, inp.rnd.y, inp.rnd.z, inp.rnd.w);
    out.sample_dir = vec4<f32>(s.wi, s.pdf);
    var ok = 0.0;
    if s.ok {
        ok = 1.0;
    }
    out.sample_value = vec4<f32>(s.value, ok);

    let re = bsdf_eval(m, wo, s.wi, eta, 0.0);
    out.resampled = vec4<f32>(re.pdf, re.value.x, re.value.y, re.value.z);

    parity_out[i] = out;
}

// ─── surface tangent harness ──────────────────────────────────────────────
//
// Drives `surface_dpdu` (surface.wgsl) directly so the per-surface dP/du
// transcription can be checked against the geom crate's `d_du` without going
// through a full render. A silent error here would misalign every anisotropic
// highlight while every isotropic material still looked perfect.

struct TangentInput {
    surface: GpuSurface,
    // uv at the hit; .zw unused.
    uv: vec4<f32>,
}

@group(0) @binding(2) var<storage, read> tangent_in: array<TangentInput>;
@group(0) @binding(3) var<storage, read_write> tangent_out: array<vec4<f32>>;

@compute @workgroup_size(64)
fn tangent_parity(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let i = global_id.x;
    if i >= arrayLength(&tangent_in) {
        return;
    }
    let inp = tangent_in[i];
    let t = surface_dpdu(inp.surface.surface_type, inp.surface.params, inp.uv.xy);
    tangent_out[i] = vec4<f32>(t, 1.0);
}

// ─── environment harness ──────────────────────────────────────────────────
//
// Drives env.wgsl's radiance / pdf / sample against the CPU EnvMap. A wrong
// PDF here is the same invisible failure as a wrong BSDF PDF: MIS silently
// mis-weights and the image is energy-wrong but plausible.

struct EnvInput {
    // Direction to evaluate radiance and pdf for; .w unused.
    dir: vec4<f32>,
    // r1, r2 for importance sampling; .zw unused.
    rnd: vec4<f32>,
    width: u32,
    height: u32,
    intensity: f32,
    rotation: f32,
    marg_int: f32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

struct EnvOutput {
    // .xyz = radiance at dir, .w = pdf at dir.
    eval: vec4<f32>,
    // .xyz = sampled direction, .w = sampled pdf.
    sample_dir: vec4<f32>,
    // .xyz = sampled radiance, .w = 1 if the sample succeeded.
    sample_radiance: vec4<f32>,
    // .x = pdf re-evaluated at the sampled direction (must equal sample_dir.w).
    resampled: vec4<f32>,
}

@group(0) @binding(4) var<storage, read> env_in: array<EnvInput>;
@group(0) @binding(5) var<storage, read_write> env_out: array<EnvOutput>;
@group(0) @binding(6) var env_pixels: texture_2d<f32>;
@group(0) @binding(7) var env_cdf: texture_2d<f32>;

@compute @workgroup_size(64)
fn env_parity(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let i = global_id.x;
    if i >= arrayLength(&env_in) {
        return;
    }
    let e = env_in[i];
    let d = normalize(e.dir.xyz);

    var out: EnvOutput;
    out.eval = vec4<f32>(
        env_image_radiance(d, e.width, e.height, e.intensity, e.rotation),
        env_image_pdf(d, e.width, e.height, e.rotation, e.marg_int),
    );

    let s = env_image_sample(
        e.rnd.x, e.rnd.y, e.width, e.height, e.intensity, e.rotation, e.marg_int,
    );
    var ok = 0.0;
    if s.ok {
        ok = 1.0;
    }
    out.sample_dir = vec4<f32>(s.dir, s.pdf);
    out.sample_radiance = vec4<f32>(s.radiance, ok);
    out.resampled = vec4<f32>(
        env_image_pdf(s.dir, e.width, e.height, e.rotation, e.marg_int),
        0.0,
        0.0,
        0.0,
    );

    env_out[i] = out;
}
