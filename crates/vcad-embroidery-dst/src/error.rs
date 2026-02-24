//! DST-specific error types.

use thiserror::Error;

/// Errors that can occur when reading or writing DST files.
#[derive(Debug, Error)]
pub enum DstError {
    /// Invalid or unrecognized DST header.
    #[error("invalid DST header: {0}")]
    InvalidHeader(String),

    /// Unexpected end of data while parsing.
    #[error("unexpected end of data at offset {0}")]
    UnexpectedEof(usize),

    /// Invalid stitch data encountered during parsing.
    #[error("invalid stitch data at offset {0}: {1}")]
    InvalidStitchData(usize, String),

    /// Stitch count exceeds the safety limit.
    #[error("too many stitches: {0} (max 1,000,000)")]
    TooManyStitches(usize),

    /// Pattern contains no stitch data.
    #[error("empty pattern")]
    EmptyPattern,
}

/// Result type for DST operations.
pub type Result<T> = std::result::Result<T, DstError>;
