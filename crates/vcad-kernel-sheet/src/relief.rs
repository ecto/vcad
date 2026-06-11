//! Parametric bend-relief notches.
//!
//! A press brake's die deforms a zone around the bend line. Where a bend
//! line's endpoint sits against adjacent material — e.g. a flange whose
//! hinge ends at a chamfered body corner — that material drags and ripples
//! when the flange folds (SendCutSend flags this as *Relief Insufficient*).
//! The standard fix is a **relief notch**: a slot at the bend-line end, at
//! least material-thickness wide, reaching past the bend centerline to the
//! shop's relief depth.
//!
//! [`add_bend_relief`] / [`add_all_bend_reliefs`] cut those notches into
//! the **parent panel outline**, so every downstream view stays consistent
//! by construction: the flat pattern ([`crate::unfold::FlatPattern`]), the
//! merged cut silhouette ([`crate::silhouette`]), the DXF
//! ([`crate::dxf`]), and the folded 3D body all derive from the same
//! outline.
//!
//! Geometry per relieved end: a **V-cut replacing the corner vertex** —
//! recede `w` along the hinge, dip `d` along the interior angle bisector,
//! return `w` along the adjacent edge:
//!
//! ```text
//!        interior            a1 = v − w·ê_hinge
//!           c                 c = v + d·b̂   (b̂ = interior bisector)
//!          ╱ ╲               b1 = v + w·ê_adjacent
//!  ── a1 ─╱   ╲─ b1 ── chamfer
//!  hinge   (v removed)
//!        bend zone (allowance strip below the hinge line)
//! ```
//!
//! The cut removes the corner material the die would drag, opening onto
//! the perimeter as a simple notch. When two hinges share the corner
//! (wing and tail roots on a polygonal body), their two requested notches
//! merge into a single V between the hinges — separate rectangular slots
//! would overlap, which is invalid geometry in 2D and 3D alike. The dip
//! depth into the parent is `relief_depth − BA/2` (the bend centerline
//! sits `BA/2` outward of the hinge line), clamped to at least half the
//! material thickness.
//!
//! Which ends need relief is decided by [`relief_needed`] — the same
//! query that backs the [`crate::manufacturability`] rule
//! `sheet.bend_relief`. Because one predicate drives both the check and
//! the fix, `add_all_bend_reliefs` always converges to a clean report.

use crate::model::{BendId, PanelId, SheetMetalModel};
use vcad_kernel_math::{Point2, Vec2};

/// Which end of a bend's hinge edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum HingeEnd {
    /// The `edge_parent.0` end.
    Start,
    /// The `edge_parent.1` end.
    End,
}

impl HingeEnd {
    /// Both ends, in deterministic order.
    pub const BOTH: [HingeEnd; 2] = [HingeEnd::Start, HingeEnd::End];
}

/// Parameters for relief-notch generation. `None` fields use defaults
/// derived from the model: width = material thickness, depth = inside
/// radius + thickness past the bend centerline.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReliefParams {
    /// Notch width along the hinge (mm). Must be ≥ thickness to do its
    /// job; defaults to exactly the thickness.
    pub width: Option<f64>,
    /// Relief depth (mm) measured past the **bend centerline** into the
    /// parent. Defaults to `bend.radius + thickness`.
    pub depth: Option<f64>,
}

/// Errors from [`add_bend_relief`] / [`add_all_bend_reliefs`].
#[derive(Debug, Clone, PartialEq)]
pub enum ReliefError {
    /// `bend_id` out of bounds.
    UnknownBend(BendId),
    /// The bend's hinge edge is no longer a contiguous edge of the parent
    /// outline (e.g. relief was already applied to it).
    HingeEdgeNotFound {
        /// The bend whose hinge could not be located.
        bend_id: BendId,
    },
    /// Notch width/depth is non-positive or the notches would consume the
    /// hinge (2·width ≥ hinge length).
    BadGeometry(&'static str),
}

impl std::fmt::Display for ReliefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReliefError::UnknownBend(b) => write!(f, "unknown bend {b}"),
            ReliefError::HingeEdgeNotFound { bend_id } => write!(
                f,
                "bend {bend_id}'s hinge is not a contiguous parent outline edge (already relieved?)"
            ),
            ReliefError::BadGeometry(what) => write!(f, "bad relief geometry: {what}"),
        }
    }
}

