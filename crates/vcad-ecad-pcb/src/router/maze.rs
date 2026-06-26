//! Maze router — single-net A* that genuinely avoids copper.
//!
//! The honest intermediate between the straight-line autorouter and full
//! topological routing. Where [`super::push_shove`] detours around *static
//! inflated bounding boxes* of other-net traces, this router searches an 8-way
//! grid and tests every candidate step against the exact incremental oracle
//! ([`RouteSession::probe`]) — the same clearance geometry the DRC clearance
//! pass uses. So a route it returns avoids *all* copper on the layer (traces,
//! pads, vias), and every emitted segment is clearance-legal by construction.
//!
//! It is deliberately not the topological engine (no rip-up, no layer changes):
//! it is the first router in vcad that *avoids* instead of *drawing-then-flagging*.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use vcad_ir::ecad::PcbLayer;
use vcad_ir::Vec2;

use crate::session::RouteSession;
use crate::spatial::{point_in_polygon, CopperGeom};

use super::congestion::Congestion;
use super::RouteResult;

/// Largest grid dimension along either axis; pitch is coarsened to fit.
const MAX_DIM: usize = 400;

/// Cost added each time the route changes direction, in pitch units. Keeps
/// routes straight (fewer, longer segments) rather than staircasing.
const BEND_PENALTY: f64 = 0.6;

/// Route a single net from `start` to `end` on `layer`, avoiding all other-net
/// copper currently in `session`.
///
/// `outline` (the board polygon) bounds the search when it has ≥3 vertices;
/// pass an empty slice to route within the start/end bounding box. The required
/// clearance is taken from the session's design rules for `net`.
///
/// On success the result's `segments` form a clearance-legal polyline from
/// `start` to `end`; on failure `success` is false and `segments` is empty.
///
/// Congestion-unaware convenience entry point: routes with a flat history field
/// (the cheapest path that clears all copper). Use [`route_net_maze_cong`] to
/// bias the search away from contested regions during negotiated routing.
pub fn route_net_maze(
    session: &RouteSession,
    outline: &[Vec2],
    layer: PcbLayer,
    net: &str,
    start: Vec2,
    end: Vec2,
    width: f64,
) -> RouteResult {
    route_net_maze_cong(session, outline, layer, net, start, end, width, None)
}

/// Route a single net, optionally biased by a PathFinder history-cost field.
///
/// Identical to [`route_net_maze`] but the A* step cost includes
/// `congestion.cost_at(cell)` for every cell entered, so a route prefers to
/// detour around regions the negotiation loop has marked as persistently
/// contested. Legality is unchanged — congestion only adds cost, never relaxes
/// the clearance constraint — so every emitted segment is still DRC-clean.
#[allow(clippy::too_many_arguments)]
pub fn route_net_maze_cong(
    session: &RouteSession,
    outline: &[Vec2],
    layer: PcbLayer,
    net: &str,
    start: Vec2,
    end: Vec2,
    width: f64,
    congestion: Option<&Congestion>,
) -> RouteResult {
    let clearance = session.clearance_for(net);
    let half_w = width / 2.0;

    let grid = Grid::new(outline, start, end, width, clearance);
    let (sx, sy) = grid.snap(start);
    let (ex, ey) = grid.snap(end);
    let start_node = grid.index(sx, sy);
    let goal_node = grid.index(ex, ey);

    let legal_step = |a: Vec2, b: Vec2| -> bool {
        session
            .probe(&CopperGeom::Segment { a, b, half_w }, layer, net, clearance)
            .legal
    };

    // Cell passability: inside the board (if bounded) and not sitting on copper.
    let cell_ok = |ix: usize, iy: usize| -> bool {
        let p = grid.world(ix, iy);
        if grid.bounded && !point_in_polygon(p, outline) {
            return false;
        }
        legal_step(p, p)
    };

    // History cost of entering a node (0 when the field is flat / absent), so a
    // flat field reproduces the congestion-unaware search exactly.
    let cong = congestion.filter(|c| !c.is_flat());
    let node_cost =
        |node: usize| -> f64 { cong.map(|c| c.cost_at(grid.world_of(node))).unwrap_or(0.0) };

    let path_nodes = astar(
        &grid,
        start_node,
        goal_node,
        &cell_ok,
        &|a, b| legal_step(grid.world_of(a), grid.world_of(b)),
        &node_cost,
    );

    let Some(nodes) = path_nodes else {
        return RouteResult {
            net: net.to_string(),
            segments: Vec::new(),
            vias: Vec::new(),
            success: false,
        };
    };

    // Grid nodes -> world points, then collapse collinear runs.
    let mut pts: Vec<Vec2> = Vec::with_capacity(nodes.len() + 2);
    pts.push(start);
    for &n in &nodes {
        let w = grid.world_of(n);
        if pts.last().map(|p| dist(*p, w) > 1e-9).unwrap_or(true) {
            pts.push(w);
        }
    }
    if pts.last().map(|p| dist(*p, end) > 1e-9).unwrap_or(true) {
        pts.push(end);
    }
    let pts = simplify(&pts);

    let segments: Vec<(Vec2, Vec2)> = pts.windows(2).map(|w| (w[0], w[1])).collect();

    // Final guarantee: re-probe every emitted segment. The grid search probes
    // node-to-node edges, but the connector from the last grid node to the
    // exact endpoint pad is off-grid and unprobed — on a crowded board it can
    // graze a neighbouring net's copper. If any segment is illegal, fail
    // honestly so the caller can try another layer rather than ship a short.
    if !segments.iter().all(|(a, b)| legal_step(*a, *b)) {
        return RouteResult {
            net: net.to_string(),
            segments: Vec::new(),
            vias: Vec::new(),
            success: false,
        };
    }

    RouteResult {
        net: net.to_string(),
        segments,
        vias: Vec::new(),
        success: true,
    }
}

