#![warn(missing_docs)]

//! 2D FDTD electromagnetics for photonics (M0).
//!
//! Models planar photonic devices — waveguides, splitters, ring resonators,
//! grating couplers — as **2D time-domain Maxwell problems** on a Yee grid,
//! with absorbing boundaries, eigenmode sources, and spectral flux monitors.
//! This is the forward-solver foundation of an inverse-design pipeline whose
//! flagship deliverable is adjoint gradients of transmission with respect to
//! the permittivity of every design cell, ending in fab-ready GDS geometry.
//!
//! The pipeline:
//!
//! 1. [`sim::Simulation`] — a Yee-grid FDTD stepper over a rectangular
//!    domain, TM or TE polarization, `f64` fields.
//! 2. [`material::Shape2`] — isotropic ε(x, y) painted from rectangles,
//!    circles, and polygons with area-weighted sub-pixel averaging.
//! 3. [`cpml`] — convolutional PML absorbing boundaries (graded polynomial
//!    conductivity, CFS α and κ knobs available).
//! 4. [`source`] — soft point and line sources; line sources carry the
//!    slab-waveguide eigenmode profile from [`modes`].
//! 5. [`monitor`] — running-DFT field monitors reduced to Poynting flux
//!    through lines: transmission and reflection spectra.
//! 6. [`dispersion`] — the solver's own error model: the discrete FDTD
//!    dispersion relation, exported so tests (and receipts) can price the
//!    difference between grid physics and continuum physics.
//!
//! # Units — read this before anything else
//!
//! Normalized Maxwell units: **c = ε₀ = μ₀ = 1** (vacuum impedance η₀ = 1).
//! Pick one length unit L (by convention in examples: **1 L = 1 µm**); then
//!
//! - frequency `f` is in units of c/L and **f = 1/λ** with λ in L,
//! - angular frequency ω = 2πf, vacuum wavenumber k₀ = 2π/λ,
//! - time is in units of L/c (one time unit = the time light takes to cross
//!   one length unit); a grid step advances `dt = courant·Δ/√2`.
//!
//! Nothing in the solver knows about meters or seconds. Materials enter as
//! **relative permittivity** ε (n = √ε). Field amplitudes are arbitrary
//! units — every shipped quantity is a ratio (transmission, reflection) or
//! is normalized explicitly. Unit confusion is the classic photonics-code
//! bug; when in doubt, λ = 1.55 means "1.55 length units" and f = 1/1.55.
//!
//! # Polarization naming — the other classic bug
//!
//! Following the photonic-crystals convention (Joannopoulos; Meep):
//!
//! - [`Polarization::Tm`] — **E out of plane**: fields (Ez, Hx, Hy).
//! - [`Polarization::Te`] — **H out of plane**: fields (Hz, Ex, Ey).
//!
//! Slab-waveguide literature names modes the opposite way (by the field in
//! the *incidence plane*): what this crate calls a TM simulation propagates
//! the slab literature's **TE** modes, and vice versa. [`modes`] documents
//! the cross-map next to the transcendental equations.
//!
//! **Scope and honesty (M0–M1):** 2D only (out-of-plane invariance — no
//! effective-index reduction of 3D structures is performed for you);
//! linear, isotropic, lossless, non-dispersive ε ≥ 1; μ = 1 everywhere; no
//! material dispersion fitting (a single ε per material — fine at one
//! design wavelength, wrong for broadband material physics); scalar
//! area-weighted sub-pixel averaging (no tensor/anisotropic smoothing);
//! soft sources radiate bidirectionally, and the M1
//! total-field/scattered-field mode plane injects +x-only with measured
//! (finite) backward leakage — see
//! [`source::SourcePlacement::TfsfVerticalLine`]; outer wall is PEC (or
//! PMC per side) behind the CPML. See `docs/photonics-m0.md` for the
//! milestone ladder.

pub mod adjoint;
pub mod cpml;
pub mod dispersion;
pub mod grid;
pub mod material;
pub mod modes;
pub mod monitor;
pub mod sim;
pub mod source;
pub mod waveform;

pub use adjoint::{
    objective_and_gradient, run_objective, DesignRegion, GradientResult, ModeOverlap,
};
pub use cpml::CpmlSpec;
pub use dispersion::{fdtd_phase_velocity, fdtd_wavenumber, fdtd_wavenumber_in_medium};
pub use grid::{Field2, GridSpec};
pub use material::Shape2;
pub use modes::{solve_slab_mode_even, ModeError, SlabMode};
pub use monitor::{dft_of_series, Cplx, FluxSpec};
pub use sim::{BoundarySpec, FluxId, Polarization, ProbeId, Simulation, Wall};
pub use source::{Source, SourcePlacement};
pub use waveform::Waveform;
