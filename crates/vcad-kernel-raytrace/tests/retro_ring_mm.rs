//! Retro incidence in a sealed room modelled in *millimetres*.
//!
//! `retro_ring.rs` builds the same room in metre-sized units and finds nothing.
//! Kosm's court is authored in millimetres: a 26 m x 17 m x 9 m hall is
//! coordinates up to 26000, where one f32 ulp is about 2e-3. The shader used
//! to lift every secondary and shadow ray off the surface by a fixed
//! `n * 1e-4` and to reject self-hits below a fixed `t > 1e-6`. Both are far
//! under an ulp there: `p + n * 1e-4` rounds straight back to `p`, the ray
//! starts *on* the wall, and the wall occludes itself. The pixels that suffer
//! most are the ones looking squarely at the far wall, which is exactly where
//! the court showed its dark disc.
//!
//! Same room, same panels, same radiance as `retro_ring.rs`, scaled by 1000.
//! The CPU tier is f64, where 1e-5 at a coordinate of 2.6e4 is still ten
//! million ulps clear, so it stays correct and makes the reference.
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test retro_ring_mm \
//!     -- --ignored --nocapture --test-threads=1
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use std::sync::Arc;

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::make_cube;
use vcad_kernel_raytrace::gpu::{
    GpuAreaLight, GpuCamera, GpuDenoiseParams, GpuRenderState, GpuScene, HistoryPipeline,
};
use vcad_kernel_raytrace::pathtrace::{
    self, AreaLight, Camera, Environment, GradientEnv, Object, PathTraceOptions, Pbr,
};
use vcad_kernel_raytrace::Bvh;

const N: u32 = 192;
const PASSES: u32 = 96;
const MAX_DEPTH: u32 = 6;
const RR_START: u32 = 3;
const CLAMP: f32 = 12.0;
const ITERS: u32 = 5;

/// Exposure chosen so the wall lands near 105/255 — where the court's CPU
/// render reads, and well off the tonemap's shoulder, so a 30% dip would be
/// thirty codes and not a rounding.
const EXPOSURE: f32 = 0.2;

/// A tall room, so no ceiling panel is in frame and this test is about the
/// wall alone.
const RX: f64 = 26_000.0;
const RY: f64 = 17_000.0;
const RZ: f64 = 9_000.0;

/// Squarely down the long axis: the view ray meets the far wall at normal
/// incidence at the exact centre of the frame.
const EYE: [f32; 3] = [13_000.0, 8_500.0, 1_700.0];
const AT: [f32; 3] = [13_000.0, 17_000.0, 1_700.0];
const FOV: f32 = 40.0;

const BASE: [f32; 3] = [0.62, 0.6, 0.58];
const ROUGH: f32 = 0.85;
const EMISSION: f32 = 18.0;

/// Widest annulus we measure, in pixels. Half the frame's half-width, so every
/// band is wall.
const BANDS: u32 = 12;

/// 1.5%: the reported disc was a 30% dip.
const TOL: f64 = 0.015;