impl std::error::Error for ReliefError {}

/// One notch that was cut.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppliedRelief {
    /// Bend the notch relieves.
    pub bend_id: BendId,
    /// Which hinge end.
    pub end: HingeEnd,
    /// Parent panel whose outline was modified.
    pub panel_id: PanelId,
    /// Notch width along the hinge (mm).
    pub width_mm: f64,
    /// Notch depth into the parent from the hinge line (mm).
    pub depth_mm: f64,
}

const EPS: f64 = 1e-9;

/// Does `end` of `bend_id`'s hinge need a relief notch?
///
/// Two conditions, both required:
///
/// 1. **Adjacent material** — the parent outline continues past the hinge
///    end with a lateral component along the hinge line (a chamfered body
///    corner, or a collinear edge continuation). A perpendicular free
///    corner (plain rectangle) has nothing in the die zone and is fine.
/// 2. **Relief zone still solid** — a probe just inside the hinge end on
///    the parent side (~`0.45·t` in each direction, inside any minimum
///    `t × t/2` notch) hits material. Once a notch is cut the probe falls
///    in the void, making the query — and therefore
///    [`add_all_bend_reliefs`] — idempotent.
pub fn relief_needed(model: &SheetMetalModel, bend_id: BendId, end: HingeEnd) -> bool {
    let Some(bend) = model.bends.get(bend_id) else {
        return false;
    };
    let Some(parent) = model.panels.get(bend.parent) else {
        return false;
    };
    let (p0, p1) = bend.edge_parent;
    let hinge = p1 - p0;
    let len = hinge.norm();
    if len < EPS {
        return false;
    }
    let e = hinge / len; // along hinge, p0 → p1
    let n_in = Vec2::new(-e.y, e.x); // interior of a CCW outline
    let (anchor, lateral) = match end {
        HingeEnd::Start => (p0, Vec2::new(-e.x, -e.y)),
        HingeEnd::End => (p1, e),
    };

    // 1. Direction the outline leaves the hinge-end vertex (excluding the
    //    hinge edge itself): the next edge for End, the previous for Start.
    let n = parent.outline.len();
    let Some(k) = (0..n).find(|&i| (parent.outline[i] - anchor).norm() < 1e-6) else {
        return false;
    };
    let neighbor = match end {
        HingeEnd::End => parent.outline[(k + 1) % n],
        HingeEnd::Start => parent.outline[(k + n - 1) % n],
    };
    let u = neighbor - anchor;
    let u_len = u.norm();
    if u_len < EPS {
        return false;
    }
    if (u.x * lateral.x + u.y * lateral.y) / u_len <= 1e-9 {
        return false; // free corner — outline turns away from the die zone
    }

    // 2. Relief zone just inside the hinge end, on the parent side.
    let d = 0.45 * model.thickness;
    let probe_relief = Point2::new(
        anchor.x - lateral.x * d + n_in.x * d,
        anchor.y - lateral.y * d + n_in.y * d,
    );
    point_in_outline(probe_relief, &parent.outline)
}

/// Cut relief notches at whichever ends of `bend_id` need them.
///
/// Returns the notches applied (possibly empty when both ends are free
/// corners). The parent panel's outline is modified in place; callers
/// should re-run [`crate::unfold::unfold`] afterwards if flat frames are
/// already computed (notches don't move frames, so this is only needed
/// for downstream consumers that cached the outline).
pub fn add_bend_relief(
    model: &mut SheetMetalModel,
    bend_id: BendId,
    params: ReliefParams,
) -> Result<Vec<AppliedRelief>, ReliefError> {
    if bend_id >= model.bends.len() {
        return Err(ReliefError::UnknownBend(bend_id));
    }
    apply_reliefs(model, &[bend_id], params)
}

/// Cut relief notches at every bend end in the model that needs one.
///
/// Detection runs against the pristine outlines first, then all notches
/// are applied in one rebuild per panel — so adjacent bends sharing a
/// corner (wing/tail roots on a polygonal body) each get their own notch
/// regardless of processing order.
pub fn add_all_bend_reliefs(
    model: &mut SheetMetalModel,
    params: ReliefParams,
) -> Result<Vec<AppliedRelief>, ReliefError> {
    let ids: Vec<BendId> = (0..model.bends.len()).collect();
    apply_reliefs(model, &ids, params)
}

