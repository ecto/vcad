//! Flow-core router bench: strip a board's routing, route it with
//! `route_flow`, and score the result (connections, split nets, via mix,
//! route-attributable DRC delta, market statistics).
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example flow_bench -- \
//!     board.pcb.json [out.pcb.json] [report.json]
//! ```
//! `VCAD_FLOW_ITERS`, `VCAD_FLOW_NETS=<json list>` (subset),
//! `VCAD_FLOW_KEEP=1` (route on top of the existing copper instead of
//! stripping it), `VCAD_FLOW_CLASSES=0` (skip name-based net classes).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use vcad_ecad_pcb::drc::check_drc;
use vcad_ecad_pcb::router::classes::{apply_classes, classify_nets};
use vcad_ecad_pcb::router::flow::{route_flow, FlowOptions};
use vcad_ir::ecad::{Pcb, Trace};

fn rule_counts(pcb: &Pcb) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for v in check_drc(pcb) {
        *m.entry(format!("{:?}", v.rule)).or_default() += 1;
    }
    m
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: flow_bench <board.pcb.json> [out] [report]");
    let out = args.next();
    let report_out = args.next();
    let text = std::fs::read_to_string(&path).expect("read board");
    let mut pcb: Pcb = if path.ends_with(".json") {
        serde_json::from_str(&text).expect("parse json")
    } else {
        vcad_ecad_symbols::parse_kicad_pcb(&text).expect("parse kicad")
    };
    let human_vias = pcb.vias.len();
    if std::env::var("VCAD_FLOW_KEEP").as_deref() != Ok("1") {
        pcb.traces.clear();
        pcb.trace_arcs.clear();
        pcb.vias.clear();
    }
    if std::env::var("VCAD_FLOW_CLASSES").as_deref() != Ok("0") {
        let all: Vec<String> = pcb
            .footprints
            .iter()
            .flat_map(|f| f.pads.iter().filter_map(|p| p.net.clone()))
            .filter(|n| !n.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let cls = classify_nets(&all);
        apply_classes(&mut pcb, &cls);
        println!(
            "classes: {} diff pairs, {} match groups",
            cls.pairs.len(),
            cls.match_groups.len()
        );
    }
    let filter: Vec<String> = std::env::var("VCAD_FLOW_NETS")
        .ok()
        .map(|p| serde_json::from_str(&std::fs::read_to_string(p).expect("nets")).expect("json"))
        .unwrap_or_default();
    let mut opts = FlowOptions::default();
    if let Some(n) = std::env::var("VCAD_FLOW_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        opts.params.iters = n;
    }
    let before = rule_counts(&pcb);
    let width = pcb.rules.default_rules.trace_width;
    let t0 = Instant::now();
    let (r, rep) = route_flow(&pcb, width, &filter, &opts);
    let secs = t0.elapsed().as_secs_f64();

    for t in &r.traces {
        pcb.traces.push(Trace {
            start: t.start,
            end: t.end,
            width: t.width,
            layer: t.layer,
            net: t.net.clone(),
            source: None,
        });
    }
    let sized = pcb.clone();
    let mut mix: BTreeMap<String, usize> = BTreeMap::new();
    for v in &r.vias {
        let bv = v.to_board_via(&sized, None);
        *mix.entry(format!(
            "{:?}",
            bv.via_type.unwrap_or(vcad_ir::ecad::ViaType::Through)
        ))
        .or_default() += 1;
        pcb.vias.push(bv);
    }
    let after = rule_counts(&pcb);
    // Split nets: every attempted net whose pads are not one island.
    let attempted: Vec<String> = r
        .routed_nets
        .iter()
        .chain(&r.unrouted_nets)
        .cloned()
        .collect();
    let islands = vcad_ecad_pcb::drc::net_islands(&pcb, &attempted);
    let connected = islands.values().filter(|&&n| n <= 1).count();

    println!(
        "graph: {} verts {} faces {} cuts, {} via sites; {} commodities ({} plane nets left to planes)",
        rep.graph[0], rep.graph[1], rep.graph[2], rep.sites, rep.commodities, rep.plane_nets
    );
    println!("iter  overflow  ovf_cuts   cost_mm     LB_mm  unrouted  site_conf");
    for s in &rep.iters {
        println!(
            "{:>4} {:>9.2} {:>9} {:>9.1} {:>9.1} {:>9} {:>10}",
            s.iter,
            s.overflow,
            s.overflowed_cuts,
            s.cost,
            s.lower_bound,
            s.unrouted,
            s.site_conflicts
        );
    }
    println!(
        "market: best LB {:.1} mm, UB {:?}, gap {:?}",
        rep.lower_bound,
        rep.upper_bound.map(|u| (u * 10.0).round() / 10.0),
        rep.gap.map(|g| (g * 1e4).round() / 1e4)
    );
    println!(
        "connections: {}/{} planned, {} committed, {} rejected by the oracle; nets {} connected / {} attempted ({} split)",
        rep.planned,
        rep.commodities,
        rep.committed,
        rep.rejected,
        connected,
        attempted.len(),
        attempted.len() - connected
    );
    println!(
        "copper: {} segments, {} vias {:?} (human had {human_vias} vias)",
        r.traces.len(),
        r.vias.len(),
        mix
    );
    let mut delta: BTreeMap<String, i64> = BTreeMap::new();
    for k in before.keys().chain(after.keys()) {
        let d = *after.get(k).unwrap_or(&0) as i64 - *before.get(k).unwrap_or(&0) as i64;
        if d != 0 {
            delta.insert(k.clone(), d);
        }
    }
    println!("drc delta (after - before, by rule): {delta:?}");
    println!(
        "time: {secs:.1} s (graph {:.0} ms, market {:.0} ms, realize+oracle {:.0} ms, legalize {:.0} ms)",
        rep.ms[0], rep.ms[1], rep.ms[2], rep.ms[3]
    );
    if let Some(o) = out {
        std::fs::write(&o, serde_json::to_string(&pcb).unwrap()).unwrap();
    }
    if let Some(o) = report_out {
        std::fs::write(&o, serde_json::to_string_pretty(&rep).unwrap()).unwrap();
    }
}
