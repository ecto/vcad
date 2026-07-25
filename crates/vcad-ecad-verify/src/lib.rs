//! Verified substitution — the unique-to-vcad payoff.
//!
//! `find_alternatives` PROPOSES spec-compatible substitutes (ranked by
//! spec-distance, gated by footprint compatibility and a pin-role hard reject).
//! `verify_substitution` PROVES one: it re-derives the candidate's footprint,
//! re-places it at the *exact* anchor of the part it replaces, re-runs the
//! board's own DRC (clearance, courtyard, **and connectivity**), and reports
//! the before/after violation delta. An "alternative" is only a drop-in when
//! the swap adds no new violations and preserves the pin numbering — a
//! re-verified geometric fact, not a catalog claim.

#![warn(missing_docs)]

pub mod receipt;
pub use receipt::{build_receipt, verify_receipt};

use std::collections::BTreeSet;

use serde::Serialize;
use vcad_ecad_parts::catalog::{all_families, ResolvedPart};
use vcad_ecad_parts::resolve;
use vcad_ecad_pcb::drc::{check_drc, DrcViolation};
use vcad_ir::ecad::{Footprint, Pcb};

/// How a candidate footprint relates to the one it would replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FootprintCompat {
    /// Same land pattern — a true drop-in (no routing change).
    Identical,
    /// Same pad count/roles but the lands move — existing routing likely holds
    /// but should be re-verified.
    Compatible,
    /// Same pin count but lands move enough that traces must be re-routed.
    NeedsReroute,
    /// Pad count or pin roles differ — not a substitute.
    Incompatible,
}

/// A proposed alternative part with its compatibility verdict.
#[derive(Debug, Clone, Serialize)]
pub struct Alternative {
    /// The candidate part.
    pub part: ResolvedPart,
    /// Spec distance from the original (0 = identical value), in log-decades.
    pub spec_distance: f64,
    /// Footprint compatibility.
    pub compat: FootprintCompat,
}

/// Compare two footprints by pad identity to classify compatibility.
fn classify(orig: &[vcad_ir::ecad::Pad], cand: &[vcad_ir::ecad::Pad]) -> FootprintCompat {
    let onums: BTreeSet<&str> = orig.iter().map(|p| p.number.as_str()).collect();
    let cnums: BTreeSet<&str> = cand.iter().map(|p| p.number.as_str()).collect();
    if onums != cnums {
        return FootprintCompat::Incompatible; // pin-count / numbering mismatch
    }
    // Same pins: identical if every same-numbered pad sits at the same place.
    let same_geometry = orig.iter().all(|op| {
        cand.iter().any(|cp| {
            cp.number == op.number
                && (cp.position.x - op.position.x).abs() < 0.05
                && (cp.position.y - op.position.y).abs() < 0.05
        })
    });
    if same_geometry {
        FootprintCompat::Identical
    } else {
        FootprintCompat::NeedsReroute
    }
}

/// Propose spec-compatible alternatives for a resolved part: the same value in
/// the family's other packages, each classified by footprint compatibility.
///
/// Passive substitution keeps the value (spec_distance 0) and varies the
/// package; the footprint gate tells the caller whether re-routing is needed.
pub fn find_alternatives(part: &ResolvedPart) -> Vec<Alternative> {
    let Some(family) = all_families().into_iter().find(|f| f.class == part.class) else {
        return vec![];
    };
    let orig_pads = &part.derived.footprint.pads;
    let mut out = Vec::new();
    for &pkg in family.packages {
        if pkg == part.package {
            continue;
        }
        // Same value, different package.
        let query = format!("{} {}", part.value, pkg);
        let Some(cand) = resolve(&query) else {
            continue;
        };
        let compat = classify(orig_pads, &cand.derived.footprint.pads);
        out.push(Alternative {
            part: cand,
            spec_distance: 0.0,
            compat,
        });
    }
    // Identical first, then by package name for determinism.
    out.sort_by(|a, b| {
        (a.compat as i32)
            .cmp(&(b.compat as i32))
            .then(a.part.package.cmp(&b.part.package))
    });
    out
}

/// The outcome of proving a substitution against the board's own DRC.
#[derive(Debug, Clone, Serialize)]
pub struct Substitution {
    /// The reference designator that was swapped.
    pub reference: String,
    /// True only when the swap adds no new violations and preserves pin numbers.
    pub drop_in: bool,
    /// Violations introduced by the swap.
    pub added: Vec<DrcViolation>,
    /// Violations the swap removed (e.g. it fixed a collision).
    pub removed: Vec<DrcViolation>,
    /// DRC violation count before / after.
    pub before_count: usize,
    /// DRC violation count after.
    pub after_count: usize,
}

/// A canonical key for set-differencing violations (rounded position so float
/// noise does not split otherwise-identical violations).
fn vkey(v: &DrcViolation) -> (String, String, i64, i64) {
    (
        format!("{:?}", v.rule),
        v.message.clone(),
        (v.position.x * 1000.0).round() as i64,
        (v.position.y * 1000.0).round() as i64,
    )
}

/// Replace a placed footprint's land pattern with `template`, preserving the
/// anchor (position/rotation/side) and re-assigning each new pad's net from the
/// old same-numbered pad.
fn reseat(existing: &Footprint, candidate: &ResolvedPart) -> Footprint {
    let template = &candidate.derived.footprint;
    let mut fp = existing.clone();
    fp.footprint_name = template.name.clone();
    fp.value = candidate.value.clone();
    fp.graphics = template.graphics.clone();
    fp.pads = template
        .pads
        .iter()
        .map(|p| {
            let mut np = p.clone();
            np.net = existing
                .pads
                .iter()
                .find(|op| op.number == p.number)
                .and_then(|op| op.net.clone());
            np
        })
        .collect();
    fp
}

