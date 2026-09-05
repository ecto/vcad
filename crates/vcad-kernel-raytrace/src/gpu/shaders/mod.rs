//! WGSL shader sources for ray tracing.

/// The shared BSDF: the single shading model used by the GPU path tracer and,
/// as a port, by the CPU renderer in [`crate::pathtrace`].
///
/// Prepended to every shader that shades, so the BSDF exists in exactly one
/// place in the codebase. It also defines `PI` and the `GpuMaterial` struct,
/// since those are part of the shading contract.
pub const BSDF_SHADER: &str = include_str!("bsdf.wgsl");

/// Body of the main ray tracing compute shader. Not valid WGSL on its own —
/// it depends on [`BSDF_SHADER`]. Use [`raytrace_shader`] for the full module.
const RAYTRACE_BODY: &str = include_str!("raytrace.wgsl");

/// The main ray tracing compute shader, BSDF included.
pub fn raytrace_shader() -> String {
    compose(RAYTRACE_BODY)
}

/// Analytic surface parameterisation: the `GpuSurface` struct, the surface
/// constants, and the `dP/du` tangent that drives anisotropic shading.
///
/// Prepended after [`BSDF_SHADER`] (on which it depends for `onb`).
pub const SURFACE_SHADER: &str = include_str!("surface.wgsl");

/// Lat-long HDR environment: nearest-texel lookup, CDF importance sampling and
/// the solid-angle PDF, ported from `pathtrace::EnvMap`.
///
/// References an `env_data: array<f32>` storage binding that the HOST shader
/// declares, so it can be composed at whatever binding index each has spare.
pub const ENV_SHADER: &str = include_str!("env.wgsl");

/// Test-only harness that evaluates and samples the shared BSDF on the GPU.
///
/// Not valid WGSL on its own — compose it with [`BSDF_SHADER`] via
/// [`compose`]. Driven by `tests/bsdf_parity.rs`.
pub const BSDF_PARITY_HARNESS: &str = include_str!("bsdf_parity.wgsl");

/// Prepend the shared BSDF to a shader body, producing a complete module.
///
/// Used by the render pipeline and by the BSDF parity test, which pairs this
/// exact BSDF source with a tiny harness entry point.
pub fn compose(body: &str) -> String {
    format!("{BSDF_SHADER}\n{SURFACE_SHADER}\n{ENV_SHADER}\n{body}")
}

/// Device-side per-pixel history and à-trous denoise.
///
/// Self-contained — it shades nothing, so unlike the others it needs no
/// [`BSDF_SHADER`] prefix. See `gpu/history.rs` for the passes it defines and
/// the order they run in.
pub const HISTORY_SHADER: &str = include_str!("history.wgsl");
