#![warn(missing_docs)]

//! Shared manufacturing cost model for the vcad kernel.
//!
//! Both [`vcad-slicer`](../vcad_slicer/index.html) and
//! [`vcad-kernel-dfm`](../vcad_kernel_dfm/index.html) consume the same
//! [`Material`] catalog and emit the same [`CostEstimate`] shape so that
//! the in-app QuotePanel, the DFM report's cost section, and the
//! slicer's per-print estimate all agree.
//!
//! The estimators here are intentionally simple — material cost, machine
//! time at a per-minute rate, and a per-process setup charge. Higher-
//! fidelity models (real CAM time, mold flow, full quote breakdown) can
//! ride on top by producing a richer `CostEstimate` with the same shape.

use serde::{Deserialize, Serialize};

/// The manufacturing processes the cost model recognises.
///
/// Mirrors the `Process` enum in `vcad-kernel-dfm`. Re-exported there
/// so callers only depend on one of the crates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Process {
    /// 3-axis CNC milling.
    Cnc3Axis,
    /// Fused-deposition-modelling 3D printing (filament).
    Fdm,
    /// Stereolithography / resin 3D printing.
    Sla,
    /// Injection molding (thermoplastic).
    Injection,
    /// Sheet-metal bending / cutting.
    SheetMetal,
    /// Sand casting (gravity-fed).
    CastingSand,
    /// Investment casting ("lost-wax").
    CastingInvestment,
}

impl Process {
    /// Parse a snake-case identifier.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "cnc_3axis" | "cnc" => Some(Self::Cnc3Axis),
            "fdm" | "filament" => Some(Self::Fdm),
            "sla" | "resin" => Some(Self::Sla),
            "injection" | "injection_molding" => Some(Self::Injection),
            "sheet_metal" | "sheet" => Some(Self::SheetMetal),
            "casting_sand" | "sand_casting" => Some(Self::CastingSand),
            "casting_investment" | "investment_casting" => Some(Self::CastingInvestment),
            _ => None,
        }
    }

    /// Canonical snake_case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cnc3Axis => "cnc_3axis",
            Self::Fdm => "fdm",
            Self::Sla => "sla",
            Self::Injection => "injection",
            Self::SheetMetal => "sheet_metal",
            Self::CastingSand => "casting_sand",
            Self::CastingInvestment => "casting_investment",
        }
    }
}

/// A manufacturable material with the parameters every estimator needs.
///
/// Some fields only matter for certain processes (e.g. `bend_cost_usd`
/// for sheet metal). Defaults keep the struct usable for one-off
/// estimates without rebuilding the catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Material {
    /// Display name, e.g. "PLA", "Aluminum 6061".
    pub name: String,
    /// Density in g/cm³ (1.24 for PLA, 2.70 for aluminum).
    pub density: f64,
    /// Material price per kilogram in USD.
    pub price_per_kg: f64,
    /// Which processes this material is compatible with.
    #[serde(default)]
    pub process_compat: Vec<Process>,
    /// Default machine setup/fixturing charge per job in USD.
    #[serde(default)]
    pub setup_cost_usd_default: f64,
    /// Machine time rate in USD per minute (CNC, printer, etc.).
    #[serde(default)]
    pub machine_rate_usd_per_min: f64,
    /// For sheet metal: amortized cost per bend in USD.
    #[serde(default)]
    pub bend_cost_usd: f64,
    /// For molding/casting: amortized tooling cost in USD.
    #[serde(default)]
    pub tooling_cost_usd: f64,
}

impl Material {
    /// Generic PLA filament for FDM.
    pub fn pla() -> Self {
        Self {
            name: "PLA".into(),
            density: 1.24,
            price_per_kg: 20.0,
            process_compat: vec![Process::Fdm],
            setup_cost_usd_default: 2.0,
            machine_rate_usd_per_min: 0.05,
            bend_cost_usd: 0.0,
            tooling_cost_usd: 0.0,
        }
    }

    /// PETG filament.
    pub fn petg() -> Self {
        Self {
            name: "PETG".into(),
            density: 1.27,
            price_per_kg: 22.0,
            process_compat: vec![Process::Fdm],
            setup_cost_usd_default: 2.0,
            machine_rate_usd_per_min: 0.05,
            bend_cost_usd: 0.0,
            tooling_cost_usd: 0.0,
        }
    }

