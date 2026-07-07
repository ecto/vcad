#![warn(missing_docs)]

//! Sketch-based operations for the vcad kernel.
//!
//! Provides extrude and revolve operations that create 3D B-rep solids
//! from 2D sketch profiles.
//!
//! # Example
//!
//! ```
//! use vcad_kernel_sketch::{SketchProfile, extrude};
//! use vcad_kernel_math::{Point3, Vec3};
//!
//! // Create a rectangular profile
//! let profile = SketchProfile::rectangle(
//!     Point3::origin(),
//!     Vec3::x(),
//!     Vec3::y(),
//!     10.0,
//!     5.0,
//! );
//!
//! // Extrude along Z axis
//! let solid = extrude(&profile, Vec3::new(0.0, 0.0, 20.0)).unwrap();
//! assert_eq!(solid.topology.faces.len(), 6);
//! ```

mod extrude;
mod profile;
mod revolve;

pub use extrude::{extrude, extrude_with_holes, extrude_with_options, ExtrudeOptions};
pub use profile::{SketchProfile, SketchSegment};
pub use revolve::revolve;

use thiserror::Error;

/// Errors from sketch-based operations.
#[derive(Debug, Clone, Error)]
pub enum SketchError {
    /// The profile is not closed (gap between first and last segment).
    #[error("profile is not closed: gap of {0:.6} mm")]
    NotClosed(f64),

    /// A segment is degenerate (zero length).
    #[error("degenerate segment at index {0}")]
    DegenerateSegment(usize),

    /// Extrusion direction has zero length.
    #[error("extrusion direction is zero")]
    ZeroExtrusion,

    /// Revolution axis has zero length.
    #[error("revolution axis is zero")]
    ZeroAxis,

    /// Revolution angle is invalid (must be in (0, 2π]).
    #[error("invalid revolution angle: {0} radians")]
    InvalidAngle(f64),

    /// Arc segments not supported for revolve (torus surfaces deferred).
    #[error("arc segments not supported for revolve operation")]
    ArcNotSupported,

    /// Profile intersects the revolution axis.
    #[error("profile intersects the revolution axis")]
    AxisIntersection,

    /// Profile has no segments.
    #[error("profile has no segments")]
    EmptyProfile,

    /// Interior hole loops are not supported by this operation.
    #[error("interior hole loops are not supported for {0}")]
    HolesUnsupported(&'static str),
}
