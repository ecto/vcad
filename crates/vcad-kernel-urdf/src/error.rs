//! Error types for URDF processing.

use thiserror::Error;

/// Errors that can occur during URDF import/export.
#[derive(Error, Debug)]
pub enum UrdfError {
    /// Failed to read the file.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to parse XML.
    #[error("XML parse error: {0}")]
    XmlParse(#[from] quick_xml::Error),

    /// Failed to deserialize XML.
    #[error("XML deserialization error: {0}")]
    XmlDeserialize(#[from] quick_xml::DeError),

    /// Failed to serialize XML.
    #[error("XML serialization error: {0}")]
    XmlSerialize(#[from] quick_xml::SeError),

    /// Missing required element.
    #[error("Missing required element: {0}")]
    MissingElement(String),

    /// Invalid attribute value.
    #[error("Invalid attribute '{attr}': {msg}")]
    InvalidAttribute {
        /// Attribute name.
        attr: String,
        /// Error message.
        msg: String,
    },

    /// Unsupported joint type.
    #[error("Unsupported joint type: {0}")]
    UnsupportedJointType(String),

    /// Reference to unknown link.
    #[error("Unknown link reference: {0}")]
    UnknownLink(String),

    /// Invalid geometry specification.
    #[error("Invalid geometry: {0}")]
    InvalidGeometry(String),

    /// Circular reference detected in kinematic chain.
    #[error("Circular reference in kinematic chain at joint: {0}")]
    CircularReference(String),

    /// Document conversion error.
    #[error("Document conversion error: {0}")]
    Conversion(String),

    /// Input violated a structural limit or was otherwise malformed.
    #[error("Invalid URDF format: {0}")]
    InvalidFormat(String),
}
