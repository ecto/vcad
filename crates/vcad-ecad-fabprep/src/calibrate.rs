//! Opt-in, fully-logged design-rule calibration.
//!
//! Imported boards routinely carry global minima that contradict their own
//! declared via classes: the CM5 reverse-engineering fixture declares a
//! 0.21/0.12 mm default via class and a global `minDrill` of 0.2 mm, so every
//! via the board itself specifies is illegal under the board's own rules —
//! 832 AnnularRing + MinDrill violations that no amount of routing can fix.
//!
//! Relaxing DRC rules to make a board pass is a serious footgun, so this
//! module is built around three guarantees:
//!
//! 1. **Opt-in.** Nothing here runs unless the caller asks for it.
//! 2. **Derived, never invented.** Every calibrated value comes from geometry
//!    the board already carries and the router did not create — a *declared*
//!    via class, or a pre-existing footprint hole pair. A rule can only be
//!    relaxed to the point where the board's own given geometry stops being
//!    illegal, never further.
//! 3. **Logged with its derivation.** Each change records the declared value,
//!    the calibrated value, and the evidence sentence
//!    ("minDrill 0.12 from via class 'Default' (0.21/0.12 mm), realized by
//!    1,867 vias") so a reader can audit the policy instead of trusting it.
//!
//! Calibration is also floored: a corrupt board cannot talk the rules below
//! the physical limits of the tightest process this codebase is willing to
//! name ([`FLOOR_DRILL`], [`FLOOR_ANNULAR_RING`], [`FLOOR_HOLE_TO_HOLE`] —
//! laser-microvia class). A derivation that would go under the floor is
//! refused and the refusal is reported.
//!
//! # What is deliberately *not* derived
//!
//! `minDrill` and `minAnnularRing` are properties of a via class, so a board
//! that declares a class is already stating them and a contradiction is a
//! genuine self-inconsistency this module can resolve. Hole-to-hole spacing is
//! not: it is a **process capability** of whoever drills the board, and no via
//! class declares it. So `holeToHole` is only ever relaxed to admit holes the
//! board already contains and the router did not create — pad drills in the
//! imported land patterns. It is never inferred from vias, because on a re-run
//! the vias are the router's own output and the rule would end up justifying
//! the copper it exists to judge.
//!
//! If a board's declared `holeToHole` is wrong for its fab, the fix is to state
//! the correct rule on the board — a decision with a named owner — not to have
//! this module guess one. Until then the violations stand, the fix loop tries to
//! route around them, and a run that cannot fails closed.

use std::collections::BTreeMap;

use vcad_ir::ecad::{NetClassRules, Pcb};

/// Smallest drill this crate will ever calibrate `minDrill` down to (mm).
/// Laser-microvia class; below this is not a drilled hole any fab quotes.
pub const FLOOR_DRILL: f64 = 0.05;

/// Smallest annular ring this crate will ever calibrate to (mm).
pub const FLOOR_ANNULAR_RING: f64 = 0.02;

/// Smallest hole-to-hole gap this crate will ever calibrate to (mm).
pub const FLOOR_HOLE_TO_HOLE: f64 = 0.05;

/// One applied design-rule calibration, with the derivation that justifies it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RuleCalibration {
    /// Rule field name as it appears in the `.pcb.json` (`minDrill`,
    /// `minAnnularRing`, `holeToHole`).
    pub rule: String,
    /// The value the board declared before calibration (mm).
    pub declared: f64,
    /// The value calibration relaxed it to (mm).
    pub calibrated: f64,
    /// Human-readable derivation: where the number came from and how much of
    /// the board's given geometry vouches for it.
    pub justification: String,
}

/// A calibration that was derived but **refused** — the derivation demanded a
/// value below the physical floor, so the declared rule stands and the board
/// keeps whatever violations that implies.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RefusedCalibration {
    /// Rule field name.
    pub rule: String,
    /// The value the derivation asked for (mm).
    pub requested: f64,
    /// The floor that refused it (mm).
    pub floor: f64,
    /// Why the request was refused.
    pub reason: String,
}

/// The outcome of a calibration pass: what changed, and what was refused.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CalibrationReport {
    /// Applied calibrations, in rule order.
    pub applied: Vec<RuleCalibration>,
    /// Derivations that hit a floor and were refused.
    pub refused: Vec<RefusedCalibration>,
}

impl CalibrationReport {
    /// True when nothing was applied and nothing was refused.
    pub fn is_empty(&self) -> bool {
        self.applied.is_empty() && self.refused.is_empty()
    }
}

