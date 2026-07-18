//! The flagship: propagate the real ISS against the real sky.
//!
//! Takes the checked-in Horizons state at t0, propagates with two-body+J2,
//! and reports the position-error growth against the checked-in ephemeris
//! over 72 h — quantifying the M0 model gap (drag, higher harmonics,
//! possible reboosts are unmodeled) instead of hiding it. Then predicts
//! the next ISS passes over San Francisco and emits the
//! `vcad.orbit-claims/1` claim set.
//!
//! Run: `cargo run --release -p vcad-kernel-orbit --example iss_pass`

use vcad_kernel_orbit::ephemeris::{iss_fixture, Ephemeris};
use vcad_kernel_orbit::groundtrack::{format_jd_utc, subpoint, Site};
use vcad_kernel_orbit::pass::predict_passes;
use vcad_kernel_orbit::propagate::{propagate, ForceModel};
use vcad_kernel_orbit::receipt::{
    predicted_claims, sky_comparison_claim, SkyComparison, SolverProvenance,
};
use vcad_kernel_orbit::secular::{apsidal_rate_deg_per_day, nodal_rate_deg_per_day};
use vcad_kernel_orbit::state::norm;

const EPHEMERIS_ID: &str = "horizons_iss_2026-07-17_72h.txt";

fn main() {
    let eph = iss_fixture().expect("checked-in fixture");
    let t0 = eph.points[0];
    let el0 = t0.state.to_elements().expect("bound orbit");
    let jd0_utc = Ephemeris::jd_utc(&t0);

    println!("== ISS vs the sky: two-body+J2 against JPL Horizons ==");
    println!("fixture: {EPHEMERIS_ID} (geocentric ICRF, TDB stamps)");
    println!("t0 = {} (JD_TDB {:.6})", format_jd_utc(jd0_utc), t0.jd_tdb);
    println!(
        "osculating elements at t0: a = {:.2} km, e = {:.5}, i = {:.3}°, T = {:.1} s",
        el0.a,
        el0.e,
        el0.i.to_degrees(),
        el0.period_s()
    );
    println!(
        "J2 secular rates: dΩ/dt = {:.4} °/day, dω/dt = {:.4} °/day",
        nodal_rate_deg_per_day(&el0),
        apsidal_rate_deg_per_day(&el0)
    );

    // -- Error growth vs the sky ------------------------------------------
    println!("\nposition error vs Horizons (J2-only model gap, honest):");
    println!(
        "{:>8} {:>14} {:>14}",
        "hours", "J2 err (km)", "2-body err (km)"
    );
    let mut error_curve: Vec<(f64, f64)> = Vec::new();
    for &h in &[1.0_f64, 3.0, 6.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0] {
        let idx = (h * 12.0).round() as usize; // 5-min rows
        let pt = &eph.points[idx];
        let dt = Ephemeris::elapsed_s(&t0, pt);
        let with_j2 = propagate(&t0.state, dt, 10.0, ForceModel::TwoBodyJ2);
        let two_body = propagate(&t0.state, dt, 10.0, ForceModel::TwoBody);
        let err = |p: [f64; 3]| {
            norm([
                p[0] - pt.state.r[0],
                p[1] - pt.state.r[1],
                p[2] - pt.state.r[2],
            ])
        };
        let e_j2 = err(with_j2.r);
        error_curve.push((h, e_j2));
        println!("{h:>8.0} {e_j2:>14.2} {:>14.2}", err(two_body.r));
    }
    let e24 = error_curve
        .iter()
        .find(|(h, _)| *h == 24.0)
        .map(|(_, e)| *e)
        .unwrap();
    println!(
        "\nmodel gap read: the J2-only propagator tracks the real ISS to \
         {e24:.0} km at 24 h;\nthe growth is dominated by unmodeled drag \
         (along-track) — M1 adds drag + SGP4-compat mode."
    );

    // -- Pass prediction over San Francisco --------------------------------
    let site = Site {
        lat_rad: 37.7749_f64.to_radians(),
        lon_rad: (-122.4194_f64).to_radians(),
        alt_km: 0.016,
    };
    let mask = 10.0_f64.to_radians();
    let passes = predict_passes(
        &t0.state,
        jd0_utc,
        jd0_utc + 1.0,
        &site,
        mask,
        ForceModel::TwoBodyJ2,
        10.0,
        30.0,
    )
    .expect("pass prediction");
    println!(
        "\nISS passes over San Francisco (37.77° N, 122.42° W), next 24 h, \
         mask 10°: {} passes",
        passes.len()
    );
    for (k, p) in passes.iter().enumerate() {
        let (lat, lon) = subpoint(
            propagate(
                &t0.state,
                (p.culmination_jd_utc - jd0_utc) * 86_400.0,
                10.0,
                ForceModel::TwoBodyJ2,
            )
            .r,
            p.culmination_jd_utc,
        );
        println!(
            "  pass {}: rise {}  set {}  max el {:>4.1}°  dur {:>5.1} min  \
             subpoint at peak {:+.1}°/{:+.1}°",
            k + 1,
            format_jd_utc(p.rise_jd_utc),
            format_jd_utc(p.set_jd_utc),
            p.max_elevation_rad.to_degrees(),
            p.duration_s() / 60.0,
            lat.to_degrees(),
            lon.to_degrees()
        );
    }
    println!("  (J2-only + GMST-only: times honest to ±minutes, not ±seconds)");

    // -- Receipt ------------------------------------------------------------
    let provenance = SolverProvenance {
        integrator: "rk4-fixed".into(),
        step_s: 10.0,
        force_model: "two-body+J2".into(),
        frame_note: "ICRF treated as inertial Earth-equator frame; GMST-only \
                     Earth rotation; TDB−UTC = 69.184 s constant"
            .into(),
        ephemeris_id: Some(EPHEMERIS_ID.into()),
    };
    let set = predicted_claims(&el0, Some(&passes), provenance);
    let mut receipt_claims = vcad_kernel_orbit::receipt::design_claims(&set);
    receipt_claims.push(sky_comparison_claim(&SkyComparison {
        horizon_h: 24.0,
        position_error_km: e24,
        budget_km: 25.0,
        ephemeris_id: EPHEMERIS_ID.into(),
    }));
    let receipt = vcad_receipt::DesignReceipt::with_claims(receipt_claims);
    println!("\nreceipt verdict: {:?}", receipt.verdict());
    println!(
        "claims: {} predicted + 1 sky-measured (position_error_km_at_24h = \
         {e24:.1} km vs 25 km budget)",
        set.claims.len()
    );
}