    /// ABS filament (also a common injection-molding resin).
    pub fn abs() -> Self {
        Self {
            name: "ABS".into(),
            density: 1.04,
            price_per_kg: 18.0,
            process_compat: vec![Process::Fdm, Process::Injection],
            setup_cost_usd_default: 2.0,
            machine_rate_usd_per_min: 0.05,
            bend_cost_usd: 0.0,
            tooling_cost_usd: 2500.0,
        }
    }

    /// TPU filament.
    pub fn tpu() -> Self {
        Self {
            name: "TPU".into(),
            density: 1.21,
            price_per_kg: 30.0,
            process_compat: vec![Process::Fdm],
            setup_cost_usd_default: 2.0,
            machine_rate_usd_per_min: 0.05,
            bend_cost_usd: 0.0,
            tooling_cost_usd: 0.0,
        }
    }

    /// Generic SLA resin.
    pub fn sla_resin() -> Self {
        Self {
            name: "SLA Resin".into(),
            density: 1.15,
            price_per_kg: 60.0,
            process_compat: vec![Process::Sla],
            setup_cost_usd_default: 3.0,
            machine_rate_usd_per_min: 0.08,
            bend_cost_usd: 0.0,
            tooling_cost_usd: 0.0,
        }
    }

    /// Aluminum 6061 stock (CNC).
    pub fn aluminum_6061() -> Self {
        Self {
            name: "Aluminum 6061".into(),
            density: 2.70,
            price_per_kg: 7.0,
            process_compat: vec![Process::Cnc3Axis, Process::SheetMetal],
            setup_cost_usd_default: 35.0,
            machine_rate_usd_per_min: 1.50,
            bend_cost_usd: 2.5,
            tooling_cost_usd: 0.0,
        }
    }

    /// Steel 1018 stock (CNC).
    pub fn steel_1018() -> Self {
        Self {
            name: "Steel 1018".into(),
            density: 7.87,
            price_per_kg: 2.0,
            process_compat: vec![Process::Cnc3Axis, Process::SheetMetal],
            setup_cost_usd_default: 45.0,
            machine_rate_usd_per_min: 2.00,
            bend_cost_usd: 3.5,
            tooling_cost_usd: 0.0,
        }
    }

    /// Brass C360 stock (CNC).
    pub fn brass_c360() -> Self {
        Self {
            name: "Brass C360".into(),
            density: 8.50,
            price_per_kg: 11.0,
            process_compat: vec![Process::Cnc3Axis],
            setup_cost_usd_default: 40.0,
            machine_rate_usd_per_min: 1.80,
            bend_cost_usd: 0.0,
            tooling_cost_usd: 0.0,
        }
    }

    /// Polycarbonate (injection).
    pub fn polycarbonate() -> Self {
        Self {
            name: "Polycarbonate".into(),
            density: 1.20,
            price_per_kg: 5.0,
            process_compat: vec![Process::Injection],
            setup_cost_usd_default: 250.0,
            machine_rate_usd_per_min: 1.20,
            bend_cost_usd: 0.0,
            tooling_cost_usd: 5000.0,
        }
    }

    /// Cast aluminum (for sand / investment casting).
    pub fn cast_aluminum_a356() -> Self {
        Self {
            name: "Cast Aluminum A356".into(),
            density: 2.68,
            price_per_kg: 3.0,
            process_compat: vec![Process::CastingSand, Process::CastingInvestment],
            setup_cost_usd_default: 80.0,
            machine_rate_usd_per_min: 0.0,
            bend_cost_usd: 0.0,
            tooling_cost_usd: 800.0,
        }
    }

    /// Cast iron (sand casting).
    pub fn cast_iron() -> Self {
        Self {
            name: "Cast Iron".into(),
            density: 7.20,
            price_per_kg: 1.5,
            process_compat: vec![Process::CastingSand],
            setup_cost_usd_default: 100.0,
            machine_rate_usd_per_min: 0.0,
            bend_cost_usd: 0.0,
            tooling_cost_usd: 1000.0,
        }
    }

