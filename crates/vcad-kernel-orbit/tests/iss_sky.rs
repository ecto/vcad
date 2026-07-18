//! The sky is the bench: propagate the checked-in ISS state and hold it
//! against the checked-in JPL Horizons ephemeris. No network, ever.
//!
//! Error budgets here are **empirical and honest**: they were set from
//! the measured model gap of the J2-only propagator against the real ISS
//! (which flies with drag, higher harmonics, and occasional thruster
//! activity that M0 does not model), with ~2× margin. If a future change
//! doubles the model gap, these fail — that is their job.

use vcad_kernel_orbit::ephemeris::{iss_fixture, Ephemeris};
use vcad_kernel_orbit::propagate::{propagate, ForceModel};
use vcad_kernel_orbit::receipt::{sky_comparison_claim, SkyComparison};
use vcad_kernel_orbit::state::norm;
use vcad_kernel_orbit::tle;

fn error_at_hours(h: f64) -> f64 {
    let eph = iss_fixture().unwrap();
    let t0 = &eph.points[0];
    let target_s = h * 3600.0;
    // Fixture rows are 300 s apart; find the row at the horizon.
    let idx = (target_s / 300.0).round() as usize;
    let pt = &eph.points[idx];
    let dt = Ephemeris::elapsed_s(t0, pt);
    assert!((dt - target_s).abs() < 1.0, "fixture row misaligned");
    let prop = propagate(&t0.state, dt, 10.0, ForceModel::TwoBodyJ2);
    let dr = [
        prop.r[0] - pt.state.r[0],
        prop.r[1] - pt.state.r[1],
        prop.r[2] - pt.state.r[2],
    ];
    norm(dr)
}

#[test]
fn j2_propagation_tracks_the_real_iss_within_budget() {
    // Budgets: measured gap ×~2 (see docs/orbit-m0.md for the measured
    // curve). J2-only vs the real sky grows mostly along-track (drag).
    // Measured gap on this fixture: 0.44 km @ 1 h, 2.33 km @ 6 h,
    // 9.77 km @ 24 h (see docs/orbit-m0.md).
    let checks = [(1.0, 2.0), (6.0, 8.0), (24.0, 25.0)];
    for (h, budget_km) in checks {
        let err = error_at_hours(h);
        assert!(
            err < budget_km,
            "position error at {h} h: {err:.1} km exceeds the {budget_km} km budget"
        );
    }
}

#[test]
fn two_body_without_j2_is_visibly_worse_at_24_hours() {
    // The J2 term must earn its keep against the real sky: switching it
    // off must widen the 24 h gap substantially (nodal regression alone
    // is ~5°/day ≈ hundreds of km of cross-track).
    let eph = iss_fixture().unwrap();
    let t0 = &eph.points[0];
    let idx = (24.0 * 3600.0 / 300.0) as usize;
    let pt = &eph.points[idx];
    let dt = Ephemeris::elapsed_s(t0, pt);
    let with_j2 = propagate(&t0.state, dt, 10.0, ForceModel::TwoBodyJ2);
    let without = propagate(&t0.state, dt, 10.0, ForceModel::TwoBody);
    let err = |p: &vcad_kernel_orbit::state::StateVector| {
        norm([
            p.r[0] - pt.state.r[0],
            p.r[1] - pt.state.r[1],
            p.r[2] - pt.state.r[2],
        ])
    };
    assert!(
        err(&without) > 3.0 * err(&with_j2),
        "two-body {:.1} km vs J2 {:.1} km at 24 h — J2 must dominate the correction",
        err(&without),
        err(&with_j2)
    );
}

#[test]
fn tle_and_horizons_agree_on_what_orbit_this_is() {
    // Two independent sky sources, one satellite: the TLE's mean elements
    // and the Horizons osculating state must describe the same orbit to
    // mean-vs-osculating accuracy (~15 km in a, ~0.01° in inclination).
    let tle = tle::parse(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/iss_2026-07-17.tle"
        ))
        .unwrap(),
    )
    .unwrap();
    let eph = iss_fixture().unwrap();
    let el = eph.points[0].state.to_elements().unwrap();
    assert!(
        (tle.semi_major_axis_km() - el.a).abs() < 20.0,
        "TLE a {} vs Horizons a {}",
        tle.semi_major_axis_km(),
        el.a
    );
    // TEME-of-date (TLE) vs ICRF (Horizons) equators differ by ~26 years
    // of precession — up to ~0.3° of apparent inclination.
    assert!(
        (tle.inclination_deg - el.i.to_degrees()).abs() < 0.5,
        "TLE i {} vs Horizons i {}",
        tle.inclination_deg,
        el.i.to_degrees()
    );
    // And the fixtures were cut on the same day.
    let jd_utc = Ephemeris::jd_utc(&eph.points[0]);
    assert!((tle.epoch_jd_utc - jd_utc).abs() < 1.0, "same-day fixtures");
}

#[test]
fn sky_measured_receipt_claim_passes_at_24h_and_says_measured() {
    let cmp = SkyComparison {
        horizon_h: 24.0,
        position_error_km: error_at_hours(24.0),
        budget_km: 25.0,
        ephemeris_id: "horizons_iss_2026-07-17_72h.txt".into(),
    };
    let claim = sky_comparison_claim(&cmp);
    assert_eq!(claim.verdict, vcad_receipt::ClaimVerdict::Pass);
    assert_eq!(claim.basis, Some(vcad_receipt::ClaimBasis::Measured));
}
