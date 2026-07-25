//! The verdict ladder: put every still-unrouted connection in front of the
//! COMPLETE window router and demand an answer — `Routed` (commit-quality
//! paths that survive an oracle probe), `ProvedInfeasible` (a bottleneck-cut
//! certificate), or `BudgetExhausted` (an honest unknown).
//!
//! Lifted out of `vcad-ecad-pcb/examples/cm5_verdict.rs` so the fab-prep
//! pipeline and the example driver run the same code rather than two copies
//! that drift.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use vcad_ecad_pcb::ratsnest::{compute_ratsnest, NetConnection, Netlist, NetlistNet};
use vcad_ecad_pcb::router::classes::via_geom_for;
use vcad_ecad_pcb::router::complete::{
    path_vias, route_window_complete_pinned, CompleteOutcome, ViaClass, WindowBudget,
};
use vcad_ecad_pcb::session::RouteSession;
use vcad_ecad_pcb::spatial::{CopperElement, CopperGeom};
use vcad_ir::ecad::{Pcb, PcbLayer, Trace, Via};
use vcad_ir::Vec2;

/// Search knobs for one pass of the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VerdictOptions {
    /// Maximum DFS node expansions per cluster before the router returns an
    /// honest unknown.
    pub budget: usize,
    /// Maximum connections coalesced into one joint search window.
    pub max_cluster: usize,
}

impl Default for VerdictOptions {
    fn default() -> Self {
        Self {
            budget: 5_000_000,
            max_cluster: 6,
        }
    }
}

/// What one pass of the ladder concluded.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerdictSummary {
    /// Unrouted connections the pass was asked about (plane nets excluded).
    pub connections: usize,
    /// Joint search windows the connections were clustered into.
    pub clusters: usize,
    /// Connections routed and committed to the board.
    pub routed: usize,
    /// Connections carrying a proved-infeasible certificate.
    pub proved_infeasible: usize,
    /// Connections that ended as honest unknowns (budget exhausted, or a path
    /// the session oracle rejected).
    pub unknown: usize,
    /// Infeasibility certificates, verbatim — capped so a large board's
    /// receipt stays readable.
    pub certificates: Vec<String>,
}

/// How many certificates a summary carries before it stops collecting.
const MAX_CERTIFICATES: usize = 64;

type Cluster = (Vec2, Vec2, Vec<(String, Vec2, Vec2)>);

/// Merged windows wider than this coarsen the router's grid pitch until
/// unrelated terminals artificially collide.
const MAX_WINDOW_MM: f64 = 20.0;

