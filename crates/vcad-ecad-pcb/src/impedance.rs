//! Controlled-impedance geometry, resolved **per layer** from the stackup.
//!
//! A net class declares one `trace_width` / `diff_pair_width` / `diff_pair_gap`
//! and the router has historically applied it on every copper layer. That is
//! not physical: the width that hits a target impedance depends on the layer.
//! An outer layer is a microstrip referenced to the plane one dielectric away;
//! an inner layer is a stripline sandwiched between the planes above and below.
//! On a 10-layer board the two answers differ by a factor of two or more.
//!
//! This module turns a class's *target impedance* plus the board stackup into a
//! per-layer `(width, gap)`. The electromagnetics is not re-derived here — it is
//! the same IPC-2141 closed form that ships behind the `calc_impedance` /
//! `size_impedance` MCP tools ([`vcad_ecad_sim::impedance`]); this module only
//! resolves the layer's `(t, h, er)` from the stackup and inverts the monotone
//! width→impedance relation by bisection.
//!
//! # Fail closed
//!
//! Every path that cannot *prove* a width returns the class's declared geometry
//! together with the reason it could not ([`GeometryBasis::Declared`]). A
//! missing dielectric thickness, a missing `er`, a target the layer cannot
//! reach inside the manufacturable width window — all of them keep the declared
//! number and report the impedance unverified. Nothing here ever substitutes a
//! plausible-looking guess for stackup data the board does not carry.
//!
//! # Modelling assumptions (stated, not hidden)
//!
//! * The copper layer immediately above/below the signal layer is taken as its
//!   reference plane. Boards that reference a *further* plane (a signal layer
//!   adjacent to another signal layer) will read low — see
//!   [`LayerEm::adjacent_reference`], which records this so callers can report
//!   it rather than discover it.
//! * The gap is held at the class's declared `diff_pair_gap` and the leg width
//!   is solved for it. Gap is usually a fab/pitch constraint, width is the free
//!   variable; solving both is underdetermined without a second objective.

use std::collections::BTreeMap;

use vcad_ecad_sim::impedance::{
    diff_microstrip_impedance, diff_stripline_impedance, microstrip_z0, stripline_z0,
};
use vcad_ir::ecad::{LayerStackup, NetClassRules, Pcb, PcbLayer};

/// Widest width the solver will consider, in mm. Beyond this a "trace" is a
/// pour, not a controlled-impedance conductor.
pub const MAX_SOLVE_WIDTH_MM: f64 = 2.0;

/// Whether a layer behaves as microstrip (outer) or stripline (inner).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    /// Outer layer: dielectric on one side, air on the other.
    Microstrip,
    /// Inner layer: dielectric on both sides, between two reference layers.
    Stripline,
}

/// The electromagnetic parameters a copper layer presents to a trace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerEm {
    /// Microstrip (outer) or stripline (inner).
    pub kind: LayerKind,
    /// Copper thickness of the signal layer, mm.
    pub copper_thickness: f64,
    /// Dielectric height, mm. For microstrip: signal → reference plane. For
    /// stripline: total separation between the reference layers (dielectric
    /// above + signal copper + dielectric below).
    pub dielectric_height: f64,
    /// Relative permittivity (the mean of the dielectrics involved).
    pub er: f64,
    /// True when the reference layer used is the immediately adjacent copper
    /// layer. Always true today — recorded so the assumption travels with the
    /// number instead of living only in this doc comment.
    pub adjacent_reference: bool,
}

/// Why a layer kept the declared geometry instead of a solved one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnverifiedReason {
    /// The class declares no target impedance — nothing to solve for.
    NoTarget,
    /// The layer is not in the stackup.
    LayerNotInStackup,
    /// The stackup does not carry the dielectric thickness and/or `er` this
    /// layer needs.
    MissingStackupData,
    /// The class declares no differential gap, so a pair width cannot be
    /// solved.
    NoDiffPairGap,
    /// The target is unreachable on this layer inside
    /// `[min_width, MAX_SOLVE_WIDTH_MM]` — typically a thin outer dielectric
    /// that cannot carry a high-impedance pair at any manufacturable width.
    TargetUnreachable,
}

/// Where a resolved width came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GeometryBasis {
    /// Solved from the stackup against the class's target impedance. The value
    /// is the impedance the solved geometry achieves (ohms).
    Derived {
        /// Impedance achieved by the solved geometry, ohms.
        achieved: f64,
    },
    /// The class's declared geometry, kept because the impedance could not be
    /// solved. The impedance of this geometry is **not** verified.
    Declared(UnverifiedReason),
}

