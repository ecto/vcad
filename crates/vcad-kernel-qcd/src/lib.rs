#![warn(missing_docs)]

//! Lattice gauge theory for the vcad solver zoo (M0).
//!
//! Computes **confinement from first principles**: SU(2) pure-gauge
//! (quenched) Wilson-action Monte Carlo on a 4D periodic hypercubic
//! lattice, small enough to run on a laptop, honest enough to carry
//! error bars on every number. This is the pure-glue sector where the
//! canonical lattice results live — plaquette expectation values,
//! Wilson loops, the area law, and (via Creutz ratios) the string
//! tension that makes quark separation cost linear energy.
//!
//! The pipeline:
//!
//! 1. [`su2`] — SU(2) group elements in the quaternion parameterization
//!    `U = a₀·1 + i a·σ` (exact unitarity by construction, renormalized
//!    against float drift).
//! 2. [`lattice`] — link variables `U_μ(x)` on a periodic 4D lattice,
//!    staple sums, plaquette and planar Wilson-loop observables.
//! 3. [`update`] — Kennedy–Pendleton heatbath plus overrelaxation
//!    sweeps (Creutz 1980; Kennedy & Pendleton 1985). Deterministic:
//!    the bundled xoshiro256++ [`rng`] is seeded explicitly, so the
//!    same [`spec::SimSpec`] reproduces bit-identical results.
//! 4. [`stats`] — binned jackknife means and errors. **Every reported
//!    observable is a [`stats::Estimate`]: mean ± error. A number
//!    without an error bar is unrepresentable in this API.**
//! 5. [`spec`] — the serde-friendly simulation spec → run → result
//!    seam (the future MCP surface).
//! 6. [`receipt`] — `vcad.qcd-claims/1` predicted claims, fail-closed:
//!    unthermalized or statistics-starved runs cannot mint claims, and
//!    every claim carries the quenched/finite-volume/finite-spacing
//!    caveat list in the same JSON object.
//!
//! Validation oracles (in `#[cfg(test)]`): the strong-coupling
//! expansion ⟨P⟩ = β/4 − β³/96 + O(β⁵) and the weak-coupling
//! expansion ⟨P⟩ = 1 − 3/(4β) + O(1/β²) for SU(2) in 4D, plus
//! W(1,1) ≡ ⟨P⟩ consistency and the area-law ordering of Wilson
//! loops in the confined phase.
//!
//! **Honesty bounds (M0).** Quenched SU(2) only — no dynamical
//! fermions, no SU(3), no hadron masses. All observables are in
//! lattice units at fixed coupling: no continuum extrapolation and no
//! scale setting, so nothing here is a claim about physical QCD —
//! the claims are about the lattice model, stated as such. Claims are
//! `basis: predicted` and cap at Provisional in the receipt
//! vocabulary; the ladder to M1+ (SU(3), Creutz-ratio string-tension
//! scaling, gradient flow, flux-tube visualization seam) is
//! `docs/qcd-m0.md`.

pub mod lattice;
pub mod receipt;
pub mod rng;
pub mod spec;
pub mod stats;
pub mod su2;
pub mod update;
