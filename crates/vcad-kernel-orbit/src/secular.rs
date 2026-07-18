//! Closed-form J2 secular rates and sun-synchronous design.
//!
//! Vallado 4th ed., Eqs. 9-38 / 9-39 (first-order J2 secular theory):
//!
//! - Nodal regression:  dΩ/dt = −(3/2) n J2 (Re/p)² cos i
//! - Apsidal rotation:  dω/dt =  (3/4) n J2 (Re/p)² (5 cos²i − 1)
//!
//! These are the headline validation targets for the numeric propagator:
//! the RK4 + J2 node drift over 10 orbits must reproduce dΩ/dt.

use crate::constants::{
    J2_EARTH, MU_EARTH_KM3_S2, R_EARTH_KM, SECONDS_PER_DAY, TROPICAL_YEAR_DAYS,
};
use crate::state::OrbitalElements;
use crate::OrbitError;

/// Nodal regression rate dΩ/dt, rad/s (Vallado Eq. 9-38).
pub fn nodal_rate_rad_s(el: &OrbitalElements) -> f64 {
    let n = el.mean_motion_rad_s();
    let p = el.semilatus_rectum_km();
    -1.5 * n * J2_EARTH * (R_EARTH_KM / p).powi(2) * el.i.cos()
}

/// Apsidal rotation rate dω/dt, rad/s (Vallado Eq. 9-39).
pub fn apsidal_rate_rad_s(el: &OrbitalElements) -> f64 {
    let n = el.mean_motion_rad_s();
    let p = el.semilatus_rectum_km();
    0.75 * n * J2_EARTH * (R_EARTH_KM / p).powi(2) * (5.0 * el.i.cos() * el.i.cos() - 1.0)
}

/// Nodal regression rate in degrees per day (reporting convenience).
pub fn nodal_rate_deg_per_day(el: &OrbitalElements) -> f64 {
    nodal_rate_rad_s(el).to_degrees() * SECONDS_PER_DAY
}

/// Apsidal rotation rate in degrees per day (reporting convenience).
pub fn apsidal_rate_deg_per_day(el: &OrbitalElements) -> f64 {
    apsidal_rate_rad_s(el).to_degrees() * SECONDS_PER_DAY
}

/// Sun-synchronous inclination (rad) for a circular orbit of semi-major
/// axis `a_km`: the inclination whose nodal rate equals +360°/tropical
/// year, found in closed form by inverting Eq. 9-38.
///
/// Fail-closed: if no inclination achieves the required rate (orbit too
/// high), returns an error rather than a clamped value.
pub fn sun_synchronous_inclination(a_km: f64) -> Result<f64, OrbitError> {
    let required_rad_s = (360.0_f64).to_radians() / (TROPICAL_YEAR_DAYS * SECONDS_PER_DAY);
    let n = (MU_EARTH_KM3_S2 / a_km.powi(3)).sqrt();
    let p = a_km; // circular
    let cos_i = required_rad_s / (-1.5 * n * J2_EARTH * (R_EARTH_KM / p).powi(2));
    if !(-1.0..=1.0).contains(&cos_i) {
        return Err(OrbitError::Invalid(format!(
            "no sun-synchronous inclination exists at a = {a_km} km (cos i = {cos_i})"
        )));
    }
    Ok(cos_i.acos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iss_nodal_regression_is_about_minus_five_degrees_per_day() {
        // Textbook anchor: the ISS node regresses ~ −5°/day.
        let el = OrbitalElements {
            a: 6795.0,
            e: 0.0007,
            i: 51.63_f64.to_radians(),
            raan: 0.0,
            argp: 0.0,
            nu: 0.0,
        };
        let rate = nodal_rate_deg_per_day(&el);
        assert!((-5.2..=-4.8).contains(&rate), "ISS nodal rate {rate} °/day");
    }

    #[test]
    fn sun_synchronous_at_700_km_is_98_point_2_degrees() {
        // Beloved sanity anchor. Note: at 700 km the answer is 98.19°,
        // not the oft-misremembered ~97.8° (that value belongs to
        // ~600 km) — the formula, not folklore, is authoritative here.
        let i = sun_synchronous_inclination(R_EARTH_KM + 700.0)
            .unwrap()
            .to_degrees();
        assert!((i - 98.19).abs() < 0.05, "SSO inclination at 700 km: {i}°");
        let i600 = sun_synchronous_inclination(R_EARTH_KM + 600.0)
            .unwrap()
            .to_degrees();
        assert!((i600 - 97.79).abs() < 0.05, "SSO at 600 km: {i600}°");
    }

    #[test]
    fn sun_synchronous_fails_closed_when_unreachable() {
        assert!(sun_synchronous_inclination(60_000.0).is_err());
    }

    #[test]
    fn critical_inclination_zeroes_the_apsidal_rate() {
        // cos²i = 1/5 → i = 63.4349° (Molniya's reason for being).
        let el = OrbitalElements {
            a: 26_562.0,
            e: 0.74,
            i: 63.4349_f64.to_radians(),
            raan: 0.0,
            argp: 0.0,
            nu: 0.0,
        };
        assert!(
            apsidal_rate_deg_per_day(&el).abs() < 1e-4,
            "apsidal rate at critical inclination: {}",
            apsidal_rate_deg_per_day(&el)
        );
    }
}