impl GeometryBasis {
    /// True when the geometry's impedance is backed by a solve.
    pub fn is_verified(&self) -> bool {
        matches!(self, GeometryBasis::Derived { .. })
    }
}

/// A class's routing geometry on one specific layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerGeometry {
    /// Trace (or differential leg) width in mm.
    pub width: f64,
    /// Edge-to-edge gap between the pair legs in mm; `None` for single-ended.
    pub gap: Option<f64>,
    /// Whether `width` was solved or merely declared.
    pub basis: GeometryBasis,
}

/// Copper layers of `stackup`, ordered top to bottom.
fn copper_order(stackup: &LayerStackup) -> Vec<PcbLayer> {
    let mut v: Vec<PcbLayer> = stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper())
        .collect();
    v.sort_by_key(|l| l.copper_position().unwrap_or(u8::MAX));
    v.dedup();
    v
}

/// Resolve the electromagnetic parameters `layer` presents, from `stackup`.
///
/// Returns `None` when the stackup lacks the data — never a default.
///
/// `dielectric_thickness` on a stackup layer is the dielectric *below* it
/// (toward the next copper layer down); the bottom layer carries none.
pub fn layer_em(stackup: &LayerStackup, layer: PcbLayer) -> Option<LayerEm> {
    let order = copper_order(stackup);
    let idx = order.iter().position(|&l| l == layer)?;
    let n = order.len();
    if n < 2 {
        return None;
    }
    let entry = |l: PcbLayer| stackup.layers.iter().find(|s| s.layer == l);
    let me = entry(layer)?;
    let t = me.copper_thickness?;

    // Dielectric below `layer` lives on `layer`; dielectric above lives on the
    // copper layer above it.
    let below = me.dielectric_thickness;
    let below_er = me.dielectric_er;
    let (above, above_er) = if idx == 0 {
        (None, None)
    } else {
        let up = entry(order[idx - 1])?;
        (up.dielectric_thickness, up.dielectric_er)
    };

    let (kind, h, er) = if idx == 0 {
        // Top layer: microstrip over the dielectric below it.
        (LayerKind::Microstrip, below?, below_er?)
    } else if idx == n - 1 {
        // Bottom layer: microstrip over the dielectric above it.
        (LayerKind::Microstrip, above?, above_er?)
    } else {
        let (a, b) = (above?, below?);
        let (ea, eb) = (above_er?, below_er?);
        (LayerKind::Stripline, a + t + b, (ea + eb) / 2.0)
    };
    // NaN or non-positive stackup numbers are missing data, not a geometry.
    if !(h.is_finite() && h > 0.0 && er.is_finite() && er > 0.0 && t.is_finite() && t > 0.0) {
        return None;
    }
    Some(LayerEm {
        kind,
        copper_thickness: t,
        dielectric_height: h,
        er,
        adjacent_reference: true,
    })
}

/// Bisect a strictly-decreasing width→impedance function for `target`.
///
/// Returns `None` when `target` lies outside the range the window can reach —
/// the fail-closed path, not a clamped guess.
fn solve_width(
    em: &LayerEm,
    target: f64,
    min_width: f64,
    z_of: impl Fn(f64) -> f64,
) -> Option<f64> {
    let (lo, hi) = (min_width.max(1e-3), MAX_SOLVE_WIDTH_MM);
    if lo >= hi {
        return None;
    }
    let (z_lo, z_hi) = (z_of(lo), z_of(hi));
    // Impedance falls as the trace widens; the target must be bracketed.
    if !(z_hi..=z_lo).contains(&target) {
        return None;
    }
    let _ = em;
    let (mut a, mut b) = (lo, hi);
    for _ in 0..80 {
        let m = 0.5 * (a + b);
        if z_of(m) > target {
            a = m;
        } else {
            b = m;
        }
    }
    Some(0.5 * (a + b))
}

/// Single-ended width on `layer` that hits `target` ohms, if solvable.
pub fn solve_single_ended_width(em: &LayerEm, target: f64, min_width: f64) -> Option<f64> {
    let (t, h, er) = (em.copper_thickness, em.dielectric_height, em.er);
    match em.kind {
        LayerKind::Microstrip => solve_width(em, target, min_width, |w| microstrip_z0(w, t, h, er)),
        LayerKind::Stripline => solve_width(em, target, min_width, |w| stripline_z0(w, t, h, er)),
    }
}

