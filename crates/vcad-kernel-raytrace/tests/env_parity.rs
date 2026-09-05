//! The analytic environment, on both tiers.
//!
//! Kosm's court renders darker on the GPU than on the CPU — 46/255 against
//! 79/255 on the walls, with the same ten panels at radiance 18 and the same
//! constant environment, and with the GPU tracing *deeper*. This is the
//! difference: `pathtrace::Environment::Gradient` carries its own zenith,
//! horizon and ground radiances, and nothing ever sent them to the GPU. The
//! shader had the default studio gradient's three colours compiled in and
//! scaled them by `env_intensity`, so a scene lit by any other gradient — the
//! constant environment among them, which is a gradient whose three colours
//! are equal — was lit by a different sky on the two tiers.
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test env_parity -- --ignored --nocapture
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use std::sync::Arc;

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::make_cube;
use vcad_kernel_raytrace::gpu::{
    GpuAreaLight, GpuCamera, GpuRenderState, GpuScene, RayTracePipeline,
};
use vcad_kernel_raytrace::pathtrace::{
    self, AreaLight, Camera, Environment, GradientEnv, Object, PathTraceOptions, Pbr,
};
use vcad_kernel_raytrace::Bvh;

const N: u32 = 64;
const PASSES: u32 = 900;
const MAX_DEPTH: u32 = 3;
const RR_START: u32 = 3;

/// A 200 x 200 slab standing in for a court's floor, with the camera above it
/// and a panel to one side. Open above, so bounce rays reach the environment
/// and the environment term is actually part of the picture — a sealed room
/// would test nothing about it.
const SX: f64 = 200.0;
const EYE: [f32; 3] = [100.0, 40.0, 70.0];
const AT: [f32; 3] = [100.0, 100.0, 4.0];
const FOV: f32 = 45.0;

const LC: [f64; 3] = [140.0, 90.0, 55.0];
const LU: [f64; 3] = [16.0, 0.0, 0.0];
const LV: [f64; 3] = [0.0, -16.0, 0.0];
// Deliberately modest against the environment below: the court's walls are
// mostly sky-lit, and a scene where the panel carries the image would hide the
// very term this is about.
const LE: f32 = 4.0;

/// The environment under test: constant radiance, which is the case Kosm's
/// court uses and the one furthest from the gradient the shader assumed.
const ENV: [f32; 3] = [1.0, 1.0, 1.0];

const BASE: [f32; 3] = [0.62, 0.6, 0.58];
const ROUGH: f32 = 0.85;

fn ctx_or_skip(test: &str) -> Option<&'static GpuContext> {
    match pollster::block_on(GpuContext::init()) {
        Ok(c) => Some(c),
        Err(GpuError::NoAdapter) => {
            eprintln!("[{test}] skipped: no compatible GPU adapter");
            None
        }
        Err(e) => panic!("GPU init failed unexpectedly: {e}"),
    }
}

fn material() -> Pbr {
    Pbr {
        base_color: BASE,
        metallic: 0.0,
        roughness: ROUGH,
        clearcoat: 0.0,
        ..Pbr::default()
    }
}

fn gpu_scene() -> GpuScene {
    let m = material();
    let mut s = GpuScene::from_brep(&make_cube(SX, SX, 4.0)).expect("scene packs");
    for g in &mut s.materials {
        g.color = [m.base_color[0], m.base_color[1], m.base_color[2], 1.0];
        g.metallic = 0.0;
        g.roughness = ROUGH;
        g.clearcoat = 0.0;
        g.anisotropy = 0.0;
    }
    s.lights = vec![GpuAreaLight {
        center: [LC[0] as f32, LC[1] as f32, LC[2] as f32, 0.0],
        u: [LU[0] as f32, LU[1] as f32, LU[2] as f32, 0.0],
        v: [LV[0] as f32, LV[1] as f32, LV[2] as f32, 0.0],
        emission: [LE, LE, LE, 0.0],
    }];
    s
}

fn cpu_scene() -> pathtrace::Scene {
    pathtrace::Scene {
        objects: vec![Object::new(
            Arc::new(Bvh::build_brep(&make_cube(SX, SX, 4.0))),
            material(),
        )],
        lights: vec![AreaLight {
            center: Point3::new(LC[0], LC[1], LC[2]),
            u: Vec3::new(LU[0], LU[1], LU[2]),
            v: Vec3::new(LV[0], LV[1], LV[2]),
            emission: [LE; 3],
        }],
        env: Environment::constant(ENV),
        ground: None,
    }
}

fn gradient() -> GradientEnv {
    GradientEnv {
        zenith: ENV,
        horizon: ENV,
        ground: ENV,
        intensity: 1.0,
    }
}

