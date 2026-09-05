//! The GPU's linear exit: one raw sample per pass, with the CPU `Film`'s guides.
//!
//! `render_resident` hands back tonemapped 8-bit pixels, which a host keeping
//! its own per-pixel history cannot use — the tonemap is not invertible and
//! averaging display-referred values is not averaging radiance.
//! `render_resident_linear` hands back a [`Film`] instead: linear radiance,
//! plus the depth, normal and albedo guides in exactly the conventions
//! `pathtrace::render` fills them, so the CPU denoiser and any reprojection
//! written against the CPU tier work on the GPU tier unchanged.
//!
//! What is checked here is that the two tiers actually agree — the sample is
//! unbiased against the CPU's mean, and the guides describe the same geometry
//! the CPU found — and that a scissored pass leaves the rest of the film
//! alone.
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test linear_film -- --ignored --nocapture
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use std::sync::Arc;
use std::time::Instant;

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::make_sphere;
use vcad_kernel_raytrace::gpu::{
    GpuAreaLight, GpuCamera, GpuRenderState, GpuScene, RayTracePipeline,
};
use vcad_kernel_raytrace::pathtrace::{
    self, AreaLight, Camera, Environment, Object, PathTraceOptions, Pbr,
};
use vcad_kernel_raytrace::Bvh;

const W: u32 = 48;
const H: u32 = 48;

const EYE: [f32; 3] = [26.0, 24.0, 20.0];
const AT: [f32; 3] = [0.0, 0.0, 0.0];
const FOV_DEG: f32 = 45.0;
const RADIUS: f64 = 6.0;

/// Path depth both tiers trace. Shallow keeps the statistical test affordable
/// without changing what it measures: the same integrator on both sides.
const MAX_DEPTH: u32 = 3;

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

/// The one softbox both tiers get, described twice.
const PANEL_CENTER: [f64; 3] = [4.0, -3.0, 22.0];
const PANEL_U: [f64; 3] = [7.0, 0.0, 0.0];
const PANEL_V: [f64; 3] = [0.0, -7.0, 0.0];
const PANEL_EMISSION: f32 = 3.0;

/// A grey dielectric, the same on both tiers.
const BASE_COLOR: [f32; 3] = [0.72, 0.70, 0.66];
const ROUGHNESS: f32 = 0.4;

fn gpu_scene() -> GpuScene {
    let solid = make_sphere(RADIUS, 48);
    let mut s = GpuScene::from_brep(&solid).expect("scene packs");
    for m in &mut s.materials {
        m.color = [BASE_COLOR[0], BASE_COLOR[1], BASE_COLOR[2], 1.0];
        m.metallic = 0.0;
        m.roughness = ROUGHNESS;
        m.clearcoat = 0.0;
        m.anisotropy = 0.0;
    }
    s.lights = vec![GpuAreaLight {
        center: [
            PANEL_CENTER[0] as f32,
            PANEL_CENTER[1] as f32,
            PANEL_CENTER[2] as f32,
            0.0,
        ],
        u: [PANEL_U[0] as f32, PANEL_U[1] as f32, PANEL_U[2] as f32, 0.0],
        v: [PANEL_V[0] as f32, PANEL_V[1] as f32, PANEL_V[2] as f32, 0.0],
        emission: [PANEL_EMISSION, PANEL_EMISSION, PANEL_EMISSION, 0.0],
    }];
    s
}

fn gpu_camera(w: u32, h: u32) -> GpuCamera {
    GpuCamera::new(EYE, AT, [0.0, 0.0, 1.0], FOV_DEG.to_radians(), w, h)
}

/// Path trace only: no edge overlay, no stylisation, no implicit floor, and
/// the analytic sky off so the softbox is the whole of the lighting and both
/// tiers can be made to agree on it exactly.
fn lights_only_state(frame: u32) -> GpuRenderState {
    let mut s = GpuRenderState::new(frame);
    s.enable_edges = 0;
    s.stylize = 0;
    s.ground_enabled = 0;
    s.env_intensity = 0.0;
    s.max_depth = MAX_DEPTH;
    s
}

fn cpu_scene() -> pathtrace::Scene {
    let solid = make_sphere(RADIUS, 48);
    let bvh = Arc::new(Bvh::build(&solid));
    pathtrace::Scene {
        objects: vec![Object::new(
            bvh,
            Pbr {
                base_color: BASE_COLOR,
                metallic: 0.0,
                roughness: ROUGHNESS,
                ..Pbr::default()
            },
        )],
        lights: vec![AreaLight {
            center: Point3::new(PANEL_CENTER[0], PANEL_CENTER[1], PANEL_CENTER[2]),
            u: Vec3::new(PANEL_U[0], PANEL_U[1], PANEL_U[2]),
            v: Vec3::new(PANEL_V[0], PANEL_V[1], PANEL_V[2]),
            emission: [PANEL_EMISSION; 3],
        }],
        env: Environment::constant([0.0, 0.0, 0.0]),
        ground: None,
    }
}

