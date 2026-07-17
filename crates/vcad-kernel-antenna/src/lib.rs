#![warn(missing_docs)]

//! Thin-wire method-of-moments antenna solver for the vcad kernel (M0).
//!
//! Predicts **input impedance, S11, and far-field gain from actual wire
//! geometry** — the quantities an antenna design lives or dies by. The
//! incumbent for this loop is NEC-2: 1981 Fortran, punched-card input
//! decks, no gradients, no connection to the CAD geometry or the receipt
//! system. Same shape of incumbent `vcad-kernel-particle` replaced for
//! charged-particle optics; this crate is the antenna rung of the same
//! ladder, with the cheapest hardware-validation loop in the portfolio (a
//! PCB antenna through the existing gerber pipeline + a ~$100 NanoVNA).
//!
//! The pipeline:
//!
//! 1. [`geometry::WireGrid`] — wires, paths, and loops in millimeters
//!    (vcad convention); [`geometry::Mesh::build`] compiles to straight
//!    segments with **triangular current bases** on interior nodes.
//! 2. [`mom`] — Galerkin fill of the thin-wire EFIE impedance matrix
//!    (mixed-potential form, singularity-extracted quadrature), delta-gap
//!    drive, LU solve: `Z_in(f)`, S11 vs any reference, frequency sweeps,
//!    resonance search.
//! 3. [`farfield`] — radiation integral over the solved currents: gain
//!    pattern, directivity, radiated power, and the **energy-balance
//!    cross-check** (radiated ≈ accepted power for lossless wire).
//!
//! **Scope and honesty (M0):** perfectly conducting thin wires in free
//! space. No dielectrics, no ground plane (M1: image theory), no ohmic
//! loss, no substrate — a PCB-trace antenna prediction at M0 is
//! **first-order only** until the effective-permittivity correction lands
//! (flagged M1.5): FR-4 shifts resonance down by roughly `1/√ε_eff` and
//! M0 will not pretend otherwise. Every validity boundary of the thin-wire
//! kernel is a **hard error** ([`error::AntennaError`]), never a silent
//! degradation.
//!
//! Validation ladder (in `tests/` and module tests): half-wave dipole
//! impedance and resonance against Balanis, 2.15 dBi dipole directivity,
//! small-loop radiation-resistance ∝ (C/λ)⁴ scaling, segment-refinement
//! convergence with a named floor, machine-precision reciprocity, and
//! far-field/feed-power energy balance.
//!
//! Units: public geometry is **millimeters** (vcad convention); everything
//! internal is SI (meters, hertz, ohms, watts). Time convention `e^{+jωt}`.

pub mod complex;
pub mod error;
pub mod farfield;
pub mod geometry;
pub mod linalg;
pub mod mom;
pub mod quad;

pub use complex::Complex;
pub use error::AntennaError;
pub use geometry::{Mesh, WireGrid};
pub use mom::{find_resonance, s11, s11_db, solve_driven, sweep, DrivenSolution, SolveOptions};

/// Physical constants (SI).
pub mod constants {
    /// Speed of light in vacuum, m/s (exact).
    pub const C0: f64 = 299_792_458.0;
    /// Vacuum permeability, T·m/A.
    pub const MU_0: f64 = 1.256_637_062_12e-6;
    /// Free-space wave impedance `μ₀ c`, Ω.
    pub const ETA_0: f64 = MU_0 * C0;
    /// Vacuum permittivity `1/(μ₀ c²)`, F/m.
    pub const EPSILON_0: f64 = 1.0 / (MU_0 * C0 * C0);
}

#[cfg(test)]
mod tests {
    #[test]
    fn constants_are_consistent() {
        // η₀ = √(μ₀/ε₀) ≈ 376.73 Ω
        let eta = (super::constants::MU_0 / super::constants::EPSILON_0).sqrt();
        assert!((eta - super::constants::ETA_0).abs() < 1e-9);
        assert!((super::constants::ETA_0 - 376.730).abs() < 1e-2);
    }
}
