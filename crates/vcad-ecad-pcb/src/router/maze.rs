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
//! It is deliberately not the topological engine (no rip-up):
//! it is the first router in vcad that *avoids* instead of *drawing-then-flagging*.
//!
//! Two searches live here: the single-layer [`route_net_maze`] and the
//! layer-aware [`route_net_maze3d`], whose A* runs over `(x, y, layer)` with
//! via transitions as costed edges — the search *chooses* where to change
//! layers instead of being handed a layer by the caller. Via legality is
//! probed on every copper layer the (through) via spans, so a 3D route is as
//! DRC-clean as a 2D one.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use vcad_ir::ecad::PcbLayer;
use vcad_ir::Vec2;

use crate::session::RouteSession;
use crate::spatial::{point_in_polygon, CopperGeom};

use super::congestion::Congestion;
use super::{RouteResult, Stopwatch};

/// Largest grid dimension along either axis; pitch is coarsened to fit.
const MAX_DIM: usize = 400;

/// Cost added each time the route changes direction, in pitch units. Keeps
/// routes straight (fewer, longer segments) rather than staircasing.
const BEND_PENALTY: f64 = 0.6;

/// Cost (mm-equivalent) of placing a via, i.e. taking a layer-transition edge
/// in the 3D search. A through via consumes routing space on *every* copper
/// layer, so it must cost noticeably more than a modest same-layer detour —
/// but not so much that the search never escapes a genuinely walled-off layer.
pub const VIA_COST: f64 = 4.0;

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

/// Result of a layer-aware maze route: segments each carrying their copper
/// layer, plus the (through) via positions where the route transitions.
#[derive(Debug, Clone)]
pub struct RouteResult3d {
    /// Net name that was routed.
    pub net: String,
    /// Routed segments as `(start, end, layer)`.
    pub segments: Vec<(Vec2, Vec2, PcbLayer)>,
    /// Vias where the route changes layer: `(position, from_layer, to_layer)`
    /// — the two copper layers the via must connect (its minimal span). A
    /// same-layer pair never occurs; the outer layers give a through via,
    /// anything else a blind/buried via.
    pub vias: Vec<(Vec2, PcbLayer, PcbLayer)>,
    /// Whether routing succeeded.
    pub success: bool,
}

impl RouteResult3d {
    fn fail(net: &str) -> Self {
        Self {
            net: net.to_string(),
            segments: Vec::new(),
            vias: Vec::new(),
            success: false,
        }
    }
}

