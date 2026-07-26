#![warn(missing_docs)]

//! PCB simulation for vcad: impedance calculation, signal integrity analysis,
//! and thermal analysis.
//!
//! This crate provides closed-form electromagnetic and thermal models for
//! printed circuit board design, following IPC-2141 and standard PCB
//! engineering formulas.
//!
//! # Modules
//!
//! The electromagnetic solvers are grouped under the [`em`] domain facade
//! (see its module docs for the claim family and domain framing):
//!
//! - [`airgap`] — First-order air-gap flux density via a reluctance network (MEC)
//! - [`impedance`] — Characteristic impedance for microstrip and stripline geometries
//! - [`induction`] — Thin-sheet axial induction machine (drag-cup / PCB-cage rotors)
//! - [`magnetics`] — Scalar-generic spiral inductance + motor torque constant
//! - [`motor`] — Analytical PM motor performance (Kt/Ke, no-load speed, stall torque, curve)
//! - [`signal_integrity`] — Propagation delay, crosstalk estimation, length matching
//!
//! Neighbors outside the EM domain:
//!
//! - [`circuit`] — Lumped-element transient circuit simulation (MNA network solver)
//! - [`thermal`] — Component junction temperature and via thermal resistance

pub mod airgap;
pub mod circuit;
pub mod em;
pub mod impedance;
pub mod induction;
pub mod magnetics;
pub mod motor;
pub mod signal_integrity;
pub mod thermal;

pub use airgap::{
    aircored_airgap_flux_density, airgap_flux_density, airgap_solve, fringing_derate,
    AirGapSolution, AirGapSpec, TeethSpec, SILICON_STEEL_KNEE_T,
};
pub use impedance::{
    diff_microstrip_impedance, diff_stripline_impedance, microstrip_impedance, stripline_impedance,
    ImpedanceResult,
};
pub use induction::{
    evaluate_thin_sheet_induction, ThinSheetInductionPerformance, ThinSheetInductionSpec,
};
pub use motor::{evaluate_motor, MotorPerformance, MotorSpec, OperatingPoint};
pub use signal_integrity::{
    estimate_crosstalk, length_matching, propagation_delay, CrosstalkResult,
};
pub use thermal::{analyze_thermal, via_thermal_resistance, ComponentThermal, ThermalResult};
