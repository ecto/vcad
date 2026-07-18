//! Exact analytic two-body propagation via the elliptic Kepler equation.
//!
//! Convert to elements, advance the mean anomaly by n·Δt, solve
//! M = E − e·sin E for the eccentric anomaly (Newton, fail-closed), and
//! convert back — exact for bound two-body motion (Vallado Algorithm 2,
//! "KepEqtnE"; Battin §4.3). This propagator is the **oracle** the RK4
//! integrator in [`crate::propagate`] is validated against.

use crate::state::{OrbitalElements, StateVector};
use crate::OrbitError;

/// Solve Kepler's equation M = E − e·sin E for E (rad), elliptic case.
///
/// Newton iteration seeded with M (or π for high e), tolerance 1e-13 rad,
/// hard cap 50 iterations — non-convergence is an error, never a
/// best-effort value.
pub fn solve_kepler(mean_anomaly: f64, e: f64) -> Result<f64, OrbitError> {
    if !(0.0..1.0).contains(&e) {
        return Err(OrbitError::NotElliptic { a: f64::NAN, e });
    }
    let m = mean_anomaly;
    let mut big_e = if e < 0.8 { m } else { std::f64::consts::PI };
    for _ in 0..50 {
        let f = big_e - e * big_e.sin() - m;
        let fp = 1.0 - e * big_e.cos();
        let delta = f / fp;
        big_e -= delta;
        if delta.abs() < 1e-13 {
            return Ok(big_e);
        }
    }
    Err(OrbitError::KeplerNoConvergence {
        mean_anomaly: m,
        eccentricity: e,
    })
}

/// True anomaly ν from eccentric anomaly E.
pub fn true_from_eccentric(big_e: f64, e: f64) -> f64 {
    let half = big_e / 2.0;
    2.0 * (((1.0 + e) / (1.0 - e)).sqrt() * half.tan()).atan()
}

/// Eccentric anomaly E from true anomaly ν.
pub fn eccentric_from_true(nu: f64, e: f64) -> f64 {
    let half = nu / 2.0;
    2.0 * (((1.0 - e) / (1.0 + e)).sqrt() * half.tan()).atan()
}

/// Propagate a state exactly under two-body gravity by `dt_s` seconds.
pub fn propagate(sv: &StateVector, dt_s: f64) -> Result<StateVector, OrbitError> {
    let el = sv.to_elements()?;
    propagate_elements(&el, dt_s)?.to_state()
}

/// Propagate elements exactly under two-body gravity by `dt_s` seconds.
pub fn propagate_elements(el: &OrbitalElements, dt_s: f64) -> Result<OrbitalElements, OrbitError> {
    let e0 = eccentric_from_true(el.nu, el.e);
    let m0 = e0 - el.e * e0.sin();
    let m1 = m0 + el.mean_motion_rad_s() * dt_s;
    let e1 = solve_kepler(m1, el.e)?;
    Ok(OrbitalElements {
        nu: true_from_eccentric(e1, el.e),
        ..*el
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::norm;

    #[test]
    fn kepler_solver_satisfies_the_equation() {
        for &e in &[0.0, 0.0007, 0.1, 0.7, 0.95] {
            for k in 0..12 {
                let m = k as f64 * 0.55 - 3.0;
                let big_e = solve_kepler(m, e).unwrap();
                assert!(
                    (big_e - e * big_e.sin() - m).abs() < 1e-12,
                    "residual at e={e}, M={m}"
                );
            }
        }
    }

    #[test]
    fn anomaly_conversions_invert() {
        for k in 1..20 {
            let nu = k as f64 * 0.3 - 3.0;
            let e = 0.3;
            let back = true_from_eccentric(eccentric_from_true(nu, e), e);
            assert!((back - nu).abs() < 1e-12, "nu={nu} back={back}");
        }
    }

    #[test]
    fn full_period_returns_to_the_same_state() {
        let el = crate::state::OrbitalElements {
            a: 6795.0,
            e: 0.0007,
            i: 0.9,
            raan: 2.6,
            argp: 5.3,
            nu: 0.7,
        };
        let sv = el.to_state().unwrap();
        let after = propagate(&sv, el.period_s()).unwrap();
        let dr = [
            after.r[0] - sv.r[0],
            after.r[1] - sv.r[1],
            after.r[2] - sv.r[2],
        ];
        assert!(norm(dr) < 1e-6, "drift after one period: {} km", norm(dr));
    }

    #[test]
    fn parabolic_input_is_rejected() {
        assert!(solve_kepler(1.0, 1.0).is_err());
    }
}