    /// The built-in catalog covering all v1 processes.
    pub fn catalog() -> Vec<Self> {
        vec![
            Self::pla(),
            Self::petg(),
            Self::abs(),
            Self::tpu(),
            Self::sla_resin(),
            Self::aluminum_6061(),
            Self::steel_1018(),
            Self::brass_c360(),
            Self::polycarbonate(),
            Self::cast_aluminum_a356(),
            Self::cast_iron(),
        ]
    }

    /// Whether this material supports a given process.
    pub fn supports(&self, p: Process) -> bool {
        self.process_compat.is_empty() || self.process_compat.contains(&p)
    }
}

/// Result of a cost estimate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    /// Which process this estimate is for.
    pub process: Process,
    /// Material display name.
    pub material: String,
    /// Effective part weight in grams (includes infill/wall adjustments for FDM).
    pub weight_grams: f64,
    /// Material-only cost in USD.
    pub material_cost_usd: f64,
    /// Estimated machine time in minutes (None when not modelled).
    pub machine_time_min: Option<f64>,
    /// Setup/fixturing charge in USD.
    pub setup_cost_usd: f64,
    /// Amortized tooling charge in USD (= tooling / qty for mold/casting).
    pub tooling_cost_usd: f64,
    /// Total USD = material + machine_time*rate + setup + tooling.
    pub total_usd: f64,
    /// Whether this is a pre-evaluation estimate (true) vs. post-process (false).
    pub is_estimate: bool,
    /// Free-form assumptions ("6061 aluminum at $7/kg", "qty=1000", …).
    pub assumptions: Vec<String>,
}

/// Pre-slice FDM estimate from BRep volume with infill + wall approximation.
///
/// Matches the historical `vcad_slicer::cost::estimate_cost_from_volume`
/// API; that crate now re-exports this function so existing WASM bindings
/// keep working unchanged.
pub fn estimate_fdm_from_volume(
    volume_mm3: f64,
    infill_density: f64,
    wall_count: u32,
    line_width: f64,
    material: &Material,
) -> CostEstimate {
    let wall_fraction = (wall_count as f64 * line_width / 10.0).min(1.0);
    let effective_density = wall_fraction + (1.0 - wall_fraction) * infill_density;
    let effective_volume_mm3 = volume_mm3 * effective_density;
    let volume_cm3 = effective_volume_mm3 / 1000.0;
    let weight_grams = volume_cm3 * material.density;
    let material_cost_usd = weight_grams * material.price_per_kg / 1000.0;
    let total = material_cost_usd + material.setup_cost_usd_default;
    CostEstimate {
        process: Process::Fdm,
        material: material.name.clone(),
        weight_grams,
        material_cost_usd,
        machine_time_min: None,
        setup_cost_usd: material.setup_cost_usd_default,
        tooling_cost_usd: 0.0,
        total_usd: total,
        is_estimate: true,
        assumptions: vec![
            format!("{} at ${:.2}/kg", material.name, material.price_per_kg),
            format!(
                "{}% infill with {} walls at {:.2}mm",
                (infill_density * 100.0).round(),
                wall_count,
                line_width
            ),
        ],
    }
}

/// Post-process FDM estimate from measured filament weight.
pub fn estimate_fdm_from_filament(filament_grams: f64, material: &Material) -> CostEstimate {
    let material_cost_usd = filament_grams * material.price_per_kg / 1000.0;
    let total = material_cost_usd + material.setup_cost_usd_default;
    CostEstimate {
        process: Process::Fdm,
        material: material.name.clone(),
        weight_grams: filament_grams,
        material_cost_usd,
        machine_time_min: None,
        setup_cost_usd: material.setup_cost_usd_default,
        tooling_cost_usd: 0.0,
        total_usd: total,
        is_estimate: false,
        assumptions: vec![format!("Measured filament: {:.1}g", filament_grams)],
    }
}

