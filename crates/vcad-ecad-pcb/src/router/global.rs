//! Capacity-mesh global routing — the stage that resolves congestion at
//! bottleneck scale *before* detailed routing spends time proving it.
//!
//! The board is covered by a uniform grid of GCells (the CUGR model). Each
//! cell-to-cell crossing has a **capacity**: roughly how many tracks fit
//! through that boundary across the whole copper stack, discounted by the
//! copper already there. Every connection is assigned a cell path by A* whose
//! cost includes a soft **overflow penalty** that grows quadratically once
//! demand exceeds capacity — so connections negotiate for corridors where
//! they *fit*, instead of all diving into the same channel and letting the
//! detailed router fail one by one. A few negotiation sweeps re-route
//! everything against the settled demand field.
//!
//! The output is one **corridor** (an inflated bbox of the assigned cell
//! path) per connection, which the detailed router uses as its search
//! window. Hard legality is entirely the detailed router's problem — the
//! mesh only decides *where to look*.

use std::collections::BinaryHeap;

use vcad_ir::Vec2;

use crate::session::RouteSession;

use super::Stopwatch;

/// GCell edge length (mm). Coarse on purpose: capacity negotiation wants
/// bottleneck granularity, not track granularity.
const GCELL: f64 = 4.0;

/// Negotiation sweeps over the demand field.
const SWEEPS: usize = 3;

/// Overflow penalty weight (mm-equivalent per unit of squared overflow).
const OVERFLOW_K: f64 = 24.0;

/// Corridor margin (mm) added around the assigned cell path's bbox.
const CORRIDOR_MARGIN: f64 = 6.0;

/// A connection to plan: `(net, from, to)` in board coordinates.
pub type PlanConn = (String, Vec2, Vec2);

/// The capacity mesh and its per-edge demand state.
pub struct CapacityMesh {
    origin: [f64; 2],
    nx: usize,
    ny: usize,
    /// Per-cell free-space fraction × layer count — the crossing budget
    /// shared by the 4 boundaries of the cell.
    capacity: Vec<f64>,
    /// Tracks assigned through each cell this planning round.
    demand: Vec<f64>,
}

impl CapacityMesh {
    /// Build the mesh over the session's copper. `track_pitch` is
    /// `width + clearance` — the space one routed track consumes; `layers`
    /// the number of copper layers.
    pub fn build(
        session: &RouteSession,
        board_lo: [f64; 2],
        board_hi: [f64; 2],
        track_pitch: f64,
        layers: usize,
    ) -> Self {
        let nx = (((board_hi[0] - board_lo[0]) / GCELL).ceil() as usize).max(1);
        let ny = (((board_hi[1] - board_lo[1]) / GCELL).ceil() as usize).max(1);
        let mut capacity = vec![0.0; nx * ny];
        let tracks_per_cell = (GCELL / track_pitch.max(0.01)).max(1.0);
        for cy in 0..ny {
            for cx in 0..nx {
                let lo = [
                    board_lo[0] + cx as f64 * GCELL,
                    board_lo[1] + cy as f64 * GCELL,
                ];
                let hi = [lo[0] + GCELL, lo[1] + GCELL];
                let copper_area = session.copper_area_in(lo, hi);
                let cell_area = GCELL * GCELL * layers as f64;
                let free = (1.0 - copper_area / cell_area).clamp(0.05, 1.0);
                capacity[cy * nx + cx] = tracks_per_cell * layers as f64 * free;
            }
        }
        Self {
            origin: board_lo,
            nx,
            ny,
            capacity,
            demand: vec![0.0; nx * ny],
        }
    }

    fn cell_of(&self, p: Vec2) -> usize {
        let cx =
            (((p.x - self.origin[0]) / GCELL).floor() as i64).clamp(0, self.nx as i64 - 1) as usize;
        let cy =
            (((p.y - self.origin[1]) / GCELL).floor() as i64).clamp(0, self.ny as i64 - 1) as usize;
        cy * self.nx + cx
    }

    fn center(&self, cell: usize) -> Vec2 {
        let (cx, cy) = (cell % self.nx, cell / self.nx);
        Vec2::new(
            self.origin[0] + (cx as f64 + 0.5) * GCELL,
            self.origin[1] + (cy as f64 + 0.5) * GCELL,
        )
    }

    /// Marginal cost of pushing one more track through `cell`: the step
    /// length plus a quadratic penalty once demand exceeds capacity.
    fn cell_cost(&self, cell: usize) -> f64 {
        let cap = self.capacity[cell].max(0.5);
        let over = (self.demand[cell] + 1.0 - cap).max(0.0) / cap;
        GCELL + OVERFLOW_K * over * over
    }