/// Route every still-unrouted connection on `pcb`, committing only copper that
/// survives the session oracle. Mutates `pcb` in place.
pub fn route_remaining(pcb: &mut Pcb, opts: VerdictOptions) -> VerdictSummary {
    // Unrouted connections = ratsnest over the board as it stands.
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
    let mut rats = compute_ratsnest(pcb, &netlist);
    // Nets that own a filled zone are connected THROUGH the plane, not by
    // pad-to-pad traces — the router intentionally stitches them with vias.
    // Their air-wires are not unrouted work and must not enter the verdict.
    let plane_nets: BTreeSet<&str> = pcb
        .zones
        .iter()
        .filter(|z| !z.net.is_empty())
        .map(|z| z.net.as_str())
        .collect();
    rats.retain(|l| !plane_nets.contains(l.net.as_str()));

    let mut summary = VerdictSummary {
        connections: rats.len(),
        ..Default::default()
    };
    if rats.is_empty() {
        return summary;
    }

    let mut session = RouteSession::from_pcb(pcb);
    let layers: Vec<_> = pcb
        .stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper())
        .collect();
    let width = pcb.rules.default_rules.trace_width;
    // Per-net class widths: an SI-class net must be routed AND committed at its
    // class width, because the DRC's MinTraceWidth rule is per-net. A joint
    // path at the default width lands hundreds of width violations on a board
    // whose pairs are classed wider.
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

    // Cluster connections whose bboxes (inflated 2mm) overlap. Two rules keep
    // the certificates honest: a cluster never holds two connections of the
    // same net (per-connection node-disjointness can't model same-net cell
    // sharing), and the merged window is capped.
    let mut clusters: Vec<Cluster> = Vec::new();
    'c: for l in &rats {
        let (lo, hi) = (
            Vec2::new(l.from.x.min(l.to.x) - 2.0, l.from.y.min(l.to.y) - 2.0),
            Vec2::new(l.from.x.max(l.to.x) + 2.0, l.from.y.max(l.to.y) + 2.0),
        );
        for (clo, chi, cc) in clusters.iter_mut() {
            let merged_w = (chi.x.max(hi.x) - clo.x.min(lo.x)).abs();
            let merged_h = (chi.y.max(hi.y) - clo.y.min(lo.y)).abs();
            if lo.x <= chi.x
                && clo.x <= hi.x
                && lo.y <= chi.y
                && clo.y <= hi.y
                && cc.len() < opts.max_cluster
                && merged_w <= MAX_WINDOW_MM
                && merged_h <= MAX_WINDOW_MM
                && cc.iter().all(|(n, _, _)| n != &l.net)
            {
                clo.x = clo.x.min(lo.x);
                clo.y = clo.y.min(lo.y);
                chi.x = chi.x.max(hi.x);
                chi.y = chi.y.max(hi.y);
                cc.push((l.net.clone(), l.from, l.to));
                continue 'c;
            }
        }
        clusters.push((lo, hi, vec![(l.net.clone(), l.from, l.to)]));
    }
    summary.clusters = clusters.len();

    for (lo, hi, conns) in &clusters {
        // Search at the widest class width in the cluster (conservative:
        // guarantees the found corridors fit every member's committed width).
        let cluster_width = conns
            .iter()
            .map(|(n, _, _)| net_width.get(n).copied().unwrap_or(width))
            .fold(width, f64::max);
        // Hand the router the via geometry the commit below actually writes, so
        // its grid pitch carries the hole-to-hole floor and its barrels are
        // drill-probed in-search. Without it the search spaces vias by copper
        // clearance alone and proposes paths the fail-closed commit has to
        // throw away as unknowns.
        let via_class = conns.iter().map(|(n, _, _)| via_geom_for(pcb, n)).fold(
            (
                pcb.rules.default_rules.via_diameter,
                pcb.rules.default_rules.via_drill,
            ),
            |(ad, adr), (d, dr)| (ad.max(d), adr.max(dr)),
        );
        match route_window_complete_pinned(
            &session,
            (*lo, *hi),
            &layers,
            conns,
            &[],
            cluster_width,
            Some(ViaClass {
                pad_diameter: via_class.0,
                drill: via_class.1,
            }),
            WindowBudget::new(opts.budget),
        ) {
            CompleteOutcome::Routed(paths) => {
                // Probe-then-commit PER PATH, in order. Cluster paths are
                // node-disjoint on the coarse window grid, but the grid pitch
                // can hide sub-clearance gaps BETWEEN two paths of the same
                // cluster. Probing each path against the session AFTER its
                // clustermates committed makes mutual legality exact; a path
                // the oracle rejects downgrades only itself to an unknown.
                for ((net, _, _), path) in conns.iter().zip(&paths) {
                    let w = net_width.get(net).copied().unwrap_or(width);
                    let legal = path.iter().all(|&(a, b, l)| {
                        let g = CopperGeom::Segment {
                            a,
                            b,
                            half_w: w / 2.0,
                        };
                        session.probe(&g, l, net, session.clearance_for(net)).legal
                    }) && vias_legal(pcb, &session, net, path);
                    if !legal {
                        summary.unknown += 1;
                        continue;
                    }
                    summary.routed += 1;
                    commit_path(pcb, &mut session, net, w, path);
                }
                // A cluster the router answered for fewer paths than it holds
                // (the zip is short) leaves the remainder unaccounted; charge
                // them as unknowns rather than silently dropping them.
                summary.unknown += conns.len().saturating_sub(paths.len());
            }
            CompleteOutcome::ProvedInfeasible { reason } => {
                summary.proved_infeasible += conns.len();
                if summary.certificates.len() < MAX_CERTIFICATES {
                    let names: Vec<&str> = conns.iter().map(|c| c.0.as_str()).collect();
                    summary
                        .certificates
                        .push(format!("{}: {reason}", names.join(", ")));
                }
            }
            CompleteOutcome::BudgetExhausted => summary.unknown += conns.len(),
        }
    }
    summary
}

