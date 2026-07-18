//! The validation ladder's headline rungs.
//!
//! The analytic Kepler propagator is the oracle for the numeric
//! integrator (tested in-crate); here the numeric J2 propagation is held
//! against the **closed-form secular theory** (Vallado Eqs. 9-38/9-39) —
//! the two implementations share no code path.

use vcad_kernel_orbit::propagate::{propagate, ForceModel};
use vcad_kernel_orbit::secular::nodal_rate_rad_s;
use vcad_kernel_orbit::state::OrbitalElements;

/// Least-squares slope of y over t (suppresses J2 short-periodic
/// oscillations that endpoint differencing would alias).
fn slope(t: &[f64], y: &[f64]) -> f64 {
    let n = t.len() as f64;
    let tm = t.iter().sum::<f64>() / n;
    let ym = y.iter().sum::<f64>() / n;
    let num: f64 = t.iter().zip(y).map(|(a, b)| (a - tm) * (b - ym)).sum();
    let den: f64 = t.iter().map(|a| (a - tm) * (a - tm)).sum();
    num / den
}

fn measured_nodal_rate(el0: &OrbitalElements, orbits: f64, sample_s: f64) -> f64 {
    let sv0 = el0.to_state().unwrap();
    let total = orbits * el0.period_s();
    let n = (total / sample_s) as usize;
    let mut t = Vec::with_capacity(n);
    let mut raan = Vec::with_capacity(n);
    let mut prev = el0.raan;
    let mut unwrapped = el0.raan;
    let mut sv = sv0;
    for k in 0..=n {
        if k > 0 {
            sv = propagate(&sv, sample_s, 5.0, ForceModel::TwoBodyJ2);
        }
        let el = sv.to_elements().unwrap();
        // Unwrap Ω across the ±2π seam.
        let mut d = el.raan - prev;
        while d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        }
        while d < -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        unwrapped += d;
        prev = el.raan;
        t.push(k as f64 * sample_s);
        raan.push(unwrapped);
    }
    slope(&t, &raan)
}

#[test]
fn j2_nodal_regression_matches_vallado_closed_form_over_ten_orbits() {
    // The headline validation: numeric RK4+J2 node drift vs Eq. 9-38,
    // across three inclinations (prograde, near-polar retrograde, and
    // the ISS's own).
    for &i_deg in &[51.63_f64, 30.0, 98.2] {
        let el = OrbitalElements {
            a: 6795.0,
            e: 0.0007,
            i: i_deg.to_radians(),
            raan: 2.6,
            argp: 5.3,
            nu: 0.0,
        };
        let predicted = nodal_rate_rad_s(&el);
        let measured = measured_nodal_rate(&el, 10.0, 60.0);
        let rel = ((measured - predicted) / predicted).abs();
        assert!(
            rel < 0.01,
            "inclination {i_deg}°: closed form {predicted:.6e} rad/s, \
             numeric {measured:.6e} rad/s, rel err {rel:.4}"
        );
    }
}

#[test]
fn nodal_rate_flips_sign_across_polar() {
    // Prograde orbits regress, retrograde orbits progress; exactly polar
    // does neither. The numeric propagator must agree in sign.
    let make = |i_deg: f64| OrbitalElements {
        a: 7078.0,
        e: 0.001,
        i: i_deg.to_radians(),
        raan: 1.0,
        argp: 0.0,
        nu: 0.0,
    };
    assert!(measured_nodal_rate(&make(60.0), 3.0, 60.0) < 0.0);
    assert!(measured_nodal_rate(&make(120.0), 3.0, 60.0) > 0.0);
}