/// A notch to cut, resolved against a specific outline edge.
struct Notch {
    bend_id: BendId,
    end: HingeEnd,
    panel: PanelId,
    /// Index of the hinge edge in the parent outline (edge i = v[i] → v[i+1]).
    edge_index: usize,
    width: f64,
    depth: f64,
}

fn apply_reliefs(
    model: &mut SheetMetalModel,
    bend_ids: &[BendId],
    params: ReliefParams,
) -> Result<Vec<AppliedRelief>, ReliefError> {
    if let Some(w) = params.width {
        if w <= 0.0 || w.is_nan() {
            return Err(ReliefError::BadGeometry("width must be > 0"));
        }
    }
    if let Some(d) = params.depth {
        if d <= 0.0 || d.is_nan() {
            return Err(ReliefError::BadGeometry("depth must be > 0"));
        }
    }

    // Phase 1: detect against pristine outlines and resolve hinge edges.
    let mut notches: Vec<Notch> = Vec::new();
    for &bend_id in bend_ids {
        let bend = model
            .bends
            .get(bend_id)
            .ok_or(ReliefError::UnknownBend(bend_id))?;
        let ends: Vec<HingeEnd> = HingeEnd::BOTH
            .into_iter()
            .filter(|&end| relief_needed(model, bend_id, end))
            .collect();
        if ends.is_empty() {
            continue;
        }

        let parent = &model.panels[bend.parent];
        let (p0, p1) = bend.edge_parent;
        let hinge_len = (p1 - p0).norm();
        let width = params.width.unwrap_or(model.thickness);
        // Depth past the centerline → depth into the parent from the hinge
        // line. The centerline sits BA/2 outward of the hinge.
        let ba = bend.allowance(model.thickness);
        let depth_past_centerline = params.depth.unwrap_or(bend.radius + model.thickness);
        let depth = (depth_past_centerline - 0.5 * ba).max(0.5 * model.thickness);

        if 2.0 * width >= hinge_len {
            return Err(ReliefError::BadGeometry(
                "notches would consume the hinge edge",
            ));
        }

        let edge_index = find_outline_edge(&parent.outline, p0, p1)
            .ok_or(ReliefError::HingeEdgeNotFound { bend_id })?;
        for end in ends {
            notches.push(Notch {
                bend_id,
                end,
                panel: bend.parent,
                edge_index,
                width,
                depth,
            });
        }
    }

    // Phase 2: rebuild each affected panel outline once.
    //
    // Every relief is a **V-cut replacing the corner vertex**: recede `w`
    // along the incoming edge, dip `d` along the interior bisector,
    // return `w` along the outgoing edge. This shape is uniform across
    // both situations a hinge end can be in:
    //
    // - hinge meets a non-bend edge (chamfered body corner): the cut
    //   opens onto the perimeter as a simple zigzag notch — no enclosed
    //   hole, no pinch vertex;
    // - hinge meets another hinge (wing/tail roots sharing a body
    //   corner): the two requested notches merge into one V between the
    //   hinges. Two separate rectangular slots would overlap there —
    //   invalid geometry in the flat pattern and the folded body alike.
    let mut applied: Vec<AppliedRelief> = Vec::new();
    let mut panels: Vec<PanelId> = notches.iter().map(|n| n.panel).collect();
    panels.sort_unstable();
    panels.dedup();
    for panel_id in panels {
        let panel_notches: Vec<&Notch> = notches.iter().filter(|n| n.panel == panel_id).collect();
        let old = model.panels[panel_id].outline.clone();
        let n = old.len();

        // Notches anchored at each vertex: an End notch on the edge
        // ending there and/or a Start notch on the edge starting there.
        let cuts_at = |v: usize| -> (Option<&Notch>, Option<&Notch>) {
            let ending = panel_notches
                .iter()
                .find(|nt| (nt.edge_index + 1) % n == v && nt.end == HingeEnd::End)
                .copied();
            let starting = panel_notches
                .iter()
                .find(|nt| nt.edge_index == v && nt.end == HingeEnd::Start)
                .copied();
            (ending, starting)
        };
        let edge_dir_len = |i: usize| -> Option<(Vec2, f64)> {
            let d = old[(i + 1) % n] - old[i];
            let l = d.norm();
            (l >= EPS).then(|| (d / l, l))
        };

        let mut new_outline: Vec<Point2> = Vec::with_capacity(n + 2 * panel_notches.len());
        for (i, &vi) in old.iter().enumerate() {
            let (ending, starting) = cuts_at(i);
            if ending.is_none() && starting.is_none() {
                new_outline.push(vi);
                continue;
            }
            let prev_edge = (i + n - 1) % n;
            let (Some((ea, la)), Some((eb, lb))) = (edge_dir_len(prev_edge), edge_dir_len(i))
            else {
                new_outline.push(vi);
                continue;
            };
            let w = ending.or(starting).map(|nt| nt.width).unwrap_or(0.0);
            let depth = ending
                .iter()
                .chain(starting.iter())
                .map(|nt| nt.depth)
                .fold(0.0_f64, f64::max);
            // Clamp the recede/advance so the cut never consumes a short
            // neighbouring edge.
            let wa = w.min(0.4 * la);
            let wb = w.min(0.4 * lb);
            let na = Vec2::new(-ea.y, ea.x);
            let nb = Vec2::new(-eb.y, eb.x);
            let bisector = Vec2::new(na.x + nb.x, na.y + nb.y);
            let bl = bisector.norm();
            new_outline.push(offset(vi, ea, -wa));
            if bl >= EPS {
                new_outline.push(offset(vi, bisector / bl, depth));
            }
            new_outline.push(offset(vi, eb, wb));
        }
        model.panels[panel_id].outline = new_outline;
        for nt in panel_notches {
            applied.push(AppliedRelief {
                bend_id: nt.bend_id,
                end: nt.end,
                panel_id: nt.panel,
                width_mm: nt.width,
                depth_mm: nt.depth,
            });
        }
    }
    Ok(applied)
}

