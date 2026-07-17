#![warn(missing_docs)]

//! Dimensional tolerance stackup analysis for the vcad kernel (M0).
//!
//! Answers the question every assembly drawing begs and almost no tool
//! answers honestly: **"does this fit, and at what yield?"** The
//! incumbent workflow is a spreadsheet and prayer; this crate is a
//! deterministic, receipt-native stackup engine that lives next to the
//! geometry it prices.
//!
//! The pipeline:
//!
//! 1. [`stackup::Stackup`] — a linear chain of dimensional
//!    contributors, each with a nominal, a signed coefficient, drawing
//!    limits, and a deviation distribution
//!    ([`dist::Distribution`]: normal, uniform, or two-point) whose
//!    provenance ([`dist::DistributionSource`]) records whether it is
//!    an assumption or a measurement.
//! 2. [`analysis`] — the three classic analyses over the chain:
//!    worst-case (interval arithmetic over drawing limits), RSS (exact
//!    linear variance propagation), and seeded Monte Carlo (hand-rolled
//!    xoshiro256++, batch statistics, and a standard error on every
//!    probability — error-bar-free probabilities are unrepresentable).
//! 3. [`capability`] — predicted gap distribution vs the requirement:
//!    Φ-based yield (hand-rolled erf, A&S 7.1.26, max error 1.5e-7),
//!    Cp and Cpk.
//! 4. [`sensitivity`] — the design compass: **exact closed-form**
//!    derivatives of the gap, σ_gap, and yield with respect to every
//!    nominal and every σ. Linear chains make these exact — no
//!    adjoint machinery, no finite differences, and we say so proudly.
//! 5. [`loops`] — 2D/3D vector loops projected onto a measure
//!    direction: exactly linear for translational legs, first-order
//!    (small-angle, bound stated) for rotations.
//!
//! Later milestones stack on this base (see `docs/tolerance-m0.md` for
//! the ladder): GD&T semantics (position at MMC with bonus, M1),
//! cost-based tolerance allocation (M2), the `.vcad` named-parameter
//! seam (M3), `vcad.tolerance-claims/1` receipt claims (M4), published
//! benchmarks + paper draft (M5), and the measurement pack binding the
//! 3DP print-then-measure loop (M6).
//!
//! **Scope and honesty (M0):**
//!
//! - **Linear chains.** The gap is G = Σ aᵢxᵢ. Exact for 1-D stacks;
//!   vector loops are linearized with a stated small-angle bound.
//!   Genuinely nonlinear mechanisms (radial fits are the everyday
//!   case — see `tests/bolt_circle.rs`) get the exact treatment in the
//!   GD&T module or a Monte Carlo over the true model, not a silent
//!   linearization.
//! - **Independence.** Contributors are assumed statistically
//!   independent. Two dimensions cut in one fixture setup are not, and
//!   both RSS and Monte Carlo will be wrong about their sum. Correlation
//!   modeling is future work.
//! - **The ±tol ↔ σ convention is an assumption** — the default
//!   ±tol = 3σ ([`dist::SigmaConvention`]) buries more products than
//!   any solver bug, so it is recorded as provenance on every receipt,
//!   never defaulted silently.
//! - **Distributions are assumptions until measured.** Every
//!   contributor carries its [`dist::DistributionSource`]; the M6
//!   measurement pack replaces assumptions with fitted coupon data.
//!
//! Units: millimeters throughout (vcad convention); probabilities are
//! dimensionless in [0, 1]; angles enter rotation legs in radians.

pub mod allocate;
pub mod analysis;
pub mod capability;
pub mod dist;
pub mod gdt;
pub mod loops;
pub mod measure;
pub mod receipt;
pub mod rng;
pub mod sensitivity;
pub mod spec;
pub mod stackup;
