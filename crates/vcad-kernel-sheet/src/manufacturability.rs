//! Manufacturability as a typed query.
//!
//! [`check_manufacturability`] takes a [`SheetMetalModel`] and a
//! [`ShopProfile`] and returns a structured list of [`Violation`]s — the
//! same data that backs the DFM inspector in the UI and (later) the
//! `sheet_metal.check` MCP tool. The point of the spec is that
//! manufacturability is a *query against the model*, not a post-hoc report:
//! every rule here is a pure function of the panel/bend graph + the shop's
//! capabilities, so an AI agent can call it, read the structured result, and
//! self-heal.
//!
//! Foundation-tier rule set (all computable from the current model):
//!
//! - **Bend radius below minimum** — `R < (R/t)_min · t` (Error).
//! - **Bend exceeds brake capacity** — hinge longer than the brake (Error).
//! - **Flange below minimum height** — too short to form on a press brake
//!   (Error).
//! - **Hole too close to bend** — punched feature inside the bend-relief
//!   zone, will distort (Warning).
//! - **Bends too close** — insufficient flat between two parallel bends on
//!   the same panel (Warning).
//!
//! Hems, jogs, back-gauge collision and grain rules join this list as the
//! operations that produce them land.

use crate::model::{BendId, PanelId, SheetMetalModel};
use serde::{Deserialize, Serialize};
use vcad_kernel_math::Point2;

/// A shop's manufacturing capabilities. Drives every rule in
/// [`check_manufacturability`]. Saved per-user and exportable as a JSON
/// profile; [`ShopProfile::generic`] gives sensible numbers so the
/// inspector shows real results from the first part.
///
/// Deserialization is field-tolerant: any key the caller omits falls back
/// to the [`ShopProfile::generic`] value (via `#[serde(default)]` +
/// [`Default`]), so an older saved profile still loads when new
/// capabilities are added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShopProfile {
    /// Human-readable name (e.g. `"Generic shop"`, `"Acme Machining"`).
    pub name: String,
    /// Maximum bend length the press brake can form (mm).
    pub max_bend_length_mm: f64,
    /// Minimum inside-bend-radius to thickness ratio `(R/t)_min` the
    /// **shop's tooling** can form. Independent of material: a shop with a
    /// big die set may still struggle to form tight bends regardless of
    /// alloy. The effective minimum used by [`check_manufacturability`] is
    /// `max(material.min_r_over_t, this)`.
    pub min_bend_radius_ratio: f64,
    /// Minimum formable flange height (mm) — shorter flanges can't be
    /// gripped between the punch and die.
    pub min_flange_height_mm: f64,
    /// Minimum distance from a punched hole to a bend line (mm); inside
    /// this zone the hole deforms into a slot.
    pub min_hole_to_bend_mm: f64,
    /// Minimum flat length between two parallel bends on the same panel
    /// (mm); the back-gauge needs a flat to register against.
    pub min_distance_between_bends_mm: f64,
    /// Required bend-relief depth (mm) past the bend centerline, where a
    /// bend end meets adjacent material. `0.0` (the default) means
    /// "auto": use `inside radius + thickness` per bend — the common rule
    /// of thumb. Shops that publish per-material relief tables
    /// (SendCutSend et al.) set this explicitly.
    pub relief_depth_mm: f64,
}

impl ShopProfile {
    /// A reasonable default shop: a 3 m brake, `R/t ≥ 1`, 5 mm minimum
    /// flange, 3 mm hole-to-bend, 6 mm bend-to-bend. Good enough that the
    /// inspector is useful before the user configures their own shop.
    pub fn generic() -> Self {
        Self {
            name: "Generic shop".to_string(),
            max_bend_length_mm: 3000.0,
            min_bend_radius_ratio: 1.0,
            min_flange_height_mm: 5.0,
            min_hole_to_bend_mm: 3.0,
            min_distance_between_bends_mm: 6.0,
            relief_depth_mm: 0.0, // auto: R + t per bend
        }
    }
}

impl Default for ShopProfile {
    /// Same as [`ShopProfile::generic`] — also the per-field fallback for
    /// tolerant deserialization.
    fn default() -> Self {
        Self::generic()
    }
}

/// Which constraint produced the bend-radius minimum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BendRadiusSource {
    /// The material would crack at a tighter radius.
    Material,
    /// The shop's tooling can't form a tighter radius.
    Shop,
}

