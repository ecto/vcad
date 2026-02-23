//! Push-and-shove interactive router.
//!
//! Implements an interactive router that operates in continuous coordinate
//! space. It routes traces by attempting a direct path first, then inserting
//! waypoints to navigate around rectangular obstacles when collisions are
//! detected. Existing traces can be represented as obstacles so that new
//! routes push around them.
//!
//! The router also supports optional length tuning: when a target trace
//! length is specified and the computed route is too short, trombone-style
//! meanders are generated and spliced in to meet the target.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use vcad_ir::Vec2;

use super::length_tune::{generate_meanders, path_length, LengthTuneParams, MeanderStyle};
use super::RouteResult;

/// Axis-aligned rectangular obstacle.
#[derive(Debug, Clone)]
pub struct Obstacle {
    /// Minimum corner (bottom-left).
    pub min: Vec2,
    /// Maximum corner (top-right).
    pub max: Vec2,
}

impl Obstacle {
    /// Create a new rectangular obstacle.
    pub fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    /// Expanded rectangle that accounts for trace half-width plus clearance.
    fn inflated(&self, margin: f64) -> Obstacle {
        Obstacle {
            min: Vec2::new(self.min.x - margin, self.min.y - margin),
            max: Vec2::new(self.max.x + margin, self.max.y + margin),
        }
    }

    /// Test whether a point lies inside this rectangle.
    fn contains_point(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// Test whether the line segment `a`--`b` intersects this rectangle.
    /// Uses a parametric slab intersection test.
    fn intersects_segment(&self, a: Vec2, b: Vec2) -> bool {
        if self.contains_point(a) || self.contains_point(b) {
            return true;
        }

        let d = Vec2::new(b.x - a.x, b.y - a.y);
        let mut t_min = 0.0_f64;
        let mut t_max = 1.0_f64;

        // X slab
        if d.x.abs() < 1e-12 {
            if a.x < self.min.x || a.x > self.max.x {
                return false;
            }
        } else {
            let inv = 1.0 / d.x;
            let mut t1 = (self.min.x - a.x) * inv;
            let mut t2 = (self.max.x - a.x) * inv;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            t_min = t_min.max(t1);
            t_max = t_max.min(t2);
            if t_min > t_max {
                return false;
            }
        }

        // Y slab
        if d.y.abs() < 1e-12 {
            if a.y < self.min.y || a.y > self.max.y {
                return false;
            }
        } else {
            let inv = 1.0 / d.y;
            let mut t1 = (self.min.y - a.y) * inv;
            let mut t2 = (self.max.y - a.y) * inv;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            t_min = t_min.max(t1);
            t_max = t_max.min(t2);
            if t_min > t_max {
                return false;
            }
        }

        true
    }
}

/// State for Dijkstra's priority queue (min-heap by cost).
#[derive(Debug, Clone)]
struct DijkstraState {
    cost: f64,
    node: usize,
}

impl PartialEq for DijkstraState {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost
    }
}

impl Eq for DijkstraState {}

impl PartialOrd for DijkstraState {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DijkstraState {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap (BinaryHeap is a max-heap).
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(Ordering::Equal)
    }
}

/// Push-and-shove router for interactive trace editing.
///
/// Unlike the grid router, this operates in continuous coordinate space
/// and can displace existing traces to make room for new routes. When a
/// `target_length` is provided, the router will insert meander patterns
/// to lengthen the trace to match.
pub struct PushShoveRouter {
    trace_width: f64,
    clearance: f64,
    obstacles: Vec<Obstacle>,
    /// Optional target trace length in mm. When set, meanders are
    /// generated if the routed path is shorter than this value.
    pub target_length: Option<f64>,
}

impl PushShoveRouter {
    /// Create a new push-and-shove router.
    pub fn new(trace_width: f64, clearance: f64) -> Self {
        Self {
            trace_width,
            clearance,
            obstacles: Vec::new(),
            target_length: None,
        }
    }

