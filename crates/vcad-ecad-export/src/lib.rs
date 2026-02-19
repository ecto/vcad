#![warn(missing_docs)]
//! Fabrication-ready file export for vcad PCB designs.
//!
//! This crate generates industry-standard manufacturing output from [`vcad_ir::ecad::Pcb`] data:
//!
//! - **Gerber RS-274X** (`*.gbr`) -- copper, mask, silk, and outline layers
//! - **Excellon** (`*.drl`) -- drill files for through-hole and via holes
//! - **Pick-and-place CSV** -- component placement data for SMT assembly
//! - **BOM CSV** -- grouped bill of materials

pub mod bom;
pub mod excellon;
pub mod gerber;
pub mod pick_place;

pub use bom::write_bom;
pub use excellon::{write_excellon, ExcellonError};
pub use gerber::{generate_gerbers, write_gerber_layer, GerberError};
pub use pick_place::write_pick_place;
