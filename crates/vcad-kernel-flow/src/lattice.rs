//! D3Q19 lattice constants, equilibrium, and physical↔lattice scaling.
//!
//! Scaling strategy (diffusive-acoustic hybrid, fail-closed): given the
//! voxel size `dx`, the fluid's kinematic viscosity ν, and a reference
//! speed `U`, pick the lattice velocity `u_lat = U·dt/dx` as large as the
//! Mach constraint allows (compressibility error ~ Ma², kept ≤ tolerance)
//! and derive the BGK relaxation time τ = 3·ν·dt/dx² + ½. If τ leaves the
//! validated stability window the scaling *refuses* with the resolution
//! that would fix it, instead of running an unstable or over-damped
//! lattice.

use serde::{Deserialize, Serialize};

/// Number of discrete velocities.
pub const Q: usize = 19;

/// D3Q19 velocity set. Index 0 is rest; opposite of `i` is [`OPP`]`[i]`.
pub const C: [[i32; 3]; Q] = [
    [0, 0, 0],
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
    [1, 1, 0],
    [-1, -1, 0],
    [1, -1, 0],
    [-1, 1, 0],
    [1, 0, 1],
    [-1, 0, -1],
    [1, 0, -1],
    [-1, 0, 1],
    [0, 1, 1],
    [0, -1, -1],
    [0, 1, -1],
    [0, -1, 1],
];

/// Quadrature weights matching [`C`].
pub const W: [f64; Q] = [
    1.0 / 3.0,
    1.0 / 18.0,
    1.0 / 18.0,
    1.0 / 18.0,
    1.0 / 18.0,
    1.0 / 18.0,
    1.0 / 18.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
    1.0 / 36.0,
];

/// Index of the opposite velocity: `C[OPP[i]] == -C[i]`.
pub const OPP: [usize; Q] = [
    0, 2, 1, 4, 3, 6, 5, 8, 7, 10, 9, 12, 11, 14, 13, 16, 15, 18, 17,
];

/// Lattice speed of sound squared, `c_s² = 1/3` in lattice units.
pub const CS2: f64 = 1.0 / 3.0;

/// Second-order BGK equilibrium for density `rho` and velocity `u`
/// (lattice units).
#[inline]
pub fn equilibrium(rho: f64, u: [f64; 3]) -> [f64; Q] {
    let uu = u[0] * u[0] + u[1] * u[1] + u[2] * u[2];
    let mut feq = [0.0; Q];
    for (i, f) in feq.iter_mut().enumerate() {
        let cu = C[i][0] as f64 * u[0] + C[i][1] as f64 * u[1] + C[i][2] as f64 * u[2];
        *f = W[i] * rho * (1.0 + 3.0 * cu + 4.5 * cu * cu - 1.5 * uu);
    }
    feq
}

/// Physical↔lattice unit mapping for one solve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Scaling {
    /// Voxel edge, m.
    pub dx_m: f64,
    /// Time step, s.
    pub dt_s: f64,
    /// BGK relaxation time, lattice units.
    pub tau: f64,
    /// Reference speed in lattice units (`U·dt/dx`).
    pub u_lattice: f64,
    /// Lattice Mach number `u_lattice / c_s`.
    pub mach: f64,
}

/// Why a scaling could not be derived. Fail-closed: each variant names
/// the change that fixes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScalingError {
    /// Inputs non-finite or non-positive.
    BadInputs,
    /// Resolving this viscosity at this speed needs τ above the stable
    /// window even at the minimum usable lattice velocity — the grid is
    /// too coarse (viscous scales unresolved).
    TooCoarse {
        /// τ that the minimum lattice velocity would give.
        tau_at_min_u: f64,
        /// Suggested minimum divisions multiplier.
        refine_factor: f64,
    },
    /// Resolving this viscosity at this speed would need τ below the
    /// stability floor even at the maximum Mach-safe lattice velocity —
    /// the cell Reynolds number is too high for the grid.
    TooFast {
        /// τ that the maximum lattice velocity would give.
        tau_at_max_u: f64,
        /// Suggested minimum divisions multiplier.
        refine_factor: f64,
    },
}

