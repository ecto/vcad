//! Generative electronic parts catalog for vcad.
//!
//! A part is a parametric **family + value + package**, resolved on demand into
//! a fully-derived [`vcad_ir::ecad::DerivedPart`] (footprint + symbol + 3D body)
//! — not a scraped database row. Standard passives get infinite value coverage
//! from a handful of families; real manufacturer part numbers are sparse
//! [`catalog::ElecXref`] bridges layered on top, so the core is fully offline
//! and durable.
//!
//! ```
//! let part = vcad_ecad_parts::resolve("10k 0603 1%").unwrap();
//! assert_eq!(part.value, "10k");
//! assert_eq!(part.derived.footprint.pads.len(), 2);
//! ```
//!
//! Named **jellybean** ICs (NE555, op-amps, logic, MCUs, ...) — whose pinouts
//! are published facts, not formulas — live in the curated [`jellybean`]
//! database. [`jellybean::resolve_part_def`] turns `("NE555", "SOIC-8")` into
//! its eight pins, so a caller never hand-types pin numbers and types.
//!
//! ```
//! let part = vcad_ecad_parts::resolve_part_def("NE555", Some("SOIC-8")).unwrap();
//! assert_eq!(part.pins.len(), 8);
//! assert_eq!(part.pins[2].name, "OUT");
//! ```

#![warn(missing_docs)]

pub mod catalog;
pub mod eseries;
pub mod jellybean;
pub mod query;
pub mod spec;

pub use catalog::{resolve, search, ComponentClass, ResolvedPart};
pub use jellybean::{
    jellybean_names, jellybean_summaries, resolve_part_def, PartPin, ResolvedPartDef,
};
pub use query::ParsedQuery;
pub use spec::SpecValue;
