#![warn(missing_docs)]

//! STEP file import/export for the vcad kernel.
//!
//! Provides reading and writing of STEP files (ISO 10303-21) for B-rep solids.
//! Targets AP214 (Automotive Design) protocol, the most common mechanical CAD protocol.
//!
//! # Example
//!
//! ```no_run
//! use vcad_kernel_step::{read_step, write_step};
//!
//! // Read a STEP file
//! let solids = read_step("model.step").unwrap();
//!
//! // Write a B-rep solid to STEP
//! write_step(&solids[0], "output.step").unwrap();
//! ```

mod entities;
mod error;
mod reader;
mod writer;

pub use error::StepError;
pub use reader::{read_step, read_step_from_buffer};
pub use writer::{write_step, write_step_to_buffer};

// Re-export stepperoni types for downstream consumers
pub use stepperoni::{
    parse, tokenize, Lexer, Parser, Position, SpannedToken, StepEntity, StepFile, StepValue, Token,
};
