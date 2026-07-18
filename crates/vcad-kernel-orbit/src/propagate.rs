//! Numeric propagation: fixed-step RK4 on Cartesian state, two-body + J2.
//!
//! The J2 (Earth-oblateness) acceleration is Vallado 4th ed. Eq. 8-30.
//! Conservation diagnostics are the conscience of this module: under
//! two-body-only forces both specific energy and the angular-momentum
//! vector are exact invariants, and the tests hold RK4 to them over many
//! orbits. Under J2 the *z*-component of angular momentum and the total
//! energy (J2 is conservative) remain invariants — also tested.

use crate::constants::{J2_EARTH, MU_EARTH_KM3_S2, R_EARTH_KM};
use crate::state::{cross, dot, norm, StateVector};

/// Force model selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForceModel {
    /// Point-mass Earth only.
    TwoBody,
    /// Point-mass + J2 oblateness (Vallado Eq. 8-30).
    TwoBodyJ2,
}

/// Acceleration (km/s²) at position `r` (km) under the given model.
pub fn acceleration(r: [f64; 3], model: ForceModel) -> [f64; 3] {
    let r_mag = norm(r);
    let r3 = r_mag.powi(3);
    let mut a = [
        -MU_EARTH_KM3_S2 * r[0] / r3,
        -MU_EARTH_KM3_S2 * r[1] / r3,
        -MU_EARTH_KM3_S2 * r[2] / r3,
    ];
    if model == ForceModel::TwoBodyJ2 {
        // Vallado Eq. 8-30.
        let factor = 1.5 * J2_EARTH * MU_EARTH_KM3_S2 * R_EARTH_KM * R_EARTH_KM / r_mag.powi(5);
        let z2_r2 = (r[2] / r_mag) * (r[2] / r_mag);
        a[0] += factor * r[0] * (5.0 * z2_r2 - 1.0);
        a[1] += factor * r[1] * (5.0 * z2_r2 - 1.0);
        a[2] += factor * r[2] * (5.0 * z2_r2 - 3.0);
    }
    a
}

fn rk4_step(sv: &StateVector, dt: f64, model: ForceModel) -> StateVector {
    let f = |s: &StateVector| (s.v, acceleration(s.r, model));
    let add = |s: &StateVector, dr: [f64; 3], dv: [f64; 3], scale: f64| StateVector {
        r: [
            s.r[0] + dr[0] * scale,
            s.r[1] + dr[1] * scale,
            s.r[2] + dr[2] * scale,
        ],
        v: [
            s.v[0] + dv[0] * scale,
            s.v[1] + dv[1] * scale,
            s.v[2] + dv[2] * scale,
        ],
    };
    let (k1r, k1v) = f(sv);
    let (k2r, k2v) = f(&add(sv, k1r, k1v, dt / 2.0));
    let (k3r, k3v) = f(&add(sv, k2r, k2v, dt / 2.0));
    let (k4r, k4v) = f(&add(sv, k3r, k3v, dt));
    StateVector {
        r: [
            sv.r[0] + dt / 6.0 * (k1r[0] + 2.0 * k2r[0] + 2.0 * k3r[0] + k4r[0]),
            sv.r[1] + dt / 6.0 * (k1r[1] + 2.0 * k2r[1] + 2.0 * k3r[1] + k4r[1]),
            sv.r[2] + dt / 6.0 * (k1r[2] + 2.0 * k2r[2] + 2.0 * k3r[2] + k4r[2]),
        ],
        v: [
            sv.v[0] + dt / 6.0 * (k1v[0] + 2.0 * k2v[0] + 2.0 * k3v[0] + k4v[0]),
            sv.v[1] + dt / 6.0 * (k1v[1] + 2.0 * k2v[1] + 2.0 * k3v[1] + k4v[1]),
            sv.v[2] + dt / 6.0 * (k1v[2] + 2.0 * k2v[2] + 2.0 * k3v[2] + k4v[2]),
        ],
    }
}

/// Propagate by `dt_s` seconds with fixed RK4 steps of at most
/// `max_step_s`. The final partial step is shrunk to land exactly on
/// `dt_s`. Negative `dt_s` integrates backward.
pub fn propagate(sv: &StateVector, dt_s: f64, max_step_s: f64, model: ForceModel) -> StateVector {
    let n = (dt_s.abs() / max_step_s).ceil().max(1.0) as usize;
    let h = dt_s / n as f64;
    let mut s = *sv;
    for _ in 0..n {
        s = rk4_step(&s, h, model);
    }
    s
}

