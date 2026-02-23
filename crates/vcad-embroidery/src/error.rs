//! Error types for embroidery operations.

use thiserror::Error;

/// Errors that can occur during embroidery operations.
#[derive(Debug, Error)]
pub enum EmbroideryError {
    /// Pattern exceeds hoop bounds.
    #[error("pattern exceeds hoop bounds: {0}")]
    ExceedsHoop(String),

    /// Invalid thread index reference.
    #[error("invalid thread index {index}, pattern has {count} threads")]
    InvalidThreadIndex {
        /// The invalid index.
        index: usize,
        /// Number of threads in the pattern.
        count: usize,
    },

    /// Pattern has no stitch groups.
    #[error("pattern has no stitch groups")]
    EmptyPattern,

    /// Stitch group has no commands.
    #[error("stitch group {0} has no commands")]
    EmptyStitchGroup(usize),
}

/// Result type for embroidery operations.
pub type Result<T> = std::result::Result<T, EmbroideryError>;
