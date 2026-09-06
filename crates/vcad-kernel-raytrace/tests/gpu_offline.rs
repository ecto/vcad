//! Offline GPU render: persistent buffers, N-spp accumulation, one HDR
//! readback.
//!
//! `RayTracePipeline::render_offline` is the entry point `vcad-render` will
//! eventually sit on. The viewport's `render_with_render_state` cannot serve
//! that role: it rebuilds every scene buffer and reads back a tonemapped
//! `Rgba8Unorm` texture on *every* call, which at 512 spp means 512 scene
//! uploads and 512 GPU->CPU round trips.
//!
//! These tests need a real adapter and are `#[ignore]`-tagged like
//! `gpu_smoke.rs`. Run locally with:
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test gpu_offline -- --ignored --nocapture
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use std::sync::Arc;

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::make_sphere;
use vcad_kernel_raytrace::gpu::{GpuCamera, GpuScene, OfflineOptions};
use vcad_kernel_raytrace::pathtrace::{
    self, Camera, Environment, Object, PathTraceOptions, Pbr, Scene,
};
use vcad_kernel_raytrace::{BrepBvh, Bvh};

/// Skip with a clear message when no adapter is available.
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

/// Sphere radius, and the camera distance that makes it overfill the frame.
const R: f64 = 5.0;
const EYE_Z: f64 = 14.0;
/// Vertical FOV. The sphere's silhouette half-angle is asin(5/14) = 20.9deg;
/// a 24deg vertical FOV puts the frame *corner* at 16.7deg, comfortably
/// inside it. Every pixel therefore hits the sphere, which is what lets the
/// CPU/GPU comparison below use a whole-image mean: the two renderers agree
/// on lighting but deliberately differ on the *visible backdrop* (the GPU's
/// `sky_color` is a themed UI choice, `env_radiance` is the shared one), so a
/// frame with visible background would compare two different things.
const FOV_DEG: f64 = 24.0;

/// The GPU's default material, spelled as a CPU `Pbr`. `GpuMaterial::default`
/// is 0.7 grey at roughness 0.5, which is *not* `Pbr::default`.
fn gpu_default_material() -> Pbr {
    Pbr {
        base_color: [0.7, 0.7, 0.7],
        metallic: 0.0,
        roughness: 0.5,
        anisotropy: 0.0,
        clearcoat: 0.0,
        clearcoat_roughness: 0.1,
        ior: 1.5,
        emissive: [0.0; 3],
        // The rest of kosm-render's principled parameters keep their defaults,
        // which are the values that reproduce the model this test was written
        // against.
        ..Pbr::default()
    }
}

/// The CPU counterpart of the scene `GpuScene::from_brep` builds: same solid,
/// same material, same studio rig derived from the same BVH root bounds, same
/// gradient environment, no ground.
fn cpu_scene(bvh: Arc<Bvh>) -> Scene {
    let root = bvh.root().expect("sphere BVH has a root");
    let aabb = match root {
        vcad_kernel_raytrace::bvh::BvhNode::Leaf { aabb, .. }
        | vcad_kernel_raytrace::bvh::BvhNode::Internal { aabb, .. } => *aabb,
    };
    let center = Point3::new(
        (aabb.min.x + aabb.max.x) * 0.5,
        (aabb.min.y + aabb.max.y) * 0.5,
        (aabb.min.z + aabb.max.z) * 0.5,
    );
    let radius = 0.5
        * ((aabb.max.x - aabb.min.x).powi(2)
            + (aabb.max.y - aabb.min.y).powi(2)
            + (aabb.max.z - aabb.min.z).powi(2))
        .sqrt();

    Scene {
        objects: vec![Object::new(bvh, gpu_default_material())],
        lights: pathtrace::studio_rig(center, radius),
        env: Environment::default(),
        ground: None,
        sun: None,
        splats: None,
    }
}

fn gpu_camera(w: u32, h: u32) -> GpuCamera {
    GpuCamera::new(
        [0.0, 0.0, EYE_Z as f32],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        (FOV_DEG as f32).to_radians(),
        w,
        h,
    )
}

fn cpu_camera() -> Camera {
    Camera::look_at(
        Point3::new(0.0, 0.0, EYE_Z),
        Point3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        FOV_DEG,
    )
}

fn offline_opts(w: u32, h: u32, spp: u32) -> OfflineOptions {
    OfflineOptions {
        width: w,
        height: h,
        spp,
        ground_enabled: false,
        seed: 7,
        ..Default::default()
    }
}

