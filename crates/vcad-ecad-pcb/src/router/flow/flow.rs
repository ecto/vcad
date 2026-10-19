//! The market: every connection is a commodity routed as a min-cost path on
//! the dual of the cut graph (faces = nodes, cuts = arcs with capacity and a
//! price λ) × layers, plus via arcs between layers at via sites.
//!
//! Via sites are Steiner vertices of every layer's triangulation
//! ([`super::cdt::LayerCdt::build`]). A via placed at a site takes room on
//! exactly the cuts incident to that vertex (the capacity they lose when the
//! barrel's grown disc stands there), so vias and wires compete for the same
//! priced resources. Each site holds one net (a unit-capacity resource with its
//! own price).
//!
//! Per iteration every commodity is re-routed at the current prices
//! (Gauss–Seidel, deterministic order, PathFinder present-congestion term for
//! convergence), then prices take a projected subgradient step
//! `λ_e ← max(0, λ_e + α_t (load_e − cap_e))`. Each iteration also evaluates
//! the Lagrangian lower bound `L(λ) = Σ_k SP_k(λ) − Σ_e λ_e cap_e − Σ_s λ_s`
//! (shortest paths at the prices alone), a valid bound on the optimal total
//! cost of any capacity-feasible routing by weak duality. With a feasible
//! iterate the gap `UB − max L` certifies how far the plan is from optimal.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use super::cdt::{CutGraph, CutParams, ObsKind, NONE};
use crate::session::RouteSession;
use crate::spatial::CopperGeom;

/// A legal via construction at a site, before the graph exists.
#[derive(Debug, Clone)]
pub struct SpanSpec {
    /// Top copper layer.
    pub a: PcbLayer,
    /// Bottom copper layer.
    pub b: PcbLayer,
    /// The physical vias (one, or a stack of microvias at one point).
    pub stack: Vec<(PcbLayer, PcbLayer)>,
    /// Cost (mm-equivalent).
    pub cost: f64,
    /// Barrel radius in centerline space (pad radius + clearance + w0/2).
    pub r_center: f64,
}

/// A via site chosen on the pad-only triangulation.
#[derive(Debug, Clone)]
pub struct SiteSpec {
    /// Position.
    pub pos: [f64; 2],
    /// Legal spans.
    pub spans: Vec<SpanSpec>,
}

/// One legal via construction at a site, wired into the graph.
#[derive(Debug, Clone)]
pub struct SiteSpan {
    /// Top copper layer of the span.
    pub a: PcbLayer,
    /// Bottom copper layer of the span.
    pub b: PcbLayer,
    /// The physical vias.
    pub stack: Vec<(PcbLayer, PcbLayer)>,
    /// Graph layer indices (signal layers) the span touches.
    pub layers: Vec<u8>,
    /// Cost (mm-equivalent).
    pub cost: f64,
    /// Cuts (graph layer, cut, units) whose room the barrel takes.
    pub cuts: Vec<(u8, u32, f64)>,
    /// Barrel radius in centerline space.
    pub r_center: f64,
}

/// A via site: a location where one via (of any legal span) may be placed.
#[derive(Debug, Clone)]
pub struct ViaSite {
    /// Position (mm).
    pub pos: [f64; 2],
    /// Legal spans here.
    pub spans: Vec<SiteSpan>,
}

/// Tuning of the price loop.
#[derive(Debug, Clone)]
pub struct FlowParams {
    /// Hard iteration budget.
    pub iters: usize,
    /// Iterations without a better (overflow, conflicts) score that count as
    /// a stall.
    pub stall: usize,
    /// Subgradient step scale (mm per unit of overflow).
    pub alpha: f64,
    /// Initial present-congestion factor (mm per unit of excess).
    pub pres0: f64,
    /// Present factor growth per iteration.
    pub pres_growth: f64,
    /// Cost of a drilled (non-micro) via (mm-equivalent).
    pub via_cost: f64,
    /// Cost of a single-pair (micro) via.
    pub micro_cost: f64,
    /// Present penalty for using a site another net holds.
    pub via_block: f64,
}

impl Default for FlowParams {
    fn default() -> Self {
        Self {
            iters: 300,
            stall: 25,
            alpha: 0.5,
            pres0: 0.5,
            pres_growth: 1.3,
            via_cost: 2.0,
            micro_cost: 1.0,
            via_block: 5.0,
        }
    }
}

/// A pad terminal of a commodity.
#[derive(Debug, Clone)]
pub struct Terminal {
    /// Pad center.
    pub point: [f64; 2],
    /// Graph nodes the path may start/end at (pad-interior faces per layer).
    pub nodes: Vec<u32>,
}

/// One 2-pin connection.
#[derive(Debug, Clone)]
pub struct Commodity {
    /// Net.
    pub net: String,
    /// Net id in [`FlowGraph::nets`].
    pub net_id: u32,
    /// Track width (mm).
    pub width: f64,
    /// Capacity units one wire consumes (`(w + s) / (w0 + s)`).
    pub units: f64,
    /// Source pad.
    pub src: Terminal,
    /// Destination pad.
    pub dst: Terminal,
}

/// A step of a routed path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Cross cut `cut` on graph layer `layer`, leaving face `from`.
    Cross {
        /// Graph layer index.
        layer: u8,
        /// Cut.
        cut: u32,
        /// Face left.
        from: u32,
        /// Whether the crossing consumes capacity (false: own-pad crossing).
        consumes: bool,
    },
    /// Move between a face and the via site at one of its corners.
    Site {
        /// Site id.
        site: u32,
        /// Graph layer.
        layer: u8,
        /// The face.
        face: u32,
    },
    /// Change layer through a via at `site` using span `span`.
    Via {
        /// Site id.
        site: u32,
        /// Span index within the site.
        span: u16,
        /// Graph layer left.
        from: u8,
        /// Graph layer entered.
        to: u8,
    },
}

