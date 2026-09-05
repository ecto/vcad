//! Carrying the per-pixel history across a camera move, on the device.
//!
//! Before this, a camera move could only be expressed through the keep mask,
//! and a viewer with no reprojection of its own had one move available:
//! upload an all-restart mask and watch the whole frame drop back to a single
//! sample. An orbit therefore looked like a frame of pure noise every time
//! the mouse moved, even though almost every pixel in it was the same wall
//! seen from a hair to the left.
//!
//! `accumulate_and_denoise_resident_reprojected` takes the previous pass's
//! camera and does the work in a compute pass: unproject through this pass's
//! depth, project into the previous view, and keep the nearest previous
//! pixel's mean and count where its depth agrees within 2% and its normal
//! within a dot product of 0.9.
//!
//! Three claims:
//!
//! * a move smaller than a pixel keeps essentially everything;
//! * a 5° orbit past an occluder keeps the wall and restarts the strip of it
//!   the occluder had been hiding — the two populations are separated on the
//!   host, out of the two views' own depth buffers, so the test knows which
//!   pixels *ought* to survive;
//! * a history carried across a move is still the mean of the samples in it,
//!   not a smear: a converged wall reprojected onto a moved view agrees with
//!   the same wall accumulated from scratch there.
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test gpu_reprojection \
//!     -- --ignored --nocapture --test-threads=1
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_sphere};
use vcad_kernel_raytrace::gpu::{
    GpuAreaLight, GpuCamera, GpuDenoiseParams, GpuRenderState, GpuScene, HistoryPipeline,
    RayTracePipeline, ResidentScene,
};

const W: u32 = 96;
const H: u32 = 96;
const FOV_DEG: f32 = 45.0;

/// The room, in the units the rest of this suite uses.
const ROOM: (f64, f64, f64) = (40.0, 40.0, 20.0);
/// The occluder, floating between the eye and the far wall.
const BALL_AT: [f64; 3] = [20.0, 20.0, 10.0];
const BALL_R: f64 = 2.5;

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

fn scene() -> GpuScene {
    let mut room = GpuScene::from_brep(&make_cube(ROOM.0, ROOM.1, ROOM.2)).expect("room packs");
    let ball = GpuScene::from_brep(&make_sphere(BALL_R, 48))
        .expect("ball packs")
        .placed(&Transform::translation(BALL_AT[0], BALL_AT[1], BALL_AT[2]));
    for m in &mut room.materials {
        m.color = [0.70, 0.68, 0.64, 1.0];
        m.metallic = 0.0;
        m.roughness = 0.8;
        m.clearcoat = 0.0;
        m.anisotropy = 0.0;
    }
    let mut s = room.merge(ball);
    s.lights = vec![GpuAreaLight {
        center: [20.0, 14.0, 19.0, 0.0],
        u: [8.0, 0.0, 0.0, 0.0],
        v: [0.0, -8.0, 0.0, 0.0],
        emission: [12.0, 12.0, 12.0, 0.0],
    }];
    s
}

/// The eye, orbited `deg` about the occluder in the floor plane, always
/// looking at the middle of the far wall.
fn camera(deg: f64) -> GpuCamera {
    let r = BALL_AT[1] - 8.0; // stand-off from the ball
    let a = deg.to_radians();
    let eye = [
        BALL_AT[0] + r * a.sin(),
        BALL_AT[1] - r * a.cos(),
        BALL_AT[2],
    ];
    GpuCamera::new(
        [eye[0] as f32, eye[1] as f32, eye[2] as f32],
        [BALL_AT[0] as f32, ROOM.1 as f32, BALL_AT[2] as f32],
        [0.0, 0.0, 1.0],
        FOV_DEG.to_radians(),
        W,
        H,
    )
}

