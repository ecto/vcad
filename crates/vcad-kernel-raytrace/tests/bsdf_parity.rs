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

/// Mirrors `GpuSurface` in `surface.wgsl` / `buffers.rs`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GpuSurfaceRaw {
    surface_type: u32,
    _pad: [u32; 3],
    params: [f32; 32],
}

/// Mirrors `TangentInput` in `bsdf_parity.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct TangentInput {
    surface: GpuSurfaceRaw,
    uv: [f32; 4],
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

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Run one entry point of the harness module.
///
/// All entry points live in one module, so the bind group must cover every
/// binding any of them uses; a pass that does not care about a buffer still
/// binds a small dummy. Inputs sit on even bindings, outputs on odd ones
/// (plus binding 6, the environment data). Returns the raw bytes of
/// `out_binding`.
fn run_pass(
    ctx: &GpuContext,
    entry: &str,
    ins: &[(u32, &[u8])],
    env: Option<&vcad_kernel_raytrace::pathtrace::GpuEnvPack>,
    out_binding: u32,
    out_size: u64,
    invocations: u32,
) -> Vec<u8> {
    use wgpu::util::DeviceExt;

    const IN_BINDINGS: [u32; 3] = [0, 2, 4];
    const OUT_BINDINGS: [u32; 3] = [1, 3, 5];

    let source = shaders::compose(shaders::BSDF_PARITY_HARNESS);
    let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("BSDF parity harness"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });

    let dummy = [0u8; 64];
    let mut buffers: Vec<(u32, wgpu::Buffer)> = Vec::new();
    for b in IN_BINDINGS {
        let bytes = ins
            .iter()
            .find(|(bind, _)| *bind == b)
            .map(|(_, d)| *d)
            .unwrap_or(&dummy);
        buffers.push((
            b,
            ctx.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("harness in"),
                    contents: bytes,
                    usage: wgpu::BufferUsages::STORAGE,
                }),
        ));
    }
    for b in OUT_BINDINGS {
        let size = if b == out_binding { out_size } else { 64 };
        buffers.push((
            b,
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("harness out"),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
        ));
    }
    buffers.sort_by_key(|(b, _)| *b);

    // Environment textures (bindings 6 and 7). A pass that does not use them
    // still binds 1x1 dummies — a texture binding cannot be null.
    let mk_tex = |w: u32, h: u32, fmt: wgpu::TextureFormat, data: &[f32]| {
        let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("env tex"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: fmt,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let bpp = if fmt == wgpu::TextureFormat::Rgba32Float {
            16
        } else {
            4
        };
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * bpp),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        tex.create_view(&wgpu::TextureViewDescriptor::default())
    };
    let (env_px, env_cdf) = match env {
        Some(e) => (
            mk_tex(
                e.width,
                e.height,
                wgpu::TextureFormat::Rgba32Float,
                &e.pixels,
            ),
            mk_tex(
                e.width + 1,
                e.height + 1,
                wgpu::TextureFormat::R32Float,
                &e.cdf,
            ),
        ),
        None => (
            mk_tex(
                1,
                1,
                wgpu::TextureFormat::Rgba32Float,
                &[0.0, 0.0, 0.0, 1.0],
            ),
            mk_tex(1, 1, wgpu::TextureFormat::R32Float, &[0.0]),
        ),
    };
    let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };

    let read_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: out_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut entries: Vec<wgpu::BindGroupLayoutEntry> = buffers
        .iter()
        .map(|(b, _)| storage_entry(*b, IN_BINDINGS.contains(b)))
        .collect();
    entries.push(tex_entry(6));
    entries.push(tex_entry(7));
    let layout = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("parity layout"),
            entries: &entries,
        });

    let pipeline_layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("parity pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("parity pipeline"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        });

    let validation = pollster::block_on(scope.pop());
    assert!(
        validation.is_none(),
        "harness failed WebGPU validation: {validation:?}\n\
         A WGSL compile error here silently no-ops the dispatch and the \
         readback comes back all zeros — which would make every parity \
         assertion below compare 0 against 0 and pass vacuously.",
    );

    let mut bg_entries: Vec<wgpu::BindGroupEntry> = buffers
        .iter()
        .map(|(b, buf)| wgpu::BindGroupEntry {
            binding: *b,
            resource: buf.as_entire_binding(),
        })
        .collect();
    bg_entries.push(wgpu::BindGroupEntry {
        binding: 6,
        resource: wgpu::BindingResource::TextureView(&env_px),
    });
    bg_entries.push(wgpu::BindGroupEntry {
        binding: 7,
        resource: wgpu::BindingResource::TextureView(&env_cdf),
    });
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("parity bind group"),
        layout: &layout,
        entries: &bg_entries,
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
        pass.dispatch_workgroups(invocations.div_ceil(64), 1, 1);
    }
    let src = &buffers.iter().find(|(b, _)| *b == out_binding).unwrap().1;
    encoder.copy_buffer_to_buffer(src, 0, &read_buf, 0, out_size);
    ctx.queue.submit(Some(encoder.finish()));

    let slice = read_buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv().expect("map channel").expect("map read");

    let data = slice
        .get_mapped_range()
        .expect("the buffer was just mapped");
    let out = data.to_vec();
    drop(data);
    read_buf.unmap();
    out
}

