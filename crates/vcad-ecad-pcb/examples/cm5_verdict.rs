//! Verdict driver: put the still-unrouted connections of a routed board in
//! front of the COMPLETE window router, cluster by cluster, and demand an
//! answer — Routed (commit-quality paths), ProvedInfeasible (bottleneck-cut
//! certificate), or BudgetExhausted (honest unknown). The campaign's closing
//! argument: every connection ends accounted for.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example cm5_verdict -- routed.pcb.json [budget]
//! ```

use vcad_ecad_pcb::router::complete::{route_window_complete, CompleteOutcome};
use vcad_ecad_pcb::ratsnest::{compute_ratsnest, NetConnection, Netlist, NetlistNet};
use vcad_ecad_pcb::session::RouteSession;
use vcad_ir::ecad::Pcb;
use vcad_ir::Vec2;
use std::collections::BTreeMap;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: cm5_verdict <routed.pcb.json> [budget]");
    let budget: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(5_000_000);
    let pcb: Pcb = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");

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
    let rats = compute_ratsnest(&pcb, &netlist);
    println!("unrouted connections: {}", rats.len());

    let session = RouteSession::from_pcb(&pcb);
    let layers: Vec<_> = pcb
        .stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper())
        .collect();
    let width = pcb.rules.default_rules.trace_width;

    // Cluster connections whose bboxes (inflated 4mm) overlap.
    let mut clusters: Vec<(Vec2, Vec2, Vec<(String, Vec2, Vec2)>)> = Vec::new();
    'c: for l in &rats {
        let (lo, hi) = (
            Vec2::new(l.from.x.min(l.to.x) - 4.0, l.from.y.min(l.to.y) - 4.0),
            Vec2::new(l.from.x.max(l.to.x) + 4.0, l.from.y.max(l.to.y) + 4.0),
        );
        for (clo, chi, cc) in clusters.iter_mut() {
            if lo.x <= chi.x && clo.x <= hi.x && lo.y <= chi.y && clo.y <= hi.y && cc.len() < 6 {
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

    let (mut routed, mut proved, mut unknown) = (0usize, 0usize, 0usize);
    for (lo, hi, conns) in &clusters {
        let names: Vec<&str> = conns.iter().map(|c| c.0.as_str()).collect();
        match route_window_complete(&session, (*lo, *hi), &layers, conns, width, budget) {
            CompleteOutcome::Routed(paths) => {
                routed += conns.len();
                let segs: usize = paths.iter().map(|p| p.len()).sum();
                println!("ROUTED   {names:?} ({segs} segments found)");
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
}