/// The eye shifted sideways by `frac` of one pixel's world footprint at the
/// far wall — a move small enough that every pixel is still looking at the
/// same square inch of plaster.
fn nudged(frac: f64) -> GpuCamera {
    let base = camera(0.0);
    let wall_dist = ROOM.1 - 8.0; // the eye is 8 units in from the near wall
    let px = 2.0 * wall_dist * (f64::from(FOV_DEG).to_radians() / 2.0).tan() / W as f64;
    let mut cam = base;
    cam.position[0] += (frac * px) as f32;
    cam.target[0] += (frac * px) as f32;
    cam
}

fn state(frame: u32) -> GpuRenderState {
    let mut s = GpuRenderState::new(frame);
    s.enable_edges = 0;
    s.stylize = 0;
    s.ground_enabled = 0;
    s.env_intensity = 0.0;
    s.max_depth = 3;
    s
}

/// A caller-owned storage texture for the resolve pass to land in. This test
/// never looks at it — every claim here is about the history behind it — but
/// the entry point needs somewhere to write.
fn target(ctx: &GpuContext) -> wgpu::TextureView {
    ctx.device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("reprojection test target"),
            size: wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

/// The primary ray for a pixel centre under `cam`, as the shader's own ray
/// generator builds it.
fn ray_dir(cam: &GpuCamera, x: u32, y: u32) -> [f64; 3] {
    let norm = |v: [f64; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let eye = [
        cam.position[0] as f64,
        cam.position[1] as f64,
        cam.position[2] as f64,
    ];
    let fwd = norm([
        cam.target[0] as f64 - eye[0],
        cam.target[1] as f64 - eye[1],
        cam.target[2] as f64 - eye[2],
    ]);
    let right = norm(cross(
        fwd,
        [cam.up[0] as f64, cam.up[1] as f64, cam.up[2] as f64],
    ));
    let up = cross(right, fwd);
    let tan_fov = (cam.fov as f64 * 0.5).tan();
    let aspect = cam.width as f64 / cam.height as f64;
    let ndc = [
        (x as f64 + 0.5) / cam.width as f64 * 2.0 - 1.0,
        1.0 - (y as f64 + 0.5) / cam.height as f64 * 2.0,
    ];
    norm([
        fwd[0] + right[0] * ndc[0] * tan_fov * aspect + up[0] * ndc[1] * tan_fov,
        fwd[1] + right[1] * ndc[0] * tan_fov * aspect + up[1] * ndc[1] * tan_fov,
        fwd[2] + right[2] * ndc[0] * tan_fov * aspect + up[2] * ndc[1] * tan_fov,
    ])
}

fn eye_of(cam: &GpuCamera) -> [f64; 3] {
    [
        cam.position[0] as f64,
        cam.position[1] as f64,
        cam.position[2] as f64,
    ]
}

/// How close the segment from `from` to `p` comes to the ball's centre,
/// relative to its radius, or `f64::INFINITY` if the nearest approach is not
/// between the two ends. Below 1 the ball was in the way.
fn ball_blocking(from: [f64; 3], p: [f64; 3]) -> f64 {
    let d = [p[0] - from[0], p[1] - from[1], p[2] - from[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let u = [d[0] / len, d[1] / len, d[2] / len];
    let c = [
        BALL_AT[0] - from[0],
        BALL_AT[1] - from[1],
        BALL_AT[2] - from[2],
    ];
    let t = c[0] * u[0] + c[1] * u[1] + c[2] * u[2];
    if t <= 0.0 || t >= len {
        return f64::INFINITY;
    }
    let perp = [c[0] - t * u[0], c[1] - t * u[1], c[2] - t * u[2]];
    (perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2]).sqrt() / BALL_R
}

/// Pixels adjacent to a depth or normal discontinuity in `film`.
///
/// These are the pixels a reprojection is *supposed* to be unsure about — a
/// silhouette, a wall seam — and they are not what any claim here is about.
fn edge_map(film: &vcad_kernel_raytrace::pathtrace::Film) -> Vec<bool> {
    let n = |i: usize| {
        [
            film.normal[i * 3] as f64,
            film.normal[i * 3 + 1] as f64,
            film.normal[i * 3 + 2] as f64,
        ]
    };
    let mut out = vec![false; (W * H) as usize];
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            let i = (y * W as i32 + x) as usize;
            for dy in -1..=1i32 {
                for dx in -1..=1i32 {
                    let (qx, qy) = (x + dx, y + dy);
                    if qx < 0 || qy < 0 || qx >= W as i32 || qy >= H as i32 {
                        out[i] = true;
                        continue;
                    }
                    let j = (qy * W as i32 + qx) as usize;
                    let (a, b) = (n(i), n(j));
                    if a[0] * b[0] + a[1] * b[1] + a[2] * b[2] < 0.99 {
                        out[i] = true;
                    }
                    let d = film.depth[i].max(1e-6);
                    if (film.depth[i] - film.depth[j]).abs() > 0.05 * d {
                        out[i] = true;
                    }
                }
            }
        }
    }
    out
}

struct Rig {
    pipeline: RayTracePipeline,
    history: HistoryPipeline,
    res: ResidentScene,
    view: wgpu::TextureView,
    denoise: GpuDenoiseParams,
    frame: u32,
}

impl Rig {
    fn new(ctx: &'static GpuContext) -> Self {
        let pipeline = RayTracePipeline::new(ctx).expect("pipeline");
        let history = HistoryPipeline::new(ctx).expect("history pipeline");
        let res = pipeline.resident_scene(ctx, &scene(), W, H);
        Self {
            pipeline,
            history,
            res,
            view: target(ctx),
            denoise: GpuDenoiseParams {
                // The filter has nothing to do with these claims and its fade
                // would only obscure them.
                iters: 0,
                ..Default::default()
            },
            frame: 0,
        }
    }

    /// One pass, optionally reprojecting from `prev`.
    fn pass(&mut self, ctx: &GpuContext, cam: &GpuCamera, prev: Option<&GpuCamera>) {
        self.frame += 1;
        self.pipeline
            .accumulate_and_denoise_resident_reprojected(
                ctx,
                &self.history,
                &mut self.res,
                cam,
                state(self.frame),
                &[],
                &self.denoise,
                &self.view,
                prev,
            )
            .expect("history pass");
    }

    fn counts(&mut self, ctx: &GpuContext) -> Vec<u32> {
        pollster::block_on(self.pipeline.read_history(ctx, &mut self.res))
            .expect("read history")
            .expect("a pass has run")
            .count
    }

    fn rgb(&mut self, ctx: &GpuContext) -> Vec<f32> {
        pollster::block_on(self.pipeline.read_history(ctx, &mut self.res))
            .expect("read history")
            .expect("a pass has run")
            .rgb
    }
}

/// (a) A move smaller than a pixel keeps the frame.
#[test]
#[ignore = "requires GPU"]
fn a_sub_pixel_camera_move_keeps_the_whole_frame() {
    let Some(ctx) = ctx_or_skip("a_sub_pixel_camera_move_keeps_the_whole_frame") else {
        return;
    };
    let mut rig = Rig::new(ctx);
    let c0 = camera(0.0);
    const N: u32 = 8;
    for _ in 0..N {
        rig.pass(ctx, &c0, None);
    }

    let c1 = nudged(0.25);
    rig.pass(ctx, &c1, Some(&c0));
    let counts = rig.counts(ctx);
    let kept = counts.iter().filter(|&&c| c == N + 1).count() as f64 / (W * H) as f64;

    // Where the rejections fall matters more than how many there are. A
    // reprojection is entitled to be unsure at a silhouette or a wall seam;
    // it is not entitled to be unsure in the middle of a wall.
    let probe = RayTracePipeline::new(ctx).expect("pipeline");
    let mut pres = probe.resident_scene(ctx, &scene(), W, H);
    let film = pollster::block_on(probe.render_resident_linear(ctx, &mut pres, &c1, state(1)))
        .expect("render");
    let edges = edge_map(&film);
    let interior_lost = (0..(W * H) as usize)
        .filter(|&i| !edges[i] && counts[i] != N + 1)
        .count();

    eprintln!(
        "sub-pixel move: {:.1}% of pixels kept their history; {} lost away from an edge",
        kept * 100.0,
        interior_lost,
    );
    assert!(
        kept >= 0.95,
        "a quarter-pixel camera move threw away {:.1}% of the history — the \
         reprojection is not finding the surface it just left",
        (1.0 - kept) * 100.0,
    );
    assert_eq!(
        interior_lost, 0,
        "{interior_lost} pixels nowhere near a silhouette or a seam lost their \
         history to a quarter-pixel move"
    );

    // And the contrast: the same move with no previous view is the old
    // behaviour, every pixel back to one sample.
    let mut plain = Rig::new(ctx);
    for _ in 0..N {
        plain.pass(ctx, &c0, None);
    }
    plain.pass(ctx, &c1, None);
    let carried = plain.counts(ctx).iter().filter(|&&c| c > 1).count();
    assert_eq!(
        carried,
        (W * H) as usize,
        "sanity: without a keep mask an un-reprojected pass still accumulates"
    );
}

/// (b) A 5° orbit keeps the wall and restarts what the ball had been hiding.
///
/// The two populations are separated on the host by geometry rather than by
/// a second copy of the shader's own arithmetic: unproject each pixel through
/// this view's depth to a world point, and ask whether the *previous* eye
/// could see that point at all, or whether the ball stood in the way. The
/// blocked ones are precisely the pixels with no history to carry.
#[test]
#[ignore = "requires GPU"]
fn an_orbit_keeps_the_wall_and_restarts_the_disocclusion() {
    let Some(ctx) = ctx_or_skip("an_orbit_keeps_the_wall_and_restarts_the_disocclusion") else {
        return;
    };
    let (c0, c1) = (camera(0.0), camera(5.0));

    // The two depth buffers, taken through the linear exit on a scene of
    // their own so the history rig below is untouched.
    let probe = RayTracePipeline::new(ctx).expect("pipeline");
    let mut pres = probe.resident_scene(ctx, &scene(), W, H);
    let d0 = pollster::block_on(probe.render_resident_linear(ctx, &mut pres, &c0, state(1)))
        .expect("render")
        .depth;
    let f1 = pollster::block_on(probe.render_resident_linear(ctx, &mut pres, &c1, state(1)))
        .expect("render");
    let d1 = f1.depth.clone();

    // The eye stands 12 units off the ball and 32 off the far wall; anything
    // nearer than 25 is the ball.
    let ball_cut = 25.0f32;
    let edges = edge_map(&f1);
    let eye0 = eye_of(&c0);
    let eye1 = eye_of(&c1);
    let mut disoccluded = Vec::new();
    let mut clean_wall = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) as usize;
            if d1[i] < ball_cut || edges[i] {
                continue; // the ball itself, or a boundary either way
            }
            let dir = ray_dir(&c1, x, y);
            let p = [
                eye1[0] + d1[i] as f64 * dir[0],
                eye1[1] + d1[i] as f64 * dir[1],
                eye1[2] + d1[i] as f64 * dir[2],
            ];
            let block = ball_blocking(eye0, p);
            if block < 0.9 {
                disoccluded.push(i); // the ball was squarely in the way
            } else if block > 1.3 && d0[i] >= ball_cut {
                clean_wall.push(i); // clear of it from both eyes
            }
        }
    }
    assert!(
        disoccluded.len() > 20,
        "the orbit disoccluded only {} pixels — it is not testing anything",
        disoccluded.len()
    );

    let mut rig = Rig::new(ctx);
    const N: u32 = 8;
    for _ in 0..N {
        rig.pass(ctx, &c0, None);
    }
    rig.pass(ctx, &c1, Some(&c0));
    let counts = rig.counts(ctx);

    let rate =
        |ix: &[usize]| ix.iter().filter(|&&i| counts[i] == N + 1).count() as f64 / ix.len() as f64;
    let (wall, hole) = (rate(&clean_wall), rate(&disoccluded));
    eprintln!(
        "5° orbit: wall kept {:.1}% ({} px), disocclusion kept {:.1}% ({} px)",
        wall * 100.0,
        clean_wall.len(),
        hole * 100.0,
        disoccluded.len(),
    );
    assert!(
        wall >= 0.95,
        "only {:.1}% of the unoccluded wall kept its history across a 5° \
         orbit — the reprojection is throwing away pixels it can see",
        wall * 100.0,
    );
    assert!(
        hole <= 0.1,
        "{:.1}% of the disoccluded strip kept a history it cannot have — the \
         reprojection is carrying the ball's samples onto the wall behind it",
        hole * 100.0,
    );
}

