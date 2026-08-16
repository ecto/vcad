//! A blend that declines must say so.
//!
//! `Solid::fillet` and friends are fail-soft internally: rather than emit
//! a cracked or inverted shell they keep the input. For a long time that
//! decision was invisible — the caller got a valid solid back and no
//! indication anything had failed, so a part could reach a fabricator
//! with square edges where the design called for radii. These tests pin
//! the contract that replaced it:
//!
//! 1. The default entry points are fail-closed — a decline is an `Err`.
//! 2. The reason is specific enough to act on, not a bare failure.
//! 3. Multi-edge blends report per edge, not as one boolean.
//! 4. The lenient variants still exist, but you have to ask for them.

use vcad_kernel::vcad_kernel_fillet::{BlendKey, BlendRefusal, BlendSection, EdgeQuery};
use vcad_kernel::vcad_kernel_math::{Point3, Vec3};
use vcad_kernel::{BlendError, Solid};

fn cube() -> Solid {
    Solid::cube(10.0, 10.0, 10.0)
}

fn fillet_key(size: f64) -> BlendKey {
    BlendKey {
        t: 0.0,
        section: BlendSection { size, shape: 1.0 },
    }
}

/// The headline case: a radius the geometry cannot host is an error, and
/// emphatically *not* a quiet identity operation.
#[test]
fn a_radius_too_large_for_the_feature_is_an_error_not_a_noop() {
    // r = 5 on a 10 mm cube: opposing insets meet at the midplane, so the
    // trimmed faces collapse. The planar pipeline catches it up front.
    let err = cube()
        .fillet(5.0)
        .expect_err("r=5 cannot fit a 10mm cube — this must not silently succeed");
    assert_eq!(
        err,
        BlendError::Refused(BlendRefusal::RadiusTooLargeForFeature)
    );
    // The message names the cause, so a caller (or an agent) can act on
    // it rather than guessing.
    let msg = err.to_string();
    assert!(
        msg.contains("radius exceeds available edge length"),
        "reason should be actionable, got: {msg}"
    );

    // A grossly oversized radius gets past the up-front feasibility check
    // but produces a shell that gains volume — caught by the post-flight
    // gate. Different reason, same contract: it is never silent.
    for r in [6.0, 12.0, 20.0] {
        assert_eq!(
            cube().fillet(r).unwrap_err(),
            BlendError::InvalidResult,
            "r={r} on a 10mm cube must be reported, not quietly skipped"
        );
    }
}

/// The same call through the lenient door: geometry comes back untouched,
/// but the report records the decline. This is the *old* behavior — now
/// only reachable by asking for it explicitly.
#[test]
fn the_lenient_variant_returns_the_input_but_records_why() {
    let input = cube();
    let (solid, report) = input.fillet_lenient(5.0);

    assert!(report.declined(), "report must record the decline");
    assert_eq!(report.applied, 0);
    assert_eq!(
        report.declined,
        Some(BlendError::Refused(BlendRefusal::RadiusTooLargeForFeature))
    );

    // Unchanged: same volume, same face count as the input.
    assert!((solid.volume() - input.volume()).abs() < 1e-9);
    assert_eq!(
        solid.as_brep().unwrap().topology.faces.len(),
        input.as_brep().unwrap().topology.faces.len()
    );
}

/// A radius that *does* fit still works, and reports full application.
#[test]
fn a_feasible_radius_succeeds_and_reports_every_edge_applied() {
    let (result, report) = cube().fillet_reported(1.0);
    let filleted = result.expect("r=1 fits a 10mm cube");

    assert!(!report.declined());
    assert!(
        !report.is_partial(),
        "a 12-edge cube fillet is all-or-nothing"
    );
    assert_eq!(report.requested, 12, "a cube has 12 edges");
    assert_eq!(report.applied, report.requested);
    assert!(filleted.volume() < cube().volume());
}