    /// A* over cells from `from` to `to` under the current demand field.
    fn assign(&self, from: Vec2, to: Vec2) -> Vec<usize> {
        let (start, goal) = (self.cell_of(from), self.cell_of(to));
        let n = self.nx * self.ny;
        let mut g = vec![f64::INFINITY; n];
        let mut came = vec![usize::MAX; n];
        let mut closed = vec![false; n];
        let mut heap: BinaryHeap<CellState> = BinaryHeap::new();
        let gc = self.center(goal);
        let h = |c: usize| {
            let p = self.center(c);
            ((p.x - gc.x).powi(2) + (p.y - gc.y).powi(2)).sqrt()
        };
        g[start] = 0.0;
        heap.push(CellState {
            f: h(start),
            cell: start,
        });
        while let Some(CellState { cell, .. }) = heap.pop() {
            if cell == goal {
                break;
            }
            if closed[cell] {
                continue;
            }
            closed[cell] = true;
            let (cx, cy) = (cell % self.nx, cell / self.nx);
            for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                let (nx_, ny_) = (cx as i64 + dx, cy as i64 + dy);
                if nx_ < 0 || ny_ < 0 || nx_ >= self.nx as i64 || ny_ >= self.ny as i64 {
                    continue;
                }
                let nb = ny_ as usize * self.nx + nx_ as usize;
                if closed[nb] {
                    continue;
                }
                let t = g[cell] + self.cell_cost(nb);
                if t < g[nb] {
                    g[nb] = t;
                    came[nb] = cell;
                    heap.push(CellState {
                        f: t + h(nb),
                        cell: nb,
                    });
                }
            }
        }
        // Reconstruct (goal may be unreached only on a degenerate mesh; the
        // caller treats an empty path as "no corridor" and searches unbounded).
        if came[goal] == usize::MAX && goal != start {
            return Vec::new();
        }
        let mut path = vec![goal];
        let mut cur = goal;
        while cur != start {
            cur = came[cur];
            if cur == usize::MAX {
                return Vec::new();
            }
            path.push(cur);
        }
        path
    }

    fn deposit(&mut self, path: &[usize], amount: f64) {
        for &c in path {
            self.demand[c] += amount;
        }
    }

    /// The corridor (window bbox) covering a cell path, inflated by margin.
    fn corridor(&self, path: &[usize], from: Vec2, to: Vec2) -> (Vec2, Vec2) {
        let mut lo = Vec2::new(from.x.min(to.x), from.y.min(to.y));
        let mut hi = Vec2::new(from.x.max(to.x), from.y.max(to.y));
        for &c in path {
            let p = self.center(c);
            lo.x = lo.x.min(p.x - GCELL);
            lo.y = lo.y.min(p.y - GCELL);
            hi.x = hi.x.max(p.x + GCELL);
            hi.y = hi.y.max(p.y + GCELL);
        }
        (
            Vec2::new(lo.x - CORRIDOR_MARGIN, lo.y - CORRIDOR_MARGIN),
            Vec2::new(hi.x + CORRIDOR_MARGIN, hi.y + CORRIDOR_MARGIN),
        )
    }
}

/// Per-connection scarcity signals from the settled demand field:
/// `min_residual` — the tightest capacity margin along the assigned corridor
/// (low = few alternatives; the CSP "most constrained variable" signal), and
/// `overlap_degree` — how many OTHER connections share a cell with this one
/// (the "most constraining variable" tiebreak).
#[derive(Debug, Clone, Copy)]
pub struct Scarcity {
    /// min(capacity - demand) along the assigned cell path.
    pub min_residual: f64,
    /// Number of other connections sharing at least one cell.
    pub overlap_degree: usize,
}

/// [`plan_corridors`] plus per-connection [`Scarcity`] — the ordering brain's
/// input. Measured lesson (CM5 apex run): ordering decides routability more
/// than any search algorithm; nets whose only corridors get consumed by
/// flexible early routes fail forever. Scarcest-first fixes that by default.
pub fn plan_with_scarcity(
    session: &RouteSession,
    board_lo: [f64; 2],
    board_hi: [f64; 2],
    track_pitch: f64,
    layers: usize,
    conns: &[PlanConn],
) -> (Vec<Option<(Vec2, Vec2)>>, Vec<Scarcity>) {
    let sw = Stopwatch::start();
    let mut mesh = CapacityMesh::build(session, board_lo, board_hi, track_pitch, layers);
    let mut paths: Vec<Vec<usize>> = vec![Vec::new(); conns.len()];
    for _sweep in 0..SWEEPS {
        mesh.demand.iter_mut().for_each(|d| *d = 0.0);
        for (i, (_, from, to)) in conns.iter().enumerate() {
            let path = mesh.assign(*from, *to);
            mesh.deposit(&path, 1.0);
            paths[i] = path;
        }
    }
    // Cell -> connections index for overlap degree.
    let mut cell_users: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, path) in paths.iter().enumerate() {
        for &c in path {
            cell_users.entry(c).or_default().push(i);
        }
    }
    let scarcity: Vec<Scarcity> = paths
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let min_residual = path
                .iter()
                .map(|&c| mesh.capacity[c] - mesh.demand[c])
                .fold(f64::INFINITY, f64::min);
            let mut others = std::collections::HashSet::new();
            for &c in path {
                for &j in &cell_users[&c] {
                    if j != i {
                        others.insert(j);
                    }
                }
            }
            Scarcity {
                min_residual: if min_residual.is_finite() {
                    min_residual
                } else {
                    f64::MAX
                },
                overlap_degree: others.len(),
            }
        })
        .collect();
    log::info!(
        "global plan+scarcity: {} conns over {}x{} mesh in {:.0}ms",
        conns.len(),
        mesh.nx,
        mesh.ny,
        sw.ms(),
    );
    let corridors = paths
        .iter()
        .zip(conns.iter())
        .map(|(path, (_, from, to))| {
            if path.len() < 2 {
                None
            } else {
                Some(mesh.corridor(path, *from, *to))
            }
        })
        .collect();
    (corridors, scarcity)
}