fn cpu_camera() -> Camera {
    Camera::look_at(
        Point3::new(EYE[0] as f64, EYE[1] as f64, EYE[2] as f64),
        Point3::new(AT[0] as f64, AT[1] as f64, AT[2] as f64),
        Vec3::new(0.0, 0.0, 1.0),
        FOV_DEG as f64,
    )
}

fn cpu_film(spp: u32, w: u32, h: u32) -> pathtrace::Film {
    pathtrace::render(
        &cpu_scene(),
        &cpu_camera(),
        w,
        h,
        &PathTraceOptions {
            spp,
            max_depth: MAX_DEPTH,
            denoise: false,
            show_background: false,
            ..Default::default()
        },
    )
}

/// Shrink a boolean mask by `r` pixels.
///
/// The two tiers jitter the primary ray differently — Halton on the GPU, the
/// pixel's own RNG on the CPU — so a pixel straddling the silhouette is a hit
/// on one and a miss on the other, and just inside it the depth gradient is
/// steep enough that a sub-pixel offset is worth a percent. Neither is a
/// disagreement about the geometry, so every comparison here stays off the
/// limb by eroding whichever set it is measuring over.
fn erode(flag: &[bool], w: u32, h: u32, r: i32) -> Vec<bool> {
    let (w, h) = (w as i32, h as i32);
    let mut out = vec![false; flag.len()];
    for y in r..h - r {
        for x in r..w - r {
            out[(y * w + x) as usize] =
                (-r..=r).all(|dy| (-r..=r).all(|dx| flag[((y + dy) * w + (x + dx)) as usize]));
        }
    }
    out
}

/// Pixels the primary ray hit on both tiers, eroded by `r`.
fn interior_mask(a: &[f32], b: &[f32], w: u32, h: u32, r: i32) -> Vec<bool> {
    let hit: Vec<bool> = a.iter().zip(b).map(|(&p, &q)| p > 0.0 && q > 0.0).collect();
    erode(&hit, w, h, r)
}

/// Pixels the primary ray escaped on both tiers, eroded by `r`.
fn background_mask(a: &[f32], b: &[f32], w: u32, h: u32, r: i32) -> Vec<bool> {
    let miss: Vec<bool> = a
        .iter()
        .zip(b)
        .map(|(&p, &q)| p == 0.0 && q == 0.0)
        .collect();
    erode(&miss, w, h, r)
}

/// (a) The raw GPU sample is an unbiased estimate of the CPU's mean.
///
/// One pass of `render_resident_linear` is one sample per pixel, so the two
/// tiers cannot be compared frame to frame — only in expectation. Averaging
/// many independent GPU passes and comparing against a converged CPU render is
/// the only comparison the estimator actually promises to pass, and it is the
/// one that catches the mistake this exit exists to avoid: returning
/// `accumulated` (already divided by `frame_index`) instead of this pass's own
/// sample would make the GPU mean fall off as 1/N.
#[test]
#[ignore = "requires GPU"]
fn the_raw_gpu_sample_averages_to_the_cpu_render() {
    let Some(ctx) = ctx_or_skip("the_raw_gpu_sample_averages_to_the_cpu_render") else {
        return;
    };
    const PASSES: u32 = 256;

    let pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");
    let scene = gpu_scene();
    let mut resident = pipeline.resident_scene(ctx, &scene, W, H);
    let cam = gpu_camera(W, H);

    let n = (W * H) as usize;
    let mut sum = vec![0.0f64; n * 3];
    let mut depth = vec![0.0f32; n];
    for frame in 1..=PASSES {
        let film = pollster::block_on(pipeline.render_resident_linear(
            ctx,
            &mut resident,
            &cam,
            lights_only_state(frame),
        ))
        .expect("linear render");
        for (s, c) in sum.iter_mut().zip(&film.rgb) {
            *s += *c as f64;
        }
        if frame == 1 {
            depth = film.depth.clone();
        }
    }

    let cpu = cpu_film(512, W, H);
    let mask = interior_mask(&depth, &cpu.depth, W, H, 1);
    let lit = mask.iter().filter(|&&m| m).count();
    assert!(
        lit > 80,
        "only {lit} interior pixels — nothing was rendered"
    );

    let (mut g, mut c) = (0.0f64, 0.0f64);
    for i in 0..n {
        if !mask[i] {
            continue;
        }
        for k in 0..3 {
            g += sum[i * 3 + k] / PASSES as f64;
            c += cpu.rgb[i * 3 + k] as f64;
        }
    }
    let (g, c) = (g / (lit * 3) as f64, c / (lit * 3) as f64);
    let rel = (g - c).abs() / c.max(1e-6);
    eprintln!("[a] GPU mean of {PASSES} raw passes {g:.5}, CPU 512 spp {c:.5} (rel {rel:.4})");
    assert!(
        c > 1e-3,
        "the CPU reference is black ({c:.6}) — the two tiers are not lit the same",
    );
    assert!(
        rel < 0.05,
        "the mean of {PASSES} raw GPU passes is {g:.5} against the CPU's \
         {c:.5}, {:.1}% apart. Either the pass is still folding itself into a \
         running average, or the two tiers are not tracing the same integrand.",
        rel * 100.0,
    );
}

