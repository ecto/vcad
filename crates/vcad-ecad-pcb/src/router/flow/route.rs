//! `route_flow`: the flow-core routing pass end to end.
//!
//! 1. Cut graph per signal layer ([`super::cdt`]) over the board's existing
//!    copper.
//! 2. Commodities: every net's pads, MST-decomposed into 2-pin connections
//!    (nets carried by a pour are left to plane access — not routed here).
//! 3. Price loop ([`super::flow`]): overflow, cost and the Lagrangian bound per
//!    iteration.
//! 4. Realization: each route's per-layer word → shortest homotopic path
//!    ([`super::funnel`]); several wires through one cut get spaced slots.
//! 5. Fail-closed gate: every connection is probed against the exact session
//!    oracle before it is committed (rejected connections stay unrouted),
//!    then the router's standard legalization runs.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use super::cdt::{clip_obstacle, orient, CopperFilter, CutGraph, CutParams, NONE};
use super::embed::{EPath, EStep, Embedding};
use super::flow::{
    choose_sites, Commodity, FlowGraph, FlowParams, FlowPath, IterStat, Step, Terminal,
};
use super::realize::{realize_wires, Wire, Wx};
use crate::router::{RouteAllResult, RoutedTrace, RoutedVia};
use crate::session::{RouteSession, SpanId};
use crate::spatial::{CopperElement, CopperGeom};

/// Options of [`route_flow`].
#[derive(Debug, Clone)]
pub struct FlowOptions {
    /// Price-loop tuning.
    pub params: FlowParams,
    /// Max rounds (market + commit on the copper committed so far).
    pub rounds: usize,
}

impl Default for FlowOptions {
    fn default() -> Self {
        Self {
            params: FlowParams::default(),
            rounds: 10,
        }
    }
}

/// What the flow pass did.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct FlowReport {
    /// Graph vertices / faces / cuts (all layers).
    pub graph: [usize; 3],
    /// Via sites placed.
    pub sites: usize,
    /// 2-pin connections planned.
    pub commodities: usize,
    /// Connections with a planned path.
    pub planned: usize,
    /// Connections whose geometry passed the oracle and was committed.
    pub committed: usize,
    /// Connections the oracle rejected.
    pub rejected: usize,
    /// Nets left to planes (not routed by this pass).
    pub plane_nets: usize,
    /// Per-iteration market statistics.
    pub iters: Vec<IterStat>,
    /// Best Lagrangian lower bound (mm).
    pub lower_bound: f64,
    /// Cost of the best capacity-feasible iterate, when one was reached (mm).
    pub upper_bound: Option<f64>,
    /// Relative duality gap `(UB − LB) / UB` when feasible.
    pub gap: Option<f64>,
    /// Timings (ms): graph, negotiation, realization+oracle, legalization.
    pub ms: [f64; 4],
    /// Per round: [open connections, committed, rejected].
    pub rounds: Vec<[usize; 3]>,
    /// Plane access: [pads needing a via, pads without a legal site, vias
    /// committed].
    pub access: [usize; 3],
    /// Final duality gap of the access auction.
    pub access_gap: Option<f64>,
    /// Open connections at the end, by cause.
    pub causes: BTreeMap<String, usize>,
    /// Per net with an open connection: the cause of its first one.
    pub net_causes: BTreeMap<String, String>,
    /// Plane nets with a pad that has no legal access site.
    pub access_unsited: BTreeMap<String, usize>,
}

#[derive(Debug, Clone)]
struct PadT {
    center: [f64; 2],
    layers: Vec<PcbLayer>,
}

/// One crossing of a run.
struct Px {
    cut: u32,
    from: u32,
    /// Free part used (`None` for an own-copper crossing).
    part: Option<u16>,
    /// Free extent (mm from cut.a) and the room kept from its ends.
    lo: f64,
    hi: f64,
    pad: f64,
    pad_hi: f64,
}

/// One per-layer run of a realized route.
struct Run {
    layer: usize,
    start: [f64; 2],
    end: [f64; 2],
    /// Inside the start pad: walk to this face centroid before the funnel.
    pre: Option<[f64; 2]>,
    /// Inside the end pad: the funnel ends here, then a stub to `end`.
    post: Option<[f64; 2]>,
    px: Vec<Px>,
}

/// The neck rule `(width, length)` governing `net` (its class, else the
/// default rules — where an inferred neck lives).
fn neck_rule(pcb: &Pcb, net: &str) -> Option<(f64, f64)> {
    let class = pcb
        .rules
        .net_class_assignments
        .iter()
        .find(|(_, nets)| nets.iter().any(|n| n == net))
        .and_then(|(name, _)| pcb.rules.class_rules.iter().find(|c| &c.name == name))
        .unwrap_or(&pcb.rules.default_rules);
    class.neck_width.map(|w| {
        (
            w,
            class
                .neck_length
                .unwrap_or(vcad_ir::ecad::DEFAULT_NECK_LENGTH),
        )
    })
}