/// Plan corridors for every connection: a few negotiated-congestion sweeps
/// of cell-path assignment, then one corridor bbox per connection (aligned
/// with the input order). `None` means "no useful corridor" — search
/// unbounded as before.
pub fn plan_corridors(
    session: &RouteSession,
    board_lo: [f64; 2],
    board_hi: [f64; 2],
    track_pitch: f64,
    layers: usize,
    conns: &[PlanConn],
) -> Vec<Option<(Vec2, Vec2)>> {
    let sw = Stopwatch::start();
    let mut mesh = CapacityMesh::build(session, board_lo, board_hi, track_pitch, layers);
    let mut paths: Vec<Vec<usize>> = vec![Vec::new(); conns.len()];

    for sweep in 0..SWEEPS {
        // Rip all demand and re-assign everything against the field the
        // previous sweep settled — cheap at GCell scale.
        mesh.demand.iter_mut().for_each(|d| *d = 0.0);
        // Re-deposit in input order so later assignments see earlier ones.
        let _ = sweep;
        for (i, (_, from, to)) in conns.iter().enumerate() {
            let path = mesh.assign(*from, *to);
            mesh.deposit(&path, 1.0);
            paths[i] = path;
        }
    }

    let overflowed = mesh
        .demand
        .iter()
        .zip(mesh.capacity.iter())
        .filter(|(d, c)| *d > *c)
        .count();
    log::info!(
        "global plan: {} conns over {}x{} mesh, {} overflowed cells, {:.0}ms",
        conns.len(),
        mesh.nx,
        mesh.ny,
        overflowed,
        sw.ms(),
    );

    paths
        .iter()
        .zip(conns.iter())
        .map(|(path, (_, from, to))| {
            if path.len() < 2 {
                None
            } else {
                Some(mesh.corridor(path, *from, *to))
            }
        })
        .collect()
}

/// A* state over GCells.
struct CellState {
    f: f64,
    cell: usize,
}
impl PartialEq for CellState {
    fn eq(&self, o: &Self) -> bool {
        self.f == o.f && self.cell == o.cell
    }
}
impl Eq for CellState {}
impl Ord for CellState {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        o.f.total_cmp(&self.f).then(self.cell.cmp(&o.cell))
    }
}
impl PartialOrd for CellState {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    fn empty_board(w: f64, h: f64) -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(w, 0.0),
                    Vec2::new(w, h),
                    Vec2::new(0.0, h),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![
                    StackupLayer {
                        layer: PcbLayer::FCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: Some(1.5),
                        dielectric_er: Some(4.5),
                        material: None,
                    },
                    StackupLayer {
                        layer: PcbLayer::BCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: None,
                        dielectric_er: None,
                        material: None,
                    },
                ],
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
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    #[test]
    fn corridors_cover_their_connections() {
        let pcb = empty_board(40.0, 40.0);
        let session = RouteSession::from_pcb(&pcb);
        let conns: Vec<PlanConn> = vec![
            ("A".into(), Vec2::new(2.0, 2.0), Vec2::new(38.0, 38.0)),
            ("B".into(), Vec2::new(2.0, 38.0), Vec2::new(38.0, 2.0)),
        ];
        let corridors = plan_corridors(&session, [0.0, 0.0], [40.0, 40.0], 0.45, 2, &conns);
        assert_eq!(corridors.len(), 2);
        for (c, (_, from, to)) in corridors.iter().zip(conns.iter()) {
            let (lo, hi) = c.expect("open board must yield a corridor");
            for p in [from, to] {
                assert!(p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y);
            }
        }
    }

    #[test]
    fn congestion_spreads_bundles_apart() {
        // 60 identical left→right connections through a 3-cell-tall board:
        // with soft capacity they must NOT all take the same middle row.
        let pcb = empty_board(60.0, 12.0);
        let session = RouteSession::from_pcb(&pcb);
        let conns: Vec<PlanConn> = (0..60)
            .map(|i| (format!("N{i}"), Vec2::new(2.0, 6.0), Vec2::new(58.0, 6.0)))
            .collect();
        let mesh_rows = 3usize; // 12mm / 4mm
        let corridors = plan_corridors(&session, [0.0, 0.0], [60.0, 12.0], 0.45, 2, &conns);
        // At least some corridors must widen beyond the single middle band
        // (i.e. include the top or bottom row) once the middle overflows.
        let widened = corridors
            .iter()
            .flatten()
            .filter(|(lo, hi)| hi.y - lo.y > GCELL * (mesh_rows as f64 - 1.0))
            .count();
        assert!(
            widened > 0,
            "overflow must push some corridors off the middle row"
        );
    }
}
