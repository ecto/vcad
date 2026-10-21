//! Meshes over the old 1M-triangle GPU cap (#971).
//!
//! The cap used to refuse any mesh scene past 1,000,000 triangles, which sent
//! real assemblies (perforated grilles) to the CPU tracer. The device's
//! `max_storage_buffer_binding_size` is the real limit; these tests pin that
//! a 1.2M-triangle scene packs, validates against a desktop-sized binding,
//! is refused against a small one, and (when an adapter exists) actually
//! uploads into GPU buffers.

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_raytrace::gpu::{GpuGeometry, GpuScene, GpuSceneError};
use vcad_kernel_raytrace::{BrepBvh, Bvh};
use vcad_kernel_tessellate::TriangleMesh;

/// A flat grid of `n * n * 2` triangles.
fn grid(n: usize) -> TriangleMesh {
    let mut vertices = Vec::with_capacity((n + 1) * (n + 1) * 3);
    for j in 0..=n {
        for i in 0..=n {
            vertices.extend_from_slice(&[i as f32, j as f32, 0.0]);
        }
    }
    let mut indices = Vec::with_capacity(n * n * 6);
    let w = (n + 1) as u32;
    for j in 0..n as u32 {
        for i in 0..n as u32 {
            let a = j * w + i;
            indices.extend_from_slice(&[a, a + 1, a + w + 1, a, a + w + 1, a + w]);
        }
    }
    let normals = vec![0.0; vertices.len()];
    TriangleMesh {
        vertices,
        indices,
        normals,
        face_kinds: Vec::new(),
        face_ids: Vec::new(),
    }
}

/// 775² × 2 = 1,201,250 triangles: comfortably over the old cap.
fn big_scene() -> GpuScene {
    let mesh = grid(775);
    assert!(mesh.indices.len() / 3 > 1_000_000);
    let bvh = Bvh::build_mesh(&mesh);
    GpuScene::from_mesh_bvh(&bvh).expect("a mesh over 1M triangles must pack")
}

#[test]
fn mesh_over_old_cap_packs_and_validates_against_the_device_binding() {
    let scene = big_scene();
    assert!(scene.surfaces.len() > 1_000_000);
    // A desktop discrete GPU's binding (2 GiB) holds it ...
    scene
        .validate(Some(2_147_483_644))
        .expect("1.2M triangles fit a 2 GiB binding");
    // ... a WebGPU-default 128 MiB binding does not, and says so.
    assert!(matches!(
        scene.validate(Some(128 << 20)),
        Err(GpuSceneError::ExceedsDeviceBinding { .. })
    ));
}

#[test]
fn mesh_over_old_cap_uploads_when_an_adapter_exists() {
    let ctx = match pollster::block_on(GpuContext::init()) {
        Ok(ctx) => ctx,
        Err(GpuError::NoAdapter) => {
            eprintln!("skipped: no compatible GPU adapter");
            return;
        }
        Err(e) => panic!("GPU init failed unexpectedly: {e}"),
    };
    let scene = big_scene();
    let cap = ctx.device.limits().max_storage_buffer_binding_size;
    if scene.validate(Some(cap)).is_err() {
        eprintln!("skipped: this adapter's {cap}-byte binding is below the scene");
        return;
    }
    for slab in scene.slabs() {
        let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(slab.label),
            size: slab.bytes.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&buf, 0, slab.bytes);
        let err = pollster::block_on(scope.pop());
        assert!(err.is_none(), "{} upload failed: {err:?}", slab.label);
    }
}