/// CNC estimate from removed-stock volume.
///
/// `stock_volume_mm3` is the bounding-box (or stock) volume and
/// `part_volume_mm3` is the final part — so removed = stock - part.
/// `feature_count` is a rough complexity proxy (pockets + holes); each
/// adds a flat per-feature time charge.
pub fn estimate_cnc_from_removed_volume(
    stock_volume_mm3: f64,
    part_volume_mm3: f64,
    feature_count: u32,
    material: &Material,
) -> CostEstimate {
    let part_weight_g = (part_volume_mm3 / 1000.0) * material.density;
    // Stock weight drives material cost (you pay for what you start with).
    let stock_weight_g = (stock_volume_mm3 / 1000.0) * material.density;
    let material_cost_usd = stock_weight_g * material.price_per_kg / 1000.0;

    // ~ 1 minute per cm³ removed plus 2 minutes per feature, very rough.
    let removed_cm3 = ((stock_volume_mm3 - part_volume_mm3) / 1000.0).max(0.0);
    let machine_time_min = removed_cm3 + (feature_count as f64) * 2.0;
    let machine_cost = machine_time_min * material.machine_rate_usd_per_min;

    let total =
        material_cost_usd + machine_cost + material.setup_cost_usd_default;

    CostEstimate {
        process: Process::Cnc3Axis,
        material: material.name.clone(),
        weight_grams: part_weight_g,
        material_cost_usd,
        machine_time_min: Some(machine_time_min),
        setup_cost_usd: material.setup_cost_usd_default,
        tooling_cost_usd: 0.0,
        total_usd: total,
        is_estimate: true,
        assumptions: vec![
            format!("{} stock at ${:.2}/kg", material.name, material.price_per_kg),
            format!(
                "{:.1} cm³ removed @ ~1 min/cm³ + {} features × 2 min",
                removed_cm3, feature_count
            ),
            format!(
                "machine rate ${:.2}/min, setup ${:.0}",
                material.machine_rate_usd_per_min, material.setup_cost_usd_default
            ),
        ],
    }
}

/// Injection-molding estimate.
///
/// Tooling cost amortizes over `qty` (default 1000 in the rule pack).
pub fn estimate_injection(
    part_volume_mm3: f64,
    qty: u32,
    material: &Material,
) -> CostEstimate {
    let weight_g = (part_volume_mm3 / 1000.0) * material.density;
    let material_cost_usd = weight_g * material.price_per_kg / 1000.0;
    let tooling_per_part = material.tooling_cost_usd / (qty.max(1) as f64);
    // Cycle time roughly 30s/part for small parts plus 0.5s per cm³.
    let cycle_min = 0.5 + (part_volume_mm3 / 1000.0) * 0.0083;
    let machine_cost = cycle_min * material.machine_rate_usd_per_min;
    let total =
        material_cost_usd + machine_cost + tooling_per_part + material.setup_cost_usd_default;
    CostEstimate {
        process: Process::Injection,
        material: material.name.clone(),
        weight_grams: weight_g,
        material_cost_usd,
        machine_time_min: Some(cycle_min),
        setup_cost_usd: material.setup_cost_usd_default,
        tooling_cost_usd: tooling_per_part,
        total_usd: total,
        is_estimate: true,
        assumptions: vec![
            format!("{} pellets at ${:.2}/kg", material.name, material.price_per_kg),
            format!("tooling ${:.0} / qty {}", material.tooling_cost_usd, qty),
        ],
    }
}

/// Sheet-metal estimate from blank area and bend count.
pub fn estimate_sheet_metal(
    blank_area_mm2: f64,
    thickness_mm: f64,
    bend_count: u32,
    material: &Material,
) -> CostEstimate {
    let volume_mm3 = blank_area_mm2 * thickness_mm;
    let weight_g = (volume_mm3 / 1000.0) * material.density;
    let material_cost_usd = weight_g * material.price_per_kg / 1000.0;
    let bend_cost = (bend_count as f64) * material.bend_cost_usd;
    let total = material_cost_usd + bend_cost + material.setup_cost_usd_default;
    CostEstimate {
        process: Process::SheetMetal,
        material: material.name.clone(),
        weight_grams: weight_g,
        material_cost_usd,
        machine_time_min: None,
        setup_cost_usd: material.setup_cost_usd_default,
        tooling_cost_usd: 0.0,
        total_usd: total,
        is_estimate: true,
        assumptions: vec![
            format!("{} sheet at ${:.2}/kg", material.name, material.price_per_kg),
            format!(
                "blank area {:.0} mm² × {:.2} mm thick, {} bend(s) @ ${:.2}",
                blank_area_mm2, thickness_mm, bend_count, material.bend_cost_usd
            ),
        ],
    }
}

