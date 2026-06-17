//! Parametric IPC-7351 package generator for vcad.
//!
//! A [`vcad_ir::ecad::PackageClass`] is the single source of truth for an
//! electronic part's *physical* package. [`derive`] turns it — in one pass —
//! into a PCB land pattern, a schematic symbol, and a real 3D body, all sharing
//! one pin numbering so they cannot disagree. This is the "generate, don't
//! aggregate" core: standard packages get infinite coverage from parametric
//! families, and the geometry is correct-by-construction rather than three
//! independently authored files that drift.
//!
//! ```
//! use vcad_ir::ecad::*;
//! use vcad_ecad_package::derive;
//!
//! let pc = PackageClass {
//!     id: "QFN-40_5x5mm_P0.4mm".into(),
//!     family: PackageFamily::NoLead,
//!     body: BodyEnvelope { length: 5.0, width: 5.0, height: 0.9, standoff: 0.0 },
//!     leads: LeadSpec {
//!         pitch: 0.4, count_per_side: 10, sides: 4,
//!         lead_length: 0.4, lead_width: 0.2, terminal: LeadTerminal::Smd,
//!     },
//!     thermal_pad: Some(ThermalPad { length: 3.7, width: 3.7 }),
//!     density: DensityLevel::Nominal,
//!     pin_map: PinMap { numbering: PinNumbering::Ccw, pins: vec![], polarity_marker: true },
//! };
//! let part = derive(&pc).unwrap();
//! assert_eq!(part.footprint.pads.len(), 41); // 40 leads + exposed pad
//! ```

#![warn(missing_docs)]

pub mod derive;
pub mod ipc7351;
pub mod presets;

pub use derive::{derive, DeriveError};
