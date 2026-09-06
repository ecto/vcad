//! WGSL for the BRep geometry module.
//!
//! The renderer's own shaders — the BSDF, the prelude, the environment, the
//! integrator, the history — live in `kosm_render::gpu::shaders`.

/// Analytic surface parameterisation: the `GpuSurface` struct, the surface
/// constants, and the `dP/du` tangent that drives anisotropic shading.
pub const SURFACE_SHADER: &str = include_str!("surface.wgsl");

/// Ray-surface intersection, trim testing, BVH traversal and the hit
/// accessors: kosm-render's geometry contract over trimmed BRep faces.
///
/// Not valid WGSL on its own — it depends on [`SURFACE_SHADER`] before it and
/// on the renderer's BSDF and prelude before that. Use
/// [`super::BrepGeometry::module`].
pub const BREP_SHADER: &str = include_str!("brep.wgsl");

/// Test-only harness that evaluates and samples the shared BSDF, and drives
/// `surface_dpdu`, on the GPU. Driven by `tests/bsdf_parity.rs`.
pub const BSDF_PARITY_HARNESS: &str = include_str!("bsdf_parity.wgsl");

/// The parity harness as a complete module: the renderer's BSDF and prelude,
/// this crate's surface parameterisation, then the harness.
pub fn compose(body: &str) -> String {
    kosm_render::gpu::shaders::compose_with(SURFACE_SHADER, body)
}
