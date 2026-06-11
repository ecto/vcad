//! Bend relief — parametric notches at bend ends.
//!
//! When a bend line ends *inside* the parent panel (a flange narrower than
//! its parent edge, or material continuing past the bend along a collinear
//! edge), the material adjacent to the bend end sits in the brake's
//! deformation zone and tears or wrinkles when formed. Fab services flag
//! this ("Relief Insufficient" at SendCutSend); shops fix it with a
//! rectangular **relief notch** cut at each affected bend end.
//!
//! Relief here is a *parametric feature of the model*, not a DXF
//! post-process: [`apply_bend_relief`] subtracts the notches from the parent
//! panel's outline, so the folded 3D body, the flat pattern, the DXF and
//! the STEP export all agree.
//!
//! Defaults (overridable per shop profile, since services publish
//! per-thickness values):
//!
//! - **width** = `1.5 × t`, min 1.0 mm
//! - **depth** = `inside_radius + t`, measured from the bend line
//! - **deformation half-width** (detection zone) = `die_width / 2`, with
//!   `die_width = 8 × t` when the shop doesn't specify a die

use crate::model::{BendId, PanelId, SheetMetalModel};
use crate::poly2d::{self, Poly};
use vcad_kernel_math::{Point2, Vec2};

/// Relief sizing parameters. `None` fields use the formula defaults above.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReliefParams {
    /// Notch width along the bend line (mm).
    pub width_mm: Option<f64>,
    /// Notch depth measured from the bend line into the parent (mm).
    pub depth_mm: Option<f64>,
    /// Brake die width (mm) — sets the deformation half-width used to
    /// decide whether a bend end needs relief.
    pub die_width_mm: Option<f64>,
}

impl ReliefParams {
    /// Effective notch width for thickness `t`.
    pub fn width(&self, t: f64) -> f64 {
        self.width_mm.unwrap_or((1.5 * t).max(1.0))
    }

    /// Effective notch depth for thickness `t` and inside radius `r`.
    pub fn depth(&self, t: f64, r: f64) -> f64 {
        self.depth_mm.unwrap_or(r + t)
    }

    /// Effective die width for thickness `t`.
    pub fn die_width(&self, t: f64) -> f64 {
        self.die_width_mm.unwrap_or(8.0 * t)
    }
}

/// A relief notch that should exist (or was applied) at one bend end.
#[derive(Debug, Clone, PartialEq)]
pub struct ReliefNotch {
    /// Bend whose end needs relief.
    pub bend_id: BendId,
    /// Parent panel the notch is cut from.
    pub panel_id: PanelId,
    /// Which end of the hinge: 0 = `edge_parent.0`, 1 = `edge_parent.1`.
    pub end: usize,
    /// Notch rectangle in parent-panel-local 2D coords.
    pub rect: [Point2; 4],
    /// Notch width along the bend line (mm).
    pub width_mm: f64,
    /// Notch depth from the bend line (mm).
    pub depth_mm: f64,
}

/// Errors from [`apply_bend_relief`].
#[derive(Debug, Clone, PartialEq)]
pub enum ReliefError {
    /// Subtracting the notches split a panel into multiple pieces (a notch
    /// wider/deeper than the surrounding material).
    PanelSplit {
        /// The panel that fell apart.
        panel_id: PanelId,
        /// Number of pieces the difference produced.
        pieces: usize,
    },
}

impl std::fmt::Display for ReliefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReliefError::PanelSplit { panel_id, pieces } => write!(
                f,
                "bend-relief notches split panel {panel_id} into {pieces} pieces — \
                 notch dimensions exceed the surrounding material"
            ),
        }
    }
}

impl std::error::Error for ReliefError {}

/// Geometry of one bend end in parent-local 2D.
struct BendEnd {
    /// The hinge endpoint.
    point: Point2,
    /// Unit vector along the hinge pointing *past* this end.
    beyond: Vec2,
    /// Unit vector from the hinge into the parent material.
    inward: Vec2,
}

fn bend_ends(model: &SheetMetalModel, bend_id: BendId) -> Option<[BendEnd; 2]> {
    let bend = model.bends.get(bend_id)?;
    let (p0, p1) = bend.edge_parent;
    let d = p1 - p0;
    let len = d.norm();
    if len < 1e-9 {
        return None;
    }
    let d = d / len;
    // Outward (toward the child / off the panel) for a CCW outline is the
    // edge direction rotated 90° clockwise; the parent material is inward.
    let inward = Vec2::new(-d.y, d.x);
    Some([
        BendEnd {
            point: p0,
            beyond: Vec2::new(-d.x, -d.y),
            inward,
        },
        BendEnd {
            point: p1,
            beyond: d,
            inward,
        },
    ])
}