/// Round to 1 µm — fab rules are quoted in whole microns, and an unrounded
/// float derivation ("0.11999999999999998") reads as noise in the receipt.
fn micron(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// The via class this board actually builds with: among the *declared*
/// classes (default + per-net-class overrides), the one whose
/// (diameter, drill) pair the most realized vias match.
///
/// Selecting among declared classes — rather than measuring the vias
/// directly — is what keeps calibration non-circular. The router places vias
/// at the default class, so a measurement-based derivation would let the
/// router justify its own rules; a declaration-based one can only ever surface
/// a contradiction the *board author* wrote down.
#[derive(Debug, Clone, PartialEq)]
pub struct ViaClassUsage {
    /// Class name.
    pub name: String,
    /// Declared via diameter (mm).
    pub diameter: f64,
    /// Declared via drill (mm).
    pub drill: f64,
    /// How many realized vias match this class's (diameter, drill).
    pub realized: usize,
}

/// Every declared via class on the board, annotated with how many realized
/// vias match it, most-used first (ties broken by name for determinism).
pub fn via_class_usage(pcb: &Pcb) -> Vec<ViaClassUsage> {
    let mut classes: Vec<&NetClassRules> = vec![&pcb.rules.default_rules];
    classes.extend(pcb.rules.class_rules.iter());

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for via in &pcb.vias {
        for c in &classes {
            if (via.diameter - c.via_diameter).abs() < 1e-9
                && (via.drill - c.via_drill).abs() < 1e-9
            {
                *counts.entry(c.name.clone()).or_default() += 1;
            }
        }
    }

    let mut out: Vec<ViaClassUsage> = classes
        .iter()
        .map(|c| ViaClassUsage {
            name: c.name.clone(),
            diameter: c.via_diameter,
            drill: c.via_drill,
            realized: counts.get(&c.name).copied().unwrap_or(0),
        })
        .collect();
    out.sort_by(|a, b| {
        b.realized
            .cmp(&a.realized)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// The tightest hole-to-hole gap among the board's **pre-existing** holes —
/// footprint pad drills, which the router never creates. Returns the gap (mm)
/// and a label for the pair that attains it.
///
/// Vias are deliberately excluded: they are (or may be) the router's own
/// output, and a rule calibrated against router output would exempt exactly
/// the copper it is supposed to be judging.
fn tightest_pad_hole_gap(pcb: &Pcb) -> Option<(f64, String)> {
    struct Hole {
        x: f64,
        y: f64,
        r: f64,
        label: String,
    }
    let mut holes: Vec<Hole> = Vec::new();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            let Some(spec) = &pad.drill else { continue };
            let drill = spec.diameter;
            if drill <= 0.0 {
                continue;
            }
            let p = vcad_ecad_pcb::geometry::pad_world_position(fp, pad);
            holes.push(Hole {
                x: p.x,
                y: p.y,
                r: drill / 2.0,
                label: format!("{}.{}", fp.reference, pad.number),
            });
        }
    }
    if holes.len() < 2 {
        return None;
    }

    // Sort by x so the O(n^2) pair scan can break out early: once the x-gap
    // alone exceeds the best edge-to-edge gap found so far, no later pair can
    // beat it. Imported boards carry thousands of THT holes.
    holes.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
    let mut best: Option<(f64, String)> = None;
    for i in 0..holes.len() {
        for j in (i + 1)..holes.len() {
            let dx = holes[j].x - holes[i].x;
            if let Some((b, _)) = &best {
                if dx - holes[i].r - holes[j].r > *b {
                    break;
                }
            }
            let dy = holes[j].y - holes[i].y;
            let gap = (dx * dx + dy * dy).sqrt() - holes[i].r - holes[j].r;
            if best.as_ref().is_none_or(|(b, _)| gap < *b) {
                best = Some((gap, format!("{}↔{}", holes[i].label, holes[j].label)));
            }
        }
    }
    best.map(|(gap, label)| (gap.max(0.0), label))
}

/// Derive and apply rule calibration to `pcb` in place, returning the log.
///
/// Only rules the board's own given geometry *contradicts* are touched, and
/// only ever in the relaxing direction. A board whose rules already admit its
/// declared classes and pre-existing holes comes back with an empty report and
/// an unmodified `pcb`.
pub fn calibrate_rules(pcb: &mut Pcb) -> CalibrationReport {
    let mut report = CalibrationReport::default();
    let usage = via_class_usage(pcb);

    // --- minDrill --------------------------------------------------------
    // The board is illegal against itself if any declared class drills
    // smaller than the global minimum. Relax to the smallest declared drill;
    // cite the class that attains it and how much of the board realizes it.
    if let Some(min_class) = usage.iter().min_by(|a, b| {
        a.drill
            .partial_cmp(&b.drill)
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        let want = micron(min_class.drill);
        if want < pcb.rules.min_drill {
            if want < FLOOR_DRILL {
                report.refused.push(RefusedCalibration {
                    rule: "minDrill".into(),
                    requested: want,
                    floor: FLOOR_DRILL,
                    reason: format!(
                        "via class '{}' declares a {want:.3}mm drill, below the {FLOOR_DRILL:.3}mm \
                         laser-microvia floor — the declared rule stands",
                        min_class.name
                    ),
                });
            } else {
                report.applied.push(RuleCalibration {
                    rule: "minDrill".into(),
                    declared: pcb.rules.min_drill,
                    calibrated: want,
                    justification: format!(
                        "minDrill {want:.3} from via class '{}' ({:.3}/{:.3} mm), realized by {} \
                         vias — the board's global minimum ({:.3}) forbids the via class the board \
                         itself declares",
                        min_class.name,
                        min_class.diameter,
                        min_class.drill,
                        min_class.realized,
                        pcb.rules.min_drill,
                    ),
                });
                pcb.rules.min_drill = want;
            }
        }
    }

    // --- minAnnularRing --------------------------------------------------
    if let Some(min_class) = usage.iter().min_by(|a, b| {
        let (ra, rb) = ((a.diameter - a.drill) / 2.0, (b.diameter - b.drill) / 2.0);
        ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
    }) {
        let want = micron((min_class.diameter - min_class.drill) / 2.0);
        if want < pcb.rules.min_annular_ring {
            if want < FLOOR_ANNULAR_RING {
                report.refused.push(RefusedCalibration {
                    rule: "minAnnularRing".into(),
                    requested: want,
                    floor: FLOOR_ANNULAR_RING,
                    reason: format!(
                        "via class '{}' implies a {want:.3}mm ring, below the \
                         {FLOOR_ANNULAR_RING:.3}mm floor — the declared rule stands",
                        min_class.name
                    ),
                });
            } else {
                report.applied.push(RuleCalibration {
                    rule: "minAnnularRing".into(),
                    declared: pcb.rules.min_annular_ring,
                    calibrated: want,
                    justification: format!(
                        "minAnnularRing {want:.3} = ({:.3} − {:.3}) / 2 from via class '{}', \
                         realized by {} vias — the board's global minimum ({:.3}) forbids its own \
                         via class",
                        min_class.diameter,
                        min_class.drill,
                        min_class.name,
                        min_class.realized,
                        pcb.rules.min_annular_ring,
                    ),
                });
                pcb.rules.min_annular_ring = want;
            }
        }
    }

    // --- holeToHole ------------------------------------------------------
    // No class declares a hole-to-hole minimum, so this one is measured — but
    // only over holes the router did not create (footprint pad drills). It
    // exempts the fixture's own land patterns, never the routing.
    if let Some((gap, pair)) = tightest_pad_hole_gap(pcb) {
        let want = micron(gap);
        if want < pcb.rules.hole_to_hole {
            if want < FLOOR_HOLE_TO_HOLE {
                report.refused.push(RefusedCalibration {
                    rule: "holeToHole".into(),
                    requested: want,
                    floor: FLOOR_HOLE_TO_HOLE,
                    reason: format!(
                        "pre-existing hole pair {pair} sits at {want:.3}mm, below the \
                         {FLOOR_HOLE_TO_HOLE:.3}mm floor — the declared rule stands and that pair \
                         stays a reported violation"
                    ),
                });
            } else {
                report.applied.push(RuleCalibration {
                    rule: "holeToHole".into(),
                    declared: pcb.rules.hole_to_hole,
                    calibrated: want,
                    justification: format!(
                        "holeToHole {want:.3} from the tightest pre-existing footprint hole pair \
                         ({pair}) — pad drills the router did not create; the board's global \
                         minimum ({:.3}) forbids its own land patterns",
                        pcb.rules.hole_to_hole,
                    ),
                });
                pcb.rules.hole_to_hole = want;
            }
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{test_board, with_tht_pad};

    #[test]
    fn contradictory_via_class_is_calibrated_and_logged() {
        let mut pcb = test_board();
        // Board declares a 0.21/0.12 via class but a 0.20mm global minDrill.
        pcb.rules.default_rules.via_diameter = 0.21;
        pcb.rules.default_rules.via_drill = 0.12;
        pcb.rules.min_drill = 0.20;
        pcb.rules.min_annular_ring = 0.15;

        let report = calibrate_rules(&mut pcb);
        assert_eq!(pcb.rules.min_drill, 0.12);
        assert_eq!(pcb.rules.min_annular_ring, 0.045);

        let drill = report
            .applied
            .iter()
            .find(|c| c.rule == "minDrill")
            .expect("minDrill calibrated");
        assert_eq!(drill.declared, 0.20);
        assert_eq!(drill.calibrated, 0.12);
        assert!(
            drill.justification.contains("0.210/0.120 mm"),
            "derivation must name the via class: {}",
            drill.justification
        );
    }

    #[test]
    fn realized_via_count_appears_in_the_derivation() {
        let mut pcb = test_board();
        pcb.rules.default_rules.via_diameter = 0.21;
        pcb.rules.default_rules.via_drill = 0.12;
        pcb.rules.min_drill = 0.20;
        for i in 0..3 {
            pcb.vias.push(vcad_ir::ecad::Via {
                position: vcad_ir::Vec2::new(2.0 + i as f64, 2.0),
                diameter: 0.21,
                drill: 0.12,
                start_layer: vcad_ir::ecad::PcbLayer::FCu,
                end_layer: vcad_ir::ecad::PcbLayer::BCu,
                net: "N1".into(),
                source: None,
            });
        }
        let report = calibrate_rules(&mut pcb);
        let drill = report
            .applied
            .iter()
            .find(|c| c.rule == "minDrill")
            .unwrap();
        assert!(
            drill.justification.contains("realized by 3 vias"),
            "{}",
            drill.justification
        );
    }

    #[test]
    fn consistent_rules_are_left_alone() {
        let mut pcb = test_board();
        pcb.rules.default_rules.via_diameter = 0.6;
        pcb.rules.default_rules.via_drill = 0.3;
        pcb.rules.min_drill = 0.2;
        pcb.rules.min_annular_ring = 0.1;
        pcb.rules.hole_to_hole = 0.2;

        let report = calibrate_rules(&mut pcb);
        assert!(report.is_empty(), "nothing to calibrate: {report:?}");
        assert_eq!(pcb.rules.min_drill, 0.2);
    }

    #[test]
    fn calibration_never_tightens_a_rule() {
        let mut pcb = test_board();
        // Declared class is far looser than the global minimum: no change.
        pcb.rules.default_rules.via_diameter = 1.0;
        pcb.rules.default_rules.via_drill = 0.6;
        pcb.rules.min_drill = 0.1;
        pcb.rules.min_annular_ring = 0.05;

        calibrate_rules(&mut pcb);
        assert_eq!(pcb.rules.min_drill, 0.1);
        assert_eq!(pcb.rules.min_annular_ring, 0.05);
    }

    #[test]
    fn a_sub_floor_derivation_is_refused_not_applied() {
        let mut pcb = test_board();
        pcb.rules.default_rules.via_diameter = 0.03;
        pcb.rules.default_rules.via_drill = 0.01;
        pcb.rules.min_drill = 0.2;

        let report = calibrate_rules(&mut pcb);
        assert_eq!(pcb.rules.min_drill, 0.2, "declared rule must stand");
        assert!(report.applied.iter().all(|c| c.rule != "minDrill"));
        let refused = report
            .refused
            .iter()
            .find(|r| r.rule == "minDrill")
            .expect("refusal recorded");
        assert_eq!(refused.floor, FLOOR_DRILL);
    }

    #[test]
    fn hole_to_hole_calibrates_off_pre_existing_pad_drills_only() {
        let mut pcb = test_board();
        pcb.rules.hole_to_hole = 0.5;
        // Two THT pads 0.4mm apart centre-to-centre with 0.3mm drills → a
        // 0.1mm edge gap the fixture already contains.
        with_tht_pad(&mut pcb, "J1", "1", 5.0, 5.0, 0.3);
        with_tht_pad(&mut pcb, "J1", "2", 5.4, 5.0, 0.3);
        // A via at a tighter spacing must NOT be allowed to justify anything.
        pcb.vias.push(vcad_ir::ecad::Via {
            position: vcad_ir::Vec2::new(9.0, 9.0),
            diameter: 0.2,
            drill: 0.1,
            start_layer: vcad_ir::ecad::PcbLayer::FCu,
            end_layer: vcad_ir::ecad::PcbLayer::BCu,
            net: "N1".into(),
            source: None,
        });
        pcb.vias.push(vcad_ir::ecad::Via {
            position: vcad_ir::Vec2::new(9.11, 9.0),
            diameter: 0.2,
            drill: 0.1,
            start_layer: vcad_ir::ecad::PcbLayer::FCu,
            end_layer: vcad_ir::ecad::PcbLayer::BCu,
            net: "N1".into(),
            source: None,
        });

        let report = calibrate_rules(&mut pcb);
        let h2h = report
            .applied
            .iter()
            .find(|c| c.rule == "holeToHole")
            .expect("holeToHole calibrated");
        assert!(
            (h2h.calibrated - 0.1).abs() < 1e-9,
            "expected the pad-pair gap, got {}",
            h2h.calibrated
        );
        assert!(
            h2h.justification.contains("J1.1↔J1.2"),
            "{}",
            h2h.justification
        );
    }
}
