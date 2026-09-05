//! `GpuScene::placed` — does a packed solid land where the CPU renderer puts
//! the same solid under the same `Object::placed` transform?
//!
//! The GPU tracer has no per-instance transform: `placed` moves the packed
//! surface frames and AABBs instead, so a scene of instances is still one BVH
//! walk in world space. That is only sound if moving the frames is exactly
//! moving the geometry, which is what these tests check — first against the
//! camera (moving the scene one way is moving the eye the other), then
//! against the CPU path tracer's own silhouette.
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test placed_instances -- --ignored
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use std::sync::Arc;

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_math::{Point3, Transform, Vec3};
use vcad_kernel_primitives::{make_cube, make_sphere};
use vcad_kernel_raytrace::gpu::{GpuCamera, GpuRenderState, GpuScene};
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

/// A render state with nothing in it but the path trace: no edge overlay, no
/// stylisation, and no implicit ground plane. The ground in particular has to
/// go — it sits at world z = 0 and does *not* move with a placed instance, so
/// leaving it on would compare a moved object against an unmoved floor and its
/// shadow.
fn photoreal_state() -> GpuRenderState {
    let mut s = GpuRenderState::new(1);
    s.enable_edges = 0;
    s.stylize = 0;
    s.ground_enabled = 0;
    s
}

fn gpu_render_with(
    ctx: &GpuContext,
    scene: &GpuScene,
    eye: [f32; 3],
    at: [f32; 3],
    tweak: impl FnOnce(&mut GpuRenderState),
) -> Vec<u8> {
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline creation");
    let camera = GpuCamera::new(eye, at, [0.0, 0.0, 1.0], 45.0_f32.to_radians(), W, H);
    let mut state = photoreal_state();
    tweak(&mut state);
    let (pixels, _accum) = pollster::block_on(
        pipeline.render_with_render_state(ctx, scene, &camera, W, H, None, state),
    )
    .expect("render");
    pixels
}

fn gpu_render(ctx: &GpuContext, scene: &GpuScene, eye: [f32; 3], at: [f32; 3]) -> Vec<u8> {
    gpu_render_with(ctx, scene, eye, at, |_| {})
}

/// Mean absolute channel difference between two RGBA buffers, in 0..255.
fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let n = a.len() / 4 * 3;
    let sum: u64 = a
        .chunks(4)
        .zip(b.chunks(4))
        .map(|(p, q)| (0..3).map(|c| p[c].abs_diff(q[c]) as u64).sum::<u64>())
        .sum();
    sum as f64 / n as f64
}

/// The centroid and area of a boolean mask, for comparing two silhouettes
/// without comparing the shading inside them.
fn centroid(mask: &[bool]) -> (f64, f64, usize) {
    let (mut sx, mut sy, mut n) = (0.0, 0.0, 0usize);
    for (i, &hit) in mask.iter().enumerate() {
        if hit {
            sx += (i as u32 % W) as f64;
            sy += (i as u32 / W) as f64;
            n += 1;
        }
    }
    assert!(n > 0, "the mask is empty — nothing was rendered at all");
    (sx / n as f64, sy / n as f64, n)
}

/// Moving the scene by `t` and moving the eye by `t` are the same picture.
///
/// This is the property `placed` exists to have, checked without the CPU
/// renderer in it at all: a translated solid seen from a translated eye must
/// land on exactly the same pixels as the original seen from the original eye.
/// Any slot of a packed surface that should have been carried as a point and
/// was not — a plane's origin, a face AABB, a BVH node — moves the geometry
/// somewhere else and shows up here.
///
/// The comparison is the flat debug silhouette rather than the shaded render:
/// the two are separate Monte Carlo estimates and their noise is not the same
/// noise, but where the geometry *is* has no noise in it.
#[test]
#[ignore = "requires GPU"]
fn a_placed_scene_is_the_scene_with_the_eye_moved_to_match() {
    let Some(ctx) = ctx_or_skip("a_placed_scene_is_the_scene_with_the_eye_moved_to_match") else {
        return;
    };

    // A cube (six planes) and a sphere, so both the plane slots and the
    // sphere slots are exercised.
    for solid in [make_cube(10.0, 10.0, 10.0), make_sphere(5.0, 32)] {
        let packed = GpuScene::from_brep(&solid).expect("scene packs");
        let (dx, dy, dz) = (17.0f32, -9.0, 4.0);
        let flat = |scene: &GpuScene, eye: [f32; 3], at: [f32; 3]| -> Vec<bool> {
            gpu_render_with(ctx, scene, eye, at, |s| s.debug_mode = 4)
                .chunks(4)
                .map(|p| p[0].max(p[1]) as i32 - p[2] as i32 > 40)
                .collect()
        };

        let here = flat(&packed, [24.0, 22.0, 20.0], [1.0, 1.0, 1.0]);
        let there = flat(
            &packed.placed(&Transform::translation(dx as f64, dy as f64, dz as f64)),
            [24.0 + dx, 22.0 + dy, 20.0 + dz],
            [1.0 + dx, 1.0 + dy, 1.0 + dz],
        );

        let differ = here.iter().zip(&there).filter(|(a, b)| a != b).count();
        let (_, _, n) = centroid(&here);
        assert!(
            differ == 0,
            "a translated scene seen from a translated eye disagrees with the \
             original on {differ} of its {n} pixels. Something in the packed \
             surface — an origin, a face AABB, a BVH node — did not move with \
             the rest.",
        );
    }
}

