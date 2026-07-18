//! Cartesian state vectors ↔ classical orbital elements.
//!
//! Vallado, *Fundamentals of Astrodynamics and Applications*, 4th ed.,
//! Algorithm 9 (RV2COE) and Algorithm 10 (COE2RV). Units: km, km/s, rad.

use crate::constants::MU_EARTH_KM3_S2;
use crate::OrbitError;

/// Cartesian inertial state: position (km) and velocity (km/s).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StateVector {
    /// Position, km.
    pub r: [f64; 3],
    /// Velocity, km/s.
    pub v: [f64; 3],
}

/// Classical (Keplerian) osculating elements. Angles in radians.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitalElements {
    /// Semi-major axis, km.
    pub a: f64,
    /// Eccentricity.
    pub e: f64,
    /// Inclination, rad.
    pub i: f64,
    /// Right ascension of the ascending node Ω, rad.
    pub raan: f64,
    /// Argument of perigee ω, rad.
    pub argp: f64,
    /// True anomaly ν, rad.
    pub nu: f64,
}

/// Dot product of two 3-vectors.
pub fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Cross product of two 3-vectors.
pub fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Euclidean norm of a 3-vector.
pub fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

fn wrap_2pi(x: f64) -> f64 {
    let two_pi = std::f64::consts::TAU;
    let r = x % two_pi;
    if r < 0.0 {
        r + two_pi
    } else {
        r
    }
}

impl StateVector {
    /// Convert to classical elements (Vallado Algorithm 9).
    ///
    /// Fail-closed on non-elliptic orbits. Near-circular / near-equatorial
    /// singularities are handled with the standard quadrant conventions;
    /// for e ≲ 1e-8 the argument of perigee is ill-conditioned (the ISS at
    /// e ≈ 7e-4 is fine).
    pub fn to_elements(&self) -> Result<OrbitalElements, OrbitError> {
        let mu = MU_EARTH_KM3_S2;
        let r_mag = norm(self.r);
        let v_mag = norm(self.v);
        let h = cross(self.r, self.v);
        let h_mag = norm(h);
        let n_vec = [-h[1], h[0], 0.0]; // ẑ × h
        let n_mag = norm(n_vec);
        let rv = dot(self.r, self.v);
        let e_vec = [
            ((v_mag * v_mag - mu / r_mag) * self.r[0] - rv * self.v[0]) / mu,
            ((v_mag * v_mag - mu / r_mag) * self.r[1] - rv * self.v[1]) / mu,
            ((v_mag * v_mag - mu / r_mag) * self.r[2] - rv * self.v[2]) / mu,
        ];
        let e = norm(e_vec);
        let xi = v_mag * v_mag / 2.0 - mu / r_mag;
        if xi >= 0.0 {
            return Err(OrbitError::NotElliptic {
                a: f64::INFINITY,
                e,
            });
        }
        let a = -mu / (2.0 * xi);
        if e >= 1.0 || a <= 0.0 {
            return Err(OrbitError::NotElliptic { a, e });
        }
        let i = (h[2] / h_mag).clamp(-1.0, 1.0).acos();

        let raan = if n_mag > 1e-12 {
            let mut o = (n_vec[0] / n_mag).clamp(-1.0, 1.0).acos();
            if n_vec[1] < 0.0 {
                o = std::f64::consts::TAU - o;
            }
            o
        } else {
            0.0 // equatorial: node undefined, conventionally 0
        };

        let argp = if n_mag > 1e-12 && e > 1e-12 {
            let mut w = (dot(n_vec, e_vec) / (n_mag * e)).clamp(-1.0, 1.0).acos();
            if e_vec[2] < 0.0 {
                w = std::f64::consts::TAU - w;
            }
            w
        } else {
            0.0
        };

        let nu = if e > 1e-12 {
            let mut nu = (dot(e_vec, self.r) / (e * r_mag)).clamp(-1.0, 1.0).acos();
            if rv < 0.0 {
                nu = std::f64::consts::TAU - nu;
            }
            nu
        } else {
            // circular: measure from the node (argument of latitude)
            let mut u = (dot(n_vec, self.r) / (n_mag * r_mag))
                .clamp(-1.0, 1.0)
                .acos();
            if self.r[2] < 0.0 {
                u = std::f64::consts::TAU - u;
            }
            u
        };

        Ok(OrbitalElements {
            a,
            e,
            i,
            raan: wrap_2pi(raan),
            argp: wrap_2pi(argp),
            nu: wrap_2pi(nu),
        })
    }
}