/// A commodity's route: start node, steps, end node.
#[derive(Debug, Clone, Default)]
pub struct FlowPath {
    /// Start node.
    pub start: u32,
    /// End node.
    pub end: u32,
    /// Steps in order.
    pub steps: Vec<Step>,
    /// Primal cost (length + via cost), mm.
    pub cost: f64,
}

/// Per-iteration market statistics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IterStat {
    /// Round of [`super::route_flow`] this iteration belongs to.
    pub round: usize,
    /// Iteration.
    pub iter: usize,
    /// Σ max(0, load − cap) over cuts (capacity units).
    pub overflow: f64,
    /// Cuts with load > cap.
    pub overflowed_cuts: usize,
    /// Σ primal path cost (mm).
    pub cost: f64,
    /// Lagrangian lower bound at this iteration's prices.
    pub lower_bound: f64,
    /// Commodities without any path.
    pub unrouted: usize,
    /// Via sites used by two or more nets.
    pub site_conflicts: usize,
    /// Vias placed.
    pub vias: usize,
}

/// Node kinds of the routing graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    /// Face `f` of graph layer `l`.
    Face(usize, u32),
    /// Site `s` on graph layer `l`, entered from a face (must take a via).
    SiteIn(u32, usize),
    /// Site `s` on graph layer `l`, arrived through a via (must leave to a face).
    SiteOut(u32, usize),
}

/// The routing graph and market state.
pub struct FlowGraph<'g> {
    /// Cut graphs.
    pub cg: &'g CutGraph,
    /// Node offset of each graph layer.
    pub layer_off: Vec<u32>,
    /// Face nodes (all layers).
    pub n_faces: usize,
    /// Total nodes (faces + 2 × sites × layers).
    pub n_nodes: usize,
    /// Via sites.
    pub sites: Vec<ViaSite>,
    /// Per face node: (site, layer) of sites at the face's corners.
    pub(super) face_sites: Vec<Vec<u32>>,
    /// Per site × layer: faces around the site vertex.
    pub(super) site_faces: Vec<Vec<u32>>,
    /// Net names.
    pub nets: Vec<String>,
    /// Net name → id.
    pub net_id: HashMap<String, u32>,
    /// Per layer, per obstacle: net id (NONE for no-net copper).
    obs_net: Vec<Vec<u32>>,
    /// Per layer, per cut: arc length (mm).
    pub(super) arc_len: Vec<Vec<f64>>,
    /// Per layer, per cut: capacity (units at the default width).
    pub cap: Vec<Vec<f64>>,
    /// Per layer, per cut: nets whose crossing is free (own copper).
    free_nets: Vec<Vec<Vec<u32>>>,
    /// Per layer, per cut: current load.
    pub load: Vec<Vec<f64>>,
    /// Per layer, per cut: price λ.
    pub lambda: Vec<Vec<f64>>,
    /// Per site: price of its unit capacity.
    pub site_lambda: Vec<f64>,
    /// Users of each site this iteration (net ids, with multiplicity).
    site_users: Vec<Vec<u32>>,
}