fn panels() -> Vec<AreaLight> {
    let mut out = Vec::new();
    for row in 0..5 {
        for col in 0..2 {
            out.push(AreaLight {
                center: Point3::new(
                    RX * (0.3 + 0.4 * col as f64),
                    RY * (0.12 + 0.19 * row as f64),
                    RZ - 500.0,
                ),
                u: Vec3::new(1_000.0, 0.0, 0.0),
                v: Vec3::new(0.0, -1_000.0, 0.0),
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
        env: Environment::constant([0.0; 3]),
        ground: None,
    }
}

fn state(frame: u32) -> GpuRenderState {
    let mut s = GpuRenderState::new(frame);
    s.enable_edges = 0;
    s.stylize = 0;
    s.ground_enabled = 0;
    s.max_depth = MAX_DEPTH;
    s.rr_start = RR_START;
    s.firefly_clamp = CLAMP;
    s.set_camera_visible_lights(true);
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

fn cpu_opts() -> PathTraceOptions {
    PathTraceOptions {
        spp: PASSES,
        max_depth: MAX_DEPTH,
        rr_start: RR_START,
        firefly_clamp: Some(CLAMP),
        denoise: false,
        show_background: false,
        denoise_iters: ITERS,
        ..Default::default()
    }
}

/// A caller-owned `Rgba8Unorm` storage texture, plus the readback that turns
/// it back into pixels. A viewer would blit this instead.
struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    padded_bpr: u32,
}

impl Target {
    fn new(ctx: &GpuContext) -> Self {
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("retro ring mm target"),
            size: wgpu::Extent3d {
                width: N,
                height: N,
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
        let padded_bpr = (N * 4).div_ceil(256) * 256;
        let readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("retro ring mm readback"),
            size: (padded_bpr as u64) * (N as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            texture,
            view,
            readback,
            padded_bpr,
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
                    rows_per_image: Some(N),
                },
            },
            wgpu::Extent3d {
                width: N,
                height: N,
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
        let mut out = Vec::with_capacity((N * N * 4) as usize);
        for row in 0..N {
            let start = (row * self.padded_bpr) as usize;
            out.extend_from_slice(&data[start..start + (N * 4) as usize]);
        }
        drop(data);
        self.readback.unmap();
        out
    }
}

/// Mean of a per-pixel scalar over the annulus `r0 <= r < r1` about the frame
/// centre — the retro-incidence pixel.
fn annulus(v: &[f64], r0: f32, r1: f32) -> f64 {
    let c = (N as f32 - 1.0) * 0.5;
    let (mut s, mut k) = (0.0, 0usize);
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let r = (dx * dx + dy * dy).sqrt();
            if r >= r0 && r < r1 {
                s += v[(y * N + x) as usize];
                k += 1;
            }
        }
    }
    assert!(k > 0, "empty annulus {r0}..{r1}");
    s / k as f64
}

fn luma_linear(rgb: &[f32]) -> Vec<f64> {
    (0..(N * N) as usize)
        .map(|i| {
            0.2126 * rgb[i * 3] as f64
                + 0.7152 * rgb[i * 3 + 1] as f64
                + 0.0722 * rgb[i * 3 + 2] as f64
        })
        .collect()
}

fn luma_srgb8(px: &[u8]) -> Vec<f64> {
    (0..(N * N) as usize)
        .map(|i| {
            0.2126 * px[i * 4] as f64
                + 0.7152 * px[i * 4 + 1] as f64
                + 0.0722 * px[i * 4 + 2] as f64
        })
        .collect()
}

/// Compare two radial profiles band by band, and the shape of each — a ring is
/// a *local* dip, so the centre-to-outer ratio is what it moves.
fn compare(label: &str, gpu: &[f64], cpu: &[f64], tol: f64) {
    let step = N as f32 / (4.0 * BANDS as f32);
    let mut g_bands = Vec::new();
    let mut c_bands = Vec::new();
    for b in 0..BANDS {
        let (r0, r1) = (b as f32 * step, (b + 1) as f32 * step);
        let (g, c) = (annulus(gpu, r0, r1), annulus(cpu, r0, r1));
        eprintln!(
            "[{label}] r {r0:>5.1}..{r1:>5.1}: GPU {g:8.4} CPU {c:8.4} ({:.2}%)",
            100.0 * g / c
        );
        g_bands.push(g);
        c_bands.push(c);
    }
    for b in 0..BANDS as usize {
        let rel = (g_bands[b] - c_bands[b]).abs() / c_bands[b];
        assert!(
            rel < tol,
            "[{label}] annulus {b} reads {:.4} on the GPU against {:.4} on the \
             CPU, {:.1}% apart — a radial artefact the CPU does not have",
            g_bands[b],
            c_bands[b],
            rel * 100.0,
        );
    }
    // The shape, independent of any overall scale: a dip at the centre shows
    // here even if both tiers were uniformly off.
    let g_shape = g_bands[0] / g_bands[BANDS as usize - 1];
    let c_shape = c_bands[0] / c_bands[BANDS as usize - 1];
    eprintln!("[{label}] centre/outer: GPU {g_shape:.4} CPU {c_shape:.4}");
    assert!(
        (g_shape - c_shape).abs() / c_shape < tol,
        "[{label}] the GPU's centre-to-outer ratio is {g_shape:.4} against the \
         CPU's {c_shape:.4}: a ring at retro incidence",
    );
}

/// The raw integrator: no filter, no history, one linear sample per pass
/// averaged by hand.
#[test]
#[ignore = "requires GPU"]
fn the_raw_trace_at_mm_scale_has_no_ring_at_retro_incidence() {
    let Some(ctx) = ctx_or_skip("the_raw_trace_at_mm_scale_has_no_ring_at_retro_incidence") else {
        return;
    };
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline");
    let sc = gpu_scene();
    let mut res = pipeline.resident_scene(ctx, &sc, N, N);
    let cam = GpuCamera::new(EYE, AT, [0.0, 0.0, 1.0], FOV.to_radians(), N, N);

    let mut sum = vec![0.0f32; (N * N * 3) as usize];
    for f in 1..=PASSES {
        let film =
            pollster::block_on(pipeline.render_resident_linear(ctx, &mut res, &cam, state(f)))
                .expect("linear render");
        for (s, c) in sum.iter_mut().zip(&film.rgb) {
            *s += *c;
        }
    }
    for s in &mut sum {
        *s /= PASSES as f32;
    }

    let cpu = pathtrace::render(&cpu_scene(), &cpu_camera(), N, N, &cpu_opts());
    compare("raw mm", &luma_linear(&sum), &luma_linear(&cpu.rgb), TOL);
}

/// The whole device path a viewer drives: raw sample, history, a-trous,
/// resolve — against the CPU's `render` + `denoise` + `to_srgb8`.
use vcad_kernel_raytrace::BrepBvh;
#[test]
#[ignore = "requires GPU"]
fn the_denoised_device_frame_at_mm_scale_has_no_ring_at_retro_incidence() {
    let Some(ctx) =
        ctx_or_skip("the_denoised_device_frame_at_mm_scale_has_no_ring_at_retro_incidence")
    else {
        return;
    };
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline");
    let history = HistoryPipeline::new(ctx).expect("history pipeline");
    let sc = gpu_scene();
    let mut res = pipeline.resident_scene(ctx, &sc, N, N);
    let cam = GpuCamera::new(EYE, AT, [0.0, 0.0, 1.0], FOV.to_radians(), N, N);
    let target = Target::new(ctx);
    let denoise = GpuDenoiseParams {
        iters: ITERS,
        exposure: EXPOSURE,
        ..Default::default()
    };
    for f in 1..=PASSES {
        pipeline
            .accumulate_and_denoise_resident(
                ctx,
                &history,
                &mut res,
                &cam,
                state(f),
                &[],
                &denoise,
                &target.view,
            )
            .expect("history pass");
    }

    let mut film = pathtrace::render(&cpu_scene(), &cpu_camera(), N, N, &cpu_opts());
    pathtrace::denoise(
        &mut film,
        &PathTraceOptions {
            denoise_iters: ITERS,
            ..Default::default()
        },
    );
    let want = film.to_srgb8(EXPOSURE, false);

    // In display codes: the wall lands near 105/255, so this tolerance is
    // about two codes, and the reported ring was thirty.
    compare(
        "denoised mm",
        &luma_srgb8(&target.pixels(ctx)),
        &luma_srgb8(&want),
        0.02,
    );
}
