//! One light per bounce, on the GPU.
//!
//! The shader draws a single area light per bounce from a power-weighted CDF
//! packed into the light buffer's spare `w` lanes, instead of shadow-raying
//! every panel. That is only sound if the estimator is unbiased, which is a
//! claim about a *mean*, not about any one frame — so both tests here converge
//! an accumulated render and compare brightness.
//!
//! The invariant the first test uses is the sharp one: splitting a softbox
//! into several coincident copies whose emissions sum to the original is the
//! same light. It changes the pick table completely (one entry becomes
//! several, with unequal probabilities) without changing a single photon, so
//! any error in the CDF, in the division by the pick probability, or in the
//! MIS light PDF shows up as a brightness shift. No CPU renderer is involved,
//! so nothing is confounded by the two renderers' different backdrops.
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test light_sampling -- --ignored
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_primitives::make_sphere;
use vcad_kernel_raytrace::gpu::{
    GpuAreaLight, GpuCamera, GpuRenderState, GpuScene, RayTracePipeline,
};

const W: u32 = 64;
const H: u32 = 64;
const FRAMES: u32 = 96;

/// A rough metal subject. Diffuse would barely exercise the other half of
/// the MIS pair — a Lambertian bounce almost never lands on a softbox — so
/// the light PDF used when a BSDF ray *does* hit an emitter would go
/// untested. A broad GGX lobe aimed at the panels tests it.
fn make_rough_metal(scene: &mut GpuScene) {
    for m in &mut scene.materials {
        m.metallic = 1.0;
        m.roughness = 0.45;
        m.color = [0.9, 0.9, 0.9, 1.0];
    }
}

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

/// Path trace only: no edges, no stylisation, no implicit floor, and the
/// analytic sky turned off so the area lights are the only illumination and
/// the measurement is entirely about them.
fn lights_only_state() -> GpuRenderState {
    let mut s = GpuRenderState::new(1);
    s.enable_edges = 0;
    s.stylize = 0;
    s.ground_enabled = 0;
    s.env_intensity = 0.0;
    s.max_depth = 3;
    s
}

fn test_camera() -> GpuCamera {
    GpuCamera::new(
        [26.0, 24.0, 20.0],
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        45.0_f32.to_radians(),
        W,
        H,
    )
}

/// The subject's silhouette, from the flat debug pass: no Monte Carlo noise in
/// it, and it does not depend on the lighting we are about to measure.
fn subject_mask(ctx: &GpuContext, scene: &GpuScene) -> Vec<bool> {
    let pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");
    let mut state = lights_only_state();
    state.debug_mode = 4;
    let (px, _) = pollster::block_on(pipeline.render_with_render_state(
        ctx,
        scene,
        &test_camera(),
        W,
        H,
        None,
        state,
    ))
    .expect("render");
    px.chunks(4)
        .map(|p| p[0].max(p[1]) as i32 - p[2] as i32 > 40)
        .collect()
}

/// Accumulate `FRAMES` progressive frames and return the mean of the RGB
/// channels *over the subject only*. Averaging the whole frame would be
/// averaging mostly backdrop, which the area lights do not touch — the shift
/// this is looking for would vanish into it.
fn converged_mean(ctx: &GpuContext, scene: &GpuScene) -> f64 {
    let mask = subject_mask(ctx, scene);
    let pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");
    let camera = test_camera();
    let mut accum = None;
    let mut pixels = Vec::new();
    for frame in 1..=FRAMES {
        let mut state = lights_only_state();
        state.frame_index = frame;
        let (px, buf) = pollster::block_on(
            pipeline.render_with_render_state(ctx, scene, &camera, W, H, accum, state),
        )
        .expect("render");
        pixels = px;
        accum = Some(buf);
    }
    let n = mask.iter().filter(|m| **m).count();
    assert!(
        n > 200,
        "the subject covers only {n} px — nothing to measure"
    );
    let sum: u64 = pixels
        .chunks(4)
        .zip(&mask)
        .filter(|(_, m)| **m)
        .map(|(p, _)| p[0] as u64 + p[1] as u64 + p[2] as u64)
        .sum();
    sum as f64 / (n * 3) as f64
}

