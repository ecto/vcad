#![warn(missing_docs)]

//! Air-side acoustics for the vcad kernel (M0).
//!
//! Where `vcad-kernel-particle` scores electrode geometry and the
//! `simulate_strike` tool scores a vibrating solid bar, this crate scores the
//! **air**: the pressure field inside cavities, ports, horns and boxes. It is
//! the missing half of the acoustics loop the workspace already proved once
//! with the glockenspiel (structural modal analysis + a microphone, verified
//! to −5 cents). The domain — loudspeaker enclosures, Helmholtz resonators,
//! ducts — is where hobby and pro audio tooling is a scatter of one-off
//! calculators and the "serious" options are 1990s freeware (Hornresp,
//! BassBox) or five-figure FEA. The ground truth here is cheap and unarguable:
//! closed-form resonances and a $20 measurement microphone.
//!
//! The pipeline:
//!
//! 1. [`medium::Medium`] — air (or any fluid): sound speed and density.
//! 2. [`cavity::Cavity`] — the axisymmetric domain as a coaxial stack of
//!    cylinders (closed cylinder, Helmholtz resonator, ported box), geometry
//!    in millimeters.
//! 3. [`helmholtz::solve_driven`] — the driven Helmholtz field `(∇²+k²)p =
//!    −jωρ s` on an (r, z) grid, finite-volume (conservative, symmetric →
//!    reciprocal), direct block-Thomas solve (the operator is indefinite —
//!    relaxation would diverge).
//! 4. [`sweep`] — swept-sine in silico: resonances as response peaks.
//! 5. [`lumped`] — the closed-form spine (duct mass, cavity compliance,
//!    Helmholtz/bass-reflex tuning) — both a feature and the field solver's
//!    oracle.
//! 6. [`radiation`] — the baffled piston (Rayleigh integral + on-axis closed
//!    form): analytic directivity to check against.
//! 7. [`fom`] — figures of merit: tuning, mode shapes, port velocity,
//!    on-axis response.
//! 8. [`optimize`] — box-constrained search that sizes a port for a target
//!    tuning (the `vcad-kernel-particle` optimizer pattern).
//! 9. [`spec`] / [`receipt`] — the `.vcad` parameter seam and the
//!    `vcad.acoustics-claims/1` predicted-performance receipt.
//!
//! **The seam to `simulate_strike` (structural ↔ air).** `simulate_strike`
//! computes how a *solid* vibrates (Euler–Bernoulli / Hermite beam modes) and
//! synthesizes what you'd hear by treating each mode as a lone radiator with a
//! damping heuristic. This crate computes the *air* physics that heuristic
//! stands in for. The coupling is a boundary condition: the structural
//! solver's mode shape is a **surface normal velocity** `v_n(x)`, which is
//! exactly the Neumann datum the air-side solve consumes
//! ([`helmholtz::Source::Piston`] is the rigid-piston special case). Surface
//! velocity in, pressure field out. Wiring the two together (a vibrating bar
//! or cone radiating into a modelled room) is **M2**; M0 states the seam and
//! keeps the two solvers independent.
//!
//! **Scope and honesty (M0):** linear, lossless acoustics — no thermoviscous
//! or radiation damping, so resonator/port **Q reads optimistic** and every
//! claim says so; no structural coupling yet (the seam above is M2);
//! interior fields and lumped tuning are solved on the grid, while open-domain
//! radiation (the baffled piston) is analytic — a radiating field boundary
//! (PML) is a later milestone. See `docs/acoustics-m0.md`.
//!
//! Units: public geometry is **millimeters** (vcad convention); the medium
//! and everything internal is SI (meters, pascals, seconds); the complex
//! pressure phasor uses the `e^{+jωt}` convention.

pub mod cavity;
pub mod complex;
pub mod fom;
pub mod helmholtz;
pub mod linalg;
pub mod lumped;
pub mod medium;
pub mod optimize;
pub mod radiation;
pub mod receipt;
pub mod spec;
pub mod strike;
pub mod sweep;

pub use cavity::{Cavity, EndCondition, Segment};
pub use complex::Cplx;
pub use medium::Medium;
