//! Bend sequence — the order to form bends on a press brake.
//!
//! The current heuristic is **outside-in**: deeper bends (further from
//! the model root) form first, so the remaining flat is smaller for each
//! subsequent setup and earlier bends never collide with the brake when
//! later (shallower) bends are formed. Ties are broken by bend id for
//! determinism.
//!
//! Real shops also factor in flange height (short flanges first),
//! back-gauge accessibility, and tooling changes; those land in a richer
//! version once a `BendInterferenceGraph` is built. For foundation tier
//! this heuristic produces correct-by-construction orderings on the
//! tree-shaped models the kernel currently supports.

use crate::model::{Bend, BendId, PanelId, SheetMetalModel};
use serde::Serialize;
use std::collections::HashSet;

/// One step in a bend sequence.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BendStep {
    /// Order index (0-based).
    pub step: usize,
    /// Bend this step forms.
    pub bend_id: BendId,
    /// Panels at the ends of the hinge.
    pub parent_panel: PanelId,
    /// Child panel created by this bend.
    pub child_panel: PanelId,
    /// Depth in the bend tree (1 = bend on the root panel).
    pub depth: usize,
    /// Bend angle target (radians).
    pub angle_rad: f64,
    /// Inside bend radius (mm).
    pub radius_mm: f64,
    /// The angle to actually form on the brake (target + springback).
    pub compensated_angle_rad: f64,
    /// Hinge length (mm) — what the brake must accommodate.
    pub hinge_length_mm: f64,
    /// Short rationale, surfaced in the UI.
    pub rationale: String,
}

/// Compute a bend sequence for a sheet-metal model.
///
/// Returns one [`BendStep`] per bend, in form-first order.
pub fn bend_sequence(model: &SheetMetalModel) -> Vec<BendStep> {
    let depths = bend_depths(model);
    let springback_factor = model.springback_per_radian();
    let mut ordered: Vec<BendId> = (0..model.bends.len()).collect();
    // Deeper first; tie-break by bend id ascending.
    ordered.sort_by(|a, b| depths[*b].cmp(&depths[*a]).then_with(|| a.cmp(b)));
    ordered
        .into_iter()
        .enumerate()
        .map(|(step, bend_id)| {
            let bend = &model.bends[bend_id];
            let depth = depths[bend_id];
            let hinge = hinge_length(bend);
            let springback = springback_factor * bend.angle;
            BendStep {
                step,
                bend_id,
                parent_panel: bend.parent,
                child_panel: bend.child,
                depth,
                angle_rad: bend.angle,
                radius_mm: bend.radius,
                compensated_angle_rad: bend.angle + springback,
                hinge_length_mm: hinge,
                rationale: rationale_for(bend, depth),
            }
        })
        .collect()
}

fn bend_depths(model: &SheetMetalModel) -> Vec<usize> {
    let mut depths = vec![0usize; model.bends.len()];
    if model.panels.is_empty() {
        return depths;
    }
    let mut panel_depth = vec![0usize; model.panels.len()];
    let mut visited = HashSet::new();
    visited.insert(model.root);
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(model.root);
    while let Some(panel) = queue.pop_front() {
        for &bend_id in &model.panels[panel].incident_bends {
            let bend = &model.bends[bend_id];
            let other = if bend.parent == panel {
                bend.child
            } else {
                bend.parent
            };
            if visited.insert(other) {
                panel_depth[other] = panel_depth[panel] + 1;
                depths[bend_id] = panel_depth[other]; // = panel depth of child
                queue.push_back(other);
            }
        }
    }
    depths
}

fn hinge_length(bend: &Bend) -> f64 {
    let (a, b) = bend.edge_parent;
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}

fn rationale_for(bend: &Bend, depth: usize) -> String {
    let is_hem = bend
        .k_factor_source
        .as_deref()
        .map(|s| s.contains(";hem:"))
        .unwrap_or(false);
    let is_jog = bend
        .k_factor_source
        .as_deref()
        .map(|s| s.contains(";jog:"))
        .unwrap_or(false);
    if is_hem {
        return format!("Hem at depth {depth} — formed early so it doesn't get caught between brake jaws on later bends.");
    }
    if is_jog {
        return format!(
            "Jog member at depth {depth} — form both halves of the Z before any outer flanges."
        );
    }
    if depth <= 1 {
        format!("Root-adjacent bend at depth {depth} — formed last so the bulk of the part stays flat as long as possible.")
    } else {
        format!("Depth {depth}: bent before its parent so the brake sees the smaller piece first.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_flange::base_flange_rect;
    use crate::bend_table::BendTable;
    use crate::edge_flange::{add_edge_flange, EdgeFlangeParams, FlangePosition};
    use crate::hem::{add_hem, HemKind, HemParams};
    use crate::model::BendDirection;
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

    #[test]
    fn empty_model_has_empty_sequence() {
        let model = base_flange_rect(50.0, 50.0, 1.0).unwrap();
        let seq = bend_sequence(&model);
        assert!(seq.is_empty());
    }

    #[test]
    fn single_flange_yields_one_step() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        m.material = "al-soft".to_string();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0, 25.0)).unwrap();
        let seq = bend_sequence(&m);
        assert_eq!(seq.len(), 1);
        assert_eq!(seq[0].step, 0);
        assert_eq!(seq[0].depth, 1);
        // Hinge length matches edge 0 of the 100×50 base.
        assert!((seq[0].hinge_length_mm - 100.0).abs() < 1e-9);
    }

    #[test]
    fn deeper_bends_form_first() {
        // L-bracket with a hem on the outer edge: hem (depth 2) before
        // the main flange (depth 1).
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        m.material = "al-soft".to_string();
        let table = BendTable::builtin();
        let (panel1, _b1) = add_edge_flange(&mut m, &table, flange(0, 0, 25.0)).unwrap();
        let _ = add_hem(
            &mut m,
            &table,
            HemParams {
                panel: panel1,
                edge_index: 2,
                kind: HemKind::Closed,
                length: 5.0,
                gap: 0.0,
                direction: BendDirection::Up,
            },
        )
        .unwrap();
        let seq = bend_sequence(&m);
        assert_eq!(seq.len(), 2);
        // First step = the hem (deeper).
        assert!(seq[0].depth >= seq[1].depth);
        assert!(seq[0].rationale.to_lowercase().contains("hem"));
    }

    #[test]
    fn ordering_is_deterministic() {
        // Two flanges off the same root — same depth, broken by id.
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        m.material = "al-soft".to_string();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0, 25.0)).unwrap();
        add_edge_flange(&mut m, &table, flange(0, 2, 25.0)).unwrap();
        let seq = bend_sequence(&m);
        assert_eq!(seq.len(), 2);
        assert_eq!(seq[0].bend_id, 0);
        assert_eq!(seq[1].bend_id, 1);
    }
}
