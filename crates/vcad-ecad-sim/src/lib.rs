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
//! - [`impedance`] — Characteristic impedance for microstrip and stripline geometries
//! - [`signal_integrity`] — Propagation delay, crosstalk estimation, length matching
//! - [`thermal`] — Component junction temperature and via thermal resistance

pub mod impedance;
pub mod signal_integrity;
pub mod thermal;

pub use impedance::{
    diff_microstrip_impedance, diff_stripline_impedance, microstrip_impedance, stripline_impedance,
    ImpedanceResult,
};
pub use signal_integrity::{
    estimate_crosstalk, length_matching, propagation_delay, CrosstalkResult,
};
pub use thermal::{analyze_thermal, via_thermal_resistance, ComponentThermal, ThermalResult};
