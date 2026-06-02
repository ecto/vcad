//! Costing as a derivative of the model.
//!
//! The spec's bet: costing should be a *pure function of the IR + a shop's
//! rates*, surfaced live in the UI so designers see the part get cheaper as
//! they fix it. No "send to shop, wait three days for a quote" loop.
//!
//! Foundation-tier model — five components, each transparently reported:
//!
//! 1. **Material** — mass × `$/kg`. Mass = flat area × thickness × density
//!    (density from the materials registry, so changing alloy updates cost).
//! 2. **Cut** — cut length × `$/m`. Cut length = sum of all closed
//!    polylines in the flat pattern (outer outlines + hole loops).
//! 3. **Pierce** — one charge per hole loop × `$/pierce`.
//! 4. **Bend** — bend count × `$/bend` (brake time amortized).
//! 5. **Setup** — one-time per-run × `1/qty`.
//!
//! Plus a `markup_pct` shop margin on the subtotal.
//!
//! `CostRates::generic()` gives reasonable low-volume laser-cut defaults
//! (USD); shops calibrate against a couple of real quotes and the model
//! stays accurate. Same field-tolerant deserialization as `ShopProfile`.

use crate::materials::lookup_or_unknown as lookup_material;
use crate::model::SheetMetalModel;
use crate::unfold::FlatPattern;
use serde::{Deserialize, Serialize};
use vcad_kernel_math::Point2;

/// Shop pricing inputs to [`estimate_cost`]. All rates per-unit-process —
/// the function below multiplies them by what the model needs.
///
/// Deserialization is field-tolerant: omitted keys fall back to
/// [`CostRates::generic`] so an older saved rate sheet still loads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostRates {
    /// Currency label for display (e.g. `"USD"`, `"EUR"`).
    pub currency: String,
    /// Raw stock cost per kilogram.
    pub material_usd_per_kg: f64,
    /// Cutting cost per metre of cut path (laser/waterjet/plasma blended).
    pub cut_usd_per_m: f64,
    /// One-time pierce charge per hole loop.
    pub pierce_usd_each: f64,
    /// Per-bend brake-time charge.
    pub bend_usd_each: f64,
    /// One-time per-run setup (programming + first-article inspection).
    pub setup_usd: f64,
    /// Shop margin (percent) applied to the subtotal.
    pub markup_pct: f64,
}

impl CostRates {
    /// Reasonable low-volume laser-cut defaults — close enough that a
    /// designer sees real numbers from the first part. Shops dial these
    /// in against two or three real quotes.
    pub fn generic() -> Self {
        Self {
            currency: "USD".to_string(),
            material_usd_per_kg: 5.0,
            cut_usd_per_m: 1.20,
            pierce_usd_each: 0.10,
            bend_usd_each: 0.75,
            setup_usd: 25.0,
            markup_pct: 30.0,
        }
    }
}

impl Default for CostRates {
    fn default() -> Self {
        Self::generic()
    }
}

/// A line-itemed cost estimate. All `*_each` fields are per finished part;
/// `total_run` is the full job at `quantity`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CostBreakdown {
    /// Currency from the rates that produced this estimate.
    pub currency: String,
    /// Quantity used for setup amortization.
    pub quantity: u32,

    // ── Per-unit line items ──
    /// Raw material cost (mass × $/kg).
    pub material_each: f64,
    /// Cutting cost (cut length × $/m).
    pub cut_each: f64,
    /// Pierce charges (holes × $/pierce).
    pub pierce_each: f64,
    /// Bending charges (bends × $/bend).
    pub bend_each: f64,
    /// Amortized setup per part.
    pub setup_each: f64,
    /// Sum of the five lines above.
    pub subtotal_each: f64,
    /// Markup amount per part.
    pub markup_each: f64,
    /// Total per part.
    pub total_each: f64,
    /// Total for the whole run.
    pub total_run: f64,

    // ── Inputs that fed the calc (transparency) ──
    /// Mass per finished part (kg).
    pub mass_kg_each: f64,
    /// Cut length per part (metres).
    pub cut_length_m: f64,
    /// Number of pierces (holes) per part.
    pub pierces: u32,
    /// Number of bends per part.
    pub bends: u32,
}