/// (b) and (c): the guide buffers describe the geometry the CPU found.
///
/// Depth is the one with a convention to get wrong — the shader's own buffer
/// stores `MAX_T` for background and the CPU film stores 0 — and the normal is
/// the one with a *sign* to get wrong, since the shader's edge-detection copy
/// is not face-forwarded and the CPU film's is.
#[test]
#[ignore = "requires GPU"]
fn the_guide_buffers_match_the_cpu_films() {
    let Some(ctx) = ctx_or_skip("the_guide_buffers_match_the_cpu_films") else {
        return;
    };
    // Finer than the statistical test: the two tiers sample different
    // sub-pixel points, and how much that is worth in world units scales with
    // the pixel's own footprint. At 48x48 the limb of this sphere moves nearly
    // a percent of its depth across one pixel, which is the pixel grid talking,
    // not the depth convention.
    const N: u32 = 192;
    let pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");
    let scene = gpu_scene();
    let mut resident = pipeline.resident_scene(ctx, &scene, N, N);
    let mut gpu_state = lights_only_state(1);
    // Sample the pixel centre, which is what the CPU film's jitter averages
    // around — this is a comparison of geometry, not of sampling patterns.
    gpu_state.jitter_x = 0.0;
    gpu_state.jitter_y = 0.0;
    let gpu = pollster::block_on(pipeline.render_resident_linear(
        ctx,
        &mut resident,
        &gpu_camera(N, N),
        gpu_state,
    ))
    .expect("linear render");
    let cpu = cpu_film(16, N, N);

    let mask = interior_mask(&gpu.depth, &cpu.depth, N, N, 5);
    let lit = mask.iter().filter(|&&m| m).count();
    assert!(
        lit > 2000,
        "only {lit} interior pixels — nothing was rendered"
    );

    // (b) depth, on hit pixels.
    let mut worst_depth = 0.0f32;
    for (i, _) in mask.iter().enumerate().filter(|(_, &m)| m) {
        let rel = (gpu.depth[i] - cpu.depth[i]).abs() / cpu.depth[i];
        worst_depth = worst_depth.max(rel);
    }
    eprintln!("[b] worst depth disagreement {:.4}%", worst_depth * 100.0);
    assert!(
        worst_depth < 0.005,
        "the GPU's depth guide is {:.3}% off the CPU film's on some interior \
         pixel. Both are meant to be the distance from the eye along the \
         primary ray, in world units.",
        worst_depth * 100.0,
    );

    // (b, continued) and background must be the CPU's sentinel, not MAX_T.
    let sky = background_mask(&gpu.depth, &cpu.depth, N, N, 5);
    let background: Vec<usize> = (0..(N * N) as usize).filter(|&i| sky[i]).collect();
    assert!(
        background.len() > 1000,
        "only {} background pixels — nothing tests the background sentinel",
        background.len(),
    );
    for &i in &background {
        assert_eq!(
            gpu.depth[i], 0.0,
            "the GPU wrote depth {} where the primary ray escaped. The film's \
             background sentinel is 0, not the shader's MAX_T — a caller's \
             `depth > 0.0` hit test would see the whole sky as geometry.",
            gpu.depth[i],
        );
    }

    // (c) normals, on hit pixels.
    let mut worst_dot = 1.0f32;
    for (i, _) in mask.iter().enumerate().filter(|(_, &m)| m) {
        let d = (0..3)
            .map(|k| gpu.normal[i * 3 + k] * cpu.normal[i * 3 + k])
            .sum::<f32>();
        worst_dot = worst_dot.min(d);
    }
    eprintln!("[c] worst normal dot {worst_dot:.5}");
    assert!(
        worst_dot > 0.99,
        "a GPU guide normal is only {worst_dot:.4} aligned with the CPU \
         film's. If it is near -1 the face-forwarding against the view ray is \
         missing.",
    );

    // The albedo guide has to be the shading albedo, not the base colour with
    // the metallic mix skipped — a denoiser demodulates by it.
    for (i, _) in mask.iter().enumerate().filter(|(_, &m)| m) {
        for k in 0..3 {
            assert!(
                (gpu.albedo[i * 3 + k] - cpu.albedo[i * 3 + k]).abs() < 1e-3,
                "albedo guide {} vs CPU {} at pixel {i} channel {k}",
                gpu.albedo[i * 3 + k],
                cpu.albedo[i * 3 + k],
            );
        }
    }
}