/// Specific orbital energy, km²/s²: v²/2 − μ/r (two-body part only —
/// the J2 potential term is excluded, see [`specific_energy_j2`]).
pub fn specific_energy(sv: &StateVector) -> f64 {
    dot(sv.v, sv.v) / 2.0 - MU_EARTH_KM3_S2 / norm(sv.r)
}

/// Specific energy including the J2 potential,
/// U = −μ/r · [1 − J2 (Re/r)² (3sin²φ − 1)/2], an exact invariant of the
/// two-body + J2 flow.
pub fn specific_energy_j2(sv: &StateVector) -> f64 {
    let r = norm(sv.r);
    let sin_phi = sv.r[2] / r;
    let u = -MU_EARTH_KM3_S2 / r
        * (1.0 - J2_EARTH * (R_EARTH_KM / r).powi(2) * (3.0 * sin_phi * sin_phi - 1.0) / 2.0);
    dot(sv.v, sv.v) / 2.0 + u
}

/// Specific angular momentum vector, km²/s.
pub fn angular_momentum(sv: &StateVector) -> [f64; 3] {
    cross(sv.r, sv.v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::OrbitalElements;

    fn iss_like() -> StateVector {
        OrbitalElements {
            a: 6795.0,
            e: 0.0007,
            i: 51.63_f64.to_radians(),
            raan: 2.6,
            argp: 5.3,
            nu: 0.9,
        }
        .to_state()
        .unwrap()
    }

    #[test]
    fn two_body_conserves_energy_and_angular_momentum_over_ten_orbits() {
        let sv = iss_like();
        let period = sv.to_elements().unwrap().period_s();
        let e0 = specific_energy(&sv);
        let h0 = angular_momentum(&sv);
        let after = propagate(&sv, 10.0 * period, 10.0, ForceModel::TwoBody);
        let e1 = specific_energy(&after);
        let h1 = angular_momentum(&after);
        // RK4 at dt = 10 s over ~15.5 h: measured drift is a few 1e-10
        // relative — the bound sits just above that floor so regressions
        // (not float noise) trip it.
        assert!(
            ((e1 - e0) / e0).abs() < 1e-9,
            "energy drift {:.3e}",
            ((e1 - e0) / e0).abs()
        );
        for k in 0..3 {
            assert!((h1[k] - h0[k]).abs() / norm(h0) < 1e-9, "h[{k}] drift");
        }
    }

    #[test]
    fn rk4_matches_the_analytic_kepler_oracle() {
        // The exact propagator is the oracle: over 10 orbits at dt = 1 s,
        // RK4 must agree to sub-meter.
        let sv = iss_like();
        let period = sv.to_elements().unwrap().period_s();
        let t = 10.0 * period;
        let numeric = propagate(&sv, t, 1.0, ForceModel::TwoBody);
        let exact = crate::kepler::propagate(&sv, t).unwrap();
        let dr = [
            numeric.r[0] - exact.r[0],
            numeric.r[1] - exact.r[1],
            numeric.r[2] - exact.r[2],
        ];
        let err_km = norm(dr);
        assert!(err_km < 1e-3, "RK4 vs Kepler after 10 orbits: {err_km} km");
    }

    #[test]
    fn j2_conserves_j2_energy_and_hz_but_precesses_the_node() {
        let sv = iss_like();
        let period = sv.to_elements().unwrap().period_s();
        let e0 = specific_energy_j2(&sv);
        let hz0 = angular_momentum(&sv)[2];
        let after = propagate(&sv, 10.0 * period, 5.0, ForceModel::TwoBodyJ2);
        let e1 = specific_energy_j2(&after);
        let hz1 = angular_momentum(&after)[2];
        assert!(((e1 - e0) / e0).abs() < 1e-9, "J2 energy drift");
        assert!(
            ((hz1 - hz0) / hz0).abs() < 1e-9,
            "h_z must be invariant under the axisymmetric J2 field"
        );
        // And the node must actually have moved (regressed) — J2 is on.
        let raan0 = sv.to_elements().unwrap().raan;
        let raan1 = after.to_elements().unwrap().raan;
        let mut d = raan1 - raan0;
        while d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        }
        while d < -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        assert!(d < -1e-3, "prograde LEO node must regress, got ΔΩ = {d}");
    }

    #[test]
    fn backward_propagation_inverts_forward() {
        let sv = iss_like();
        let fwd = propagate(&sv, 3000.0, 5.0, ForceModel::TwoBodyJ2);
        let back = propagate(&fwd, -3000.0, 5.0, ForceModel::TwoBodyJ2);
        let dr = [
            back.r[0] - sv.r[0],
            back.r[1] - sv.r[1],
            back.r[2] - sv.r[2],
        ];
        assert!(norm(dr) < 1e-6, "round trip error {} km", norm(dr));
    }
}
