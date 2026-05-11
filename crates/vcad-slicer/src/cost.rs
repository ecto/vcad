//! Print cost estimation.
//!
//! As of v1 DFM, this module is a back-compat shim over the shared
//! [`vcad_kernel_cost`] crate. The historical
//! `vcad_slicer::cost::Material` and `CostEstimate` types are direct
//! re-exports of the shared types so QuotePanel, the DFM cost section,
//! and the slicer's per-print quote all agree.
//!
//! New callers should depend on `vcad-kernel-cost` directly. The
//! re-export lives here to keep the existing `vcad-kernel-wasm`
//! `estimatePrintCost` binding working without churn.

pub use vcad_kernel_cost::{
    estimate_fdm_from_filament as estimate_cost_from_filament,
    estimate_fdm_from_volume as _estimate_fdm_from_volume,
    CostEstimate, Material, Process,
};

/// Pre-slice FDM estimate from BRep volume.
///
/// Wraps [`vcad_kernel_cost::estimate_fdm_from_volume`]. Same signature
/// and semantics as the legacy slicer version — the only difference is
/// the returned `CostEstimate` now carries the richer fields the DFM
/// cost panel needs (`material_cost_usd`, `total_usd`, `setup_cost_usd`,
/// `assumptions`). Existing JS consumers that only read `weight_grams`
/// and `material` keep working unchanged.
pub fn estimate_cost_from_volume(
    volume_mm3: f64,
    infill_density: f64,
    wall_count: u32,
    line_width: f64,
    material: &Material,
) -> CostEstimate {
    _estimate_fdm_from_volume(volume_mm3, infill_density, wall_count, line_width, material)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fdm_from_volume_returns_a_cost() {
        let pla = Material::pla();
        let cost = estimate_cost_from_volume(4000.0, 0.15, 3, 0.45, &pla);
        assert!(cost.weight_grams > 0.0);
        assert!(cost.material_cost_usd > 0.0);
        assert!(cost.is_estimate);
        assert!(cost.material_cost_usd < 1.0);
    }

    #[test]
    fn fdm_from_filament_round_trips() {
        let pla = Material::pla();
        let cost = estimate_cost_from_filament(5.0, &pla);
        assert_eq!(cost.weight_grams, 5.0);
        assert!((cost.material_cost_usd - 0.10).abs() < 0.01);
        assert!(!cost.is_estimate);
    }
}