/// Run the BSDF harness over `inputs` and read back one output per input.
fn run_harness(ctx: &GpuContext, inputs: &[ParityInput]) -> Vec<ParityOutput> {
    let bytes = run_pass(
        ctx,
        "bsdf_parity",
        &[(0, bytemuck::cast_slice(inputs))],
        None,
        1,
        (inputs.len() * std::mem::size_of::<ParityOutput>()) as u64,
        inputs.len() as u32,
    );
    bytemuck::cast_slice(&bytes).to_vec()
}

/// Run the surface-tangent harness and read back one dP/du per input.
fn run_tangent_harness(ctx: &GpuContext, inputs: &[TangentInput]) -> Vec<[f32; 4]> {
    let bytes = run_pass(
        ctx,
        "tangent_parity",
        &[(2, bytemuck::cast_slice(inputs))],
        None,
        3,
        (inputs.len() * 16) as u64,
        inputs.len() as u32,
    );
    bytemuck::cast_slice(&bytes).to_vec()
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
            anisotropy: 0.0,
            _pad: [0.0; 2],
        },
        // High IOR dielectric, strong coat — pushes F0 and coat attenuation.
        GpuMaterial {
            color: [0.2, 0.35, 0.7, 1.0],
            metallic: 0.0,
            roughness: 0.25,
            clearcoat: 1.0,
            clearcoat_roughness: 0.03,
            ior: 1.8,
            anisotropy: 0.0,
            _pad: [0.0; 2],
        },
        // Brushed metal — anisotropy swept across both signs and both
        // extremes. The anisotropic D/G/VNDF paths are separate branches from
        // the isotropic ones, so they need their own coverage or a divergence
        // hides until someone ships a brushed material.
        GpuMaterial {
            color: [0.91, 0.92, 0.92, 1.0],
            metallic: 1.0,
            roughness: 0.3,
            anisotropy: 0.85,
            ..Default::default()
        },
        GpuMaterial {
            color: [0.91, 0.92, 0.92, 1.0],
            metallic: 1.0,
            roughness: 0.3,
            anisotropy: -0.85,
            ..Default::default()
        },
        // Anisotropic AND clearcoated: the coat must stay isotropic while the
        // substrate takes the grain.
        GpuMaterial {
            color: [0.6, 0.15, 0.15, 1.0],
            metallic: 0.2,
            roughness: 0.35,
            clearcoat: 0.7,
            clearcoat_roughness: 0.08,
            ior: 1.5,
            anisotropy: 0.5,
            _pad: [0.0; 2],
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

        for (c, &rv) in ref_value.iter().enumerate() {
            let d = (out.eval[c] - rv).abs();
            let tol = 2e-4 * rv.abs().max(1.0);
            worst_value = worst_value.max(d);
            assert!(
                d <= tol,
                "input {i}: BSDF value channel {c} diverged — GPU {} vs CPU {} \
                 (delta {d}). The WGSL port in bsdf.wgsl no longer matches \
                 pathtrace::bsdf_eval; the viewport and --photoreal will \
                 disagree on this material.",
                out.eval[c],
                rv,
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
        anisotropy: 0.0,
        _pad: [0.0; 2],
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

/// Guard against the anisotropy field being silently dropped somewhere in the
/// Rust→WGSL struct layout. If the GPU read it as zero, the parity test above
/// would still pass for every isotropic material and quietly stop covering the
/// anisotropic D/G/VNDF branches at all.
#[test]
#[ignore = "requires GPU"]
fn gpu_anisotropy_actually_changes_the_lobe() {
    let Some(ctx) = ctx_or_skip("gpu_anisotropy_actually_changes_the_lobe") else {
        return;
    };

    let base = GpuMaterial {
        color: [0.9, 0.9, 0.9, 1.0],
        metallic: 1.0,
        roughness: 0.3,
        ..Default::default()
    };
    let mk = |anisotropy: f32| GpuMaterial { anisotropy, ..base };

    // Off-axis half-vector so the tangent and bitangent alphas differ.
    let wo = unit([0.4, 0.0, 0.92]);
    let wi = unit([0.0, 0.45, 0.89]);
    let inputs: Vec<ParityInput> = [-0.85f32, 0.0, 0.85]
        .iter()
        .map(|&a| ParityInput {
            material: mk(a),
            wo: [wo[0], wo[1], wo[2], 0.0],
            wi: [wi[0], wi[1], wi[2], 0.0],
            rnd: [0.3, 0.4, 0.6, 0.0],
        })
        .collect();

    let out = run_harness(ctx, &inputs);
    let (neg, iso, pos) = (out[0].eval[0], out[1].eval[0], out[2].eval[0]);
    assert!(
        (pos - iso).abs() > 1e-4 && (neg - iso).abs() > 1e-4,
        "anisotropy did not change the BSDF on the GPU (neg {neg}, iso {iso}, \
         pos {pos}) — the field is probably not reaching the shader, which \
         would make the anisotropic parity coverage vacuous",
    );
    assert!(
        (pos - neg).abs() > 1e-4,
        "positive and negative anisotropy produced the same value ({pos} vs \
         {neg}) — the tangent/bitangent alphas are not being swapped",
    );
}

/// The GPU's `surface_dpdu` must agree with the geom crate's `d_du` — the same
/// quantity `intersect::surface_tangent` hands the CPU path tracer.
///
/// This is what aligns an anisotropic highlight with the surface's own grain.
/// Get it wrong and every isotropic material still looks perfect, so nothing
/// else in the suite would notice.
///
/// Compared as *directions*: the frame normalises the tangent and the GGX alpha
/// ellipse is symmetric, so magnitude and sign are both irrelevant — but
/// degeneracy (a zero-length tangent at a pole or apex) must agree exactly.
#[test]
#[ignore = "requires GPU"]
fn gpu_surface_tangent_matches_geom_d_du() {
    use vcad_kernel_geom::{
        ConeSurface, CylinderSurface, Plane, SphereSurface, Surface, TorusSurface,
    };
    use vcad_kernel_math::{Point2, Point3, Vec3};
    use vcad_kernel_raytrace::gpu::GpuSurface;
    use vcad_kernel_raytrace::intersect::surface_tangent;

    let Some(ctx) = ctx_or_skip("gpu_surface_tangent_matches_geom_d_du") else {
        return;
    };

    // Off-origin, off-axis where the constructors allow it, so a transcription
    // that silently assumed a canonical frame would show up.
    let o = Point3::new(1.0, -2.0, 0.5);
    let tilted = Vec3::new(0.3, -0.2, 1.0);
    let surfaces: Vec<Box<dyn Surface>> = vec![
        Box::new(Plane::new(
            o,
            Vec3::new(0.8, 0.6, 0.0),
            Vec3::new(-0.6, 0.8, 0.0),
        )),
        Box::new(CylinderSurface::with_axis(o, tilted, 7.5)),
        Box::new(SphereSurface::with_center(o, 4.25)),
        Box::new(ConeSurface::new(0.45)),
        Box::new(TorusSurface::with_axis(o, tilted, 9.0, 2.5)),
    ];

    // Sweep u right around, and v across both hemispheres / both sides of the
    // cone apex, so the degenerate cases are covered too.
    let uvs: Vec<(f64, f64)> = (0..8)
        .flat_map(|i| {
            let u = i as f64 * std::f64::consts::TAU / 8.0;
            [-1.2, -0.5, 0.0, 0.5, 1.2].map(move |v| (u, v))
        })
        .collect();

    let mut inputs = Vec::new();
    let mut expected = Vec::new();
    for s in &surfaces {
        let packed = GpuSurface::from_surface(s.as_ref());
        for &(u, v) in &uvs {
            inputs.push(TangentInput {
                surface: GpuSurfaceRaw {
                    surface_type: packed.surface_type,
                    _pad: [0; 3],
                    params: packed.params,
                },
                uv: [u as f32, v as f32, 0.0, 0.0],
            });
            expected.push(surface_tangent(s.as_ref(), Point2::new(u, v)));
        }
    }

    let out = run_tangent_harness(ctx, &inputs);
    assert_eq!(out.len(), inputs.len());

    let mut compared = 0usize;
    for (i, (got, want)) in out.iter().zip(&expected).enumerate() {
        let g = Vec3::new(got[0] as f64, got[1] as f64, got[2] as f64);
        match want {
            None => assert!(
                g.norm() <= 1e-6,
                "input {i}: CPU reports a degenerate tangent but the GPU \
                 returned {g:?} — the shader would build a frame from noise",
            ),
            Some(w) => {
                assert!(
                    g.norm() > 1e-9,
                    "input {i}: GPU returned a zero tangent where the CPU has \
                     {w:?} — anisotropic shading would silently fall back to \
                     an arbitrary basis here",
                );
                // Direction only, sign-insensitive.
                let cosang = (g.normalize().dot(w.normalize())).abs();
                assert!(
                    cosang > 1.0 - 1e-4,
                    "input {i}: tangent direction diverged — GPU {:?} vs CPU \
                     {:?} (|cos| {cosang}). An anisotropic highlight would run \
                     the wrong way across this surface.",
                    g.normalize(),
                    w.normalize(),
                );
                compared += 1;
            }
        }
    }
    assert!(
        compared > inputs.len() / 2,
        "only {compared}/{} tangents were non-degenerate — the sweep is not \
         exercising the surfaces",
        inputs.len(),
    );
}

/// A `MaterialDef` must produce the same render material on both paths.
///
/// `setMaterial` used to carry only colour/metallic/roughness, so clearcoat,
/// IOR and anisotropy were silently dropped on the way to the GPU and a
/// brushed or lacquered part shaded differently in the viewport than under
/// `--photoreal`. Both now go through `pathtrace::from_material_def`; this pins that
/// the GPU packing round-trips it without loss.
#[test]
fn gpu_material_round_trips_the_shared_derivation() {
    use vcad_ir::MaterialDef;

    let defs = [
        // Explicit anisotropy wins over the name heuristic.
        MaterialDef {
            name: "brushed_aluminum".into(),
            color: [0.91, 0.92, 0.92],
            metallic: 1.0,
            roughness: 0.28,
            anisotropy: Some(-0.4),
            ..Default::default()
        },
        // Name heuristic fills in when the document says nothing.
        MaterialDef {
            name: "turned_shaft".into(),
            color: [0.7, 0.7, 0.72],
            metallic: 1.0,
            roughness: 0.2,
            ..Default::default()
        },
        // Glossy dielectric: picks up the derived clearcoat.
        MaterialDef {
            name: "abs_gloss".into(),
            color: [0.2, 0.35, 0.7],
            metallic: 0.0,
            roughness: 0.15,
            ior: Some(1.6),
            ..Default::default()
        },
    ];

    for d in &defs {
        let cpu = vcad_kernel_raytrace::pathtrace::from_material_def(Some(d), None);
        let gpu = GpuMaterial::from_pbr(cpu).to_pbr();
        assert_eq!(
            cpu.metallic, gpu.metallic,
            "{}: metallic lost in the GPU packing",
            d.name
        );
        assert_eq!(cpu.roughness, gpu.roughness, "{}: roughness lost", d.name);
        assert_eq!(
            cpu.clearcoat, gpu.clearcoat,
            "{}: clearcoat lost — the viewport would render this matte",
            d.name
        );
        assert_eq!(cpu.ior, gpu.ior, "{}: ior lost", d.name);
        assert_eq!(
            cpu.anisotropy, gpu.anisotropy,
            "{}: anisotropy lost — the grain would vanish in the viewport",
            d.name
        );
        assert_eq!(cpu.base_color, gpu.base_color, "{}: colour lost", d.name);
    }

    // The heuristic must actually be doing something, or the assertions above
    // are comparing zero against zero.
    let turned = vcad_kernel_raytrace::pathtrace::from_material_def(Some(&defs[1]), None);
    assert!(
        turned.anisotropy > 0.5,
        "the name heuristic did not fire for 'turned_shaft' (got {})",
        turned.anisotropy
    );
    let brushed = vcad_kernel_raytrace::pathtrace::from_material_def(Some(&defs[0]), None);
    assert!(
        (brushed.anisotropy - -0.4).abs() < 1e-6,
        "explicit anisotropy should win over the name heuristic (got {})",
        brushed.anisotropy
    );
    assert!(
        vcad_kernel_raytrace::pathtrace::from_material_def(Some(&defs[2]), None).clearcoat > 0.0,
        "a glossy dielectric should pick up a clearcoat"
    );
}

/// Mirrors `EnvInput` in `bsdf_parity.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct EnvInput {
    dir: [f32; 4],
    rnd: [f32; 4],
    width: u32,
    height: u32,
    intensity: f32,
    rotation: f32,
    marg_int: f32,
    _pad: [u32; 3],
}

/// Mirrors `EnvOutput` in `bsdf_parity.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct EnvOutput {
    eval: [f32; 4],
    sample_dir: [f32; 4],
    sample_radiance: [f32; 4],
    resampled: [f32; 4],
}

/// A small but genuinely high-frequency lat-long map: a bright "sun" texel
/// against a dim sky, plus a warm horizon band. Flat maps would let a broken
/// CDF pass, since every bin would be equally likely.
fn test_envmap() -> vcad_kernel_raytrace::pathtrace::EnvMap {
    use vcad_kernel_raytrace::pathtrace::EnvMap;
    let (w, h) = (32usize, 16usize);
    let mut px = vec![[0.02f32, 0.03, 0.05]; w * h];
    for (j, row) in (0..h).enumerate() {
        let _ = row;
        for i in 0..w {
            // Warm band across the horizon.
            if j == h / 2 {
                px[j * w + i] = [0.6, 0.45, 0.3];
            }
        }
    }
    // A hot, tiny sun well away from the poles.
    px[4 * w + 9] = [180.0, 170.0, 150.0];
    px[4 * w + 10] = [90.0, 85.0, 75.0];
    EnvMap::new(w, h, px)
        .expect("valid dimensions")
        .with_intensity(1.3)
        .with_rotation_deg(37.0)
}

/// The GPU environment must agree with `pathtrace::EnvMap` on radiance, on the
/// solid-angle PDF, and on what importance sampling draws.
///
/// The PDF is the dangerous one: MIS combines it with the BSDF PDF, so a
/// mismatch produces an image that is energy-wrong yet entirely plausible.
#[test]
#[ignore = "requires GPU"]
fn gpu_environment_matches_cpu_envmap() {
    use vcad_kernel_math::Vec3;

    let Some(ctx) = ctx_or_skip("gpu_environment_matches_cpu_envmap") else {
        return;
    };

    let env = test_envmap();
    let pack = env.pack_for_gpu();

    // Directions spread over the whole sphere, including near-polar ones where
    // the sin(theta) Jacobian blows up.
    let dirs: Vec<Vec3> = (0..64)
        .map(|k| {
            let a = k as f64 * 0.9312;
            let z = -0.985 + 1.97 * (k as f64 / 63.0);
            let r = (1.0 - z * z).max(0.0).sqrt();
            Vec3::new(r * a.cos(), r * a.sin(), z).normalize()
        })
        .collect();

    let inputs: Vec<EnvInput> = dirs
        .iter()
        .enumerate()
        .map(|(k, d)| EnvInput {
            dir: [d.x as f32, d.y as f32, d.z as f32, 0.0],
            rnd: [halton(k as u32 + 1, 2), halton(k as u32 + 1, 3), 0.0, 0.0],
            width: pack.width,
            height: pack.height,
            intensity: pack.intensity,
            rotation: pack.rotation,
            marg_int: pack.marg_int,
            _pad: [0; 3],
        })
        .collect();

    let bytes = run_pass(
        ctx,
        "env_parity",
        &[(4, bytemuck::cast_slice(&inputs))],
        Some(&pack),
        5,
        (inputs.len() * std::mem::size_of::<EnvOutput>()) as u64,
        inputs.len() as u32,
    );
    let out: Vec<EnvOutput> = bytemuck::cast_slice(&bytes).to_vec();
    assert_eq!(out.len(), inputs.len());

    let mut sampled = 0usize;
    let mut hot = 0usize;
    for (k, (d, o)) in dirs.iter().zip(&out).enumerate() {
        // Radiance.
        let want = env.radiance(*d);
        for (c, &wc) in want.iter().enumerate() {
            let tol = 1e-4 * wc.abs().max(1.0);
            assert!(
                (o.eval[c] - wc).abs() <= tol,
                "dir {k} channel {c}: env radiance diverged — GPU {} vs CPU {wc} \
                 (the two renderers would light the scene differently)",
                o.eval[c],
            );
        }
        if want[0] > 1.0 {
            hot += 1;
        }

        // PDF.
        let want_pdf = env.pdf(*d);
        assert!(
            (o.eval[3] - want_pdf).abs() <= 1e-4 * want_pdf.abs().max(1.0),
            "dir {k}: env PDF diverged — GPU {} vs CPU {want_pdf}. MIS weights \
             come from this number, so the image goes energy-wrong invisibly.",
            o.eval[3],
        );

        // Sampling: the returned pdf must equal the pdf re-evaluated at the
        // sampled direction, exactly as the CPU recomputes it through `pdf`.
        if o.sample_radiance[3] > 0.5 {
            sampled += 1;
            let p = o.sample_dir[3];
            assert!(
                (p - o.resampled[0]).abs() <= 1e-4 * p.max(1.0),
                "sample {k}: env sample pdf {p} != pdf at the sampled direction \
                 {}",
                o.resampled[0],
            );
            // And that direction's CPU pdf must match too.
            let sd = Vec3::new(
                o.sample_dir[0] as f64,
                o.sample_dir[1] as f64,
                o.sample_dir[2] as f64,
            );
            let cpu_p = env.pdf(sd);
            assert!(
                (p - cpu_p).abs() <= 2e-3 * p.max(1.0),
                "sample {k}: GPU sampled pdf {p} vs CPU pdf {cpu_p} at the same \
                 direction — the two CDFs disagree",
            );
        }
    }

    assert!(
        sampled > inputs.len() / 2,
        "only {sampled}/{} env samples succeeded — the harness probably did \
         not run",
        inputs.len(),
    );
    assert!(
        hot > 0,
        "no direction landed on the bright sun texel — the sweep is not \
         exercising the high-frequency part of the map, which is exactly where \
         a broken CDF would show",
    );
}

/// Importance sampling must actually concentrate on the bright texels. A CDF
/// that degenerated to uniform would still satisfy the pdf-agreement checks
/// above while converging far more slowly than the CPU.
#[test]
#[ignore = "requires GPU"]
fn gpu_environment_sampling_finds_the_sun() {
    let Some(ctx) = ctx_or_skip("gpu_environment_sampling_finds_the_sun") else {
        return;
    };

    let env = test_envmap();
    let pack = env.pack_for_gpu();

    let n = 2048u32;
    let inputs: Vec<EnvInput> = (0..n)
        .map(|k| EnvInput {
            dir: [0.0, 0.0, 1.0, 0.0],
            rnd: [halton(k + 1, 2), halton(k + 1, 3), 0.0, 0.0],
            width: pack.width,
            height: pack.height,
            intensity: pack.intensity,
            rotation: pack.rotation,
            marg_int: pack.marg_int,
            _pad: [0; 3],
        })
        .collect();

    let bytes = run_pass(
        ctx,
        "env_parity",
        &[(4, bytemuck::cast_slice(&inputs))],
        Some(&pack),
        5,
        (inputs.len() * std::mem::size_of::<EnvOutput>()) as u64,
        inputs.len() as u32,
    );
    let out: Vec<EnvOutput> = bytemuck::cast_slice(&bytes).to_vec();

    let bright = out
        .iter()
        .filter(|o| o.sample_radiance[3] > 0.5 && o.sample_radiance[0] > 1.0)
        .count();
    // The sun is 2 texels of 512, i.e. 0.4% of the image by area, but carries
    // almost all the energy — importance sampling should land on it far more
    // often than uniform sampling would.
    assert!(
        bright > out.len() / 10,
        "only {bright}/{} samples hit the bright texels — the environment CDF \
         is not concentrating on the energy, so the GPU would converge far \
         more slowly than the CPU on the same map",
        out.len(),
    );

    // Monte Carlo sanity: E[L/pdf] over the sphere is the mean radiance times
    // 4*pi. A mis-normalised CDF shows up here even when every individual pdf
    // is self-consistent.
    let mut sum = 0.0f64;
    let mut used = 0usize;
    for o in &out {
        if o.sample_radiance[3] > 0.5 && o.sample_dir[3] > 0.0 {
            sum += (o.sample_radiance[0] / o.sample_dir[3]) as f64;
            used += 1;
        }
    }
    assert!(used > 0, "no usable samples");
    let integral = sum / used as f64;

    // Reference: integrate the same quantity on the CPU with its own sampler.
    let mut cpu_sum = 0.0f64;
    let mut cpu_used = 0usize;
    for k in 0..n {
        let r1 = halton(k + 1, 5) as f64;
        let r2 = halton(k + 1, 7) as f64;
        if let Some((_d, li, pdf)) = env.sample(r1, r2) {
            if pdf > 0.0 {
                cpu_sum += (li[0] / pdf) as f64;
                cpu_used += 1;
            }
        }
    }
    let cpu_integral = cpu_sum / cpu_used.max(1) as f64;
    let rel = (integral - cpu_integral).abs() / cpu_integral.max(1e-6);
    assert!(
        rel < 0.05,
        "GPU env integral {integral} vs CPU {cpu_integral} (rel {rel}) — the \
         estimators disagree, so the two renderers would converge to different \
         images",
    );
}

/// The render shader must stay inside the browser's storage-buffer budget.
///
/// WebGPU's `maxStorageBuffersPerShaderStage` is **10** in Chrome, and the
/// renderer sits exactly at that limit. Native Metal allows far more, so every
/// GPU test here passes while the app viewport dies with an "Invalid
/// BindGroupLayout" — the shader compiles, the pipeline is rejected, and the
/// overlay silently never paints. That happened once; this is the guard.
///
/// If you need more data in the shader, use a texture (48 slots) rather than an
/// eleventh storage buffer, as the HDR environment does.
#[test]
fn render_shader_fits_the_browser_storage_buffer_budget() {
    const BROWSER_LIMIT: usize = 10;
    let src = kosm_render::gpu::shaders::trace_shader(
        &vcad_kernel_raytrace::gpu::BrepGeometry::module().wgsl,
    );
    let count = src
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("//") && t.contains("var<storage")
        })
        .count();
    assert!(
        count <= BROWSER_LIMIT,
        "the ray trace shader declares {count} storage buffers, over the \
         browser limit of {BROWSER_LIMIT}. WebGPU will reject the bind group \
         layout and the viewport will render nothing, while every native GPU \
         test still passes. Move the new data into a texture instead.",
    );
}