#[derive(Copy, Clone, PartialEq)]
struct Item {
    f: f64,
    g: f64,
    node: u32,
}
impl Eq for Item {}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.total_cmp(&self.f).then_with(|| o.node.cmp(&self.node))
    }
}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// How a commodity may cross a cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Cross {
    /// Not crossable by this net.
    Blocked,
    /// Inside the net's own copper: no room consumed.
    Free,
    /// Consumes capacity.
    Consumes,
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Choose via sites on a site-free cut graph: free-space face centroids where
/// some legal span fits (exact session probes: hole-to-hole, no-via zones,
/// copper clearance on every spanned layer), greedily thinned to one site per
/// via pitch so any set of used sites is mutually compatible.
pub fn choose_sites(pcb: &Pcb, cg: &CutGraph, params: &FlowParams) -> Vec<SiteSpec> {
    let sizing = crate::via_sizing::ViaSizing::from_pcb(pcb);
    let copper: Vec<PcbLayer> = PcbLayer::COPPER_STACK
        .iter()
        .copied()
        .filter(|l| pcb.stackup.layers.iter().any(|s| s.layer == *l))
        .collect();
    if copper.len() < 2 || cg.layers.len() < 2 {
        return Vec::new();
    }
    let sig: Vec<PcbLayer> = cg.layers.iter().map(|l| l.layer).collect();
    // Span catalog: every single via of the board's catalog that joins two
    // signal layers, plus stacks of two consecutive microvias (when the board
    // allows stacking) — on an HDI stack the signal layers sit behind planes
    // (F–In1–In2), so a signal layer change is a stack.
    let stacks = signal_stacks(pcb, &sig, &sizing);
    let spans: Vec<(PcbLayer, PcbLayer)> = stacks.iter().flatten().copied().collect();
    let idx = |l: PcbLayer| copper.iter().position(|&c| c == l).unwrap_or(0);
    let session = RouteSession::from_pcb(pcb);
    let clr = pcb.rules.default_rules.clearance;
    let w0 = cg.params.width;
    let max_spec = spans
        .iter()
        .map(|&(a, b)| sizing.spec("", a, b))
        .fold((0.0f64, 0.0f64), |m, s| {
            (m.0.max(s.diameter), m.1.max(s.drill))
        });
    let pitch = (max_spec.0 + clr).max(max_spec.1 + pcb.rules.hole_to_hole) + 1e-3;
    // Via-in-pad sites first (the HDI escape: a microvia stack straight down
    // from an SMD pad; only the pad's own net can reach it, since the faces
    // around it are inside the pad), then free-space face centroids.
    let planes = super::access::plane_poured(pcb);
    type Cand = ([f64; 2], Option<(String, PcbLayer)>);
    let mut pad_sites: Vec<Cand> = Vec::new();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            let Some(net) = pad.net.as_deref().filter(|n| !n.is_empty()) else {
                continue;
            };
            if pad.drill.is_some() || planes.contains_key(net) {
                continue;
            }
            let Some(&pl) = pad.layers.iter().find(|l| l.is_copper()) else {
                continue;
            };
            let c = crate::geometry::pad_world_position(fp, pad);
            pad_sites.push(([c.x, c.y], Some((net.to_string(), pl))));
        }
    }
    pad_sites.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]).then(a.0[1].total_cmp(&b.0[1])));
    let mut free: Vec<[f64; 2]> = Vec::new();
    for lg in &cg.layers {
        for f in 0..lg.tris.len() {
            if lg.tri_live[f] && lg.tri_obs[f] == NONE && !lg.tri_block[f] {
                free.push(lg.tri_centroid(f as u32));
            }
        }
    }
    free.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    free.dedup_by(|a, b| dist(*a, *b) < 1e-6);
    let cands: Vec<Cand> = pad_sites
        .into_iter()
        .chain(free.into_iter().map(|p| (p, None)))
        .collect();
    let mut grid: HashMap<(i64, i64), Vec<[f64; 2]>> = HashMap::new();
    let key = |p: [f64; 2]| ((p[0] / pitch).floor() as i64, (p[1] / pitch).floor() as i64);
    let probe_net = "\u{2}flow-site";
    let mut out = Vec::new();
    for (p, owner) in cands {
        let k = key(p);
        let near = (-1..=1).any(|dx| {
            (-1..=1).any(|dy| {
                grid.get(&(k.0 + dx, k.1 + dy))
                    .is_some_and(|v| v.iter().any(|q| dist(*q, p) < pitch))
            })
        });
        if near {
            continue;
        }
        let center = Vec2::new(p[0], p[1]);
        let mut site = SiteSpec {
            pos: p,
            spans: Vec::new(),
        };
        let pnet = owner.as_ref().map(|o| o.0.as_str()).unwrap_or(probe_net);
        for stack in &stacks {
            // an in-pad stack must start on the pad's layer
            if let Some((_, pl)) = &owner {
                if !stack.iter().any(|&(a, b)| a == *pl || b == *pl) {
                    continue;
                }
            }
            let legal = stack.iter().all(|&(a, b)| {
                let spec = sizing.spec("", a, b);
                let (ia, ib) = (idx(a), idx(b));
                session.probe_via_span(center, pnet, a, b).legal
                    && copper[ia.min(ib)..=ia.max(ib)].iter().all(|&l| {
                        session
                            .probe(
                                &CopperGeom::Disc {
                                    center,
                                    r: spec.r(),
                                },
                                l,
                                pnet,
                                clr,
                            )
                            .legal
                    })
            });
            if !legal {
                continue;
            }
            let ext = |pick: fn(usize, usize) -> usize| {
                stack
                    .iter()
                    .flat_map(|&(a, b)| [idx(a), idx(b)])
                    .reduce(pick)
                    .map(|i| copper[i])
                    .unwrap()
            };
            let (a, b) = (ext(usize::min), ext(usize::max));
            site.spans.push(SpanSpec {
                a,
                b,
                stack: stack.clone(),
                cost: stack
                    .iter()
                    .map(|&(x, y)| {
                        if sizing.is_micro_span(x, y) {
                            params.micro_cost
                        } else {
                            params.via_cost
                        }
                    })
                    .sum(),
                r_center: stack
                    .iter()
                    .map(|&(x, y)| sizing.spec("", x, y).r())
                    .fold(0.0, f64::max)
                    + clr
                    + w0 / 2.0,
            });
        }
        if site.spans.is_empty() {
            continue;
        }
        grid.entry(k).or_default().push(p);
        out.push(site);
    }
    out
}

/// Via constructions that join two signal layers: single vias of the
/// board's catalog ([`super::access::span_catalog`]) that touch two signal
/// layers, and two-microvia stacks (when the board allows stacking) whose
/// extent does.
pub fn signal_stacks(
    pcb: &Pcb,
    sig: &[PcbLayer],
    sizing: &crate::via_sizing::ViaSizing,
) -> Vec<Vec<(PcbLayer, PcbLayer)>> {
    let cat = super::access::span_catalog(pcb);
    let touches = |a: PcbLayer, b: PcbLayer| sig.iter().filter(|l| l.spanned_by(a, b)).count();
    let mut out: Vec<Vec<(PcbLayer, PcbLayer)>> = cat
        .iter()
        .filter(|&&(a, b)| touches(a, b) >= 2)
        .map(|&s| vec![s])
        .collect();
    for &x in &cat {
        for &y in &cat {
            if x != y && x.1 == y.0 && sizing.may_stack(x, y) && touches(x.0, y.1) >= 2 {
                out.push(vec![x, y]);
            }
        }
    }
    out
}

