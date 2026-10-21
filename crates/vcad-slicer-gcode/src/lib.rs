#![warn(missing_docs)]

//! G-code generation for the vcad slicer.
//!
//! This crate converts sliced models into G-code for 3D printers.
//! It supports multiple printer profiles and G-code flavors.
//!
//! # Example
//!
//! ```ignore
//! use vcad_slicer::{slice, SliceSettings};
//! use vcad_slicer_gcode::{generate_gcode, GcodeSettings, PrinterProfile};
//!
//! let slice_result = slice(&mesh, &SliceSettings::default())?;
//!
//! let gcode_settings = GcodeSettings {
//!     printer: PrinterProfile::bambu_x1c(),
//!     print_temp: 220,
//!     bed_temp: 55,
//!     ..Default::default()
//! };
//!
//! let gcode = generate_gcode(&slice_result, gcode_settings);
//! std::fs::write("output.gcode", gcode)?;
//! ```

pub mod flavor;
pub mod gcode;
pub mod lint;
pub mod printer;

pub use flavor::GcodeFlavor;
pub use gcode::{generate_gcode, GcodeGenerator, GcodeSettings};
pub use lint::{lint, LintFinding, LintLimits, LintReport};
pub use printer::PrinterProfile;
