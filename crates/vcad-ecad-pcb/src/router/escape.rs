//! Flow-based pin-field (BGA/fine-pitch) escape routing.
//!
//! Dense pin fields defeat per-net maze search: the interstitial channels
//! between pads carry only so many tracks, and greedy per-net routing
//! discovers the contention one failure at a time. The literature answer
//! (Fang & Wong, DAC'09) is a dedicated escape phase that solves pin-field
//! egress as a min-cost max-flow problem over the field's interstitial grid,
//! *before* (or here: as a rescue for) maze routing.
//!
//! This module detects dense pin fields, builds a flow network whose nodes
//! are the gaps between adjacent pads, solves min-cost max-flow with
//! successive shortest paths, and turns each unit of flow into a concrete
//! escape polyline (pad → assigned egress point just outside the field) on
//! the pad's layer. Every returned polyline is clearance-probed against the
//! [`RouteSession`] — an unprobeable escape returns `None` for that
//! connection. Nothing here mutates the session; committing is the caller's
//! job (see `try_route_escape` in `auto.rs`).

use vcad_ir::ecad::{PadShape, Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::geometry::pad_world_position;
use crate::session::RouteSession;
use crate::spatial::{point_in_polygon, CopperGeom};

/// Minimum pad count for a footprint to qualify as a dense pin field.
const MIN_FIELD_PADS: usize = 40;
/// Maximum nearest-neighbor pad pitch (mm) for a footprint to qualify.
const MAX_FIELD_PITCH: f64 = 1.0;
/// Cap on tracks assigned through any single interstitial gap.
const MAX_GAP_TRACKS: i64 = 4;
/// Fixed-point scale for integer flow costs (mm → cost units).
const COST_SCALE: f64 = 1000.0;

/// One pad of a detected field: world position and a conservative radius.
#[derive(Debug, Clone)]
struct FieldPad {
    pos: Vec2,
    r: f64,
}

/// A detected dense pin field (one fine-pitch footprint's pad lattice).
#[derive(Debug, Clone)]
pub struct PinField {
    /// Pads (world coordinates, conservative circumradius).
    pads: Vec<FieldPad>,
    /// Field bounding box, min corner (pad centers, mm).
    min: Vec2,
    /// Field bounding box, max corner (mm).
    max: Vec2,
    /// Nearest-neighbor pad pitch (mm) — the lattice spacing.
    pitch: f64,
    /// Copper layer the field's pads (and thus escapes) live on.
    layer: PcbLayer,
}

impl PinField {
    /// Whether `p` lies inside the field (bbox inflated by one pitch) — the
    /// gate for "this endpoint needs an escape".
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x - self.pitch
            && p.x <= self.max.x + self.pitch
            && p.y >= self.min.y - self.pitch
            && p.y <= self.max.y + self.pitch
    }
}

/// A planned escape for one connection endpoint: a polyline from the pad to
/// its assigned egress point just outside the field, on `layer`.
#[derive(Debug, Clone)]
pub struct EscapePlan {
    /// Polyline segments, pad-first. Fully clearance-probed at plan time.
    pub segments: Vec<(Vec2, Vec2)>,
    /// The egress point (last polyline vertex) — the new routing terminal.
    pub egress: Vec2,
    /// Layer the escape copper lives on.
    pub layer: PcbLayer,
}

/// Conservative circumradius of a pad shape (half the largest extent).
fn pad_radius(shape: &PadShape) -> f64 {
    match shape {
        PadShape::Circle { diameter } => diameter / 2.0,
        PadShape::Rect { width, height }
        | PadShape::Oval { width, height }
        | PadShape::RoundRect { width, height, .. } => width.max(*height) / 2.0,
        PadShape::Custom { vertices } => {
            let mut r: f64 = 0.0;
            for v in vertices {
                r = r.max((v.x * v.x + v.y * v.y).sqrt());
            }
            r
        }
    }
}

