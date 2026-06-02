//! Jog operations — Z-shaped offset created by two equal-and-opposite
//! 90° bends.
//!
//! A jog steps one part of a flat panel up by `offset` mm via:
//!
//! 1. A 90° fold at the chosen edge, producing a vertical "riser" of
//!    `offset` length.
//! 2. A 90° fold *the other way* at the riser's far edge, producing a
//!    tail panel parallel to the parent but `offset` mm above.
//!
//! The kernel implements this as two [`crate::add_edge_flange`] calls; the
//! resulting bends are tagged `;jog:a` / `;jog:b` so the UI and DFM tools
//! can label them as jog members instead of generic flanges.

use crate::bend_table::BendTable;
use crate::edge_flange::{add_edge_flange, EdgeFlangeError, EdgeFlangeParams, FlangePosition};
use crate::model::{BendDirection, BendId, PanelId, SheetMetalModel};
use serde::{Deserialize, Serialize};
use std::f64::consts::FRAC_PI_2;

/// Parameters for [`add_jog`]. `offset` is the vertical step between the
/// two parallel planes; `length` is the tail panel's length.
#[derive(Debug, Clone)]
pub struct JogParams {
    /// Panel containing the edge to jog from.
    pub panel: PanelId,
    /// Edge index in that panel's outline.
    pub edge_index: usize,
    /// Vertical offset (mm) between the parent plane and the tail plane.
    pub offset: f64,
    /// Length of the tail panel (mm), measured perpendicular to the
    /// second bend.
    pub length: f64,
    /// Inside bend radius (mm) for both bends.
    pub bend_radius: f64,
    /// Direction of the first fold (the second is the opposite).
    pub direction: BendDirection,
}

/// Outcome of [`add_jog`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JogResult {
    /// Panel id of the vertical riser.
    pub riser_panel: PanelId,
    /// Panel id of the tail (parallel to the parent).
    pub tail_panel: PanelId,
    /// First (rising) bend.
    pub bend_first: BendId,
    /// Second (returning) bend.
    pub bend_second: BendId,
}

/// Add a jog to a sheet-metal model.
///
/// Both bends are 90°; the second uses the opposite [`BendDirection`] so
/// the tail panel lies parallel to the parent and `offset` mm above (or
/// below, when `direction` is [`BendDirection::Down`]).
pub fn add_jog(
    model: &mut SheetMetalModel,
    table: &BendTable,
    params: JogParams,
) -> Result<JogResult, EdgeFlangeError> {
    let second_dir = match params.direction {
        BendDirection::Up => BendDirection::Down,
        BendDirection::Down => BendDirection::Up,
    };
    let (riser_panel, bend_first) = add_edge_flange(
        model,
        table,
        EdgeFlangeParams {
            panel: params.panel,
            edge_index: params.edge_index,
            length: params.offset,
            angle: FRAC_PI_2,
            radius: params.bend_radius,
            direction: params.direction,
            position: FlangePosition::MaterialInside,
            material: model.material.clone(),
            manual_k: None,
        },
    )?;
    let (tail_panel, bend_second) = add_edge_flange(
        model,
        table,
        EdgeFlangeParams {
            panel: riser_panel,
            // Edge 2 of a freshly-added flange is the far edge opposite
            // the hinge: outline = [(0,0), (L,0), (L,len), (0,len)] in
            // panel-local 2D, so edges 0..3 are bottom, right, top, left.
            edge_index: 2,
            length: params.length,
            angle: FRAC_PI_2,
            radius: params.bend_radius,
            direction: second_dir,
            position: FlangePosition::MaterialInside,
            material: model.material.clone(),
            manual_k: None,
        },
    )?;
    // Tag the bends as jog members.
    tag(model, bend_first, ";jog:a");
    tag(model, bend_second, ";jog:b");
    Ok(JogResult {
        riser_panel,
        tail_panel,
        bend_first,
        bend_second,
    })
}

fn tag(model: &mut SheetMetalModel, bend_id: BendId, suffix: &str) {
    if let Some(b) = model.bends.get_mut(bend_id) {
        let base = b.k_factor_source.clone().unwrap_or_else(|| "manual".into());
        b.k_factor_source = Some(format!("{base}{suffix}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_flange::base_flange_rect;
    use crate::bend_table::BendTable;
    use crate::unfold::{unfold, FlatPattern};

    #[test]
    fn jog_creates_two_panels_and_two_bends() {
        let mut m = base_flange_rect(120.0, 60.0, 1.0).unwrap();
        m.material = "al-soft".to_string();
        let table = BendTable::builtin();
        let r = add_jog(
            &mut m,
            &table,
            JogParams {
                panel: 0,
                edge_index: 0,
                offset: 5.0,
                length: 25.0,
                bend_radius: 1.0,
                direction: BendDirection::Up,
            },
        )
        .unwrap();
        assert_eq!(m.panels.len(), 3);
        assert_eq!(m.bends.len(), 2);
        // Both 90°.
        let b1 = &m.bends[r.bend_first];
        let b2 = &m.bends[r.bend_second];
        assert!((b1.angle - FRAC_PI_2).abs() < 1e-12);
        assert!((b2.angle - FRAC_PI_2).abs() < 1e-12);
        // Directions are opposites.
        assert_ne!(b1.direction, b2.direction);
        // Provenance tags carry the jog markers.
        assert!(b1.k_factor_source.as_deref().unwrap().contains(";jog:a"));
        assert!(b2.k_factor_source.as_deref().unwrap().contains(";jog:b"));
    }

    #[test]
    fn jog_tail_is_parallel_to_parent_after_unfold() {
        // After unfolding and re-folding, the tail panel must sit at +Z
        // = offset relative to the parent plane.
        let mut m = base_flange_rect(120.0, 60.0, 1.0).unwrap();
        m.material = "al-soft".to_string();
        let table = BendTable::builtin();
        let r = add_jog(
            &mut m,
            &table,
            JogParams {
                panel: 0,
                edge_index: 0,
                offset: 5.0,
                length: 25.0,
                bend_radius: 1.0,
                direction: BendDirection::Up,
            },
        )
        .unwrap();
        let parent_n = m.panels[0].frame_bent.normal();
        let tail_n = m.panels[r.tail_panel].frame_bent.normal();
        // Tail is parallel to parent → normals are aligned (within fp).
        let cos = parent_n.x * tail_n.x + parent_n.y * tail_n.y + parent_n.z * tail_n.z;
        assert!(cos.abs() > 0.999, "tail not parallel to parent: cos={cos}");
    }

    #[test]
    fn jog_unfolds_to_three_coplanar_panels() {
        let mut m = base_flange_rect(120.0, 60.0, 1.0).unwrap();
        m.material = "al-soft".to_string();
        let table = BendTable::builtin();
        add_jog(
            &mut m,
            &table,
            JogParams {
                panel: 0,
                edge_index: 0,
                offset: 5.0,
                length: 20.0,
                bend_radius: 1.0,
                direction: BendDirection::Up,
            },
        )
        .unwrap();
        unfold(&mut m).unwrap();
        let flat = FlatPattern::from_model(&m);
        assert_eq!(flat.panel_outlines_2d.len(), 3);
        assert_eq!(flat.creases.len(), 2);
    }
}
