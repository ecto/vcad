//! An analytic surface must be where the CPU says it is, however far from the
//! eye the scene puts it.
//!
//! Kosm's court is authored in millimetres, and `GpuScene::placed` bakes each
//! placement into the packed surface frame rather than carrying a per-instance
//! transform. A basketball parked at half court is therefore a sphere of
//! radius 120 and a seam torus of R 74, r 2 sitting at a world coordinate like
//! (-9990, -6600, 120), solved from a courtside eye ten or twenty metres away
//! — and solved in f32.
//!
//! The seam came back as a burr of black spikes around the ball. The cause is
//! not the world coordinate on its own: every intersector already subtracts
//! the surface's centre, and that subtraction is exact. It is the *ray
//! origin*, which stays where the eye is. Ferrari's quartic then forms its
//! coefficients out of `od ~ 1e4` and `oo ~ 1e8` terms, and its depressed
//! `p = b - 3a²/8` cancels two of them against each other, to resolve a tube
//! 2 mm across. In f32 there is nothing left. Measured against the CPU's f64
//! solve of the same quartic, a torus of R 74 / r 2 framed from ten metres
//! away came back with a mean |Δt| of 203 mm and a silhouette wrong in three
//! quarters of its pixels.
//!
//! The fix is `closest_approach` in `raytrace.wgsl`: re-origin the ray at its
//! nearest point to the surface before forming the polynomial, and add the
//! shift back afterwards. `t` is invariant under it — it is the same rigid
//! translation applied to ray and surface together — and every coefficient
//! becomes the size of the surface instead of the size of the scene.
//!
//! Each case frames the same object the same way from further and further
//! back, shrinking the fov to match, because it is the eye-to-surface distance
//! the precision turns on and the court is what makes that distance large.
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test large_coordinate_analytic \
//!     -- --ignored --nocapture --test-threads=1
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_math::{Point3, Transform, Vec3};
use vcad_kernel_primitives::{make_sphere, make_torus};
use vcad_kernel_raytrace::gpu::{GpuCamera, GpuRenderState, GpuScene, RayTracePipeline};
use vcad_kernel_raytrace::pathtrace::GradientEnv;
use vcad_kernel_raytrace::{Bvh, Ray};

const W: u32 = 96;
const H: u32 = 96;

/// How far off the world origin each case puts the object, in millimetres —
/// the last two are half-court and a full court diagonal.
const OFFSETS: [f64; 4] = [0.0, 3e3, 1e4, 2e4];

/// The eye stands off this far from the object at offset 0; beyond that it
/// stays put at the world origin and the object walks away from it.
const NEAR_STANDOFF: f64 = 700.0;

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

/// Eye, target and fov for one case: the object at `offset` along +x, framed
/// to the same `radius` on screen however far away it is.
fn framing(offset: f64, radius: f64) -> ([f32; 3], [f32; 3], f32) {
    let at = [offset, 0.0, 120.0];
    let eye = [0.0, -NEAR_STANDOFF, 120.0 + 0.25 * NEAR_STANDOFF];
    let dist =
        ((at[0] - eye[0]).powi(2) + (at[1] - eye[1]).powi(2) + (at[2] - eye[2]).powi(2)).sqrt();
    // A hair over the object, so the silhouette has background around it.
    let fov = 2.0 * (1.35 * radius / dist).atan();
    (
        [eye[0] as f32, eye[1] as f32, eye[2] as f32],
        [at[0] as f32, at[1] as f32, at[2] as f32],
        fov as f32,
    )
}

/// The GPU's ray for a pixel centre, reproduced bit for bit from the camera
/// uniform the shader reads — f32 basis, then widened, so the two tiers are
/// asking the same question and any disagreement is in the solve.
fn gpu_ray(cam: &GpuCamera, px: u32, py: u32) -> (Point3, Vec3) {
    let aspect = cam.width as f32 / cam.height as f32;
    let fov_tan = (cam.fov * 0.5).tan();
    let ndc = [
        (px as f32 + 0.5) / cam.width as f32 * 2.0 - 1.0,
        1.0 - (py as f32 + 0.5) / cam.height as f32 * 2.0,
    ];
    let sub = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let norm = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let forward = norm(sub(cam.target, cam.position));
    let right = norm(cross(forward, [cam.up[0], cam.up[1], cam.up[2]]));
    let up = cross(right, forward);
    let dir = norm([
        forward[0] + right[0] * ndc[0] * fov_tan * aspect + up[0] * ndc[1] * fov_tan,
        forward[1] + right[1] * ndc[0] * fov_tan * aspect + up[1] * ndc[1] * fov_tan,
        forward[2] + right[2] * ndc[0] * fov_tan * aspect + up[2] * ndc[1] * fov_tan,
    ]);
    (
        Point3::new(
            cam.position[0] as f64,
            cam.position[1] as f64,
            cam.position[2] as f64,
        ),
        Vec3::new(dir[0] as f64, dir[1] as f64, dir[2] as f64),
    )
}

