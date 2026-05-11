//! GPU raytrace smoke tests.
//!
//! These tests caught a class of bug that almost shipped: a WGSL compile
//! error in `heat_color()` (parameter `t` was shadowed by a `let t`) made
//! the shader module invalid, which silently invalidated all three compute
//! pipelines. `dispatch_workgroups` then no-op'd and the readback returned
//! a buffer of the correct size filled with zeros — no error propagated to
//! Rust. See PR #185 for the war story.
//!
//! Both tests are `#[ignore]`-tagged because they need a real GPU and
//! aren't appropriate for CI by default; run locally with:
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test gpu_smoke -- --ignored
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_primitives::make_cube;
use vcad_kernel_raytrace::gpu::{GpuCamera, GpuScene, RayTracePipeline};

/// Skip with a clear message when no adapter is available (e.g. macOS
/// native where GpuContext only requests WebGPU + GL backends).
fn ctx_or_skip(test_name: &str) -> Option<&'static GpuContext> {
    match pollster::block_on(GpuContext::init()) {
        Ok(ctx) => Some(ctx),
        Err(GpuError::NoAdapter) => {
            eprintln!("[{test_name}] skipped: no compatible GPU adapter");
            None
        }
        Err(e) => panic!("GPU init failed unexpectedly: {e}"),
    }
}

/// Pipeline construction must not emit WebGPU validation errors. This
/// directly catches shader / bind-group-layout / pipeline-layout
/// regressions of the kind that hid the all-zeros bug for too long.
#[test]
#[ignore = "requires GPU"]
fn pipeline_construction_is_validation_clean() {
    let Some(ctx) = ctx_or_skip("pipeline_construction_is_validation_clean") else {
        return;
    };

    ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let _pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");
    let err = pollster::block_on(ctx.device.pop_error_scope());

    assert!(
        err.is_none(),
        "RayTracePipeline construction emitted a WebGPU validation error: {err:?}\n\
         This usually means the WGSL shader has a compile error or the bind \
         group layout exceeds adapter limits. The pipeline will silently \
         no-op every render until fixed.",
    );
}

/// A render against a real BRep solid must produce at least one
/// non-zero pixel. If the shader compiles but the pipeline never
/// actually runs (e.g. device error state from an earlier failure),
/// the readback is all zeros — the exact symptom from PR #185.
#[test]
#[ignore = "requires GPU"]
fn render_cube_produces_non_zero_pixels() {
    let Some(ctx) = ctx_or_skip("render_cube_produces_non_zero_pixels") else {
        return;
    };
    let pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");

    let cube = make_cube(10.0, 10.0, 10.0);
    let scene = GpuScene::from_brep(&cube).expect("scene upload");

    let w = 64u32;
    let h = 64u32;
    let camera = GpuCamera::new(
        [30.0, 30.0, 30.0], // position — look at the cube from the +XYZ corner
        [5.0, 5.0, 5.0],    // target — center of the cube
        [0.0, 0.0, 1.0],    // up (Z-up, the kernel convention)
        45.0_f32.to_radians(),
        w,
        h,
    );

    let pixels = pollster::block_on(pipeline.render(ctx, &scene, &camera, w, h)).expect("render");

    assert_eq!(pixels.len() as u32, w * h * 4, "pixel buffer wrong size");

    let non_zero = pixels
        .chunks(4)
        .filter(|p| p.iter().any(|&b| b != 0))
        .count();
    assert!(
        non_zero > 0,
        "render returned all-zero pixels — the pipeline ran but wrote nothing. \
         This is the failure mode from PR #185: a silent WebGPU validation \
         error (bad shader, oversized bind group, etc.) puts the device in \
         an error state and dispatch_workgroups no-ops. Check the \
         `on_uncaptured_error` log for the actual cause.",
    );
}
