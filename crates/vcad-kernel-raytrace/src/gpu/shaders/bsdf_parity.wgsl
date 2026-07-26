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
    // bsdf_eval(m, wo, wi): .xyz = f*cos, .w = pdf.
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

    let e = bsdf_eval(m, wo, inp.wi.xyz);
    out.eval = vec4<f32>(e.value, e.pdf);

    let s = bsdf_sample(m, wo, inp.rnd.x, inp.rnd.y, inp.rnd.z);
    out.sample_dir = vec4<f32>(s.wi, s.pdf);
    var ok = 0.0;
    if s.ok {
        ok = 1.0;
    }
    out.sample_value = vec4<f32>(s.value, ok);

    let re = bsdf_eval(m, wo, s.wi);
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
