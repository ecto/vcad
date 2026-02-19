//! Differential pair router.
//!
//! Routes two traces in parallel with a controlled gap, maintaining
//! impedance matching for high-speed differential signals (USB, HDMI,
//! Ethernet, etc.).
//!
//! # Status
//!
//! This is currently a stub. Differential pair routing requires:
//! - Coupled-line impedance calculations from the stackup
//! - Symmetric obstacle avoidance for both traces
//! - Phase matching (length matching between P and N traces)

use vcad_ir::Vec2;

use super::RouteResult;

/// Differential pair router.
///
/// Routes P/N trace pairs with controlled spacing for impedance matching.
pub struct DiffPairRouter {
    _trace_width: f64,
    _gap: f64,
}

impl DiffPairRouter {
    /// Create a new differential pair router.
    ///
    /// # Arguments
    ///
    /// * `trace_width` -- Width of each trace in the pair (mm).
    /// * `gap` -- Gap between the two traces (mm).
    pub fn new(trace_width: f64, gap: f64) -> Self {
        Self {
            _trace_width: trace_width,
            _gap: gap,
        }
    }

    /// Route a differential pair from start pads to end pads.
    ///
    /// # TODO
    ///
    /// Implement differential pair routing:
    /// 1. Route the center path from midpoint(start_p, start_n) to midpoint(end_p, end_n)
    /// 2. Offset the center path by +/- (trace_width/2 + gap/2) to get P and N paths
    /// 3. Handle corners with symmetric bends (mitered or curved)
    /// 4. Check for phase length matching
    /// 5. Insert meanders if needed for length matching
    pub fn route_pair(
        &mut self,
        net_p: &str,
        _net_n: &str,
        _start_p: Vec2,
        _start_n: Vec2,
        _end_p: Vec2,
        _end_n: Vec2,
    ) -> (RouteResult, RouteResult) {
        // TODO: implement differential pair routing algorithm
        let fail = RouteResult {
            net: net_p.to_string(),
            segments: vec![],
            vias: vec![],
            success: false,
        };
        (fail.clone(), fail)
    }
}
