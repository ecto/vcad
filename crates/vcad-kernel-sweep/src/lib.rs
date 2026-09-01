#![warn(missing_docs)]

//! Sweep and loft operations for the vcad kernel.
//!
//! Provides operations to create 3D B-rep solids by:
//! - **Sweep**: Moving a 2D profile along a 3D path curve
//! - **Loft**: Interpolating between multiple 2D profiles
//!
//! # Example
//!
//! ```
//! use vcad_kernel_sweep::{sweep, SweepOptions};
//! use vcad_kernel_sketch::SketchProfile;
//! use vcad_kernel_geom::Circle3d;
//! use vcad_kernel_math::{Point3, Vec3};
//!
//! // Create a circular profile
//! let profile = SketchProfile::circle(Point3::origin(), Vec3::z(), 2.0, 8);
//!
//! // Create a helical path
//! let path = vcad_kernel_sweep::Helix::new(10.0, 5.0, 50.0, 4.0);
//!
//! // Sweep the profile along the path
//! let solid = sweep(&profile, &path, SweepOptions::default()).unwrap();
//! ```

mod frenet;
mod loft;
mod sweep;

pub use frenet::FrenetFrame;
pub use loft::{loft, LoftMode, LoftOptions};
pub use sweep::{sweep, sweep_cylindrical, CylindricalPath, Helix, SweepOptions, DEFAULT_SEG_DEG};

use thiserror::Error;

/// Errors from sweep operations.
#[derive(Debug, Clone, Error)]
pub enum SweepError {
    /// The path has zero length.
    #[error("path has zero length")]
    ZeroLengthPath,

    /// The profile is invalid.
    #[error("invalid profile: {0}")]
    InvalidProfile(String),

    /// Path segments is zero.
    #[error("path segments must be at least 2")]
    TooFewSegments,

    /// The computed frame is degenerate (e.g., curvature is zero).
    #[error("degenerate frame at parameter t={0}")]
    DegenerateFrame(f64),
}

/// Errors from loft operations.
#[derive(Debug, Clone, Error)]
pub enum LoftError {
    /// Need at least 2 profiles for lofting.
    #[error("loft requires at least 2 profiles, got {0}")]
    TooFewProfiles(usize),

    /// Profiles have different segment counts.
    #[error("profiles have different segment counts: {0} vs {1}")]
    MismatchedSegmentCounts(usize, usize),

    /// A profile is invalid.
    #[error("invalid profile at index {0}: {1}")]
    InvalidProfile(usize, String),
}
