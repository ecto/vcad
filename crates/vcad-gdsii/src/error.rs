//! Error types for GDSII parsing, writing, and flattening.

/// Errors produced by the GDSII reader, writer, flattener, and bridge.
#[derive(Debug, thiserror::Error)]
pub enum GdsError {
    /// The byte stream ended in the middle of a record.
    #[error("unexpected end of GDSII stream")]
    UnexpectedEof,

    /// A record payload did not match its declared data type or size.
    #[error("invalid GDSII record 0x{rectype:02x}: {reason}")]
    InvalidRecord {
        /// The record type byte.
        rectype: u8,
        /// Human-readable description of the problem.
        reason: String,
    },

    /// A record that the stream grammar requires was missing.
    #[error("missing required GDSII record: {0}")]
    MissingRecord(&'static str),

    /// An SREF/AREF referenced a structure that does not exist in the library.
    #[error("reference to unknown cell `{0}`")]
    UnknownCell(String),

    /// SREF/AREF references form a cycle.
    #[error("circular cell reference involving `{0}`")]
    CircularReference(String),

    /// A PATH element used a pathtype other than 0 (flush ends). Only
    /// pathtype 0 is implemented by the flattener.
    #[error("unsupported pathtype {0} (only 0 = flush ends is implemented)")]
    UnsupportedPathType(i16),

    /// A PATH element could not be expanded to a boundary polygon.
    #[error("invalid path geometry: {0}")]
    InvalidPath(String),

    /// A value could not be encoded (e.g. a real outside excess-64 range).
    #[error("value not encodable in GDSII: {0}")]
    Unencodable(String),
}

/// Convenience result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, GdsError>;