/// Differential leg width on `layer` that hits `target` ohms at `gap`, if
/// solvable.
pub fn solve_diff_pair_width(em: &LayerEm, target: f64, gap: f64, min_width: f64) -> Option<f64> {
    if !(gap.is_finite() && gap > 0.0) {
        return None;
    }
    let (t, h, er) = (em.copper_thickness, em.dielectric_height, em.er);
    match em.kind {
        LayerKind::Microstrip => solve_width(em, target, min_width, |w| {
            diff_microstrip_impedance(w, gap, t, h, er)
        }),
        LayerKind::Stripline => solve_width(em, target, min_width, |w| {
            diff_stripline_impedance(w, gap, t, h, er)
        }),
    }
}

/// Impedance a differential pair of `(width, gap)` achieves on `em`.
pub fn diff_impedance(em: &LayerEm, width: f64, gap: f64) -> f64 {
    let (t, h, er) = (em.copper_thickness, em.dielectric_height, em.er);
    match em.kind {
        LayerKind::Microstrip => diff_microstrip_impedance(width, gap, t, h, er),
        LayerKind::Stripline => diff_stripline_impedance(width, gap, t, h, er),
    }
}

/// The class's declared differential geometry (leg width, gap).
fn declared_diff(class: &NetClassRules) -> (f64, Option<f64>) {
    (
        class.diff_pair_width.unwrap_or(class.trace_width),
        class.diff_pair_gap,
    )
}

/// Resolve `class`'s differential geometry on `layer`.
///
/// Falls back to the declared geometry — reporting why — whenever the solve
/// cannot be completed.
pub fn diff_pair_geometry_for_layer(
    stackup: &LayerStackup,
    layer: PcbLayer,
    class: &NetClassRules,
    min_width: f64,
) -> LayerGeometry {
    let (dw, dgap) = declared_diff(class);
    let declared = |why| LayerGeometry {
        width: dw,
        gap: dgap,
        basis: GeometryBasis::Declared(why),
    };
    let Some(target) = class.target_diff_impedance else {
        return declared(UnverifiedReason::NoTarget);
    };
    let Some(gap) = dgap else {
        return declared(UnverifiedReason::NoDiffPairGap);
    };
    let Some(em) = layer_em(stackup, layer) else {
        // Distinguish "layer isn't in the stackup" from "stackup is silent".
        let known = copper_order(stackup).contains(&layer);
        return declared(if known {
            UnverifiedReason::MissingStackupData
        } else {
            UnverifiedReason::LayerNotInStackup
        });
    };
    let Some(w) = solve_diff_pair_width(&em, target, gap, min_width) else {
        return declared(UnverifiedReason::TargetUnreachable);
    };
    LayerGeometry {
        width: w,
        gap: Some(gap),
        basis: GeometryBasis::Derived {
            achieved: diff_impedance(&em, w, gap),
        },
    }
}

/// Resolve `class`'s single-ended width on `layer`.
pub fn trace_geometry_for_layer(
    stackup: &LayerStackup,
    layer: PcbLayer,
    class: &NetClassRules,
    min_width: f64,
) -> LayerGeometry {
    let declared = |why| LayerGeometry {
        width: class.trace_width,
        gap: None,
        basis: GeometryBasis::Declared(why),
    };
    let Some(target) = class.target_impedance else {
        return declared(UnverifiedReason::NoTarget);
    };
    let Some(em) = layer_em(stackup, layer) else {
        let known = copper_order(stackup).contains(&layer);
        return declared(if known {
            UnverifiedReason::MissingStackupData
        } else {
            UnverifiedReason::LayerNotInStackup
        });
    };
    let Some(w) = solve_single_ended_width(&em, target, min_width) else {
        return declared(UnverifiedReason::TargetUnreachable);
    };
    let z = match em.kind {
        LayerKind::Microstrip => microstrip_z0(w, em.copper_thickness, em.dielectric_height, em.er),
        LayerKind::Stripline => stripline_z0(w, em.copper_thickness, em.dielectric_height, em.er),
    };
    LayerGeometry {
        width: w,
        gap: None,
        basis: GeometryBasis::Derived { achieved: z },
    }
}

/// Board minimum trace width — the narrow end of the solver's window.
pub fn board_min_width(pcb: &Pcb) -> f64 {
    pcb.rules.default_rules.trace_width.max(1e-3)
}

