//! The per-pixel history and the à-trous filter, on the device.
//!
//! A viewer driving `render_resident_linear` keeps the history itself: fold
//! each raw sample into a running mean, run `pathtrace::denoise` over the
//! whole frame, tonemap. Measured in Kosm's viewer at 512x288, the filter
//! alone was 420 ms against 25 ms of tracing — the CPU denoiser *was* the
//! frame time. `accumulate_and_denoise_resident` does all three in compute
//! passes and writes into a texture the caller owns.
//!
//! Three claims, one test each:
//!
//! * the device mean is the CPU running mean of the same raw samples,
//! * a keep mask restarts the pixels it zeroes and no others,
//! * the denoised frame is what `pathtrace::denoise` would have produced from
//!   the same `Film`.
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test gpu_history -- --ignored --nocapture
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use std::time::Instant;

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_primitives::make_sphere;
use vcad_kernel_raytrace::gpu::{
    GpuAreaLight, GpuCamera, GpuDenoiseParams, GpuRenderState, GpuScene, HistoryPipeline,
    RayTracePipeline, ResidentScene,
};
use vcad_kernel_raytrace::pathtrace::{self, PathTraceOptions};

const W: u32 = 64;
const H: u32 = 64;
const MAX_DEPTH: u32 = 3;

const EYE: [f32; 3] = [26.0, 24.0, 20.0];
const AT: [f32; 3] = [0.0, 0.0, 0.0];
const FOV_DEG: f32 = 45.0;

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
    let solid = make_sphere(6.0, 48);
    let mut s = GpuScene::from_brep(&solid).expect("scene packs");
    for m in &mut s.materials {
        m.color = [0.72, 0.70, 0.66, 1.0];
        m.metallic = 0.0;
        m.roughness = 0.4;
        m.clearcoat = 0.0;
        m.anisotropy = 0.0;
    }
    s.lights = vec![GpuAreaLight {
        center: [4.0, -3.0, 22.0, 0.0],
        u: [7.0, 0.0, 0.0, 0.0],
        v: [0.0, -7.0, 0.0, 0.0],
        emission: [3.0, 3.0, 3.0, 0.0],
    }];
    s
}

fn camera(w: u32, h: u32) -> GpuCamera {
    GpuCamera::new(EYE, AT, [0.0, 0.0, 1.0], FOV_DEG.to_radians(), w, h)
}

/// Path trace only: no edge overlay, no stylisation, no implicit floor, and
/// the analytic sky off, so the softbox is the whole of the lighting.
fn state(frame: u32) -> GpuRenderState {
    let mut s = GpuRenderState::new(frame);
    s.enable_edges = 0;
    s.stylize = 0;
    s.ground_enabled = 0;
    s.env_intensity = 0.0;
    s.max_depth = MAX_DEPTH;
    s
}

/// A caller-owned `Rgba8Unorm` storage texture, plus the readback machinery
/// that turns it back into pixels. A viewer would blit this instead.
struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    padded_bpr: u32,
    width: u32,
    height: u32,
}

impl Target {
    fn new(ctx: &GpuContext, width: u32, height: u32) -> Self {
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("history test target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let padded_bpr = (width * 4).div_ceil(256) * 256;
        let readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("history test readback"),
            size: (padded_bpr as u64) * (height as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            texture,
            view,
            readback,
            padded_bpr,
            width,
            height,
        }
    }

    fn pixels(&self, ctx: &GpuContext) -> Vec<u8> {
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bpr),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue.submit(Some(enc.finish()));

        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let data = slice.get_mapped_range().expect("mapped");
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        for row in 0..self.height {
            let start = (row * self.padded_bpr) as usize;
            out.extend_from_slice(&data[start..start + (self.width * 4) as usize]);
        }
        drop(data);
        self.readback.unmap();
        out
    }
}

