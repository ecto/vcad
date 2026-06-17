//! Tracked regression for a BRep boolean robustness bug exposed by chained
//! booleans over curved surfaces.
//!
//! Minimal trigger (all three required — removing any one makes it pass):
//!   1. a *filleted* body (curved blend faces),
//!   2. a *sphere* subtracted from it (leaves a concave spherical face), then
//!   3. a *cylinder* subtracted from that result (a curved tool).
//!
//! Expected: `(filleted_cube − sphere) − cylinder` is the body with a spherical
//! dish and a small cylindrical notch — its bounding box stays within the body
//! (z ≈ 0..28). Observed: the second difference produces invalid topology in
//! which the subtracted sphere *resurfaces as positive geometry*, ballooning the
//! bounding box to the sphere's extent (z ≈ 94).
//!
//! This is NOT a loon / IR / evaluator bug — the IR is a correct Difference
//! chain and the node evaluator composes it correctly (see
//! `vcad-eval/tests/loon_boolean_composition.rs`, which passes on planar
//! geometry). The fault is in the curved-surface SSI / classification path of
//! `vcad-kernel-booleans`. The companion tests
//! `chained_planar_boolean_is_robust` and `single_curved_difference_is_robust`
//! below are NOT ignored and guard the working cases.
//!
//! When the kernel is fixed, drop the `#[ignore]` from
//! `chained_curved_boolean_resurfaces_sphere`.

use vcad_kernel::Solid;

fn zmax(s: &Solid) -> f64 {
    s.bounding_box().1[2]
}

/// The exact reported failing geometry. Ignored until the boolean kernel is
/// fixed; flips from `z≈94` (sphere resurfaced) to `z≈28` when correct.
#[test]
#[ignore = "known kernel boolean bug: chained curved difference resurfaces the subtracted sphere"]
fn chained_curved_boolean_resurfaces_sphere() {
    let body = Solid::cube(90.0, 60.0, 28.0).fillet(14.0);
    let sphere = Solid::sphere(36.0, 0).translate(45.0, 30.0, 58.0);
    let cyl = Solid::cylinder(5.0, 4.0, 0).translate(45.0, 42.0, 25.0);

    let dished = body.difference(&sphere); // works alone (z stays ~28)
    let result = dished.difference(&cyl); // bug: sphere comes back

    let z = zmax(&result);
    assert!(
        z <= 30.0,
        "subtracted sphere resurfaced: result z-max = {z:.1} (body is ~28; sphere would push to ~94)"
    );
}

/// Guard: the same chain over planar tools is robust — proves nesting itself is
/// fine and pins the failure to curved surfaces.
#[test]
fn chained_planar_boolean_is_robust() {
    let body = Solid::cube(100.0, 100.0, 100.0);
    let h1 = Solid::cube(10.0, 10.0, 10.0).translate(10.0, 10.0, 10.0);
    let h2 = Solid::cube(10.0, 10.0, 10.0).translate(50.0, 50.0, 50.0);
    let h3 = Solid::cube(10.0, 10.0, 10.0).translate(80.0, 80.0, 80.0);

    let result = body.difference(&h1).difference(&h2).difference(&h3);
    let v = result.volume();
    assert!(
        (v - 997_000.0).abs() < 1.0,
        "planar chained difference vol={v}, want 997000"
    );
}

/// Guard: a single curved difference (sphere out of a filleted cube) is robust —
/// pins the failure to *chaining* a further curved cut, not the first one.
#[test]
fn single_curved_difference_is_robust() {
    let body = Solid::cube(90.0, 60.0, 28.0).fillet(14.0);
    let sphere = Solid::sphere(36.0, 0).translate(45.0, 30.0, 58.0);
    let z = zmax(&body.difference(&sphere));
    assert!(
        z <= 30.0,
        "single curved difference z-max={z:.1} (want ~28)"
    );
}
