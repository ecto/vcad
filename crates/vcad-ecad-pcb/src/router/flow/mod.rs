//! The "flow core" router: everything is min-cost flow under prices on one
//! graph (see `docs/router-one-market-math.md`).
//!
//! - [`cdt`] — per signal layer, a Delaunay triangulation of clearance-inflated
//!   obstacles; triangle edges are *cuts* with exact wire capacities.
//! - [`human`] — map an existing (human) routing onto the cut graph and
//!   compare per-cut load against capacity (the capacity-model sanity check).

pub mod cdt;
pub mod human;

/// A blank `w × h` test board with the given copper layers (signal role) and
/// 0.2/0.2 default rules.
#[cfg(test)]
pub(crate) fn test_board(w: f64, h: f64, layers: &[&str]) -> vcad_ir::ecad::Pcb {
    let layers: Vec<serde_json::Value> = layers
        .iter()
        .map(|l| serde_json::json!({"layer": l, "role": {"kind": "signal"}}))
        .collect();
    serde_json::from_value(serde_json::json!({
        "outline": {"vertices": [{"x":0.0,"y":0.0},{"x":w,"y":0.0},{"x":w,"y":h},{"x":0.0,"y":h}],
                    "thickness": 1.6},
        "stackup": {"layers": layers},
        "nets": [],
        "rules": {"defaultRules": {"name":"Default","traceWidth":0.2,"clearance":0.2,
                                   "viaDiameter":0.6,"viaDrill":0.3},
                  "edgeClearance":0.3,"holeToHole":0.25,"minAnnularRing":0.1,"minDrill":0.2},
        "footprints": [], "traces": [], "vias": [], "zones": []
    }))
    .expect("test board json")
}