impl OrbitalElements {
    /// Convert to a Cartesian state (Vallado Algorithm 10).
    pub fn to_state(&self) -> Result<StateVector, OrbitError> {
        if self.e >= 1.0 || self.a <= 0.0 {
            return Err(OrbitError::NotElliptic {
                a: self.a,
                e: self.e,
            });
        }
        let mu = MU_EARTH_KM3_S2;
        let p = self.a * (1.0 - self.e * self.e);
        let (sin_nu, cos_nu) = self.nu.sin_cos();
        let r_mag = p / (1.0 + self.e * cos_nu);
        // Perifocal (PQW) frame.
        let r_pqw = [r_mag * cos_nu, r_mag * sin_nu, 0.0];
        let sqrt_mu_p = (mu / p).sqrt();
        let v_pqw = [-sqrt_mu_p * sin_nu, sqrt_mu_p * (self.e + cos_nu), 0.0];
        // Rotate PQW → inertial: R3(−Ω) R1(−i) R3(−ω).
        let (so, co) = self.raan.sin_cos();
        let (si, ci) = self.i.sin_cos();
        let (sw, cw) = self.argp.sin_cos();
        let rot = [
            [co * cw - so * sw * ci, -co * sw - so * cw * ci, so * si],
            [so * cw + co * sw * ci, -so * sw + co * cw * ci, -co * si],
            [sw * si, cw * si, ci],
        ];
        let apply = |v: [f64; 3]| {
            [
                rot[0][0] * v[0] + rot[0][1] * v[1] + rot[0][2] * v[2],
                rot[1][0] * v[0] + rot[1][1] * v[1] + rot[1][2] * v[2],
                rot[2][0] * v[0] + rot[2][1] * v[1] + rot[2][2] * v[2],
            ]
        };
        Ok(StateVector {
            r: apply(r_pqw),
            v: apply(v_pqw),
        })
    }

    /// Orbital period, seconds: T = 2π√(a³/μ).
    pub fn period_s(&self) -> f64 {
        std::f64::consts::TAU * (self.a.powi(3) / MU_EARTH_KM3_S2).sqrt()
    }

    /// Mean motion n, rad/s.
    pub fn mean_motion_rad_s(&self) -> f64 {
        (MU_EARTH_KM3_S2 / self.a.powi(3)).sqrt()
    }

    /// Semi-latus rectum p = a(1−e²), km.
    pub fn semilatus_rectum_km(&self) -> f64 {
        self.a * (1.0 - self.e * self.e)
    }
}

/// Speed on the orbit at radius `r_km` via vis-viva: v² = μ(2/r − 1/a).
pub fn vis_viva_speed_km_s(a_km: f64, r_km: f64) -> f64 {
    (MU_EARTH_KM3_S2 * (2.0 / r_km - 1.0 / a_km)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn elements_state_round_trip() {
        let el = OrbitalElements {
            a: 6795.0,
            e: 0.0007,
            i: 51.6_f64.to_radians(),
            raan: 149.7_f64.to_radians(),
            argp: 306.9_f64.to_radians(),
            nu: 1.234,
        };
        let sv = el.to_state().unwrap();
        let back = sv.to_elements().unwrap();
        assert!(close(back.a, el.a, 1e-6), "a: {} vs {}", back.a, el.a);
        assert!(close(back.e, el.e, 1e-9));
        assert!(close(back.i, el.i, 1e-9));
        assert!(close(back.raan, el.raan, 1e-9));
        assert!(close(back.argp, el.argp, 1e-6));
        assert!(close(back.nu, el.nu, 1e-6));
    }

    #[test]
    fn vis_viva_circular_matches_sqrt_mu_over_r() {
        let r = 6778.0;
        let v = vis_viva_speed_km_s(r, r);
        assert!(close(v, (MU_EARTH_KM3_S2 / r).sqrt(), 1e-12));
        // LEO speed sanity: ~7.67 km/s at 400 km altitude.
        assert!(v > 7.6 && v < 7.8, "v = {v}");
    }

    #[test]
    fn period_of_geostationary_orbit_is_a_sidereal_day() {
        // a for a period of one sidereal day (86164.09 s): the classic
        // 42164 km anchor.
        let el = OrbitalElements {
            a: 42_164.17,
            e: 0.0,
            i: 0.0,
            raan: 0.0,
            argp: 0.0,
            nu: 0.0,
        };
        assert!(
            (el.period_s() - 86_164.09).abs() < 5.0,
            "T = {}",
            el.period_s()
        );
    }

    #[test]
    fn hyperbolic_state_is_rejected() {
        let sv = StateVector {
            r: [7000.0, 0.0, 0.0],
            v: [0.0, 12.0, 0.0], // > escape speed
        };
        assert!(matches!(
            sv.to_elements(),
            Err(OrbitError::NotElliptic { .. })
        ));
    }
}