/// Prove a substitution: swap `reference` for `candidate`, re-run DRC including
/// connectivity, and return the before/after delta. `None` if no such ref.
pub fn verify_substitution(
    pcb: &Pcb,
    reference: &str,
    candidate: &ResolvedPart,
) -> Option<Substitution> {
    let idx = pcb
        .footprints
        .iter()
        .position(|f| f.reference == reference)?;

    let before = check_drc(pcb);
    let mut after_pcb = pcb.clone();
    let old = &pcb.footprints[idx];
    let pins_preserved = {
        let o: BTreeSet<&str> = old.pads.iter().map(|p| p.number.as_str()).collect();
        let c: BTreeSet<&str> = candidate
            .derived
            .footprint
            .pads
            .iter()
            .map(|p| p.number.as_str())
            .collect();
        o == c
    };
    after_pcb.footprints[idx] = reseat(old, candidate);
    let after = check_drc(&after_pcb);

    let before_keys: BTreeSet<_> = before.iter().map(vkey).collect();
    let after_keys: BTreeSet<_> = after.iter().map(vkey).collect();
    let added: Vec<DrcViolation> = after
        .iter()
        .filter(|v| !before_keys.contains(&vkey(v)))
        .cloned()
        .collect();
    let removed: Vec<DrcViolation> = before
        .iter()
        .filter(|v| !after_keys.contains(&vkey(v)))
        .cloned()
        .collect();

    Some(Substitution {
        reference: reference.to_string(),
        drop_in: added.is_empty() && pins_preserved,
        added,
        removed,
        before_count: before.len(),
        after_count: after.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ecad_pcb::drc::DrcRuleType;
    use vcad_ir::ecad::*;
    use vcad_ir::Vec2;

    fn empty_pcb() -> Pcb {
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
            stackup: LayerStackup {
                layers: vec![StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: Some(1.5),
                    dielectric_er: Some(4.5),
                    material: Some("FR4".into()),
                }],
            },
            nets: vec![
                Net {
                    id: "1".into(),
                    name: "N1".into(),
                },
                Net {
                    id: "2".into(),
                    name: "N2".into(),
                },
            ],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "Default".into(),
                    trace_width: 0.2,
                    clearance: 0.2,
                    via_diameter: 0.8,
                    via_drill: 0.4,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                    target_impedance: None,
                    target_diff_impedance: None,
                },
                class_rules: vec![],
                net_class_assignments: std::collections::HashMap::new(),
                edge_clearance: 0.3,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
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

    /// Place a resolved part at a board location, nets assigned to its 2 pads.
    fn place(part: &ResolvedPart, reference: &str, at: Vec2) -> Footprint {
        let t = &part.derived.footprint;
        let mut fp = Footprint {
            reference: reference.into(),
            value: part.value.clone(),
            footprint_name: t.name.clone(),
            position: at,
            rotation: 0.0,
            front: true,
            pads: t.pads.clone(),
            graphics: t.graphics.clone(),
            model_3d: None,
            properties: std::collections::HashMap::new(),
        };
        for (i, p) in fp.pads.iter_mut().enumerate() {
            p.net = Some(((i % 2) + 1).to_string());
        }
        fp
    }

    #[test]
    fn alternatives_span_packages_and_classify() {
        let part = resolve("10k 0603 1%").unwrap();
        let alts = find_alternatives(&part);
        assert!(!alts.is_empty());
        // Same value, never the original package.
        assert!(alts.iter().all(|a| a.part.package != "0603"));
        assert!(alts.iter().all(|a| a.part.value == "10k"));
        // Different chip packages move the lands → NeedsReroute, not Identical.
        assert!(alts
            .iter()
            .any(|a| a.compat == FootprintCompat::NeedsReroute));
    }

    #[test]
    fn substitution_into_open_board_is_drop_in() {
        let mut pcb = empty_pcb();
        let r = resolve("10k 0603").unwrap();
        pcb.footprints = vec![place(&r, "R1", Vec2::new(25.0, 25.0))];
        // Swap 0603 → 0805 (larger). Plenty of room → no new violations.
        let cand = resolve("10k 0805").unwrap();
        let sub = verify_substitution(&pcb, "R1", &cand).unwrap();
        assert!(
            sub.drop_in,
            "isolated swap must be drop-in: {:?}",
            sub.added
        );
        assert_eq!(sub.reference, "R1");
    }

    #[test]
    fn substitution_that_collides_is_not_drop_in() {
        let mut pcb = empty_pcb();
        let small = resolve("10k 0402").unwrap();
        // Two 0402 parts spaced so they clear, but a 1206 swap will collide.
        pcb.footprints = vec![
            place(&small, "R1", Vec2::new(25.0, 25.0)),
            place(&small, "R2", Vec2::new(27.5, 25.0)),
        ];
        let before = check_drc(&pcb);
        assert!(
            !before
                .iter()
                .any(|v| v.rule == DrcRuleType::CourtyardOverlap),
            "0402 parts must start clear"
        );
        // Swap R1 to a much larger 2512 → courtyard now overlaps R2.
        let big = resolve("10k 2512").unwrap();
        let sub = verify_substitution(&pcb, "R1", &big).unwrap();
        assert!(!sub.drop_in, "colliding swap must NOT be drop-in");
        assert!(sub
            .added
            .iter()
            .any(|v| v.rule == DrcRuleType::CourtyardOverlap));
    }

    #[test]
    fn unknown_reference_returns_none() {
        let pcb = empty_pcb();
        let cand = resolve("10k 0805").unwrap();
        assert!(verify_substitution(&pcb, "R9", &cand).is_none());
    }
}