fn panel(center: [f32; 3], half: f32, emission: [f32; 3]) -> GpuAreaLight {
    GpuAreaLight {
        center: [center[0], center[1], center[2], 0.0],
        // Faces -z, i.e. down onto the subject at the origin.
        u: [half, 0.0, 0.0, 0.0],
        v: [0.0, -half, 0.0, 0.0],
        emission: [emission[0], emission[1], emission[2], 0.0],
    }
}

/// Cut a panel into strips along its u axis at the given cumulative
/// fractions, keeping the emission. The strips tile the original exactly:
/// same emitting surface, same radiance, same shadows — but now several
/// entries in the power table, with shares proportional to their widths.
///
/// Coincident duplicates would have been the simpler split, and they are the
/// wrong test: two lights at the same place are two NEE techniques that can
/// both produce one direction, and the MIS weight for that direction would
/// have to sum over them. Tiling keeps every direction owned by exactly one
/// strip, so the invariant really is "the same light".
fn split_along_u(l: &GpuAreaLight, cuts: &[f32]) -> Vec<GpuAreaLight> {
    let mut out = Vec::new();
    let mut lo = 0.0f32;
    for &hi in cuts.iter().chain(std::iter::once(&1.0)) {
        let (a, b) = (2.0 * lo - 1.0, 2.0 * hi - 1.0);
        let mid = 0.5 * (a + b);
        let scale = 0.5 * (b - a);
        out.push(GpuAreaLight {
            center: [
                l.center[0] + l.u[0] * mid,
                l.center[1] + l.u[1] * mid,
                l.center[2] + l.u[2] * mid,
                0.0,
            ],
            u: [l.u[0] * scale, l.u[1] * scale, l.u[2] * scale, 0.0],
            v: l.v,
            emission: l.emission,
        });
        lo = hi;
    }
    out
}

/// Retiling a softbox into unequal strips must not change the picture.
///
/// One panel, then the same panel as two strips split 1:3, then as four
/// strips of quite different widths. The emitting surface is identical in all
/// three; what changes is the power table — one entry becomes four, with
/// shares from 10% to 45%. Any error in the CDF, in the division by the pick
/// probability, or in the MIS light PDF shows up as a brightness shift.
#[test]
#[ignore = "requires GPU"]
fn retiling_a_softbox_into_unequal_strips_changes_nothing() {
    let Some(ctx) = ctx_or_skip("retiling_a_softbox_into_unequal_strips_changes_nothing") else {
        return;
    };
    let solid = make_sphere(6.0, 32);
    let whole = panel([4.0, -3.0, 22.0], 7.0, [2.4; 3]);

    let rigs: [(&str, Vec<GpuAreaLight>); 3] = [
        ("one panel", vec![whole]),
        ("two strips, 1:3", split_along_u(&whole, &[0.25])),
        (
            "four unequal strips",
            split_along_u(&whole, &[0.1, 0.3, 0.55]),
        ),
    ];

    let mut reference: Option<(&str, f64)> = None;
    for (name, lights) in rigs {
        let mut scene = GpuScene::from_brep(&solid).expect("scene packs");
        scene.lights = lights;
        make_rough_metal(&mut scene);
        let mean = converged_mean(ctx, &scene);
        match reference {
            None => reference = Some((name, mean)),
            Some((rname, r)) => {
                let rel = (mean - r).abs() / r;
                assert!(
                    rel < 0.02,
                    "[{name}] converged to {mean:.3}/255 where [{rname}] gave \
                     {r:.3} — a {:.1}% shift. Strips that tile the original are \
                     the same light, so the power table, the division by the \
                     pick probability, or the MIS light PDF is wrong.",
                    rel * 100.0,
                );
            }
        }
    }
    assert!(
        reference.map(|(_, r)| r > 8.0).unwrap_or(false),
        "the subject came out black — the test measured nothing"
    );
}

