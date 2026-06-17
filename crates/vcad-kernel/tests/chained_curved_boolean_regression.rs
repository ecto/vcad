//! Regression test for invalid topology produced by chained curved booleans.
//!
//! The BRep boolean pipeline (`vcad-kernel-booleans`) produced invalid topology
//! when a curved tool was subtracted from a solid that already carried a concave
//! curved face from a prior boolean. A subtracted sphere would "resurface" as
//! positive geometry, blowing the result's bounding box well past the original
//! body.
//!
//! Minimal trigger (all three are required — removing any one makes it pass):
//!   1. a filleted body (curved blend faces),
//!   2. a sphere subtracted from it (leaves a concave spherical face),
//!   3. a cylinder subtracted from that result (a curved tool).
//!
//! The two guard tests below confirm this is NOT a loon/IR/evaluator issue:
//! chained *planar* booleans and a *single* curved difference are both robust.

use vcad_kernel::Solid;

/// z-extent the model should never exceed: the filleted body tops out at 28.
const EXPECTED_Z_MAX: f64 = 28.0;
/// Generous tolerance — the bug pushes z-max to ~94, far outside this band.
const Z_TOLERANCE: f64 = 2.0;

fn z_max(solid: &Solid) -> f64 {
    let (_min, max) = solid.bounding_box();
    max[2]
}

/// The headline regression: filleted body − sphere − cylinder must not let the
/// subtracted sphere resurface above the body.
#[test]
fn chained_curved_boolean_resurfaces_sphere() {
    let body = Solid::cube(90.0, 60.0, 28.0).fillet(14.0);
    let sphere = Solid::sphere(36.0, 0).translate(45.0, 30.0, 58.0);
    let cyl = Solid::cylinder(5.0, 4.0, 0).translate(45.0, 42.0, 25.0);

    let result = body.difference(&sphere).difference(&cyl);

    let zmax = z_max(&result);
    assert!(
        zmax <= EXPECTED_Z_MAX + Z_TOLERANCE,
        "subtracted sphere resurfaced: result z-max = {zmax:.2}, expected <= {:.2}",
        EXPECTED_Z_MAX + Z_TOLERANCE
    );
}

/// Guard: the same nesting structure with planar tools is robust. Confirms the
/// fault is specific to the curved-surface boolean path, not nesting/eval.
#[test]
fn chained_planar_boolean_is_robust() {
    let body = Solid::cube(90.0, 60.0, 28.0);
    let notch = Solid::cube(20.0, 20.0, 20.0).translate(35.0, 20.0, 18.0);
    let slot = Solid::cube(10.0, 8.0, 20.0).translate(40.0, 26.0, 14.0);

    let result = body.difference(&notch).difference(&slot);

    let zmax = z_max(&result);
    assert!(
        zmax <= EXPECTED_Z_MAX + Z_TOLERANCE,
        "chained planar boolean is unexpectedly broken: z-max = {zmax:.2}"
    );
}

/// Guard: a single curved difference (no prior concave curved face) is robust.
#[test]
fn single_curved_difference_is_robust() {
    let body = Solid::cube(90.0, 60.0, 28.0).fillet(14.0);
    let cyl = Solid::cylinder(5.0, 4.0, 0).translate(45.0, 42.0, 25.0);

    let result = body.difference(&cyl);

    let zmax = z_max(&result);
    assert!(
        zmax <= EXPECTED_Z_MAX + Z_TOLERANCE,
        "single curved difference is unexpectedly broken: z-max = {zmax:.2}"
    );
}
