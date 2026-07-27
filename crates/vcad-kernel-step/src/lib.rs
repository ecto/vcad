#![warn(missing_docs)]

//! STEP file import/export for the vcad kernel.
//!
//! Provides reading and writing of STEP files (ISO 10303-21) for B-rep solids.
//! Targets AP214 (Automotive Design) protocol, the most common mechanical CAD protocol.
//!
//! # Single-body import
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
//!
//! # Assembly import
//!
//! ```no_run
//! use vcad_kernel_step::read_step_assembly;
//!
//! let asm = read_step_assembly("assembly.step").unwrap();
//! for part in &asm.parts {
//!     println!("{}: {} solids", part.name, part.solids.len());
//! }
//! for inst in &asm.instances {
//!     let tx = &inst.transform.origin;
//!     println!("  instance {} at ({:.1}, {:.1}, {:.1})", inst.id, tx[0], tx[1], tx[2]);
//! }
//! ```
//!
//! # PMI extraction (stretch)
//!
//! ```no_run
//! use vcad_kernel_step::read_step_pmi;
//!
//! let pmi = read_step_pmi("annotated.step").unwrap();
//! for dim in &pmi.dimensions {
//!     println!("dim {:?}: {}", dim.kind, dim.value);
//! }
//! ```

mod assembly;
mod entities;
mod error;
mod pmi;
mod reader;
mod reconstruct;
mod writer;

pub use assembly::{
    read_step_assembly, read_step_assembly_from_buffer, StepAssembly, StepInstance, StepPart,
    StepPlacement,
};
pub use error::StepError;
pub use pmi::{
    read_step_pmi, read_step_pmi_from_buffer, DimensionKind, StepDatum, StepDimension, StepPmi,
    StepTolerance,
};
pub use reader::{read_step, read_step_from_buffer};
pub use writer::{
    write_step, write_step_solids, write_step_solids_to_buffer, write_step_to_buffer,
};

// Re-export stepperoni types for downstream consumers
pub use stepperoni::{
    parse, tokenize, Lexer, Parser, Position, SpannedToken, StepEntity, StepFile, StepValue, Token,
};
