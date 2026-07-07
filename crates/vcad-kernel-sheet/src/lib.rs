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
pub mod hem;
pub mod jog;
pub mod manufacturability;
pub mod materials;
pub mod model;
pub mod nesting;
pub mod poly2d;
pub mod receipt;
pub mod relief;
pub mod sequence;
pub mod shop_profiles;
pub mod silhouette;
pub mod unfold;

pub use base_flange::{
    base_flange_polygon, base_flange_polygon_with_holes, base_flange_rect, BaseFlangeError,
};
pub use bend_table::{BendAllowance, BendTable, KFactorSource};
pub use cost::{estimate_cost, CostBreakdown, CostRates};
pub use dxf::{
    flat_pattern_to_dxf, flat_pattern_to_dxf_with, nested_dxf, DxfOptions, NestedPlacement,
};
pub use edge_flange::{add_edge_flange, EdgeFlangeError, FlangePosition};
pub use hem::{add_hem, HemKind, HemParams};
pub use jog::{add_jog, JogParams, JogResult};
pub use manufacturability::{check_manufacturability, Severity, ShopProfile, Violation};
pub use materials::{
    builtin_materials, lookup as lookup_material, lookup_or_unknown as lookup_material_or_unknown,
    MaterialProperties,
};
pub use model::{Bend, BendDirection, BendId, Frame, Panel, PanelId, SheetMetalModel};
pub use nesting::{nest_rectangles, NestingParams, NestingResult, PartFootprint, Placement};
pub use poly2d::Poly;
pub use receipt::{cost_claims, manufacturability_claims, sheet_metal_receipt};
pub use relief::{apply_bend_relief, find_missing_reliefs, ReliefError, ReliefNotch, ReliefParams};
pub use sequence::{bend_sequence, BendStep};
pub use shop_profiles::{
    builtin_shop_ids, shop_catalog, ShopCatalog, ShopLookupError, ShopMaterial, ShopRow,
};
pub use silhouette::{silhouette, BendLine, Silhouette, SilhouetteError};
pub use unfold::{refold, unfold, FlatPattern, UnfoldError};