/// The raw per-pass samples the history is going to be fed, taken through the
/// linear exit.
///
/// The shader's jitter and RNG are pure functions of the pixel and
/// `frame_index`, so a pass at frame `k` produces the same sample whichever
/// exit asked for it. That is what makes the two accumulations comparable at
/// all: this is the *same* sequence the device history will fold in.
fn raw_films(
    ctx: &GpuContext,
    pipeline: &RayTracePipeline,
    res: &mut ResidentScene,
    passes: u32,
) -> Vec<pathtrace::Film> {
    (1..=passes)
        .map(|f| {
            pollster::block_on(pipeline.render_resident_linear(ctx, res, &camera(W, H), state(f)))
                .expect("linear render")
        })
        .collect()
}

/// (a) The device history is the running mean of the same raw samples.
#[test]
#[ignore = "requires GPU"]
fn the_device_history_is_the_cpu_running_mean() {
    let Some(ctx) = ctx_or_skip("the_device_history_is_the_cpu_running_mean") else {
        return;
    };
    const PASSES: u32 = 24;

    let pipeline = RayTracePipeline::new(ctx).expect("pipeline");
    let history = HistoryPipeline::new(ctx).expect("history pipeline");
    let sc = scene();

    // The reference: the same samples, folded in on the CPU exactly as the
    // shader folds them.
    let mut ref_res = pipeline.resident_scene(ctx, &sc, W, H);
    let films = raw_films(ctx, &pipeline, &mut ref_res, PASSES);
    let n = (W * H) as usize;
    let mut mean = vec![0.0f32; n * 3];
    for (k, film) in films.iter().enumerate() {
        let count = (k + 1) as f32;
        for (m, &c) in mean.iter_mut().zip(&film.rgb) {
            *m += (c - *m) / count;
        }
    }

    // The device history over the same sequence, with an all-keep mask.
    let mut res = pipeline.resident_scene(ctx, &sc, W, H);
    let target = Target::new(ctx, W, H);
    let keep = vec![1u8; n];
    let denoise = GpuDenoiseParams::default();
    for f in 1..=PASSES {
        pipeline
            .accumulate_and_denoise_resident(
                ctx,
                &history,
                &mut res,
                &camera(W, H),
                state(f),
                &keep,
                &denoise,
                &target.view,
            )
            .expect("history pass");
    }
    let got = pollster::block_on(pipeline.read_history(ctx, &mut res))
        .expect("readback")
        .expect("a history exists after 24 passes");

    assert!(got.count.iter().all(|&c| c == PASSES));
    let worst = got
        .rgb
        .iter()
        .zip(&mean)
        .fold(0.0f32, |w, (&g, &m)| w.max((g - m).abs()));
    let bright = mean.iter().cloned().fold(0.0f32, f32::max);
    eprintln!(
        "[a] worst |device mean - CPU mean| {worst:.3e} over {PASSES} passes (peak {bright:.3})"
    );
    assert!(
        bright > 1e-3,
        "the reference mean is black ({bright:.6}) — nothing was rendered",
    );
    assert!(
        worst < 1e-4,
        "the device history is {worst:.3e} off the CPU running mean of the \
         same raw samples. Either the passes are not the same samples, or the \
         device fold is not `m += (c - m) / n`.",
    );
}

