//! `GpuScene` as a kosm-render geometry module.
//!
//! Five storage buffers — surfaces, faces, BVH nodes, trim vertices, inner
//! loop descriptors — bound at 1..=5, and `brep.wgsl` implementing
//! `trace_scene` / `hit_normal` / `hit_tangent` / `hit_material_index` /
//! `hit_orientation` over them. See `kosm_render::gpu::geometry` for the
//! contract and the binding split.

use kosm_render::gpu::{storage_entry, GeometryModule, GeometrySlab, GpuGeometry, SceneRef};

use super::buffers::GpuScene;

/// The BRep geometry module.
pub struct BrepGeometry;

impl BrepGeometry {
    /// The WGSL and bindings a [`kosm_render::gpu::RayTracePipeline`] needs to
    /// trace trimmed analytic faces.
    pub fn module() -> GeometryModule {
        GeometryModule {
            wgsl: format!(
                "{}\n{}",
                super::shaders::SURFACE_SHADER,
                super::shaders::BREP_SHADER
            ),
            layout: (1..=5).map(storage_entry).collect(),
        }
    }
}

impl GpuGeometry for GpuScene {
    fn slabs(&self) -> Vec<GeometrySlab<'_>> {
        vec![
            GeometrySlab {
                label: "Surfaces",
                bytes: slice_or_pad(&self.surfaces),
            },
            GeometrySlab {
                label: "Faces",
                bytes: slice_or_pad(&self.faces),
            },
            GeometrySlab {
                label: "BVH Nodes",
                bytes: slice_or_pad(&self.bvh_nodes),
            },
            GeometrySlab {
                label: "Trim Vertices",
                bytes: slice_or_pad(&self.trim_verts),
            },
            GeometrySlab {
                label: "Inner Loop Descs",
                bytes: slice_or_pad(&self.inner_loop_descs),
            },
        ]
    }
}

/// A zero-length storage buffer is invalid, so an empty slab is padded to one
/// zeroed element. The shader's own counts are what it loops over, so the
/// padding is never read.
static ZEROS: [u8; 256] = [0; 256];

fn slice_or_pad<T: bytemuck::Pod>(v: &[T]) -> &[u8] {
    if v.is_empty() {
        &ZEROS[..std::mem::size_of::<T>()]
    } else {
        bytemuck::cast_slice(v)
    }
}

impl<'a> From<&'a GpuScene> for SceneRef<'a> {
    fn from(s: &'a GpuScene) -> Self {
        SceneRef {
            geometry: s,
            materials: &s.materials,
            lights: &s.lights,
            environment: s.environment.as_ref(),
        }
    }
}