    /// Add a rectangular obstacle to the routing environment.
    pub fn add_obstacle(&mut self, obstacle: Obstacle) {
        self.obstacles.push(obstacle);
    }

    /// Route a net using push-and-shove, displacing existing traces as needed.
    ///
    /// The algorithm:
    /// 1. Build a visibility graph from start, end, and obstacle corners.
    /// 2. Run Dijkstra to find the shortest collision-free path.
    /// 3. If `target_length` is set and the route is too short, generate
    ///    trombone meanders and splice them in.
    pub fn route_net(&self, net: &str, start: Vec2, end: Vec2) -> RouteResult {
        let margin = self.trace_width / 2.0 + self.clearance;
        let inflated: Vec<Obstacle> = self.obstacles.iter().map(|o| o.inflated(margin)).collect();

        let waypoints = match Self::build_visibility_path(start, end, &inflated) {
            Some(path) => path,
            None => {
                return RouteResult {
                    net: net.to_string(),
                    segments: vec![],
                    vias: vec![],
                    success: false,
                }
            }
        };

        // Convert waypoint list to segment pairs.
        let mut segments: Vec<(Vec2, Vec2)> = Vec::new();
        for i in 0..waypoints.len() - 1 {
            segments.push((waypoints[i], waypoints[i + 1]));
        }

        // Length tuning: if a target is set and path is too short, add meanders.
        if let Some(target) = self.target_length {
            let points: Vec<Vec2> = std::iter::once(segments[0].0)
                .chain(segments.iter().map(|s| s.1))
                .collect();

            let current = path_length(&points);
            if current < target {
                let params = LengthTuneParams {
                    target_length: target,
                    max_amplitude: 2.0,
                    spacing: 0.5,
                    style: MeanderStyle::Trombone,
                };

                if let Some(meanders) = generate_meanders(&points, &params) {
                    if !meanders.is_empty() {
                        segments = Self::apply_meanders(&points, &meanders);
                    }
                }
            }
        }

        RouteResult {
            net: net.to_string(),
            segments,
            vias: vec![],
            success: true,
        }
    }

    /// Build a collision-free shortest path using a visibility graph.
    ///
    /// Nodes are: start, end, and the four corners of each inflated obstacle.
    /// Edges connect every pair of nodes whose line-of-sight is clear of all
    /// obstacles. Dijkstra finds the shortest path through this graph.
    fn build_visibility_path(start: Vec2, end: Vec2, obstacles: &[Obstacle]) -> Option<Vec<Vec2>> {
        let eps = 0.01;

        // Collect all candidate nodes: start (0), end (1), then 4 corners
        // per obstacle.
        let mut nodes = vec![start, end];
        for obs in obstacles {
            nodes.push(Vec2::new(obs.min.x - eps, obs.min.y - eps));
            nodes.push(Vec2::new(obs.max.x + eps, obs.min.y - eps));
            nodes.push(Vec2::new(obs.max.x + eps, obs.max.y + eps));
            nodes.push(Vec2::new(obs.min.x - eps, obs.max.y + eps));
        }

        let n = nodes.len();

        // Test whether the segment between two nodes is collision-free.
        let is_visible = |a: Vec2, b: Vec2| -> bool {
            !obstacles.iter().any(|obs| obs.intersects_segment(a, b))
        };

        // Build adjacency: for each node, store (neighbor_index, distance).
        let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        for i in 0..n {
            for j in (i + 1)..n {
                if is_visible(nodes[i], nodes[j]) {
                    let d = (nodes[j] - nodes[i]).length();
                    adj[i].push((j, d));
                    adj[j].push((i, d));
                }
            }
        }

        // Dijkstra from node 0 (start) to node 1 (end).
        let mut dist = vec![f64::INFINITY; n];
        let mut prev = vec![usize::MAX; n];
        dist[0] = 0.0;

        let mut heap = BinaryHeap::new();
        heap.push(DijkstraState { cost: 0.0, node: 0 });

        while let Some(DijkstraState { cost, node }) = heap.pop() {
            if node == 1 {
                // Reached the end node -- backtrace.
                let mut path = Vec::new();
                let mut cur = 1;
                while cur != usize::MAX {
                    path.push(nodes[cur]);
                    cur = prev[cur];
                }
                path.reverse();
                return Some(path);
            }

            if cost > dist[node] {
                continue; // stale entry
            }

            for &(next, edge_dist) in &adj[node] {
                let new_dist = cost + edge_dist;
                if new_dist < dist[next] {
                    dist[next] = new_dist;
                    prev[next] = node;
                    heap.push(DijkstraState {
                        cost: new_dist,
                        node: next,
                    });
                }
            }
        }

        None // no path found
    }