/// (b) A keep mask that zeroes a rectangle restarts exactly that rectangle.
#[test]
#[ignore = "requires GPU"]
fn a_keep_mask_restarts_only_the_pixels_it_zeroes() {
    let Some(ctx) = ctx_or_skip("a_keep_mask_restarts_only_the_pixels_it_zeroes") else {
        return;
    };
    const PASSES: u32 = 8;
    let (rx, ry, rw, rh) = (12u32, 20u32, 25u32, 17u32);

    let pipeline = RayTracePipeline::new(ctx).expect("pipeline");
    let history = HistoryPipeline::new(ctx).expect("history pipeline");
    let sc = scene();
    let mut res = pipeline.resident_scene(ctx, &sc, W, H);
    let target = Target::new(ctx, W, H);
    let denoise = GpuDenoiseParams::default();
    let n = (W * H) as usize;

    let all_keep = vec![1u8; n];
    for f in 1..=PASSES {
        pipeline
            .accumulate_and_denoise_resident(
                ctx,
                &history,
                &mut res,
                &camera(W, H),
                state(f),
                &all_keep,
                &denoise,
                &target.view,
            )
            .expect("history pass");
    }
    let before = pollster::block_on(pipeline.read_history(ctx, &mut res))
        .expect("readback")
        .expect("history");

    // One more pass, restarting the rectangle.
    let mut mask = vec![1u8; n];
    for y in ry..ry + rh {
        for x in rx..rx + rw {
            mask[(y * W + x) as usize] = 0;
        }
    }
    pipeline
        .accumulate_and_denoise_resident(
            ctx,
            &history,
            &mut res,
            &camera(W, H),
            state(PASSES + 1),
            &mask,
            &denoise,
            &target.view,
        )
        .expect("history pass");
    let after = pollster::block_on(pipeline.read_history(ctx, &mut res))
        .expect("readback")
        .expect("history");

    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) as usize;
            let inside = x >= rx && x < rx + rw && y >= ry && y < ry + rh;
            let want = if inside { 1 } else { PASSES + 1 };
            assert_eq!(
                after.count[i],
                want,
                "pixel ({x}, {y}) is {} the restarted rectangle and has count \
                 {} after {} passes, expected {want}",
                if inside { "inside" } else { "outside" },
                after.count[i],
                PASSES + 1,
            );
        }
    }
    // A restarted pixel's mean is this pass's sample, so it is (almost surely)
    // not what it was — the count is not the only thing that was reset.
    let mut moved = 0usize;
    for y in ry..ry + rh {
        for x in rx..rx + rw {
            let i = (y * W + x) as usize;
            if (0..3).any(|k| (after.rgb[i * 3 + k] - before.rgb[i * 3 + k]).abs() > 1e-6) {
                moved += 1;
            }
        }
    }
    assert!(
        moved > (rw * rh) as usize / 4,
        "only {moved} of {} restarted pixels changed value — the mean was not \
         reset with the count",
        rw * rh,
    );
}

/// (c) The denoised frame is what the CPU filter would have produced.
///
/// One pass, so every pixel's history is a single sample: the fade that scales
/// the filter down as a pixel converges is at full strength and the shader is
/// doing exactly what `pathtrace::denoise` does. The comparison is on the
/// tonemapped 8-bit frame, which is what the two are for.
#[test]
#[ignore = "requires GPU"]
fn the_device_denoise_matches_the_cpu_filter() {
    let Some(ctx) = ctx_or_skip("the_device_denoise_matches_the_cpu_filter") else {
        return;
    };
    let pipeline = RayTracePipeline::new(ctx).expect("pipeline");
    let history = HistoryPipeline::new(ctx).expect("history pipeline");
    let sc = scene();
    let n = (W * H) as usize;
    let denoise = GpuDenoiseParams::default();

    // The CPU reference: the same single raw sample, filtered and tonemapped
    // through the CPU tier.
    let mut ref_res = pipeline.resident_scene(ctx, &sc, W, H);
    let mut film = raw_films(ctx, &pipeline, &mut ref_res, 1).remove(0);
    let opts = PathTraceOptions {
        denoise_iters: denoise.iters,
        sigma_normal: denoise.sigma_normal,
        sigma_depth: denoise.sigma_depth,
        sigma_lum: denoise.sigma_lum,
        ..Default::default()
    };
    pathtrace::denoise(&mut film, &opts);
    let want = film.to_srgb8(denoise.exposure, false);

    // The device: the same pass, filtered and tonemapped on the GPU.
    let mut res = pipeline.resident_scene(ctx, &sc, W, H);
    let target = Target::new(ctx, W, H);
    pipeline
        .accumulate_and_denoise_resident(
            ctx,
            &history,
            &mut res,
            &camera(W, H),
            state(1),
            &[],
            &denoise,
            &target.view,
        )
        .expect("history pass");
    let got = target.pixels(ctx);

    // Tolerance. Both tiers run the same filter in f32, so the only
    // disagreement available is arithmetic: `exp` and `pow` are
    // implementation-defined to about a couple of ULP on either side, and the
    // 25 taps are summed in a different order. Through ACES and the sRGB
    // transfer — whose slope is at most 12.92, and only on the darkest
    // hundredth of the range — that is worth well under one code. One code of
    // slack is the quantiser's own; two is generous, and a real disagreement
    // about a filter weight moves whole regions by tens.
    let mut worst = 0u8;
    let mut sum = 0.0f64;
    let mut over = 0usize;
    for i in 0..n {
        for k in 0..3 {
            let d = got[i * 4 + k].abs_diff(want[i * 4 + k]);
            worst = worst.max(d);
            sum += d as f64;
            if d > 2 {
                over += 1;
            }
        }
    }
    let mean = sum / (n * 3) as f64;
    eprintln!("[c] worst channel difference {worst}/255, mean {mean:.4}, {over} channels over 2");
    assert!(
        want.chunks(4).any(|p| p[0] > 8),
        "the CPU reference frame is black — nothing was rendered",
    );
    assert_eq!(
        over, 0,
        "{over} channels differ from the CPU filter by more than 2/255 (worst \
         {worst}). The device à-trous pass is not the same filter as \
         `pathtrace::denoise`.",
    );
    assert!(mean < 0.2, "mean channel difference {mean:.4} is too large");
}