/// (c) A carried history is still a mean, not a smear.
///
/// A converged wall reprojected onto a moved view has to agree with the same
/// wall converged at that view from scratch. If the reprojection picked the
/// wrong pixels, or blended what it found, the carried mean drifts and takes
/// `count_cutoff` samples to come back.
#[test]
#[ignore = "requires GPU"]
fn a_reprojected_mean_is_the_mean_the_moved_view_would_have_converged_to() {
    let Some(ctx) =
        ctx_or_skip("a_reprojected_mean_is_the_mean_the_moved_view_would_have_converged_to")
    else {
        return;
    };
    let (c0, c1) = (camera(0.0), nudged(0.25));
    const N: u32 = 48;

    // Converged at the moved view, with no history to carry.
    let mut fresh = Rig::new(ctx);
    for _ in 0..N {
        fresh.pass(ctx, &c1, None);
    }
    let reference = fresh.rgb(ctx);

    // Converged at the original view, then carried across the move.
    let mut carried = Rig::new(ctx);
    for _ in 0..N {
        carried.pass(ctx, &c0, None);
    }
    carried.pass(ctx, &c1, Some(&c0));
    let moved = carried.rgb(ctx);
    let counts = carried.counts(ctx);

    // The far wall, away from the ball's silhouette and the softbox: a large
    // flat population whose mean is well determined at 48 samples.
    let probe = RayTracePipeline::new(ctx).expect("pipeline");
    let mut pres = probe.resident_scene(ctx, &scene(), W, H);
    let film = pollster::block_on(probe.render_resident_linear(ctx, &mut pres, &c1, state(1)))
        .expect("render");

    let (mut a, mut b, mut n) = (0.0f64, 0.0f64, 0usize);
    for i in 0..(W * H) as usize {
        if film.depth[i] < 15.0 || counts[i] <= 1 {
            continue; // the ball, or a pixel that legitimately restarted
        }
        let lum = |v: &[f32]| {
            0.2126 * v[i * 3] as f64 + 0.7152 * v[i * 3 + 1] as f64 + 0.0722 * v[i * 3 + 2] as f64
        };
        a += lum(&moved);
        b += lum(&reference);
        n += 1;
    }
    assert!(n > 500, "only {n} wall pixels to compare");
    let (a, b) = (a / n as f64, b / n as f64);
    let rel = (a - b).abs() / b;
    eprintln!(
        "wall mean: carried {a:.6}, converged fresh {b:.6} over {n} px ({:.3}%)",
        rel * 100.0
    );
    assert!(
        rel < 0.01,
        "a wall carried across a sub-pixel move reads {a:.6} against the \
         {b:.6} the moved view converges to on its own, {:.2}% apart — the \
         reprojection is not carrying the mean it thinks it is",
        rel * 100.0,
    );
}
