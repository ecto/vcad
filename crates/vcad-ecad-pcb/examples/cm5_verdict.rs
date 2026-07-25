//! Verdict driver: put the still-unrouted connections of a routed board in
//! front of the COMPLETE window router, cluster by cluster, and demand an
//! answer — Routed (commit-quality paths) or ProvedInfeasible (bottleneck-cut
//! certificate). The campaign's closing argument: every connection ends
//! accounted for, and nothing ends "unknown".
//!
//! The ladder, per cluster, escalates rather than accepting an unknown:
//!
//! 1. **joint window, base budget** — the k connections decided together;
//! 2. **joint window, 5× then 25× budget**, cells-per-axis raised with the last
//!    rung so a wider search keeps its pitch at the `width + separation` floor
//!    instead of coarsening until unrelated terminals collide in one cell;
//! 3. **per-connection endgame** — whatever is left is decided one connection at
//!    a time in its own window: 2 mm, then 8 mm, then 20 mm of margin, each with
//!    a cell budget that holds the pitch at that floor. A lone connection is
//!    settled by reachability, which is exact and cannot trip a budget: it
//!    routes, or its severed reachable component *is* the proof.
//!
//! Splitting is not a fallback for weak proofs, it is what makes the
//! certificates *per connection*: a joint infeasibility says only that the k
//! cannot all be routed at once, so it is never charged to the individual
//! connections — they each get their own verdict. And because each rung searches
//! a strictly larger space than the last, the certificate that survives to the
//! end is the strongest one available.
//!
//! Everything commits fail-closed through the session oracle — traces, via
//! barrels on every layer they span, drills against the hole-to-hole rule, and
//! diff-pair coupling scored the way the board's DRC scores it.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example cm5_verdict -- routed.pcb.json [budget] [out.pcb.json] [max_cluster]
//! ```
//!
//! `budget` is DFS expansions; a value below 1000 is read as a multiplier of
//! 1e6 expansions (so `3` and `3000000` mean the same thing).

use std::collections::{BTreeMap, HashMap};
use vcad_ecad_pcb::ratsnest::{compute_ratsnest, NetConnection, Netlist, NetlistNet};
use vcad_ecad_pcb::router::complete::{
    route_window_complete_pinned, CompleteOutcome, TerminalLayers, ViaClass, WindowBudget,
};
use vcad_ecad_pcb::session::RouteSession;
use vcad_ecad_pcb::spatial::{CopperElement, CopperGeom};
use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

/// A connection awaiting a verdict.
#[derive(Clone)]
struct Conn {
    net: String,
    from: Vec2,
    to: Vec2,
}

impl Conn {
    fn tuple(&self) -> (String, Vec2, Vec2) {
        (self.net.clone(), self.from, self.to)
    }
}

/// Cluster = merged window plus the connections decided in it.
struct Cluster {
    lo: Vec2,
    hi: Vec2,
    conns: Vec<Conn>,
}

/// Joint rungs: `(budget multiplier, cells-per-axis cap)`. The cap rises with
/// the budget — raising the window without raising the cell count would only
/// coarsen the pitch until unrelated terminals collide.
const JOINT_RUNGS: [(usize, usize); 3] = [(1, 48), (5, 48), (25, 96)];

