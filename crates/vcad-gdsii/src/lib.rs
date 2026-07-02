#![warn(missing_docs)]

//! GDSII (Calma stream format) reader/writer for vcad.
//!
//! GDSII is the interchange format for IC mask layout: a binary stream of
//! big-endian records describing a library of cells containing polygons,
//! paths, text, and (arrayed) cell references. This crate provides:
//!
//! - **Records** ([`record`]): lossless record-level encode/decode,
//!   including the 8-byte excess-64 float format ([`real`]) that GDSII
//!   inherited from IBM System/360.
//! - **Model** ([`model`]): a plain-data [`Library`] / [`Cell`] /
//!   [`Element`] tree with `i32` database-unit coordinates.
//! - **Flatten** ([`flatten()`]): recursive SREF/AREF resolution
//!   (translation, rotation, mirror, magnification) into flat per-layer
//!   polygons in f64 database units, with PATH → boundary expansion and
//!   cycle detection.
//! - **Bridge** ([`bridge`], `vcad-ir` feature, on by default):
//!   [`to_vcad_document`] turns flattened layers into a `vcad_ir::Document`
//!   — one sketch-extrude part per layer of a caller-supplied layer stack.
//!
//! # Example
//!
//! ```
//! use vcad_gdsii::{read_library, write_library, flatten, Cell, Element, Library};
//!
//! // Build a library: one cell with a 1 µm square on layer 1 (nm grid).
//! let mut cell = Cell::new("pixel");
//! cell.elements.push(Element::Boundary {
//!     layer: 1,
//!     datatype: 0,
//!     xy: vec![(0, 0), (1000, 0), (1000, 1000), (0, 1000), (0, 0)],
//! });
//! let mut lib = Library::new("mylib");
//! lib.cells.push(cell);
//!
//! // Write to GDSII bytes and read back losslessly.
//! let bytes = write_library(&lib).unwrap();
//! assert_eq!(read_library(&bytes).unwrap(), lib);
//!
//! // Flatten to per-layer polygons (f64 database units).
//! let flat = flatten(&lib, "pixel").unwrap();
//! assert_eq!(flat[0].layer, 1);
//! assert_eq!(flat[0].polygons[0].len(), 4);
//! ```

pub mod error;
pub mod flatten;
pub mod model;
pub mod reader;
pub mod real;
pub mod record;
pub mod writer;

#[cfg(feature = "vcad-ir")]
pub mod bridge;

pub use error::{GdsError, Result};
pub use flatten::{flatten, LayerPolygons};
pub use model::{Cell, Element, Library, Strans};
pub use reader::read_library;
pub use writer::write_library;

#[cfg(feature = "vcad-ir")]
pub use bridge::{to_vcad_document, LayerStackEntry, DEFAULT_VIEW_SCALE};
