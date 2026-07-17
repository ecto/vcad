#![warn(missing_docs)]

//! Monte Carlo neutron transport for shielding and dosimetry design (M0).
//!
//! Answers the question every neutron-producing bench experiment must
//! answer before first beam: **what is the dose rate at the operator's
//! chair, and how much moderator makes it acceptable?** First customer:
//! the shielded-grid IEC experiment's Phase B shield
//! (`docs/shielded-grid-experiment.md`) — an isotropic 2.45 MeV D-D point
//! source behind layered HDPE/borated-poly. Second: neutron-activation
//! feasibility (thermal flux available at a sample position).
//!
//! The pipeline:
//!
//! 1. [`materials`] — a small bundled multigroup library (HDPE, borated
//!    poly, water, lead, concrete, air) with per-value literature
//!    citations. This is a **design-estimate library, not an evaluated
//!    nuclear data file** — see the module docs for exactly what that
//!    means.
//! 2. [`geometry::Geometry`] — 1D slab or 1D spherical layered shields,
//!    thicknesses in millimeters (vcad convention).
//! 3. [`transport::run`] — analog fixed-source Monte Carlo: exponential
//!    flight sampling, isotropic elastic scatter with per-material
//!    group-transfer matrices, absorption, group downscatter.
//! 4. [`tally`] — track-length flux per group per region, surface
//!    currents, leakage. **Every tally is an [`tally::Estimate`]:
//!    mean ± relative standard error from batch statistics. A result
//!    without an error bar is unrepresentable in this API.**
//! 5. [`dose`] — ambient dose equivalent H*(10) via bundled ICRP-74-style
//!    flux-to-dose factors.
//!
//! Determinism: the bundled [`rng`] (xoshiro256++) is seeded explicitly;
//! the same [`transport::RunConfig`] reproduces bit-identical results.
//!
//! **Scope and refusals.** This crate models *moderation, shielding, and
//! dose* from fixed external neutron sources. It deliberately contains no
//! fission physics: no fission cross sections, no neutron multiplication,
//! no criticality search, and it will not grow them. Requests in that
//! direction are out of scope for vcad — the legitimate uses (reactor
//! design) are served by regulated, export-controlled codes and their
//! institutional review chains, which an open CAD kernel neither can nor
//! should replace. What an open kernel *can* responsibly do — and what the
//! incumbent tools make needlessly painful for hobbyists and small labs —
//! is benign shielding design with honest error bars. That is the whole
//! scope. Further honesty bounds (M0): no photon transport (capture
//! gammas are flagged as a caveat, never silently dropped), free-atom
//! scattering except a documented bound-hydrogen thermal adjustment,
//! isotropic lab-frame scattering (P1 anisotropy is the M1 rung).
//!
//! Units: geometry is **millimeters** (vcad convention); cross sections
//! are internal cm⁻¹; energies are eV; doses are reported in µSv/h.

pub mod dose;
pub mod geometry;
pub mod groups;
pub mod materials;
pub mod rng;
pub mod scatter;
pub mod tally;
pub mod transport;

/// Physical constants.
pub mod constants {
    /// Avogadro's number, 1/mol (CODATA 2018 exact).
    pub const AVOGADRO: f64 = 6.022_140_76e23;
    /// D-D fusion neutron energy, eV. D(d,n)³He at low beam energy emits
    /// ~2.45 MeV neutrons (Bosch & Hale 1992 kinematics).
    pub const DD_NEUTRON_EV: f64 = 2.45e6;
    /// Thermal reference energy (2200 m/s), eV.
    pub const THERMAL_EV: f64 = 0.0253;
}