/// How bad a [`Violation`] is. `Error` means the part cannot be built as
/// drawn; `Warning` means it will probably build but with quality risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Severity {
    /// Part cannot be manufactured as modelled.
    Error,
    /// Manufacturable, but with a quality / yield risk.
    Warning,
}

/// A structured manufacturability finding. Every variant carries the ids
/// needed to fly the camera to it and the measured-vs-required numbers a
/// fix suggestion needs.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind")]
pub enum Violation {
    /// Inside bend radius is tighter than the material allows.
    BendRadiusBelowMinimum {
        /// Offending bend.
        bend_id: BendId,
        /// Modelled inside radius (mm).
        actual_mm: f64,
        /// Minimum allowed radius for this thickness (mm). Equal to
        /// `max(material_min, shop_min) · t`.
        required_mm: f64,
        /// Material the minimum was computed against.
        material: String,
        /// Whether the strictest constraint came from the **material**
        /// (cracking fibre) or the **shop** floor override.
        source: BendRadiusSource,
    },
    /// Bend line is longer than the press brake can form in one hit.
    BendExceedsBrakeCapacity {
        /// Offending bend.
        bend_id: BendId,
        /// Hinge length (mm).
        actual_mm: f64,
        /// Brake maximum bend length (mm).
        required_mm: f64,
    },
    /// Flange is too short to grip in the press brake.
    FlangeBelowMinHeight {
        /// Bend that produced the flange.
        bend_id: BendId,
        /// The flange panel.
        panel_id: PanelId,
        /// Modelled flange height (mm).
        actual_mm: f64,
        /// Minimum formable height (mm).
        required_mm: f64,
    },
    /// A punched hole sits inside a bend's relief zone.
    HoleTooCloseToBend {
        /// The bend.
        bend_id: BendId,
        /// Panel the hole lives on.
        panel_id: PanelId,
        /// Index of the hole within `panel.holes`.
        hole_index: usize,
        /// Closest approach of the hole to the bend line (mm).
        actual_mm: f64,
        /// Required clearance (mm).
        required_mm: f64,
    },
    /// Two parallel bends on the same panel are too close to form.
    BendsTooClose {
        /// First bend.
        bend_id_a: BendId,
        /// Second bend.
        bend_id_b: BendId,
        /// Flat distance between the two hinges (mm).
        actual_mm: f64,
        /// Minimum flat the back-gauge needs (mm).
        required_mm: f64,
    },
    /// A bend line's end meets adjacent material without a relief notch —
    /// the die's deformation zone will drag and ripple the corner.
    BendEndNeedsRelief {
        /// The bend.
        bend_id: BendId,
        /// Parent panel carrying the adjacent material.
        panel_id: PanelId,
        /// Which hinge end (the `edge_parent.0` or `.1` side).
        end: crate::relief::HingeEnd,
        /// Required notch width along the hinge (mm) — the material
        /// thickness.
        required_width_mm: f64,
        /// Required notch reach past the bend centerline (mm).
        required_depth_mm: f64,
    },
}

impl Violation {
    /// Severity of this finding.
    pub fn severity(&self) -> Severity {
        match self {
            Violation::BendRadiusBelowMinimum { .. }
            | Violation::BendExceedsBrakeCapacity { .. }
            | Violation::FlangeBelowMinHeight { .. } => Severity::Error,
            Violation::HoleTooCloseToBend { .. }
            | Violation::BendsTooClose { .. }
            | Violation::BendEndNeedsRelief { .. } => Severity::Warning,
        }
    }