/// Detect dense pin fields: footprints with at least [`MIN_FIELD_PADS`] pads
/// whose nearest-neighbor pitch is at most [`MAX_FIELD_PITCH`].
pub fn detect_pin_fields(pcb: &Pcb) -> Vec<PinField> {
    let mut fields = Vec::new();
    for fp in &pcb.footprints {
        if fp.pads.len() < MIN_FIELD_PADS {
            continue;
        }
        let pads: Vec<FieldPad> = fp
            .pads
            .iter()
            .map(|p| FieldPad {
                pos: pad_world_position(fp, p),
                r: pad_radius(&p.shape),
            })
            .collect();
        // Nearest-neighbor pitch: min over pads of the distance to the
        // closest other pad. O(n²) — fields are a few hundred pads.
        let mut pitch = f64::INFINITY;
        for i in 0..pads.len() {
            let mut best = f64::INFINITY;
            for j in 0..pads.len() {
                if i != j {
                    let d = dist(pads[i].pos, pads[j].pos);
                    // Coincident pads (stacked pad/drill artifacts at one
                    // position) are not lattice spacing — skip them or the
                    // pitch degenerates to 0 and the interstitial grid with it.
                    if d > 1e-6 && d < best {
                        best = d;
                    }
                }
            }
            if best < pitch {
                pitch = best;
            }
        }
        // Reject non-lattices: sub-manufacturable "pitch" means the footprint
        // isn't a pin field the flow model can grid.
        if !pitch.is_finite() || !(0.1..=MAX_FIELD_PITCH).contains(&pitch) {
            continue;
        }
        let (mut min, mut max) = (pads[0].pos, pads[0].pos);
        for p in &pads {
            min.x = min.x.min(p.pos.x);
            min.y = min.y.min(p.pos.y);
            max.x = max.x.max(p.pos.x);
            max.y = max.y.max(p.pos.y);
        }
        let layer = fp
            .pads
            .iter()
            .flat_map(|p| p.layers.first())
            .next()
            .copied()
            .unwrap_or(PcbLayer::FCu);
        log::info!(
            "escape: dense pin field {} ({} pads, pitch {:.3} mm, bbox {:.1}x{:.1} mm)",
            fp.reference,
            pads.len(),
            pitch,
            max.x - min.x,
            max.y - min.y,
        );
        fields.push(PinField {
            pads,
            min,
            max,
            pitch,
            layer,
        });
    }
    fields
}

/// The field containing point `p`, if any.
pub fn field_containing(fields: &[PinField], p: Vec2) -> Option<&PinField> {
    fields.iter().find(|f| f.contains(p))
}

