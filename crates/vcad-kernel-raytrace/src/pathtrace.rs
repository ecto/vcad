//! The integrator, and the one thing about it that is vcad's.
//!
//! `Scene`, `Object`, `Pbr`, the lights, the film, the denoiser and `render`
//! all live in [`kosm_render::pathtrace`] now — none of it knew what a B-rep
//! was. What stays here is [`from_material_def`]: turning a `vcad_ir`
//! material definition into a [`Pbr`] is a fact about vcad's document
//! format, and a renderer has no business reading one.

pub use kosm_render::pathtrace::{
    denoise, light_power_table, linear_to_srgb, power_table_from_weights, reference_bsdf_eval,
    render, render_into, studio_rig, tonemap_aces, AreaLight, Camera, EnvMap, Environment, Film,
    GpuEnvPack, GradientEnv, Ground, PathTraceOptions, Pbr,
};

use crate::bvh::BrepGeom;

/// A traceable object: a BVH over a vcad solid, a material, a placement.
pub type Object = kosm_render::pathtrace::Object<BrepGeom>;

/// Everything the integrator needs to render a frame of vcad geometry.
pub type Scene = kosm_render::pathtrace::Scene<BrepGeom>;

/// Circumferential grain implied by a material's name, when the document does
/// not state anisotropy explicitly.
fn anisotropy_from_name(name: &str) -> f32 {
    let n = name.to_ascii_lowercase();
    // Turning and boring cut circumferentially, which is the +u direction on
    // a cylinder — the same direction the tangent frame is built from.
    if n.contains("turned") || n.contains("machined") || n.contains("bored") {
        0.6
    } else if n.contains("brushed") {
        0.7
    } else {
        0.0
    }
}

/// Derive a render material from an IR material definition.
///
/// Single source of truth for BOTH renderers: `vcad-render --photoreal`
/// and the GPU viewport call this, so a part cannot pick up a different
/// clearcoat, IOR or grain depending on which one drew it.
pub fn from_material_def(mat: Option<&vcad_ir::MaterialDef>, tint: Option<[f64; 3]>) -> Pbr {
    let base = mat.map(|m| m.color).or(tint).unwrap_or([0.62, 0.64, 0.67]);
    let base_color = [base[0] as f32, base[1] as f32, base[2] as f32];

    let metallic = mat
        .map(|m| m.metallic as f32)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    // Perfectly sharp mirrors read as CG. Floor roughness slightly.
    let roughness = mat
        .map(|m| m.roughness as f32)
        .unwrap_or(0.35)
        .clamp(0.03, 1.0);
    let ior = mat
        .and_then(|m| m.ior)
        .map(|v| v as f32)
        .unwrap_or(1.5)
        .clamp(1.0, 3.0);

    // Dielectrics that are already glossy get a clearcoat; rough matte
    // surfaces (sandblasted, as-printed) do not.
    let clearcoat = if metallic < 0.5 && roughness < 0.5 {
        0.35 * (1.0 - roughness / 0.5)
    } else {
        0.0
    };

    // Anisotropy is a real IR field (`MaterialDef::anisotropy`) rather than
    // a rendering-time guess: Rust is the source of truth for IR types, the
    // value is a genuine property of the surface finish, and it round-trips
    // in `.vcad`. The name heuristic only fills in when the document says
    // nothing — a document that names its material "brushed_aluminum" or
    // "turned_shaft" has told us the finish, and rendering that as a uniform
    // polish is the CG tell this feature exists to remove. Anything explicit
    // always wins.
    let anisotropy = mat
        .and_then(|m| m.anisotropy)
        .map(|v| v as f32)
        .unwrap_or_else(|| mat.map(|m| anisotropy_from_name(&m.name)).unwrap_or(0.0))
        .clamp(-1.0, 1.0);

    Pbr {
        base_color,
        metallic,
        roughness,
        anisotropy,
        clearcoat,
        clearcoat_roughness: 0.08,
        ior,
        emissive: [0.0; 3],
    }
}