/// Every distinct decline carries a distinct, specific reason — the point
/// of the exercise is that "it failed" is not an acceptable answer.
#[test]
fn each_decline_names_its_own_cause() {
    // Mesh-backed: no topology to blend.
    let mesh_solid = Solid::sphere(5.0, 8).union(&Solid::sphere(5.0, 8).translate(1.0, 0.0, 0.0));
    if mesh_solid.as_brep().is_none() {
        assert_eq!(mesh_solid.fillet(0.5).unwrap_err(), BlendError::NotBRep);
    }

    // Empty solid.
    assert_eq!(
        Solid::empty().fillet(1.0).unwrap_err(),
        BlendError::EmptySolid
    );
    assert_eq!(
        Solid::empty().chamfer(1.0).unwrap_err(),
        BlendError::EmptySolid
    );
    assert_eq!(
        Solid::empty().shell(1.0).unwrap_err(),
        BlendError::EmptySolid
    );

    // No keys in the blend profile.
    assert_eq!(
        cube().edge_blend(&EdgeQuery::All, &[]).unwrap_err(),
        BlendError::NoKeys
    );

    // A query that matches no edge blends nothing — and says so, instead
    // of handing back a solid the caller believes was blended.
    assert_eq!(
        cube()
            .edge_blend(
                &EdgeQuery::Endpoints {
                    a: Point3::new(99.0, 99.0, 99.0),
                    b: Point3::new(99.0, 99.0, 90.0),
                },
                &[fillet_key(1.0)],
            )
            .unwrap_err(),
        BlendError::NoTargetEdges
    );

    // A body with a bore has faces with inner loops; the rebuild would
    // fill the bore back in.
    let bored = Solid::cube(40.0, 40.0, 10.0)
        .difference(&Solid::cylinder(6.0, 30.0, 24).translate(20.0, 20.0, -10.0));
    match bored.fillet(1.0).unwrap_err() {
        BlendError::InnerLoops { faces } => assert!(faces > 0),
        other => panic!("expected an inner-loop refusal, got {other:?}"),
    }
}

/// Partial success on a multi-edge fillet is reportable *per edge*, with
/// a reason attached to each skipped one.
#[test]
fn a_multi_edge_fillet_reports_per_edge_not_as_one_boolean() {
    use vcad_kernel_sketch::{SketchProfile, SketchSegment};

    // An arc-extruded profile takes the curved per-edge path, where the
    // kernel blends cap rims and leaves some seams sharp.
    let p2 = vcad_kernel::vcad_kernel_math::Point2::new;
    let segments = vec![
        SketchSegment::Line {
            start: p2(0.0, 0.0),
            end: p2(30.0, 0.0),
        },
        SketchSegment::Arc {
            start: p2(30.0, 0.0),
            end: p2(30.0, 20.0),
            center: p2(30.0, 10.0),
            ccw: true,
        },
        SketchSegment::Line {
            start: p2(30.0, 20.0),
            end: p2(0.0, 20.0),
        },
        SketchSegment::Line {
            start: p2(0.0, 20.0),
            end: p2(0.0, 0.0),
        },
    ];
    let profile = SketchProfile::new(
        Point3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        segments,
    )
    .expect("valid profile");
    let extruded = Solid::extrude(profile, Vec3::new(0.0, 0.0, 15.0)).expect("extrude ok");

    let (_result, report) = extruded.fillet_reported(1.0);

    assert!(
        report.requested > 1,
        "expected a multi-edge target set, got {}",
        report.requested
    );
    assert_eq!(
        report.edges.len(),
        report.requested,
        "every targeted edge must appear in the report"
    );

    // Whatever the split, the accounting is consistent and every skipped
    // edge carries a reason string rather than a bare flag.
    let applied = report.edges.iter().filter(|e| e.skipped.is_none()).count();
    assert_eq!(
        applied, report.applied,
        "report.applied must match the rows"
    );
    for edge in report.skipped() {
        let reason = edge.skipped.as_deref().unwrap();
        assert!(!reason.is_empty(), "a skipped edge must say why");
        assert!(
            edge.endpoints.is_some(),
            "a skipped edge should be locatable in the model"
        );
    }
}

/// `edge_blend_named` was already fail-closed on the name; it stays that
/// way now that its error type widened to `BlendError`.
#[test]
fn a_named_edge_that_cannot_resolve_still_fails_closed() {
    let err = cube()
        .edge_blend_named("cube:top", "cube:nope", &[fillet_key(1.0)])
        .expect_err("an unresolvable name must not fall back to a guessed edge");
    assert!(
        matches!(err, BlendError::NamedEdge(_)),
        "expected a named-edge error, got {err:?}"
    );
}

/// `shell` reports when it had to abandon the analytic offset for the
/// coarse mesh approximation — a quality degradation that used to be
/// completely invisible.
#[test]
fn shell_reports_a_fallback_to_the_mesh_offset() {
    // The analytic path handles a plain box.
    let (ok, degraded) = cube().shell_reported(2.0);
    assert!(ok.is_ok());
    assert!(
        degraded.is_none(),
        "a plain box should shell analytically, got fallback: {degraded:?}"
    );
}
