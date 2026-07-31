//! Atomic & molecular design and simulation for vcad.
//!
//! This crate is the atomic-domain analog of `vcad-kernel-physics`: it reads the
//! optional [`vcad_ir::molecule::MoleculeSystem`] on a document and provides
//! structure I/O, classical molecular dynamics and energy minimization, an ML
//! interatomic-potential adapter seam, a differentiable inverse-design loop, a
//! gym-style environment mirroring the physics `RobotEnv`, and reproducibility
//! receipts.
//!
//! Units are Ångström / eV / amu / fs / e / K throughout (see [`units`]) — the
//! molecular domain keeps its own convention and never routes through the
//! millimeter-oriented CAD converters.
//!
//! ## Layers
//! - [`system::AtomSystem`] — the simulator's working state, built from the IR.
//! - [`potential`] — composable force-field terms behind the [`potential::ForceField`] trait.
//! - [`integrate`] / [`minimize`](mod@minimize) — velocity-Verlet dynamics and FIRE relaxation.
//! - [`fd`] — the finite-difference oracle every force term is validated against.
//! - [`mlip`] — ML-potential adapter (the near-DFT force engine seam).
//! - [`inverse`] — property-target inverse design.
//! - [`gym`] — reset/step/observe environment.
//! - [`receipt`] — reproducible simulation records.

#![warn(missing_docs)]
// The force/integration loops index several parallel per-atom arrays
// (positions, velocities, masses, forces) by the same index; explicit range
// indexing reads more clearly than nested zips for this numeric code.
#![allow(clippy::needless_range_loop)]

pub mod builder;
pub mod element;
pub mod fd;
pub mod gym;
pub mod homogenize;
pub mod inspect;
pub mod integrate;
pub mod inverse;
pub mod io;
pub mod minimize;
pub mod mlip;
pub mod potential;
pub mod receipt;
pub mod system;
pub mod units;
pub mod vec3;

#[cfg(feature = "wasm")]
pub mod wasm;

pub use gym::{MdEnv, MdObservation};
pub use homogenize::{homogenize, HomogenizeOptions, MaterialCard};
pub use inspect::{report, MoleculeReport};
pub use integrate::{Integrator, Thermostat};
pub use minimize::{minimize, MinimizeOptions, MinimizeResult};
pub use potential::{Coulomb, ForceField, HarmonicAngles, HarmonicBonds, LennardJones, Sum};
pub use receipt::SimReceipt;
pub use system::AtomSystem;

// Re-export the IR molecule types so consumers get them from one place.
pub use vcad_ir::molecule::{Bond, Cell, MoleculeSystem, Species};