/// A uniform routing grid over the board bounding box.
struct Grid {
    origin: Vec2,
    pitch: f64,
    nx: usize,
    ny: usize,
    bounded: bool,
}

impl Grid {
    fn new(outline: &[Vec2], start: Vec2, end: Vec2, width: f64, clearance: f64) -> Self {
        let (mut min, mut max) = if outline.len() >= 3 {
            bbox(outline)
        } else {
            // Fall back to the start/end span with a margin.
            let lo = Vec2::new(start.x.min(end.x) - 5.0, start.y.min(end.y) - 5.0);
            let hi = Vec2::new(start.x.max(end.x) + 5.0, start.y.max(end.y) + 5.0);
            (lo, hi)
        };
        // Ensure both endpoints are inside the grid.
        min.x = min.x.min(start.x).min(end.x);
        min.y = min.y.min(start.y).min(end.y);
        max.x = max.x.max(start.x).max(end.x);
        max.y = max.y.max(start.y).max(end.y);

        let span_x = (max.x - min.x).max(1e-3);
        let span_y = (max.y - min.y).max(1e-3);
        let mut pitch = (width + clearance).max(0.05);
        // Coarsen so neither axis exceeds MAX_DIM cells.
        let need = (span_x / pitch).max(span_y / pitch);
        if need > MAX_DIM as f64 {
            pitch = (span_x / MAX_DIM as f64).max(span_y / MAX_DIM as f64);
        }
        // Align the origin so `start` lands exactly on a grid node (and stays
        // ≤ the board minimum). Endpoints then sit on node lines, so the
        // connector from the last node to the exact endpoint runs along an axis
        // and collapses into the route instead of adding a staircase kink.
        let kx = ((start.x - min.x) / pitch).ceil();
        let ky = ((start.y - min.y) / pitch).ceil();
        let origin = Vec2::new(start.x - kx * pitch, start.y - ky * pitch);
        let nx = ((max.x - origin.x) / pitch).ceil() as usize + 1;
        let ny = ((max.y - origin.y) / pitch).ceil() as usize + 1;
        Self {
            origin,
            pitch,
            nx,
            ny,
            bounded: outline.len() >= 3,
        }
    }

    fn index(&self, ix: usize, iy: usize) -> usize {
        iy * self.nx + ix
    }

    fn coords(&self, node: usize) -> (usize, usize) {
        (node % self.nx, node / self.nx)
    }

