//! A sealed room, on both tiers.
//!
//! Kosm's court is a closed gym: four walls, a ceiling, a floor, ten downward
//! ceiling panels of radiance 18, matte dielectric everywhere. No ray ever
//! escapes, so every photon that reaches a wall got there by bouncing — which
//! is why the environment fix moved nothing there. The GPU still rendered it
//! far darker than the CPU.
//!
//! This is that room, and it says where the darkness was. The walls between
//! the panels agree to a tenth of a percent at every path depth, with and
//! without Russian roulette: the transport is the same on both tiers. The gap
//! is entirely the panels themselves — the shader dropped an emitter hit at
//! depth 0 so the viewport's auto-sized rig would not swing through frame as
//! white slabs, and `pathtrace::render` draws them at full emission. Ten
//! radiance-18 panels in frame against a wall at 0.6 is a factor on any mean
//! that includes them: measured here, the GPU came out at 57% of the CPU.
//!
//! `GpuRenderState::set_camera_visible_lights` is the choice, and with it on
//! the two tiers converge to the same image.
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test closed_room \
//!     -- --ignored --nocapture --test-threads=1
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use std::sync::Arc;

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::make_cube;
use vcad_kernel_raytrace::gpu::{GpuAreaLight, GpuCamera, GpuRenderState, GpuScene};
use vcad_kernel_raytrace::pathtrace::{
    self, AreaLight, Camera, Environment, GradientEnv, Object, PathTraceOptions, Pbr,
};
use vcad_kernel_raytrace::Bvh;

const N: u32 = 64;
const PASSES: u32 = 500;
const CLAMP: f32 = 12.0;

/// The room: a cube the camera sits inside. Both tiers face-forward the
/// shading normal, so an outward-facing box read from within is six inward
/// walls — the sealed gym, without a court's worth of geometry.
const RX: f64 = 30.0;
const RY: f64 = 50.0;
const RZ: f64 = 12.0;

/// Squarely down the long axis at the far wall, so the view ray meets it at
/// normal incidence at the frame centre.
const EYE: [f32; 3] = [15.0, 6.0, 6.0];
const AT: [f32; 3] = [15.0, 50.0, 6.0];
const FOV: f32 = 50.0;

const BASE: [f32; 3] = [0.62, 0.6, 0.58];
const ROUGH: f32 = 0.85;

/// Ten downward ceiling panels of radiance 18, as the court's rig has.
const EMISSION: f32 = 18.0;
const PANEL_HALF: f64 = 2.0;

fn panels() -> Vec<AreaLight> {
    let mut out = Vec::new();
    for row in 0..5 {
        for col in 0..2 {
            out.push(AreaLight {
                center: Point3::new(
                    RX * (0.3 + 0.4 * col as f64),
                    RY * (0.12 + 0.19 * row as f64),
                    RZ - 0.5,
                ),
                // cross(u, v) points at -z: the emitting face looks down.
                u: Vec3::new(PANEL_HALF, 0.0, 0.0),
                v: Vec3::new(0.0, -PANEL_HALF, 0.0),
                emission: [EMISSION; 3],
            });
        }
    }
    out
}

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
    let mut s = GpuScene::from_brep(&make_cube(RX, RY, RZ)).expect("scene packs");
    for g in &mut s.materials {
        g.color = [BASE[0], BASE[1], BASE[2], 1.0];
        g.metallic = 0.0;
        g.roughness = ROUGH;
        g.clearcoat = 0.0;
        g.anisotropy = 0.0;
    }
    s.lights = panels().iter().map(GpuAreaLight::from_area_light).collect();
    s
}

fn cpu_scene() -> pathtrace::Scene {
    pathtrace::Scene {
        objects: vec![Object::new(
            Arc::new(Bvh::build_brep(&make_cube(RX, RY, RZ))),
            material(),
        )],
        lights: panels(),
        // Sealed: no ray escapes, so the sky is a formality. Black on both
        // tiers keeps it one, and keeps this test about transport.
        env: Environment::constant([0.0; 3]),
        sun: None,
        ground: None,
    }
}