/// What the device history costs per pass, against the CPU filter it replaces.
/// Not an assertion — a measurement, printed with `--nocapture`.
#[test]
#[ignore = "requires GPU"]
fn measure_the_history_pass() {
    let Some(ctx) = ctx_or_skip("measure_the_history_pass") else {
        return;
    };
    const N: u32 = 512;
    const M: u32 = 288;
    const REPS: u32 = 30;

    let pipeline = RayTracePipeline::new(ctx).expect("pipeline");
    let history = HistoryPipeline::new(ctx).expect("history pipeline");
    let sc = scene();
    let mut res = pipeline.resident_scene(ctx, &sc, N, M);
    let target = Target::new(ctx, N, M);
    let cam = GpuCamera::new(EYE, AT, [0.0, 0.0, 1.0], FOV_DEG.to_radians(), N, M);
    let denoise = GpuDenoiseParams::default();

    let st = |f: u32| {
        let mut s = state(f);
        s.frame_index = f;
        s
    };

    // Warm up, then measure.
    for f in 1..=2 {
        pipeline
            .accumulate_and_denoise_resident(
                ctx,
                &history,
                &mut res,
                &cam,
                st(f),
                &[],
                &denoise,
                &target.view,
            )
            .expect("pass");
    }
    let t = Instant::now();
    for f in 0..REPS {
        pipeline
            .accumulate_and_denoise_resident(
                ctx,
                &history,
                &mut res,
                &cam,
                st(f + 3),
                &[],
                &denoise,
                &target.view,
            )
            .expect("pass");
    }
    // The submit is asynchronous, so close the loop with a readback before
    // stopping the clock: what is being timed is the work, not the enqueue.
    let _ = target.pixels(ctx);
    let elapsed = t.elapsed();
    eprintln!(
        "[measure] trace + accumulate + {} a-trous iterations + tonemap, no \
         readback, at {N}x{M}: {:.2} ms/pass",
        denoise.iters,
        elapsed.as_secs_f64() * 1000.0 / REPS as f64,
    );

    // The CPU filter alone, on a film of the same size, for scale.
    let mut film = pathtrace::Film::new(N, M);
    for (i, d) in film.depth.iter_mut().enumerate() {
        *d = 10.0 + (i % 7) as f32 * 0.01;
    }
    for a in film.albedo.iter_mut() {
        *a = 0.5;
    }
    for v in film.variance.iter_mut() {
        *v = 0.01;
    }
    let opts = PathTraceOptions::default();
    pathtrace::denoise(&mut film, &opts);
    let t = Instant::now();
    for _ in 0..REPS {
        pathtrace::denoise(&mut film, &opts);
    }
    eprintln!(
        "[measure] pathtrace::denoise on the CPU at {N}x{M}: {:.2} ms/call",
        t.elapsed().as_secs_f64() * 1000.0 / REPS as f64,
    );
}