fn panel_poly(model: &SheetMetalModel, panel_id: PanelId) -> Poly {
    let panel = &model.panels[panel_id];
    Poly {
        outer: panel.outline.clone(),
        holes: panel.holes.clone(),
    }
}

/// Does parent material sit in the deformation zone past this bend end?
///
/// Probes the would-be notch footprint (width × min(depth, die/2)) just
/// past the bend end. Probing the *notch* footprint — rather than the full
/// half-die zone — keeps the query idempotent: once the notch is cut, the
/// same query returns false.
fn needs_relief(parent: &Poly, end: &BendEnd, width: f64, depth: f64, die_half: f64) -> bool {
    let probe_depth = depth.min(die_half).max(0.2);
    let alongs = [0.1_f64.min(width * 0.25), width * 0.5, width * 0.9];
    let depths = [
        0.1_f64.min(probe_depth * 0.25),
        probe_depth * 0.5,
        probe_depth * 0.9,
    ];
    for &a in &alongs {
        for &dp in &depths {
            let p = Point2::new(
                end.point.x + end.beyond.x * a + end.inward.x * dp,
                end.point.y + end.beyond.y * a + end.inward.y * dp,
            );
            if poly2d::contains_point(parent, p) {
                return true;
            }
        }
    }
    false
}

/// Notch rectangle for a bend end. Pokes 10 µm past the bend line so the
/// boolean difference cleanly opens the notch at the part edge.
fn notch_rect(end: &BendEnd, width: f64, depth: f64) -> [Point2; 4] {
    const OPEN_FUZZ: f64 = 0.01;
    // dp < 0 = outside the part edge (opening), dp > 0 = into the material.
    let p = |a: f64, dp: f64| -> Point2 {
        Point2::new(
            end.point.x + end.beyond.x * a + end.inward.x * dp,
            end.point.y + end.beyond.y * a + end.inward.y * dp,
        )
    };
    [
        p(0.0, -OPEN_FUZZ),
        p(width, -OPEN_FUZZ),
        p(width, depth),
        p(0.0, depth),
    ]
}

/// Find every bend end that needs relief but doesn't have it.
///
/// Pure query — drives the `sheet.bend_relief` manufacturability rule and
/// the auto-fix path. Deterministic order: bends in id order, end 0 then 1.
pub fn find_missing_reliefs(model: &SheetMetalModel, params: &ReliefParams) -> Vec<ReliefNotch> {
    let t = model.thickness;
    let mut out = Vec::new();
    for (bend_id, bend) in model.bends.iter().enumerate() {
        let Some(ends) = bend_ends(model, bend_id) else {
            continue;
        };
        if model.panels.get(bend.parent).is_none() {
            continue;
        }
        let parent = panel_poly(model, bend.parent);
        let width = params.width(t);
        let depth = params.depth(t, bend.radius);
        let die_half = params.die_width(t) * 0.5;
        for (i, end) in ends.iter().enumerate() {
            if needs_relief(&parent, end, width, depth, die_half) {
                out.push(ReliefNotch {
                    bend_id,
                    panel_id: bend.parent,
                    end: i,
                    rect: notch_rect(end, width, depth),
                    width_mm: width,
                    depth_mm: depth,
                });
            }
        }
    }
    out
}

