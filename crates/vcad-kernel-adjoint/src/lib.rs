//! The coupled discrete-adjoint seam.
//!
//! vcad has a dozen single-physics adjoints — thermal, flow, em,
//! photonics, antenna, particle, neutronics, fea, the differentiable
//! geometry seam, the phyz rollout adjoint. Each one is correct on its
//! own. **Chaining two of them across a coupling interface and calling
//! the product a coupled gradient is wrong**, and not by a little.
//!
//! SU2 published the numbers. Differentiating a conjugate-heat-transfer
//! heat flux with the cross terms dropped gives 39% error; drop the
//! fluid-structure terms in a three-physics case and it reaches 87%;
//! drop every coupling term and the gradient comes back as −0.525 where
//! the finite-difference truth is +0.251 — **the wrong sign**. An
//! optimizer driven by an under-coupled adjoint does not converge
//! slowly. It walks uphill.
//!
//! ```text
//! O. Burghardt, "The Discrete Adjoint Solver in SU2 and its Application
//! to Multiphysics Problems", NASA Ames AMS Seminar, September 2024.
//! ```
//!
//! # What this crate is
//!
//! Three things, none of which is a solver:
//!
//! 1. [`ledger`] — the z×z **cross-term ledger**. For z coupled
//!    disciplines there are z² Jacobian blocks; this records, per block,
//!    whether it is implemented (and by what method), deliberately
//!    frozen (with the physical assumption named and a bound on the
//!    error it costs), structurally absent, or *missing*. The ledger is
//!    machine-readable, it rolls up to a [`ledger::Completeness`], and
//!    that maps onto the receipt's [`vcad_receipt::ClaimBasis`] /
//!    [`vcad_receipt::ClaimVerdict`] so an incomplete gradient can never
//!    be reported as a passing claim. SU2 documents this in prose on
//!    slides; here it is a type the driver refuses to run without.
//!
//! 2. [`couple`] — the block-Jacobi coupled-adjoint driver, which is
//!    SU2's four operations (`ComputeAdjoints`, `Iterate`,
//!    `AddExternal`, `UpdateCrossTerm`) expressed as two Rust traits
//!    plus a fixed-point loop. Disciplines keep their own hand-derived
//!    transposes in their own crates; this only orchestrates them.
//!
//! 3. [`sensitivity`] — the reporting vocabulary: a sensitivity carries
//!    its value, unit, differentiation route, provenance, completeness,
//!    and a **trust radius** — the interval of the parameter over which
//!    the derivative is meaningful. A gradient that does not know where
//!    it stops being true is a liability in a CAD system, where a fillet
//!    can vanish and take the linearization with it.
//!
//! plus [`validate`], the harness that produces SU2-style evidence:
//! a finite-difference step-size sweep that has to *plateau* before it
//! is allowed to serve as a reference, and an ablation check that proves
//! a cross term is load-bearing by deleting it and watching the error
//! blow up.
//!
//! # Why ablation, and not just a passing FD check
//!
//! A single finite-difference agreement does not prove a cross term is
//! wired in. The term may simply be near zero in that fixture. The
//! ablation check ([`validate::ablation`]) recomputes the gradient with
//! the term deliberately removed and requires the error to *grow* by a
//! stated factor. That is a test of the wiring, not of the arithmetic.
//!
//! **Honesty:** nothing here differentiates anything. The correctness of
//! a coupled gradient rests entirely on the blocks the disciplines
//! register, and on the ledger being an accurate description of them. A
//! ledger that claims `Implemented` for a block whose `CrossTerm` was
//! never registered is caught ([`couple::CoupleError::LedgerMismatch`]);
//! a ledger that claims `Absent` for a block that is physically nonzero
//! is not, and cannot be. That one is on the author.

#![warn(missing_docs)]

pub mod couple;
pub mod ledger;
pub mod sensitivity;
pub mod validate;

pub use couple::{
    solve_coupled, AdjointDiscipline, CoupleError, CoupleOptions, CoupledAdjoint, CrossTerm,
};
pub use ledger::{BlockMethod, BlockStatus, Completeness, CouplingLedger, LedgerError};
pub use sensitivity::{Route, Sensitivity, SensitivityTable, TrustLimit, TrustRadius};
pub use validate::{ablation, fd_sweep, AblationReport, FdRow, FdSweep, SweepError};

// The receipt vocabulary a sensitivity carries. Re-exported so consumers
// reporting gradients do not have to depend on `vcad-receipt` directly
// just to name a basis.
pub use vcad_receipt::{ClaimBasis, ClaimVerdict, OracleRef, ReceiptClaim};

/// Schema id for the serialized coupling ledger. Bump on wire-shape
/// changes (same convention as `vcad.receipt/1`).
pub const LEDGER_SCHEMA: &str = "vcad.coupling-ledger/1";
