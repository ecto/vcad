//! Material properties registry for sheet metal.
//!
//! Replaces the spec's "single global `min_bend_radius_ratio` lie" with a
//! per-material lookup: each alloy/temper carries its own minimum R/t, yield
//! strength, modulus, density (for cost) and a coarse springback factor.
//!
//! For tier 1 the registry is a hard-coded curated list sourced from
//! Machinery's Handbook + supplier data. Later tiers replace it with the
//! same open community-contribution path the bend tables target.
//!
//! `lookup` matches by case-insensitive name and accepts a few aliases
//! (e.g. `"aluminum"` → `Al-soft`, `"stainless"` → `SS-304`) so an agent
//! using natural names still gets a hit; misses fall back to a conservative
//! generic material so checks never silently pass on an unknown alloy.

use serde::{Deserialize, Serialize};

/// Mechanical properties of a sheet-metal material at room temperature.
///
/// All fields are user-facing and round-trip through JSON for the bend-table
/// editor and MCP tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialProperties {
    /// Stable key (matches the `material` field on bend-table rows and the
    /// `material` argument to `base_flange_rect`). Lowercase ASCII +
    /// hyphens, e.g. `"al-soft"`.
    pub name: String,
    /// Pretty name for the UI (e.g. `"Aluminum 1100 / 3003 (soft)"`).
    pub display_name: String,
    /// Minimum inside-bend-radius / thickness ratio. A bend tighter than
    /// `min_r_over_t · t` cracks the outer fibre.
    pub min_r_over_t: f64,
    /// Yield strength (MPa). Feeds springback estimation.
    pub yield_mpa: f64,
    /// Young's modulus (GPa). Feeds springback estimation.
    pub modulus_gpa: f64,
    /// Density (kg/m³). Feeds mass + raw-material cost.
    pub density_kg_m3: f64,
    /// Coarse springback factor: estimated extra angle (radians) per
    /// radian of formed bend. Used until the closed-form elastoplastic
    /// model lands. Typical values 0.005–0.04.
    pub springback_per_radian: f64,
}

/// Conservative fallback for unknown materials — picks the strictest
/// reasonable defaults so a bend that would fail any real alloy still
/// fails the check.
pub fn unknown_material(name: impl Into<String>) -> MaterialProperties {
    let raw = name.into();
    MaterialProperties {
        name: raw.clone(),
        display_name: format!("{raw} (unknown — using conservative defaults)"),
        min_r_over_t: 2.0,
        yield_mpa: 300.0,
        modulus_gpa: 200.0,
        density_kg_m3: 7850.0,
        springback_per_radian: 0.03,
    }
}

/// The curated default material registry. Six common shop materials
/// covering aluminum (soft/hard), mild + stainless steel, brass, copper.
pub fn builtin_materials() -> Vec<MaterialProperties> {
    vec![
        MaterialProperties {
            name: "al-soft".to_string(),
            display_name: "Aluminum 1100 / 3003 (soft)".to_string(),
            min_r_over_t: 0.0,
            yield_mpa: 35.0,
            modulus_gpa: 69.0,
            density_kg_m3: 2710.0,
            springback_per_radian: 0.010,
        },
        MaterialProperties {
            name: "al-hard".to_string(),
            display_name: "Aluminum 6061-T6 (hard)".to_string(),
            min_r_over_t: 1.5,
            yield_mpa: 276.0,
            modulus_gpa: 69.0,
            density_kg_m3: 2700.0,
            springback_per_radian: 0.025,
        },
        MaterialProperties {
            name: "steel-mild".to_string(),
            display_name: "Mild steel (A36 / CRS)".to_string(),
            min_r_over_t: 0.5,
            yield_mpa: 250.0,
            modulus_gpa: 200.0,
            density_kg_m3: 7850.0,
            springback_per_radian: 0.012,
        },
        MaterialProperties {
            name: "ss-304".to_string(),
            display_name: "Stainless steel 304".to_string(),
            min_r_over_t: 0.5,
            yield_mpa: 215.0,
            modulus_gpa: 193.0,
            density_kg_m3: 8000.0,
            springback_per_radian: 0.020,
        },
        MaterialProperties {
            name: "brass".to_string(),
            display_name: "Brass C260 (cartridge)".to_string(),
            min_r_over_t: 0.0,
            yield_mpa: 124.0,
            modulus_gpa: 110.0,
            density_kg_m3: 8530.0,
            springback_per_radian: 0.012,
        },
        MaterialProperties {
            name: "copper".to_string(),
            display_name: "Copper C110 (soft)".to_string(),
            min_r_over_t: 0.0,
            yield_mpa: 70.0,
            modulus_gpa: 117.0,
            density_kg_m3: 8960.0,
            springback_per_radian: 0.008,
        },
    ]
}