/// (d) A scissored pass traces its rectangle and leaves the rest of the film
/// exactly as the previous pass left it.
///
/// The buffers are resident, so "untouched" means *stale*, not zero — which is
/// the whole point: a caller re-rendering a rectangle into a history it keeps
/// itself wants the rest of that history back unchanged.
#[test]
#[ignore = "requires GPU"]
fn a_scissored_linear_pass_leaves_the_rest_of_the_film_alone() {
    let Some(ctx) = ctx_or_skip("a_scissored_linear_pass_leaves_the_rest_of_the_film_alone") else {
        return;
    };
    let pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");
    let scene = gpu_scene();
    let mut resident = pipeline.resident_scene(ctx, &scene, W, H);
    let cam = gpu_camera(W, H);

    let full = pollster::block_on(pipeline.render_resident_linear(
        ctx,
        &mut resident,
        &cam,
        lights_only_state(1),
    ))
    .expect("full pass");

    let (sx, sy, sw, sh) = (12u32, 10u32, 20u32, 16u32);
    let mut state = lights_only_state(7);
    state.set_scissor([sx, sy, sw, sh]);
    let scissored =
        pollster::block_on(pipeline.render_resident_linear(ctx, &mut resident, &cam, state))
            .expect("scissored pass");

    let mut inside_changed = 0usize;
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) as usize;
            let within = x >= sx && x < sx + sw && y >= sy && y < sy + sh;
            let same = (0..3).all(|k| full.rgb[i * 3 + k] == scissored.rgb[i * 3 + k])
                && full.depth[i] == scissored.depth[i]
                && (0..3).all(|k| full.normal[i * 3 + k] == scissored.normal[i * 3 + k]);
            if within {
                if !same {
                    inside_changed += 1;
                }
            } else {
                assert!(
                    same,
                    "pixel ({x}, {y}) is outside the scissor and changed. A \
                     scissored pass must dispatch only over its rectangle.",
                );
            }
        }
    }
    assert!(
        inside_changed > (sw * sh) as usize / 4,
        "only {inside_changed} of {} pixels inside the scissor changed — the \
         rectangle was not actually re-traced",
        sw * sh,
    );
}

/// What each exit costs per pass at 512x512. Not an assertion — a measurement,
/// printed with `--nocapture`.
#[test]
#[ignore = "requires GPU"]
fn measure_the_three_exits() {
    let Some(ctx) = ctx_or_skip("measure_the_three_exits") else {
        return;
    };
    const N: u32 = 512;
    const REPS: u32 = 20;

    let pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");
    let scene = gpu_scene();
    let cam = gpu_camera(N, N);
    let mut resident = pipeline.resident_scene(ctx, &scene, N, N);

    {
        let p = &pipeline;
        let r = &mut resident;
        let c = &cam;
        // Each closure borrows separately; run them one at a time.
        let mut run = |frame: u32| {
            pollster::block_on(p.render_resident_linear(ctx, r, c, lights_only_state(frame)))
                .expect("linear");
        };
        run(1);
        let t = Instant::now();
        for i in 0..REPS {
            run(i + 2);
        }
        eprintln!(
            "[measure] render_resident_linear (guides read back): {:.2} ms/pass",
            t.elapsed().as_secs_f64() * 1000.0 / REPS as f64
        );
    }
    {
        let p = &pipeline;
        let r = &mut resident;
        let c = &cam;
        let mut run = |frame: u32| {
            pollster::block_on(p.render_resident(ctx, r, c, lights_only_state(frame)))
                .expect("resident");
        };
        run(1);
        let t = Instant::now();
        for i in 0..REPS {
            run(i + 2);
        }
        eprintln!(
            "[measure] render_resident (tonemapped readback): {:.2} ms/pass",
            t.elapsed().as_secs_f64() * 1000.0 / REPS as f64
        );
    }
    {
        let run = |frame: u32| {
            pollster::block_on(pipeline.render_with_render_state(
                ctx,
                &scene,
                &cam,
                N,
                N,
                None,
                lights_only_state(frame),
            ))
            .expect("one-shot");
        };
        run(1);
        let t = Instant::now();
        for i in 0..REPS {
            run(i + 2);
        }
        eprintln!(
            "[measure] render_with_render_state (re-upload + tonemapped): {:.2} ms/pass",
            t.elapsed().as_secs_f64() * 1000.0 / REPS as f64
        );
    }
}
