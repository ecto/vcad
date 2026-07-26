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