fn state(frame: u32, send_gradient: bool) -> GpuRenderState {
    let mut s = GpuRenderState::new(frame);
    s.enable_edges = 0;
    s.stylize = 0;
    s.ground_enabled = 0;
    s.max_depth = MAX_DEPTH;
    s.rr_start = RR_START;
    if send_gradient {
        s.set_gradient_env(&gradient());
    } else {
        // What a caller could say before the gradient was transportable: an
        // overall multiplier on a sky whose three colours the shader chose.
        s.env_intensity = 1.0;
    }
    s
}

/// Mean linear radiance over the pixels both tiers found geometry at.
fn means(ctx: &GpuContext, send_gradient: bool) -> (f64, f64) {
    let pipeline = RayTracePipeline::new(ctx).expect("pipeline");
    let sc = gpu_scene();
    let mut res = pipeline.resident_scene(ctx, &sc, N, N);
    let cam = GpuCamera::new(EYE, AT, [0.0, 0.0, 1.0], FOV.to_radians(), N, N);
    let n = (N * N) as usize;

    let mut sum = vec![0.0f64; n * 3];
    let mut depth = vec![0.0f32; n];
    for f in 1..=PASSES {
        let film = pollster::block_on(pipeline.render_resident_linear(
            ctx,
            &mut res,
            &cam,
            state(f, send_gradient),
        ))
        .expect("linear render");
        for (s, c) in sum.iter_mut().zip(&film.rgb) {
            *s += *c as f64;
        }
        if f == 1 {
            depth.copy_from_slice(&film.depth);
        }
    }

    let cpu = pathtrace::render(
        &cpu_scene(),
        &Camera::look_at(
            Point3::new(EYE[0] as f64, EYE[1] as f64, EYE[2] as f64),
            Point3::new(AT[0] as f64, AT[1] as f64, AT[2] as f64),
            Vec3::new(0.0, 0.0, 1.0),
            FOV as f64,
        ),
        N,
        N,
        &PathTraceOptions {
            spp: PASSES,
            max_depth: MAX_DEPTH,
            rr_start: RR_START,
            denoise: false,
            show_background: false,
            ..Default::default()
        },
    );

    let (mut g, mut c, mut k) = (0.0f64, 0.0f64, 0usize);
    for i in 0..n {
        if depth[i] <= 0.0 || cpu.depth[i] <= 0.0 {
            continue;
        }
        for j in 0..3 {
            g += sum[i * 3 + j] / PASSES as f64;
            c += cpu.rgb[i * 3 + j] as f64;
        }
        k += 3;
    }
    assert!(k > 3000, "only {k} channels hit geometry on both tiers");
    (g / k as f64, c / k as f64)
}

/// The converged GPU image must match the converged CPU image to 2%.
///
/// The two tiers draw their samples from different sequences — Halton and
/// per-pixel PCG — so this is a claim about expectations, not about frames.
/// At 900 samples over three thousand-odd channels the residual noise on the
/// mean is a small fraction of a percent, and the failure this is watching for
/// is a *factor*: a sky the two tiers disagree about is worth tens of percent,
/// not two.
#[test]
#[ignore = "requires GPU"]
fn the_gpu_matches_the_cpu_under_a_transported_gradient() {
    let Some(ctx) = ctx_or_skip("the_gpu_matches_the_cpu_under_a_transported_gradient") else {
        return;
    };

    // What the shader used to have no way of being told. Kept as a
    // measurement, not an assertion — it is the size of the bug, and it
    // depends on how far the caller's gradient is from the studio default.
    let (g_old, c) = means(ctx, false);
    eprintln!(
        "[before] GPU {g_old:.5} vs CPU {c:.5} — GPU is {:.1}% of the CPU",
        100.0 * g_old / c,
    );

    let (g, c) = means(ctx, true);
    let rel = (g - c).abs() / c;
    eprintln!(
        "[after]  GPU {g:.5} vs CPU {c:.5} (rel {:.3}%)",
        rel * 100.0
    );
    assert!(c > 1e-3, "the CPU reference is black ({c:.6})");
    assert!(
        rel < 0.02,
        "the converged GPU mean is {g:.5} against the CPU's {c:.5}, {:.1}% \
         apart. Both tiers were handed the same constant environment, the same \
         panel and the same path depth, so the sky each is integrating must be \
         the same one.",
        rel * 100.0,
    );
}

/// The default state still describes the studio gradient the shader used to
/// have compiled in, so a caller that never mentions the environment renders
/// exactly what it rendered before.
use vcad_kernel_raytrace::{BrepBvh};
#[test]
fn the_default_render_state_carries_the_default_gradient() {
    let s = GpuRenderState::new(1);
    let d = GradientEnv::default();
    assert_eq!(&s.env_zenith[..3], &d.zenith[..]);
    assert_eq!(&s.env_horizon[..3], &d.horizon[..]);
    assert_eq!(&s.env_ground[..3], &d.ground[..]);
    assert_eq!(s.env_intensity, d.intensity);
}