fn dist(a: Vec2, b: Vec2) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Distance from point `p` to segment `ab`.
fn dist_point_seg(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 <= 1e-12 {
        return dist(p, a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    dist(p, Vec2::new(a.x + t * dx, a.y + t * dy))
}

// ---------------------------------------------------------------------------
// Min-cost max-flow (successive shortest paths, SPFA). Networks are small
// (hundreds of nodes), so simplicity beats asymptotics here.
// ---------------------------------------------------------------------------

struct FlowEdge {
    to: usize,
    cap: i64,
    cost: i64,
    flow: i64,
}

struct FlowNet {
    /// Edge storage; adjacency holds indices. Edge `2k+1` is `2k`'s reverse.
    edges: Vec<FlowEdge>,
    adj: Vec<Vec<usize>>,
}

impl FlowNet {
    fn new(n: usize) -> Self {
        Self {
            edges: Vec::new(),
            adj: vec![Vec::new(); n],
        }
    }

    fn add_edge(&mut self, from: usize, to: usize, cap: i64, cost: i64) -> usize {
        let id = self.edges.len();
        self.edges.push(FlowEdge {
            to,
            cap,
            cost,
            flow: 0,
        });
        self.edges.push(FlowEdge {
            to: from,
            cap: 0,
            cost: -cost,
            flow: 0,
        });
        self.adj[from].push(id);
        self.adj[to].push(id + 1);
        id
    }

    /// Push up to `want` units from `s` to `t`; returns the units placed.
    fn mcmf(&mut self, s: usize, t: usize, want: i64) -> i64 {
        let n = self.adj.len();
        let mut total = 0i64;
        while total < want {
            // SPFA shortest path by cost on the residual graph.
            let mut dist = vec![i64::MAX; n];
            let mut in_q = vec![false; n];
            let mut prev_edge = vec![usize::MAX; n];
            dist[s] = 0;
            let mut q = std::collections::VecDeque::new();
            q.push_back(s);
            in_q[s] = true;
            while let Some(u) = q.pop_front() {
                in_q[u] = false;
                for &eid in &self.adj[u] {
                    let e = &self.edges[eid];
                    if e.cap - e.flow <= 0 || dist[u] == i64::MAX {
                        continue;
                    }
                    let nd = dist[u] + e.cost;
                    if nd < dist[e.to] {
                        dist[e.to] = nd;
                        prev_edge[e.to] = eid;
                        if !in_q[e.to] {
                            q.push_back(e.to);
                            in_q[e.to] = true;
                        }
                    }
                }
            }
            if dist[t] == i64::MAX {
                break;
            }
            // Bottleneck along the path.
            let mut push = want - total;
            let mut v = t;
            while v != s {
                let eid = prev_edge[v];
                push = push.min(self.edges[eid].cap - self.edges[eid].flow);
                v = self.edges[eid ^ 1].to;
            }
            let mut v = t;
            while v != s {
                let eid = prev_edge[v];
                self.edges[eid].flow += push;
                self.edges[eid ^ 1].flow -= push;
                v = self.edges[eid ^ 1].to;
            }
            total += push;
        }
        total
    }
}

// ---------------------------------------------------------------------------
// Escape planning
// ---------------------------------------------------------------------------

/// The interstitial grid over a field: nodes at the centers of pad quads
/// (offset half a pitch from the pad lattice), plus one ring outside the
/// field bbox (the egress ring).
struct Grid {
    nx: usize,
    ny: usize,
    origin: Vec2,
    pitch: f64,
}

impl Grid {
    fn for_field(field: &PinField) -> Grid {
        let pitch = field.pitch;
        // One ring of nodes outside the pad bbox on every side; nodes sit at
        // origin + (i + 0.5, j + 0.5) * pitch, i.e. between lattice rows.
        let origin = Vec2::new(field.min.x - 2.0 * pitch, field.min.y - 2.0 * pitch);
        let nx = (((field.max.x - origin.x) + 2.0 * pitch) / pitch).ceil() as usize;
        let ny = (((field.max.y - origin.y) + 2.0 * pitch) / pitch).ceil() as usize;
        Grid {
            nx,
            ny,
            origin,
            pitch,
        }
    }

    fn pos(&self, i: usize, j: usize) -> Vec2 {
        Vec2::new(
            self.origin.x + (i as f64 + 0.5) * self.pitch,
            self.origin.y + (j as f64 + 0.5) * self.pitch,
        )
    }

    fn idx(&self, i: usize, j: usize) -> usize {
        j * self.nx + i
    }
}

/// How many tracks of width `w` at clearance `clr` fit through a copper gap
/// of width `gap` (copper-to-copper): `n·w + (n+1)·clr ≤ gap`.
fn tracks_fit(gap: f64, w: f64, clr: f64) -> i64 {
    if gap <= 0.0 {
        return 0;
    }
    (((gap - clr) / (w + clr)).floor() as i64).clamp(0, MAX_GAP_TRACKS)
}

/// Free channel width (copper-to-copper) around point `p` given the field's
/// pads: twice the distance from `p` to the nearest pad edge.
fn channel_at_point(field: &PinField, p: Vec2) -> f64 {
    let mut d = f64::INFINITY;
    for pad in &field.pads {
        d = d.min(dist(p, pad.pos) - pad.r);
    }
    (2.0 * d).max(0.0)
}

/// Free channel width across segment `ab` (the constriction a track routed
/// along the segment must pass).
fn channel_along_seg(field: &PinField, a: Vec2, b: Vec2) -> f64 {
    let mut d = f64::INFINITY;
    for pad in &field.pads {
        d = d.min(dist_point_seg(pad.pos, a, b) - pad.r);
    }
    (2.0 * d).max(0.0)
}

/// Plan escapes for a batch of connection endpoints inside `field`.
///
/// `endpoints` are `(net, pad_position)` pairs; the result is index-aligned:
/// `Some(plan)` when the flow solver assigned the endpoint an egress and the
/// resulting polyline probed clean, `None` otherwise. Plans in one batch are
/// vertex-disjoint on the interstitial grid (node capacities), so they do not
/// cross each other; each is additionally probed against `session`.
pub fn plan_field_escapes(
    session: &RouteSession,
    pcb: &Pcb,
    field: &PinField,
    endpoints: &[(String, Vec2)],
    width: f64,
) -> Vec<Option<EscapePlan>> {
    let mut out: Vec<Option<EscapePlan>> = vec![None; endpoints.len()];
    if endpoints.is_empty() {
        return out;
    }
    let grid = Grid::for_field(field);
    let n_grid = grid.nx * grid.ny;
    if n_grid == 0 || n_grid > 40_000 {
        return out;
    }

    // Conservative geometry: plan with the widest requirements in the batch
    // so one network serves all endpoints (per-net widths differ rarely
    // inside a BGA).
    let w = endpoints
        .iter()
        .map(|(net, _)| session.width_for(net, width))
        .fold(width, f64::max);
    let clr = endpoints
        .iter()
        .map(|(net, _)| session.clearance_for(net))
        .fold(0.0, f64::max);

    // Node layout: [grid in 0..n][grid out n..2n][pad nodes][S][T].
    let pad_base = 2 * n_grid;
    let s = pad_base + endpoints.len();
    let t = s + 1;
    let mut net = FlowNet::new(t + 1);

    let in_board = |p: Vec2| -> bool {
        pcb.outline.vertices.len() < 3 || point_in_polygon(p, &pcb.outline.vertices)
    };
    let inside_bbox = |p: Vec2| -> bool {
        p.x >= field.min.x && p.x <= field.max.x && p.y >= field.min.y && p.y <= field.max.y
    };

    // Grid nodes: capacity from the local channel; the egress ring (nodes
    // outside the pad bbox) feeds the super-sink.
    let mut node_cap = vec![0i64; n_grid];
    for j in 0..grid.ny {
        for i in 0..grid.nx {
            let p = grid.pos(i, j);
            if !in_board(p) {
                continue;
            }
            let cap = tracks_fit(channel_at_point(field, p), w, clr);
            if cap <= 0 {
                continue;
            }
            let id = grid.idx(i, j);
            node_cap[id] = cap;
            // Node-split arc capped at 1: plans in a batch are then
            // vertex-disjoint on the grid, so they can never cross.
            net.add_edge(id, n_grid + id, 1, 0);
            if !inside_bbox(p) {
                // Egress ring: each ring node accepts one escape so egress
                // points stay distinct.
                net.add_edge(n_grid + id, t, 1, 0);
            }
        }
    }
    // Grid edges (4-neighborhood): capacity = tracks through the pad gap the
    // edge crosses; cost = edge length.
    for j in 0..grid.ny {
        for i in 0..grid.nx {
            let a_id = grid.idx(i, j);
            if node_cap[a_id] == 0 {
                continue;
            }
            let a = grid.pos(i, j);
            for (di, dj) in [(1isize, 0isize), (0, 1)] {
                let (ni, nj) = (i as isize + di, j as isize + dj);
                if ni < 0 || nj < 0 || ni >= grid.nx as isize || nj >= grid.ny as isize {
                    continue;
                }
                let b_id = grid.idx(ni as usize, nj as usize);
                if node_cap[b_id] == 0 {
                    continue;
                }
                let b = grid.pos(ni as usize, nj as usize);
                let cap = tracks_fit(channel_along_seg(field, a, b), w, clr);
                if cap <= 0 {
                    continue;
                }
                let cost = (dist(a, b) * COST_SCALE) as i64;
                net.add_edge(n_grid + a_id, b_id, cap, cost);
                net.add_edge(n_grid + b_id, a_id, cap, cost);
            }
        }
    }
    // Endpoint pads: S → pad node (cap 1) → nearby usable grid nodes. Each
    // first hop is clearance-probed here so the flow only ever uses legal
    // pad exits (the pad's own copper is same-net and doesn't block).
    for (k, (conn_net, pad_pt)) in endpoints.iter().enumerate() {
        let pad_node = pad_base + k;
        net.add_edge(s, pad_node, 1, 0);
        let clearance = session.clearance_for(conn_net);
        let hw = session.width_for(conn_net, width) / 2.0;
        let fi = ((pad_pt.x - grid.origin.x) / grid.pitch - 0.5).round() as isize;
        let fj = ((pad_pt.y - grid.origin.y) / grid.pitch - 0.5).round() as isize;
        for (di, dj) in [
            (0isize, 0isize),
            (-1, 0),
            (0, -1),
            (-1, -1),
            (1, 0),
            (0, 1),
            (1, 1),
            (-1, 1),
            (1, -1),
        ] {
            let (i, j) = (fi + di, fj + dj);
            if i < 0 || j < 0 || i >= grid.nx as isize || j >= grid.ny as isize {
                continue;
            }
            let id = grid.idx(i as usize, j as usize);
            if node_cap[id] == 0 {
                continue;
            }
            let gp = grid.pos(i as usize, j as usize);
            let hop = CopperGeom::Segment {
                a: *pad_pt,
                b: gp,
                half_w: hw,
            };
            if !session.probe(&hop, field.layer, conn_net, clearance).legal {
                continue;
            }
            let cost = (dist(*pad_pt, gp) * COST_SCALE) as i64;
            net.add_edge(pad_node, id, 1, cost);
        }
    }

    let placed = net.mcmf(s, t, endpoints.len() as i64);
    log::info!(
        "escape: field flow placed {placed}/{} escapes (grid {}x{}, w={w:.2} clr={clr:.2})",
        endpoints.len(),
        grid.nx,
        grid.ny,
    );
    if placed == 0 {
        return out;
    }

    // Decompose flow into per-endpoint paths, then probe each polyline.
    for (k, (conn_net, pad_pt)) in endpoints.iter().enumerate() {
        let pad_node = pad_base + k;
        // Did this endpoint's unit make it into the network?
        let mut cur = usize::MAX;
        for &eid in &net.adj[pad_node] {
            let e = &net.edges[eid];
            if eid % 2 == 0 && e.flow > 0 {
                cur = e.to;
                net.edges[eid].flow -= 1;
                break;
            }
        }
        if cur == usize::MAX {
            continue;
        }
        // Walk grid flow (consuming it, so units through shared arcs are
        // claimed once) until we exit to the sink.
        let mut pts: Vec<Vec2> = vec![*pad_pt];
        let mut reached_sink = false;
        let mut guard = 0;
        while guard < 4 * n_grid {
            guard += 1;
            let gid = cur % n_grid; // in-node or out-node → grid cell
            let (i, j) = (gid % grid.nx, gid / grid.nx);
            if pts.last().map(|p| dist(*p, grid.pos(i, j)) > 1e-9) == Some(true) {
                pts.push(grid.pos(i, j));
            }
            // Traverse the node-split arc if we're on the in-node.
            let from = if cur < n_grid { n_grid + gid } else { cur };
            let mut advanced = false;
            for &eid in &net.adj[from] {
                if eid % 2 != 0 {
                    continue;
                }
                let (to, flow) = (net.edges[eid].to, net.edges[eid].flow);
                if flow <= 0 {
                    continue;
                }
                if to == t {
                    net.edges[eid].flow -= 1;
                    reached_sink = true;
                } else {
                    net.edges[eid].flow -= 1;
                    cur = to;
                    advanced = true;
                }
                break;
            }
            if reached_sink || !advanced {
                break;
            }
        }
        if !reached_sink || pts.len() < 2 {
            continue;
        }
        let segments: Vec<(Vec2, Vec2)> = pts.windows(2).map(|p| (p[0], p[1])).collect();
        // Probe every segment against the live session before trusting the plan.
        let clearance = session.clearance_for(conn_net);
        let ww = session.width_for(conn_net, width);
        let legal = segments.iter().all(|(a, b)| {
            session
                .probe(
                    &CopperGeom::Segment {
                        a: *a,
                        b: *b,
                        half_w: ww / 2.0,
                    },
                    field.layer,
                    conn_net,
                    clearance,
                )
                .legal
        });
        if !legal {
            log::info!("escape: planned escape for {conn_net} failed session probe — dropped");
            continue;
        }
        let egress = *pts.last().expect("polyline has >= 2 points");
        out[k] = Some(EscapePlan {
            segments,
            egress,
            layer: field.layer,
        });
    }
    out
}

/// Plan an escape for a single endpoint (the integration entry point).
pub fn plan_escape(
    session: &RouteSession,
    pcb: &Pcb,
    field: &PinField,
    net: &str,
    pad_pt: Vec2,
    width: f64,
) -> Option<EscapePlan> {
    plan_field_escapes(session, pcb, field, &[(net.to_string(), pad_pt)], width).remove(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    fn pad(num: &str, x: f64, y: f64, net: &str, size: f64) -> Pad {
        Pad {
            number: num.into(),
            pad_type: PadType::SMD,
            shape: PadShape::Circle { diameter: size },
            position: Vec2::new(x, y),
            rotation: 0.0,
            drill: None,
            net: Some(net.into()),
            layers: vec![PcbLayer::FCu],
        }
    }

    fn board(footprints: Vec<Footprint>, width: f64, clearance: f64) -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(60.0, 0.0),
                    Vec2::new(60.0, 60.0),
                    Vec2::new(0.0, 60.0),
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
                        material: Some("FR4".into()),
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
                    trace_width: width,
                    clearance,
                    via_diameter: 0.4,
                    via_drill: 0.2,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                    target_impedance: None,
                    target_diff_impedance: None,
                },
                class_rules: vec![],
                net_class_assignments: Default::default(),
                edge_clearance: 0.5,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints,
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn bga(nx: usize, ny: usize, pitch: f64, pad_d: f64, origin: Vec2) -> Footprint {
        let mut pads = Vec::new();
        for j in 0..ny {
            for i in 0..nx {
                pads.push(pad(
                    &format!("P{i}_{j}"),
                    i as f64 * pitch,
                    j as f64 * pitch,
                    &format!("N{i}_{j}"),
                    pad_d,
                ));
            }
        }
        Footprint {
            reference: "U1".into(),
            value: "bga".into(),
            footprint_name: "BGA".into(),
            position: origin,
            rotation: 0.0,
            front: true,
            pads,
            graphics: vec![],
            model_3d: None,
            properties: Default::default(),
        }
    }

    /// Proper segment intersection (excluding shared endpoints).
    fn segs_cross(a: (Vec2, Vec2), b: (Vec2, Vec2)) -> bool {
        let d = |p: Vec2, q: Vec2, r: Vec2| (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
        let (p1, p2) = a;
        let (p3, p4) = b;
        let (d1, d2) = (d(p3, p4, p1), d(p3, p4, p2));
        let (d3, d4) = (d(p1, p2, p3), d(p1, p2, p4));
        d1 * d2 < -1e-12 && d3 * d4 < -1e-12
    }

    #[test]
    fn detects_dense_field() {
        let pcb = board(vec![bga(8, 8, 0.65, 0.3, Vec2::new(25.0, 25.0))], 0.1, 0.1);
        let fields = detect_pin_fields(&pcb);
        assert_eq!(fields.len(), 1);
        assert!((fields[0].pitch - 0.65).abs() < 1e-9);
        assert!(fields[0].contains(Vec2::new(27.0, 27.0)));
        assert!(!fields[0].contains(Vec2::new(5.0, 5.0)));
    }

    #[test]
    fn eight_inner_pads_escape_without_crossing() {
        // Synthetic 8x8 BGA at 0.65 mm pitch, 0.3 mm pads. The 8 pads of the
        // inner region must all find probed, mutually non-crossing escapes.
        let pcb = board(vec![bga(8, 8, 0.65, 0.3, Vec2::new(25.0, 25.0))], 0.1, 0.1);
        let fields = detect_pin_fields(&pcb);
        let field = &fields[0];
        let session = RouteSession::from_pcb(&pcb);

        // 8 inner pads (rows 2..=3, cols 2..=5 — well inside the field).
        let mut eps = Vec::new();
        for j in 2..=3usize {
            for i in 2..=5usize {
                eps.push((
                    format!("N{i}_{j}"),
                    Vec2::new(25.0 + i as f64 * 0.65, 25.0 + j as f64 * 0.65),
                ));
            }
        }
        let plans = plan_field_escapes(&session, &pcb, field, &eps, 0.1);
        let ok: Vec<&EscapePlan> = plans.iter().flatten().collect();
        assert_eq!(ok.len(), 8, "all 8 inner pads must escape");
        for p in &ok {
            let outside = p.egress.x < field.min.x
                || p.egress.x > field.max.x
                || p.egress.y < field.min.y
                || p.egress.y > field.max.y;
            assert!(
                outside,
                "egress must be outside the pad bbox: {:?}",
                p.egress
            );
        }
        // Pairwise non-crossing.
        for (a, pa) in ok.iter().enumerate() {
            for pb in ok.iter().skip(a + 1) {
                for sa in &pa.segments {
                    for sb in &pb.segments {
                        assert!(
                            !segs_cross(*sa, *sb),
                            "escapes must not cross: {sa:?} {sb:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn capacity_respected_through_gap_wall() {
        // A closed box of wall pads with exactly 3 single-pad gaps; 20 source
        // pads inside. Each gap channel fits exactly one track, so at most 3
        // escapes can be planned — the flow model must not cram 20 through.
        let pitch = 0.65;
        let pad_d = 0.3;
        let n = 14; // 14x14 perimeter box
        let origin = Vec2::new(25.0, 25.0);
        let mut pads = Vec::new();
        // Gap positions on the perimeter (skip these wall pads). Each gap is
        // two missing pads wide: the interstitial grid nodes sit half a pitch
        // off the pad lattice, so a channel must straddle a lattice line
        // between two missing pads to carry a grid edge.
        let gaps = [(0usize, 4usize), (0, 5), (13, 7), (13, 8), (6, 13), (7, 13)];
        for j in 0..n {
            for i in 0..n {
                let perimeter = i == 0 || j == 0 || i == n - 1 || j == n - 1;
                if !perimeter || gaps.contains(&(i, j)) {
                    continue;
                }
                pads.push(pad(
                    &format!("W{i}_{j}"),
                    i as f64 * pitch,
                    j as f64 * pitch,
                    "WALL",
                    pad_d,
                ));
            }
        }
        // 20 source pads inside, sparse (2-pitch spacing keeps interior open).
        let mut eps = Vec::new();
        for k in 0..20usize {
            let (i, j) = (2 + 2 * (k % 5), 2 + 2 * (k / 5));
            let name = format!("S{k}");
            pads.push(pad(&name, i as f64 * pitch, j as f64 * pitch, &name, pad_d));
            eps.push((
                name,
                Vec2::new(origin.x + i as f64 * pitch, origin.y + j as f64 * pitch),
            ));
        }
        assert!(pads.len() >= MIN_FIELD_PADS);
        let fp = Footprint {
            reference: "U1".into(),
            value: "walled".into(),
            footprint_name: "WALL".into(),
            position: origin,
            rotation: 0.0,
            front: true,
            pads,
            graphics: vec![],
            model_3d: None,
            properties: Default::default(),
        };
        // Rules where a single-pad gap (2·pitch − pad_d = 1.0 mm channel)
        // fits exactly one 0.25 mm track at 0.2 mm clearance.
        let pcb = board(vec![fp], 0.25, 0.2);
        // The un-gapped wall itself is impassable at these rules.
        assert_eq!(tracks_fit(pitch - pad_d, 0.25, 0.2), 0);
        let fields = detect_pin_fields(&pcb);
        assert_eq!(fields.len(), 1);
        let session = RouteSession::from_pcb(&pcb);
        let plans = plan_field_escapes(&session, &pcb, &fields[0], &eps, 0.25);
        let ok = plans.iter().flatten().count();
        assert!(ok > 0, "some escapes must be planned");
        assert!(
            ok <= 6,
            "20 pins cannot all escape a 3-gap wall (planned {ok})"
        );
    }
}
