//! Does the compute shader's torus have the same silhouette as the CPU
//! integrator's?
//!
//! The torus is the only quartic surface in the tracer, and Ferrari's method
//! is the least forgiving thing in either renderer: it goes through a
//! resolvent cubic whose roots are differences of large numbers, and the GPU
//! does it all in f32. A wrong root selection does not miss the torus — it
//! draws a *bigger* one, filling the hole with spurious hits, which is exactly
//! the kind of error a "does it render anything" smoke test sails past.
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test torus_silhouette -- --ignored
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use std::sync::Arc;

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_math::{Point3, Transform, Vec3};
use vcad_kernel_primitives::make_torus;
use vcad_kernel_raytrace::gpu::{GpuCamera, GpuRenderState, GpuScene, RayTracePipeline};
use vcad_kernel_raytrace::pathtrace::{self, Camera, Environment, Object, PathTraceOptions, Pbr};
use vcad_kernel_raytrace::Bvh;

const W: u32 = 96;
const H: u32 = 96;

fn ctx_or_skip(test: &str) -> Option<&'static GpuContext> {
    match pollster::block_on(GpuContext::init()) {
        Ok(ctx) => Some(ctx),
        Err(GpuError::NoAdapter) => {
            eprintln!("[{test}] skipped: no compatible GPU adapter");
            None
        }
        Err(e) => panic!("GPU init failed unexpectedly: {e}"),
    }
}

/// The GPU's silhouette, from the flat debug pass: every face painted flat,
/// the sky left alone, and no Monte Carlo noise in it.
fn gpu_mask(ctx: &GpuContext, scene: &GpuScene, eye: [f32; 3], at: [f32; 3]) -> Vec<bool> {
    let pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");
    let camera = GpuCamera::new(eye, at, [0.0, 0.0, 1.0], 45.0_f32.to_radians(), W, H);
    let mut state = GpuRenderState::new(1);
    state.enable_edges = 0;
    state.stylize = 0;
    state.ground_enabled = 0;
    state.debug_mode = 4;
    // Centre of the pixel, not a jittered sample: this is a geometry
    // comparison, and half a pixel of jitter on each side would show up as
    // disagreement all the way round a thin hoop's outline.
    state.jitter_x = 0.0;
    state.jitter_y = 0.0;
    let (px, _) = pollster::block_on(
        pipeline.render_with_render_state(ctx, scene, &camera, W, H, None, state),
    )
    .expect("render");
    px.chunks(4)
        .map(|p| p[0].max(p[1]) as i32 - p[2] as i32 > 40)
        .collect()
}

/// The CPU's, from the depth buffer, whose zero is the background sentinel.
/// The CPU's, as fractional coverage. `render` jitters within the pixel, so at
/// enough samples `alpha` is the fraction of the pixel the solid covers —
/// which lets the comparison say which pixels are unambiguously inside or
/// outside and leave the genuinely half-covered boundary out of it.
fn cpu_coverage(
    solid_bvh: &Arc<Bvh>,
    to_world: &Transform,
    eye: [f32; 3],
    at: [f32; 3],
) -> Vec<f32> {
    let scene = pathtrace::Scene {
        objects: vec![Object::placed(
            Arc::clone(solid_bvh),
            Pbr::default(),
            placement(&to_world),
        )],
        lights: Vec::new(),
        env: Environment::default(),
        ground: None,
    };
    let cam = Camera::look_at(
        Point3::new(eye[0] as f64, eye[1] as f64, eye[2] as f64),
        Point3::new(at[0] as f64, at[1] as f64, at[2] as f64),
        Vec3::new(0.0, 0.0, 1.0),
        45.0,
    );
    let film = pathtrace::render(
        &scene,
        &cam,
        W,
        H,
        &PathTraceOptions {
            spp: 32,
            max_depth: 1,
            denoise: false,
            ..Default::default()
        },
    );
    film.alpha
}

/// Intersection over union, counting only the pixels the CPU is certain about:
/// fully covered or fully empty. A pixel the solid half-fills is a question
/// about the sampler, not about where the geometry is.
fn iou(gpu: &[bool], cov: &[f32]) -> (f64, usize, usize) {
    let (mut both, mut either) = (0usize, 0usize);
    for (g, c) in gpu.iter().zip(cov) {
        let inside = if *c > 0.999 {
            true
        } else if *c < 0.001 {
            false
        } else {
            continue;
        };
        if *g && inside {
            both += 1;
        }
        if *g || inside {
            either += 1;
        }
    }
    let na = gpu.iter().filter(|x| **x).count();
    let nb = cov.iter().filter(|c| **c > 0.5).count();
    assert!(either > 0, "neither renderer drew anything at all");
    (both as f64 / either as f64, na, nb)
}

/// The one that matters. A torus at several radii and orientations must have
/// the same outline on the GPU as on the CPU — including its hole.
use vcad_kernel_raytrace::{tlas::placement, BrepBvh};
#[test]
#[ignore = "requires GPU"]
fn the_gpu_torus_has_the_cpu_torus_silhouette() {
    let Some(ctx) = ctx_or_skip("the_gpu_torus_has_the_cpu_torus_silhouette") else {
        return;
    };

    // Radii from a fat ring (R barely above r) to a thin hoop, and poses that
    // put the tube edge-on, face-on, and obliquely to the camera. Ferrari's
    // root selection fails differently in each.
    let radii = [(6.0, 1.5), (6.0, 4.5), (10.0, 1.0), (3.0, 2.5)];
    let poses = [
        Transform::identity(),
        Transform::rotation_x(std::f64::consts::FRAC_PI_2),
        Transform::rotation_x(0.7).then(&Transform::rotation_z(0.4)),
        Transform::translation(2.0, -3.0, 1.5).then(&Transform::rotation_y(1.1)),
    ];
    let (eye, at) = ([26.0f32, 22.0, 18.0], [0.0f32, 0.0, 0.0]);

    let mut worst = 1.0f64;
    let mut report = String::new();
    for (major, minor) in radii {
        let solid = make_torus(major, minor, 64);
        let packed = GpuScene::from_brep(&solid).expect("scene packs");
        let bvh = Arc::new(Bvh::build_brep(&solid));
        for (pi, pose) in poses.iter().enumerate() {
            let g = gpu_mask(ctx, &packed.placed(pose), eye, at);
            let c = cpu_coverage(&bvh, pose, eye, at);
            let (i, ng, nc) = iou(&g, &c);
            report.push_str(&format!(
                "  R={major} r={minor} pose {pi}: IoU {i:.3} (GPU {ng} px, CPU {nc} px)\n"
            ));
            worst = worst.min(i);
        }
    }
    assert!(
        worst > 0.98,
        "the GPU and CPU tori disagree — worst IoU {worst:.3}:\n{report}\
         The quartic's root selection, or the trim that follows it, differs \
         between the two."
    );
}
