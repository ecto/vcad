#![warn(missing_docs)]

//! Charged-particle optics for the vcad kernel (M0).
//!
//! Models axisymmetric electrostatic devices — fusors, shielded-grid IEC
//! machines, ion traps, electron guns — as **vacuum-field single-particle
//! optics**: solve the electrode potential, trace charged particles through
//! it, and score the electrode geometry with figures of merit.
//!
//! The pipeline:
//!
//! 1. [`device::Device`] — electrodes as wire rings + a grounded chamber,
//!    geometry in millimeters (vcad convention), potentials in volts,
//!    optional ampere-turns per ring for magnetic self-shielding.
//! 2. [`poisson::solve`] — axisymmetric Laplace solver (finite difference,
//!    SOR) for the potential φ(r, z) and field E = −∇φ.
//! 3. [`field::FieldMap`] — E from the grid + analytic B from every
//!    current-carrying ring (complete elliptic integrals, exact off-axis).
//! 4. [`trace`] — Boris pusher with adaptive substepping; particles
//!    terminate on electrode or wall impact, core passes are counted.
//! 5. [`fom`] — ensemble statistics: interception fraction, mean
//!    recirculation count, effective grid transparency.
//! 6. [`optimize`] — derivative-free/finite-difference maximization of any
//!    figure of merit over electrode parameters (M0 stand-in for the
//!    adjoint; see `docs/particle-optics-m0.md`).
//!
//! **Scope and honesty (M0):** no space charge, no collisions, no neutral
//! gas — the regime where electrode geometry dominates and where every
//! classic electrode-design tool (SIMION lineage) operates. Self-consistent
//! plasma effects are explicitly out of scope until a later milestone; see
//! the milestone ladder in `docs/particle-optics-m0.md`.
//!
//! Units: public geometry is **millimeters** (vcad convention); everything
//! internal is SI (meters, volts, tesla, seconds); particle energies are
//! reported in electron-volts.

pub mod device;
pub mod elliptic;
pub mod field;
pub mod fom;
pub mod optimize;
pub mod poisson;
pub mod trace;

/// Physical constants (SI).
pub mod constants {
    /// Elementary charge, C.
    pub const ELEMENTARY_CHARGE: f64 = 1.602_176_634e-19;
    /// Deuteron mass, kg.
    pub const DEUTERON_MASS: f64 = 3.343_583_772e-27;
    /// Electron mass, kg.
    pub const ELECTRON_MASS: f64 = 9.109_383_701_5e-31;
    /// Vacuum permeability, T·m/A.
    pub const MU_0: f64 = 1.256_637_062_12e-6;
}
