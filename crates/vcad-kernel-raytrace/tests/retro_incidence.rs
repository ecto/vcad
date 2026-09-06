//! Normal incidence: the picture, not just the BSDF.
//!
//! A ~6% dark ring was reported in a GPU render of Kosm's court, a few pixels
//! out from the point where the camera's view ray meets a wall head-on, and
//! absent from the CPU render of the same scene. That geometry is
//! retro-reflection — `wo`, `wi` and the normal all within a degree or two —
//! and it is the corner of the shading model most likely to come apart in f32:
//! see `gpu_bsdf_matches_cpu_reference_at_retro_angles` in `bsdf_parity.rs`,
//! which pins the BSDF itself there, and `d_ggx`, whose denominator used to
//! cancel to nothing at `n·h = 1`.
//!
//! This is the same claim one level up: the whole GPU integrator, on the scene
//! the report describes — one plane facing the camera, one area light, direct
//! lighting only — against a converged CPU render of the same thing. The
//! comparison is binned into annuli about the normal-incidence pixel, which is
//! the shape the artefact would have and the only way to see a few percent
//! through Monte Carlo noise.
//!
//! The camera's field of view is 6 degrees, not the viewport's 45: a 30-pixel
//! ring in a 512-wide frame sits about two degrees off the normal, and at a
//! normal field of view that is two pixels. The narrow lens spreads those two
//! degrees across the whole frame.
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test retro_incidence -- --ignored --nocapture
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use std::sync::Arc;

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::make_cube;
use vcad_kernel_raytrace::gpu::{GpuAreaLight, GpuCamera, GpuRenderState, GpuScene};
use vcad_kernel_raytrace::pathtrace::{
    self, AreaLight, Camera, Environment, Object, PathTraceOptions, Pbr,
};
use vcad_kernel_raytrace::Bvh;

const N: u32 = 97;
const FOV: f32 = 6.0;
const PASSES: u32 = 1500;

/// A slab 200 x 200 x 4 standing in for the wall; the camera is above its
/// centre looking straight down, so the middle pixel meets it at exactly
/// normal incidence and the frame's corners at about 4 degrees.
const SX: f64 = 200.0;
const EYE: [f32; 3] = [100.0, 100.0, 64.0];
const AT: [f32; 3] = [100.0, 100.0, 4.0];

/// The panel sits just behind the eye, facing the wall, which puts the light
/// direction on the view direction: `wo`, `wi` and the normal coincide at the
/// centre pixel. Retro-reflection in the strict sense, and the configuration
/// under which `d_ggx`'s old denominator lost every significant digit.
const LC: [f64; 3] = [100.0, 100.0, 70.0];
const LU: [f64; 3] = [6.0, 0.0, 0.0];
const LV: [f64; 3] = [0.0, -6.0, 0.0];
const LE: f32 = 18.0;

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