    /// Stable rule id, e.g. `"sheet.bend_radius"`. Used as a dedupe key and
    /// the anchor for fix suggestions.
    pub fn rule(&self) -> &'static str {
        match self {
            Violation::BendRadiusBelowMinimum { .. } => "sheet.bend_radius",
            Violation::BendExceedsBrakeCapacity { .. } => "sheet.brake_capacity",
            Violation::FlangeBelowMinHeight { .. } => "sheet.flange_height",
            Violation::HoleTooCloseToBend { .. } => "sheet.hole_to_bend",
            Violation::BendsTooClose { .. } => "sheet.bend_to_bend",
            Violation::BendEndNeedsRelief { .. } => "sheet.bend_relief",
        }
    }

    /// One-line human-readable summary for the inspector row.
    pub fn message(&self) -> String {
        match self {
            Violation::BendRadiusBelowMinimum {
                bend_id,
                actual_mm,
                required_mm,
                material,
                source,
            } => {
                let reason = match source {
                    BendRadiusSource::Material if !material.is_empty() => {
                        format!("{material} cracks below this")
                    }
                    BendRadiusSource::Material => "material cracks below this".to_string(),
                    BendRadiusSource::Shop => "shop tooling can't form tighter".to_string(),
                };
                format!(
                    "Bend #{bend_id} radius {actual_mm:.2} mm below minimum {required_mm:.2} mm — {reason}"
                )
            }
            Violation::BendExceedsBrakeCapacity {
                bend_id,
                actual_mm,
                required_mm,
            } => format!(
                "Bend #{bend_id} is {actual_mm:.0} mm long, brake maxes at {required_mm:.0} mm"
            ),
            Violation::FlangeBelowMinHeight {
                bend_id,
                actual_mm,
                required_mm,
                ..
            } => format!(
                "Flange off bend #{bend_id} is {actual_mm:.2} mm, minimum formable is {required_mm:.2} mm"
            ),
            Violation::HoleTooCloseToBend {
                bend_id,
                actual_mm,
                required_mm,
                ..
            } => format!(
                "Hole {actual_mm:.2} mm from bend #{bend_id}, needs {required_mm:.2} mm clearance"
            ),
            Violation::BendsTooClose {
                bend_id_a,
                bend_id_b,
                actual_mm,
                required_mm,
            } => format!(
                "Bends #{bend_id_a} and #{bend_id_b} are {actual_mm:.2} mm apart, need {required_mm:.2} mm"
            ),
            Violation::BendEndNeedsRelief {
                bend_id,
                end,
                required_width_mm,
                required_depth_mm,
                ..
            } => {
                let side = match end {
                    crate::relief::HingeEnd::Start => "start",
                    crate::relief::HingeEnd::End => "end",
                };
                format!(
                    "Bend #{bend_id} {side} meets adjacent material without relief — needs a ≥{required_width_mm:.1}×{required_depth_mm:.1} mm notch"
                )
            }
        }
    }
}