/// The copper layers a barrel from `start` to `end` occupies, in stackup
/// order — endpoints *and* every interior layer.
///
/// A barrel is a hole through all of them: probing or committing only the two
/// endpoint layers leaves the interior invisible to the oracle, and a later
/// path is then routed straight through the barrel on an inner layer (measured
/// on the CM5: two 0.000mm shorts, both a FCu..In2Cu barrel against an In1Cu
/// trace).
fn spanned_layers(pcb: &Pcb, start: PcbLayer, end: PcbLayer) -> Vec<PcbLayer> {
    let mut out: Vec<PcbLayer> = pcb
        .stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper() && l.spanned_by(start, end))
        .collect();
    for endpoint in [start, end] {
        if !out.contains(&endpoint) {
            out.push(endpoint);
        }
    }
    out
}

/// Fail-closed via legality for a candidate path, against the session as it
/// stands (clustermates already committed).
///
/// Two rules the segment probe structurally cannot answer, and which
/// `commit_path` used to commit blind:
///
/// * the barrel's annulus must clear foreign copper on **every** layer it
///   spans, not just its endpoints;
/// * the *drill* must keep `hole_to_hole` from every other hole on the board —
///   layer- and net-agnostic, so two vias whose copper never meets still
///   collide in the drill file. The window router spaces vias inside ONE
///   window by a drill-aware grid pitch; nothing spaced them against the vias
///   of earlier windows, earlier rounds, or the arriving board (measured on the
///   CM5: four hole-to-hole violations, every one of them copper-legal and
///   drill-illegal — the 0.29..0.37mm window between the two rules).
fn vias_legal(
    pcb: &Pcb,
    session: &RouteSession,
    net: &str,
    path: &[(Vec2, Vec2, PcbLayer)],
) -> bool {
    let (via_d, via_drill) = via_geom_for(pcb, net);
    let clearance = session.clearance_for(net);
    let vias = path_vias(path);
    vias.iter().all(|&(p, l0, l1)| {
        if !session.probe_via_drill(p, net).legal {
            return false;
        }
        // The path's own barrels must clear each other too — the session
        // cannot judge them until they are committed.
        let self_clear = vias.iter().all(|&(q, _, _)| {
            let (dx, dy) = (q.x - p.x, q.y - p.y);
            dx.abs() + dy.abs() < 1e-9
                || (dx * dx + dy * dy).sqrt() - via_drill >= pcb.rules.hole_to_hole - 1e-6
        });
        if !self_clear {
            return false;
        }
        let disc = CopperGeom::Disc {
            center: p,
            r: via_d / 2.0,
        };
        spanned_layers(pcb, l0, l1)
            .into_iter()
            .all(|l| session.probe(&disc, l, net, clearance).legal)
    })
}

