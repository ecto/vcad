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

    /// Ramp angle is invalid (must be > 0 and < 90 degrees).
    #[error("invalid ramp angle: {0} degrees (must be > 0 and < 90)")]
    InvalidRampAngle(f64),

    /// Stock to leave is negative, which would cut into the part.
    #[error("invalid stock to leave: {0} (must be >= 0)")]
    InvalidStockToLeave(f64),

    /// A negative bottom allowance cuts past the underside of the stock, which
    /// needs a sacrificial board thick enough to take it.
    #[error(
        "cutting {overcut:.3} mm past the bottom of the stock needs a spoilboard at least that \
         thick; {spoilboard:.3} mm was declared. Set the spoilboard thickness, or leave a skin."
    )]
    BreakThroughWithoutSpoilboard {
        /// How far past the underside of the stock the cut would go, in mm.
        overcut: f64,
        /// Spoilboard thickness the caller declared, in mm.
        spoilboard: f64,
    },

    /// The onion skin is as thick as the part: nothing would be cut.
    #[error("bottom allowance {allowance:.3} mm leaves nothing of the {depth:.3} mm depth to cut")]
    BottomAllowanceExceedsDepth {
        /// The allowance asked for, in mm.
        allowance: f64,
        /// The depth asked for, in mm.
        depth: f64,
    },

    /// The centre-line fallback would cut further past the wall than allowed.
    #[error(
        "following the centre line would cut {overcut:.3} mm past the contour, more than the \
         {tolerance:.3} mm allowed: the cutter is too wide for this opening"
    )]
    CentreLineOvercut {
        /// Deepest the cutter would reach past the wall, in mm.
        overcut: f64,
        /// Largest overcut the caller allowed, in mm.
        tolerance: f64,
    },

    /// Centre-line tolerance is negative.
    #[error("invalid centre-line tolerance: {0} (must be >= 0)")]
    InvalidCentreLineTolerance(f64),

    /// This build has no polygon offsetter, so the centre-line fallback cannot
    /// be computed.
    #[error("the centre-line fallback needs the native polygon offsetter, which this build lacks")]
    CentreLineUnavailable,
}