/// The ten-panel case the one-per-bounce sampler exists for: a ring of dim
/// panels around one bright one, so a uniform pick would badly under-sample
/// the panel carrying the light. Retiling every panel into two unequal strips
/// doubles the table and reshuffles every share, and must land on the same
/// image.
#[test]
#[ignore = "requires GPU"]
fn a_ten_panel_rig_is_energy_stable_under_retiling() {
    let Some(ctx) = ctx_or_skip("a_ten_panel_rig_is_energy_stable_under_retiling") else {
        return;
    };
    let solid = make_sphere(6.0, 32);

    let ring: Vec<GpuAreaLight> = (0..10)
        .map(|i| {
            let a = i as f32 / 10.0 * std::f32::consts::TAU;
            let e = if i == 3 { 2.0 } else { 0.1 };
            panel([a.cos() * 16.0, a.sin() * 16.0, 20.0], 5.0, [e; 3])
        })
        .collect();

    let mut whole = GpuScene::from_brep(&solid).expect("scene packs");
    whole.lights = ring.clone();
    make_rough_metal(&mut whole);

    let mut split = GpuScene::from_brep(&solid).expect("scene packs");
    split.lights = ring
        .iter()
        .flat_map(|l| split_along_u(l, &[0.35]))
        .collect();
    make_rough_metal(&mut split);

    let a = converged_mean(ctx, &whole);
    let b = converged_mean(ctx, &split);
    assert!(a > 8.0, "the ten-panel rig came out black ({a:.3})");
    let rel = (a - b).abs() / a;
    assert!(
        rel < 0.02,
        "ten panels converged to {a:.3}/255 and the same rig retiled into \
         twenty strips to {b:.3} — a {:.1}% shift. The power table does not \
         describe the same distribution at both sizes.",
        rel * 100.0,
    );
}

/// The compute pass's scissor: a rectangle's pixels must come out exactly as
/// they do in a full-frame pass, and everything outside it must be left alone.
///
/// Bit-identical, not "close": the shader offsets the invocation rather than
/// re-deriving anything, so a scissored pixel runs the same code with the same
/// seed. That is the property `render_into` has on the CPU, and it is what
/// lets a caller re-trace a region into a frame it already has.
#[test]
#[ignore = "requires GPU"]
fn a_scissored_pass_matches_the_full_frame_inside_and_leaves_the_rest() {
    let Some(ctx) =
        ctx_or_skip("a_scissored_pass_matches_the_full_frame_inside_and_leaves_the_rest")
    else {
        return;
    };
    let mut scene = GpuScene::from_brep(&make_sphere(6.0, 32)).expect("scene packs");
    scene.lights = vec![panel([4.0, -3.0, 22.0], 7.0, [2.4; 3])];

    let pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");
    let camera = test_camera();
    let render = |rect: Option<[u32; 4]>| -> Vec<u8> {
        let mut state = lights_only_state();
        state.frame_index = 1;
        if let Some(r) = rect {
            state.set_scissor(r);
        }
        pollster::block_on(
            pipeline.render_with_render_state(ctx, &scene, &camera, W, H, None, state),
        )
        .expect("render")
        .0
    };

    let full = render(None);
    let rect = [17u32, 9, 24, 30];
    let scissored = render(Some(rect));

    let mut inside = 0usize;
    let mut differed = 0usize;
    for y in 0..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            let within =
                x >= rect[0] && y >= rect[1] && x < rect[0] + rect[2] && y < rect[1] + rect[3];
            if within {
                inside += 1;
                assert_eq!(
                    &scissored[i..i + 3],
                    &full[i..i + 3],
                    "pixel ({x}, {y}) is inside the scissor but the scissored pass \
                     and the full frame disagree — the invocation offset is not \
                     landing on the same pixel's work",
                );
            } else if scissored[i..i + 3] != full[i..i + 3] {
                differed += 1;
            }
        }
    }
    assert!(inside > 500, "the scissor covered only {inside} px");
    // Outside the rect the pass wrote nothing, so the texture keeps its clear
    // value — which is not what the full frame painted there.
    assert!(
        differed > 500,
        "only {differed} px outside the scissor differ from the full frame; the \
         pass is evidently still covering the whole image",
    );
}
