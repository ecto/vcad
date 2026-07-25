//! Verdict driver: put the still-unrouted connections of a routed board in
//! front of the COMPLETE window router, cluster by cluster, and demand an
//! answer — Routed (commit-quality paths), ProvedInfeasible (bottleneck-cut
//! certificate), or BudgetExhausted (honest unknown). The campaign's closing
//! argument: every connection ends accounted for.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example cm5_verdict -- routed.pcb.json [budget] [out.pcb.json] [max_cluster]
//! ```

use std::collections::BTreeMap;
use vcad_ecad_pcb::ratsnest::{compute_ratsnest, NetConnection, Netlist, NetlistNet};
use vcad_ecad_pcb::router::complete::{route_window_complete, CompleteOutcome};
use vcad_ecad_pcb::session::RouteSession;
use vcad_ecad_pcb::spatial::{CopperElement, CopperGeom};
use vcad_ir::ecad::Pcb;
use vcad_ir::Vec2;

type Cluster = (Vec2, Vec2, Vec<(String, Vec2, Vec2)>);

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: cm5_verdict <routed.pcb.json> [budget]");
    let budget: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5_000_000);
    let out_json = args.next();
    let max_cluster: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(6);
    let mut pcb: Pcb =
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

    let mut session = RouteSession::from_pcb(&pcb);
    let layers: Vec<_> = pcb
        .stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper())
        .collect();
    // Fallback width only: `route_window_complete` searches at the widest
    // net-class width in each window and reports the per-net commit width.
    let width = pcb.rules.default_rules.trace_width;

    // Cluster connections whose bboxes (inflated 2mm) overlap. Two rules keep
    // the certificates honest: a cluster never holds two connections of the
    // same net (per-connection node-disjointness can't model same-net cell
    // sharing), and the merged window is capped — a big coalesced window
    // coarsens the grid pitch (MAX_AXIS_CELLS) until unrelated terminals
    // artificially collide.
    const MAX_WINDOW_MM: f64 = 20.0;
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
                && cc.len() < max_cluster
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
    println!("clusters: {}", clusters.len());

    let via_radius = pcb.rules.default_rules.via_diameter / 2.0;
    let (mut routed, mut proved, mut unknown) = (0usize, 0usize, 0usize);
    for (lo, hi, conns) in &clusters {
        let names: Vec<&str> = conns.iter().map(|c| c.0.as_str()).collect();
        match route_window_complete(
            &session,
            (*lo, *hi),
            &layers,
            conns,
            width,
            via_radius,
            budget,
        ) {
            CompleteOutcome::Routed(paths) => {
                // The router has already replayed these paths in order against
                // a scratch session, so mutual legality is settled: commit the
                // legal ones (at their own class width), in order. A path it
                // flags illegal pinched a clustermate through a gap the coarse
                // grid could not see — an honest unknown for that net alone.
                let mut cluster_routed = 0usize;
                for path in &paths {
                    let (net, w) = (&path.net, path.width);
                    if !path.legal {
                        unknown += 1;
                        println!("UNKNOWN  [{net:?}] (path failed mutual-legality probe)");
                        continue;
                    }
                    cluster_routed += 1;
                    for &(a, b, layer) in &path.segments {
                        session.commit(CopperElement {
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
                        pcb.traces.push(vcad_ir::ecad::Trace {
                            start: a,
                            end: b,
                            width: w,
                            layer,
                            net: net.clone(),
                            source: None,
                        });
                    }
                    for &(c, l0, l1) in &path.vias {
                        let r = pcb.rules.default_rules.via_diameter / 2.0;
                        for layer in [l0, l1] {
                            session.commit(CopperElement {
                                min: [c.x - r, c.y - r],
                                max: [c.x + r, c.y + r],
                                net: net.clone(),
                                layer,
                                geom: CopperGeom::Disc { center: c, r },
                            });
                        }
                        pcb.vias.push(vcad_ir::ecad::Via {
                            position: c,
                            diameter: pcb.rules.default_rules.via_diameter,
                            drill: pcb.rules.default_rules.via_drill,
                            start_layer: l0,
                            end_layer: l1,
                            net: net.clone(),
                            source: None,
                        });
                    }
                }
                routed += cluster_routed;
                if cluster_routed > 0 {
                    println!(
                        "ROUTED   {names:?} ({cluster_routed}/{} committed)",
                        conns.len()
                    );
                }
            }
            CompleteOutcome::ProvedInfeasible { reason } => {
                proved += conns.len();
                println!("PROVED   {names:?}: {reason}");
            }
            CompleteOutcome::BudgetExhausted => {
                unknown += conns.len();
                println!("UNKNOWN  {names:?} (budget {budget} exhausted)");
            }
        }
    }
    println!("\n== VERDICT: routed {routed} / proved-infeasible {proved} / unknown {unknown} ==");
    if let Some(out) = out_json {
        std::fs::write(&out, serde_json::to_string(&pcb).expect("serialize")).expect("write");
        eprintln!("wrote {out}");
    }
}
