//! A scene held on the GPU across passes must render what the one-shot path
//! renders.
//!
//! `ResidentScene` exists to stop reallocating every buffer per frame, and the
//! only interesting question about it is whether the frames come out the same.
//! They must, byte for byte: the shader is unchanged and the buffers hold the
//! same bytes, so any difference means something was written to the wrong
//! place or not written at all.
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test resident_scene -- --ignored
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_sphere};
use vcad_kernel_raytrace::gpu::{GpuAreaLight, GpuCamera, GpuRenderState, GpuScene};

const W: u32 = 64;
const H: u32 = 64;

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

fn state(frame: u32) -> GpuRenderState {
    let mut s = GpuRenderState::new(frame);
    s.enable_edges = 0;
    s.stylize = 0;
    s.ground_enabled = 0;
    s
}

fn camera() -> GpuCamera {
    GpuCamera::new(
        [26.0, 24.0, 20.0],
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        45.0_f32.to_radians(),
        W,
        H,
    )
}

fn panel(center: [f32; 3], emission: f32) -> GpuAreaLight {
    GpuAreaLight {
        center: [center[0], center[1], center[2], 0.0],
        u: [6.0, 0.0, 0.0, 0.0],
        v: [0.0, -6.0, 0.0, 0.0],
        emission: [emission, emission, emission, 0.0],
    }
}

fn lit_scene(solid: &vcad_kernel_primitives::BRepSolid) -> GpuScene {
    let mut s = GpuScene::from_brep(solid).expect("scene packs");
    s.lights = vec![panel([3.0, -2.0, 22.0], 2.4), panel([-8.0, 6.0, 18.0], 0.6)];
    s
}

/// The resident path and the one-shot path must agree exactly, on the first
/// pass and after re-placing the geometry.
#[test]
#[ignore = "requires GPU"]
fn a_resident_scene_renders_what_the_one_shot_path_renders() {
    let Some(ctx) = ctx_or_skip("a_resident_scene_renders_what_the_one_shot_path_renders") else {
        return;
    };
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline creation");
    let solid = make_sphere(6.0, 32);
    let scene = lit_scene(&solid);
    let cam = camera();

    let mut resident = pipeline.resident_scene(ctx, &scene, W, H);
    assert_eq!(resident.size(), (W, H));

    for pose in [
        Transform::identity(),
        Transform::translation(4.0, -3.0, 2.0),
        Transform::translation(-6.0, 1.0, 0.0).then(&Transform::rotation_z(0.9)),
    ] {
        let placed = scene.placed(&pose);
        resident.update_scene(ctx, &placed);
        resident.reset_accumulation(ctx);

        let got = pollster::block_on(pipeline.render_resident(ctx, &mut resident, &cam, state(1)))
            .expect("resident render");
        let (want, _) = pollster::block_on(pipeline.render_with_render_state(
            ctx,
            &placed,
            &cam,
            W,
            H,
            None,
            state(1),
        ))
        .expect("one-shot render");
        assert_eq!(
            got, want,
            "the resident scene and the one-shot path disagree after a re-place \
             — a buffer was written to the wrong place, or not rewritten at all",
        );
    }
}

/// Accumulation must survive across passes on a resident scene: frame 2 has to
/// see frame 1's average, or progressive refinement silently does nothing.
#[test]
#[ignore = "requires GPU"]
fn accumulation_persists_across_resident_passes() {
    let Some(ctx) = ctx_or_skip("accumulation_persists_across_resident_passes") else {
        return;
    };
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline creation");
    let solid = make_sphere(6.0, 32);
    let scene = lit_scene(&solid);
    let cam = camera();
    let mut resident = pipeline.resident_scene(ctx, &scene, W, H);

    let one = pollster::block_on(pipeline.render_resident(ctx, &mut resident, &cam, state(1)))
        .expect("render");
    let mut many = one.clone();
    for f in 2..=24u32 {
        many = pollster::block_on(pipeline.render_resident(ctx, &mut resident, &cam, state(f)))
            .expect("render");
    }
    assert_ne!(one, many, "24 accumulated frames look exactly like one");

    // And resetting really does start over: the next frame 1 is the first
    // frame again.
    resident.reset_accumulation(ctx);
    let again = pollster::block_on(pipeline.render_resident(ctx, &mut resident, &cam, state(1)))
        .expect("render");
    assert_eq!(
        one, again,
        "after reset_accumulation, frame 1 must be frame 1 again",
    );
}