    fn world(&self, ix: usize, iy: usize) -> Vec2 {
        Vec2::new(
            self.origin.x + ix as f64 * self.pitch,
            self.origin.y + iy as f64 * self.pitch,
        )
    }

    fn world_of(&self, node: usize) -> Vec2 {
        let (ix, iy) = self.coords(node);
        self.world(ix, iy)
    }

    fn snap(&self, p: Vec2) -> (usize, usize) {
        let ix = (((p.x - self.origin.x) / self.pitch).round() as i64).clamp(0, self.nx as i64 - 1);
        let iy = (((p.y - self.origin.y) / self.pitch).round() as i64).clamp(0, self.ny as i64 - 1);
        (ix as usize, iy as usize)
    }
}

/// A* search state ordered by f-cost for a min-heap (via [`BinaryHeap`]).
struct State {
    f: f64,
    node: usize,
}
impl PartialEq for State {
    fn eq(&self, o: &Self) -> bool {
        self.f == o.f && self.node == o.node
    }
}
impl Eq for State {}
impl Ord for State {
    fn cmp(&self, o: &Self) -> Ordering {
        // Reversed: BinaryHeap is a max-heap, we want the smallest f first.
        o.f.total_cmp(&self.f).then(self.node.cmp(&o.node))
    }
}
impl PartialOrd for State {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// 8-connected A* on the grid. `cell_ok` tests node passability; `edge_ok`
/// tests whether the step between two nodes is clearance-legal; `node_cost`
/// adds a PathFinder history penalty for entering a node (0 when uncongested).
fn astar(
    grid: &Grid,
    start: usize,
    goal: usize,
    cell_ok: &dyn Fn(usize, usize) -> bool,
    edge_ok: &dyn Fn(usize, usize) -> bool,
    node_cost: &dyn Fn(usize) -> f64,
) -> Option<Vec<usize>> {
    let n = grid.nx * grid.ny;
    if start >= n || goal >= n {
        return None;
    }
    let (gx, gy) = grid.coords(goal);
    let heuristic = |node: usize| -> f64 {
        let (ix, iy) = grid.coords(node);
        let dx = ix as f64 - gx as f64;
        let dy = iy as f64 - gy as f64;
        (dx * dx + dy * dy).sqrt() * grid.pitch
    };

    let mut g = vec![f64::INFINITY; n];
    let mut came: Vec<usize> = vec![usize::MAX; n];
    let mut closed = vec![false; n];
    let mut heap = BinaryHeap::new();
    g[start] = 0.0;
    heap.push(State {
        f: heuristic(start),
        node: start,
    });

    while let Some(State { node, .. }) = heap.pop() {
        if node == goal {
            return Some(reconstruct(came, start, goal));
        }
        if closed[node] {
            continue;
        }
        closed[node] = true;
        let (ix, iy) = grid.coords(node);
        // Incoming direction (for the bend penalty).
        let in_dir = if came[node] == usize::MAX {
            (0i64, 0i64)
        } else {
            let (px, py) = grid.coords(came[node]);
            (ix as i64 - px as i64, iy as i64 - py as i64)
        };

        for (dx, dy) in NEIGHBORS {
            let nx = ix as i64 + dx;
            let ny = iy as i64 + dy;
            if nx < 0 || ny < 0 || nx >= grid.nx as i64 || ny >= grid.ny as i64 {
                continue;
            }
            let (nix, niy) = (nx as usize, ny as usize);
            let nb = grid.index(nix, niy);
            if closed[nb] || !cell_ok(nix, niy) || !edge_ok(node, nb) {
                continue;
            }
            let step = if dx == 0 || dy == 0 {
                grid.pitch
            } else {
                grid.pitch * std::f64::consts::SQRT_2
            };
            let bend = if in_dir != (0, 0) && in_dir != (dx, dy) {
                BEND_PENALTY * grid.pitch
            } else {
                0.0
            };
            // PathFinder history penalty for routing through this cell.
            let tentative = g[node] + step + bend + node_cost(nb);
            if tentative < g[nb] {
                g[nb] = tentative;
                came[nb] = node;
                heap.push(State {
                    f: tentative + heuristic(nb),
                    node: nb,
                });
            }
        }
    }
    None
}

/// The 8 grid neighbor offsets.
const NEIGHBORS: [(i64, i64); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

fn reconstruct(came: Vec<usize>, start: usize, goal: usize) -> Vec<usize> {
    let mut path = vec![goal];
    let mut cur = goal;
    while cur != start {
        cur = came[cur];
        if cur == usize::MAX {
            break;
        }
        path.push(cur);
    }
    path.reverse();
    path
}

/// Drop interior points that are collinear with their neighbors.
fn simplify(pts: &[Vec2]) -> Vec<Vec2> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let mut out = vec![pts[0]];
    for i in 1..pts.len() - 1 {
        let a = *out.last().unwrap();
        let b = pts[i];
        let c = pts[i + 1];
        let cross = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        if cross.abs() > 1e-9 {
            out.push(b);
        }
    }
    out.push(*pts.last().unwrap());
    out
}

fn bbox(poly: &[Vec2]) -> (Vec2, Vec2) {
    let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in poly {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    (min, max)
}

fn dist(a: Vec2, b: Vec2) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::RouteSession;
    use vcad_ir::ecad::*;

    fn board(traces: Vec<Trace>) -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(40.0, 0.0),
                    Vec2::new(40.0, 40.0),
                    Vec2::new(0.0, 40.0),
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

    /// Every returned segment must itself probe legal — the avoidance guarantee.
    fn all_segments_legal(session: &RouteSession, r: &RouteResult, net: &str, width: f64) -> bool {
        let clr = session.clearance_for(net);
        r.segments.iter().all(|(a, b)| {
            session
                .probe(
                    &CopperGeom::Segment {
                        a: *a,
                        b: *b,
                        half_w: width / 2.0,
                    },
                    PcbLayer::FCu,
                    net,
                    clr,
                )
                .legal
        })
    }

    #[test]
    fn routes_straight_on_empty_board() {
        let pcb = board(vec![]);
        let session = RouteSession::from_pcb(&pcb);
        let r = route_net_maze(
            &session,
            &pcb.outline.vertices,
            PcbLayer::FCu,
            "SIG",
            Vec2::new(5.0, 20.0),
            Vec2::new(35.0, 20.0),
            0.25,
        );
        assert!(r.success);
        assert!(all_segments_legal(&session, &r, "SIG", 0.25));
        // A clear shot should be one (or very few) straight segment(s).
        assert!(r.segments.len() <= 2, "got {} segments", r.segments.len());
    }

    #[test]
    fn detours_around_a_wall_with_a_gap() {
        // A GND wall spans most of the board at x=20 with a gap near the top;
        // the router must thread the gap, and every segment must be legal.
        let pcb = board(vec![trace(
            "GND",
            Vec2::new(20.0, 0.0),
            Vec2::new(20.0, 32.0),
        )]);
        let session = RouteSession::from_pcb(&pcb);
        let r = route_net_maze(
            &session,
            &pcb.outline.vertices,
            PcbLayer::FCu,
            "SIG",
            Vec2::new(5.0, 16.0),
            Vec2::new(35.0, 16.0),
            0.25,
        );
        assert!(r.success, "should find a path through the gap");
        assert!(r.segments.len() > 1, "must detour, not go straight");
        assert!(
            all_segments_legal(&session, &r, "SIG", 0.25),
            "every routed segment must clear the GND wall"
        );
    }

    #[test]
    fn fails_when_fully_walled_off() {
        // A GND wall spanning the whole board height isolates start from end.
        let pcb = board(vec![trace(
            "GND",
            Vec2::new(20.0, 0.0),
            Vec2::new(20.0, 40.0),
        )]);
        let session = RouteSession::from_pcb(&pcb);
        let r = route_net_maze(
            &session,
            &pcb.outline.vertices,
            PcbLayer::FCu,
            "SIG",
            Vec2::new(5.0, 20.0),
            Vec2::new(35.0, 20.0),
            0.25,
        );
        assert!(!r.success, "a full wall must make routing fail honestly");
        assert!(r.segments.is_empty());
    }
}