impl<'g> FlowGraph<'g> {
    /// Build the routing graph over `cg`, whose Steiner vertices are the
    /// positions of `specs` (in order).
    pub fn build(cg: &'g CutGraph, specs: &[SiteSpec], cp: &CutParams) -> Self {
        let mut nets: Vec<String> = Vec::new();
        let mut net_id: HashMap<String, u32> = HashMap::new();
        let mut intern = |n: &str, nets: &mut Vec<String>| -> u32 {
            if n.is_empty() {
                return NONE;
            }
            if let Some(&i) = net_id.get(n) {
                return i;
            }
            let i = nets.len() as u32;
            nets.push(n.to_string());
            net_id.insert(n.to_string(), i);
            i
        };
        let mut layer_off = Vec::new();
        let mut n = 0u32;
        let mut obs_net = Vec::new();
        let mut arc_len = Vec::new();
        let mut cap = Vec::new();
        let mut free_nets = Vec::new();
        for lg in &cg.layers {
            layer_off.push(n);
            n += lg.tris.len() as u32;
            let on: Vec<u32> = lg
                .obstacles
                .iter()
                .map(|o| match o.kind {
                    ObsKind::Edge | ObsKind::Keepout => NONE,
                    _ => intern(&o.net, &mut nets),
                })
                .collect();
            let mut al = Vec::with_capacity(lg.cuts.len());
            let mut cpv = Vec::with_capacity(lg.cuts.len());
            let mut fr = Vec::with_capacity(lg.cuts.len());
            for (ci, c) in lg.cuts.iter().enumerate() {
                let a = lg.vpos(c.a);
                let b = lg.vpos(c.b);
                // Portal point: middle of the widest free part (or the cut
                // midpoint when fully covered).
                let mid = c
                    .parts
                    .iter()
                    .max_by(|x, y| (x[1] - x[0]).total_cmp(&(y[1] - y[0])))
                    .map(|p| (p[0] + p[1]) / 2.0)
                    .unwrap_or(c.len / 2.0);
                let u = mid / c.len.max(1e-12);
                let m = [a[0] + u * (b[0] - a[0]), a[1] + u * (b[1] - a[1])];
                let l = match (c.faces[0], c.faces[1]) {
                    (f, g) if f != NONE && g != NONE => {
                        dist(lg.tri_centroid(f), m) + dist(m, lg.tri_centroid(g))
                    }
                    _ => f64::INFINITY,
                };
                al.push(l);
                cpv.push(c.cap as f64);
                // Nets that cross for free: owners of copper the cut touches.
                let mut cand: Vec<u32> = Vec::new();
                for v in [c.a, c.b] {
                    let o = lg.vobs[v as usize];
                    if o != NONE && on[o as usize] != NONE {
                        cand.push(on[o as usize]);
                    }
                }
                for (_, o) in &c.blocks {
                    if *o != NONE && on[*o as usize] != NONE {
                        cand.push(on[*o as usize]);
                    }
                }
                cand.sort_unstable();
                cand.dedup();
                let free: Vec<u32> = cand
                    .into_iter()
                    .filter(|&nid| {
                        let name = &nets[nid as usize];
                        if c.parts.is_empty() {
                            // Fully covered: free only inside one net's copper.
                            !c.blocks.is_empty()
                                && c.blocks
                                    .iter()
                                    .all(|(_, o)| *o != NONE && on[*o as usize] == nid)
                        } else {
                            // A free part next to own copper still holds the
                            // wire for everyone else's purposes (it is ordered
                            // and spaced there like any other), so it is not
                            // free: only crossings wholly inside own copper are.
                            let _ = (ci, name);
                            false
                        }
                    })
                    .collect();
                fr.push(free);
            }
            obs_net.push(on);
            arc_len.push(al);
            cap.push(cpv);
            free_nets.push(fr);
        }
        let n_faces = n as usize;
        let nl = cg.layers.len();
        // Vertex → incident faces / cuts, per layer (only Steiner vertices
        // are asked for).
        let mut vfaces: Vec<HashMap<u32, Vec<u32>>> = vec![HashMap::new(); nl];
        let mut vcuts: Vec<HashMap<u32, Vec<u32>>> = vec![HashMap::new(); nl];
        for (li, lg) in cg.layers.iter().enumerate() {
            let st: std::collections::HashSet<u32> = lg.steiner.iter().copied().collect();
            for (f, t) in lg.tris.iter().enumerate() {
                for &v in t {
                    if st.contains(&v) {
                        vfaces[li].entry(v).or_default().push(f as u32);
                    }
                }
            }
            for (c, cut) in lg.cuts.iter().enumerate() {
                for v in [cut.a, cut.b] {
                    if st.contains(&v) {
                        vcuts[li].entry(v).or_default().push(c as u32);
                    }
                }
            }
        }
        let mut sites = Vec::with_capacity(specs.len());
        let mut site_faces = vec![Vec::new(); specs.len() * nl];
        let mut face_sites = vec![Vec::new(); n_faces];
        for (si, sp) in specs.iter().enumerate() {
            let mut spans = Vec::new();
            for ss in &sp.spans {
                let mut layers = Vec::new();
                let mut cuts = Vec::new();
                for (li, lg) in cg.layers.iter().enumerate() {
                    if !lg.layer.spanned_by(ss.a, ss.b) {
                        continue;
                    }
                    let v = lg.steiner[si];
                    layers.push(li as u8);
                    for &c in vcuts[li].get(&v).into_iter().flatten() {
                        let before = lg.cuts[c as usize].cap;
                        let after = lg.cap_with_disc(c, sp.pos, ss.r_center, cp);
                        if before > after {
                            cuts.push((li as u8, c, (before - after) as f64));
                        }
                    }
                }
                spans.push(SiteSpan {
                    a: ss.a,
                    b: ss.b,
                    stack: ss.stack.clone(),
                    layers,
                    cost: ss.cost,
                    cuts,
                    r_center: ss.r_center,
                });
            }
            for (li, lg) in cg.layers.iter().enumerate() {
                let v = lg.steiner[si];
                if lg.vobs[v as usize] != NONE {
                    continue; // merged into an obstacle vertex: no site here
                }
                let fs: Vec<u32> = vfaces[li]
                    .get(&v)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|&f| lg.tri_live[f as usize])
                    .collect();
                for &f in &fs {
                    face_sites[(layer_off[li] + f) as usize].push((si * nl + li) as u32);
                }
                site_faces[si * nl + li] = fs;
            }
            sites.push(ViaSite { pos: sp.pos, spans });
        }
        let n_nodes = n_faces + 2 * specs.len() * nl;
        Self {
            cg,
            layer_off,
            n_faces,
            n_nodes,
            site_lambda: vec![0.0; sites.len()],
            site_users: vec![Vec::new(); sites.len()],
            sites,
            face_sites,
            site_faces,
            nets,
            net_id,
            obs_net,
            arc_len,
            load: cap.iter().map(|c| vec![0.0; c.len()]).collect(),
            lambda: cap.iter().map(|c| vec![0.0; c.len()]).collect(),
            cap,
            free_nets,
        }
    }

    /// Node id of face `f` on graph layer `l`.
    pub fn node(&self, l: usize, f: u32) -> u32 {
        self.layer_off[l] + f
    }

    fn site_node(&self, s: u32, l: usize, out: bool) -> u32 {
        let nl = self.cg.layers.len();
        (self.n_faces + 2 * (s as usize * nl + l) + out as usize) as u32
    }

    /// Decode a node id.
    pub fn kind(&self, node: u32) -> Node {
        let n = node as usize;
        if n < self.n_faces {
            let l = self.layer_off.partition_point(|&o| o <= node) - 1;
            return Node::Face(l, node - self.layer_off[l]);
        }
        let nl = self.cg.layers.len();
        let k = n - self.n_faces;
        let (sl, out) = (k / 2, k % 2 == 1);
        let (s, l) = ((sl / nl) as u32, sl % nl);
        if out {
            Node::SiteOut(s, l)
        } else {
            Node::SiteIn(s, l)
        }
    }

    /// (layer, face) of a face node.
    pub fn split(&self, node: u32) -> (usize, u32) {
        match self.kind(node) {
            Node::Face(l, f) => (l, f),
            Node::SiteIn(_, l) | Node::SiteOut(_, l) => (l, NONE),
        }
    }

    fn node_pos(&self, node: u32) -> [f64; 2] {
        match self.kind(node) {
            Node::Face(l, f) => self.cg.layers[l].tri_centroid(f),
            Node::SiteIn(s, _) | Node::SiteOut(s, _) => self.sites[s as usize].pos,
        }
    }

    /// Graph layer index of a copper layer.
    pub fn layer_index(&self, l: PcbLayer) -> Option<usize> {
        self.cg.layers.iter().position(|g| g.layer == l)
    }

    pub(super) fn cross(&self, l: usize, c: u32, net: u32) -> Cross {
        if self.free_nets[l][c as usize].contains(&net) {
            return Cross::Free;
        }
        if self.cap[l][c as usize] > 0.0 {
            Cross::Consumes
        } else {
            Cross::Blocked
        }
    }

    pub(super) fn face_ok(&self, l: usize, f: u32, net: u32) -> bool {
        let lg = &self.cg.layers[l];
        if !lg.tri_live[f as usize] || lg.tri_block[f as usize] {
            return false;
        }
        let o = lg.tri_obs[f as usize];
        o == NONE || self.obs_net[l][o as usize] == net
    }

    /// Terminal nodes of a pad at `point` on graph layer `l`: the usable
    /// faces whose centroid lies inside the pad's raw copper (connected
    /// region around the point), or the face containing the point when the
    /// pad is smaller than its faces.
    pub fn pad_nodes(&self, l: usize, point: [f64; 2], net: u32) -> Vec<u32> {
        let lg = &self.cg.layers[l];
        let Some(f0) = lg.locate(point) else {
            return Vec::new();
        };
        let own: Vec<u32> = lg
            .obstacles_at(point, 0.0)
            .into_iter()
            .filter(|&o| {
                let ob = &lg.obstacles[o as usize];
                self.obs_net[l][o as usize] == net
                    && super::cdt::obstacle_contains(ob, point, -ob.grow)
            })
            .collect();
        let raw = |f: u32| {
            let c = lg.tri_centroid(f);
            own.iter().any(|&o| {
                let ob = &lg.obstacles[o as usize];
                super::cdt::obstacle_contains(ob, c, -ob.grow)
            })
        };
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![f0];
        while let Some(f) = stack.pop() {
            if !seen.insert(f) || !raw(f) {
                continue;
            }
            if self.face_ok(l, f, net) {
                out.push(self.node(l, f));
            }
            for k in 0..3 {
                let g = lg.tri_nbr[f as usize][k];
                if g != NONE {
                    stack.push(g);
                }
            }
        }
        if out.is_empty() && lg.tri_live[f0 as usize] {
            out.push(self.node(l, f0));
        }
        out.sort_unstable();
        out
    }

    fn site_used_by_other(&self, s: u32, net: u32) -> bool {
        self.site_users[s as usize].iter().any(|&o| o != net)
    }

    /// A* from `src` to `dst` at current prices. `pres` scales the present
    /// congestion term (0 for the Lagrangian pass). Returns the path and its
    /// *priced* cost.
    pub fn shortest(
        &self,
        k: &Commodity,
        pres: f64,
        params: &FlowParams,
    ) -> Option<(FlowPath, f64)> {
        let n = self.n_nodes;
        let mut g = vec![f64::INFINITY; n];
        let mut pred: Vec<(u32, Option<Step>)> = vec![(NONE, None); n];
        let dst_set: std::collections::HashSet<u32> = k.dst.nodes.iter().copied().collect();
        let tgt = k.dst.point;
        let slack = k
            .dst
            .nodes
            .iter()
            .map(|&nd| dist(self.node_pos(nd), tgt))
            .fold(0.0f64, f64::max);
        let h = |nd: u32| (dist(self.node_pos(nd), tgt) - slack).max(0.0);
        let mut heap = BinaryHeap::new();
        for &s in &k.src.nodes {
            g[s as usize] = 0.0;
            heap.push(Item {
                f: h(s),
                g: 0.0,
                node: s,
            });
        }
        let mut found = NONE;
        let nl = self.cg.layers.len();
        let relax = |heap: &mut BinaryHeap<Item>,
                     g: &mut Vec<f64>,
                     pred: &mut Vec<(u32, Option<Step>)>,
                     u: u32,
                     v: u32,
                     nv: f64,
                     st: Step| {
            if nv < g[v as usize] {
                g[v as usize] = nv;
                pred[v as usize] = (u, Some(st));
                heap.push(Item {
                    f: nv + h(v),
                    g: nv,
                    node: v,
                });
            }
        };
        while let Some(Item { g: gu, node: u, .. }) = heap.pop() {
            if gu > g[u as usize] {
                continue;
            }
            if dst_set.contains(&u) {
                found = u;
                break;
            }
            match self.kind(u) {
                Node::Face(l, f) => {
                    let lg = &self.cg.layers[l];
                    for kk in 0..3 {
                        let c = lg.tri_cut[f as usize][kk];
                        let nb = lg.tri_nbr[f as usize][kk];
                        // a blocked face is still enterable when it is the
                        // destination pad's own face (small pads)
                        if nb == NONE
                            || (!self.face_ok(l, nb, k.net_id)
                                && !dst_set.contains(&self.node(l, nb)))
                        {
                            continue;
                        }
                        let mode = self.cross(l, c, k.net_id);
                        if mode == Cross::Blocked {
                            continue;
                        }
                        let len = self.arc_len[l][c as usize];
                        if !len.is_finite() {
                            continue;
                        }
                        let mut w = len;
                        if mode == Cross::Consumes {
                            let ci = c as usize;
                            w += k.units * self.lambda[l][ci];
                            let excess = (self.load[l][ci] + k.units - self.cap[l][ci]).max(0.0);
                            w += pres * excess * len.max(0.1);
                        }
                        let st = Step::Cross {
                            layer: l as u8,
                            cut: c,
                            from: f,
                            consumes: mode == Cross::Consumes,
                        };
                        relax(
                            &mut heap,
                            &mut g,
                            &mut pred,
                            u,
                            self.node(l, nb),
                            gu + w,
                            st,
                        );
                    }
                    let fnode = self.node(l, f) as usize;
                    for &sl in &self.face_sites[fnode] {
                        let s = sl / nl as u32;
                        let w = dist(lg.tri_centroid(f), self.sites[s as usize].pos);
                        let st = Step::Site {
                            site: s,
                            layer: l as u8,
                            face: f,
                        };
                        relax(
                            &mut heap,
                            &mut g,
                            &mut pred,
                            u,
                            self.site_node(s, l, false),
                            gu + w,
                            st,
                        );
                    }
                }
                Node::SiteIn(s, l) => {
                    let site = &self.sites[s as usize];
                    let other = self.site_used_by_other(s, k.net_id);
                    for (si, sp) in site.spans.iter().enumerate() {
                        if !sp.layers.contains(&(l as u8)) {
                            continue;
                        }
                        let mut w = sp.cost + self.site_lambda[s as usize];
                        for &(cl, cc, units) in &sp.cuts {
                            let (cl, cc) = (cl as usize, cc as usize);
                            w += units * self.lambda[cl][cc];
                            let excess = (self.load[cl][cc] + units - self.cap[cl][cc]).max(0.0);
                            w += pres * excess;
                        }
                        if pres > 0.0 && other {
                            w += params.via_block * (1.0 + pres);
                        }
                        for &to in &sp.layers {
                            if to as usize == l {
                                continue;
                            }
                            let st = Step::Via {
                                site: s,
                                span: si as u16,
                                from: l as u8,
                                to,
                            };
                            let v = self.site_node(s, to as usize, true);
                            relax(&mut heap, &mut g, &mut pred, u, v, gu + w, st);
                        }
                    }
                }
                Node::SiteOut(s, l) => {
                    let lg = &self.cg.layers[l];
                    for &f in &self.site_faces[s as usize * nl + l] {
                        if !self.face_ok(l, f, k.net_id) {
                            continue;
                        }
                        let w = dist(lg.tri_centroid(f), self.sites[s as usize].pos);
                        let st = Step::Site {
                            site: s,
                            layer: l as u8,
                            face: f,
                        };
                        relax(&mut heap, &mut g, &mut pred, u, self.node(l, f), gu + w, st);
                    }
                }
            }
        }
        if found == NONE {
            return None;
        }
        let priced = g[found as usize];
        let mut steps = Vec::new();
        let mut cur = found;
        let mut start = found;
        while let (p, Some(st)) = pred[cur as usize] {
            steps.push(st);
            cur = p;
            start = p;
        }
        steps.reverse();
        let mut path = FlowPath {
            start,
            end: found,
            steps,
            cost: 0.0,
        };
        path.cost = self.primal_cost(&path);
        Some((path, priced))
    }

    /// Length + via cost of a path (no prices).
    pub fn primal_cost(&self, p: &FlowPath) -> f64 {
        p.steps
            .iter()
            .map(|s| match *s {
                Step::Cross { layer, cut, .. } => self.arc_len[layer as usize][cut as usize],
                Step::Site { site, layer, face } => dist(
                    self.cg.layers[layer as usize].tri_centroid(face),
                    self.sites[site as usize].pos,
                ),
                Step::Via { site, span, .. } => self.sites[site as usize].spans[span as usize].cost,
            })
            .sum()
    }

    fn apply(&mut self, k: &Commodity, p: &FlowPath, sign: f64) {
        for s in &p.steps {
            match *s {
                Step::Cross {
                    layer,
                    cut,
                    consumes: true,
                    ..
                } => self.load[layer as usize][cut as usize] += sign * k.units,
                Step::Via { site, span, .. } => {
                    let sp = &self.sites[site as usize].spans[span as usize];
                    for &(cl, cc, units) in &sp.cuts {
                        self.load[cl as usize][cc as usize] += sign * units;
                    }
                    let u = &mut self.site_users[site as usize];
                    if sign > 0.0 {
                        u.push(k.net_id);
                    } else if let Some(i) = u.iter().position(|&x| x == k.net_id) {
                        u.remove(i);
                    }
                }
                _ => {}
            }
        }
    }

    fn overflow(&self) -> (f64, usize) {
        let mut tot = 0.0;
        let mut n = 0;
        for (ls, cs) in self.load.iter().zip(&self.cap) {
            for (l, c) in ls.iter().zip(cs) {
                if *l > c + 1e-9 {
                    tot += l - c;
                    n += 1;
                }
            }
        }
        (tot, n)
    }

    fn site_conflicts(&self) -> usize {
        self.site_users
            .iter()
            .filter(|u| {
                let mut v = (*u).clone();
                v.sort_unstable();
                v.dedup();
                v.len() > 1
            })
            .count()
    }

    /// Run the price loop. Returns per-commodity paths (None = unroutable)
    /// of the best iterate (least overflow + site conflicts, then cost) and
    /// the per-iteration statistics.
    pub fn negotiate(
        &mut self,
        ks: &[Commodity],
        params: &FlowParams,
    ) -> (Vec<Option<FlowPath>>, Vec<IterStat>) {
        let mut paths: Vec<Option<FlowPath>> = vec![None; ks.len()];
        let mut stats = Vec::new();
        let mut best: Option<(f64, f64, Vec<Option<FlowPath>>)> = None;
        // Run to feasibility (overflow 0, no site conflict) or a provable
        // stall: `stall` iterations without a better (overflow, conflicts)
        // score. `iters` is the hard work budget.
        let mut since_best = 0usize;
        let mut best_score = f64::INFINITY;
        for it in 0..params.iters {
            let pres = if it == 0 {
                0.0
            } else {
                (params.pres0 * params.pres_growth.powi(it as i32 - 1)).min(1e4)
            };
            for (i, k) in ks.iter().enumerate() {
                if let Some(p) = paths[i].take() {
                    self.apply(k, &p, -1.0);
                }
                if let Some((p, _)) = self.shortest(k, pres, params) {
                    self.apply(k, &p, 1.0);
                    paths[i] = Some(p);
                }
            }
            let (ovf, ovf_n) = self.overflow();
            let cost: f64 = paths.iter().flatten().map(|p| p.cost).sum();
            // Lagrangian bound at the current prices alone.
            let mut lb: f64 = 0.0;
            for k in ks {
                if let Some((_, c)) = self.shortest(k, 0.0, params) {
                    lb += c;
                }
            }
            let lam_cap: f64 = self
                .lambda
                .iter()
                .zip(&self.cap)
                .map(|(ls, cs)| ls.iter().zip(cs).map(|(l, c)| l * c).sum::<f64>())
                .sum::<f64>()
                + self.site_lambda.iter().sum::<f64>();
            lb -= lam_cap;
            let conflicts = self.site_conflicts();
            let unrouted = paths.iter().filter(|p| p.is_none()).count();
            let vias = paths
                .iter()
                .flatten()
                .flat_map(|p| p.steps.iter())
                .filter(|s| matches!(s, Step::Via { .. }))
                .count();
            stats.push(IterStat {
                round: 0,
                iter: it,
                overflow: ovf,
                overflowed_cuts: ovf_n,
                cost,
                lower_bound: lb,
                unrouted,
                site_conflicts: conflicts,
                vias,
            });
            log::info!(
                "flow it {it}: overflow {ovf:.2} on {ovf_n} cuts, cost {cost:.1} mm, LB {lb:.1}, \
                 unrouted {unrouted}, site conflicts {conflicts}, vias {vias}"
            );
            let score = (ovf + conflicts as f64, cost);
            if best
                .as_ref()
                .is_none_or(|b| score.0 < b.0 - 1e-9 || (score.0 <= b.0 + 1e-9 && score.1 < b.1))
            {
                best = Some((score.0, score.1, paths.clone()));
            }
            if ovf <= 1e-9 && conflicts == 0 {
                break;
            }
            let sc = ovf + conflicts as f64;
            if sc < best_score - 1e-9 {
                best_score = sc;
                since_best = 0;
            } else {
                since_best += 1;
                if since_best >= params.stall {
                    log::info!("flow market: stalled at overflow {ovf:.2} after {it} iterations");
                    break;
                }
            }
            // Price step: projected subgradient, diminishing step.
            let a = params.alpha / ((it + 1) as f64).sqrt();
            for (l, ls) in self.lambda.iter_mut().enumerate() {
                for (c, lam) in ls.iter_mut().enumerate() {
                    *lam = (*lam + a * (self.load[l][c] - self.cap[l][c])).max(0.0);
                }
            }
            for (s, lam) in self.site_lambda.iter_mut().enumerate() {
                let mut u = self.site_users[s].clone();
                u.sort_unstable();
                u.dedup();
                *lam = (*lam + a * (u.len() as f64 - 1.0)).max(0.0);
            }
        }
        if log::log_enabled!(log::Level::Debug) {
            self.log_overflow(ks, &paths);
        }
        let paths = best.map(|b| b.2).unwrap_or(paths);
        (paths, stats)
    }

    fn log_overflow(&self, ks: &[Commodity], paths: &[Option<FlowPath>]) {
        let mut users: HashMap<(usize, u32), Vec<String>> = HashMap::new();
        for (k, p) in ks.iter().zip(paths) {
            for st in p.iter().flat_map(|p| p.steps.iter()) {
                match *st {
                    Step::Cross {
                        layer,
                        cut,
                        consumes: true,
                        ..
                    } => users
                        .entry((layer as usize, cut))
                        .or_default()
                        .push(k.net.clone()),
                    Step::Via { site, span, .. } => {
                        for &(l, c, _) in &self.sites[site as usize].spans[span as usize].cuts {
                            users
                                .entry((l as usize, c))
                                .or_default()
                                .push(format!("via:{}", k.net));
                        }
                    }
                    _ => {}
                }
            }
        }
        for (l, ls) in self.load.iter().enumerate() {
            for (c, ld) in ls.iter().enumerate() {
                if *ld > self.cap[l][c] + 1e-9 {
                    let cut = &self.cg.layers[l].cuts[c];
                    let a = self.cg.layers[l].vpos(cut.a);
                    log::debug!(
                        "overflow {:?} cut {c} @({:.2},{:.2}) len {:.3} cap {} load {:.2}: {:?}",
                        self.cg.layers[l].layer,
                        a[0],
                        a[1],
                        cut.len,
                        self.cap[l][c],
                        ld,
                        users.get(&(l, c as u32))
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::flow::cdt::{CopperFilter, CutGraph, CutParams};

    /// One layer; nets A and B both want the single-wire gap between the two
    /// X pads. The market must clear (one of them goes around) and its
    /// Lagrangian bound must stay below the primal cost.
    #[test]
    fn market_clears_a_one_wire_gap() {
        let mut pcb = crate::router::flow::test_board(20.0, 20.0, &["FCu"]);
        let pad = |num: &str, x: f64, y: f64, w: f64, h: f64, net: &str| {
            serde_json::json!({"number": num, "padType": "SMD",
                "shape": {"type": "Rect", "width": w, "height": h},
                "position": {"x": x, "y": y}, "rotation": 0.0,
                "net": net, "layers": ["FCu"]})
        };
        // X pads leave a 9.65..10.35 = 0.7 mm copper gap: one 0.2 mm wire
        // with 0.2 mm clearance each side, not two.
        let fp: vcad_ir::ecad::Footprint = serde_json::from_value(serde_json::json!({
            "ref": "U1", "value": "", "footprintName": "T", "position": {"x": 0.0, "y": 0.0},
            "rotation": 0.0, "front": true, "graphics": [],
            "pads": [pad("1", 10.0, 6.65, 1.0, 6.0, "X"), pad("2", 10.0, 13.35, 1.0, 6.0, "X"),
                     pad("3", 2.0, 9.6, 0.6, 0.6, "A"), pad("4", 18.0, 9.6, 0.6, 0.6, "A"),
                     pad("5", 2.0, 10.4, 0.6, 0.6, "B"), pad("6", 18.0, 10.4, 0.6, 0.6, "B")]
        }))
        .unwrap();
        pcb.footprints.push(fp);
        let params = FlowParams::default();
        let cp = CutParams::from_pcb(&pcb);
        let cg = CutGraph::build(&pcb, cp, CopperFilter::All);
        let mut g = FlowGraph::build(&cg, &[], &cp);
        let session = RouteSession::from_pcb(&pcb);
        let skip = std::collections::BTreeSet::from(["X".to_string()]);
        let ks = crate::router::flow::route::commodities(&pcb, &g, &session, 0.2, &[], &skip);
        assert_eq!(ks.len(), 2);
        let (paths, stats) = g.negotiate(&ks, &params);
        assert!(paths.iter().all(|p| p.is_some()));
        let last = stats.last().unwrap();
        assert_eq!(last.overflow, 0.0, "{stats:?}");
        // First iteration: both take the gap (overflow); the market prices
        // one of them around.
        assert!(stats[0].overflow > 0.0, "{stats:?}");
        let lb = stats.iter().map(|s| s.lower_bound).fold(f64::MIN, f64::max);
        assert!(lb <= last.cost + 1e-9, "LB {lb} > cost {}", last.cost);
        assert!(lb > 0.0);
    }
}