/// Growing the scene past what is allocated must still work — that is the path
/// that reallocates a buffer and has to rebuild the bind group with it.
#[test]
#[ignore = "requires GPU"]
fn a_resident_scene_can_grow() {
    let Some(ctx) = ctx_or_skip("a_resident_scene_can_grow") else {
        return;
    };
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline creation");
    let cam = camera();

    // Six planes, then a 32-segment sphere: strictly more of everything.
    let small = lit_scene(&make_cube(8.0, 8.0, 8.0));
    let big = lit_scene(&make_sphere(6.0, 32));
    let mut resident = pipeline.resident_scene(ctx, &small, W, H);
    let _ = pollster::block_on(pipeline.render_resident(ctx, &mut resident, &cam, state(1)))
        .expect("render");

    resident.update_scene(ctx, &big);
    resident.reset_accumulation(ctx);
    let got = pollster::block_on(pipeline.render_resident(ctx, &mut resident, &cam, state(1)))
        .expect("render");
    let (want, _) = pollster::block_on(pipeline.render_with_render_state(
        ctx,
        &big,
        &cam,
        W,
        H,
        None,
        state(1),
    ))
    .expect("one-shot render");
    assert_eq!(
        got, want,
        "a scene that outgrew its buffers renders wrong — the bind group was \
         not rebuilt around the reallocated ones",
    );
}

/// Swapping the rig must go through the power table again, not reuse the old
/// one, and must not disturb the geometry.
#[test]
#[ignore = "requires GPU"]
fn set_lights_reuploads_the_power_table() {
    let Some(ctx) = ctx_or_skip("set_lights_reuploads_the_power_table") else {
        return;
    };
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline creation");
    let solid = make_sphere(6.0, 32);
    let scene = lit_scene(&solid);
    let cam = camera();
    let mut resident = pipeline.resident_scene(ctx, &scene, W, H);

    let rig = vec![
        panel([3.0, -2.0, 22.0], 0.4),
        panel([-8.0, 6.0, 18.0], 3.0),
        panel([0.0, 12.0, 16.0], 1.1),
    ];
    resident.set_lights(ctx, &rig);
    resident.reset_accumulation(ctx);
    let got = pollster::block_on(pipeline.render_resident(ctx, &mut resident, &cam, state(1)))
        .expect("render");

    let mut swapped = GpuScene::from_brep(&solid).expect("scene packs");
    swapped.lights = rig;
    let (want, _) = pollster::block_on(pipeline.render_with_render_state(
        ctx,
        &swapped,
        &cam,
        W,
        H,
        None,
        state(1),
    ))
    .expect("one-shot render");
    assert_eq!(got, want, "set_lights did not reach the shader intact");
}

/// The no-readback entry point must put the same pixels in a texture the
/// caller owns.
#[test]
#[ignore = "requires GPU"]
fn rendering_into_a_caller_texture_matches_the_readback_path() {
    let Some(ctx) = ctx_or_skip("rendering_into_a_caller_texture_matches_the_readback_path") else {
        return;
    };
    let pipeline = vcad_kernel_raytrace::gpu::brep_pipeline(ctx).expect("pipeline creation");
    let scene = lit_scene(&make_sphere(6.0, 32));
    let cam = camera();
    let mut resident = pipeline.resident_scene(ctx, &scene, W, H);

    let want = pollster::block_on(pipeline.render_resident(ctx, &mut resident, &cam, state(1)))
        .expect("render");
    resident.reset_accumulation(ctx);

    let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("caller target"),
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
    });
    let view = target.create_view(&Default::default());
    pipeline.render_resident_into(ctx, &mut resident, &cam, state(1), &view);

    // Pull the caller's texture back only to check it — the point of the entry
    // point is that a real caller would not.
    let padded = (W * 4).div_ceil(256) * 256;
    let buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("caller readback"),
        size: (padded * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit(Some(enc.finish()));
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
    let data = slice.get_mapped_range().expect("map");
    let mut got = Vec::with_capacity((W * H * 4) as usize);
    for row in 0..H {
        let s = (row * padded) as usize;
        got.extend_from_slice(&data[s..s + (W * 4) as usize]);
    }
    drop(data);
    buf.unmap();

    assert_eq!(
        got, want,
        "the storage-texture entry point and the readback one disagree",
    );
}