/// The core test: a 64-spp offline render must return finite, non-negative,
/// non-constant HDR radiance whose mean luminance lands near an equivalent
/// CPU render.
#[test]
#[ignore = "requires GPU"]
fn offline_hdr_matches_cpu_mean_luminance() {
    let Some(ctx) = ctx_or_skip("offline_hdr_matches_cpu_mean_luminance") else {
        return;
    };
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline creation");

    let sphere = make_sphere(R, 32);
    let scene = GpuScene::from_brep(&sphere).expect("scene builds");

    let (w, h) = (64u32, 64u32);
    let spp = 64;
    let out = pipeline
        .render_offline(ctx, &scene, &gpu_camera(w, h), &offline_opts(w, h, spp))
        .expect("offline render");

    assert_eq!(out.width, w);
    assert_eq!(out.height, h);
    assert_eq!(out.spp, spp);
    assert_eq!(
        out.rgba.len() as u32,
        w * h * 4,
        "HDR buffer is the wrong size"
    );

    // The readback is raw f32 from a compute shader: NaN or a negative
    // radiance means the integrator is broken, and no tonemap will save it.
    assert!(
        out.rgba.iter().all(|v| v.is_finite()),
        "HDR buffer contains NaN or infinity"
    );
    assert!(
        out.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[..3].iter().all(|&c| c >= 0.0)),
        "HDR buffer contains negative radiance"
    );

    let mean = out.mean_luminance();
    assert!(
        mean > 1e-4,
        "offline render is black (mean luminance {mean}) -- the dispatch \
         loop ran but wrote nothing to the accumulation buffer"
    );

    // A constant image would mean the accumulation buffer was never actually
    // shaded (e.g. every dispatch no-op'd and we read back a cleared buffer).
    let lum: Vec<f32> = out
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2])
        .collect();
    let (lo, hi) = lum
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    assert!(
        hi - lo > 1e-3,
        "offline render is a constant image (luminance {lo}..{hi})"
    );

    // CPU reference over the same scene, camera and sample count.
    let bvh = Arc::new(Bvh::build_brep(&sphere));
    let cpu = pathtrace::render(
        &cpu_scene(bvh),
        &cpu_camera(),
        w,
        h,
        &PathTraceOptions {
            spp,
            denoise: false,
            show_background: true,
            seed: 7,
            ..Default::default()
        },
    );

    // Every pixel must be on the sphere, or the two renderers are being
    // compared over different backdrops and the mean is meaningless.
    let covered = cpu.depth.iter().filter(|&&d| d > 0.0).count();
    let total = (w * h) as usize;
    assert!(
        covered * 100 >= total * 98,
        "the subject does not fill the frame ({covered}/{total} pixels hit) -- \
         the CPU/GPU mean-luminance comparison assumes it does"
    );

    let cpu_mean = cpu
        .rgb
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]) as f64)
        .sum::<f64>()
        / total as f64;
    let ratio = mean as f64 / cpu_mean;

    // Deliberately loose. The two integrators share a BSDF and an
    // environment but not their sampling: different RNG, different MIS
    // bookkeeping, f32 on the GPU against f64 on the CPU. A factor-of-two
    // band still catches every failure that matters -- a dead light rig, a
    // dropped bounce, a tonemap sneaking into the accumulation buffer -- and
    // does not fire on Monte Carlo disagreement.
    assert!(
        (0.5..=2.0).contains(&ratio),
        "GPU mean luminance {mean} vs CPU {cpu_mean} (ratio {ratio:.3}) -- \
         these should agree to within a factor of two; a large gap means the \
         two paths disagree about lighting, not about noise"
    );
}

/// The same seed must give the same image, twice. Without this the offline
/// path is unusable for regression baselines.
#[test]
#[ignore = "requires GPU"]
fn offline_render_is_deterministic_for_a_fixed_seed() {
    let Some(ctx) = ctx_or_skip("offline_render_is_deterministic_for_a_fixed_seed") else {
        return;
    };
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline creation");
    let sphere = make_sphere(R, 32);
    let scene = GpuScene::from_brep(&sphere).expect("scene builds");

    let (w, h) = (32u32, 32u32);
    let opts = offline_opts(w, h, 8);
    let cam = gpu_camera(w, h);

    let a = pipeline
        .render_offline(ctx, &scene, &cam, &opts)
        .expect("render a");
    let b = pipeline
        .render_offline(ctx, &scene, &cam, &opts)
        .expect("render b");
    assert_eq!(a.rgba, b.rgba, "same seed produced a different image");

    // ...and a different seed must actually change the noise, or the seed
    // plumbing into RenderState is not reaching the shader's RNG.
    let c = pipeline
        .render_offline(ctx, &scene, &cam, &OfflineOptions { seed: 99, ..opts })
        .expect("render c");
    assert_ne!(
        a.rgba, c.rgba,
        "changing the seed did not change the image -- render_state.seed is \
         not reaching rand_uniform"
    );
}