/// Check a sheet-metal model against a shop profile.
///
/// Pure function of `(model, shop)` — no I/O, deterministic order (bends in
/// id order, then bend-pair checks). Returns an empty vec for a shop-ready
/// part.
pub fn check_manufacturability(model: &SheetMetalModel, shop: &ShopProfile) -> Vec<Violation> {
    let mut out = Vec::new();
    let t = model.thickness;

    // Material drives the *physical* min R/t (cracking fibre); the shop is
    // a tooling-side floor. The strictest of the two wins, and we record
    // which one so the UI can explain. An empty `model.material` means the
    // user hasn't specified — defer to the shop alone rather than picking
    // an arbitrary fallback.
    let material = model.material_properties();
    let (min_ratio, radius_source) = match &material {
        Some(m) if m.min_r_over_t >= shop.min_bend_radius_ratio => {
            (m.min_r_over_t, BendRadiusSource::Material)
        }
        _ => (shop.min_bend_radius_ratio, BendRadiusSource::Shop),
    };
    let min_radius = min_ratio * t;
    let material_name = material
        .as_ref()
        .map(|m| m.name.clone())
        .unwrap_or_default();

    for (bend_id, bend) in model.bends.iter().enumerate() {
        // Bend radius vs (material ∨ shop) minimum.
        if bend.radius + 1e-9 < min_radius {
            out.push(Violation::BendRadiusBelowMinimum {
                bend_id,
                actual_mm: bend.radius,
                required_mm: min_radius,
                material: material_name.clone(),
                source: radius_source,
            });
        }

        // Hinge length vs brake capacity.
        let (h0, h1) = bend.edge_parent;
        let hinge_len = (h1 - h0).norm();
        if hinge_len > shop.max_bend_length_mm + 1e-9 {
            out.push(Violation::BendExceedsBrakeCapacity {
                bend_id,
                actual_mm: hinge_len,
                required_mm: shop.max_bend_length_mm,
            });
        }

        // Flange height: the child panel's extent perpendicular to the
        // hinge. `add_edge_flange` builds the child in a local frame whose
        // hinge lies on y = 0 and the flange extends +y, so the height is
        // the child outline's y-span.
        if let Some(child) = model.panels.get(bend.child) {
            let height = y_span(&child.outline);
            if height + 1e-9 < shop.min_flange_height_mm {
                out.push(Violation::FlangeBelowMinHeight {
                    bend_id,
                    panel_id: bend.child,
                    actual_mm: height,
                    required_mm: shop.min_flange_height_mm,
                });
            }
        }

        // Un-relieved bend ends: the hinge endpoint meets adjacent parent
        // material (chamfered corner, edge continuation) with no notch.
        // Same predicate the fix uses — `crate::relief::add_bend_relief`
        // clears exactly what this flags.
        for end in crate::relief::HingeEnd::BOTH {
            if crate::relief::relief_needed(model, bend_id, end) {
                let required_depth = if shop.relief_depth_mm > 0.0 {
                    shop.relief_depth_mm
                } else {
                    bend.radius + t
                };
                out.push(Violation::BendEndNeedsRelief {
                    bend_id,
                    panel_id: bend.parent,
                    end,
                    required_width_mm: t,
                    required_depth_mm: required_depth,
                });
            }
        }

        // Holes on the parent panel that crowd this bend line. The hinge
        // segment and the holes are both in parent-panel-local 2D.
        if let Some(parent) = model.panels.get(bend.parent) {
            for (hole_index, hole) in parent.holes.iter().enumerate() {
                let d = hole
                    .iter()
                    .map(|&p| dist_point_segment(p, h0, h1))
                    .fold(f64::INFINITY, f64::min);
                if d.is_finite() && d + 1e-9 < shop.min_hole_to_bend_mm {
                    out.push(Violation::HoleTooCloseToBend {
                        bend_id,
                        panel_id: bend.parent,
                        hole_index,
                        actual_mm: d,
                        required_mm: shop.min_hole_to_bend_mm,
                    });
                }
            }
        }
    }

    // Pairs of bends sharing a parent panel: need a flat between them.
    for a in 0..model.bends.len() {
        for b in (a + 1)..model.bends.len() {
            let ba = &model.bends[a];
            let bb = &model.bends[b];
            if ba.parent != bb.parent {
                continue;
            }
            let d = dist_segment_segment(
                ba.edge_parent.0,
                ba.edge_parent.1,
                bb.edge_parent.0,
                bb.edge_parent.1,
            );
            if d + 1e-9 < shop.min_distance_between_bends_mm {
                out.push(Violation::BendsTooClose {
                    bend_id_a: a,
                    bend_id_b: b,
                    actual_mm: d,
                    required_mm: shop.min_distance_between_bends_mm,
                });
            }
        }
    }

    out
}

fn y_span(outline: &[Point2]) -> f64 {
    if outline.is_empty() {
        return 0.0;
    }
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for p in outline {
        lo = lo.min(p.y);
        hi = hi.max(p.y);
    }
    hi - lo
}

fn dist_point_segment(p: Point2, a: Point2, b: Point2) -> f64 {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    if len2 < 1e-18 {
        return (p - a).norm();
    }
    let ap = p - a;
    let t = ((ap.x * ab.x + ap.y * ab.y) / len2).clamp(0.0, 1.0);
    let proj = Point2::new(a.x + ab.x * t, a.y + ab.y * t);
    (p - proj).norm()
}

