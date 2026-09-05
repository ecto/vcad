//! GPU timing for next-event estimation cost vs light count.
#[cfg(not(all(feature = "gpu", not(target_arch = "wasm32"))))]
fn main() {
    eprintln!("build with --features gpu");
}

#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
mod bench {
    use std::time::Instant;
    use vcad_kernel_gpu::GpuContext;
    use vcad_kernel_primitives::make_sphere;
    use vcad_kernel_raytrace::gpu::*;

    pub fn run() {
        let n: u32 = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);
        let (w, h) = (512u32, 512u32);
        let ctx = pollster::block_on(GpuContext::init()).expect("gpu");
        let mut scene = GpuScene::from_brep(&make_sphere(6.0, 48)).expect("pack");
        scene.lights = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                GpuAreaLight {
                    center: [a.cos() * 16.0, a.sin() * 16.0, 20.0, 0.0],
                    u: [5.0, 0.0, 0.0, 0.0],
                    v: [0.0, -5.0, 0.0, 0.0],
                    emission: [1.0 + i as f32 * 0.2; 4],
                }
            })
            .collect();
        let pipeline = RayTracePipeline::new(ctx).expect("pipeline");
        let cam = GpuCamera::new(
            [26.0, 24.0, 20.0],
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            45f32.to_radians(),
            w,
            h,
        );
        let mk = |frame: u32| {
            let mut s = GpuRenderState::new(frame);
            s.enable_edges = 0;
            s.stylize = 0;
            s.max_depth = 4;
            s
        };
        let mut best = f64::INFINITY;
        for _ in 0..3 {
            let mut accum = None;
            let t = Instant::now();
            for frame in 1..=64u32 {
                let (_, buf) = pollster::block_on(pipeline.render_with_render_state(
                    ctx,
                    &scene,
                    &cam,
                    w,
                    h,
                    accum,
                    mk(frame),
                ))
                .expect("render");
                accum = Some(buf);
            }
            best = best.min(t.elapsed().as_secs_f64());
        }
        println!("{n} lights: 64 one-shot frames at {w}x{h} in {best:.3}s");

        // The same work through the resident path: nothing reallocated between
        // frames.
        let mut res = pipeline.resident_scene(ctx, &scene, w, h);
        let mut best_r = f64::INFINITY;
        for _ in 0..3 {
            res.reset_accumulation(ctx);
            let t = Instant::now();
            for frame in 1..=64u32 {
                let _ =
                    pollster::block_on(pipeline.render_resident(ctx, &mut res, &cam, mk(frame)))
                        .expect("render");
            }
            best_r = best_r.min(t.elapsed().as_secs_f64());
        }
        println!("{n} lights: 64 resident frames at {w}x{h} in {best_r:.3}s");

        // And with the instances re-placed every frame, which is what an
        // animation actually does.
        let mut best_p = f64::INFINITY;
        for _ in 0..3 {
            res.reset_accumulation(ctx);
            let t = Instant::now();
            for frame in 1..=64u32 {
                let placed = scene.placed(&vcad_kernel_math::Transform::translation(
                    frame as f64 * 0.01,
                    0.0,
                    0.0,
                ));
                res.update_scene(ctx, &placed);
                let _ =
                    pollster::block_on(pipeline.render_resident(ctx, &mut res, &cam, mk(frame)))
                        .expect("render");
            }
            best_p = best_p.min(t.elapsed().as_secs_f64());
        }
        println!("{n} lights: 64 resident frames + re-place at {w}x{h} in {best_p:.3}s");
    }
}

#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
fn main() {
    bench::run();
}