/// Per-connection rungs: `(window margin in mm, cells-per-axis cap)`. Each rung
/// searches a strictly larger space than the last, so escalating can only turn
/// an infeasibility into a routing — and the certificate that survives to the
/// end is the strongest one (the widest space exhausted).
const SINGLE_RUNGS: [(f64, usize); 3] = [(2.0, 160), (8.0, 320), (20.0, 640)];

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: cm5_verdict <routed.pcb.json> [budget] [out.pcb.json] [max_cluster]");
    let budget: usize = args
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|b| if b < 1000.0 { b * 1e6 } else { b } as usize)
        .unwrap_or(5_000_000);
    let out_json = args.next();
    let max_cluster: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(6);
    let pcb: Pcb =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");

    // Unrouted connections = ratsnest over the routed board.
    let mut map: BTreeMap<String, Vec<NetConnection>> = BTreeMap::new();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if let Some(net) = &pad.net {
                if !net.is_empty() {
                    map.entry(net.clone()).or_default().push(NetConnection {
                        component_ref: fp.reference.clone(),
                        pin_number: pad.number.clone(),
                    });
                }
            }
        }
    }
    let netlist = Netlist {
        nets: map
            .into_iter()
            .map(|(name, connections)| NetlistNet { name, connections })
            .collect(),
    };
    let mut rats = compute_ratsnest(&pcb, &netlist);
    // Nets that own a filled zone are connected THROUGH the plane, not by
    // pad-to-pad traces — the router intentionally stitches them with vias.
    // Their air-wires are not unrouted work and must not enter the verdict.
    let plane_nets: std::collections::BTreeSet<&str> = pcb
        .zones
        .iter()
        .filter(|z| !z.net.is_empty())
        .map(|z| z.net.as_str())
        .collect();
    rats.retain(|l| !plane_nets.contains(l.net.as_str()));
    println!("unrouted connections (plane nets excluded): {}", rats.len());

    let clusters = cluster(&rats, max_cluster);
    println!("clusters: {}", clusters.len());

    let mut driver = Driver::new(pcb, budget);
    for c in &clusters {
        driver.resolve_cluster(c);
    }
    println!(
        "\n== VERDICT: routed {} / proved-infeasible {} / unknown {} ==",
        driver.routed, driver.proved, driver.unknown
    );
    if let Some(out) = out_json {
        std::fs::write(&out, serde_json::to_string(&driver.pcb).expect("serialize"))
            .expect("write");
        eprintln!("wrote {out}");
    }
}

/// Cluster connections whose bboxes (inflated 2 mm) overlap. Two rules keep the
/// joint instances tractable: a cluster never holds two connections of the same
/// net (per-connection node-disjointness can't model same-net cell sharing), and
/// the merged window is capped, because the cost of a joint instance grows with
/// its window while the *benefit* of merging does not. The cap is not what keeps
/// the pitch honest any more — the per-rung cell budget does that, so a window
/// stays at its `width + separation` floor however large it gets.
fn cluster(rats: &[vcad_ecad_pcb::ratsnest::RatsnestLine], max_cluster: usize) -> Vec<Cluster> {
    const MAX_WINDOW_MM: f64 = 20.0;
    let mut clusters: Vec<Cluster> = Vec::new();
    'c: for l in rats {
        let (lo, hi) = (
            Vec2::new(l.from.x.min(l.to.x) - 2.0, l.from.y.min(l.to.y) - 2.0),
            Vec2::new(l.from.x.max(l.to.x) + 2.0, l.from.y.max(l.to.y) + 2.0),
        );
        let conn = Conn {
            net: l.net.clone(),
            from: l.from,
            to: l.to,
        };
        for c in clusters.iter_mut() {
            let merged_w = (c.hi.x.max(hi.x) - c.lo.x.min(lo.x)).abs();
            let merged_h = (c.hi.y.max(hi.y) - c.lo.y.min(lo.y)).abs();
            if lo.x <= c.hi.x
                && c.lo.x <= hi.x
                && lo.y <= c.hi.y
                && c.lo.y <= hi.y
                && c.conns.len() < max_cluster
                && merged_w <= MAX_WINDOW_MM
                && merged_h <= MAX_WINDOW_MM
                && c.conns.iter().all(|k| k.net != conn.net)
            {
                c.lo.x = c.lo.x.min(lo.x);
                c.lo.y = c.lo.y.min(lo.y);
                c.hi.x = c.hi.x.max(hi.x);
                c.hi.y = c.hi.y.max(hi.y);
                c.conns.push(conn);
                continue 'c;
            }
        }
        clusters.push(Cluster {
            lo,
            hi,
            conns: vec![conn],
        });
    }
    clusters
}

/// Distance from `p` to the segment `a`–`b` (centreline, mm).
fn point_on_segment(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 < 1e-18 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    };
    ((p.x - a.x - t * dx).powi(2) + (p.y - a.y - t * dy).powi(2)).sqrt()
}

/// Board state plus the running verdict tally.
struct Driver {
    pcb: Pcb,
    session: RouteSession,
    layers: Vec<PcbLayer>,
    /// Per-net class trace width: SI-class nets must be searched and committed
    /// at their class width (the DRC's MinTraceWidth rule is per-net), so a
    /// joint path at default width would land hundreds of width violations on a
    /// board whose pairs are classed at 0.2 mm.
    net_width: HashMap<String, f64>,
    default_width: f64,
    budget: usize,
    routed: usize,
    proved: usize,
    unknown: usize,
}

