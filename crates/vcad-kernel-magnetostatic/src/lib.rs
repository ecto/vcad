#![warn(missing_docs)]

//! Grid-free 3D magnetostatics for **air-core** machines.
//!
//! This is the exact oracle behind air-core motor claims: given the actual coil
//! copper and rotor magnets, it computes flux linkage, back-EMF and torque
//! constants, inductance, and the torque waveform — with no closed-form fudge
//! factors and no fitted coefficients.
//!
//! # Why there is no grid
//!
//! An air-core machine has no iron. No iron means no `B`–`H` curve, so `μ = μ₀`
//! everywhere, the problem is **linear**, and superposition holds exactly. That
//! collapses the usual finite-element machinery into a sum over current
//! segments ([`filament`]): closed-form Biot-Savart for `B`, closed-form
//! `∮A·dl` for flux linkage. No mesh, no iteration, no convergence study, and no
//! truncation boundary to place — the three things that make
//! [`vcad_kernel_em`](https://docs.rs/vcad-kernel-em)'s grid solvers expensive
//! and approximate are simply absent.
//!
//! The price is that this crate is **only** valid without ferromagnetic
//! material. Anything with a steel back-iron, a slotted stator, or a saturable
//! pole must go to the grid solver instead.
//!
//! # Scope and honesty
//!
//! What this crate models exactly (to discretization):
//!
//! - Fields of arbitrary 3D conductor paths, including real spiral PCB copper.
//! - Permanent magnets as **equivalent surface currents** `K = M × n̂`, which is
//!   exact for uniform magnetization in a linear medium with `μ_rec = 1` — the
//!   right model for sintered ferrite and NdFeB, whose recoil permeability is
//!   1.05–1.1. That 5–10% is the model's dominant magnet error.
//! - Torque by two independent routes (Lorentz `∮I dl × B` and the co-energy
//!   derivative `dW/dθ`), whose agreement is reported rather than assumed.
//!
//! What it does **not** model:
//!
//! - Any ferromagnetic material — see above. A steel back-iron behind the
//!   magnets, which most axial-flux rotors have, roughly doubles airgap flux and
//!   is **not** captured. Model it in the grid solver, or treat this crate's
//!   result as the no-back-iron lower bound.
//! - Eddy currents, skin and proximity effect: statics only. DC resistance and
//!   low-frequency inductance are in scope; AC losses are not.
//! - Self-inductance is filamentary, so it depends on the conductor radius used
//!   to regularize the on-axis singularity. Rectangular PCB traces have no
//!   single "radius"; we use the geometric-mean-distance equivalent, and the
//!   residual error on `L` is larger than on `Kt` — see [`machine`].
//! - Temperature. `Kt` shifts with magnet remanence (≈ −0.2%/K for NdFeB,
//!   ≈ −0.2%/K for ferrite) and `R` with copper resistivity (+0.39%/K).
//!   Callers supply the operating temperature; nothing here guesses it.
//!
//! # Cogging
//!
//! A true air-core machine has **no cogging torque**, because cogging is the
//! magnets detenting against ferromagnetic structure and there is none. This
//! crate therefore returns zero cogging *by construction*, not as a computed
//! result — do not read it as a validated prediction. What *is* computed is
//! torque ripple under load, which for an air-core machine comes from winding
//! and magnet-field harmonics alone; see [`kpi`].
//!
//! # Units
//!
//! The public machine description is in **millimetres** (vcad convention);
//! everything internal is SI. Field accessors take and return SI.

pub mod filament;
pub mod iron;
pub mod vec3;

pub use filament::{Filament, Segment};
pub use iron::IronStack;
pub use vec3::Vec3;

/// Vacuum permeability, H/m (CODATA 2018).
pub const MU_0: f64 = 1.256_637_062_12e-6;

/// Copper resistivity at 20 °C, Ω·m.
pub const RHO_CU_20C: f64 = 1.68e-8;

/// Copper temperature coefficient of resistivity, 1/K.
pub const ALPHA_CU: f64 = 0.00393;

/// Millimetres to metres.
#[inline]
pub const fn mm(v: f64) -> f64 {
    v * 1e-3
}
