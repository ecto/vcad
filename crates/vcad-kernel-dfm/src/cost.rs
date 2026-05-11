//! Thin per-process wrappers over the shared
//! [`vcad_kernel_cost`] estimators.
//!
//! Each variant of [`Process`] gets one entry point that pulls the
//! numbers the estimator needs out of the BRep (volume, bbox, feature
//! count) and delegates to the shared crate. Higher-fidelity models
//! (real CAM time, actual sprue volume, FEA-aware section analysis) can
//! ride on top by replacing the body of any one of these functions
//! without touching callers.

use vcad_kernel_cost::{
    estimate_casting, estimate_cnc_from_removed_volume, estimate_fdm_from_volume,
    estimate_injection, estimate_sheet_metal, CostEstimate, Material, Process,
};
use vcad_kernel_primitives::BRepSolid;

use crate::geom;

/// Estimate manufacturing cost for a BRep in a given process.
///
/// `qty` is used for amortizing tooling costs on mold/casting (set to 0
/// to use the per-process default in the catalog). `feature_count` is a
/// CNC complexity proxy and is ignored for non-machining processes.
pub fn estimate_for_process(
    brep: &BRepSolid,
    process: Process,
    material: &Material,
    qty: u32,
    feature_count: u32,
) -> CostEstimate {
    let (lo, hi) = geom::brep_bbox(brep);
    let bbox_vol = (hi[0] - lo[0]) * (hi[1] - lo[1]) * (hi[2] - lo[2]);
    let part_vol = geom::approximate_part_volume_mm3(brep);
    match process {
        Process::Fdm | Process::Sla => estimate_fdm_from_volume(part_vol, 0.20, 3, 0.45, material),
        Process::Cnc3Axis => {
            estimate_cnc_from_removed_volume(bbox_vol, part_vol, feature_count, material)
        }
        Process::Injection => {
            let q = if qty == 0 { 1000 } else { qty };
            estimate_injection(part_vol, q, material)
        }
        Process::SheetMetal => {
            // Sheet metal cost wants blank area + thickness. Coarse
            // approximation from bbox until the sheet-feature pass
            // lands.
            let area = (hi[0] - lo[0]) * (hi[1] - lo[1]);
            let thickness = (hi[2] - lo[2]).max(0.5);
            estimate_sheet_metal(area, thickness, 0, material)
        }
        Process::CastingSand | Process::CastingInvestment => {
            estimate_casting(process, part_vol, qty, 0, material)
        }
    }
}