/// Look up a material by name. Case-insensitive, hyphen/underscore-tolerant,
/// and recognises a few common aliases. Returns `None` only for a truly
/// unknown name — call [`unknown_material`] to get a conservative fallback.
pub fn lookup(name: &str) -> Option<MaterialProperties> {
    let key = normalise(name);
    // Direct hit on registry keys.
    for m in builtin_materials() {
        if normalise(&m.name) == key {
            return Some(m);
        }
    }
    // A few user-friendly aliases. Keep deliberately narrow — when in
    // doubt return None so the caller falls back to `unknown_material`.
    let alias = match key.as_str() {
        "aluminum" | "aluminium" | "al" | "al-1100" | "al-3003" | "1100" | "3003" => {
            Some("al-soft")
        }
        "al-6061" | "6061" | "6061-t6" => Some("al-hard"),
        "steel" | "mild-steel" | "a36" | "crs" | "1018" => Some("steel-mild"),
        "stainless" | "ss" | "304" | "ss304" => Some("ss-304"),
        "cu" => Some("copper"),
        _ => None,
    };
    alias.and_then(lookup)
}

/// Look up a material, falling back to a conservative unknown profile.
pub fn lookup_or_unknown(name: &str) -> MaterialProperties {
    lookup(name).unwrap_or_else(|| unknown_material(name))
}

fn normalise(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_covers_the_six_shop_basics() {
        let names: Vec<_> = builtin_materials().into_iter().map(|m| m.name).collect();
        for expected in [
            "al-soft",
            "al-hard",
            "steel-mild",
            "ss-304",
            "brass",
            "copper",
        ] {
            assert!(names.iter().any(|n| n == expected), "missing {expected}");
        }
    }

    #[test]
    fn lookup_is_case_and_separator_insensitive() {
        let a = lookup("Al-Soft").unwrap();
        let b = lookup("AL_SOFT").unwrap();
        let c = lookup("al-soft").unwrap();
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn lookup_resolves_common_aliases() {
        assert_eq!(lookup("aluminum").unwrap().name, "al-soft");
        assert_eq!(lookup("6061-T6").unwrap().name, "al-hard");
        assert_eq!(lookup("stainless").unwrap().name, "ss-304");
        assert_eq!(lookup("A36").unwrap().name, "steel-mild");
    }

    #[test]
    fn unknown_material_is_conservative() {
        let m = unknown_material("Unobtanium");
        assert!(m.min_r_over_t >= 1.0, "must require at least R/t = 1");
        assert!(m.display_name.contains("unknown"));
    }

    #[test]
    fn lookup_or_unknown_never_panics() {
        let m = lookup_or_unknown("not-a-real-alloy");
        assert!(m.min_r_over_t > 0.0);
    }

    #[test]
    fn material_properties_round_trip_json() {
        let m = lookup("al-hard").unwrap();
        let json = serde_json::to_string(&m).unwrap();
        let m2: MaterialProperties = serde_json::from_str(&json).unwrap();
        assert_eq!(m, m2);
    }
}
