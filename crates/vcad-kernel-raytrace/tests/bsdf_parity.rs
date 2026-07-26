//! Parity tests for the shared BSDF.
//!
//! The GPU viewport and the CPU photoreal renderer are supposed to shade with
//! ONE model: `pathtrace.rs` is the reference, `gpu/shaders/bsdf.wgsl` is a
//! port of it. Nothing about a rendered image makes a divergence between them
//! obvious — a wrong PDF or a dropped Fresnel term just shifts energy a little
//! and still looks like a plausible render. These tests run the WGSL on real
//! hardware and compare it to the Rust, number by number.
//!
//! Three things are checked:
//!
//! 1. `gpu_bsdf_eval_matches_cpu_reference` — the port agrees with the
//!    reference on `(f*cos, pdf)` across a grid of materials and directions.
//! 2. `gpu_bsdf_sample_pdf_matches_eval_pdf` — the PDF returned by
//!    `bsdf_sample` equals the PDF `bsdf_eval` reports for the sampled
//!    direction. This is the MIS invariant, and the GPU-side mirror of
//!    `pathtrace::tests::bsdf_sample_pdf_matches_eval_pdf`.
//! 3. `gpu_furnace_conserves_energy` — the Monte Carlo estimator `E[f/pdf]`
//!    lands on a plausible directional albedo. Unlike (2), which the current
//!    implementations satisfy structurally, this would actually catch a
//!    sampling routine that drew from a different distribution than its PDF
//!    claims.
//!
//! `#[ignore]`-tagged like the other GPU tests; run locally with:
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test bsdf_parity -- --ignored
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use bytemuck::{Pod, Zeroable};
use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_math::Vec3;
use vcad_kernel_raytrace::gpu::shaders;
use vcad_kernel_raytrace::gpu::GpuMaterial;
use vcad_kernel_raytrace::pathtrace::{reference_bsdf_eval, Pbr};

/// Mirrors `ParityInput` in `bsdf_parity.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ParityInput {
    material: GpuMaterial,
    wo: [f32; 4],
    wi: [f32; 4],
    rnd: [f32; 4],
}

/// Mirrors `ParityOutput` in `bsdf_parity.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct ParityOutput {
    eval: [f32; 4],
    sample_dir: [f32; 4],
    sample_value: [f32; 4],
    resampled: [f32; 4],
}

fn ctx_or_skip(test_name: &str) -> Option<&'static GpuContext> {
    match pollster::block_on(GpuContext::init()) {
        Ok(ctx) => Some(ctx),
        Err(GpuError::NoAdapter) => {
            eprintln!("[{test_name}] skipped: no compatible GPU adapter");
            None
        }
        Err(e) => panic!("GPU init failed unexpectedly: {e}"),
    }
}

/// Run the BSDF harness over `inputs` and read back one output per input.
fn run_harness(ctx: &GpuContext, inputs: &[ParityInput]) -> Vec<ParityOutput> {
    use wgpu::util::DeviceExt;

    let source = shaders::compose(shaders::BSDF_PARITY_HARNESS);
    ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("BSDF parity harness"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });

    let in_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("parity in"),
            contents: bytemuck::cast_slice(inputs),
            usage: wgpu::BufferUsages::STORAGE,
        });

    let out_size = (inputs.len() * std::mem::size_of::<ParityOutput>()) as u64;
    let out_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("parity out"),
        size: out_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("parity readback"),
        size: out_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let layout = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("parity layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

    let pipeline_layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("parity pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });

    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("parity pipeline"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("bsdf_parity"),
            compilation_options: Default::default(),
            cache: None,
        });

    let validation = pollster::block_on(ctx.device.pop_error_scope());
    assert!(
        validation.is_none(),
        "BSDF harness failed WebGPU validation: {validation:?}\n\
         A WGSL compile error here silently no-ops the dispatch and the \
         readback comes back all zeros — which would make every parity \
         assertion below compare 0 against 0 and pass vacuously.",
    );

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("parity bind group"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: in_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: out_buf.as_entire_binding(),
            },
        ],
    });

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("parity encoder"),
        });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("parity pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups((inputs.len() as u32).div_ceil(64), 1, 1);
    }
    encoder.copy_buffer_to_buffer(&out_buf, 0, &read_buf, 0, out_size);
    ctx.queue.submit(Some(encoder.finish()));

    let slice = read_buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    ctx.device.poll(wgpu::Maintain::Wait);
    rx.recv().expect("map channel").expect("map read");

    let data = slice.get_mapped_range();
    let out: Vec<ParityOutput> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    read_buf.unmap();
    out
}

