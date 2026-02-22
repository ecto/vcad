#![warn(missing_docs)]

//! Bambu Lab printer integration for the vcad slicer.
//!
//! This crate provides:
//! - 3MF file generation (always available)
//! - MQTT communication with Bambu printers (requires `network` feature)
//! - Printer discovery via SSDP (requires `network` feature)
//! - Print control (start, pause, stop) (requires `network` feature)
//! - Status monitoring (requires `network` feature)
//!
//! # Example
//!
//! ```ignore
//! use vcad_slicer_bambu::{BambuPrinter, discover_printers_async};
//! use std::time::Duration;
//!
//! // Discover printers
//! let printers = discover_printers_async(Duration::from_secs(5)).await?;
//!
//! // Connect to first printer
//! let printer = BambuPrinter::connect(
//!     printers[0].ip,
//!     &printers[0].serial,
//!     "access_code_here"
//! ).await?;
//!
//! // Get status
//! let status = printer.status().await?;
//! println!("State: {:?}, Progress: {}%", status.state, status.progress_percent);
//!
//! // Control print
//! printer.pause().await?;
//! printer.resume().await?;
//! ```

pub mod commands;
#[cfg(feature = "network")]
pub mod discovery;
pub mod error;
#[cfg(feature = "network")]
pub mod ftp;
#[cfg(feature = "network")]
pub mod mqtt;
pub mod status;
pub mod threemf;

pub use commands::PrinterCommand;
#[cfg(feature = "network")]
pub use discovery::{discover_printers, discover_printers_async, PrinterInfo};
pub use error::{BambuError, Result};
#[cfg(feature = "network")]
pub use mqtt::{BambuConfig, BambuMqttClient, BambuPrinter};
pub use status::{AmsSlot, AmsStatus, AmsUnit, PrintState, PrinterStatus};
pub use threemf::{PlateConfig, PrintSettings, ThreeMfModel};