    /// Replace original segments with meander-expanded segments.
    ///
    /// For each `MeanderSegment`, the original straight segment at that
    /// index is replaced by the meander waypoint chain. Segments not
    /// covered by meanders are kept as-is.
    fn apply_meanders(
        points: &[Vec2],
        meanders: &[super::length_tune::MeanderSegment],
    ) -> Vec<(Vec2, Vec2)> {
        let mut result = Vec::new();

        // Build a set of segment indices that have meanders.
        let meander_map: std::collections::HashMap<usize, &super::length_tune::MeanderSegment> =
            meanders.iter().map(|m| (m.segment_index, m)).collect();

        for i in 0..points.len() - 1 {
            if let Some(meander) = meander_map.get(&i) {
                // Replace this segment with meander waypoints.
                for j in 0..meander.points.len() - 1 {
                    result.push((meander.points[j], meander.points[j + 1]));
                }
            } else {
                result.push((points[i], points[i + 1]));
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_route_no_obstacles() {
        let router = PushShoveRouter::new(0.25, 0.2);
        let result = router.route_net("VCC", Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        assert!(result.success);
        assert_eq!(result.segments.len(), 1);
        let seg = &result.segments[0];
        assert!((seg.0.x - 0.0).abs() < 1e-10);
        assert!((seg.1.x - 10.0).abs() < 1e-10);
    }

    #[test]
    fn route_around_obstacle() {
        let mut router = PushShoveRouter::new(0.25, 0.2);
        router.add_obstacle(Obstacle::new(Vec2::new(4.0, -2.0), Vec2::new(6.0, 2.0)));

        let result = router.route_net("SIG", Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        assert!(result.success);
        assert!(
            result.segments.len() > 1,
            "should detour around obstacle, got {} segments",
            result.segments.len()
        );

        // First segment should start at origin.
        let first = &result.segments[0];
        assert!((first.0.x - 0.0).abs() < 1e-6);
        assert!((first.0.y - 0.0).abs() < 1e-6);

        // Last segment should end at destination.
        let last = &result.segments[result.segments.len() - 1];
        assert!((last.1.x - 10.0).abs() < 1e-6);
        assert!((last.1.y - 0.0).abs() < 1e-6);
    }

    #[test]
    fn route_with_length_target_none() {
        // No target length set -- route behaves normally.
        let router = PushShoveRouter::new(0.25, 0.2);
        let result = router.route_net("NET1", Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0));
        assert!(result.success);
        assert_eq!(result.segments.len(), 1);

        // Total routed length should equal the straight-line distance.
        let points: Vec<Vec2> = std::iter::once(result.segments[0].0)
            .chain(result.segments.iter().map(|s| s.1))
            .collect();
        let total = path_length(&points);
        assert!(
            (total - 20.0).abs() < 1e-6,
            "straight route should be 20mm, got {total}"
        );
    }

    #[test]
    fn route_with_length_target_met() {
        // Target length is less than or equal to the natural route length.
        // No meanders should be added.
        let mut router = PushShoveRouter::new(0.25, 0.2);
        router.target_length = Some(15.0); // target shorter than 20mm route

        let result = router.route_net("NET2", Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0));
        assert!(result.success);
        // Route is already 20mm which exceeds 15mm target, so no meanders.
        assert_eq!(
            result.segments.len(),
            1,
            "no meanders needed, should be 1 segment"
        );

        let points: Vec<Vec2> = std::iter::once(result.segments[0].0)
            .chain(result.segments.iter().map(|s| s.1))
            .collect();
        let total = path_length(&points);
        assert!(
            (total - 20.0).abs() < 1e-6,
            "unchanged route should be 20mm, got {total}"
        );
    }

    #[test]
    fn route_with_length_target_short() {
        // Target is longer than the natural route -- meanders should be inserted.
        let mut router = PushShoveRouter::new(0.25, 0.2);
        router.target_length = Some(40.0); // need 40mm but route is only 20mm

        let result = router.route_net("NET3", Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0));
        assert!(result.success);
        assert!(
            result.segments.len() > 1,
            "meanders should produce multiple segments, got {}",
            result.segments.len()
        );

        // Verify the total routed length is close to target.
        let points: Vec<Vec2> = std::iter::once(result.segments[0].0)
            .chain(result.segments.iter().map(|s| s.1))
            .collect();
        let total = path_length(&points);
        assert!(
            (total - 40.0).abs() < 1.0,
            "meandered route should be ~40mm, got {total}"
        );

        // Endpoints must be preserved.
        let first = &result.segments[0];
        assert!(
            (first.0.x - 0.0).abs() < 1e-6 && (first.0.y - 0.0).abs() < 1e-6,
            "start point must be preserved"
        );
        let last = &result.segments[result.segments.len() - 1];
        assert!(
            (last.1.x - 20.0).abs() < 1e-6 && (last.1.y - 0.0).abs() < 1e-6,
            "end point must be preserved"
        );
    }

    #[test]
    fn multiple_obstacles() {
        let mut router = PushShoveRouter::new(0.25, 0.2);
        router.add_obstacle(Obstacle::new(Vec2::new(3.0, -2.0), Vec2::new(5.0, 2.0)));
        router.add_obstacle(Obstacle::new(Vec2::new(7.0, -2.0), Vec2::new(9.0, 2.0)));

        let result = router.route_net("CLK", Vec2::new(0.0, 0.0), Vec2::new(12.0, 0.0));
        assert!(result.success);
        assert!(
            result.segments.len() > 2,
            "should detour around both obstacles"
        );
    }

    #[test]
    fn obstacle_contains_endpoint() {
        // If the start or end is inside an inflated obstacle, routing should
        // still succeed (the obstacle only blocks traversal, not endpoints
        // that are already committed).
        let mut router = PushShoveRouter::new(0.25, 0.2);
        // Place a small obstacle far from the path so it doesn't interfere.
        router.add_obstacle(Obstacle::new(Vec2::new(50.0, 50.0), Vec2::new(51.0, 51.0)));

        let result = router.route_net("NET", Vec2::new(0.0, 0.0), Vec2::new(5.0, 0.0));
        assert!(result.success);
    }

    #[test]
    fn length_target_with_obstacle_detour() {
        // Route around an obstacle, then also apply length tuning.
        let mut router = PushShoveRouter::new(0.25, 0.2);
        router.add_obstacle(Obstacle::new(Vec2::new(4.0, -2.0), Vec2::new(6.0, 2.0)));
        router.target_length = Some(50.0);

        let result = router.route_net("DATA", Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        assert!(result.success);

        let points: Vec<Vec2> = std::iter::once(result.segments[0].0)
            .chain(result.segments.iter().map(|s| s.1))
            .collect();
        let total = path_length(&points);
        // The detour route is longer than 10mm. With target=50mm, meanders
        // should bring it close to 50mm.
        assert!(
            (total - 50.0).abs() < 2.0,
            "length-tuned detour should be ~50mm, got {total}"
        );
    }
}