fn segments_intersect(a0: Point2, a1: Point2, b0: Point2, b1: Point2) -> bool {
    let o = |p: Point2, q: Point2, r: Point2| -> f64 {
        (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x)
    };
    let d1 = o(b0, b1, a0);
    let d2 = o(b0, b1, a1);
    let d3 = o(a0, a1, b0);
    let d4 = o(a0, a1, b1);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

fn dist_segment_segment(a0: Point2, a1: Point2, b0: Point2, b1: Point2) -> f64 {
    if segments_intersect(a0, a1, b0, b1) {
        return 0.0;
    }
    dist_point_segment(a0, b0, b1)
        .min(dist_point_segment(a1, b0, b1))
        .min(dist_point_segment(b0, a0, a1))
        .min(dist_point_segment(b1, a0, a1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_flange::base_flange_rect;
    use crate::bend_table::BendTable;
    use crate::edge_flange::{add_edge_flange, EdgeFlangeParams, FlangePosition};
    use crate::model::BendDirection;
    use std::f64::consts::FRAC_PI_2;

    fn flange(panel: usize, edge: usize, length: f64, radius: f64) -> EdgeFlangeParams {
        EdgeFlangeParams {
            panel,
            edge_index: edge,
            length,
            angle: FRAC_PI_2,
            radius,
            direction: BendDirection::Up,
            position: FlangePosition::MaterialInside,
            material: "Al-soft".into(),
            manual_k: Some(0.42),
        }
    }

    #[test]
    fn clean_part_has_no_violations() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0, 25.0, 1.0)).unwrap();
        let v = check_manufacturability(&m, &ShopProfile::generic());
        assert!(v.is_empty(), "expected shop-ready, got {v:?}");
    }

    #[test]
    fn flags_tight_radius() {
        let mut m = base_flange_rect(100.0, 50.0, 2.0).unwrap();
        let table = BendTable::builtin();
        // R = 0.5 mm, t = 2 mm → R/t = 0.25, below the generic 1.0.
        add_edge_flange(&mut m, &table, flange(0, 0, 25.0, 0.5)).unwrap();
        let v = check_manufacturability(&m, &ShopProfile::generic());
        assert!(v
            .iter()
            .any(|x| matches!(x, Violation::BendRadiusBelowMinimum { .. })));
        assert_eq!(v[0].severity(), Severity::Error);
        assert_eq!(v[0].rule(), "sheet.bend_radius");
    }

    #[test]
    fn flags_short_flange() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0, 2.0, 1.0)).unwrap();
        let v = check_manufacturability(&m, &ShopProfile::generic());
        let f = v
            .iter()
            .find(|x| matches!(x, Violation::FlangeBelowMinHeight { .. }))
            .expect("flange-height violation");
        if let Violation::FlangeBelowMinHeight {
            actual_mm,
            required_mm,
            ..
        } = f
        {
            assert!((actual_mm - 2.0).abs() < 1e-6);
            assert!((required_mm - 5.0).abs() < 1e-6);
        }
        assert_eq!(f.severity(), Severity::Error);
    }

    #[test]
    fn flags_hole_too_close_to_bend() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = BendTable::builtin();
        // Edge 0 is (0,0)->(100,0): a hole near y=1 sits 1 mm off it.
        m.panels[0].holes.push(vec![
            Point2::new(40.0, 0.5),
            Point2::new(42.0, 0.5),
            Point2::new(42.0, 2.5),
            Point2::new(40.0, 2.5),
        ]);
        add_edge_flange(&mut m, &table, flange(0, 0, 25.0, 1.0)).unwrap();
        let v = check_manufacturability(&m, &ShopProfile::generic());
        let h = v
            .iter()
            .find(|x| matches!(x, Violation::HoleTooCloseToBend { .. }))
            .expect("hole-to-bend violation");
        assert_eq!(h.severity(), Severity::Warning);
    }

    #[test]
    fn flags_bends_too_close() {
        // 8 mm deep base: edge 0 (y=0) and edge 2 (y=8) flanges are only
        // 8 mm apart, under the generic 6 mm? No — make it 4 mm deep so
        // the two parallel bends are 4 mm apart.
        let mut m = base_flange_rect(100.0, 4.0, 1.0).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0, 20.0, 1.0)).unwrap();
        add_edge_flange(&mut m, &table, flange(0, 2, 20.0, 1.0)).unwrap();
        let v = check_manufacturability(&m, &ShopProfile::generic());
        let c = v
            .iter()
            .find(|x| matches!(x, Violation::BendsTooClose { .. }))
            .expect("bend-to-bend violation");
        if let Violation::BendsTooClose { actual_mm, .. } = c {
            assert!((actual_mm - 4.0).abs() < 1e-6, "got {actual_mm}");
        }
    }

    #[test]
    fn shop_profile_deserialize_is_field_tolerant() {
        // Only one field present — the rest fall back to generic().
        let p: ShopProfile = serde_json::from_str(r#"{"min_bend_radius_ratio": 2.5}"#).unwrap();
        assert_eq!(p.min_bend_radius_ratio, 2.5);
        assert_eq!(
            p.max_bend_length_mm,
            ShopProfile::generic().max_bend_length_mm
        );
        assert_eq!(p.name, "Generic shop");
        // Round-trips.
        let json = serde_json::to_string(&p).unwrap();
        let q: ShopProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(p, q);
    }

    #[test]
    fn material_aware_radius_flags_al_hard_at_r_over_t_1() {
        // Al-hard requires R/t >= 1.5; generic shop allows 1.0. With
        // material set to al-hard the strictest constraint flips to the
        // material and a 1 mm radius on 1 mm stock fails.
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        m.material = "al-hard".to_string();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0, 25.0, 1.0)).unwrap();
        let v = check_manufacturability(&m, &ShopProfile::generic());
        let r = v
            .iter()
            .find(|x| matches!(x, Violation::BendRadiusBelowMinimum { .. }))
            .expect("expected a material-driven radius violation");
        if let Violation::BendRadiusBelowMinimum {
            source, material, ..
        } = r
        {
            assert_eq!(*source, BendRadiusSource::Material);
            assert_eq!(material, "al-hard");
        }
    }

    #[test]
    fn soft_aluminum_with_shop_floor_only() {
        // Al-soft has min R/t = 0; only the shop's 1.0 ratio applies.
        // R = 0.5 mm on 1 mm stock fails — but tagged as a *shop* limit,
        // not a material one.
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        m.material = "al-soft".to_string();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0, 25.0, 0.5)).unwrap();
        let v = check_manufacturability(&m, &ShopProfile::generic());
        let r = v
            .iter()
            .find(|x| matches!(x, Violation::BendRadiusBelowMinimum { .. }))
            .expect("R below shop floor");
        if let Violation::BendRadiusBelowMinimum { source, .. } = r {
            assert_eq!(*source, BendRadiusSource::Shop);
        }
    }

    #[test]
    fn flags_unrelieved_bend_ends_and_clears_after_fix() {
        use crate::base_flange::base_flange_polygon;
        use crate::relief::{add_all_bend_reliefs, ReliefParams};
        // Hexagonal body, neck flange off the front edge: both hinge ends
        // meet chamfered corners.
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
        let mut p = flange(0, 0, 35.0, 0.5);
        p.angle = 1.1;
        add_edge_flange(&mut m, &table, p).unwrap();

        let v = check_manufacturability(&m, &ShopProfile::generic());
        let reliefs: Vec<_> = v
            .iter()
            .filter(|x| matches!(x, Violation::BendEndNeedsRelief { .. }))
            .collect();
        assert_eq!(reliefs.len(), 2, "both hinge ends flagged: {v:?}");
        assert_eq!(reliefs[0].severity(), Severity::Warning);
        assert_eq!(reliefs[0].rule(), "sheet.bend_relief");
        // Auto depth = R + t.
        if let Violation::BendEndNeedsRelief {
            required_depth_mm, ..
        } = reliefs[0]
        {
            assert!((required_depth_mm - 1.0).abs() < 1e-9);
        }

        // The fix clears exactly what the rule flags.
        add_all_bend_reliefs(&mut m, ReliefParams::default()).unwrap();
        let v = check_manufacturability(&m, &ShopProfile::generic());
        assert!(
            !v.iter()
                .any(|x| matches!(x, Violation::BendEndNeedsRelief { .. })),
            "relief violations persist after fix: {v:?}"
        );
    }

    #[test]
    fn shop_relief_depth_overrides_auto() {
        use crate::base_flange::base_flange_polygon;
        let outline = vec![
            Point2::new(20.0, 0.0),
            Point2::new(40.0, 0.0),
            Point2::new(60.0, 40.0),
            Point2::new(40.0, 80.0),
            Point2::new(20.0, 80.0),
            Point2::new(0.0, 40.0),
        ];
        let mut m = base_flange_polygon(outline, 0.5).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0, 35.0, 0.5)).unwrap();
        let mut shop = ShopProfile::generic();
        shop.relief_depth_mm = 2.5;
        let v = check_manufacturability(&m, &shop);
        let r = v
            .iter()
            .find(|x| matches!(x, Violation::BendEndNeedsRelief { .. }))
            .expect("relief violation");
        if let Violation::BendEndNeedsRelief {
            required_depth_mm, ..
        } = r
        {
            assert!((required_depth_mm - 2.5).abs() < 1e-9);
        }
    }

    #[test]
    fn flags_bend_over_brake_length() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0, 25.0, 1.0)).unwrap();
        let mut tiny_brake = ShopProfile::generic();
        tiny_brake.max_bend_length_mm = 50.0; // hinge is 100 mm
        let v = check_manufacturability(&m, &tiny_brake);
        assert!(v
            .iter()
            .any(|x| matches!(x, Violation::BendExceedsBrakeCapacity { .. })));
    }
}
