//! `vcad_cam_outline_from_scene`: the one CAM entry point that cannot be
//! shared, because only the FFI holds a scene.
//!
//! Everything about *sectioning* lives in [`vcad_cam_api`]. What is here is
//! the part only this crate can do: reach a part of an evaluated scene and
//! decide which of its two meshes to hand over.
//!
//! # Which mesh gets sectioned, and why it matters
//!
//! A scene part carries two meshes' worth of geometry: the `EvaluatedMesh` the
//! app renders, which came from [`vcad_kernel::Solid::to_mesh`] — that is
//! `tessellate_brep` followed by `repair_export_mesh`, the *export* boundary —
//! and, where the root is still a B-rep, the solid itself.
//!
//! Measured on the stator (2026-09-17): the export mesh sections with tears up
//! to **0.4 mm** on the faces where tangent fillets meet, while the raw
//! tessellation of the same B-rep sections cleanly to **0.005 mm**. The repair
//! pass closes the boundary for printing and ray-tracing by moving vertices
//! onto their analytic carriers; a plane through the moved region then cuts a
//! slightly different shape. A CAM contour is a wall the cutter follows, so
//! 0.4 mm is not a rounding difference — it is a quarter of a slot mouth.
//!
//! So: **section the raw tessellation whenever there is a B-rep behind the
//! part**, and say which was used in `mesh_source`. A part with no B-rep — an
//! imported mesh, or a **root-mesh cache hit**, which lands in the scene as a
//! mesh with `solid: None` because the cache stores triangles and not
//! topology — can only be sectioned as it stands. That is not wrong, only
//! less exact, and the response says so rather than letting the caller assume
//! the better path was taken.

use serde_json::Value;

/// Section a part of an evaluated scene at `z`, or at the mid-height of its
/// own bounds when `auto_z`.
pub fn outline(
    scene: *const crate::VcadScene,
    part_index: usize,
    z: f64,
    auto_z: bool,
    options: &str,
) -> Result<Value, String> {
    if scene.is_null() {
        return Err("outline from scene: the scene handle is null.".into());
    }
    let s: &crate::VcadScene = unsafe { &*scene };
    let part = s.inner.parts.get(part_index).ok_or_else(|| {
        format!(
            "this scene has {} part(s), so there is no part {part_index}.",
            s.inner.parts.len()
        )
    })?;

    // The raw tessellation where there is topology to build it from; the
    // scene's own mesh otherwise. See the module notes.
    if let Some(brep) = part.solid.as_ref().and_then(|solid| solid.as_brep()) {
        let segments = vcad_cam_api::section_segments(options)?;
        let mesh = vcad_kernel_tessellate::tessellate_brep(brep, segments);
        let positions: Vec<[f64; 3]> = mesh
            .vertices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| [c[0] as f64, c[1] as f64, c[2] as f64])
            .collect();
        return vcad_cam_api::section_mesh(
            &positions,
            &mesh.indices,
            z,
            auto_z,
            options,
            "raw_tessellation",
        );
    }

    let cached = s
        .inner
        .root_keys
        .get(part_index)
        .and_then(|k| k.as_ref())
        .is_some();
    let source = if cached {
        "cached_root_mesh"
    } else {
        "export_mesh"
    };
    let positions: Vec<[f64; 3]> = part
        .mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0] as f64, c[1] as f64, c[2] as f64])
        .collect();
    vcad_cam_api::section_mesh(&positions, &part.mesh.indices, z, auto_z, options, source)
}