/// Commit one accepted path to both the legality oracle and the board.
fn commit_path(
    pcb: &mut Pcb,
    session: &mut RouteSession,
    net: &str,
    w: f64,
    path: &[(Vec2, Vec2, vcad_ir::ecad::PcbLayer)],
) {
    for (a, b, layer) in path {
        session.commit(CopperElement {
            min: [a.x.min(b.x) - w, a.y.min(b.y) - w],
            max: [a.x.max(b.x) + w, a.y.max(b.y) + w],
            net: net.to_string(),
            layer: *layer,
            geom: CopperGeom::Segment {
                a: *a,
                b: *b,
                half_w: w / 2.0,
            },
        });
        pcb.traces.push(Trace {
            start: *a,
            end: *b,
            width: w,
            layer: *layer,
            net: net.to_string(),
            source: None,
        });
    }
    // Layer changes are barrels. `path_vias` merges a multi-step transition at
    // one point into a single barrel — two stacked vias at the same position
    // are a hole-to-hole violation against each other.
    let (via_d, via_drill) = via_geom_for(pcb, net);
    let r = via_d / 2.0;
    for (p, l0, l1) in path_vias(path) {
        // Copper on every spanned layer, and the drill in the hole index, so
        // the next path sees the whole barrel — both the annulus it must clear
        // and the hole it must keep `hole_to_hole` from.
        for layer in spanned_layers(pcb, l0, l1) {
            session.commit(CopperElement {
                min: [p.x - r, p.y - r],
                max: [p.x + r, p.y + r],
                net: net.to_string(),
                layer,
                geom: CopperGeom::Disc { center: p, r },
            });
        }
        session.commit_drill(p, via_drill);
        pcb.vias.push(Via {
            position: p,
            diameter: via_d,
            drill: via_drill,
            start_layer: l0,
            end_layer: l1,
            net: net.to_string(),
            source: None,
        });
    }
}

/// Remove all board-level copper (traces, trace arcs, vias) belonging to
/// `nets`. Returns `(traces, arcs, vias)` removed.
pub fn strip_nets(pcb: &mut Pcb, nets: &BTreeSet<String>) -> (usize, usize, usize) {
    let before = (pcb.traces.len(), pcb.trace_arcs.len(), pcb.vias.len());
    pcb.traces.retain(|t| !nets.contains(&t.net));
    pcb.trace_arcs.retain(|t| !nets.contains(&t.net));
    pcb.vias.retain(|v| !nets.contains(&v.net));
    (
        before.0 - pcb.traces.len(),
        before.1 - pcb.trace_arcs.len(),
        before.2 - pcb.vias.len(),
    )
}

/// Nets owning board-level copper within `radius` of `at`.
///
/// The attribution channel of last resort. Three rules — hole-to-hole, annular
/// ring, minimum drill — report a measurement and a position but name no net at
/// all, so a message-based census cannot see them and the fix loop would declare
/// them unreachable when they are in fact ordinary re-routing work: move the
/// offending via and the violation goes away. Reading the nets off the copper
/// sitting at the reported position closes that gap.
pub fn nets_near(pcb: &Pcb, at: [f64; 2], radius: f64) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for via in &pcb.vias {
        let (dx, dy) = (via.position.x - at[0], via.position.y - at[1]);
        if (dx * dx + dy * dy).sqrt() <= radius + via.diameter / 2.0 {
            out.insert(via.net.clone());
        }
    }
    for t in &pcb.traces {
        if point_segment_distance(at, [t.start.x, t.start.y], [t.end.x, t.end.y])
            <= radius + t.width / 2.0
        {
            out.insert(t.net.clone());
        }
    }
    out
}