/// Casting estimate (sand or investment).
pub fn estimate_casting(
    process: Process,
    part_volume_mm3: f64,
    qty: u32,
    core_count: u32,
    material: &Material,
) -> CostEstimate {
    debug_assert!(matches!(
        process,
        Process::CastingSand | Process::CastingInvestment
    ));
    let weight_g = (part_volume_mm3 / 1000.0) * material.density;
    // Account for sprue/risers — empirical 1.3x for sand, 1.15x for investment.
    let pour_multiplier = if process == Process::CastingSand { 1.3 } else { 1.15 };
    let poured_weight_g = weight_g * pour_multiplier;
    let material_cost_usd = poured_weight_g * material.price_per_kg / 1000.0;
    let default_qty = if process == Process::CastingSand { 100u32 } else { 500u32 };
    let effective_qty = if qty == 0 { default_qty } else { qty };
    let tooling_per_part = material.tooling_cost_usd / (effective_qty.max(1) as f64);
    let per_core_surcharge = (core_count as f64) * 5.0;
    let total = material_cost_usd
        + tooling_per_part
        + per_core_surcharge
        + material.setup_cost_usd_default;
    CostEstimate {
        process,
        material: material.name.clone(),
        weight_grams: weight_g,
        material_cost_usd,
        machine_time_min: None,
        setup_cost_usd: material.setup_cost_usd_default,
        tooling_cost_usd: tooling_per_part,
        total_usd: total,
        is_estimate: true,
        assumptions: vec![
            format!("{} pour at ${:.2}/kg", material.name, material.price_per_kg),
            format!(
                "pour multiplier {:.2}x (sprue+risers), tooling ${:.0} / qty {}",
                pour_multiplier, material.tooling_cost_usd, effective_qty
            ),
            format!("{} core(s)", core_count),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fdm_back_compat_with_slicer_numbers() {
        let pla = Material::pla();
        let est = estimate_fdm_from_volume(4000.0, 0.15, 3, 0.45, &pla);
        assert!(est.weight_grams > 0.0);
        assert!(est.material_cost_usd > 0.0);
        assert!(est.is_estimate);
        // Same envelope as the historical slicer test: small cube < $1 + setup.
        assert!(est.material_cost_usd < 1.0);
    }

    #[test]
    fn fdm_from_filament_matches_old_api() {
        let pla = Material::pla();
        let est = estimate_fdm_from_filament(5.0, &pla);
        assert_eq!(est.weight_grams, 5.0);
        assert!((est.material_cost_usd - 0.10).abs() < 0.01);
        assert!(!est.is_estimate);
    }

    #[test]
    fn cnc_volume_makes_sense() {
        let alu = Material::aluminum_6061();
        // 100x50x25 stock, 100x50x10 part → 75 cm³ removed.
        let est = estimate_cnc_from_removed_volume(125_000.0, 50_000.0, 2, &alu);
        assert_eq!(est.process, Process::Cnc3Axis);
        assert!(est.machine_time_min.unwrap() > 0.0);
        assert!(est.total_usd > est.material_cost_usd);
    }

    #[test]
    fn injection_tooling_amortizes_with_qty() {
        let abs = Material::abs();
        let cheap = estimate_injection(10_000.0, 10_000, &abs);
        let expensive = estimate_injection(10_000.0, 100, &abs);
        assert!(expensive.tooling_cost_usd > cheap.tooling_cost_usd);
    }

    #[test]
    fn casting_pour_multiplier_increases_material() {
        let alu = Material::cast_aluminum_a356();
        let part_vol = 100_000.0; // 100 cm³
        let est = estimate_casting(Process::CastingSand, part_vol, 100, 0, &alu);
        assert!(est.weight_grams > 0.0);
        assert!(est.material_cost_usd > est.weight_grams * alu.price_per_kg / 1000.0 * 0.99);
    }

    #[test]
    fn process_round_trip_str() {
        for p in [
            Process::Cnc3Axis,
            Process::Fdm,
            Process::Sla,
            Process::Injection,
            Process::SheetMetal,
            Process::CastingSand,
            Process::CastingInvestment,
        ] {
            let s = p.as_str();
            assert_eq!(Some(p), Process::from_str(s));
        }
    }
}
