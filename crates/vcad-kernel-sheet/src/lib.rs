#![warn(missing_docs)]

//! Sheet-metal modeling for the vcad kernel.
//!
//! Treats sheet metal as a **constraint manifold inside the BRep state space**:
//! a [`SheetMetalModel`] is a graph of flat [`Panel`]s connected by cylindrical
//! [`Bend`]s. The graph is the source of truth — both the bent 3D body and the
//! flat pattern are views computed from it.
//!
//! This buys us **lossless bidirectional unfold**: because the bend metadata
//! (radius, angle, K-factor) lives on the bend itself rather than being
//! reconstructed from cylindrical face geometry, [`unfold`] and [`refold`] are
//! exact inverses by construction. See [`unfold::unfold`] and [`unfold::refold`].
//!
//! # Foundation tier
//!
//! The MVP exposes:
//! - [`base_flange::base_flange_rect`] — start a model from a rectangular sheet
//! - [`edge_flange::add_edge_flange`] — add a flange off an existing panel edge
//! - [`unfold::unfold`] / [`unfold::refold`] — lossless 2D ↔ 3D round-trip
//! - [`bend_table::BendTable`] — `BA = θ·(R + K·t)` with provenance
//!
//! Later tiers add hems, jogs, miters, lofted flanges, manufacturability
//! checks, costing, and DXF export. See `docs/design/sheet-metal.md`.

pub mod base_flange;
pub mod bend_table;
pub mod edge_flange;
pub mod model;
pub mod unfold;

pub use base_flange::{base_flange_rect, BaseFlangeError};
pub use bend_table::{BendAllowance, BendTable, KFactorSource};
pub use edge_flange::{add_edge_flange, EdgeFlangeError, FlangePosition};
pub use model::{Bend, BendDirection, BendId, Frame, Panel, PanelId, SheetMetalModel};
pub use unfold::{refold, unfold, FlatPattern, UnfoldError};
