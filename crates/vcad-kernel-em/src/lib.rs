#![warn(missing_docs)]

//! Electromagnetic field solver for the vcad kernel (M0).
//!
//! Replaces formula-grade EM estimates (Wheeler coils, reluctance
//! networks, first-order motor constants) with **solved fields from the
//! actual geometry** — the role FEMM has played since 2004, rebuilt as a
//! differentiable, receipt-native library inside the kernel.
//!
//! The pipeline:
//!
//! 1. Describe the device: rectangular coil cross-sections, permanent
//!    magnets, linear-μ/ε regions — in millimeters (vcad convention).
//! 2. Solve: every M0 formulation is the same divergence-form elliptic
//!    problem `∇·(c∇u) = −s` on the shared symmetric finite-volume core
//!    ([`grid`]), by SOR with scale-invariant stopping.
//!    - [`axisym`] — axisymmetric magnetostatics on the flux function
//!      `ψ = r·A_θ` (coils of revolution: solenoids, coaxial coil pairs).
//!    - [`planar`] — planar magnetostatics on `A_z` (motor cross-sections,
//!      unrolled airgaps; permanent magnets as bound-current sheets).
//!    - [`electro`] — electrostatics on `φ`, both geometries.
//! 3. Extract quantities of interest, each by **two independent routes**
//!    whose agreement is part of the result: inductance (energy vs flux
//!    linkage), capacitance (energy vs induced charge), force and torque
//!    (`J×B` volume integral vs Maxwell stress on a closed surface).
//! 4. Validate: the ladder in each module and `tests/validation.rs` runs
//!    against exact anchors (infinite solenoid, coax, series dielectrics),
//!    published closed forms (Wheeler 1928, Maxwell's mutual-inductance
//!    formula), and an **independent codebase**
//!    (`vcad_kernel_particle::field::b_ring`, elliptic-integral loop
//!    fields written for the particle-optics crate).
//!
//! **Scope and honesty (M0):** linear materials only (constant μ_r, ε_r —
//! saturation and B–H curves are M1); statics only (no eddy currents, no
//! skin effect — AC phasors are M1); smooth fields on staircased material
//! boundaries (first-order interface accuracy — convergence studies must
//! bracket any claimed number); finite `u = 0` truncation boundaries
//! underestimate far-reaching return flux unless placed far or replaced
//! by the symmetry the problem actually has (Neumann). Planar motor
//! models ignore curvature and radial end effects. Grid masks floor thin
//! features: a conductor thinner than a cell is smeared, exactly like the
//! particle crate's wire-mask floor.
//!
//! Units: geometry in **millimeters**, everything internal SI; field
//! sampling accessors take SI meters and return tesla / volts per meter.
//! Planar quantities are per meter of depth.

pub mod ac;
pub mod analytic;
pub mod axisym;
pub mod electro;
pub mod grid;
pub mod material;
pub mod planar;

/// Physical constants (SI; CODATA 2018).
pub mod constants {
    /// Vacuum permeability μ₀, T·m/A.
    pub const MU_0: f64 = 1.256_637_062_12e-6;
    /// Vacuum permittivity ε₀, F/m.
    pub const EPS_0: f64 = 8.854_187_812_8e-12;
}
