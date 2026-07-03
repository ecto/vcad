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

/// Convert `eV/Å / amu` to `Å/fs²`.
///
/// Derived from `1 eV/Å = 1.602176634e-9 N`, `1 amu = 1.66053906660e-27 kg`,
/// and `1 m/s² = 1e-20 Å/fs²`.
pub const FORCE_TO_ACCEL: f64 = 9.648_533_212_331_e-3;

/// Boltzmann constant in eV/K.
pub const KB_EV_PER_K: f64 = 8.617_333_262e-5;

/// Coulomb constant `1/(4πε₀)` in eV·Å/e² (so `E = KE_COULOMB q_i q_j / r`).
pub const KE_COULOMB: f64 = 14.399_645_351_950_54;