/// Retro-reflection: `wo`, `wi` and the normal all within a couple of degrees.
///
/// This is the corner the rest of the grid never visits — its directions are
/// drawn independently, so `wo ≈ wi` essentially never comes up — and it is
/// the one where the specular lobe's algebra is most likely to fall over. The
/// half-vector `normalize(wo + wi)` is being asked for the direction of a sum
/// of two nearly identical unit vectors; `d_ggx` divides by
/// `(n·h)²(a² - 1) + 1`, which is `a²` at `n·h = 1` and small for a smooth
/// material; `v_smith`'s two square roots meet at `n·v = n·l = 1`; and
/// `fresnel` is evaluated at `o·h = 1`, where the Schlick term's `(1 - cosθ)⁵`
/// is a fifth power of a number the f32 subtraction has just cancelled most of
/// the significant digits out of.
///
/// A ~6% dark ring was reported in a GPU render exactly where a camera ray
/// meets a matte wall at normal incidence — the retro configuration — so this
/// checks the shading model there directly, over a range of roughnesses and
/// with the metals and the coat included. It holds to the same f32 tolerance
/// the general grid does.
#[test]
#[ignore = "requires GPU"]
fn gpu_bsdf_matches_cpu_reference_at_retro_angles() {
    let Some(ctx) = ctx_or_skip("gpu_bsdf_matches_cpu_reference_at_retro_angles") else {
        return;
    };

    let mut inputs = Vec::new();
    for mut m in test_materials() {
        // The reported material: matte, uncoated, roughness 0.85. The rest of
        // `test_materials` covers the smooth and metallic ends, where the
        // specular lobe at `n·h = 1` is at its narrowest.
        for rough in [m.roughness, 0.85] {
            m.roughness = rough;
            for k in 0..14u32 {
                // (a) The view ray on the normal, the light swept off it by up
                //     to ~18 degrees: `n·h` runs from 1 down through the band
                //     where the reported ring sits.
                let a = k as f32 * 0.023;
                let wo = [0.0, 0.0, 1.0];
                let wi = unit([a.sin(), 0.0, a.cos()]);
                inputs.push(ParityInput {
                    material: m,
                    wo: [wo[0], wo[1], wo[2], 0.0],
                    wi: [wi[0], wi[1], wi[2], 0.0],
                    rnd: [0.3, 0.4, 0.5, 0.0],
                });
                // (b) Exact retro, `wo == wi`, swept off the normal: the
                //     half-vector is the direction itself and `o·h` is 1.
                let b = k as f32 * 0.05;
                let w = unit([b.sin(), 0.0, b.cos()]);
                inputs.push(ParityInput {
                    material: m,
                    wo: [w[0], w[1], w[2], 0.0],
                    wi: [w[0], w[1], w[2], 0.0],
                    rnd: [0.3, 0.4, 0.5, 0.0],
                });
            }
        }
    }

    let outputs = run_harness(ctx, &inputs);
    let mut worst_value = 0.0f32;
    let mut worst_pdf = 0.0f32;
    for (i, (inp, out)) in inputs.iter().zip(&outputs).enumerate() {
        let pbr: Pbr = inp.material.to_pbr();
        let wo = Vec3::new(inp.wo[0] as f64, inp.wo[1] as f64, inp.wo[2] as f64);
        let wi = Vec3::new(inp.wi[0] as f64, inp.wi[1] as f64, inp.wi[2] as f64);
        let (ref_value, ref_pdf) = reference_bsdf_eval(&pbr, wo, wi);

        for (c, &rv) in ref_value.iter().enumerate() {
            let d = (out.eval[c] - rv).abs();
            worst_value = worst_value.max(d);
            assert!(
                d <= 2e-4 * rv.abs().max(1.0),
                "retro input {i} (wo {:?}, wi {:?}, roughness {}): BSDF value \
                 channel {c} is {} on the GPU against {rv} on the CPU. A \
                 degeneracy at n·h → 1 shows up here and nowhere else in the \
                 grid.",
                inp.wo,
                inp.wi,
                inp.material.roughness,
                out.eval[c],
            );
        }
        let dp = (out.eval[3] - ref_pdf).abs();
        worst_pdf = worst_pdf.max(dp);
        assert!(
            dp <= 2e-4 * ref_pdf.abs().max(1.0),
            "retro input {i}: PDF is {} on the GPU against {ref_pdf} on the \
             CPU. MIS is computed from this, so a disagreement here is an \
             energy error concentrated exactly at normal incidence.",
            out.eval[3],
        );
        assert!(
            out.eval[0].is_finite() && out.eval[3].is_finite(),
            "retro input {i} produced a non-finite value or PDF: {:?}",
            out.eval,
        );
    }
    eprintln!("retro: worst value delta {worst_value:e}, worst pdf delta {worst_pdf:e}");

    let non_zero = outputs.iter().filter(|o| o.eval[3] > 0.0).count();
    assert_eq!(
        non_zero,
        inputs.len(),
        "every retro input is inside the hemisphere and must have a positive \
         PDF; {} did not",
        inputs.len() - non_zero,
    );
}
