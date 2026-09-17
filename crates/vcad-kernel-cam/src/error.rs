//! Error types for CAM operations.

use thiserror::Error;

/// Errors from CAM toolpath generation.
#[derive(Debug, Clone, Error)]
pub enum CamError {
    /// The contour is empty or has no segments.
    #[error("contour is empty")]
    EmptyContour,

    /// The contour is not closed.
    #[error("contour is not closed: gap of {0:.6} mm")]
    NotClosed(f64),

    /// Tool diameter is invalid (zero or negative).
    #[error("invalid tool diameter: {0}")]
    InvalidToolDiameter(f64),

    /// Stepover is invalid (must be > 0 and <= tool diameter).
    #[error("invalid stepover: {0} (must be > 0 and <= tool diameter)")]
    InvalidStepover(f64),

    /// Stepdown is invalid (must be > 0).
    #[error("invalid stepdown: {0} (must be > 0)")]
    InvalidStepdown(f64),

    /// Depth is invalid (must be > 0).
    #[error("invalid depth: {0} (must be > 0)")]
    InvalidDepth(f64),

    /// Feed rate is invalid (must be > 0).
    #[error("invalid feed rate: {0} (must be > 0)")]
    InvalidFeedRate(f64),

    /// Spindle speed is invalid (must be > 0).
    #[error("invalid spindle speed: {0} (must be > 0)")]
    InvalidSpindleSpeed(f64),

    /// Bounds are degenerate (zero area).
    #[error("degenerate bounds: width={0}, height={1}")]
    DegenerateBounds(f64, f64),

    /// Pocket offset resulted in empty geometry.
    #[error("pocket offset resulted in empty geometry")]
    EmptyPocketOffset,

    /// The tool-compensated contour fell into separate pieces: the cutter does
    /// not fit through a neck of the contour.
    #[error(
        "the cutter does not fit through the contour: its path splits into {0} separate regions"
    )]
    ContourSplit(usize),

    /// Tab position is out of range.
    #[error("tab position {0} is out of contour range")]
    InvalidTabPosition(f64),
}