impl std::fmt::Display for ScalingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScalingError::BadInputs => write!(f, "scaling inputs non-finite or non-positive"),
            ScalingError::TooCoarse {
                tau_at_min_u,
                refine_factor,
            } => write!(
                f,
                "grid too coarse for this viscosity/speed: tau = {tau_at_min_u:.3} exceeds \
                 the stable window at the slowest usable lattice velocity; refine divisions \
                 by ~{refine_factor:.1}x"
            ),
            ScalingError::TooFast {
                tau_at_max_u,
                refine_factor,
            } => write!(
                f,
                "cell Reynolds number too high: tau = {tau_at_max_u:.4} is below the \
                 stability floor at the Mach limit; refine divisions by ~{refine_factor:.1}x"
            ),
        }
    }
}

impl std::error::Error for ScalingError {}

/// Stable τ window validated by the M0 ladder. The floor is a cell-
/// Reynolds stability bound, not numerology: at `u_lat = 0.05`,
/// τ = 0.52 keeps the mean cell Reynolds number `u_lat/ν_lat ≤ 7.5`
/// (peak ~15 for developed laminar profiles), which the ladder's duct
/// runs converge at; BGK at τ ≈ 0.51 was observed to diverge from
/// developed-profile peaks. MRT collision would relax this floor — a
/// flagged milestone option, not an M0 knob.
pub const TAU_MIN: f64 = 0.52;
/// Upper τ bound: beyond this the BGK truncation error dominates.
pub const TAU_MAX: f64 = 1.95;
/// Mach ceiling: compressibility error ~ Ma² ≈ 0.75% here.
pub const U_LAT_MAX: f64 = 0.05;
/// Floor on the lattice velocity: below this, steady convergence takes
/// impractically many steps for no accuracy gain.
pub const U_LAT_MIN: f64 = 0.002;

impl Scaling {
    /// Derive a scaling from voxel size (m), kinematic viscosity (m²/s),
    /// and reference speed (m/s).
    pub fn derive(dx_m: f64, nu_m2_s: f64, u_ref_m_s: f64) -> Result<Scaling, ScalingError> {
        if !(dx_m.is_finite() && nu_m2_s.is_finite() && u_ref_m_s.is_finite())
            || dx_m <= 0.0
            || nu_m2_s <= 0.0
            || u_ref_m_s <= 0.0
        {
            return Err(ScalingError::BadInputs);
        }
        // tau(u_lat) = 3 * nu * dt / dx^2 + 0.5, with dt = u_lat*dx/U:
        // tau = 3 * nu * u_lat / (U * dx) + 0.5 — monotone in u_lat.
        let tau_of = |u_lat: f64| 3.0 * nu_m2_s * u_lat / (u_ref_m_s * dx_m) + 0.5;
        let tau_hi = tau_of(U_LAT_MAX);
        let tau_lo = tau_of(U_LAT_MIN);
        // Prefer the largest Mach-safe u_lat; walk down if tau exceeds
        // the window.
        let u_lat = if tau_hi <= TAU_MAX {
            U_LAT_MAX
        } else if tau_lo <= TAU_MAX {
            // Solve tau_of(u) = TAU_MAX for u.
            (TAU_MAX - 0.5) * u_ref_m_s * dx_m / (3.0 * nu_m2_s)
        } else {
            return Err(ScalingError::TooCoarse {
                tau_at_min_u: tau_lo,
                refine_factor: (tau_lo - 0.5) / (TAU_MAX - 0.5),
            });
        };
        let tau = tau_of(u_lat);
        if tau < TAU_MIN {
            // Even the fastest allowed clock leaves tau at the floor:
            // the grid cannot resolve this cell Reynolds number.
            let needed = (TAU_MIN - 0.5) / (tau - 0.5);
            return Err(ScalingError::TooFast {
                tau_at_max_u: tau,
                refine_factor: needed.max(2.0),
            });
        }
        let dt_s = u_lat * dx_m / u_ref_m_s;
        Ok(Scaling {
            dx_m,
            dt_s,
            tau,
            u_lattice: u_lat,
            mach: u_lat / CS2.sqrt(),
        })
    }

