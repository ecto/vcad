#![warn(missing_docs)]

//! URDF robot description format import/export for vcad.
//!
//! This crate provides reading and writing of URDF (Unified Robot Description Format)
//! files, the standard format for describing robot kinematics and dynamics in ROS.
//!
//! # Supported Features
//!
//! - **Joints**: fixed, revolute, continuous, prismatic → vcad Joint types
//! - **Geometry**: box, cylinder, sphere, mesh references
//! - **Links**: with visual and collision geometry
//! - **Materials**: basic color support
//!
//! # Example
//!
//! ```no_run
//! use vcad_kernel_urdf::{read_urdf, write_urdf};
//!
//! // Import a URDF file
//! let doc = read_urdf("robot.urdf").unwrap();
//!
//! // Export back to URDF
//! write_urdf(&doc, "output.urdf").unwrap();
//! ```

mod error;
mod reader;
mod types;
mod writer;

pub use error::UrdfError;
pub use reader::{
    read_urdf, read_urdf_from_str, read_urdf_from_str_with_options, read_urdf_with_options,
    UrdfReadOptions,
};
pub use writer::{write_urdf, write_urdf_to_string};