fn state(frame: u32, max_depth: u32, rr_start: u32, visible_lights: bool) -> GpuRenderState {
    let mut s = GpuRenderState::new(frame);
    s.enable_edges = 0;
    s.stylize = 0;
    s.ground_enabled = 0;
    // `GpuRenderState::new` escalates depth with the frame index for the
    // viewport's draft frames; a parity measurement wants the same depth every
    // pass, and the same one the CPU is given.
    s.max_depth = max_depth;
    s.rr_start = rr_start;
    s.firefly_clamp = CLAMP;
    s.set_camera_visible_lights(visible_lights);
    s.set_gradient_env(&GradientEnv {
        zenith: [0.0; 3],
        horizon: [0.0; 3],
        ground: [0.0; 3],
        intensity: 1.0,
    });
    s
}

fn cpu_camera() -> Camera {
    Camera::look_at(
        Point3::new(EYE[0] as f64, EYE[1] as f64, EYE[2] as f64),
        Point3::new(AT[0] as f64, AT[1] as f64, AT[2] as f64),
        Vec3::new(0.0, 0.0, 1.0),
        FOV as f64,
    )
}

/// Converged linear images from both tiers, plus the GPU's hit mask.
///
/// The GPU side is the mean of `PASSES` independent raw samples, which is what
/// a host driving `render_resident_linear` accumulates; the CPU side is the
/// same number of samples per pixel through `pathtrace::render`. The two draw
/// from different sequences, so this is a claim about expectations.
fn images(
    ctx: &GpuContext,
    max_depth: u32,
    rr_start: u32,
    visible_lights: bool,
) -> (Vec<f64>, Vec<f32>, Vec<f32>) {
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline");
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
            state(f, max_depth, rr_start, visible_lights),
        ))
        .expect("linear render");
        for (s, c) in sum.iter_mut().zip(&film.rgb) {
            *s += *c as f64;
        }
        if f == 1 {
            depth.copy_from_slice(&film.depth);
        }
    }
    for s in &mut sum {
        *s /= PASSES as f64;
    }

    let cpu = pathtrace::render(
        &cpu_scene(),
        &cpu_camera(),
        N,
        N,
        &PathTraceOptions {
            spp: PASSES,
            max_depth,
            rr_start,
            firefly_clamp: Some(CLAMP),
            denoise: false,
            show_background: false,
            ..Default::default()
        },
    );
    (sum, cpu.rgb, depth)
}

fn mean64(v: &[f64], mask: &[f32]) -> f64 {
    let (mut s, mut k) = (0.0, 0usize);
    for (i, m) in mask.iter().enumerate() {
        if *m <= 0.0 {
            continue;
        }
        for j in 0..3 {
            s += v[i * 3 + j];
        }
        k += 3;
    }
    s / k.max(1) as f64
}

fn mean32(v: &[f32], mask: &[f32]) -> f64 {
    let (mut s, mut k) = (0.0, 0usize);
    for (i, m) in mask.iter().enumerate() {
        if *m <= 0.0 {
            continue;
        }
        for j in 0..3 {
            s += v[i * 3 + j] as f64;
        }
        k += 3;
    }
    s / k.max(1) as f64
}

/// Split the hit mask into the pixels that see an emitter directly and the
/// pixels that see a wall. A radiance-18 panel and a wall at 0.6 are three
/// orders of magnitude apart, and a mean over both is a mean over the panels.
fn split(cpu: &[f32], depth: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let n = (N * N) as usize;
    let (mut wall, mut hot) = (vec![0.0f32; n], vec![0.0f32; n]);
    for i in 0..n {
        if depth[i] <= 0.0 {
            continue;
        }
        let l = (cpu[i * 3] + cpu[i * 3 + 1] + cpu[i * 3 + 2]) / 3.0;
        // A generous threshold: a pixel that a jittered sample only sometimes
        // finds a panel through still carries panel energy, and it belongs
        // with the panels rather than with the walls.
        if l > 2.0 {
            hot[i] = 1.0;
        } else {
            wall[i] = 1.0;
        }
    }
    (wall, hot)
}

