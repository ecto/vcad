//! Trace routing algorithms for PCB layout.
//!
//! This module provides multiple routing strategies:
//!
//! - [`grid`] -- Lee/wave BFS-based grid router
//! - [`maze`] -- Single-net A* that avoids real copper via the incremental oracle
//! - [`push_shove`] -- Interactive push-and-shove router with visibility-graph pathfinding
//! - [`diff_pair`] -- Differential pair router with phase matching
//! - [`length_tune`] -- Length tuning meander generator with DRC-aware clearance checking

pub mod auto;
pub mod diff_pair;
pub mod grid;
pub mod length_tune;
pub mod maze;
pub mod push_shove;

pub use auto::{route_all, RouteAllResult, RoutedTrace, RoutedVia};
pub use maze::route_net_maze;

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::session::RouteSession;
use push_shove::{Obstacle, PushShoveRouter};

/// Route a single net on a board with the avoiding A* maze router.
///
/// Builds a [`RouteSession`] from `pcb` (so the route avoids every trace, pad,
/// and via already on `layer`, not just other-net trace bounding boxes) and
/// searches for a clearance-legal path. Convenience wrapper over
/// [`route_net_maze`] for one-shot single-net routing; to route many nets while
/// each avoids the ones before it, hold a `RouteSession` and commit between
/// calls.
pub fn route_net_maze_pcb(
    pcb: &Pcb,
    layer: PcbLayer,
    net: &str,
    start: Vec2,
    end: Vec2,
    width: f64,
) -> RouteResult {
    let session = RouteSession::from_pcb(pcb);
    route_net_maze(
        &session,
        &pcb.outline.vertices,
        layer,
        net,
        start,
        end,
        width,
    )
}

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

/// Route a single net on a board with the push-and-shove router.
///
/// Existing traces on **other** nets become rectangular obstacles, so the new
/// route detours around copper already on the board instead of crossing it —
/// the continuous-space counterpart to the grid router used by the basic
/// autorouter. Coordinates are board-space millimetres (the returned segments
/// are in the same frame), so callers don't need the grid router's
/// origin-offset bookkeeping.
///
/// `width` is the new trace's width; clearance is taken from the board's
/// default net-class rules.
pub fn route_net_push_shove(
    pcb: &Pcb,
    net: &str,
    start: Vec2,
    end: Vec2,
    width: f64,
) -> RouteResult {
    let clearance = pcb.rules.default_rules.clearance;
    let mut router = PushShoveRouter::new(width, clearance);

    for trace in &pcb.traces {
        if trace.net == net {
            continue;
        }
        let hw = trace.width * 0.5;
        let min = Vec2::new(
            trace.start.x.min(trace.end.x) - hw,
            trace.start.y.min(trace.end.y) - hw,
        );
        let max = Vec2::new(
            trace.start.x.max(trace.end.x) + hw,
            trace.start.y.max(trace.end.y) + hw,
        );
        router.add_obstacle(Obstacle::new(min, max));
    }

    router.route_net(net, start, end)
}

#[cfg(test)]
mod pcb_route_tests {
    use super::*;
    use vcad_ir::ecad::*;

    /// Bare board with a configurable trace list — enough to exercise the
    /// push-and-shove integration without the full footprint scaffolding.
    fn board(traces: Vec<Trace>) -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(20.0, 0.0),
                    Vec2::new(20.0, 20.0),
                    Vec2::new(0.0, 20.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: Some(1.5),
                    dielectric_er: Some(4.5),
                    material: Some("FR4".into()),
                }],
            },
            nets: vec![],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "Default".into(),
                    trace_width: 0.25,
                    clearance: 0.2,
                    via_diameter: 0.8,
                    via_drill: 0.4,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                },
                class_rules: vec![],
                net_class_assignments: Default::default(),
                edge_clearance: 0.5,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints: vec![],
            traces,
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn trace(net: &str, a: Vec2, b: Vec2) -> Trace {
        Trace {
            start: a,
            end: b,
            width: 0.25,
            layer: PcbLayer::FCu,
            net: net.into(),
        }
    }

    #[test]
    fn straight_route_when_board_is_empty() {
        let pcb = board(vec![]);
        let r = route_net_push_shove(&pcb, "SIG", Vec2::new(0.0, 5.0), Vec2::new(15.0, 5.0), 0.25);
        assert!(r.success);
        assert_eq!(r.segments.len(), 1, "no obstacles → one straight segment");
    }

    #[test]
    fn detours_around_a_trace_on_another_net() {
        // A GND trace straddles the straight path from start to end; the
        // router must shove the SIG route around it.
        let blocker = trace("GND", Vec2::new(7.5, 2.0), Vec2::new(7.5, 8.0));
        let pcb = board(vec![blocker]);
        let r = route_net_push_shove(&pcb, "SIG", Vec2::new(0.0, 5.0), Vec2::new(15.0, 5.0), 0.25);
        assert!(r.success);
        assert!(
            r.segments.len() > 1,
            "should detour around the GND trace, got {} segment(s)",
            r.segments.len()
        );
        // Endpoints are preserved.
        assert!((r.segments[0].0.x - 0.0).abs() < 1e-6);
        let last = r.segments.last().unwrap();
        assert!((last.1.x - 15.0).abs() < 1e-6);
    }

    #[test]
    fn ignores_obstacles_on_the_same_net() {
        // A same-net trace is not an obstacle — co-net copper may touch.
        let same = trace("SIG", Vec2::new(7.5, 2.0), Vec2::new(7.5, 8.0));
        let pcb = board(vec![same]);
        let r = route_net_push_shove(&pcb, "SIG", Vec2::new(0.0, 5.0), Vec2::new(15.0, 5.0), 0.25);
        assert!(r.success);
        assert_eq!(r.segments.len(), 1, "same-net copper is not shoved around");
    }
}