/// Distance from a point to a segment.
fn point_segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (abx, aby) = (b[0] - a[0], b[1] - a[1]);
    let len2 = abx * abx + aby * aby;
    let t = if len2 <= f64::EPSILON {
        0.0
    } else {
        (((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / len2).clamp(0.0, 1.0)
    };
    let (dx, dy) = (p[0] - (a[0] + t * abx), p[1] - (a[1] + t * aby));
    (dx * dx + dy * dy).sqrt()
}

/// Every net that currently owns board-level copper.
pub fn nets_with_copper(pcb: &Pcb) -> BTreeSet<String> {
    pcb.traces
        .iter()
        .map(|t| t.net.clone())
        .chain(pcb.trace_arcs.iter().map(|t| t.net.clone()))
        .chain(pcb.vias.iter().map(|v| v.net.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{test_board, with_trace};

    #[test]
    fn strip_removes_only_the_named_nets() {
        let mut pcb = test_board();
        with_trace(&mut pcb, "A", 1.0, 1.0, 5.0, 1.0);
        with_trace(&mut pcb, "B", 1.0, 3.0, 5.0, 3.0);
        let nets: BTreeSet<String> = ["A".to_string()].into_iter().collect();
        let (t, _, _) = strip_nets(&mut pcb, &nets);
        assert_eq!(t, 1);
        assert_eq!(pcb.traces.len(), 1);
        assert_eq!(pcb.traces[0].net, "B");
    }

    #[test]
    fn nets_with_copper_sees_traces_and_vias() {
        let mut pcb = test_board();
        with_trace(&mut pcb, "A", 1.0, 1.0, 5.0, 1.0);
        pcb.vias.push(Via {
            position: Vec2::new(2.0, 2.0),
            diameter: 0.4,
            drill: 0.2,
            start_layer: vcad_ir::ecad::PcbLayer::FCu,
            end_layer: vcad_ir::ecad::PcbLayer::BCu,
            net: "B".into(),
            source: None,
        });
        let nets = nets_with_copper(&pcb);
        assert!(nets.contains("A") && nets.contains("B"));
    }

    /// A four-layer board, so a FCu..In2Cu barrel has an interior layer that a
    /// two-endpoint model cannot see.
    fn four_layer_board() -> Pcb {
        let mut pcb = test_board();
        pcb.stackup.layers = [
            PcbLayer::FCu,
            PcbLayer::In1Cu,
            PcbLayer::In2Cu,
            PcbLayer::BCu,
        ]
        .into_iter()
        .map(|layer| vcad_ir::ecad::StackupLayer {
            layer,
            copper_thickness: Some(0.035),
            dielectric_thickness: Some(0.2),
            dielectric_er: Some(4.5),
            material: Some("FR4".into()),
        })
        .collect();
        pcb
    }

    /// A two-step path FCu -> In2Cu through (5,5): one barrel, spanning In1Cu.
    fn transition_path() -> Vec<(Vec2, Vec2, PcbLayer)> {
        vec![
            (Vec2::new(3.0, 5.0), Vec2::new(5.0, 5.0), PcbLayer::FCu),
            (Vec2::new(5.0, 5.0), Vec2::new(7.0, 5.0), PcbLayer::In2Cu),
        ]
    }

    #[test]
    fn barrel_spans_interior_layers() {
        let pcb = four_layer_board();
        assert_eq!(
            spanned_layers(&pcb, PcbLayer::FCu, PcbLayer::In2Cu),
            vec![PcbLayer::FCu, PcbLayer::In1Cu, PcbLayer::In2Cu],
        );
        // Order-insensitive: the router emits (start, end) in path order.
        assert_eq!(
            spanned_layers(&pcb, PcbLayer::In2Cu, PcbLayer::FCu),
            vec![PcbLayer::FCu, PcbLayer::In1Cu, PcbLayer::In2Cu],
        );
    }

    /// A barrel overlapping foreign copper on an INTERIOR layer of its span
    /// must be refused. Committing only the endpoint layers is how two 0.000mm
    /// shorts reached the CM5 fab package: FCu..In2Cu barrels with an In1Cu
    /// trace driven through them.
    #[test]
    fn via_over_interior_layer_copper_is_refused() {
        let mut pcb = four_layer_board();
        let clear = RouteSession::from_pcb(&pcb);
        assert!(vias_legal(&pcb, &clear, "A", &transition_path()));

        pcb.traces.push(Trace {
            start: Vec2::new(5.0, 4.0),
            end: Vec2::new(5.0, 6.0),
            width: 0.2,
            layer: PcbLayer::In1Cu,
            net: "VICTIM".into(),
            source: None,
        });
        let session = RouteSession::from_pcb(&pcb);
        assert!(
            !vias_legal(&pcb, &session, "A", &transition_path()),
            "a barrel straight through an interior-layer foreign trace must be illegal"
        );
    }

    /// Hole-to-hole is a drill rule: two vias whose copper never shares a layer
    /// still collide in the drill file. The copper probe cannot see it, so the
    /// commit gate has to probe the drill index — the four CM5 hole-to-hole
    /// violations were all copper-legal.
    #[test]
    fn via_too_close_to_an_existing_hole_is_refused() {
        let mut pcb = four_layer_board();
        // Copper-legal by a mile (disjoint layers: In3Cu is not even in the
        // stackup used here), drill-illegal: centres 0.5mm, drills 0.3 → 0.2mm
        // hole-to-hole against a 0.25mm rule.
        pcb.vias.push(Via {
            position: Vec2::new(5.5, 5.0),
            diameter: 0.6,
            drill: 0.3,
            start_layer: PcbLayer::In2Cu,
            end_layer: PcbLayer::BCu,
            net: "OTHER".into(),
            source: None,
        });
        let session = RouteSession::from_pcb(&pcb);
        assert!(
            session
                .probe(
                    &CopperGeom::Disc {
                        center: Vec2::new(5.0, 5.0),
                        r: 0.3,
                    },
                    PcbLayer::FCu,
                    "A",
                    0.2,
                )
                .legal
        );
        assert!(
            !vias_legal(&pcb, &session, "A", &transition_path()),
            "a barrel 0.20mm hole-to-hole from an existing via must be illegal"
        );
    }

    /// Two barrels of ONE path have to clear each other, which the session
    /// cannot judge until they are committed.
    #[test]
    fn two_barrels_of_one_path_must_clear_each_other() {
        let pcb = four_layer_board();
        let session = RouteSession::from_pcb(&pcb);
        let path = vec![
            (Vec2::new(3.0, 5.0), Vec2::new(5.0, 5.0), PcbLayer::FCu),
            (Vec2::new(5.0, 5.0), Vec2::new(5.4, 5.0), PcbLayer::In2Cu),
            (Vec2::new(5.4, 5.0), Vec2::new(7.0, 5.0), PcbLayer::FCu),
        ];
        assert!(
            !vias_legal(&pcb, &session, "A", &path),
            "two barrels 0.10mm hole-to-hole apart must be illegal"
        );
    }

    /// The commit indexes the whole barrel — every spanned layer's copper AND
    /// the drill — so the next path sees it.
    #[test]
    fn commit_indexes_barrel_copper_and_drill() {
        let mut pcb = four_layer_board();
        let mut session = RouteSession::from_pcb(&pcb);
        commit_path(&mut pcb, &mut session, "A", 0.2, &transition_path());
        assert_eq!(pcb.vias.len(), 1);
        assert_eq!(pcb.vias[0].position, Vec2::new(5.0, 5.0));
        // Interior layer now blocked for a foreign net...
        assert!(
            !session
                .probe(
                    &CopperGeom::Segment {
                        a: Vec2::new(5.0, 4.0),
                        b: Vec2::new(5.0, 6.0),
                        half_w: 0.1,
                    },
                    PcbLayer::In1Cu,
                    "VICTIM",
                    0.2,
                )
                .legal
        );
        // ...and the drill is in the hole index.
        assert!(!session.probe_drill(Vec2::new(5.5, 5.0), 0.3).legal);
        // A second path through the same spot is now refused rather than
        // stacking a coincident drill.
        assert!(!vias_legal(&pcb, &session, "B", &transition_path()));
    }
}