/// Compute a transparent line-itemed cost for a sheet-metal model.
///
/// Pure function of `(model, flat, qty, rates)` — no I/O, deterministic.
/// `qty` is clamped to `>= 1` so the function is total.
pub fn estimate_cost(
    model: &SheetMetalModel,
    flat: &FlatPattern,
    qty: u32,
    rates: &CostRates,
) -> CostBreakdown {
    let qty = qty.max(1);
    let material = lookup_material(&model.material);

    // Mass from flat area (incl. bend strips) × thickness × density.
    // area_mm2 is mm² and thickness mm → volume in mm³ → m³ via 1e-9.
    let volume_m3 = flat.area_mm2 * model.thickness * 1e-9;
    let mass_kg = volume_m3 * material.density_kg_m3;

    // Cut length: outer panel outlines + hole loops. Sum closed-polyline
    // perimeters in mm, convert to metres.
    let outline_mm: f64 = flat
        .panel_outlines_2d
        .iter()
        .map(|loop_pts| closed_polyline_length(loop_pts))
        .sum();
    let hole_mm: f64 = flat
        .panel_holes_2d
        .iter()
        .flat_map(|panel_holes| panel_holes.iter())
        .map(|loop_pts| closed_polyline_length(loop_pts))
        .sum();
    let cut_length_m = (outline_mm + hole_mm) / 1000.0;

    let pierces: u32 = flat
        .panel_holes_2d
        .iter()
        .map(|panel_holes| panel_holes.len() as u32)
        .sum();
    let bends: u32 = model.bends.len() as u32;

    let material_each = mass_kg * rates.material_usd_per_kg;
    let cut_each = cut_length_m * rates.cut_usd_per_m;
    let pierce_each = f64::from(pierces) * rates.pierce_usd_each;
    let bend_each = f64::from(bends) * rates.bend_usd_each;
    let setup_each = rates.setup_usd / f64::from(qty);

    let subtotal_each = material_each + cut_each + pierce_each + bend_each + setup_each;
    let markup_each = subtotal_each * rates.markup_pct / 100.0;
    let total_each = subtotal_each + markup_each;
    let total_run = total_each * f64::from(qty);

    CostBreakdown {
        currency: rates.currency.clone(),
        quantity: qty,
        material_each,
        cut_each,
        pierce_each,
        bend_each,
        setup_each,
        subtotal_each,
        markup_each,
        total_each,
        total_run,
        mass_kg_each: mass_kg,
        cut_length_m,
        pierces,
        bends,
    }
}

fn closed_polyline_length(pts: &[Point2]) -> f64 {
    if pts.len() < 2 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        sum += (b - a).norm();
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_flange::base_flange_rect;
    use crate::bend_table::BendTable;
    use crate::edge_flange::{add_edge_flange, EdgeFlangeParams, FlangePosition};
    use crate::model::BendDirection;
    use crate::unfold::{unfold, FlatPattern};
    use std::f64::consts::FRAC_PI_2;

    fn flange(panel: usize, edge: usize, length: f64) -> EdgeFlangeParams {
        EdgeFlangeParams {
            panel,
            edge_index: edge,
            length,
            angle: FRAC_PI_2,
            radius: 1.0,
            direction: BendDirection::Up,
            position: FlangePosition::MaterialInside,
            material: "al-soft".into(),
            manual_k: None,
        }
    }

    fn l_bracket_flat() -> (SheetMetalModel, FlatPattern) {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        m.material = "al-soft".to_string();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0, 25.0)).unwrap();
        unfold(&mut m).unwrap();
        let flat = FlatPattern::from_model(&m);
        (m, flat)
    }

    #[test]
    fn rates_round_trip_json_with_partial_fields() {
        // Field-tolerant: omitted keys fall back to generic.
        let r: CostRates = serde_json::from_str(r#"{"material_usd_per_kg": 9.5}"#).unwrap();
        assert!((r.material_usd_per_kg - 9.5).abs() < 1e-9);
        assert_eq!(r.currency, CostRates::generic().currency);
    }

    #[test]
    fn estimate_breakdown_is_self_consistent() {
        let (model, flat) = l_bracket_flat();
        let rates = CostRates::generic();
        let q = 100;
        let c = estimate_cost(&model, &flat, q, &rates);
        // Subtotal is the sum of the five line items.
        let sum = c.material_each + c.cut_each + c.pierce_each + c.bend_each + c.setup_each;
        assert!((c.subtotal_each - sum).abs() < 1e-9);
        // Total = subtotal + markup.
        assert!((c.total_each - (c.subtotal_each + c.markup_each)).abs() < 1e-9);
        // total_run == total_each * qty.
        assert!((c.total_run - c.total_each * f64::from(q)).abs() < 1e-6);
        // Bend count + pierce count match the model/flat.
        assert_eq!(c.bends, model.bends.len() as u32);
        assert_eq!(c.pierces, 0);
        // Mass is positive — flat area is non-zero.
        assert!(c.mass_kg_each > 0.0);
        // Cut length is roughly the L-bracket perimeter (~350+ mm = 0.35+ m).
        assert!(c.cut_length_m > 0.30);
    }

    #[test]
    fn higher_quantity_drops_per_unit_setup() {
        let (model, flat) = l_bracket_flat();
        let rates = CostRates::generic();
        let one = estimate_cost(&model, &flat, 1, &rates);
        let hundred = estimate_cost(&model, &flat, 100, &rates);
        // Setup amortizes: per-part setup at qty 100 is 1/100 of qty 1.
        assert!(hundred.setup_each < one.setup_each / 50.0);
        // Per-part total drops with volume.
        assert!(hundred.total_each < one.total_each);
    }

    #[test]
    fn switching_material_changes_cost() {
        let (mut model, flat) = l_bracket_flat();
        let rates = CostRates::generic();
        let al = estimate_cost(&model, &flat, 1, &rates);
        // Steel ~ 2.9× denser than aluminum → mass roughly 2.9× higher.
        model.material = "steel-mild".to_string();
        let steel = estimate_cost(&model, &flat, 1, &rates);
        assert!(
            steel.mass_kg_each > 2.5 * al.mass_kg_each,
            "got steel {} vs al {}",
            steel.mass_kg_each,
            al.mass_kg_each
        );
    }

    #[test]
    fn qty_zero_is_clamped_not_a_divide_by_zero() {
        let (model, flat) = l_bracket_flat();
        let rates = CostRates::generic();
        let c = estimate_cost(&model, &flat, 0, &rates);
        assert_eq!(c.quantity, 1);
        assert!(c.total_each.is_finite());
    }
}