/// Every surface a bounce reaches must converge to the same radiance on both
/// tiers, at every path depth and with Russian roulette on or off.
///
/// This is the whole of the indirect transport in a room where nothing else
/// exists: the one-light power pick, the light PDF with ten panels, the
/// BSDF-hits-an-emitter MIS branch, the firefly clamp, the roulette weighting,
/// and how each tier counts a bounce. Any of those wrong shows up here as a
/// factor.
#[test]
#[ignore = "requires GPU"]
fn the_walls_of_a_sealed_room_converge_alike_at_every_depth() {
    let Some(ctx) = ctx_or_skip("the_walls_of_a_sealed_room_converge_alike_at_every_depth") else {
        return;
    };
    // Depth 1 is direct lighting alone; depth 6 with roulette from 3 is what
    // both renderers default to.
    for (max_depth, rr_start) in [(1u32, 99u32), (2, 99), (6, 99), (6, 3)] {
        // With the panels visible on both tiers there is no depth-0
        // divergence left to confound the measurement: what remains is
        // transport, and the walls are where transport shows.
        let (gpu, cpu, depth) = images(ctx, max_depth, rr_start, true);
        let (wall, _) = split(&cpu, &depth);
        let g = mean64(&gpu, &wall);
        let c = mean32(&cpu, &wall);
        let rel = (g - c).abs() / c;
        eprintln!(
            "  depth {max_depth}, rr {rr_start}: walls GPU {g:.5} vs CPU {c:.5} \
             ({:.2}% apart)",
            rel * 100.0,
        );
        assert!(c > 1e-3, "the CPU reference is black ({c:.6})");
        assert!(
            rel < 0.02,
            "at depth {max_depth} with roulette from {rr_start}, the converged \
             GPU wall mean is {g:.5} against the CPU's {c:.5}, {:.1}% apart",
            rel * 100.0,
        );
    }
}

/// With the panels visible to camera rays, the whole frame agrees — and
/// without, the emitter pixels are the entire disagreement.
#[test]
#[ignore = "requires GPU"]
fn camera_visible_panels_close_the_gap_in_a_sealed_room() {
    let Some(ctx) = ctx_or_skip("camera_visible_panels_close_the_gap_in_a_sealed_room") else {
        return;
    };

    // What the shader did before there was a choice: a mean over a frame with
    // ten panels in it that it declined to draw. Kept as a measurement, not an
    // assertion — it is the size of the bug.
    let (gpu, cpu, depth) = images(ctx, 6, 3, false);
    let (wall, hot) = split(&cpu, &depth);
    assert!(
        hot.iter().any(|x| *x > 0.0),
        "no panel is in frame, so this test would be measuring nothing"
    );
    eprintln!(
        "[hidden] frame GPU {:.5} vs CPU {:.5} — GPU is {:.1}% of the CPU",
        mean64(&gpu, &depth),
        mean32(&cpu, &depth),
        100.0 * mean64(&gpu, &depth) / mean32(&cpu, &depth),
    );
    eprintln!(
        "[hidden] walls  GPU {:.5} vs CPU {:.5}  |  panels GPU {:.5} vs CPU {:.5}",
        mean64(&gpu, &wall),
        mean32(&cpu, &wall),
        mean64(&gpu, &hot),
        mean32(&cpu, &hot),
    );

    let (gpu, cpu, depth) = images(ctx, 6, 3, true);
    let g = mean64(&gpu, &depth);
    let c = mean32(&cpu, &depth);
    let rel = (g - c).abs() / c;
    eprintln!(
        "[visible] frame GPU {g:.5} vs CPU {c:.5} (rel {:.2}%)",
        rel * 100.0
    );
    assert!(
        rel < 0.02,
        "with the panels visible to camera rays the converged GPU frame mean \
         is {g:.5} against the CPU's {c:.5}, {:.1}% apart",
        rel * 100.0,
    );
}

/// The flag is off unless asked for, so a caller that never mentions it
/// renders exactly what it rendered before.
use vcad_kernel_raytrace::BrepBvh;
#[test]
fn camera_visible_lights_is_off_by_default_and_independent_of_raw_sample() {
    let mut s = GpuRenderState::new(1);
    assert!(!s.camera_visible_lights());
    assert!(!s.raw_sample());

    // The two flags share one word; setting either must not disturb the other.
    s.set_camera_visible_lights(true);
    assert!(s.camera_visible_lights());
    assert!(!s.raw_sample());
    s.set_raw_sample(true);
    assert!(s.camera_visible_lights());
    assert!(s.raw_sample());
    s.set_camera_visible_lights(false);
    assert!(!s.camera_visible_lights());
    assert!(s.raw_sample());
    s.set_raw_sample(false);
    assert!(!s.camera_visible_lights());
    assert!(!s.raw_sample());
}
