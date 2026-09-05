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
    GpuEnvPack, GradientEnv, Ground, PathTraceOptions, Pbr, PixelFilter, Sun,
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

    // The transmissive extensions, mapped to mean what the three.js viewport
    // already makes them mean, so a part that reads as glass in the browser
    // reads as the same glass here.
    //
    // - `transmission` is `MeshPhysicalMaterial.transmission`, 0..1.
    // - `attenuationDistance` / `attenuationColor` are Beer-Lambert, in
    //   millimetres, which is the document's own unit — so they carry over
    //   unscaled.
    // - `thickness` is three.js' *volume* thickness, and three's convention is
    //   that `0` means "no volume": a thin sheet whose refraction is a
    //   straight pass-through. That is exactly `thin_walled`. An absent
    //   thickness is not zero — `SceneMesh.tsx` fills in 0.5 — so only an
    //   explicit zero flips the flag.
    //
    // `vcad_ir` has no dispersion field, so `abbe`/`sellmeier` stay off and a
    // vcad glass is achromatic. A future `MaterialDef::abbe` is all it would
    // take.
    let transmission = mat
        .and_then(|m| m.transmission)
        .map(|v| v as f32)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let thin_walled = transmission > 0.0 && mat.and_then(|m| m.thickness) == Some(0.0);
    let attenuation_color = mat
        .and_then(|m| m.attenuation_color)
        .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
        .unwrap_or([1.0; 3]);
    let attenuation_distance = mat
        .and_then(|m| m.attenuation_distance)
        .filter(|d| *d > 0.0)
        .map(|d| d as f32)
        .unwrap_or(f32::INFINITY);

    // Dielectrics that are already glossy get a clearcoat; rough matte
    // surfaces (sandblasted, as-printed) do not. Glass does not: the
    // heuristic exists to put a lacquer on an opaque plastic, and a
    // transmissive material that wants a coat has `clearcoat` in the IR to
    // say so.
    let clearcoat = if transmission == 0.0 && metallic < 0.5 && roughness < 0.5 {
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

    // The principled parameters kosm-render added (EON diffuse roughness,
    // subsurface, specular/specular_tint, the sheen layer) have no `vcad_ir`
    // spelling yet, so they stay at their defaults — which are exactly the
    // values that reproduce the model as it was before they existed.
    Pbr {
        base_color,
        metallic,
        roughness,
        anisotropy,
        clearcoat,
        clearcoat_roughness: 0.08,
        ior,
        transmission,
        attenuation_color,
        attenuation_distance,
        thin_walled,
        emissive: [0.0; 3],
        ..Pbr::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glass(thickness: Option<f64>) -> vcad_ir::MaterialDef {
        vcad_ir::MaterialDef {
            name: "glass".into(),
            color: [0.95, 0.97, 1.0],
            metallic: 0.0,
            roughness: 0.02,
            transmission: Some(1.0),
            ior: Some(1.5),
            thickness,
            ..Default::default()
        }
    }

    /// An opaque document must map exactly as it did before transmission
    /// existed — the whole point of the extensions being `Option`.
    #[test]
    fn an_opaque_material_is_untouched() {
        let m = vcad_ir::MaterialDef {
            name: "abs_white".into(),
            color: [0.9, 0.9, 0.9],
            metallic: 0.0,
            roughness: 0.4,
            ..Default::default()
        };
        let p = from_material_def(Some(&m), None);
        assert_eq!(p.transmission, 0.0);
        assert!(!p.thin_walled);
        assert_eq!(p.attenuation_color, [1.0; 3]);
        assert!(p.attenuation_distance.is_infinite());
        assert!(p.clearcoat > 0.0, "the glossy-dielectric coat still applies");
        assert_eq!(from_material_def(None, None).transmission, 0.0);
    }

    /// The `glass` preset the browser ships: full transmission, IOR 1.5, a
    /// real volume (thickness 2 mm), no absorption, and no lacquer heuristic
    /// on top of it.
    #[test]
    fn the_glass_preset_becomes_glass() {
        let p = from_material_def(Some(&glass(Some(2.0))), None);
        assert_eq!(p.transmission, 1.0);
        assert_eq!(p.ior, 1.5);
        assert!(!p.thin_walled);
        assert_eq!(p.clearcoat, 0.0, "glass does not get the coat heuristic");
    }

    /// three.js reads `thickness == 0` as "no volume", and so do we.
    #[test]
    fn zero_thickness_is_a_thin_wall() {
        assert!(from_material_def(Some(&glass(Some(0.0))), None).thin_walled);
        // Absent is not zero: `SceneMesh.tsx` fills in 0.5, so it has volume.
        assert!(!from_material_def(Some(&glass(None)), None).thin_walled);
    }

    /// Tinted glass carries its Beer-Lambert pair over in millimetres.
    #[test]
    fn tinted_glass_carries_its_absorption() {
        let m = vcad_ir::MaterialDef {
            attenuation_distance: Some(25.0),
            attenuation_color: Some([0.3, 0.45, 0.55]),
            ..glass(Some(3.0))
        };
        let p = from_material_def(Some(&m), None);
        assert_eq!(p.attenuation_distance, 25.0);
        assert_eq!(p.attenuation_color, [0.3, 0.45, 0.55]);
        // One attenuation distance reproduces the colour that named it.
        let sigma = p.extinction();
        for c in 0..3 {
            let t = (-sigma[c] * 25.0).exp();
            assert!((t - p.attenuation_color[c]).abs() < 1e-6);
        }
    }
}