/// Cut every missing relief notch out of its parent panel.
///
/// Returns the number of notches applied. Recomputing is idempotent: a
/// second call finds nothing to cut. Callers should re-run
/// [`crate::unfold::unfold`] afterwards if they cached a flat pattern
/// (outlines changed, frames did not).
pub fn apply_bend_relief(
    model: &mut SheetMetalModel,
    params: &ReliefParams,
) -> Result<usize, ReliefError> {
    let notches = find_missing_reliefs(model, params);
    if notches.is_empty() {
        return Ok(0);
    }
    // Group notches by panel and subtract them in one boolean per panel.
    let mut by_panel: std::collections::BTreeMap<PanelId, Vec<Poly>> =
        std::collections::BTreeMap::new();
    for n in &notches {
        by_panel
            .entry(n.panel_id)
            .or_default()
            .push(Poly::new(n.rect.to_vec()));
    }
    for (panel_id, cuts) in by_panel {
        let subject = panel_poly(model, panel_id);
        let mut result = poly2d::difference(&[subject], &cuts);
        if result.len() != 1 {
            return Err(ReliefError::PanelSplit {
                panel_id,
                pieces: result.len(),
            });
        }
        let poly = result.swap_remove(0);
        let panel = &mut model.panels[panel_id];
        panel.outline = poly.outer;
        panel.holes = poly.holes;
    }
    Ok(notches.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_flange::{base_flange_polygon, base_flange_rect};
    use crate::bend_table::BendTable;
    use crate::edge_flange::{add_edge_flange, EdgeFlangeParams, FlangePosition};
    use crate::model::BendDirection;
    use std::f64::consts::FRAC_PI_2;

    fn flange(panel: usize, edge: usize) -> EdgeFlangeParams {
        EdgeFlangeParams {
            panel,
            edge_index: edge,
            length: 20.0,
            angle: FRAC_PI_2,
            radius: 1.0,
            direction: BendDirection::Up,
            position: FlangePosition::MaterialInside,
            material: "Al-soft".into(),
            manual_k: None,
        }
    }

    /// Base whose bottom edge (y = 0) is split into three collinear
    /// segments so a flange can take just the middle one — leaving parent
    /// material past both bend ends.
    fn partial_edge_model() -> crate::model::SheetMetalModel {
        let outline = vec![
            Point2::new(0.0, 0.0),
            Point2::new(30.0, 0.0), // edge 1 = (30,0)→(70,0): the flange edge
            Point2::new(70.0, 0.0),
            Point2::new(100.0, 0.0),
            Point2::new(100.0, 50.0),
            Point2::new(0.0, 50.0),
        ];
        let mut m = base_flange_polygon(outline, 1.0).unwrap();
        m.material = "al-soft".into();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 1)).unwrap();
        m
    }

    #[test]
    fn full_edge_flange_needs_no_relief() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0)).unwrap();
        let missing = find_missing_reliefs(&m, &ReliefParams::default());
        assert!(missing.is_empty(), "got {missing:?}");
    }

    #[test]
    fn partial_edge_flange_needs_relief_at_both_ends() {
        let m = partial_edge_model();
        let missing = find_missing_reliefs(&m, &ReliefParams::default());
        assert_eq!(missing.len(), 2, "got {missing:?}");
        assert_eq!(missing[0].end, 0);
        assert_eq!(missing[1].end, 1);
        // Default sizing for t=1, R=1: width max(1.5, 1.0) = 1.5, depth 2.0.
        assert!((missing[0].width_mm - 1.5).abs() < 1e-9);
        assert!((missing[0].depth_mm - 2.0).abs() < 1e-9);
    }

    #[test]
    fn apply_cuts_notches_and_is_idempotent() {
        let mut m = partial_edge_model();
        let area_before = crate::poly2d::Poly {
            outer: m.panels[0].outline.clone(),
            holes: m.panels[0].holes.clone(),
        }
        .area();
        let n = apply_bend_relief(&mut m, &ReliefParams::default()).unwrap();
        assert_eq!(n, 2);
        let area_after = crate::poly2d::Poly {
            outer: m.panels[0].outline.clone(),
            holes: m.panels[0].holes.clone(),
        }
        .area();
        // Two 1.5 × 2.0 notches (plus a 10 µm opening fuzz strip).
        let expected_removed = 2.0 * 1.5 * 2.0;
        assert!(
            (area_before - area_after - expected_removed).abs() < 0.05,
            "removed {}",
            area_before - area_after
        );
        // Second pass: nothing left to cut.
        let again = apply_bend_relief(&mut m, &ReliefParams::default()).unwrap();
        assert_eq!(again, 0);
        assert!(find_missing_reliefs(&m, &ReliefParams::default()).is_empty());
    }

    #[test]
    fn shop_override_drives_notch_size() {
        let m = partial_edge_model();
        let params = ReliefParams {
            width_mm: Some(3.0),
            depth_mm: Some(4.5),
            die_width_mm: Some(12.0),
        };
        let missing = find_missing_reliefs(&m, &params);
        assert_eq!(missing.len(), 2);
        assert!((missing[0].width_mm - 3.0).abs() < 1e-9);
        assert!((missing[0].depth_mm - 4.5).abs() < 1e-9);
    }

    #[test]
    fn unfold_still_works_after_relief() {
        let mut m = partial_edge_model();
        apply_bend_relief(&mut m, &ReliefParams::default()).unwrap();
        crate::unfold::unfold(&mut m).unwrap();
        let flat = crate::unfold::FlatPattern::from_model(&m);
        // Notched outline projects into the flat pattern (outline gained
        // vertices) and the merged silhouette still forms one region.
        assert!(flat.panel_outlines_2d[0].len() > 6);
        let sil = crate::silhouette::silhouette(&flat).unwrap();
        assert!(sil.exterior.len() > 8, "notches visible in silhouette");
    }
}