fn state() -> GpuRenderState {
    let mut s = GpuRenderState::new(1);
    s.enable_edges = 0;
    s.stylize = 0;
    s.ground_enabled = 0;
    s.max_depth = 1;
    // A geometry comparison, not a sampling one: the CPU ray goes through the
    // pixel centre, so the GPU's must too.
    s.jitter_x = 0.0;
    s.jitter_y = 0.0;
    s.set_gradient_env(&GradientEnv {
        zenith: [1.0; 3],
        horizon: [1.0; 3],
        ground: [1.0; 3],
        intensity: 1.0,
    });
    s
}

struct Case {
    /// Intersection over union of the two silhouettes.
    iou: f64,
    /// Mean |Δt| over the pixels both tiers hit, in millimetres.
    mean_dt: f64,
    /// Worst |Δt| over the same pixels.
    max_dt: f64,
    gpu_px: usize,
    cpu_px: usize,
}

/// One offset of one shape: render the placed scene on the GPU, trace the
/// unplaced one on the CPU with the eye translated back by the same vector.
/// The placement is a pure translation, so `t` is the same number in both.
fn case(
    ctx: &'static GpuContext,
    solid: &vcad_kernel_primitives::BRepSolid,
    offset: f64,
    radius: f64,
) -> Case {
    let (eye, at, fov) = framing(offset, radius);
    let packed = GpuScene::from_brep(solid)
        .expect("scene packs")
        .placed(&Transform::translation(offset, 0.0, 120.0));
    let bvh = Bvh::build(solid);

    let pipeline = RayTracePipeline::new(ctx).expect("pipeline");
    let cam = GpuCamera::new(eye, at, [0.0, 0.0, 1.0], fov, W, H);
    let mut res = pipeline.resident_scene(ctx, &packed, W, H);
    let film = pollster::block_on(pipeline.render_resident_linear(ctx, &mut res, &cam, state()))
        .expect("linear render");

    let (mut both, mut either) = (0usize, 0usize);
    let (mut sum, mut n, mut worst) = (0.0f64, 0usize, 0.0f64);
    let (mut gpu_px, mut cpu_px) = (0usize, 0usize);
    for py in 0..H {
        for px in 0..W {
            let i = (py * W + px) as usize;
            // `Film::depth` is distance from the eye along the primary ray,
            // and 0 for background.
            let g_t = film.depth[i] as f64;
            let g_hit = g_t > 0.0;

            let (o, d) = gpu_ray(&cam, px, py);
            let local = Point3::new(o.x - offset, o.y, o.z - 120.0);
            let c_hit = bvh.trace_closest(&Ray::new(local, d));

            if g_hit {
                gpu_px += 1;
            }
            if c_hit.is_some() {
                cpu_px += 1;
            }
            if g_hit && c_hit.is_some() {
                both += 1;
            }
            if g_hit || c_hit.is_some() {
                either += 1;
            }
            if let (true, Some(h)) = (g_hit, c_hit) {
                let e = (g_t - h.t).abs();
                sum += e;
                n += 1;
                if e > worst {
                    worst = e;
                }
            }
        }
    }
    assert!(either > 0, "neither renderer drew anything at all");
    Case {
        iou: both as f64 / either as f64,
        mean_dt: sum / n.max(1) as f64,
        max_dt: worst,
        gpu_px,
        cpu_px,
    }
}

/// A thin torus is the hard one — the quartic has to resolve a 2 mm tube from
/// twenty metres — and the sphere is the control: milder, because a quadratic
/// has no depressed coefficients to cancel, but wrong by millimetres at
/// twenty metres all the same.
#[test]
#[ignore = "requires GPU"]
fn an_analytic_surface_lands_where_the_cpu_puts_it_at_court_coordinates() {
    let Some(ctx) =
        ctx_or_skip("an_analytic_surface_lands_where_the_cpu_puts_it_at_court_coordinates")
    else {
        return;
    };

    let shapes: [(&str, vcad_kernel_primitives::BRepSolid, f64); 2] = [
        ("seam torus R74 r2", make_torus(74.0, 2.0, 64), 76.0),
        ("ball sphere r120", make_sphere(120.0, 64), 120.0),
    ];

    let mut report = String::new();
    let mut worst_iou = 1.0f64;
    let mut worst_dt = 0.0f64;
    for (name, solid, radius) in &shapes {
        for offset in OFFSETS {
            let c = case(ctx, solid, offset, *radius);
            report.push_str(&format!(
                "  {name:<18} offset {offset:>7.0} mm: IoU {:.4}  mean |Δt| {:.5} mm  \
                 max {:.5} mm  (GPU {} px, CPU {} px)\n",
                c.iou, c.mean_dt, c.max_dt, c.gpu_px, c.cpu_px,
            ));
            worst_iou = worst_iou.min(c.iou);
            worst_dt = worst_dt.max(c.mean_dt);
        }
    }
    eprintln!("{report}");
    assert!(
        worst_iou >= 0.98,
        "worst silhouette IoU {worst_iou:.4} — the GPU's analytic solve has \
         lost the surface at a court coordinate:\n{report}"
    );
    assert!(
        worst_dt < 0.05,
        "worst mean |Δt| {worst_dt:.5} mm — the GPU's hit distances have \
         drifted off the CPU's:\n{report}"
    );
}