/// Layer-aware A* over `(x, y, layer)`: routes `net` from `start` to `end`
/// across `layers` (the board's copper stack, front → back), choosing via
/// positions as part of the search.
///
/// `start_layers` / `end_layers` are the copper layers the endpoints' pads
/// actually live on (a front SMD pad is `[FCu]`, a through-hole pad is every
/// copper layer); the route must begin and end on one of them — transitioning
/// away from an endpoint drops a via *at* the pad exactly like the historical
/// two-layer router. Via legality is probed on every layer in `layers` (all
/// vias are through vias for now), and every emitted segment is re-probed on
/// its own layer before the result is trusted.
///
/// `max_expansions` bounds the number of A* node pops (0 = unbounded): an
/// unroutable connection otherwise floods the entire `(x, y, layer)` space —
/// millions of oracle probes — before failing. The budget converts "prove
/// impossibility exhaustively" into "give up after a fair search", which is
/// what rip-up and negotiation want anyway.
///
/// `pitch_scale` scales the grid pitch below the default `width + clearance`
/// (1.0 = default; 0.5 = half pitch, with the grid-size cap doubled to
/// match). On HDI boards the clear channel between BGA pads can be narrower
/// than the default pitch — the coarse grid then has no node inside a
/// perfectly routable channel. Callers retry failures at 0.5.
#[allow(clippy::too_many_arguments)]
pub fn route_net_maze3d(
    session: &RouteSession,
    outline: &[Vec2],
    layers: &[PcbLayer],
    net: &str,
    start: Vec2,
    start_layers: &[PcbLayer],
    end: Vec2,
    end_layers: &[PcbLayer],
    width: f64,
    via_diameter: f64,
    congestion: Option<&Congestion>,
    max_expansions: usize,
    pitch_scale: f64,
    window: Option<(Vec2, Vec2)>,
    tree_goals: &[(CopperGeom, [f64; 2], [f64; 2], PcbLayer)],
) -> RouteResult3d {
    let nl = layers.len();
    if nl == 0 {
        return RouteResult3d::fail(net);
    }
    let clearance = session.clearance_for(net);
    let half_w = width / 2.0;
    let via_r = via_diameter / 2.0;

    let grid = Grid::new_scaled(outline, start, end, width, clearance, pitch_scale, window);
    let plane = grid.nx * grid.ny;
    let (sx, sy) = grid.snap(start);
    let (ex, ey) = grid.snap(end);

    let layer_idx = |l: PcbLayer| layers.iter().position(|&x| x == l);
    let start_lis: Vec<usize> = start_layers.iter().filter_map(|&l| layer_idx(l)).collect();
    let end_lis: Vec<usize> = end_layers.iter().filter_map(|&l| layer_idx(l)).collect();
    if start_lis.is_empty() || end_lis.is_empty() {
        return RouteResult3d::fail(net);
    }

    let legal_step = |a: Vec2, b: Vec2, layer: PcbLayer| -> bool {
        session
            .probe(&CopperGeom::Segment { a, b, half_w }, layer, net, clearance)
            .legal
    };
    // A via at `p` spanning layer indices `a..=b` must clear other-net
    // copper on every spanned layer (and only those — a microvia between
    // adjacent layers doesn't care about copper elsewhere in the stack).
    let via_ok = |p: Vec2, a: usize, b: usize| -> bool {
        let disc = CopperGeom::Disc {
            center: p,
            r: via_r,
        };
        layers[a.min(b)..=a.max(b)]
            .iter()
            .all(|&l| session.probe(&disc, l, net, clearance).legal)
    };
    let cong = congestion.filter(|c| !c.is_flat());
    let node_cost =
        |cell: usize| -> f64 { cong.map(|c| c.cost_at(grid.world_of(cell))).unwrap_or(0.0) };

    // --- A* over (cell, layer) -------------------------------------------
    let n = plane * nl;
    let heuristic = |node: usize| -> f64 {
        let cell = node % plane;
        let (ix, iy) = grid.coords(cell);
        let dx = ix as f64 - ex as f64;
        let dy = iy as f64 - ey as f64;
        (dx * dx + dy * dy).sqrt() * grid.pitch
    };

    let mut g = vec![f64::INFINITY; n];
    let mut came: Vec<usize> = vec![usize::MAX; n];
    let mut closed = vec![false; n];
    // Memoized via legality per (cell, span pair) — spans are (lo, hi)
    // layer-index pairs, lo < hi — and the via POSITION chosen for the cell:
    // the cell centre when it clears, otherwise an off-grid candidate found
    // by ring search. Grid-quantized via sites lose exactly the positions a
    // human uses inside a BGA field (between the balls); the candidates
    // recover them.
    let mut via_cache: Vec<i8> = vec![-1; plane * nl * nl];
    let mut via_pos: Vec<Vec2> = vec![Vec2::new(0.0, 0.0); plane * nl * nl];
    // A candidate must clear its span AND connect legally to the cell centre
    // on both endpoint layers (the search continues from the cell node).
    let find_via_site = |center: Vec2, lo: usize, hi: usize| -> Option<Vec2> {
        if via_ok(center, lo, hi) {
            return Some(center);
        }
        let r1 = grid.pitch / 3.0;
        let r2 = grid.pitch / 2.0;
        for r in [r1, r2] {
            for k in 0..8 {
                let a = std::f64::consts::TAU * k as f64 / 8.0;
                let p = Vec2::new(center.x + r * a.cos(), center.y + r * a.sin());
                if via_ok(p, lo, hi)
                    && legal_step(center, p, layers[lo])
                    && legal_step(center, p, layers[hi])
                {
                    return Some(p);
                }
            }
        }
        None
    };
    // Occupancy raster: O(1) cell passability, and WIDE↔WIDE edges are legal
    // without touching the oracle at all.
    let sw_raster = Stopwatch::start();
    let raster = Raster::build(session, &grid, outline, layers, net, half_w);
    let raster_ms = sw_raster.ms();
    let sw_search = Stopwatch::start();
    let mut heap = BinaryHeap::new();

    let goal_cell = grid.index(ex, ey);
    // Route-to-tree goals: any cell whose centre lands ON copper of the
    // target pad's connected component (overlap, not proximity — a trace
    // ending there is electrically joined). Terminating on the tree needs no
    // endpoint via: the copper is already on that layer.
    let mut tree_goal = vec![false; n];
    for (geom, emin, emax, glayer) in tree_goals {
        let Some(li) = layers.iter().position(|l| l == glayer) else {
            continue;
        };
        let ix0 = (((emin[0] - grid.origin.x) / grid.pitch).floor()).max(0.0) as usize;
        let iy0 = (((emin[1] - grid.origin.y) / grid.pitch).floor()).max(0.0) as usize;
        let ix1 = ((((emax[0] - grid.origin.x) / grid.pitch).ceil()) as usize)
            .min(grid.nx.saturating_sub(1));
        let iy1 = ((((emax[1] - grid.origin.y) / grid.pitch).ceil()) as usize)
            .min(grid.ny.saturating_sub(1));
        for iy in iy0..=iy1 {
            for ix in ix0..=ix1 {
                let probe_pt = CopperGeom::Disc {
                    center: grid.world(ix, iy),
                    r: 0.0,
                };
                // Strict overlap certificate: a trace disc (radius half_w) at
                // this cell centre physically overlaps the tree element, so
                // terminating here is guaranteed electrical contact. Elements
                // smaller than a grid cell may mark no cells — that is the
                // safe direction (degrades to the exact pad goal); loosening
                // the test would let routes "connect" to copper they never
                // touch.
                if geom.distance_to(&probe_pt) < half_w {
                    tree_goal[li * plane + grid.index(ix, iy)] = true;
                }
            }
        }
    }
    let is_goal = |node: usize| {
        (node % plane == goal_cell && end_lis.contains(&(node / plane))) || tree_goal[node]
    };

    for &li in &start_lis {
        let node = li * plane + grid.index(sx, sy);
        g[node] = 0.0;
        heap.push(State {
            f: heuristic(node),
            node,
        });
    }

    let mut found: Option<usize> = None;
    let mut pops: usize = 0;
    while let Some(State { node, .. }) = heap.pop() {
        if is_goal(node) {
            found = Some(node);
            break;
        }
        if closed[node] {
            continue;
        }
        pops += 1;
        if max_expansions > 0 && pops > max_expansions {
            break;
        }
        closed[node] = true;
        let (li, cell) = (node / plane, node % plane);
        let (ix, iy) = grid.coords(cell);
        // Incoming direction for the bend penalty; a via resets it.
        let in_dir = if came[node] == usize::MAX || came[node] % plane == cell {
            (0i64, 0i64)
        } else {
            let (px, py) = grid.coords(came[node] % plane);
            (ix as i64 - px as i64, iy as i64 - py as i64)
        };

        // In-plane moves.
        for (dx, dy) in NEIGHBORS {
            let nx = ix as i64 + dx;
            let ny = iy as i64 + dy;
            if nx < 0 || ny < 0 || nx >= grid.nx as i64 || ny >= grid.ny as i64 {
                continue;
            }
            let (nix, niy) = (nx as usize, ny as usize);
            let ncell = grid.index(nix, niy);
            let nb = li * plane + ncell;
            if closed[nb] || raster.state(ncell, li) == CELL_BLOCKED {
                continue;
            }
            // WIDE↔WIDE edges are clearance-legal by construction; anything
            // touching a TIGHT cell still pays for an exact oracle probe.
            let tight = raster.state(cell, li) != CELL_WIDE || raster.state(ncell, li) != CELL_WIDE;
            if tight && !legal_step(grid.world(ix, iy), grid.world(nix, niy), layers[li]) {
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
            let tentative = g[node] + step + bend + node_cost(nb % plane);
            if tentative < g[nb] {
                g[nb] = tentative;
                came[nb] = node;
                heap.push(State {
                    f: tentative + heuristic(nb),
                    node: nb,
                });
            }
        }

        // Via transitions: same cell, any other layer. The via spans exactly
        // the layers between the two endpoints (blind/buried supported), is
        // probed only on that span, and costs more the deeper it goes — so
        // the search prefers a microvia hop over a stack-piercing through
        // via when both work.
        if nl > 1 {
            for lj in 0..nl {
                if lj == li {
                    continue;
                }
                let nb = lj * plane + cell;
                if closed[nb] || raster.state(cell, lj) == CELL_BLOCKED {
                    continue;
                }
                let (lo, hi) = (li.min(lj), li.max(lj));
                let key = cell * nl * nl + lo * nl + hi;
                if via_cache[key] < 0 {
                    match find_via_site(grid.world(ix, iy), lo, hi) {
                        Some(p) => {
                            via_cache[key] = 1;
                            via_pos[key] = p;
                        }
                        None => via_cache[key] = 0,
                    }
                }
                if via_cache[key] != 1 {
                    continue;
                }
                let span_frac = (hi - lo) as f64 / (nl - 1).max(1) as f64;
                let tentative = g[node] + VIA_COST * (0.4 + 0.6 * span_frac);
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
    }

    let search_ms = sw_search.ms();
    if search_ms + raster_ms > 200.0 {
        log::debug!(
            "maze3d slow: net={net} {}x{}x{nl} pitch={:.3} scale={pitch_scale:.2} \
             pops={pops}/{max_expansions} raster={raster_ms:.0}ms search={search_ms:.0}ms found={}",
            grid.nx,
            grid.ny,
            grid.pitch,
            found.is_some(),
        );
    } else {
        log::trace!(
            "maze3d: net={net} pops={pops} raster={raster_ms:.0}ms search={search_ms:.0}ms found={}",
            found.is_some(),
        );
    }
    let Some(goal_node) = found else {
        return RouteResult3d::fail(net);
    };

    // --- Reconstruct: per-layer polylines + vias at transitions -----------
    let mut nodes = vec![goal_node];
    let mut cur = goal_node;
    while came[cur] != usize::MAX {
        cur = came[cur];
        nodes.push(cur);
    }
    nodes.reverse();

    let mut segments: Vec<(Vec2, Vec2, PcbLayer)> = Vec::new();
    let mut vias: Vec<(Vec2, PcbLayer, PcbLayer)> = Vec::new();
    let mut run: Vec<Vec2> = vec![start];
    let mut run_layer = nodes[0] / plane;

    // Taut pull: greedy line-of-sight shortcutting over a run's points. A
    // grid path staircases; every removed kink both shortens the copper and
    // frees channel space the next net needs. Each shortcut is probed, so
    // legality is preserved by construction.
    let pull_taut = |pts: &[Vec2], layer: PcbLayer| -> Vec<Vec2> {
        if pts.len() <= 2 {
            return pts.to_vec();
        }
        let mut out = vec![pts[0]];
        let mut i = 0;
        while i + 1 < pts.len() {
            // Furthest j with a clear legal segment i→j.
            let mut j = i + 1;
            for k in ((i + 2)..pts.len()).rev() {
                if legal_step(pts[i], pts[k], layer) {
                    j = k;
                    break;
                }
            }
            out.push(pts[j]);
            i = j;
        }
        out
    };
    let flush_run = |run: &mut Vec<Vec2>, layer: PcbLayer, segs: &mut Vec<_>| {
        let pts = pull_taut(&simplify(run), layer);
        segs.extend(
            pts.windows(2)
                .filter(|w| dist(w[0], w[1]) > 1e-9)
                .map(|w| (w[0], w[1], layer)),
        );
        run.clear();
    };

    for &node in &nodes {
        let (li, cell) = (node / plane, node % plane);
        let w = grid.world_of(cell);
        if li != run_layer {
            // Layer change: the via sits at the position the search chose for
            // this (cell, span) — the cell centre, or an off-grid candidate
            // between obstacles. Route the old-layer run to it and start the
            // new-layer run from it.
            let (lo, hi) = (run_layer.min(li), run_layer.max(li));
            let key = cell * nl * nl + lo * nl + hi;
            let at = if via_cache[key] == 1 {
                via_pos[key]
            } else {
                *run.last().expect("run always starts non-empty")
            };
            if run.last().map(|p| dist(*p, at) > 1e-9).unwrap_or(true) {
                run.push(at);
            }
            flush_run(&mut run, layers[run_layer], &mut segments);
            if vias
                .last()
                .map(|&(v, _, _)| dist(v, at) > 1e-9)
                .unwrap_or(true)
            {
                vias.push((at, layers[lo], layers[hi]));
            }
            run.push(at);
            run_layer = li;
        }
        if run.last().map(|p| dist(*p, w) > 1e-9).unwrap_or(true) {
            run.push(w);
        }
    }
    let landed_on_tree = tree_goal[goal_node] && goal_node % plane != grid.index(ex, ey);
    if !landed_on_tree && run.last().map(|p| dist(*p, end) > 1e-9).unwrap_or(true) {
        run.push(end);
    }
    flush_run(&mut run, layers[run_layer], &mut segments);

    // Final guarantee: re-probe every emitted segment on its own layer and
    // every via on every layer (endpoint connectors are off-grid; see the
    // 2D router). Fail honestly rather than ship a short.
    let span_idx = |l: PcbLayer| layers.iter().position(|&x| x == l).unwrap_or(0);
    if !segments.iter().all(|(a, b, l)| legal_step(*a, *b, *l))
        || !vias
            .iter()
            .all(|&(v, la, lb)| via_ok(v, span_idx(la), span_idx(lb)))
    {
        return RouteResult3d::fail(net);
    }

    RouteResult3d {
        net: net.to_string(),
        segments,
        vias,
        success: true,
    }
}

/// Per-cell occupancy state in a [`Raster`].
const CELL_BLOCKED: u8 = 0;
/// Free, but with little slack — edges into this cell need an exact probe.
const CELL_TIGHT: u8 = 1;
/// Free with ≥ `pitch·√2/2` of extra slack: any edge between two WIDE cells
/// is clearance-legal without probing (the capsule between adjacent centers
/// never strays farther than that from a center).
const CELL_WIDE: u8 = 2;

/// Occupancy raster for one connection: for every (cell, layer), whether
/// other-net copper blocks a trace centre there, and whether there is enough
/// slack that edges between adjacent free cells are legal by construction.
///
/// Built once per search by sweeping the session's copper (exact
/// `distance_to` per nearby cell — the raster is *not* an approximation for
/// cells, and deliberately conservative for edges: WIDE↔WIDE edges skip the
/// oracle entirely, anything involving a TIGHT cell still probes). Replaces
/// millions of per-step R-tree probes with O(1) byte tests.
struct Raster {
    states: Vec<u8>,
    plane: usize,
}

impl Raster {
    fn build(
        session: &RouteSession,
        grid: &Grid,
        outline: &[Vec2],
        layers: &[PcbLayer],
        net: &str,
        half_w: f64,
    ) -> Self {
        let plane = grid.nx * grid.ny;
        let nl = layers.len();
        let mut states = vec![CELL_WIDE; plane * nl];

        // Cells outside the board outline are blocked on every layer.
        if grid.bounded {
            for iy in 0..grid.ny {
                for ix in 0..grid.nx {
                    if !point_in_polygon(grid.world(ix, iy), outline) {
                        let cell = grid.index(ix, iy);
                        for li in 0..nl {
                            states[li * plane + cell] = CELL_BLOCKED;
                        }
                    }
                }
            }
        }

        let wide_margin = grid.pitch * std::f64::consts::FRAC_1_SQRT_2;
        let win_lo = [grid.origin.x, grid.origin.y];
        let win_hi = [
            grid.origin.x + (grid.nx.saturating_sub(1)) as f64 * grid.pitch,
            grid.origin.y + (grid.ny.saturating_sub(1)) as f64 * grid.pitch,
        ];

        for (li, &layer) in layers.iter().enumerate() {
            let base = li * plane;
            session.for_each_blocking(layer, net, win_lo, win_hi, |geom, emin, emax, req| {
                // Same math as the probe: a trace centre at the cell is legal
                // iff point-to-edge distance ≥ half_w + required clearance.
                let reach_block = half_w + req;
                let reach = reach_block + wide_margin;
                let ix0 =
                    (((emin[0] - reach - grid.origin.x) / grid.pitch).floor()).max(0.0) as usize;
                let iy0 =
                    (((emin[1] - reach - grid.origin.y) / grid.pitch).floor()).max(0.0) as usize;
                let ix1 = ((((emax[0] + reach - grid.origin.x) / grid.pitch).ceil()) as usize)
                    .min(grid.nx.saturating_sub(1));
                let iy1 = ((((emax[1] + reach - grid.origin.y) / grid.pitch).ceil()) as usize)
                    .min(grid.ny.saturating_sub(1));
                for iy in iy0..=iy1 {
                    for ix in ix0..=ix1 {
                        let cell = grid.index(ix, iy);
                        let s = &mut states[base + cell];
                        if *s == CELL_BLOCKED {
                            continue;
                        }
                        let probe_pt = CopperGeom::Disc {
                            center: grid.world(ix, iy),
                            r: 0.0,
                        };
                        let d = geom.distance_to(&probe_pt);
                        if d < reach_block {
                            *s = CELL_BLOCKED;
                        } else if d < reach && *s == CELL_WIDE {
                            *s = CELL_TIGHT;
                        }
                    }
                }
            });
        }

        Self { states, plane }
    }

    #[inline]
    fn state(&self, cell: usize, li: usize) -> u8 {
        self.states[li * self.plane + cell]
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
        Self::new_scaled(outline, start, end, width, clearance, 1.0, None)
    }

    /// [`Grid::new`] with the pitch scaled by `pitch_scale` (and the
    /// grid-size cap scaled inversely, so a finer grid may actually resolve
    /// more cells rather than just re-coarsening back).
    /// `window`, when given, clips the search area to a corridor instead of
    /// the whole board — the board outline still bounds legality (cells
    /// outside it stay blocked), the window only shrinks the grid.
    fn new_scaled(
        outline: &[Vec2],
        start: Vec2,
        end: Vec2,
        width: f64,
        clearance: f64,
        pitch_scale: f64,
        window: Option<(Vec2, Vec2)>,
    ) -> Self {
        let (mut min, mut max) = if let Some((wlo, whi)) = window {
            // Clip to the board bbox so a generous corridor never exceeds it.
            if outline.len() >= 3 {
                let (blo, bhi) = bbox(outline);
                (
                    Vec2::new(wlo.x.max(blo.x), wlo.y.max(blo.y)),
                    Vec2::new(whi.x.min(bhi.x), whi.y.min(bhi.y)),
                )
            } else {
                (wlo, whi)
            }
        } else if outline.len() >= 3 {
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
        let scale = pitch_scale.clamp(0.1, 1.0);
        let mut pitch = ((width + clearance) * scale).max(0.02);
        // Coarsen so neither axis exceeds the (scaled) cell cap.
        let max_dim = (MAX_DIM as f64 / scale) as usize;
        let need = (span_x / pitch).max(span_y / pitch);
        if need > max_dim as f64 {
            pitch = (span_x / max_dim as f64).max(span_y / max_dim as f64);
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
            source: None,
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

    /// Multi-layer board: the same fixture with FCu/In1Cu/BCu copper.
    fn board3(traces: Vec<Trace>) -> Pcb {
        let mut pcb = board(traces);
        pcb.stackup.layers = [PcbLayer::FCu, PcbLayer::In1Cu, PcbLayer::BCu]
            .into_iter()
            .map(|layer| StackupLayer {
                layer,
                copper_thickness: Some(0.035),
                dielectric_thickness: Some(0.5),
                dielectric_er: Some(4.5),
                material: Some("FR4".into()),
            })
            .collect();
        pcb
    }

    const STACK3: [PcbLayer; 3] = [PcbLayer::FCu, PcbLayer::In1Cu, PcbLayer::BCu];

    #[test]
    fn maze3d_routes_flat_on_empty_board_without_vias() {
        let pcb = board3(vec![]);
        let session = RouteSession::from_pcb(&pcb);
        let r = route_net_maze3d(
            &session,
            &pcb.outline.vertices,
            &STACK3,
            "SIG",
            Vec2::new(5.0, 20.0),
            &[PcbLayer::FCu],
            Vec2::new(35.0, 20.0),
            &[PcbLayer::FCu],
            0.25,
            0.8,
            None,
            0,
            1.0,
            None,
            &[],
        );
        assert!(r.success);
        assert!(r.vias.is_empty(), "no reason to leave the start layer");
        assert!(r.segments.iter().all(|(_, _, l)| *l == PcbLayer::FCu));
    }

    #[test]
    fn maze3d_dives_through_via_past_a_full_wall() {
        // A GND wall spans the whole board on FCu: the 2D router fails here
        // (see fails_when_fully_walled_off), but the 3D search must drop a via,
        // cross on another layer, and come back to the FCu endpoint.
        let pcb = board3(vec![trace(
            "GND",
            Vec2::new(20.0, 0.0),
            Vec2::new(20.0, 40.0),
        )]);
        let session = RouteSession::from_pcb(&pcb);
        let r = route_net_maze3d(
            &session,
            &pcb.outline.vertices,
            &STACK3,
            "SIG",
            Vec2::new(5.0, 20.0),
            &[PcbLayer::FCu],
            Vec2::new(35.0, 20.0),
            &[PcbLayer::FCu],
            0.25,
            0.8,
            None,
            0,
            1.0,
            None,
            &[],
        );
        assert!(r.success, "3D search must cross under the wall");
        assert!(
            r.vias.len() >= 2,
            "must dive and resurface (got {} vias)",
            r.vias.len()
        );
        // Every segment legal on its own layer.
        let clr = session.clearance_for("SIG");
        for (a, b, l) in &r.segments {
            assert!(
                session
                    .probe(
                        &CopperGeom::Segment {
                            a: *a,
                            b: *b,
                            half_w: 0.125,
                        },
                        *l,
                        "SIG",
                        clr,
                    )
                    .legal,
                "segment on {l:?} must clear the wall"
            );
        }
        // The crossing itself must not happen on FCu.
        assert!(
            r.segments.iter().any(|(_, _, l)| *l != PcbLayer::FCu),
            "some copper must run on another layer"
        );
    }

    /// A wall on FCu only: the dive should be a minimal blind via
    /// (FCu↔In1Cu), not a stack-piercing through via — the span-scaled via
    /// cost makes the shallow hop cheaper, and the emitted spans say so.
    #[test]
    fn maze3d_prefers_minimal_via_spans() {
        let pcb = board3(vec![trace(
            "GND",
            Vec2::new(20.0, 0.0),
            Vec2::new(20.0, 40.0),
        )]);
        let session = RouteSession::from_pcb(&pcb);
        let r = route_net_maze3d(
            &session,
            &pcb.outline.vertices,
            &STACK3,
            "SIG",
            Vec2::new(5.0, 20.0),
            &[PcbLayer::FCu],
            Vec2::new(35.0, 20.0),
            &[PcbLayer::FCu],
            0.25,
            0.8,
            None,
            0,
            1.0,
            None,
            &[],
        );
        assert!(r.success);
        assert!(!r.vias.is_empty());
        for (_, la, lb) in &r.vias {
            assert_eq!(
                (*la, *lb),
                (PcbLayer::FCu, PcbLayer::In1Cu),
                "crossing under an FCu-only wall needs only the first hop"
            );
        }
    }

    #[test]
    fn maze3d_respects_endpoint_layers() {
        // End pad lives on BCu only: the route must finish there, via'ing
        // somewhere along the way.
        let pcb = board3(vec![]);
        let session = RouteSession::from_pcb(&pcb);
        let r = route_net_maze3d(
            &session,
            &pcb.outline.vertices,
            &STACK3,
            "SIG",
            Vec2::new(5.0, 20.0),
            &[PcbLayer::FCu],
            Vec2::new(35.0, 20.0),
            &[PcbLayer::BCu],
            0.25,
            0.8,
            None,
            0,
            1.0,
            None,
            &[],
        );
        assert!(r.success);
        assert!(!r.vias.is_empty(), "front→back must place a via");
        assert!(r.segments.iter().any(|(_, _, l)| *l == PcbLayer::BCu));
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
