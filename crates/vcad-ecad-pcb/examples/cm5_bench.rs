//! CM5 ground-truth benchmark: import a routed KiCad board (the reference
//! fixture is schlae/cm5-reveng's 10-layer Raspberry Pi CM5 reverse
//! engineering), strip its human-routed copper, autoroute, and score against
//! the human layout — completion, via count, and total copper length.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example cm5_bench -- \
//!     CM5RevEng.kicad_pcb [effort] [max_nets] [out.pcb.json]
//! ```
//!
//! `max_nets` (default all) routes only the N highest-pad-count nets — handy
//! for a quick smoke run on the full board. `out.pcb.json` saves the board
//! with the routed copper applied, so rendering (vcad-render's `cm5_render`)
//! and styling iteration never pay for another routing run.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use vcad_ecad_pcb::router::{route_all_with_opts, RouteOptions};
use vcad_ecad_symbols::parse_kicad_pcb;
use vcad_ir::ecad::{Trace, Via};

fn seg_len(a: vcad_ir::Vec2, b: vcad_ir::Vec2) -> f64 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| {
        eprintln!("usage: cm5_bench <board.kicad_pcb> [effort] [max_nets]");
        std::process::exit(2);
    });
    let effort: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let max_nets: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    let out_json = args.next();

    let text = std::fs::read_to_string(&path).expect("read kicad_pcb");
    let mut pcb = parse_kicad_pcb(&text).expect("parse kicad_pcb");

    // Ground truth: the human routing we are about to strip.
    let human_traces = pcb.traces.len();
    let human_vias = pcb.vias.len();
    let human_len: f64 = pcb.traces.iter().map(|t| seg_len(t.start, t.end)).sum();
    let human_nets: BTreeSet<&str> = pcb.traces.iter().map(|t| t.net.as_str()).collect();

    println!(
        "board: {} copper layers, {} nets, {} footprints, {} pads",
        pcb.stackup
            .layers
            .iter()
            .filter(|l| l.layer.is_copper())
            .count(),
        pcb.nets.len(),
        pcb.footprints.len(),
        pcb.footprints.iter().map(|f| f.pads.len()).sum::<usize>(),
    );
    println!(
        "human: {human_traces} segments, {human_vias} vias, {:.0} mm copper, {} routed nets",
        human_len,
        human_nets.len()
    );

    // Strip everything the autorouter is expected to produce. Zones stay: a
    // poured plane is design intent (the router stitches to it, not through it).
    pcb.traces.clear();
    pcb.trace_arcs.clear();
    pcb.vias.clear();

    // Optional subset: the N nets with the most pads (the hard ones first).
    // Keyed by the pad net strings — the exact names the router's pad-derived
    // netlist and ratsnest match on (`pcb.nets` ids can differ from pad net
    // names on imported boards, which made the filter silently select nothing).
    let filter: Vec<String> = if max_nets == usize::MAX {
        Vec::new()
    } else {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for p in pcb.footprints.iter().flat_map(|f| f.pads.iter()) {
            if let Some(net) = p.net.as_deref() {
                if !net.is_empty() {
                    *counts.entry(net).or_default() += 1;
                }
            }
        }
        let mut counts: Vec<(&str, usize)> = counts.into_iter().filter(|(_, c)| *c >= 2).collect();
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        counts
            .into_iter()
            .take(max_nets)
            .map(|(n, _)| n.to_string())
            .collect()
    };

    let width = pcb.rules.default_rules.trace_width;
    let t0 = Instant::now();
    let r = route_all_with_opts(
        &pcb,
        width,
        &filter,
        &RouteOptions {
            effort,
            ..Default::default()
        },
    );
    let elapsed = t0.elapsed();

    let routed_len: f64 = r.traces.iter().map(|t| seg_len(t.start, t.end)).sum();

    println!(
        "vcad:  {} segments, {} vias, {:.0} mm copper, {} routed / {} unrouted nets",
        r.traces.len(),
        r.vias.len(),
        routed_len,
        r.routed_nets.len(),
        r.unrouted_nets.len(),
    );
    println!(
        "score: routability {:.3}, via ratio {:.2}x human, length ratio {:.2}x human, {:.1}s",
        r.routability,
        if human_vias > 0 {
            r.vias.len() as f64 / human_vias as f64
        } else {
            f64::NAN
        },
        if human_len > 0.0 {
            routed_len / human_len
        } else {
            f64::NAN
        },
        elapsed.as_secs_f64(),
    );
    for d in r.diagnostics.iter().take(10) {
        eprintln!("unrouted {}: {}", d.net, d.reason);
    }

    // Save the routed board for rendering / inspection without re-routing.
    if let Some(out) = out_json {
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
        let copper: Vec<_> = pcb
            .stackup
            .layers
            .iter()
            .map(|l| l.layer)
            .filter(|l| l.is_copper())
            .collect();
        let start_layer = *copper.first().expect("board has copper");
        let end_layer = *copper.last().expect("board has copper");
        for v in &r.vias {
            pcb.vias.push(Via {
                position: v.position,
                diameter: pcb.rules.default_rules.via_diameter,
                drill: pcb.rules.default_rules.via_drill,
                start_layer,
                end_layer,
                net: v.net.clone(),
                source: None,
            });
        }
        std::fs::write(&out, serde_json::to_string(&pcb).expect("serialize board"))
            .expect("write routed board json");
        eprintln!("wrote {out}");
    }
}