impl Driver {
    fn new(pcb: Pcb, budget: usize) -> Self {
        let mut session = RouteSession::from_pcb(&pcb);
        // Judge diff-pair coupling the way the board's DRC will, pads and via
        // annuli included. Costs a few routings; buys copper that cannot land a
        // pair-gap violation.
        session.set_strict_pair_coupling(true);
        let layers: Vec<_> = pcb
            .stackup
            .layers
            .iter()
            .map(|l| l.layer)
            .filter(|l| l.is_copper())
            .collect();
        let net_width: HashMap<String, f64> = pcb
            .rules
            .class_rules
            .iter()
            .flat_map(|rule| {
                pcb.rules
                    .net_class_assignments
                    .get(&rule.name)
                    .into_iter()
                    .flatten()
                    .map(move |net| (net.clone(), rule.trace_width))
            })
            .collect();
        let default_width = pcb.rules.default_rules.trace_width;
        Self {
            pcb,
            session,
            layers,
            net_width,
            default_width,
            budget,
            routed: 0,
            proved: 0,
            unknown: 0,
        }
    }

    /// The copper layers each terminal of `conn` may attach on: the layers where
    /// the net *already has copper* at that endpoint.
    ///
    /// A ratsnest line joins two copper groups, so an endpoint is a pad on some
    /// layers, or a point on an existing trace or via of the net — and only
    /// those layers are electrically live there. A stub that starts anywhere
    /// else is dangling: nothing joins it to the group, and the board gains a
    /// net island instead of a connection.
    fn terminal_layers(&self, conn: &Conn) -> TerminalLayers {
        let at = |p: Vec2| -> Vec<PcbLayer> {
            let mut out: Vec<PcbLayer> = Vec::new();
            for fp in &self.pcb.footprints {
                for pad in &fp.pads {
                    if pad.net.as_deref() != Some(conn.net.as_str()) {
                        continue;
                    }
                    let w = vcad_ecad_pcb::geometry::pad_world_position(fp, pad);
                    if (w.x - p.x).abs() < 1e-3 && (w.y - p.y).abs() < 1e-3 {
                        out.extend(pad.layers.iter().filter(|l| self.layers.contains(l)));
                    }
                }
            }
            for trace in &self.pcb.traces {
                if trace.net != conn.net {
                    continue;
                }
                if point_on_segment(p, trace.start, trace.end) <= trace.width / 2.0 + 1e-3 {
                    out.push(trace.layer);
                }
            }
            for via in &self.pcb.vias {
                if via.net != conn.net {
                    continue;
                }
                let d = ((via.position.x - p.x).powi(2) + (via.position.y - p.y).powi(2)).sqrt();
                if d <= via.diameter / 2.0 + 1e-3 {
                    out.extend(self.span(via.start_layer, via.end_layer));
                }
            }
            out.retain(|l| l.is_copper());
            out.sort_by_key(|l| {
                self.layers
                    .iter()
                    .position(|s| s == l)
                    .unwrap_or(usize::MAX)
            });
            out.dedup();
            out
        };
        TerminalLayers {
            from: at(conn.from),
            to: at(conn.to),
        }
    }

    /// The via geometry this driver commits — handed to the router so its
    /// legality model is the same one the commit will enforce.
    fn via_class(&self) -> ViaClass {
        ViaClass {
            pad_diameter: self.pcb.rules.default_rules.via_diameter,
            drill: self.pcb.rules.default_rules.via_drill,
        }
    }

    fn width_of(&self, net: &str) -> f64 {
        self.net_width
            .get(net)
            .copied()
            .unwrap_or(self.default_width)
    }

    /// Search width for a set of connections: the widest class width among them
    /// (conservative — the corridors found then fit every member's committed
    /// width).
    fn search_width(&self, conns: &[Conn]) -> f64 {
        conns
            .iter()
            .map(|c| self.width_of(&c.net))
            .fold(self.default_width, f64::max)
    }

