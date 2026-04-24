//! Render-bake pipeline — turns a raw tessellated mesh into a render-ready
//! mesh by running whatever post-processing the renderer needs.
//!
//! Today the pipeline is exactly one pass (angle-based creased vertex
//! normals via [`apply_creased_normals`]). The module exists so that
//! future render-only transforms — tangent-space generation for normal
//! maps, LOD simplification, morph-target baking, palette compression —
//! land in a single place instead of sprouting more `_for_render` variants
//! across the call graph.
//!
//! The invariant: callers that need a mesh for **rendering, STL / GLB
//! export, or the ray tracer** always go through [`render_bake`]. Callers
//! that need a mesh for **analysis** — booleans, boundary detection,
//! volume, clash, topology inspection — use the raw tessellator output and
//! never touch this module. The type system doesn't enforce this (both are
//! [`TriangleMesh`]) but the import boundary does.

use crate::{apply_creased_normals, TriangleMesh, DEFAULT_CREASE_ANGLE_RAD};

/// Options controlling the render-bake passes.
///
/// Use `RenderBakeOptions::default()` for the app's standard behavior.
/// Fields are public so the small set of callers that need a non-default
/// crease angle (document-specific overrides, future editorial control)
/// can tweak them without a constructor.
#[derive(Debug, Clone, Copy)]
pub struct RenderBakeOptions {
    /// Angle (radians) above which adjacent faces produce a hard crease.
    /// Below this, normals are averaged across the shared edge.
    pub crease_angle_rad: f64,
    // Future:
    // pub generate_tangents: bool,
    // pub weld_epsilon: f32,
    // pub simplify_target_ratio: Option<f32>,
}

impl Default for RenderBakeOptions {
    fn default() -> Self {
        Self {
            crease_angle_rad: DEFAULT_CREASE_ANGLE_RAD,
        }
    }
}

/// Run the render-bake passes on a mesh in place.
///
/// This is the single canonical "prepare a mesh for rendering" entry point
/// used by every render / export path. It's idempotent-safe on already-baked
/// meshes (a second pass produces the same per-triangle flat normals).
pub fn render_bake(mesh: &mut TriangleMesh, opts: RenderBakeOptions) {
    apply_creased_normals(mesh, opts.crease_angle_rad);
}

/// Convenience for the common case of default options.
pub fn render_bake_default(mesh: &mut TriangleMesh) {
    render_bake(mesh, RenderBakeOptions::default());
}
