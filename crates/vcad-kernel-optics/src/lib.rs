#![warn(missing_docs)]

//! Sequential ray-tracing lens design for the vcad kernel (M0).
//!
//! Models imaging optics — singlets, achromatic doublets, objectives — as
//! **sequential geometric ray tracing**: exact 3D Snell refraction through
//! an ordered list of spherical/conic surfaces, scored by spot-size figures
//! of merit at the image plane.
//!
//! The pipeline:
//!
//! 1. [`prescription::Prescription`] — the lens data table: per surface a
//!    radius of curvature, conic constant, semi-diameter, thickness to the
//!    next surface, and the glass that follows (mm, vcad convention).
//! 2. [`glass`] — Sellmeier dispersion for stock glasses (Schott catalog
//!    coefficients, cited) plus catalog-index fallbacks.
//! 3. [`trace`] — exact sequential trace: closed-form ray/conic
//!    intersection (conics are quadrics — no iteration at M0), vector-form
//!    Snell refraction, **fail-closed ray fates** (a ray that suffers total
//!    internal reflection, misses a surface, or is vignetted is *reported*,
//!    never silently dropped).
//! 4. [`paraxial`] — the independent y-u paraxial trace: EFL, BFD, front
//!    focal distance, Lagrange invariant. This is the analytic cross-check
//!    for the exact tracer, not a convenience.
//! 5. [`spot`] — RMS spot size over a field × wavelength grid, centroids,
//!    vignetting accounting, Airy-radius context.
//! 6. [`thirdorder`] — Seidel third-order thin-lens spherical aberration
//!    (the validation ladder's U-curve reference).
//! 7. [`optimize`] — scale-invariant finite-difference minimization
//!    (the M0 stand-in for the adjoint through the Snell chain).
//! 8. [`receipt`] — predicted claims (`vcad.optics-claims/1`) with full
//!    trace provenance; rolls into the unified `vcad.receipt/1` as
//!    Provisional, never Pass.
//!
//! **Scope and honesty (M0):** geometric optics only. No diffraction, no
//! physical optics — RMS spot size is a *geometric* claim, and every claim
//! set carries the Airy radius next to it so a sub-diffraction spot number
//! cannot overreach. Sequential surfaces only (no ghost/stray analysis).
//! Tolerancing belongs to `vcad-kernel-tolerance` (a later seam). See the
//! milestone ladder in `docs/optics-m0.md`.
//!
//! Units: geometry in **millimeters** (vcad convention), wavelengths in
//! **micrometers** (Sellmeier convention), spot sizes reported in µm.

pub mod glass;
pub mod optimize;
pub mod paraxial;
pub mod prescription;
pub mod receipt;
pub mod spot;
pub mod thirdorder;
pub mod trace;

/// Fraunhofer spectral lines used throughout (µm).
pub mod lines {
    /// Helium d line, 587.5618 nm — the visible design/reference line.
    pub const D: f64 = 0.5875618;
    /// Hydrogen F line, 486.1327 nm — the blue chromatic reference.
    pub const F: f64 = 0.4861327;
    /// Hydrogen C line, 656.2725 nm — the red chromatic reference.
    pub const C: f64 = 0.6562725;
}