    fn resolve_cluster(&mut self, cluster: &Cluster) {
        let names: Vec<&str> = cluster.conns.iter().map(|c| c.net.as_str()).collect();
        let mut pending = cluster.conns.clone();
        // Joint rungs only earn their keep when there is something joint to
        // decide; a lone connection goes straight to the exact endgame.
        if pending.len() > 1 {
            let width = self.search_width(&pending);
            for (mul, cells) in JOINT_RUNGS {
                let limits =
                    WindowBudget::new(self.budget.saturating_mul(mul)).with_max_axis_cells(cells);
                let conns: Vec<(String, Vec2, Vec2)> = pending.iter().map(|c| c.tuple()).collect();
                let pins: Vec<TerminalLayers> =
                    pending.iter().map(|c| self.terminal_layers(c)).collect();
                match route_window_complete_pinned(
                    &self.session,
                    (cluster.lo, cluster.hi),
                    &self.layers,
                    &conns,
                    &pins,
                    width,
                    Some(self.via_class()),
                    limits,
                ) {
                    CompleteOutcome::Routed(paths) => {
                        // Probe-then-commit PER PATH, in order: cluster paths
                        // are node-disjoint on the window grid, but the pitch
                        // can hide sub-clearance gaps BETWEEN two paths of the
                        // same cluster. Probing each path against the session
                        // AFTER its clustermates committed makes mutual
                        // legality exact; a path the oracle rejects drops to
                        // the per-connection endgame rather than to an unknown.
                        let mut rejected = Vec::new();
                        let mut committed = 0usize;
                        for (conn, path) in pending.iter().zip(&paths) {
                            if self.commit_if_legal(conn, path) {
                                committed += 1;
                                self.routed += 1;
                            } else {
                                rejected.push(conn.clone());
                            }
                        }
                        if committed > 0 {
                            println!(
                                "ROUTED   {names:?} ({committed}/{} committed jointly at \
                                 {mul}x budget)",
                                pending.len()
                            );
                        }
                        pending = rejected;
                        break;
                    }
                    CompleteOutcome::ProvedInfeasible { reason } => {
                        // A joint proof is NOT a per-connection proof: it says
                        // the k cannot all be routed at once. Charge it to
                        // nobody and let each connection earn its own verdict.
                        println!("JOINT-NO {names:?}: {reason}");
                        break;
                    }
                    CompleteOutcome::BudgetExhausted => continue,
                }
            }
        }
        for conn in std::mem::take(&mut pending) {
            self.resolve_single(&conn);
        }
    }

    /// Decide one connection on its own. Reachability is exact for k = 1, so
    /// every rung answers definitively; escalation exists to make a *routing*
    /// more likely (wider window, finer pitch) and, failing that, to make the
    /// surviving certificate the strongest one available.
    fn resolve_single(&mut self, conn: &Conn) {
        let width = self.width_of(&conn.net);
        let mut last: Option<(bool, String)> = None;
        for (margin, cells) in SINGLE_RUNGS {
            let window = (
                Vec2::new(
                    conn.from.x.min(conn.to.x) - margin,
                    conn.from.y.min(conn.to.y) - margin,
                ),
                Vec2::new(
                    conn.from.x.max(conn.to.x) + margin,
                    conn.from.y.max(conn.to.y) + margin,
                ),
            );
            match route_window_complete_pinned(
                &self.session,
                window,
                &self.layers,
                &[conn.tuple()],
                &[self.terminal_layers(conn)],
                width,
                Some(self.via_class()),
                WindowBudget::new(self.budget).with_max_axis_cells(cells),
            ) {
                CompleteOutcome::Routed(paths) => {
                    if self.commit_if_legal(conn, &paths[0]) {
                        self.routed += 1;
                        println!("ROUTED   [{:?}] (alone, {margin:.0}mm window)", conn.net);
                        return;
                    }
                    last = Some((
                        false,
                        format!(
                            "window path rejected by the fail-closed session oracle \
                             ({margin:.0}mm window, {cells} cells/axis)"
                        ),
                    ));
                }
                CompleteOutcome::ProvedInfeasible { reason } => last = Some((true, reason)),
                // k = 1 is decided by reachability, which cannot trip the
                // expansion budget — but keep the honest branch anyway.
                CompleteOutcome::BudgetExhausted => {
                    last = Some((
                        false,
                        format!("budget {} exhausted ({margin:.0}mm window)", self.budget),
                    ))
                }
            }
        }
        match last {
            Some((true, reason)) => {
                self.proved += 1;
                println!("PROVED   [{:?}]: {reason}", conn.net);
            }
            Some((false, why)) => {
                self.unknown += 1;
                println!("UNKNOWN  [{:?}] ({why})", conn.net);
            }
            None => {
                self.unknown += 1;
                println!("UNKNOWN  [{:?}] (no rung reported)", conn.net);
            }
        }
    }

    /// Every copper layer a via spanning `a`..`b` passes through, in stack
    /// order. A barrel's annulus lands on all of them, so all of them must
    /// clear.
    fn span(&self, a: PcbLayer, b: PcbLayer) -> Vec<PcbLayer> {
        let idx = |l: PcbLayer| self.layers.iter().position(|s| *s == l).unwrap_or(0);
        let (i, j) = (idx(a), idx(b));
        self.layers[i.min(j)..=i.max(j)].to_vec()
    }

