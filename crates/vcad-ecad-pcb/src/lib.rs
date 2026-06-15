#![warn(missing_docs)]
//! PCB layout operations for vcad.
//!
//! This crate provides routing, copper pour, design rule checking (DRC), and
//! spatial indexing for PCB layouts defined in [`vcad_ir::ecad`].
//!
//! # Modules
//!
//! - [`router`] -- Trace routing algorithms (grid, push-and-shove, diff pair, length tuning)
//! - [`session`] -- Incremental routing session: the in-loop legality oracle
//! - [`copper_pour`] -- Zone fill algorithm
//! - [`drc`] -- Design rule checking engine
//! - [`spatial`] -- R-tree spatial index for copper elements

pub mod component_mesh;
pub mod copper_pour;
pub mod drc;
pub mod geometry;
pub mod ratsnest;
pub mod router;
pub mod session;
pub mod spatial;
pub mod teardrop;

pub use copper_pour::{fill_zones, FilledZone};
pub use drc::{check_drc, DrcRuleType, DrcSeverity, DrcViolation};
pub use router::grid::GridRouter;
pub use session::{Blocker, ProbeResult, RouteSession, SpanId};
pub use spatial::{CopperElement, CopperGeom, SpatialIndex};
pub use teardrop::{generate_teardrops, Teardrop};

/// Errors returned by PCB operations.
#[derive(Debug, thiserror::Error)]
pub enum PcbError {
    /// A routing operation failed.
    #[error("routing failed: {0}")]
    RoutingFailed(String),

    /// A DRC check encountered an invalid configuration.
    #[error("DRC configuration error: {0}")]
    DrcConfig(String),

    /// A spatial index operation failed.
    #[error("spatial index error: {0}")]
    SpatialIndex(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        let err = PcbError::RoutingFailed("no path found".to_string());
        assert_eq!(err.to_string(), "routing failed: no path found");

        let err = PcbError::DrcConfig("missing rules".to_string());
        assert_eq!(err.to_string(), "DRC configuration error: missing rules");

        let err = PcbError::SpatialIndex("tree empty".to_string());
        assert_eq!(err.to_string(), "spatial index error: tree empty");
    }
}
