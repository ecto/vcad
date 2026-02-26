//! Print cost estimation.
//!
//! Provides instant cost estimates from BRep volume (pre-slice)
//! and more accurate estimates from actual filament usage (post-slice).

use serde::{Deserialize, Serialize};

/// Material properties for cost estimation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Material {
    /// Material name.
    pub name: String,
    /// Density in g/cm³.
    pub density: f64,
    /// Price per kilogram in USD.
    pub price_per_kg: f64,
}

impl Material {
    /// Generic PLA.
    pub fn pla() -> Self {
        Self {
            name: "PLA".into(),
            density: 1.24,
            price_per_kg: 20.0,
        }
    }

    /// PETG.
    pub fn petg() -> Self {
        Self {
            name: "PETG".into(),
            density: 1.27,
            price_per_kg: 22.0,
        }
    }

    /// ABS.
    pub fn abs() -> Self {
        Self {
            name: "ABS".into(),
            density: 1.04,
            price_per_kg: 18.0,
        }
    }

    /// TPU.
    pub fn tpu() -> Self {
        Self {
            name: "TPU".into(),
            density: 1.21,
            price_per_kg: 30.0,
        }
    }

    /// All built-in materials.
    pub fn all_materials() -> Vec<Self> {
        vec![Self::pla(), Self::petg(), Self::abs(), Self::tpu()]
    }
}

/// Cost estimate result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    /// Material name.
    pub material: String,
    /// Filament weight in grams.
    pub weight_grams: f64,
    /// Filament cost in USD.
    pub cost_usd: f64,
    /// Whether this is a pre-slice estimate (less accurate) or post-slice.
    pub is_estimate: bool,
}

/// Estimate cost from BRep volume (instant, pre-slice).
///
/// Uses solid volume + infill approximation. Less accurate than post-slice
/// but available immediately without slicing.
pub fn estimate_cost_from_volume(
    volume_mm3: f64,
    infill_density: f64,
    wall_count: u32,
    line_width: f64,
    material: &Material,
) -> CostEstimate {
    // Approximate: solid walls + infilled interior
    // Wall volume ≈ surface_fraction * volume (rough heuristic)
    let wall_fraction = (wall_count as f64 * line_width / 10.0).min(1.0);
    let effective_density = wall_fraction + (1.0 - wall_fraction) * infill_density;
    let effective_volume_mm3 = volume_mm3 * effective_density;

    // Convert mm³ to cm³
    let volume_cm3 = effective_volume_mm3 / 1000.0;
    let weight_grams = volume_cm3 * material.density;
    let cost_usd = weight_grams * material.price_per_kg / 1000.0;

    CostEstimate {
        material: material.name.clone(),
        weight_grams,
        cost_usd,
        is_estimate: true,
    }
}

/// Estimate cost from actual filament usage (post-slice, more accurate).
pub fn estimate_cost_from_filament(filament_grams: f64, material: &Material) -> CostEstimate {
    let cost_usd = filament_grams * material.price_per_kg / 1000.0;

    CostEstimate {
        material: material.name.clone(),
        weight_grams: filament_grams,
        cost_usd,
        is_estimate: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_from_volume() {
        let pla = Material::pla();
        // 20x20x10mm cube = 4000mm³
        let cost = estimate_cost_from_volume(4000.0, 0.15, 3, 0.45, &pla);

        assert!(cost.weight_grams > 0.0);
        assert!(cost.cost_usd > 0.0);
        assert!(cost.is_estimate);
        // Sanity: a small cube shouldn't cost more than a few cents
        assert!(cost.cost_usd < 1.0);
    }

    #[test]
    fn test_cost_from_filament() {
        let pla = Material::pla();
        let cost = estimate_cost_from_filament(5.0, &pla);

        assert_eq!(cost.weight_grams, 5.0);
        assert!((cost.cost_usd - 0.1).abs() < 0.01); // 5g * $20/kg = $0.10
        assert!(!cost.is_estimate);
    }
}