    /// Probe a candidate path — segments *and* the vias as they will be
    /// committed — against the session, and commit it only if every piece is
    /// legal. Returns whether the path landed on the board.
    fn commit_if_legal(&mut self, conn: &Conn, path: &[(Vec2, Vec2, PcbLayer)]) -> bool {
        let net = &conn.net;
        let w = self.width_of(net);
        let clr = self.session.clearance_for(net);
        let via_d = self.pcb.rules.default_rules.via_diameter;
        let via_r = via_d / 2.0;
        let segs_ok = path.iter().all(|&(a, b, layer)| {
            self.session
                .probe(
                    &CopperGeom::Segment {
                        a,
                        b,
                        half_w: w / 2.0,
                    },
                    layer,
                    net,
                    clr,
                )
                .legal
        });
        if !segs_ok {
            return false;
        }
        // Layer transitions become real vias, so they must clear as real vias:
        // probing only the traces let via copper land unchecked. Consecutive
        // transitions at one point are ONE barrel spanning the whole run —
        // emitting them as separate vias would stack drills at zero spacing and
        // fail hole-to-hole against itself.
        let mut vias: Vec<(Vec2, PcbLayer, PcbLayer)> = Vec::new();
        for w in path.windows(2) {
            let (_, b0, l0) = w[0];
            let (a1, _, l1) = w[1];
            if l0 == l1 || (b0.x - a1.x).abs() > 1e-9 || (b0.y - a1.y).abs() > 1e-9 {
                continue;
            }
            match vias.last_mut() {
                Some((p, _, end))
                    if (p.x - b0.x).abs() < 1e-9 && (p.y - b0.y).abs() < 1e-9 && *end == l0 =>
                {
                    *end = l1;
                }
                _ => vias.push((b0, l0, l1)),
            }
        }
        let via_drill = self.pcb.rules.default_rules.via_drill;
        let vias_ok = vias.iter().all(|&(p, l0, l1)| {
            // The barrel exists on every layer it spans, not just its two ends.
            self.span(l0, l1).iter().all(|&layer| {
                self.session
                    .probe(&CopperGeom::Disc { center: p, r: via_r }, layer, net, clr)
                    .legal
            })
            // Hole-to-hole is layer-agnostic, so the copper probe above cannot
            // see it: two vias on disjoint layer spans share no layer yet still
            // collide in the drill file. The probe compares net-agnostically
            // (only a coincident same-net barrel is exempt), matching the DRC.
            && self.session.probe_hole(p, via_drill, net).legal
            // ...and two vias of THIS path must clear each other, which the
            // session cannot judge until they are committed.
            && vias.iter().all(|&(q, _, _)| {
                let d = ((q.x - p.x).powi(2) + (q.y - p.y).powi(2)).sqrt() - via_drill;
                (q.x - p.x).abs() + (q.y - p.y).abs() < 1e-9
                    || d >= self.pcb.rules.hole_to_hole - 1e-6
            })
        });
        if !vias_ok {
            return false;
        }
        for &(a, b, layer) in path {
            self.session.commit(CopperElement {
                min: [a.x.min(b.x) - w, a.y.min(b.y) - w],
                max: [a.x.max(b.x) + w, a.y.max(b.y) + w],
                net: net.clone(),
                layer,
                geom: CopperGeom::Segment {
                    a,
                    b,
                    half_w: w / 2.0,
                },
            });
            self.pcb.traces.push(vcad_ir::ecad::Trace {
                start: a,
                end: b,
                width: w,
                layer,
                net: net.clone(),
                source: None,
            });
        }
        for (p, l0, l1) in vias {
            for layer in self.span(l0, l1) {
                self.session.commit(CopperElement {
                    min: [p.x - via_r, p.y - via_r],
                    max: [p.x + via_r, p.y + via_r],
                    net: net.clone(),
                    layer,
                    geom: CopperGeom::Disc {
                        center: p,
                        r: via_r,
                    },
                });
            }
            self.session.commit_hole(p, via_drill, net);
            self.pcb.vias.push(vcad_ir::ecad::Via {
                position: p,
                diameter: via_d,
                drill: via_drill,
                start_layer: l0,
                end_layer: l1,
                net: net.clone(),
                source: None,
            });
        }
        true
    }
}
