//! Hem operations — 180° folds at the edge of a flange.
//!
//! A hem stiffens an exposed sheet edge and removes the sharp burr left
//! by laser cutting. Two foundation-tier variants:
//!
//! - **Closed** — the back-flange touches the parent (gap ≈ 0). Modeled
//!   as a half-turn around a radius of `t/2`.
//! - **Open** — a user-specified `gap` between parent and back-flange.
//!   Modeled as a half-turn around a radius of `(gap + t)/2` so the
//!   back-flange ends up exactly `gap` mm above the parent.
//!
//! Teardrop and rolled hems require curved back-flanges and land with the
//! lofted-flange tier. Here a hem is a thin wrapper around
//! [`crate::add_edge_flange`] with the angle pinned at π — same lossless
//! unfold, DXF export, and DFM machinery picks it up for free.
//!
//! Provenance: each hem-generated [`crate::Bend`] gets its
//! `k_factor_source` tagged with a `;hem:closed|open` suffix so the UI can
//! label it as a hem rather than a generic 180° bend.

use crate::bend_table::BendTable;
use crate::edge_flange::{add_edge_flange, EdgeFlangeError, EdgeFlangeParams, FlangePosition};
use crate::model::{BendDirection, BendId, PanelId, SheetMetalModel};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Kind of hem fold. Round/teardrop variants land with the curved-flange
/// tier; for now Closed and Open cover the bulk of real parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum HemKind {
    /// Fully folded; the back-flange touches the parent.
    #[default]
    Closed,
    /// Folded with a gap between the parent and the back-flange.
    Open,
}

/// Parameters for [`add_hem`]. The `panel` + `edge_index` identify the
/// edge to fold over; `length` is the back-flange length (mm).
#[derive(Debug, Clone)]
pub struct HemParams {
    /// Panel containing the edge to hem.
    pub panel: PanelId,
    /// Edge index in that panel's outline.
    pub edge_index: usize,
    /// Hem kind (closed/open).
    pub kind: HemKind,
    /// Back-flange length perpendicular to the hinge (mm).
    pub length: f64,
    /// Gap between parent and back-flange (mm). Ignored for closed hems;
    /// must be `>= 0` for open hems and is clamped to at least the
    /// material thickness to keep the geometry physical.
    pub gap: f64,
    /// Fold direction.
    pub direction: BendDirection,
}

/// Add a hem to a sheet-metal model.
///
/// Returns the `(panel_id, bend_id)` pair created. The bend has its
/// `angle` set to π and its `radius` chosen so closed hems pack tight and
/// open hems honour `gap`.
pub fn add_hem(
    model: &mut SheetMetalModel,
    table: &BendTable,
    params: HemParams,
) -> Result<(PanelId, BendId), EdgeFlangeError> {
    let t = model.thickness;
    let radius = match params.kind {
        HemKind::Closed => t * 0.5,
        // For a 180° fold the back-flange sits 2R + t above the parent's
        // mid-plane; we want a gap of `gap` between the two outside
        // faces → 2R = gap, so R = gap/2. Add a tiny floor so the radius
        // never collapses to zero on a zero-gap "open" hem.
        HemKind::Open => (params.gap.max(0.0) * 0.5).max(t * 0.5),
    };
    let suffix = match params.kind {
        HemKind::Closed => ";hem:closed",
        HemKind::Open => ";hem:open",
    };
    let (panel_id, bend_id) = add_edge_flange(
        model,
        table,
        EdgeFlangeParams {
            panel: params.panel,
            edge_index: params.edge_index,
            length: params.length,
            angle: PI,
            radius,
            direction: params.direction,
            position: FlangePosition::MaterialInside,
            material: model.material.clone(),
            manual_k: None,
        },
    )?;
    // Tag the bend's provenance so the UI / DXF / agent can label it
    // as a hem rather than a generic 180° fold.
    if let Some(bend) = model.bends.get_mut(bend_id) {
        bend.append_source_tag(suffix);
    }
    Ok((panel_id, bend_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_flange::base_flange_rect;
    use crate::bend_table::BendTable;
    use crate::unfold::{unfold, FlatPattern};

    fn base() -> SheetMetalModel {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        m.material = "al-soft".to_string();
        m
    }

    #[test]
    fn closed_hem_creates_a_180_bend_with_tight_radius() {
        let mut m = base();
        let table = BendTable::builtin();
        let (_, bend_id) = add_hem(
            &mut m,
            &table,
            HemParams {
                panel: 0,
                edge_index: 0,
                kind: HemKind::Closed,
                length: 5.0,
                gap: 0.0,
                direction: BendDirection::Up,
            },
        )
        .unwrap();
        let bend = &m.bends[bend_id];
        assert!((bend.angle - PI).abs() < 1e-12);
        // Closed hem packs tight: radius = t/2.
        assert!((bend.radius - 0.5).abs() < 1e-9);
        // Provenance carries the hem tag.
        assert!(bend
            .k_factor_source
            .as_deref()
            .unwrap()
            .contains(";hem:closed"));
    }

    #[test]
    fn open_hem_radius_honours_gap() {
        let mut m = base();
        let table = BendTable::builtin();
        let (_, bend_id) = add_hem(
            &mut m,
            &table,
            HemParams {
                panel: 0,
                edge_index: 0,
                kind: HemKind::Open,
                length: 8.0,
                gap: 2.0,
                direction: BendDirection::Up,
            },
        )
        .unwrap();
        let bend = &m.bends[bend_id];
        // Open hem: R = gap/2 (since gap > t).
        assert!((bend.radius - 1.0).abs() < 1e-9, "got {}", bend.radius);
        assert!(bend
            .k_factor_source
            .as_deref()
            .unwrap()
            .contains(";hem:open"));
    }

    #[test]
    fn hem_round_trips_through_unfold() {
        let mut m = base();
        let table = BendTable::builtin();
        add_hem(
            &mut m,
            &table,
            HemParams {
                panel: 0,
                edge_index: 0,
                kind: HemKind::Closed,
                length: 6.0,
                gap: 0.0,
                direction: BendDirection::Up,
            },
        )
        .unwrap();
        // The hem panel must appear in the flat pattern.
        unfold(&mut m).unwrap();
        let flat = FlatPattern::from_model(&m);
        assert_eq!(flat.panel_outlines_2d.len(), 2);
        assert_eq!(flat.creases.len(), 1);
        assert!((flat.creases[0].angle - PI).abs() < 1e-12);
    }
}
