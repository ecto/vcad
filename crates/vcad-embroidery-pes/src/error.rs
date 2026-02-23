//! PES-specific error types.

use thiserror::Error;

/// Errors that can occur when reading or writing PES files.
#[derive(Debug, Error)]
pub enum PesError {
    /// Invalid or unrecognized PES header.
    #[error("invalid PES header: {0}")]
    InvalidHeader(String),

    /// PEC section offset is out of bounds.
    #[error("PEC offset {offset} exceeds data length {length}")]
    InvalidPecOffset {
        /// The PEC offset read from the file.
        offset: u32,
        /// The total file size.
        length: usize,
    },

    /// Unexpected end of data while parsing.
    #[error("unexpected end of data at offset {0}")]
    UnexpectedEof(usize),

    /// Invalid stitch encoding.
    #[error("invalid stitch encoding at offset {0}")]
    InvalidStitch(usize),

    /// Too many stitches for PES format.
    #[error("stitch count {0} exceeds PES maximum of 300000")]
    TooManyStitches(usize),

    /// Too many colors for PES format.
    #[error("color count {0} exceeds PES maximum of 127")]
    TooManyColors(usize),

    /// Pattern label too long.
    #[error("label exceeds 16 characters: {0}")]
    LabelTooLong(String),

    /// Core embroidery error.
    #[error(transparent)]
    Embroidery(#[from] vcad_embroidery::EmbroideryError),
}

/// Result type for PES operations.
pub type Result<T> = std::result::Result<T, PesError>;
