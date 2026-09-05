//! Rays leaving a surface at a large coordinate must clear it.
//!
//! A path tracer is scale-free: multiply every length in a scene by k and the
//! radiance at every corresponding point is unchanged. An f32 renderer is only
//! scale-free if its self-intersection epsilon is, and this shader's was not —
//! it lifted secondary and shadow rays by a fixed `1e-4` and rejected self-hits
//! below a fixed `t > 1e-6`. Model the same room in millimetres instead of
//! metres and both vanish under one ulp, so every shadow ray restarts on the
//! wall it left and the wall shadows itself.
//!
//! Two pins, one cheap and one on the device:
//!
//! * the arithmetic — a fixed `1e-4` offset is a no-op at a coordinate of 1e4,
//!   and the shader's `ray_eps` is not;
//! * the render — the same sealed room at 1x and at 1000x must agree, GPU
//!   against GPU, with no CPU tier in the comparison.
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test large_coordinate_eps \
//!     -- --ignored --nocapture --test-threads=1
//! ```

/// Mirrors `RAY_EPS_ABS` / `RAY_EPS_REL` in `raytrace.wgsl`.
const RAY_EPS_ABS: f32 = 1e-4;
const RAY_EPS_REL: f32 = 2e-6;

fn ray_eps(p: [f32; 3]) -> f32 {
    let scale = p[0].abs().max(p[1].abs()).max(p[2].abs());
    RAY_EPS_ABS.max(scale * RAY_EPS_REL)
}

/// The bug, in three lines of f32: at a court-sized coordinate the old fixed
/// offset does not move the point at all, so the "offset" origin still lies
/// exactly on the surface.
#[test]
fn a_shadow_ray_from_a_wall_at_1e4_clears_the_wall() {
    let p: f32 = 1e4;

    // What the shader used to do.
    assert_eq!(
        p + RAY_EPS_ABS,
        p,
        "a fixed 1e-4 offset should be lost at 1e4 — if it is not, f32 has \
         changed under this test and the premise needs revisiting"
    );

    // What it does now: the offset survives, and lands within a couple of
    // dozen ulps rather than kilometres away.
    let eps = ray_eps([p, 0.0, 0.0]);
    let moved = p + eps;
    assert!(
        moved > p,
        "the scale-aware offset must separate {p} from the surface it left"
    );
    let ulp = f32::from_bits(p.to_bits() + 1) - p;
    assert!(
        moved - p <= 32.0 * ulp,
        "offset {} is {} ulps at {p}: far enough is not the same as far",
        moved - p,
        (moved - p) / ulp,
    );

    // And the near-origin behaviour the small-unit tests pin is untouched.
    assert_eq!(ray_eps([1.0, 0.5, 0.0]), RAY_EPS_ABS);
    assert_eq!(ray_eps([-30.0, 0.0, 0.0]), RAY_EPS_ABS);
}

#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
mod device {
    use vcad_kernel_gpu::{GpuContext, GpuError};
    use vcad_kernel_math::{Point3, Vec3};
    use vcad_kernel_primitives::make_cube;
    use vcad_kernel_raytrace::gpu::{
        GpuAreaLight, GpuCamera, GpuRenderState, GpuScene, RayTracePipeline,
    };
    use vcad_kernel_raytrace::pathtrace::{AreaLight, GradientEnv};

    const N: u32 = 96;
    const PASSES: u32 = 24;

    /// The room, in its own units. Instantiated at k = 1 and k = 1000.
    fn scene(k: f64) -> GpuScene {
        let (rx, ry, rz) = (26.0 * k, 17.0 * k, 9.0 * k);
        let mut s = GpuScene::from_brep(&make_cube(rx, ry, rz)).expect("scene packs");
        for g in &mut s.materials {
            g.color = [0.62, 0.6, 0.58, 1.0];
            g.metallic = 0.0;
            g.roughness = 0.85;
            g.clearcoat = 0.0;
            g.anisotropy = 0.0;
        }
        let mut lights = Vec::new();
        for row in 0..5 {
            for col in 0..2 {
                lights.push(AreaLight {
                    center: Point3::new(
                        rx * (0.3 + 0.4 * col as f64),
                        ry * (0.12 + 0.19 * row as f64),
                        rz - 0.5 * k,
                    ),
                    u: Vec3::new(k, 0.0, 0.0),
                    v: Vec3::new(0.0, -k, 0.0),
                    emission: [18.0; 3],
                });
            }
        }
        s.lights = lights.iter().map(GpuAreaLight::from_area_light).collect();
        s
    }

    fn camera(k: f64) -> GpuCamera {
        let f = k as f32;
        GpuCamera::new(
            [13.0 * f, 8.5 * f, 1.7 * f],
            [13.0 * f, 17.0 * f, 1.7 * f],
            [0.0, 0.0, 1.0],
            40f32.to_radians(),
            N,
            N,
        )
    }

    fn state(frame: u32) -> GpuRenderState {
        let mut s = GpuRenderState::new(frame);
        s.enable_edges = 0;
        s.stylize = 0;
        s.ground_enabled = 0;
        s.max_depth = 6;
        s.rr_start = 3;
        s.firefly_clamp = 12.0;
        s.set_camera_visible_lights(true);
        s.set_gradient_env(&GradientEnv {
            zenith: [0.0; 3],
            horizon: [0.0; 3],
            ground: [0.0; 3],
            intensity: 1.0,
        });
        s
    }

    fn mean_luma(ctx: &'static GpuContext, pipeline: &RayTracePipeline, k: f64) -> f64 {
        let sc = scene(k);
        let mut res = pipeline.resident_scene(ctx, &sc, N, N);
        let cam = camera(k);
        let mut sum = 0.0f64;
        for f in 1..=PASSES {
            let film =
                pollster::block_on(pipeline.render_resident_linear(ctx, &mut res, &cam, state(f)))
                    .expect("linear render");
            for i in 0..(N * N) as usize {
                sum += 0.2126 * film.rgb[i * 3] as f64
                    + 0.7152 * film.rgb[i * 3 + 1] as f64
                    + 0.0722 * film.rgb[i * 3 + 2] as f64;
            }
        }
        sum / (PASSES as f64 * (N * N) as f64)
    }

    /// Radiance does not depend on the unit the room is authored in. It did:
    /// the millimetre room came back 21% dark, because every shadow ray was
    /// starting inside the wall it left.
    #[test]
    #[ignore = "requires GPU"]
    fn the_same_room_in_millimetres_is_not_darker_than_in_metres() {
        let ctx = match pollster::block_on(GpuContext::init()) {
            Ok(c) => c,
            Err(GpuError::NoAdapter) => {
                eprintln!("skipped: no compatible GPU adapter");
                return;
            }
            Err(e) => panic!("GPU init failed unexpectedly: {e}"),
        };
        let pipeline = RayTracePipeline::new(ctx).expect("pipeline");
        let metres = mean_luma(ctx, &pipeline, 1.0);
        let millimetres = mean_luma(ctx, &pipeline, 1000.0);
        eprintln!(
            "mean luma: 1x {metres:.5}  1000x {millimetres:.5}  ({:.2}%)",
            100.0 * millimetres / metres
        );
        let rel = (millimetres - metres).abs() / metres;
        assert!(
            rel < 0.015,
            "the room reads {millimetres:.5} at 1000x against {metres:.5} at 1x, \
             {:.1}% apart — the shader's self-intersection epsilon is not \
             scale-free",
            rel * 100.0,
        );
    }
}