fn offset(p: Point2, dir: Vec2, by: f64) -> Point2 {
    Point2::new(p.x + dir.x * by, p.y + dir.y * by)
}

/// Find the outline edge whose endpoints match `(p0, p1)`.
fn find_outline_edge(outline: &[Point2], p0: Point2, p1: Point2) -> Option<usize> {
    let n = outline.len();
    (0..n).find(|&i| (outline[i] - p0).norm() < 1e-6 && (outline[(i + 1) % n] - p1).norm() < 1e-6)
}

/// Even-odd point-in-polygon test (ray cast along +x).
pub(crate) fn point_in_outline(p: Point2, outline: &[Point2]) -> bool {
    let n = outline.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let a = outline[i];
        let b = outline[j];
        if ((a.y > p.y) != (b.y > p.y)) && (p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_flange::{base_flange_polygon, base_flange_rect};
    use crate::bend_table::BendTable;
    use crate::edge_flange::{add_edge_flange, EdgeFlangeParams, FlangePosition};
    use crate::model::BendDirection;
    use std::f64::consts::FRAC_PI_2;

    fn flange_params(panel: usize, edge: usize, length: f64, angle: f64) -> EdgeFlangeParams {
        EdgeFlangeParams {
            panel,
            edge_index: edge,
            length,
            angle,
            radius: 0.5,
            direction: BendDirection::Up,
            position: FlangePosition::MaterialInside,
            material: "Al-soft".into(),
            manual_k: None,
        }
    }

    /// Hexagonal crane body with a neck flange off the front edge — the
    /// hinge ends meet chamfered corners on both sides.
    fn crane_neck_model() -> SheetMetalModel {
        let outline = vec![
            Point2::new(20.0, 0.0),
            Point2::new(40.0, 0.0),
            Point2::new(60.0, 40.0),
            Point2::new(40.0, 80.0),
            Point2::new(20.0, 80.0),
            Point2::new(0.0, 40.0),
        ];
        let mut m = base_flange_polygon(outline, 0.5).unwrap();
        m.material = "Al-soft".into();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange_params(0, 0, 35.0, 1.1)).unwrap();
        m
    }

    #[test]
    fn rect_corner_is_a_free_corner() {
        // L-bracket: hinge ends at perpendicular rectangle corners — no
        // material past the hinge ends, no relief needed.
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange_params(0, 0, 25.0, FRAC_PI_2)).unwrap();
        assert!(!relief_needed(&m, 0, HingeEnd::Start));
        assert!(!relief_needed(&m, 0, HingeEnd::End));
        let applied = add_all_bend_reliefs(&mut m, ReliefParams::default()).unwrap();
        assert!(applied.is_empty());
        assert_eq!(m.panels[0].outline.len(), 4, "outline untouched");
    }

    #[test]
    fn chamfered_corner_needs_relief_at_both_ends() {
        let m = crane_neck_model();
        assert!(relief_needed(&m, 0, HingeEnd::Start));
        assert!(relief_needed(&m, 0, HingeEnd::End));
    }

    #[test]
    fn notches_cut_corner_material_and_clear_the_probe() {
        let mut m = crane_neck_model();
        let before = polygon_area(&m.panels[0].outline);
        let applied = add_all_bend_reliefs(&mut m, ReliefParams::default()).unwrap();
        assert_eq!(applied.len(), 2, "both hinge ends notched: {applied:?}");
        assert!(
            (applied[0].width_mm - 0.5).abs() < 1e-12,
            "width defaults to thickness"
        );
        // Each V-cut replaces 1 corner vertex with 3 (a1, c, b1).
        assert_eq!(m.panels[0].outline.len(), 6 + 2 * 2);
        let after = polygon_area(&m.panels[0].outline);
        assert!(
            after < before - 1e-9,
            "V-cuts must remove material ({before} → {after})"
        );
        // The query is now clean — applying again is a no-op.
        assert!(!relief_needed(&m, 0, HingeEnd::Start));
        assert!(!relief_needed(&m, 0, HingeEnd::End));
    }

    #[test]
    fn depth_reaches_past_the_bend_centerline() {
        let mut m = crane_neck_model();
        let bend = m.bends[0].clone();
        let ba = bend.allowance(m.thickness);
        let applied = add_all_bend_reliefs(&mut m, ReliefParams::default()).unwrap();
        // depth into parent + BA/2 (hinge → centerline) = requested reach.
        let reach = applied[0].depth_mm + 0.5 * ba;
        let expected = bend.radius + m.thickness;
        assert!(
            (reach - expected).abs() < 1e-9,
            "reach {reach} != R+t {expected}"
        );
    }

    #[test]
    fn explicit_params_override_defaults() {
        let mut m = crane_neck_model();
        let applied = add_bend_relief(
            &mut m,
            0,
            ReliefParams {
                width: Some(2.0),
                depth: Some(3.0),
            },
        )
        .unwrap();
        assert_eq!(applied.len(), 2);
        assert!((applied[0].width_mm - 2.0).abs() < 1e-12);
        let ba = m.bends[0].allowance(m.thickness);
        assert!((applied[0].depth_mm - (3.0 - 0.5 * ba)).abs() < 1e-9);
    }

    #[test]
    fn rejects_bad_params_and_unknown_bend() {
        let mut m = crane_neck_model();
        assert!(matches!(
            add_bend_relief(&mut m, 99, ReliefParams::default()),
            Err(ReliefError::UnknownBend(99))
        ));
        assert!(matches!(
            add_bend_relief(
                &mut m,
                0,
                ReliefParams {
                    width: Some(-1.0),
                    depth: None
                }
            ),
            Err(ReliefError::BadGeometry(_))
        ));
        // Notches wider than half the 20 mm hinge.
        assert!(matches!(
            add_bend_relief(
                &mut m,
                0,
                ReliefParams {
                    width: Some(10.0),
                    depth: None
                }
            ),
            Err(ReliefError::BadGeometry(_))
        ));
    }

    #[test]
    fn second_application_on_relieved_hinge_is_a_noop() {
        let mut m = crane_neck_model();
        add_all_bend_reliefs(&mut m, ReliefParams::default()).unwrap();
        let verts = m.panels[0].outline.len();
        // The probes are clean, so no notch is attempted and the broken
        // hinge-edge lookup is never reached.
        let applied = add_all_bend_reliefs(&mut m, ReliefParams::default()).unwrap();
        assert!(applied.is_empty());
        assert_eq!(m.panels[0].outline.len(), verts);
    }

    fn polygon_area(ring: &[Point2]) -> f64 {
        let mut sum = 0.0;
        for i in 0..ring.len() {
            let a = ring[i];
            let b = ring[(i + 1) % ring.len()];
            sum += a.x * b.y - b.x * a.y;
        }
        0.5 * sum.abs()
    }
}
