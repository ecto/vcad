//! **M0 — harness.** Extrude distance `d`, QoI = volume; `dV/dd` is analytic.
//!
//! This is the smoke test that proves the whole pipeline + the central-
//! difference oracle end-to-end with zero geometric subtlety. Everything
//! downstream reuses this harness (`audit`).
//!
//! Acceptance gate: max relative error ≤ 1e-6 (per node and for the volume
//! derivative). We typically see ~1e-9.

use vcad_kernel_tessellate::frozen::{audit, models::ExtrudedBox, ParametricModel};

const GATE: f64 = 1e-6;
const H: f64 = 1e-6;

#[test]
fn m0_extrude_volume_derivative_matches_fd_and_closed_form() {
    let model = ExtrudedBox::new([3.0, 5.0], 4.0);
    let report = audit(&model, H).expect("no topology change for a valid extrude");
    eprintln!(
        "M0 max_node_rel_err={:e} vol_rel_err={:e}",
        report.max_node_rel_err, report.vol_rel_err
    );

    // Node-wise analytic dx/dθ vs central-difference oracle.
    assert!(
        report.max_node_rel_err <= GATE,
        "node dx/dd max rel err {} exceeds gate {}",
        report.max_node_rel_err,
        GATE
    );

    // Analytic dV/dd vs FD oracle.
    assert!(
        report.vol_rel_err <= GATE,
        "dV/dd analytic-vs-FD rel err {} exceeds gate",
        report.vol_rel_err
    );

    // Both must match the closed form dV/dd = sx·sy = 15.
    let closed = model.analytic_dvol();
    assert!((report.analytic_dvol - closed).abs() / closed <= GATE);
    assert!((report.fd_dvol - closed).abs() / closed <= GATE);

    // Sanity: the volume itself is sx·sy·d = 60 at d=4.
    let tess = model.tessellation();
    let v = tess.volume(&model.build(model.theta0()));
    assert!((v - 60.0).abs() < 1e-9, "volume {v}");
}
