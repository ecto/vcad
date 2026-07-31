#![warn(missing_docs)]

//! CAM toolpath generation for vcad.
//!
//! This crate provides 2.5D CAM operations for generating toolpaths from
//! geometry, and post-processors for converting toolpaths to G-code.
//!
//! # Operations
//!
//! - [`Face`] - Surface facing with raster pattern
//! - [`Pocket2D`] - 2D pocket clearing with offset rings
//! - [`Contour2D`] - Profile machining with optional tabs
//!
//! # Example
//!
//! ```
//! use vcad_kernel_cam::{Tool, Face, CamSettings, post::{GrblPost, PostProcessor}};
//!
//! // Define a flat end mill
//! let tool = Tool::FlatEndMill {
//!     diameter: 6.0,
//!     flute_length: 20.0,
//!     flutes: 2,
//! };
//!
//! // Create a facing operation
//! let face = Face {
//!     min_x: 0.0,
//!     min_y: 0.0,
//!     max_x: 100.0,
//!     max_y: 50.0,
//!     depth: 1.0,
//! };
//!
//! let settings = CamSettings {
//!     stepover: 4.0,
//!     stepdown: 1.0,
//!     feed_rate: 1000.0,
//!     plunge_rate: 300.0,
//!     spindle_rpm: 12000.0,
//!     safe_z: 5.0,
//!     retract_z: 10.0,
//! };
//!
//! // Generate toolpath
//! let toolpath = face.generate(&tool, &settings).unwrap();
//!
//! // Export to G-code
//! let post = GrblPost::default();
//! let gcode = post.generate("facing_op", &tool, &toolpath, &settings);
//! ```

pub mod dropcutter;
mod error;
mod operation;
pub mod post;
mod tool;
mod toolpath;

// Re-exports
pub use error::CamError;
pub use operation::{
    CamOperation, Contour, Contour2D, ContourSegment, Face, Pocket2D, Point2D, Roughing3D, Tab,
};
pub use tool::{Tool, ToolEntry, ToolHolder, ToolLibrary};
pub use toolpath::{ArcDir, ArcPlane, CoolantMode, SpindleDir, Toolpath, ToolpathSegment};

use serde::{Deserialize, Serialize};

/// Settings for CAM operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CamSettings {
    /// Stepover distance between passes (mm).
    pub stepover: f64,
    /// Stepdown distance per Z level (mm).
    pub stepdown: f64,
    /// Feed rate for cutting moves (mm/min).
    pub feed_rate: f64,
    /// Plunge rate for Z moves (mm/min).
    pub plunge_rate: f64,
    /// Spindle speed (RPM).
    pub spindle_rpm: f64,
    /// Safe Z height for rapid moves between cuts (mm).
    pub safe_z: f64,
    /// Retract Z height for tool changes (mm).
    pub retract_z: f64,
}

impl Default for CamSettings {
    fn default() -> Self {
        Self {
            stepover: 3.0,
            stepdown: 2.0,
            feed_rate: 1000.0,
            plunge_rate: 300.0,
            spindle_rpm: 12000.0,
            safe_z: 5.0,
            retract_z: 10.0,
        }
    }
}

/// A complete CAM job with multiple operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CamJob {
    /// Job name for the G-code file header.
    pub name: String,
    /// Stock dimensions [x, y, z] in mm.
    pub stock: [f64; 3],
    /// Work coordinate system origin offset.
    pub wcs_offset: [f64; 3],
    /// Tool library for this job.
    pub tools: ToolLibrary,
    /// Operations in order of execution.
    pub operations: Vec<CamOperationEntry>,
}

/// An operation entry in a CAM job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CamOperationEntry {
    /// Operation name.
    pub name: String,
    /// Tool index in the tool library.
    pub tool_index: usize,
    /// The operation parameters.
    pub operation: CamOperation,
    /// Override settings for this operation (uses job defaults if None).
    pub settings: Option<CamSettings>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cam_settings_default() {
        let settings = CamSettings::default();
        assert!(settings.stepover > 0.0);
        assert!(settings.stepdown > 0.0);
        assert!(settings.feed_rate > 0.0);
        assert!(settings.safe_z > 0.0);
    }

    #[test]
    fn test_cam_settings_serialization() {
        let settings = CamSettings::default();
        let json = serde_json::to_string(&settings).unwrap();
        let parsed: CamSettings = serde_json::from_str(&json).unwrap();
        assert!((parsed.stepover - settings.stepover).abs() < 1e-6);
    }
}