/// The two materials worth looking at here: the matte dielectric the report
/// names, and a semi-gloss with a clearcoat, whose narrow specular lobe is
/// pointed straight back at the camera by this light placement and is where a
/// precision failure at `n·h → 1` would be largest.
fn materials() -> [(&'static str, Pbr); 2] {
    [
        (
            "matte, roughness 0.85",
            Pbr {
                base_color: [0.6, 0.6, 0.6],
                metallic: 0.0,
                roughness: 0.85,
                clearcoat: 0.0,
                ..Pbr::default()
            },
        ),
        (
            "semi-gloss with clearcoat, roughness 0.15",
            Pbr {
                base_color: [0.6, 0.6, 0.6],
                metallic: 0.0,
                roughness: 0.15,
                clearcoat: 0.5,
                clearcoat_roughness: 0.1,
                ..Pbr::default()
            },
        ),
    ]
}

fn gpu_scene(m: &Pbr) -> GpuScene {
    let mut s = GpuScene::from_brep(&make_cube(SX, SX, 4.0)).expect("scene packs");
    for g in &mut s.materials {
        g.color = [m.base_color[0], m.base_color[1], m.base_color[2], 1.0];
        g.metallic = m.metallic;
        g.roughness = m.roughness;
        g.clearcoat = m.clearcoat;
        g.clearcoat_roughness = m.clearcoat_roughness;
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

fn cpu_scene(m: &Pbr) -> pathtrace::Scene {
    pathtrace::Scene {
        objects: vec![Object::new(
            Arc::new(Bvh::build_brep(&make_cube(SX, SX, 4.0))),
            *m,
        )],
        lights: vec![AreaLight {
            center: Point3::new(LC[0], LC[1], LC[2]),
            u: Vec3::new(LU[0], LU[1], LU[2]),
            v: Vec3::new(LV[0], LV[1], LV[2]),
            emission: [LE; 3],
        }],
        sun: None,
        env: Environment::constant([0.0, 0.0, 0.0]),
        ground: None,
        splats: None,
    }
}

/// Direct lighting only, as the report describes: no bounce, no environment,
/// no implicit floor, no stylisation.
fn state(frame: u32) -> GpuRenderState {
    let mut s = GpuRenderState::new(frame);
    s.enable_edges = 0;
    s.stylize = 0;
    s.ground_enabled = 0;
    s.env_intensity = 0.0;
    s.max_depth = 1;
    s
}

/// The GPU and CPU images of one material must agree in every annulus about
/// the normal-incidence pixel.
///
/// The tolerance is 1.5%, against a reported artefact of 6%. Each annulus
/// averages tens to hundreds of pixels of 1500 samples, so the residual noise
/// is a few tenths of a percent and the innermost rings — a single pixel, then
/// eight — are the loosest. A systematic error concentrated at normal
/// incidence would move one band and leave the rest alone, which no amount of
/// noise does.
use vcad_kernel_raytrace::BrepBvh;
#[test]
#[ignore = "requires GPU"]
fn the_gpu_has_no_ring_at_normal_incidence() {
    let Some(ctx) = ctx_or_skip("the_gpu_has_no_ring_at_normal_incidence") else {
        return;
    };
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline");
    let cam = GpuCamera::new(EYE, AT, [0.0, 1.0, 0.0], FOV.to_radians(), N, N);
    let n = (N * N) as usize;

    for (name, m) in materials() {
        let sc = gpu_scene(&m);
        let mut res = pipeline.resident_scene(ctx, &sc, N, N);
        let mut sum = vec![0.0f64; n * 3];
        for f in 1..=PASSES {
            let film =
                pollster::block_on(pipeline.render_resident_linear(ctx, &mut res, &cam, state(f)))
                    .expect("linear render");
            for (s, c) in sum.iter_mut().zip(&film.rgb) {
                *s += *c as f64;
            }
        }
        for s in sum.iter_mut() {
            *s /= PASSES as f64;
        }

        let cpu = pathtrace::render(
            &cpu_scene(&m),
            &Camera::look_at(
                Point3::new(EYE[0] as f64, EYE[1] as f64, EYE[2] as f64),
                Point3::new(AT[0] as f64, AT[1] as f64, AT[2] as f64),
                Vec3::new(0.0, 1.0, 0.0),
                FOV as f64,
            ),
            N,
            N,
            &PathTraceOptions {
                spp: PASSES,
                max_depth: 1,
                denoise: false,
                show_background: false,
                ..Default::default()
            },
        );

        // Bin into annuli about the centre pixel, which is the one whose view
        // ray meets the wall at exactly normal incidence.
        let c = (N / 2) as f64;
        let bins = (N / 2) as usize;
        let mut gpu_sum = vec![0.0f64; bins];
        let mut cpu_sum = vec![0.0f64; bins];
        let mut counts = vec![0.0f64; bins];
        for y in 0..N as usize {
            for x in 0..N as usize {
                let i = y * N as usize + x;
                let r = ((x as f64 - c).powi(2) + (y as f64 - c).powi(2)).sqrt() as usize;
                if r >= bins {
                    continue;
                }
                for k in 0..3 {
                    gpu_sum[r] += sum[i * 3 + k];
                    cpu_sum[r] += cpu.rgb[i * 3 + k] as f64;
                }
                counts[r] += 3.0;
            }
        }

        // The innermost annuli hold one pixel, then eight: at 1500 samples of
        // a narrow specular lobe that is several percent of noise on its own,
        // which says nothing about a systematic shift. Start where an annulus
        // has enough pixels to average that down — 20 pixels, which is radius
        // 3 and about a tenth of a degree off normal.
        // Three channels per pixel, so 60 channels is 20 pixels.
        const MIN_CHANNELS: f64 = 60.0;
        let mut worst = 0.0f64;
        let mut worst_r = 0usize;
        for r in 0..bins {
            if counts[r] < MIN_CHANNELS {
                continue;
            }
            let g = gpu_sum[r] / counts[r];
            let cc = cpu_sum[r] / counts[r];
            assert!(
                cc > 1e-3,
                "[{name}] annulus {r} of the CPU reference is black ({cc:.6}) \
                 — the scene is not lit",
            );
            let rel = (g / cc - 1.0).abs();
            if rel > worst {
                worst = rel;
                worst_r = r;
            }
        }
        eprintln!(
            "[{name}] worst annulus disagreement {:.3}% at radius {worst_r}",
            worst * 100.0
        );
        assert!(
            worst < 0.015,
            "[{name}] the GPU and CPU images disagree by {:.2}% in the annulus \
             {worst_r} pixels from the normal-incidence point. That is the \
             shape and the place of the ring reported from the viewport: a \
             precision failure in the specular lobe at n·h → 1, or a light \
             sample the two tiers weight differently when wo, wi and the \
             normal coincide.",
            worst * 100.0,
        );
    }
}