    /// Convert a lattice velocity to m/s.
    pub fn velocity_to_si(&self, u_lat: f64) -> f64 {
        u_lat * self.dx_m / self.dt_s
    }

    /// Convert a physical velocity (m/s) to lattice units.
    pub fn velocity_to_lattice(&self, u_m_s: f64) -> f64 {
        u_m_s * self.dt_s / self.dx_m
    }

    /// Convert a lattice density deviation to a gauge pressure in Pa for
    /// a fluid of density ρ: `p = c_s²·(ρ_lat − 1)·ρ_phys·(dx/dt)²`.
    pub fn pressure_to_si(&self, rho_lat: f64, density_kg_m3: f64) -> f64 {
        let c = self.dx_m / self.dt_s;
        CS2 * (rho_lat - 1.0) * density_kg_m3 * c * c
    }

    /// Convert an acceleration (m/s²) to lattice units.
    pub fn accel_to_lattice(&self, a_m_s2: f64) -> f64 {
        a_m_s2 * self.dt_s * self.dt_s / self.dx_m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velocity_set_is_consistent() {
        let wsum: f64 = W.iter().sum();
        assert!((wsum - 1.0).abs() < 1e-15);
        for i in 0..Q {
            let (ci, copp) = (C[i], C[OPP[i]]);
            for (a, b) in ci.iter().zip(copp.iter()) {
                assert_eq!(*b, -a, "opposite of {i}");
            }
        }
        // First and second moments of the weights.
        for axis in [0usize, 1, 2] {
            let m1: f64 = (0..Q).map(|i| W[i] * C[i][axis] as f64).sum();
            assert!(m1.abs() < 1e-15);
            let m2: f64 = (0..Q)
                .map(|i| W[i] * (C[i][axis] * C[i][axis]) as f64)
                .sum();
            assert!((m2 - CS2).abs() < 1e-15);
        }
    }

    #[test]
    fn equilibrium_conserves_moments() {
        let u = [0.03, -0.01, 0.02];
        let rho = 1.02;
        let feq = equilibrium(rho, u);
        let m0: f64 = feq.iter().sum();
        assert!((m0 - rho).abs() < 1e-12);
        for a in 0..3 {
            let m1: f64 = (0..Q).map(|i| feq[i] * C[i][a] as f64).sum();
            assert!((m1 - rho * u[a]).abs() < 1e-12, "axis {a}");
        }
    }

    #[test]
    fn scaling_air_typical_duct() {
        // 1 mm voxels, air, 0.1 m/s (faster flows at this resolution sit
        // below the tau floor and are correctly refused — see
        // scaling_refuses_unresolvable).
        let s = Scaling::derive(1e-3, 1.516e-5, 0.1).unwrap();
        assert!(s.tau >= TAU_MIN && s.tau <= TAU_MAX, "tau = {}", s.tau);
        assert!(s.u_lattice <= U_LAT_MAX + 1e-12);
        // Round trip.
        let u = s.velocity_to_si(s.velocity_to_lattice(0.37));
        assert!((u - 0.37).abs() < 1e-12);
    }

    #[test]
    fn scaling_refuses_unresolvable() {
        // Honey-viscosity fluid at high speed on a 1 mm grid: tau blows
        // past the window even at the slowest clock -> refuse.
        assert!(Scaling::derive(1e-4, 1e-3, 0.01).is_err());
        // Cell Reynolds too high: air at 10 m/s on a 1 mm grid sits
        // below the tau floor at the Mach limit -> refuse.
        assert!(matches!(
            Scaling::derive(1e-3, 1.516e-5, 10.0),
            Err(ScalingError::TooFast { .. })
        ));
    }
}