fn d2(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Build the commodities of `pcb` (MST per net over its pad centers).
pub(super) fn commodities(
    pcb: &Pcb,
    g: &FlowGraph,
    session: &RouteSession,
    width: f64,
    nets_filter: &[String],
    skip: &BTreeSet<String>,
) -> Vec<Commodity> {
    let mut by_net: BTreeMap<String, Vec<PadT>> = BTreeMap::new();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            let Some(net) = pad.net.as_deref().filter(|n| !n.is_empty()) else {
                continue;
            };
            let c = crate::geometry::pad_world_position(fp, pad);
            let mut layers: Vec<PcbLayer> = pad
                .layers
                .iter()
                .copied()
                .filter(|l| l.is_copper())
                .collect();
            if pad.drill.is_some() {
                layers = g.cg.layers.iter().map(|l| l.layer).collect();
            }
            by_net.entry(net.to_string()).or_default().push(PadT {
                center: [c.x, c.y],
                layers,
            });
        }
    }
    let w0 = g.cg.params.width;
    let s0 = g.cg.params.clearance;
    let mut out = Vec::new();
    let areas = std::sync::Arc::new(pcb.rules.rule_areas.clone());
    for (net, pads) in by_net {
        if pads.len() < 2 || skip.contains(&net) {
            continue;
        }
        if !nets_filter.is_empty() && !nets_filter.contains(&net) {
            continue;
        }
        let Some(&nid) = g.net_id.get(&net) else {
            continue;
        };
        // Prim MST over pad centers (deterministic: lowest index ties).
        let n = pads.len();
        let mut inn = vec![false; n];
        let mut best = vec![(f64::INFINITY, 0usize); n];
        inn[0] = true;
        for j in 1..n {
            best[j] = (d2(pads[0].center, pads[j].center), 0);
        }
        let w = session.width_for(&net, width);
        let s = session.clearance_for(&net);
        let units = (w + s) / (w0 + s0);
        let neck = neck_rule(pcb, &net)
            .filter(|&(nw, _)| nw < w - 1e-9)
            .map(|(nw, len)| (nw, (nw + s) / (w0 + s0), len));
        let area = areas
            .iter()
            .map(|a| a.min_trace_width)
            .fold(None::<f64>, |m, x| Some(m.map_or(x, |m| m.min(x))))
            .filter(|&aw| aw < w - 1e-9)
            .map(|aw| (aw, (aw + s) / (w0 + s0), areas.clone()));
        for _ in 1..n {
            let (j, _) = (0..n)
                .filter(|&j| !inn[j])
                .map(|j| (j, best[j].0))
                .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
                .unwrap();
            inn[j] = true;
            let i = best[j].1;
            let term = |p: &PadT| Terminal {
                point: p.center,
                nodes: p
                    .layers
                    .iter()
                    .filter_map(|&l| g.layer_index(l))
                    .flat_map(|li| g.pad_nodes(li, p.center, nid))
                    .collect(),
            };
            let (a, b) = (term(&pads[i]), term(&pads[j]));
            // Coincident pads (same position) need no wire.
            if d2(a.point, b.point) > 1e-6 {
                out.push(Commodity {
                    net: net.clone(),
                    net_id: nid,
                    width: w,
                    units,
                    neck,
                    area: area.clone(),
                    src: a,
                    dst: b,
                });
            }
            for k in 0..n {
                if !inn[k] {
                    let d = d2(pads[j].center, pads[k].center);
                    if d < best[k].0 {
                        best[k] = (d, j);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| {
        d2(a.src.point, a.dst.point)
            .total_cmp(&d2(b.src.point, b.dst.point))
            .then(a.net.cmp(&b.net))
    });
    out
}

/// Split an embedded path into per-layer runs with portals: the free part the
/// wire was embedded in (shrunk by `eps`), or the whole cut for an own-copper
/// crossing.
fn runs_of(g: &FlowGraph, emb: &Embedding, k: &Commodity, p: &EPath, eps: f64) -> Vec<Run> {
    // A wire wider than the graph's design width needs the extra half-width
    // clear on both sides of its portal.
    // (Per crossing: the neck width in the wire's own pad neck regions.)
    let eps0 = eps;
    let (l0, f0) = g.split(p.start);
    let (le, fe) = g.split(p.end);
    let contains = |l: usize, f: u32, q: [f64; 2]| {
        let lg = &g.cg.layers[l];
        let t = lg.tris[f as usize];
        let (a, b, c) = (lg.vpos(t[0]), lg.vpos(t[1]), lg.vpos(t[2]));
        orient(a, b, q) >= -1e-12 && orient(b, c, q) >= -1e-12 && orient(c, a, q) >= -1e-12
    };
    let pre = (!contains(l0, f0, k.src.point)).then(|| g.cg.layers[l0].tri_centroid(f0));
    let mut runs = vec![Run {
        layer: l0,
        start: k.src.point,
        end: k.dst.point,
        pre,
        post: None,
        px: Vec::new(),
    }];
    for st in &p.steps {
        match *st {
            EStep::Cross {
                layer,
                cut,
                from,
                part,
            } => {
                let lg = &g.cg.layers[layer as usize];
                let c = &lg.cuts[cut as usize];
                let (pa, pb) = (lg.vpos(c.a), lg.vpos(c.b));
                let eps = eps0
                    + ((k.width_at(g.cut_mid(layer as usize, cut)) - g.cg.params.width) / 2.0)
                        .max(0.0);
                let (s0, s1, pad, pad_hi) = match part {
                    Some(pt) => {
                        // a used via site at a cut end is a corner too: the
                        // extent runs to the site vertex and the wire keeps
                        // the barrel's radius from it
                        let raw = c.parts[pt as usize];
                        let q = emb.bounds(layer, cut, pt);
                        let (lo, pl) = if q[0] > raw[0] + 1e-9 {
                            (0.0, q[0] + eps)
                        } else {
                            (raw[0], eps)
                        };
                        let (hi, ph) = if q[1] < raw[1] - 1e-9 {
                            (c.len, c.len - q[1] + eps)
                        } else {
                            (raw[1], eps)
                        };
                        (lo, hi, pl, ph)
                    }
                    None => {
                        // Own-copper crossing: keep the wire inside the raw
                        // copper of its own pad (the clearance ring around it
                        // is where other nets' wires run).
                        let mut own_obs: Vec<u32> = c
                            .blocks
                            .iter()
                            .map(|(_, o)| *o)
                            .chain([lg.vobs[c.a as usize], lg.vobs[c.b as usize]])
                            .filter(|&o| o != NONE && lg.obstacles[o as usize].net == k.net)
                            .collect();
                        own_obs.sort_unstable();
                        own_obs.dedup();
                        let mut iv = Vec::new();
                        for o in own_obs {
                            let mut raw = lg.obstacles[o as usize].clone();
                            raw.grow = 0.0;
                            clip_obstacle(pa, pb, &raw, &mut iv);
                        }
                        let best = iv
                            .iter()
                            .max_by(|x, y| (x[1] - x[0]).total_cmp(&(y[1] - y[0])))
                            .map(|x| [x[0] * c.len, x[1] * c.len]);
                        match best {
                            Some(x) if x[1] - x[0] > 2.0 * eps => {
                                (x[0] + eps, x[1] - eps, 0.0, 0.0)
                            }
                            Some(x) => ((x[0] + x[1]) / 2.0, (x[0] + x[1]) / 2.0, 0.0, 0.0),
                            None => (c.len / 2.0, c.len / 2.0, 0.0, 0.0),
                        }
                    }
                };
                runs.last_mut().unwrap().px.push(Px {
                    cut,
                    from,
                    part,
                    lo: s0,
                    hi: s1,
                    pad,
                    pad_hi,
                });
            }
            EStep::Via { site, to, .. } => {
                let pos = g.sites[site as usize].pos;
                runs.last_mut().unwrap().end = pos;
                runs.push(Run {
                    layer: to as usize,
                    start: pos,
                    end: k.dst.point,
                    pre: None,
                    post: None,
                    px: Vec::new(),
                });
            }
        }
    }
    if let Some(last) = runs.last_mut() {
        if last.layer == le && !contains(le, fe, k.dst.point) {
            last.post = Some(g.cg.layers[le].tri_centroid(fe));
        }
    }
    runs
}

/// Candidate geometry of each commodity: embed it planarly at the market's
/// prices ([`Embedding`]), funnel every per-layer run, spread the wires that
/// share a free part into slots in their embedded order, funnel again.
/// Commodities the embedding cannot place come back empty.
fn realize(
    cg: &CutGraph,
    g: &FlowGraph,
    ks: &[Commodity],
    plan: &[Option<FlowPath>],
    session: &RouteSession,
    params: &FlowParams,
) -> Vec<(Vec<RoutedTrace>, Vec<RoutedVia>)> {
    let eps = 0.01;
    let mut emb = Embedding::new(g);
    let plan_cuts = |p: &Option<FlowPath>| -> Vec<(u8, u32)> {
        p.iter()
            .flat_map(|p| p.steps.iter())
            .filter_map(|st| match *st {
                Step::Cross {
                    layer,
                    cut,
                    consumes: true,
                    ..
                } => Some((layer, cut)),
                _ => None,
            })
            .collect()
    };
    for p in plan {
        for key in plan_cuts(p) {
            *emb.reserved.entry(key).or_default() += 1;
        }
    }
    // The plan's corridor: its faces plus two rings of neighbours.
    let corridor = |p: &FlowPath| {
        let mut set: std::collections::HashSet<u32> = std::collections::HashSet::new();
        set.insert(p.start);
        set.insert(p.end);
        for st in &p.steps {
            match *st {
                Step::Cross {
                    layer, cut, from, ..
                } => {
                    let lg = &cg.layers[layer as usize];
                    set.insert(g.node(layer as usize, from));
                    for f in lg.cuts[cut as usize].faces {
                        if f != NONE {
                            set.insert(g.node(layer as usize, f));
                        }
                    }
                }
                Step::Site { layer, face, .. } => {
                    set.insert(g.node(layer as usize, face));
                }
                Step::Via { .. } => {}
            }
        }
        for _ in 0..2 {
            let cur: Vec<u32> = set.iter().copied().collect();
            for nd in cur {
                let (l, f) = g.split(nd);
                if f == NONE {
                    continue;
                }
                for &nb in &cg.layers[l].tri_nbr[f as usize] {
                    if nb != NONE {
                        set.insert(g.node(l, nb));
                    }
                }
            }
        }
        set
    };
    let epaths: Vec<Option<EPath>> = ks
        .iter()
        .enumerate()
        .map(|(ki, k)| {
            // This commodity's own promises are its to use now.
            for (l, c) in plan_cuts(&plan[ki]) {
                if let Some(r) = emb.reserved.get_mut(&(l, c)) {
                    *r = r.saturating_sub(1);
                }
            }
            let near = plan[ki].as_ref().map(corridor);
            emb.route(ki as u32, k, params, near.as_ref(), 40_000)
                .or_else(|| emb.route(ki as u32, k, params, None, 150_000))
                .or_else(|| {
                    // Last resort: the plan's promises to later commodities
                    // are soft — take promised room rather than drop this one.
                    let held = std::mem::take(&mut emb.reserved);
                    let r = emb.route(ki as u32, k, params, None, 150_000);
                    emb.reserved = held;
                    r
                })
        })
        .collect();
    log::info!(
        "flow embed: {} placed, {} hit the work bound, {} exhausted",
        epaths.iter().filter(|p| p.is_some()).count(),
        emb.stats[0],
        emb.stats[1]
    );
    let all_runs: Vec<Vec<Run>> = ks
        .iter()
        .zip(&epaths)
        .map(|(k, p)| {
            p.as_ref()
                .map(|p| runs_of(g, &emb, k, p, eps))
                .unwrap_or_default()
        })
        .collect();
    // One wire per run; the embedding's order along every cut.
    let mut wires: Vec<Wire> = Vec::new();
    let mut wire_of: HashMap<(usize, usize), usize> = HashMap::new();
    for (ki, runs) in all_runs.iter().enumerate() {
        let k = &ks[ki];
        for (ri, r) in runs.iter().enumerate() {
            wire_of.insert((ki, ri), wires.len());
            wires.push(Wire {
                layer: r.layer,
                width: k.width,
                clearance: session.clearance_for(&k.net),
                net: k.net_id,
                net_name: k.net.clone(),
                start_path: None,
                twin: twin_of(&k.net, session, &g.net_id),
                start: r.start,
                end: r.end,
                lead: r.pre,
                tail: r.post,
                neck: k
                    .neck
                    .map(|(nw, _, rad)| (nw, rad, vec![k.src.point, k.dst.point])),
                area: k.area.as_ref().map(|(aw, _, a)| (*aw, a.clone())),
                xs: r
                    .px
                    .iter()
                    .map(|x| Wx {
                        w: k.width_at(g.cut_mid(r.layer, x.cut)),
                        cut: x.cut,
                        from: x.from,
                        lo: x.lo,
                        hi: x.hi,
                        pad: x.pad,
                        pad_hi: x.pad_hi,
                    })
                    .collect(),
            });
        }
    }
    let mut order: HashMap<(usize, u32), Vec<usize>> = HashMap::new();
    for (ki, runs) in all_runs.iter().enumerate() {
        for (ri, r) in runs.iter().enumerate() {
            for x in &r.px {
                if x.part.is_none() || order.contains_key(&(r.layer, x.cut)) {
                    continue;
                }
                let c = &cg.layers[r.layer].cuts[x.cut as usize];
                let mut ord = Vec::new();
                for pt in 0..c.parts.len() as u16 {
                    for &w in emb.part_wires(r.layer as u8, x.cut, pt) {
                        // the run of commodity `w` that crosses this cut
                        let runs_w = &all_runs[w as usize];
                        if let Some(rw) = runs_w
                            .iter()
                            .position(|q| q.layer == r.layer && q.px.iter().any(|y| y.cut == x.cut))
                        {
                            ord.push(wire_of[&(w as usize, rw)]);
                        }
                    }
                }
                let _ = (ki, ri);
                order.insert((r.layer, x.cut), ord);
            }
        }
    }
    // The gate sees the planned vias too: a wire must not hug a barrel the
    // embedding placed for another net.
    let mut gate = session.clone();
    {
        let sizing = session.via_sizing().clone();
        for (k, p) in ks.iter().zip(&epaths) {
            for st in p.iter().flat_map(|p| p.steps.iter()) {
                if let EStep::Via { site, span, .. } = *st {
                    let sp = &g.sites[site as usize].spans[span as usize];
                    let pos =
                        Vec2::new(g.sites[site as usize].pos[0], g.sites[site as usize].pos[1]);
                    for &(a, b) in &sp.stack {
                        let spec = sizing.spec(&k.net, a, b);
                        let r = spec.r();
                        for l in
                            g.cg.layers
                                .iter()
                                .map(|l| l.layer)
                                .filter(|l| l.spanned_by(a, b))
                        {
                            gate.commit(CopperElement {
                                min: [pos.x - r, pos.y - r],
                                max: [pos.x + r, pos.y + r],
                                net: k.net.clone(),
                                layer: l,
                                geom: CopperGeom::Disc { center: pos, r },
                            });
                        }
                    }
                }
            }
        }
    }
    let polys = realize_wires(cg, &wires, &order, None, 3, Some(&gate));
    let sizing = session.via_sizing();
    ks.iter()
        .zip(&epaths)
        .zip(&all_runs)
        .enumerate()
        .map(|(ki, ((k, p), runs))| {
            let Some(p) = p else {
                return (Vec::new(), Vec::new());
            };
            let mut segs = Vec::new();
            for (ri, r) in runs.iter().enumerate() {
                let mut path = vec![r.start];
                path.extend(polys[wire_of[&(ki, ri)]].iter().copied());
                path.push(r.end);
                for w in path.windows(2) {
                    if d2(w[0], w[1]) < 1e-6 {
                        continue;
                    }
                    segs.push(RoutedTrace {
                        start: Vec2::new(w[0][0], w[0][1]),
                        end: Vec2::new(w[1][0], w[1][1]),
                        width: k.width,
                        layer: cg.layers[r.layer].layer,
                        net: k.net.clone(),
                    });
                }
            }
            let mut vs = Vec::new();
            for st in &p.steps {
                if let EStep::Via { site, span, .. } = *st {
                    let s = &g.sites[site as usize];
                    let sp = &s.spans[span as usize];
                    for &(a, b) in &sp.stack {
                        let mut v = RoutedVia::new(Vec2::new(s.pos[0], s.pos[1]), &k.net, a, b);
                        v.size_with(sizing);
                        vs.push(v);
                    }
                }
            }
            (segs, vs)
        })
        .collect()
}

/// The differential partner of `net` (by the session's pair rules) and the
/// pair gap.
fn twin_of(net: &str, session: &RouteSession, ids: &HashMap<String, u32>) -> Option<(u32, f64)> {
    ids.iter()
        .filter(|(n, _)| n.as_str() != net)
        .find_map(|(n, &id)| session.pair_gap_between(net, n).map(|gap| (id, gap)))
}

/// Vias committed so far: position → (net, spans standing there).
type PlacedVias = HashMap<(i64, i64), (String, Vec<(PcbLayer, PcbLayer)>)>;

fn via_key(p: Vec2) -> (i64, i64) {
    ((p.x * 1e4).round() as i64, (p.y * 1e4).round() as i64)
}

fn spanned(pcb: &Pcb, v: &RoutedVia) -> Vec<PcbLayer> {
    pcb.stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper() && l.spanned_by(v.start_layer, v.end_layer))
        .collect()
}

/// Probe one connection's geometry against the session; commit it when every
/// piece is legal. A via of the same net already standing at the same point
/// is reused (dropped from `vs`), not drilled twice.
fn try_commit(
    pcb: &Pcb,
    session: &mut RouteSession,
    placed_vias: &mut PlacedVias,
    net: &str,
    segs: &[RoutedTrace],
    vs: &mut Vec<RoutedVia>,
    base_len: usize,
) -> Result<Vec<SpanId>, String> {
    let clr = session.clearance_for(net);
    // the same via already standing (same net, same span): reuse it
    vs.retain(|v| {
        !placed_vias
            .get(&via_key(v.position))
            .is_some_and(|(n, spans)| n == net && spans.contains(&(v.start_layer, v.end_layer)))
    });
    // a same-net via at the point on other layers: this one stacks on it
    let stacks_on = |v: &RoutedVia| {
        placed_vias
            .get(&via_key(v.position))
            .is_some_and(|(n, _)| n == net)
    };
    let seg_ok = |s: &RouteSession, t: &RoutedTrace| {
        s.probe(
            &CopperGeom::Segment {
                a: t.start,
                b: t.end,
                half_w: t.width / 2.0,
            },
            t.layer,
            net,
            clr,
        )
        .legal
    };
    let via_ok = |s: &RouteSession, v: &RoutedVia| {
        (stacks_on(v) || !placed_vias.contains_key(&via_key(v.position)))
            && (stacks_on(v)
                || s.probe_via_span(v.position, net, v.start_layer, v.end_layer)
                    .legal)
            && spanned(pcb, v).into_iter().all(|l| {
                s.probe(
                    &CopperGeom::Disc {
                        center: v.position,
                        r: v.diameter / 2.0,
                    },
                    l,
                    net,
                    clr,
                )
                .legal
            })
    };
    let legal = segs.iter().all(|t| seg_ok(session, t)) && vs.iter().all(|v| via_ok(session, v));
    if !legal {
        // cause: the first offending piece
        let mut why = String::from("rejected: via");
        if let Some(v) = vs.iter().find(|v| !via_ok(session, v)) {
            why = if !stacks_on(v) && placed_vias.contains_key(&via_key(v.position)) {
                "rejected: via site held by another net".into()
            } else if !stacks_on(v)
                && !session
                    .probe_via_span(v.position, net, v.start_layer, v.end_layer)
                    .legal
            {
                "rejected: via hole-to-hole".into()
            } else {
                let routed = spanned(pcb, v).into_iter().any(|l| {
                    session
                        .probe(
                            &CopperGeom::Disc {
                                center: v.position,
                                r: v.diameter / 2.0,
                            },
                            l,
                            net,
                            clr,
                        )
                        .blockers
                        .iter()
                        .any(|b| b.span >= base_len)
                });
                format!(
                    "rejected: via barrel vs {} copper",
                    if routed { "routed" } else { "fixed" }
                )
            };
        }
        for t in segs {
            let pr = session.probe(
                &CopperGeom::Segment {
                    a: t.start,
                    b: t.end,
                    half_w: t.width / 2.0,
                },
                t.layer,
                net,
                clr,
            );
            if let Some(b) = pr.blockers.first() {
                why = format!(
                    "rejected: trace vs {} copper",
                    if b.span >= base_len {
                        "routed"
                    } else {
                        "fixed"
                    }
                );
                break;
            }
        }
        if log::log_enabled!(log::Level::Debug) {
            for t in segs {
                let pr = session.probe(
                    &CopperGeom::Segment {
                        a: t.start,
                        b: t.end,
                        half_w: t.width / 2.0,
                    },
                    t.layer,
                    net,
                    clr,
                );
                if let Some(b) = pr.blockers.first() {
                    log::debug!(
                        "flow reject {net} seg {:?}->{:?} {:?}: blocker {:?} span {} at {:.4} mm",
                        t.start,
                        t.end,
                        t.layer,
                        b.net,
                        b.span,
                        b.distance
                    );
                }
            }
            for v in vs.iter().filter(|v| !via_ok(session, v)) {
                log::debug!("flow reject {net} via {:?}", v.position);
            }
        }
        return Err(why);
    }
    let mut ids = Vec::new();
    for t in segs {
        let hw = t.width / 2.0;
        ids.push(session.commit(CopperElement {
            min: [t.start.x.min(t.end.x) - hw, t.start.y.min(t.end.y) - hw],
            max: [t.start.x.max(t.end.x) + hw, t.start.y.max(t.end.y) + hw],
            net: t.net.clone(),
            layer: t.layer,
            geom: CopperGeom::Segment {
                a: t.start,
                b: t.end,
                half_w: hw,
            },
        }));
    }
    for v in vs.iter() {
        let r = v.diameter / 2.0;
        ids.push(session.commit_drill(v.position, v.drill));
        let e = placed_vias
            .entry(via_key(v.position))
            .or_insert_with(|| (net.to_string(), Vec::new()));
        e.1.push((v.start_layer, v.end_layer));
        for l in spanned(pcb, v) {
            ids.push(session.commit(CopperElement {
                min: [v.position.x - r, v.position.y - r],
                max: [v.position.x + r, v.position.y + r],
                net: v.net.clone(),
                layer: l,
                geom: CopperGeom::Disc {
                    center: v.position,
                    r,
                },
            }));
        }
    }
    Ok(ids)
}

/// A committed connection: its copper and its spans in the session.
struct Committed {
    segs: Vec<RoutedTrace>,
    vs: Vec<RoutedVia>,
    ids: Vec<SpanId>,
}

fn seg_cross(p: [f64; 2], q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> bool {
    let (d1, d2, d3, d4) = (
        orient(p, q, a),
        orient(p, q, b),
        orient(a, b, p),
        orient(a, b, q),
    );
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

fn conn_key(k: &Commodity) -> (String, [i64; 4]) {
    let q = |x: f64| (x * 1e4).round() as i64;
    (
        k.net.clone(),
        [
            q(k.src.point[0]),
            q(k.src.point[1]),
            q(k.dst.point[0]),
            q(k.dst.point[1]),
        ],
    )
}

/// The flow-core router (see the module docs). Returns the same result shape
/// as [`crate::router::route_all_with_opts`] plus the market report.
///
/// Runs in rounds: each round triangulates the board *with the copper
/// committed so far*, runs the market on the still-open connections, and
/// commits whatever the oracle accepts. A connection the oracle rejects (its
/// realized geometry crosses or crowds another net's) re-enters the next
/// round, where the committed copper is an obstacle with exact capacities.
pub fn route_flow(
    pcb: &Pcb,
    width: f64,
    nets_filter: &[String],
    opts: &FlowOptions,
) -> (RouteAllResult, FlowReport) {
    let mut rep = FlowReport::default();
    // Nets carried by a plane-role pour reach it through access vias (the
    // exact sub-market in `access`); every other net is routed.
    let plane_nets: BTreeSet<String> = super::access::plane_poured(pcb)
        .keys()
        .map(|n| n.to_string())
        .filter(|n| nets_filter.is_empty() || nets_filter.contains(n))
        .collect();
    rep.plane_nets = plane_nets.len();
    let mut session = RouteSession::from_pcb(pcb);
    let base_len = session.len();
    let mut causes: BTreeMap<(String, [i64; 4]), String> = BTreeMap::new();
    log::debug!("flow: base session spans {}", session.len());
    let mut board = pcb.clone();
    let mut traces: Vec<RoutedTrace> = Vec::new();
    let mut vias: Vec<RoutedVia> = Vec::new();
    let mut placed_vias: PlacedVias = HashMap::new();
    if !plane_nets.is_empty() {
        let sw = crate::router::Stopwatch::start();
        let mk =
            super::access::AccessMarket::build(pcb, opts.params.micro_cost, opts.params.via_cost);
        let (choice, stats) = mk.solve(200, 2.0);
        let sizing = session.via_sizing().clone();
        let mut ok = 0usize;
        for av in mk.vias(&choice) {
            let pad = &mk.pads[av.pad];
            if !plane_nets.contains(&pad.net) {
                continue;
            }
            let pos = Vec2::new(av.pos[0], av.pos[1]);
            let c = Vec2::new(pad.center[0], pad.center[1]);
            let mut segs: Vec<RoutedTrace> = if (pos.x - c.x).abs() + (pos.y - c.y).abs() > 1e-9 {
                vec![RoutedTrace {
                    start: c,
                    end: pos,
                    width: session.width_for(&pad.net, width),
                    layer: pad.layer,
                    net: pad.net.clone(),
                }]
            } else {
                Vec::new()
            };
            if let Some((pl, q)) = av.stub {
                segs.push(RoutedTrace {
                    start: pos,
                    end: Vec2::new(q[0], q[1]),
                    width: session.width_for(&pad.net, width),
                    layer: pl,
                    net: pad.net.clone(),
                });
            }
            let mut v = RoutedVia::new(pos, &pad.net, av.span.0, av.span.1);
            v.size_with(&sizing);
            let mut vs = vec![v];
            if try_commit(
                pcb,
                &mut session,
                &mut placed_vias,
                &pad.net,
                &segs,
                &mut vs,
                base_len,
            )
            .is_ok()
            {
                ok += 1;
                for t in &segs {
                    board.traces.push(vcad_ir::ecad::Trace {
                        start: t.start,
                        end: t.end,
                        width: t.width,
                        layer: t.layer,
                        net: t.net.clone(),
                        source: None,
                    });
                }
                for v in &vs {
                    board.vias.push(v.to_board_via(pcb, None));
                }
                traces.extend(segs);
                vias.extend(vs);
            }
        }
        let last = stats.last();
        rep.access = [
            mk.pads.len(),
            mk.options.iter().filter(|o| o.is_empty()).count(),
            ok,
        ];
        rep.access_gap = last.map(|s| s.gap);
        for (p, o) in mk.pads.iter().zip(&mk.options) {
            if o.is_empty() {
                *rep.access_unsited.entry(p.net.clone()).or_default() += 1;
            }
        }
        log::info!(
            "flow access: {} pads need a plane via, {} without a legal site, {ok} committed; \
             auction {} iterations, gap {:?}, {:.0} ms",
            rep.access[0],
            rep.access[1],
            stats.len(),
            rep.access_gap,
            sw.ms()
        );
    }
    // board copper that never moves: the base + plane access
    let fixed_board = board.clone();
    let mut committed_map: BTreeMap<(String, [i64; 4]), Committed> = BTreeMap::new();
    let mut ripped: BTreeMap<(String, [i64; 4]), usize> = BTreeMap::new();
    let mut first: BTreeSet<(String, [i64; 4])> = BTreeSet::new();
    let mut done: BTreeSet<(String, [i64; 4])> = BTreeSet::new();
    let mut all: BTreeSet<(String, [i64; 4])> = BTreeSet::new();
    let mut net_of_conn: BTreeMap<(String, [i64; 4]), String> = BTreeMap::new();
    let mut params = CutParams::from_pcb(pcb);
    // One graph at the narrowest width any wire may run (a neck), so the
    // neck regions' cuts count their extra wires.
    params.width = pcb
        .rules
        .class_rules
        .iter()
        .chain(std::iter::once(&pcb.rules.default_rules))
        .filter_map(|c| c.neck_width)
        .chain(pcb.rules.rule_areas.iter().map(|a| a.min_trace_width))
        .fold(width, f64::min);
    for round in 0..opts.rounds.max(1) {
        let sw = crate::router::Stopwatch::start();
        let cg0 = CutGraph::build(&board, params, CopperFilter::All);
        let specs = choose_sites(&board, &cg0, &opts.params);
        drop(cg0);
        let pts: Vec<[f64; 2]> = specs.iter().map(|s| s.pos).collect();
        let cg = CutGraph::build_with(&board, params, CopperFilter::All, &pts);
        let mut g = FlowGraph::build(&cg, &specs, &params);
        rep.ms[0] += sw.ms();
        let mut ks: Vec<Commodity> =
            commodities(pcb, &g, &session, width, nets_filter, &plane_nets)
                .into_iter()
                .filter(|k| !done.contains(&conn_key(k)))
                .collect();
        // connections that failed or were ripped up go first
        ks.sort_by_key(|k| !first.contains(&conn_key(k)));
        if round == 0 {
            rep.graph = cg.layers.iter().fold([0; 3], |a, l| {
                [
                    a[0] + l.verts.len(),
                    a[1] + l.tris.len(),
                    a[2] + l.cuts.len(),
                ]
            });
            rep.sites = g.sites.len();
            rep.commodities = ks.len();
            for k in &ks {
                all.insert(conn_key(k));
                net_of_conn.insert(conn_key(k), k.net.clone());
            }
        }
        if ks.is_empty() {
            break;
        }
        let sw = crate::router::Stopwatch::start();
        // Round 0 runs the market to feasibility or a stall; later rounds
        // re-plan a residue on a mostly fixed board with a tighter budget.
        let mut mp = opts.params.clone();
        if round > 0 {
            mp.iters = mp.iters.min(60);
            mp.stall = mp.stall.min(10);
        }
        let (paths, mut stats) = g.negotiate(&ks, &mp);
        rep.ms[1] += sw.ms();
        if round == 0 {
            rep.planned = paths.iter().filter(|p| p.is_some()).count();
            rep.lower_bound = stats
                .iter()
                .map(|s| s.lower_bound)
                .fold(f64::NEG_INFINITY, f64::max);
            rep.upper_bound = stats
                .iter()
                .filter(|s| s.overflow <= 1e-9 && s.site_conflicts == 0)
                .map(|s| s.cost)
                .reduce(f64::min);
            rep.gap = rep
                .upper_bound
                .map(|ub| ((ub - rep.lower_bound) / ub.max(1e-9)).max(0.0));
        }
        for s in stats.iter_mut() {
            s.round = round;
        }
        rep.iters.extend(stats);

        let sw = crate::router::Stopwatch::start();
        let geo = realize(&cg, &g, &ks, &paths, &session, &opts.params);
        let mut committed = 0usize;
        let mut rejected = 0usize;
        let mut unembedded = 0usize;
        let mut round_why: BTreeMap<String, usize> = BTreeMap::new();
        for (ki, (k, (segs, mut vs))) in ks.iter().zip(geo).enumerate() {
            if segs.is_empty() && vs.is_empty() {
                if paths[ki].is_none() {
                    causes.insert(conn_key(k), "no plan (market: no path)".into());
                    log::debug!(
                        "flow round {round}: no plan for {} {:?}->{:?} (src {} nodes, dst {} nodes)",
                        k.net,
                        k.src.point,
                        k.dst.point,
                        k.src.nodes.len(),
                        k.dst.nodes.len()
                    );
                } else {
                    unembedded += 1;
                    causes.insert(conn_key(k), "no planar embedding".into());
                }
                continue;
            }

            let res = try_commit(
                pcb,
                &mut session,
                &mut placed_vias,
                &k.net,
                &segs,
                &mut vs,
                base_len,
            );
            if let Err(why) = &res {
                causes.insert(conn_key(k), why.clone());
                *round_why.entry(why.clone()).or_default() += 1;
            }
            match res {
                Ok(ids) => {
                    done.insert(conn_key(k));
                    committed += 1;
                    committed_map.insert(conn_key(k), Committed { segs, vs, ids });
                }
                Err(_) => rejected += 1,
            }
        }
        rep.ms[2] += sw.ms();
        log::info!(
            "flow round {round}: {} open, {} planned, {committed} committed, {rejected} rejected \
             ({unembedded} without a planar embedding)",
            ks.len(),
            paths.iter().filter(|p| p.is_some()).count(),
        );
        log::info!("flow round {round} rejections: {round_why:?}");
        rep.rounds.push([ks.len(), committed, rejected]);
        if round == 0 {
            rep.rejected = rejected;
        }
        // Rip-up: an open connection the board has walled off is planned on
        // the board without routed copper; committed connections whose wires
        // cross that plan's cuts are ripped up (each at most twice) and both
        // go first next round.
        let open: Vec<&Commodity> = ks.iter().filter(|k| !done.contains(&conn_key(k))).collect();
        for k in &open {
            first.insert(conn_key(k));
        }
        let mut rips = 0usize;
        if round + 1 < opts.rounds.max(1) && !open.is_empty() && !committed_map.is_empty() {
            let free_cg0 = CutGraph::build(&fixed_board, params, CopperFilter::All);
            let fspecs = choose_sites(&fixed_board, &free_cg0, &opts.params);
            drop(free_cg0);
            let fpts: Vec<[f64; 2]> = fspecs.iter().map(|s| s.pos).collect();
            let fcg = CutGraph::build_with(&fixed_board, params, CopperFilter::All, &fpts);
            let fg = FlowGraph::build(&fcg, &fspecs, &params);
            let fks = commodities(pcb, &fg, &session, width, nets_filter, &plane_nets);
            let want: BTreeSet<(String, [i64; 4])> = open.iter().map(|k| conn_key(k)).collect();
            let mut victims: BTreeSet<(String, [i64; 4])> = BTreeSet::new();
            for fk in fks.iter().filter(|k| want.contains(&conn_key(k))) {
                let Some((p, _)) = fg.shortest(fk, 0.0, &opts.params) else {
                    continue;
                };
                for st in &p.steps {
                    let Step::Cross { layer, cut, .. } = *st else {
                        continue;
                    };
                    let lg = &fcg.layers[layer as usize];
                    let c = &lg.cuts[cut as usize];
                    let (a, b) = (lg.vpos(c.a), lg.vpos(c.b));
                    for (key, cm) in &committed_map {
                        if key.0 == fk.net || ripped.get(key).copied().unwrap_or(0) >= 2 {
                            continue;
                        }
                        if cm.segs.iter().any(|t| {
                            t.layer == lg.layer
                                && seg_cross(a, b, [t.start.x, t.start.y], [t.end.x, t.end.y])
                        }) {
                            victims.insert(key.clone());
                        }
                    }
                }
            }
            for key in victims {
                if let Some(cm) = committed_map.remove(&key) {
                    for id in cm.ids {
                        session.remove(id);
                    }
                    for v in &cm.vs {
                        if let Some(e) = placed_vias.get_mut(&via_key(v.position)) {
                            e.1.retain(|s| *s != (v.start_layer, v.end_layer));
                            if e.1.is_empty() {
                                placed_vias.remove(&via_key(v.position));
                            }
                        }
                    }
                    done.remove(&key);
                    *ripped.entry(key.clone()).or_default() += 1;
                    first.insert(key);
                    rips += 1;
                }
            }
            log::info!("flow round {round}: ripped up {rips} connections blocking open ones");
        }
        // the next round's board: fixed copper + everything committed
        board = fixed_board.clone();
        for cm in committed_map.values() {
            for t in &cm.segs {
                board.traces.push(vcad_ir::ecad::Trace {
                    start: t.start,
                    end: t.end,
                    width: t.width,
                    layer: t.layer,
                    net: t.net.clone(),
                    source: None,
                });
            }
            for v in &cm.vs {
                board.vias.push(v.to_board_via(pcb, None));
            }
        }
        if committed == 0 && rips == 0 {
            break;
        }
    }
    for cm in committed_map.into_values() {
        traces.extend(cm.segs);
        vias.extend(cm.vs);
    }
    rep.committed = done.len();
    for key in all.iter().filter(|k| !done.contains(*k)) {
        let c = causes
            .get(key)
            .cloned()
            .unwrap_or_else(|| "no plan (market: no path)".into());
        *rep.causes.entry(c.clone()).or_default() += 1;
        rep.net_causes.entry(key.0.clone()).or_insert(c);
    }

    let sw = crate::router::Stopwatch::start();
    let legal = crate::router::legalize::legalize(pcb, &mut traces, &mut vias, &[], true, false);
    rep.ms[3] = sw.ms();
    let demoted: BTreeSet<String> = legal.demoted.iter().map(|(n, _)| n.clone()).collect();

    let mut net_ok: BTreeMap<String, bool> = BTreeMap::new();
    for key in &all {
        let net = &net_of_conn[key];
        let e = net_ok.entry(net.clone()).or_insert(true);
        *e &= done.contains(key) && !demoted.contains(net);
    }
    let routed: Vec<String> = net_ok
        .iter()
        .filter(|(_, v)| **v)
        .map(|(n, _)| n.clone())
        .collect();
    let unrouted: Vec<String> = net_ok
        .iter()
        .filter(|(_, v)| !**v)
        .map(|(n, _)| n.clone())
        .collect();
    let routability = if all.is_empty() {
        1.0
    } else {
        done.len() as f64 / all.len() as f64
    };
    (
        RouteAllResult {
            traces,
            vias,
            routed_nets: routed,
            unrouted_nets: unrouted,
            diagnostics: Vec::new(),
            routability,
            zones: Vec::new(),
        },
        rep,
    )
}

/// `VCAD_FLOW=1` selects the flow router in callers that support both.
pub fn flow_enabled() -> bool {
    std::env::var("VCAD_FLOW").as_deref() == Ok("1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drc::{check_drc, DrcRuleType};

    /// A 2-layer board with three 2-pin nets, two of which must cross.
    fn crossing_board() -> Pcb {
        let mut pcb = crate::router::flow::test_board(20.0, 20.0, &["FCu", "BCu"]);
        let pad = |num: &str, x: f64, y: f64, net: &str| {
            serde_json::json!({"number": num, "padType": "SMD",
                "shape": {"type": "Rect", "width": 1.0, "height": 1.0},
                "position": {"x": x, "y": y}, "rotation": 0.0,
                "net": net, "layers": ["FCu"]})
        };
        let fp: vcad_ir::ecad::Footprint = serde_json::from_value(serde_json::json!({
            "ref": "U1", "value": "", "footprintName": "T", "position": {"x": 0.0, "y": 0.0},
            "rotation": 0.0, "front": true, "graphics": [],
            "pads": [pad("1", 2.0, 10.0, "A"), pad("2", 18.0, 10.0, "A"),
                     pad("3", 10.0, 2.0, "B"), pad("4", 10.0, 18.0, "B"),
                     pad("5", 2.0, 4.0, "C"), pad("6", 18.0, 4.0, "C")]
        }))
        .expect("footprint json");
        pcb.footprints.push(fp);
        pcb
    }

    #[test]
    fn routes_crossing_nets_drc_clean() {
        let pcb = crossing_board();
        let (r, rep) = route_flow(&pcb, 0.2, &[], &FlowOptions::default());
        assert_eq!(rep.commodities, 3);
        assert_eq!(r.unrouted_nets, Vec::<String>::new(), "report {rep:?}");
        assert!(!r.vias.is_empty(), "the crossing nets need a layer change");
        assert!(rep.causes.is_empty(), "nothing left open: {:?}", rep.causes);
        // The market reached a capacity-feasible plan and certified it.
        assert!(
            rep.gap.is_some_and(|g| (0.0..0.5).contains(&g)),
            "gap {:?}",
            rep.gap
        );
        let mut out = pcb.clone();
        for t in &r.traces {
            out.traces.push(vcad_ir::ecad::Trace {
                start: t.start,
                end: t.end,
                width: t.width,
                layer: t.layer,
                net: t.net.clone(),
                source: None,
            });
        }
        for v in &r.vias {
            out.vias.push(v.to_board_via(&pcb, None));
        }
        let bad: Vec<_> = check_drc(&out)
            .into_iter()
            .filter(|v| {
                matches!(
                    v.rule,
                    DrcRuleType::Clearance
                        | DrcRuleType::Short
                        | DrcRuleType::UnconnectedNet
                        | DrcRuleType::HoleToHole
                        | DrcRuleType::EdgeClearance
                )
            })
            .collect();
        assert!(bad.is_empty(), "{bad:?}");
        // Deterministic.
        let (r2, _) = route_flow(&pcb, 0.2, &[], &FlowOptions::default());
        assert_eq!(r.traces.len(), r2.traces.len());
        assert_eq!(r.vias.len(), r2.vias.len());
    }
}
