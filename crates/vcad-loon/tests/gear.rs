//! End-to-end: `gear` / `ring-gear` in loon source produce real geometry.
//!
//! The unit tests in `vcad_loon::gear` pin the profile math. These pin the
//! language surface — that the stdlib closures exist, arity-check, and lower
//! to an extruded sketch rather than to nothing. A gear that silently
//! evaluates to a blank cylinder is the exact failure this primitive exists
//! to prevent, so "it parsed" is not the assertion; "it has teeth" is.

use vcad_ir::{CsgOp, Document};
use vcad_loon::eval_vcad;

fn doc(src: &str) -> Document {
    eval_vcad(src, None).unwrap_or_else(|e| panic!("eval failed for {src:?}: {e}"))
}

/// The sketch profile behind the single extrude in `doc`, as radii.
fn profile_radii(doc: &Document) -> Vec<f64> {
    let sketch_id = doc
        .nodes
        .values()
        .find_map(|n| match &n.op {
            CsgOp::Extrude { sketch, .. } => Some(*sketch),
            _ => None,
        })
        .expect("expected an Extrude node");
    match &doc.nodes[&sketch_id].op {
        CsgOp::Sketch2D { segments, .. } => segments
            .iter()
            .map(|s| match s {
                vcad_ir::SketchSegment2D::Line { start, .. } => start.x.hypot(start.y),
                vcad_ir::SketchSegment2D::Arc { start, .. } => start.x.hypot(start.y),
            })
            .collect(),
        other => panic!("expected Sketch2D, got {other:?}"),
    }
}

fn extents(doc: &Document) -> (f64, f64) {
    let rs = profile_radii(doc);
    (
        rs.iter().cloned().fold(f64::MAX, f64::min),
        rs.iter().cloned().fold(f64::MIN, f64::max),
    )
}

#[test]
fn external_gear_lowers_to_an_extruded_involute_profile() {
    // rana-60c sun: 10T, m0.5, face 6.
    let d = doc("[gear 0.5 10.0 6.0]");
    let (rmin, rmax) = extents(&d);
    assert!((rmax - 3.0).abs() < 1e-9, "tip radius {rmax}, want 3.0");
    assert!((rmin - 1.875).abs() < 1e-9, "root radius {rmin}, want 1.875");

    // Extruded the full face width from z = 0 up, matching `cylinder`.
    let ex = d
        .nodes
        .values()
        .find_map(|n| match &n.op {
            CsgOp::Extrude { direction, sketch, .. } => Some((*direction, *sketch)),
            _ => None,
        })
        .expect("extrude");
    assert!((ex.0.z - 6.0).abs() < 1e-12, "face width {}", ex.0.z);
    match &d.nodes[&ex.1].op {
        CsgOp::Sketch2D { origin, .. } => {
            assert!(origin.z.abs() < 1e-12, "origin z {}", origin.z)
        }
        _ => unreachable!(),
    }
}

#[test]
fn ring_gear_is_the_bore_not_the_ring() {
    // rana-60c ring: 50T, m0.5. Teeth point inward, so the profile's LARGEST
    // radius is the root (13.125) and its smallest the tip (12.0). If this
    // ever inverts, the ring is being modelled as a positive solid again.
    let d = doc("[ring-gear 0.5 50.0 6.0]");
    let (rmin, rmax) = extents(&d);
    assert!((rmax - 13.125).abs() < 1e-9, "root radius {rmax}");
    assert!((rmin - 12.0).abs() < 1e-9, "tip radius {rmin}");
}

#[test]
fn the_60c_train_meshes_with_clearance_at_cd_7p5() {
    const CD: f64 = 7.5;
    let (sun_root, sun_tip) = extents(&doc("[gear 0.5 10.0 6.0]"));
    let (planet_root, planet_tip) = extents(&doc("[gear 0.5 20.0 6.0]"));
    let (ring_tip, ring_root) = extents(&doc("[ring-gear 0.5 50.0 6.0]"));

    // Centre distances are consistent: m(10+20)/2 = m(50-20)/2 = 7.5.
    for (name, c) in [
        ("sun tip / planet root", CD - sun_tip - planet_root),
        ("planet tip / sun root", CD - planet_tip - sun_root),
        ("planet tip / ring root", ring_root - (CD + planet_tip)),
        ("ring tip / planet root", ring_tip - (CD + planet_root)),
    ] {
        assert!(c >= 0.1, "{name}: clearance {c:.4} < 0.1 at cd {CD}");
    }
}

#[test]
fn backlash_thins_teeth_without_moving_tip_or_root() {
    let plain = extents(&doc("[gear 0.5 20.0 6.0]"));
    let thinned = extents(&doc("[gear-backlash 0.5 20.0 6.0 0.02]"));
    assert!((plain.0 - thinned.0).abs() < 1e-12);
    assert!((plain.1 - thinned.1).abs() < 1e-12);

    // ...but the teeth really are thinner: the enclosed profile area drops
    // by roughly (backlash x tooth height) per tooth. Area is the honest
    // whole-profile measure — a tip/root check alone passes on a blank.
    let area = |src: &str| {
        let d = doc(src);
        let sketch_id = d
            .nodes
            .values()
            .find_map(|n| match &n.op {
                CsgOp::Extrude { sketch, .. } => Some(*sketch),
                _ => None,
            })
            .unwrap();
        match &d.nodes[&sketch_id].op {
            CsgOp::Sketch2D { segments, .. } => segments
                .iter()
                .map(|s| match s {
                    vcad_ir::SketchSegment2D::Line { start, end } => {
                        start.x * end.y - end.x * start.y
                    }
                    _ => 0.0,
                })
                .sum::<f64>()
                / 2.0,
            _ => unreachable!(),
        }
    };
    let a_plain = area("[gear 0.5 20.0 6.0]");
    let a_thin = area("[gear-backlash 0.5 20.0 6.0 0.02]");
    assert!(a_thin < a_plain, "backlash did not thin the teeth: {a_thin} vs {a_plain}");
    // 20 teeth x 0.02 thinning x ~1.125 mm of tooth height, within a factor
    // of two — enough to catch a backlash that is ignored or double-applied.
    let removed = a_plain - a_thin;
    assert!(
        (0.2..0.9).contains(&removed),
        "removed area {removed:.4} outside the expected band"
    );
}

#[test]
fn a_blank_gear_cannot_be_expressed() {
    // Zero module, three teeth, zero face and a backlash that eats the tooth
    // are all hard errors rather than a silently blank cylinder.
    for src in [
        "[gear 0.0 10.0 6.0]",
        "[gear 0.5 3.0 6.0]",
        "[gear 0.5 10.0 0.0]",
        "[gear-backlash 0.5 10.0 6.0 5.0]",
    ] {
        assert!(eval_vcad(src, None).is_err(), "{src} should be rejected");
    }
}