/// A spread of materials that exercises every lobe and their combinations.
fn test_materials() -> Vec<GpuMaterial> {
    vec![
        // Rough dielectric — diffuse dominant.
        GpuMaterial {
            color: [0.8, 0.7, 0.6, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            ..Default::default()
        },
        // Smooth metal — specular only, no diffuse lobe.
        GpuMaterial {
            color: [0.95, 0.93, 0.88, 1.0],
            metallic: 1.0,
            roughness: 0.08,
            ..Default::default()
        },
        // Half-metal at mid roughness — the awkward blend.
        GpuMaterial {
            color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.3,
            roughness: 0.4,
            ..Default::default()
        },
        // Clearcoated plastic — all three lobes active. Matches the material
        // used by the Rust-side pdf test.
        GpuMaterial {
            color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.3,
            roughness: 0.4,
            clearcoat: 0.5,
            clearcoat_roughness: 0.1,
            ior: 1.5,
            _pad: [0.0; 3],
        },
        // High IOR dielectric, strong coat — pushes F0 and coat attenuation.
        GpuMaterial {
            color: [0.2, 0.35, 0.7, 1.0],
            metallic: 0.0,
            roughness: 0.25,
            clearcoat: 1.0,
            clearcoat_roughness: 0.03,
            ior: 1.8,
            _pad: [0.0; 3],
        },
    ]
}

/// Deterministic low-discrepancy-ish sequence, so a failure reproduces.
fn halton(mut i: u32, base: u32) -> f32 {
    let mut f = 1.0f32;
    let mut r = 0.0f32;
    while i > 0 {
        f /= base as f32;
        r += f * (i % base) as f32;
        i /= base;
    }
    r
}

fn unit(v: [f32; 3]) -> [f32; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / n, v[1] / n, v[2] / n]
}

/// The port must agree with the Rust reference on both the BSDF value and the
/// PDF. Tolerance is f32-scale: the reference computes in f64.
#[test]
#[ignore = "requires GPU"]
fn gpu_bsdf_eval_matches_cpu_reference() {
    let Some(ctx) = ctx_or_skip("gpu_bsdf_eval_matches_cpu_reference") else {
        return;
    };

    let mut inputs = Vec::new();
    for m in test_materials() {
        for k in 1..=24u32 {
            // A spread of view and light directions in the upper hemisphere,
            // including grazing angles where Fresnel and Smith terms bite.
            let wo = unit([
                0.9 * (halton(k, 2) - 0.5),
                0.9 * (halton(k, 3) - 0.5),
                0.15 + 0.85 * halton(k, 5),
            ]);
            let wi = unit([
                0.9 * (halton(k + 7, 3) - 0.5),
                0.9 * (halton(k + 7, 5) - 0.5),
                0.15 + 0.85 * halton(k + 7, 2),
            ]);
            inputs.push(ParityInput {
                material: m,
                wo: [wo[0], wo[1], wo[2], 0.0],
                wi: [wi[0], wi[1], wi[2], 0.0],
                rnd: [halton(k, 7), halton(k, 11), halton(k, 13), 0.0],
            });
        }
    }

    let outputs = run_harness(ctx, &inputs);
    assert_eq!(outputs.len(), inputs.len());

    let mut worst_value = 0.0f32;
    let mut worst_pdf = 0.0f32;
    for (i, (inp, out)) in inputs.iter().zip(&outputs).enumerate() {
        let pbr: Pbr = inp.material.to_pbr();
        let wo = Vec3::new(inp.wo[0] as f64, inp.wo[1] as f64, inp.wo[2] as f64);
        let wi = Vec3::new(inp.wi[0] as f64, inp.wi[1] as f64, inp.wi[2] as f64);
        let (ref_value, ref_pdf) = reference_bsdf_eval(&pbr, wo, wi);

        for c in 0..3 {
            let d = (out.eval[c] - ref_value[c]).abs();
            let tol = 2e-4 * ref_value[c].abs().max(1.0);
            worst_value = worst_value.max(d);
            assert!(
                d <= tol,
                "input {i}: BSDF value channel {c} diverged — GPU {} vs CPU {} \
                 (delta {d}). The WGSL port in bsdf.wgsl no longer matches \
                 pathtrace::bsdf_eval; the viewport and --photoreal will \
                 disagree on this material.",
                out.eval[c],
                ref_value[c],
            );
        }

        let dp = (out.eval[3] - ref_pdf).abs();
        let tolp = 2e-4 * ref_pdf.abs().max(1.0);
        worst_pdf = worst_pdf.max(dp);
        assert!(
            dp <= tolp,
            "input {i}: PDF diverged — GPU {} vs CPU {ref_pdf} (delta {dp}). \
             MIS weights are computed from this number, so a mismatch makes \
             the GPU image energy-wrong in a way that is invisible by eye.",
            out.eval[3],
        );
    }
    eprintln!("worst value delta {worst_value:e}, worst pdf delta {worst_pdf:e}");

    // Guard against a vacuous pass: if the dispatch silently no-op'd, every
    // output would be zero and the comparisons above would still hold only if
    // the reference were also zero everywhere. Require real signal.
    let non_zero = outputs.iter().filter(|o| o.eval[3] > 0.0).count();
    assert!(
        non_zero > inputs.len() / 2,
        "only {non_zero}/{} inputs produced a positive PDF — the harness \
         probably did not run",
        inputs.len(),
    );
}