/// More samples must reduce the estimator's noise. This is the cheapest
/// end-to-end proof that the accumulation loop is actually *accumulating*
/// rather than overwriting the buffer each dispatch.
#[test]
#[ignore = "requires GPU"]
fn more_samples_reduce_noise() {
    let Some(ctx) = ctx_or_skip("more_samples_reduce_noise") else {
        return;
    };
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline creation");
    let sphere = make_sphere(R, 32);
    let scene = GpuScene::from_brep(&sphere).expect("scene builds");

    let (w, h) = (48u32, 48u32);
    let cam = gpu_camera(w, h);

    // Local neighbour-difference energy: a noisy image has large pixel-to-
    // pixel jumps on a smooth sphere, a converged one does not.
    let roughness = |img: &vcad_kernel_raytrace::gpu::OfflineResult| -> f64 {
        let mut acc = 0.0f64;
        for y in 0..h {
            for x in 1..w {
                let a = img.pixel(x, y);
                let b = img.pixel(x - 1, y);
                acc += (0..3).map(|c| (a[c] - b[c]).abs() as f64).sum::<f64>();
            }
        }
        acc / ((w - 1) * h) as f64
    };

    let low = pipeline
        .render_offline(ctx, &scene, &cam, &offline_opts(w, h, 1))
        .expect("1 spp");
    let high = pipeline
        .render_offline(ctx, &scene, &cam, &offline_opts(w, h, 64))
        .expect("64 spp");

    let (r_low, r_high) = (roughness(&low), roughness(&high));
    assert!(
        r_high < r_low * 0.8,
        "64 spp is no smoother than 1 spp ({r_high:.5} vs {r_low:.5}) -- the \
         sample loop is overwriting the accumulation buffer instead of \
         averaging into it"
    );
}

/// Measurement, not an assertion: per-spp cost of the persistent-buffer loop
/// against calling the viewport entry point in a loop, same scene and spp.
/// Run with `--ignored --nocapture` to see the numbers.
#[test]
#[ignore = "benchmark; requires GPU"]
fn bench_offline_vs_viewport_loop() {
    let Some(ctx) = ctx_or_skip("bench_offline_vs_viewport_loop") else {
        return;
    };
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline creation");
    let sphere = make_sphere(R, 32);
    let scene = GpuScene::from_brep(&sphere).expect("scene builds");

    let (w, h) = (512u32, 512u32);
    let spp = 128u32;
    let cam = gpu_camera(w, h);

    // Warm up shader/pipeline caches so the first timed run is not paying
    // for them.
    let _ = pipeline
        .render_offline(ctx, &scene, &cam, &offline_opts(w, h, 2))
        .expect("warmup");

    let t0 = std::time::Instant::now();
    let out = pipeline
        .render_offline(ctx, &scene, &cam, &offline_opts(w, h, spp))
        .expect("offline render");
    let offline = t0.elapsed();
    assert!(out.mean_luminance() > 0.0);

    let t1 = std::time::Instant::now();
    let mut accum = None;
    for frame in 1..=spp {
        let state = vcad_kernel_raytrace::gpu::GpuRenderState::new(frame);
        let (_pixels, buf) = pollster::block_on(
            pipeline.render_with_render_state(ctx, &scene, &cam, w, h, accum, state),
        )
        .expect("viewport render");
        accum = Some(buf);
    }
    let viewport = t1.elapsed();

    eprintln!(
        "\n{w}x{h} @ {spp} spp\n  \
         render_offline:              {:>9.3?}  ({:>8.3?} / spp)\n  \
         render_with_render_state x N: {:>9.3?}  ({:>8.3?} / spp)\n  \
         speedup: {:.2}x\n",
        offline,
        offline / spp,
        viewport,
        viewport / spp,
        viewport.as_secs_f64() / offline.as_secs_f64(),
    );
}
