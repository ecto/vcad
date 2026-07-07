#![warn(missing_docs)]

//! Stock simulation for CAM material removal visualization.
//!
//! This crate provides octree-based SDF stock simulation for visualizing
//! material removal during CNC machining operations.
//!
//! # Features
//!
//! - SDF octree representation of stock
//! - Swept volume subtraction for toolpath simulation, with per-tool cutter
//!   envelopes (flat, ball, bull, and cone bottoms)
//! - Verification oracle: gouge and excess checks against a target mesh
//!   ([`verify_stock_against_mesh`])
//! - Marching cubes mesh extraction for visualization
//! - Collision detection for tool holders
//!
//! # Example
//!
//! ```
//! use vcad_kernel_stocksim::Stock;
//!
//! // Create a rectangular stock block
//! let mut stock = Stock::from_box([0.0, 0.0, 0.0, 100.0, 100.0, 50.0], 2.0);
//!
//! // Stock can be subtracted from using toolpaths
//! // and converted to mesh for visualization
//! let mesh = stock.to_mesh();
//! ```

mod collision;
mod marching_cubes;
mod octree;
mod subtract;
mod verify;

pub use collision::{CollisionResult, CollisionType};
pub use marching_cubes::MarchingCubes;
pub use octree::{OctreeNode, Stock};
pub use subtract::SweptVolume;
pub use verify::{
    verify_stock_against_mesh, CheckReport, StockVerification, VerifyOptions, ViolationSample,
};

use thiserror::Error;

/// Errors from stock simulation operations.
#[derive(Debug, Clone, Error)]
pub enum StockSimError {
    /// Invalid stock bounds (zero or negative dimensions).
    #[error("invalid stock bounds: {0}")]
    InvalidBounds(String),

    /// Resolution too small.
    #[error("resolution too small: {0}")]
    ResolutionTooSmall(f64),

    /// Tool too large for operation.
    #[error("tool diameter too large: {0}")]
    ToolTooLarge(f64),
}