/// `class`'s differential geometry on every copper layer of the board.
pub fn diff_pair_geometry_by_layer(
    pcb: &Pcb,
    class: &NetClassRules,
) -> BTreeMap<u8, (PcbLayer, LayerGeometry)> {
    let min_w = board_min_width(pcb);
    copper_order(&pcb.stackup)
        .into_iter()
        .map(|l| {
            (
                l.copper_position().unwrap_or(u8::MAX),
                (
                    l,
                    diff_pair_geometry_for_layer(&pcb.stackup, l, class, min_w),
                ),
            )
        })
        .collect()
}

/// The copper layers on which `class`'s **declared** differential geometry is
/// impedance-correct to within `tol_pct` of its target.
///
/// This is what the router prefers: rather than changing a trace's width
/// mid-route at a via — which the corridor search cannot express today — the
/// pair search first tries to find its path on layers where the width it is
/// already committed to is the physically right one, and only widens the layer
/// set when that fails. Returns `None` when there is nothing to prefer (no
/// target, no stackup data, or every layer qualifies/disqualifies alike), in
/// which case the caller must not restrict anything.
pub fn impedance_correct_layers(
    pcb: &Pcb,
    class: &NetClassRules,
    tol_pct: f64,
) -> Option<Vec<PcbLayer>> {
    let target = class.target_diff_impedance?;
    let gap = class.diff_pair_gap?;
    let (declared_w, _) = declared_diff(class);
    let mut ok = Vec::new();
    let mut any_solvable = false;
    for layer in copper_order(&pcb.stackup) {
        let Some(em) = layer_em(&pcb.stackup, layer) else {
            continue;
        };
        any_solvable = true;
        let z = diff_impedance(&em, declared_w, gap);
        if ((z - target) / target).abs() * 100.0 <= tol_pct {
            ok.push(layer);
        }
    }
    if !any_solvable || ok.is_empty() {
        return None;
    }
    Some(ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::StackupLayer;

    /// Four-layer stackup: 0.1 mm prepreg to the plane under F.Cu, a 0.5 mm
    /// core in the middle, symmetric.
    fn populated_stackup() -> LayerStackup {
        let mk = |layer, diel: Option<f64>| StackupLayer {
            layer,
            copper_thickness: Some(0.035),
            dielectric_thickness: diel,
            dielectric_er: diel.map(|_| 4.3),
            material: diel.map(|_| "FR4".to_string()),
        };
        LayerStackup {
            layers: vec![
                mk(PcbLayer::FCu, Some(0.1)),
                mk(PcbLayer::In1Cu, Some(0.5)),
                mk(PcbLayer::In2Cu, Some(0.1)),
                mk(PcbLayer::BCu, None),
            ],
        }
    }

    /// The same stackup with the dielectric data stripped — the fail-closed
    /// case.
    fn bare_stackup() -> LayerStackup {
        LayerStackup {
            layers: populated_stackup()
                .layers
                .into_iter()
                .map(|mut l| {
                    l.dielectric_thickness = None;
                    l.dielectric_er = None;
                    l
                })
                .collect(),
        }
    }

    /// Bare board carrying `stackup` — only the stackup and rules matter here.
    fn test_pcb(stackup: LayerStackup) -> Pcb {
        use vcad_ir::ecad::{BoardOutline, DesignRules};
        use vcad_ir::Vec2;
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(50.0, 0.0),
                    Vec2::new(50.0, 50.0),
                    Vec2::new(0.0, 50.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup,
            nets: vec![],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "default".into(),
                    trace_width: 0.1,
                    clearance: 0.1,
                    via_diameter: 0.4,
                    via_drill: 0.2,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                    target_impedance: None,
                    target_diff_impedance: None,
                },
                class_rules: vec![],
                net_class_assignments: Default::default(),
                edge_clearance: 0.3,
                hole_to_hole: 0.25,
                min_annular_ring: 0.05,
                min_drill: 0.2,
            },
            footprints: vec![],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn class_90r() -> NetClassRules {
        NetClassRules {
            name: "DIFF".to_string(),
            trace_width: 0.15,
            clearance: 0.1,
            via_diameter: 0.4,
            via_drill: 0.2,
            diff_pair_gap: Some(0.15),
            diff_pair_width: Some(0.15),
            target_impedance: Some(50.0),
            target_diff_impedance: Some(90.0),
        }
    }

    #[test]
    fn outer_is_microstrip_inner_is_stripline() {
        let s = populated_stackup();
        assert_eq!(
            layer_em(&s, PcbLayer::FCu).unwrap().kind,
            LayerKind::Microstrip
        );
        assert_eq!(
            layer_em(&s, PcbLayer::BCu).unwrap().kind,
            LayerKind::Microstrip
        );
        assert_eq!(
            layer_em(&s, PcbLayer::In1Cu).unwrap().kind,
            LayerKind::Stripline
        );
        // Stripline height spans plane-to-plane: 0.1 above + 0.035 copper + 0.5
        // below.
        let inner = layer_em(&s, PcbLayer::In1Cu).unwrap();
        assert!((inner.dielectric_height - 0.635).abs() < 1e-9);
        // Microstrip height is the single dielectric to the adjacent plane.
        assert!((layer_em(&s, PcbLayer::FCu).unwrap().dielectric_height - 0.1).abs() < 1e-9);
    }

    /// Acceptance 1: one class, two different widths — and the declared width
    /// when the stackup is silent.
    #[test]
    fn same_class_resolves_per_layer_and_fails_closed() {
        let class = class_90r();
        let s = populated_stackup();

        let outer = diff_pair_geometry_for_layer(&s, PcbLayer::FCu, &class, 0.05);
        let inner = diff_pair_geometry_for_layer(&s, PcbLayer::In1Cu, &class, 0.05);
        assert!(outer.basis.is_verified(), "outer should solve: {outer:?}");
        assert!(inner.basis.is_verified(), "inner should solve: {inner:?}");
        assert!(
            (outer.width - inner.width).abs() > 0.02,
            "microstrip {} vs stripline {} should differ materially",
            outer.width,
            inner.width
        );
        // Both hit the target they were solved for.
        for g in [outer, inner] {
            let GeometryBasis::Derived { achieved } = g.basis else {
                unreachable!()
            };
            assert!((achieved - 90.0).abs() < 0.5, "achieved {achieved}");
        }

        // Stackup missing dielectric data → declared width, unverified.
        let bare = bare_stackup();
        let g = diff_pair_geometry_for_layer(&bare, PcbLayer::In1Cu, &class, 0.05);
        assert_eq!(g.width, 0.15);
        assert_eq!(
            g.basis,
            GeometryBasis::Declared(UnverifiedReason::MissingStackupData)
        );
        assert!(!g.basis.is_verified());
    }

    #[test]
    fn no_target_keeps_declared_width() {
        let mut class = class_90r();
        class.target_diff_impedance = None;
        let g = diff_pair_geometry_for_layer(&populated_stackup(), PcbLayer::FCu, &class, 0.05);
        assert_eq!(g.width, 0.15);
        assert_eq!(g.basis, GeometryBasis::Declared(UnverifiedReason::NoTarget));
    }

    #[test]
    fn unreachable_target_fails_closed() {
        let class = NetClassRules {
            // 300 ohm differential is not reachable on a 0.1 mm microstrip at
            // any manufacturable width.
            target_diff_impedance: Some(300.0),
            ..class_90r()
        };
        let g = diff_pair_geometry_for_layer(&populated_stackup(), PcbLayer::FCu, &class, 0.05);
        assert_eq!(g.width, 0.15);
        assert_eq!(
            g.basis,
            GeometryBasis::Declared(UnverifiedReason::TargetUnreachable)
        );
    }

    /// The router's layer preference: with the declared 0.15/0.15 geometry the
    /// inner striplines land at ~98Ω (within 10% of the 90Ω target) while the
    /// outer microstrips land at ~76Ω — so only the inners qualify.
    #[test]
    fn impedance_correct_layers_is_a_proper_subset() {
        let mut pcb = test_pcb(populated_stackup());
        pcb.rules.class_rules = vec![class_90r()];
        let ok = impedance_correct_layers(&pcb, &class_90r(), 10.0).unwrap();
        assert_eq!(ok, vec![PcbLayer::In1Cu, PcbLayer::In2Cu]);
        // Widen the tolerance and every layer qualifies — nothing to prefer.
        let all = impedance_correct_layers(&pcb, &class_90r(), 30.0).unwrap();
        assert_eq!(all.len(), 4);
        // No target, or no stackup data: no preference at all.
        let mut no_target = class_90r();
        no_target.target_diff_impedance = None;
        assert!(impedance_correct_layers(&pcb, &no_target, 10.0).is_none());
        let bare = test_pcb(bare_stackup());
        assert!(impedance_correct_layers(&bare, &class_90r(), 10.0).is_none());
    }

    #[test]
    fn solved_width_round_trips_through_the_forward_model() {
        let em = layer_em(&populated_stackup(), PcbLayer::In1Cu).unwrap();
        let w = solve_diff_pair_width(&em, 100.0, 0.15, 0.05).unwrap();
        assert!((diff_impedance(&em, w, 0.15) - 100.0).abs() < 0.1);
    }
}
