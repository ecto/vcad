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
//! - [`dxf::flat_pattern_to_dxf`] — layered DXF (CUT / BEND_UP / BEND_DOWN)
//! - [`manufacturability::check_manufacturability`] — typed DFM query
//!
//! Later tiers add hems, jogs, miters, lofted flanges, and costing. See
//! `docs/design/sheet-metal.md`.

pub mod base_flange;
pub mod bend_table;
pub mod cost;
pub mod dxf;
pub mod edge_flange;
pub mod manufacturability;
pub mod materials;
pub mod model;
pub mod unfold;

pub use base_flange::{base_flange_rect, BaseFlangeError};
pub use bend_table::{BendAllowance, BendTable, KFactorSource};
pub use cost::{estimate_cost, CostBreakdown, CostRates};
pub use dxf::flat_pattern_to_dxf;
pub use edge_flange::{add_edge_flange, EdgeFlangeError, FlangePosition};
pub use manufacturability::{check_manufacturability, Severity, ShopProfile, Violation};
pub use materials::{
    builtin_materials, lookup as lookup_material, lookup_or_unknown as lookup_material_or_unknown,
    MaterialProperties,
};
pub use model::{Bend, BendDirection, BendId, Frame, Panel, PanelId, SheetMetalModel};
pub use unfold::{refold, unfold, FlatPattern, UnfoldError};