/// The MIS invariant, checked on the GPU: the PDF `bsdf_sample` hands back must
/// be the PDF `bsdf_eval` reports for the direction it sampled.
#[test]
#[ignore = "requires GPU"]
fn gpu_bsdf_sample_pdf_matches_eval_pdf() {
    let Some(ctx) = ctx_or_skip("gpu_bsdf_sample_pdf_matches_eval_pdf") else {
        return;
    };

    let wo = unit([0.3, 0.15, 0.94]);
    let mut inputs = Vec::new();
    for m in test_materials() {
        for k in 1..=256u32 {
            inputs.push(ParityInput {
                material: m,
                wo: [wo[0], wo[1], wo[2], 0.0],
                wi: [0.0, 0.0, 1.0, 0.0],
                rnd: [halton(k, 2), halton(k, 3), halton(k, 5), 0.0],
            });
        }
    }

    let outputs = run_harness(ctx, &inputs);
    let mut sampled = 0usize;
    for (i, out) in outputs.iter().enumerate() {
        if out.sample_value[3] < 0.5 {
            continue; // sample was rejected (below horizon); nothing to check
        }
        sampled += 1;
        let pdf = out.sample_dir[3];
        let re_pdf = out.resampled[0];
        assert!(
            (pdf - re_pdf).abs() <= 1e-4 * pdf.max(1.0),
            "sample {i}: pdf mismatch — bsdf_sample returned {pdf}, \
             bsdf_eval reports {re_pdf} for the same directions. MIS combines \
             these two numbers; when they disagree the image is energy-wrong \
             and still looks plausible.",
        );
    }
    assert!(
        sampled > inputs.len() / 4,
        "only {sampled}/{} samples succeeded — the harness probably did not run",
        inputs.len(),
    );
}

/// White furnace: `E[f/pdf]` is the directional albedo. If `bsdf_sample` drew
/// from a distribution that did not match the PDF it reports, this integral
/// would drift away from unity even though test (2) still passed.
#[test]
#[ignore = "requires GPU"]
fn gpu_furnace_conserves_energy() {
    let Some(ctx) = ctx_or_skip("gpu_furnace_conserves_energy") else {
        return;
    };

    // Pure-white rough dielectric, viewed head-on — matches the Rust-side
    // `furnace_conserves_energy_roughly` setup.
    let m = GpuMaterial {
        color: [1.0, 1.0, 1.0, 1.0],
        metallic: 0.0,
        roughness: 0.5,
        clearcoat: 0.0,
        clearcoat_roughness: 0.1,
        ior: 1.5,
        _pad: [0.0; 3],
    };

    let n = 20_000u32;
    let inputs: Vec<ParityInput> = (0..n)
        .map(|k| ParityInput {
            material: m,
            wo: [0.0, 0.0, 1.0, 0.0],
            wi: [0.0, 0.0, 1.0, 0.0],
            // Three independent-ish streams for lobe pick and the two lobe
            // parameters.
            rnd: [halton(k + 1, 2), halton(k + 1, 3), halton(k + 1, 5), 0.0],
        })
        .collect();

    let outputs = run_harness(ctx, &inputs);
    let mut sum = 0.0f64;
    for out in &outputs {
        if out.sample_value[3] < 0.5 {
            continue;
        }
        let pdf = out.sample_dir[3];
        if pdf > 0.0 {
            sum += (out.sample_value[0] / pdf) as f64;
        }
    }
    let albedo = sum / n as f64;
    assert!(
        (0.75..=1.05).contains(&albedo),
        "GPU directional albedo {albedo} outside the plausible range — the \
         WGSL sampling routine and its PDF disagree, so the path tracer will \
         gain or lose energy at every bounce.",
    );
}