/// A sphere is its own rotation. Turning one about its centre must change
/// nothing on screen, however the packed axis and reference direction are
/// carried — and it is carrying those as *directions* rather than points that
/// this catches.
#[test]
#[ignore = "requires GPU"]
fn rotating_a_sphere_about_its_centre_changes_nothing() {
    let Some(ctx) = ctx_or_skip("rotating_a_sphere_about_its_centre_changes_nothing") else {
        return;
    };
    let mut packed = GpuScene::from_brep(&make_sphere(5.0, 32)).expect("scene packs");
    packed.lights.clear();
    let (eye, at) = ([20.0, 20.0, 20.0], [0.0, 0.0, 0.0]);

    let still = gpu_render(ctx, &packed, eye, at);
    for angle in [0.4, 1.3, 2.7] {
        let turned = gpu_render(ctx, &packed.placed(&Transform::rotation_z(angle)), eye, at);
        let diff = mean_abs_diff(&still, &turned);
        assert!(
            diff < 2.0,
            "a sphere turned {angle} rad about z rendered {diff:.2}/255 a \
             channel away from the sphere at rest. A rotation applied to a \
             centre as if it were a direction (or the other way round) moves \
             the sphere off its own axis.",
        );
    }
}

/// The one that matters: the GPU's placed instance and the CPU renderer's
/// `Object::placed` must put the solid in the same place on screen.
///
/// The two renderers do not agree on shading — different environment model,
/// different sampling — so this compares *where the geometry is*, not what
/// colour it came out. The CPU side has a depth buffer, whose zero is the
/// background sentinel. The GPU side gets a silhouette by differencing the
/// render against the same scene with the instance pushed a kilometre away:
/// same camera, same sky, so every pixel that changed is the object.
use vcad_kernel_raytrace::{tlas::placement, BrepBvh};
#[test]
#[ignore = "requires GPU"]
fn a_placed_instance_lands_where_the_cpu_renderer_puts_it() {
    let Some(ctx) = ctx_or_skip("a_placed_instance_lands_where_the_cpu_renderer_puts_it") else {
        return;
    };

    let solid = make_cube(10.0, 6.0, 4.0);
    let to_world = Transform::translation(-3.0, 2.0, 8.0)
        .then(&Transform::rotation_z(0.6))
        .then(&Transform::rotation_x(0.25));
    let (eye, at) = ([40.0f32, 34.0, 30.0], [0.0f32, 0.0, 5.0]);

    // ---- the GPU's silhouette
    //
    // Debug mode 4 paints every face flat — green where it faces out, red
    // where it is reversed — and leaves the sky alone. That is a silhouette
    // with no Monte Carlo noise in it, which differencing two path-traced
    // frames is not: the sky the shader paints is bluish, so "the red or
    // green channel beats the blue one" is exactly the geometry.
    let packed = GpuScene::from_brep(&solid).expect("scene packs");
    let flat = gpu_render_with(ctx, &packed.placed(&to_world), eye, at, |s| {
        s.debug_mode = 4
    });
    let gpu_mask: Vec<bool> = flat
        .chunks(4)
        .map(|p| p[0].max(p[1]) as i32 - p[2] as i32 > 40)
        .collect();

    // ---- the CPU's, from the same placement
    let bvh = Arc::new(Bvh::build_brep(&solid));
    let scene = pathtrace::Scene {
        objects: vec![Object::placed(bvh, Pbr::default(), placement(&to_world))],
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
            spp: 4,
            max_depth: 2,
            denoise: false,
            ..Default::default()
        },
    );
    let cpu_mask: Vec<bool> = film.depth.iter().map(|&d| d > 0.0).collect();

    // ---- and they must be the same silhouette
    let (gx, gy, gn) = centroid(&gpu_mask);
    let (cx, cy, cn) = centroid(&cpu_mask);
    let both = gpu_mask
        .iter()
        .zip(&cpu_mask)
        .filter(|(a, b)| **a && **b)
        .count();
    let either = gpu_mask
        .iter()
        .zip(&cpu_mask)
        .filter(|(a, b)| **a || **b)
        .count();
    let iou = both as f64 / either as f64;

    assert!(
        (gx - cx).abs() < 1.0 && (gy - cy).abs() < 1.0,
        "the placed instance is at ({gx:.1}, {gy:.1}) on the GPU and \
         ({cx:.1}, {cy:.1}) on the CPU. `GpuScene::placed` and \
         `Object::placed` disagree about where the transform puts the solid.",
    );
    assert!(
        iou > 0.93,
        "the GPU silhouette ({gn} px) and the CPU silhouette ({cn} px) overlap \
         at IoU {iou:.3}. The two renderers agree on the centre of the placed \
         instance but not on its shape — check the rotation of the surface \
         frames, not the translation.",
    );
}
