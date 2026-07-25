//! Unit system for the atomic domain.
//!
//! We use a self-consistent "metal-like" unit system:
//! - length: Ångström (Å)
//! - energy: electron-volt (eV)
//! - mass: atomic mass unit (amu)
//! - time: femtosecond (fs)
//! - charge: elementary charge (e)
//! - temperature: kelvin (K)
//!
//! The only non-trivial conversion is turning a force in eV/Å acting on a mass
//! in amu into an acceleration in Å/fs². [`FORCE_TO_ACCEL`] is that factor:
//! `a[Å/fs²] = FORCE_TO_ACCEL * F[eV/Å] / m[amu]`. Kinetic energy in eV is then
//! `0.5 * m * v² / FORCE_TO_ACCEL`, which keeps energy conservation exact under
//! this convention.
//!
//! The constants are re-exported from [`phyz_md::field::units`] so this crate
//! and the phyz-md engine it delegates to share one set of values.

pub use phyz_md::field::units::{FORCE_TO_ACCEL, KB_EV_PER_K, KE_COULOMB};
