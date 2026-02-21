//! Trace routing algorithms for PCB layout.
//!
//! This module provides multiple routing strategies:
//!
//! - [`grid`] -- Lee/wave BFS-based grid router (fully implemented)
//! - [`push_shove`] -- Interactive push-and-shove router (stub)
//! - [`diff_pair`] -- Differential pair router (stub)
//! - [`length_tune`] -- Length tuning meander generator (stub)

pub mod diff_pair;
pub mod grid;
pub mod length_tune;
pub mod push_shove;

use vcad_ir::Vec2;

/// Unique identifier for a net within the router.
pub type NetId = u32;

/// Route result for a single net.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RouteResult {
    /// Net name that was routed.
    pub net: String,
    /// Routed trace segments as (start, end) pairs in board coordinates (mm).
    pub segments: Vec<(Vec2, Vec2)>,
    /// Via locations where the route changes layers.
    pub vias: Vec<Vec2>,
    /// Whether routing succeeded.
    pub success: bool,
}

/// Common routing configuration shared across algorithms.
#[derive(Debug, Clone)]
pub struct RouteConfig {
    /// Default trace width in mm.
    pub trace_width: f64,
    /// Default clearance in mm.
    pub clearance: f64,
    /// Default via diameter in mm.
    pub via_diameter: f64,
    /// Default via drill in mm.
    pub via_drill: f64,
}

impl Default for RouteConfig {
    fn default() -> Self {
        Self {
            trace_width: 0.25,
            clearance: 0.2,
            via_diameter: 0.8,
            via_drill: 0.4,
        }
    }
}
