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
//! - [`airgap`] — First-order air-gap flux density via a reluctance network (MEC)
//! - [`circuit`] — Lumped-element transient circuit simulation (MNA network solver)
//! - [`impedance`] — Characteristic impedance for microstrip and stripline geometries
//! - [`magnetics`] — Scalar-generic spiral inductance + motor torque constant
//! - [`motor`] — Analytical motor performance (Kt/Ke, no-load speed, stall torque, curve)
//! - [`signal_integrity`] — Propagation delay, crosstalk estimation, length matching
//! - [`thermal`] — Component junction temperature and via thermal resistance

pub mod airgap;
pub mod circuit;
pub mod impedance;
pub mod magnetics;
pub mod motor;
pub mod signal_integrity;
pub mod thermal;

pub use airgap::{aircored_airgap_flux_density, airgap_flux_density, AirGapSpec};
pub use impedance::{
    diff_microstrip_impedance, diff_stripline_impedance, microstrip_impedance, stripline_impedance,
    ImpedanceResult,
};
pub use motor::{evaluate_motor, MotorPerformance, MotorSpec, OperatingPoint};
pub use signal_integrity::{
    estimate_crosstalk, length_matching, propagation_delay, CrosstalkResult,
};
pub use thermal::{analyze_thermal, via_thermal_resistance, ComponentThermal, ThermalResult};
