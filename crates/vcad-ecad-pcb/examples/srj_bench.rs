//! Route a Simple Route JSON problem (the tscircuit benchmark format) and
//! print a metrics line — the plug-in point for scoring vcad's router on the
//! public tscircuit datasets.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example srj_bench -- problem.srj.json [effort]
//! ```

use std::time::Instant;

use vcad_ecad_pcb::router::{route_all_with_opts, RouteOptions};
use vcad_ecad_pcb::srj::{srj_net_filter, srj_to_pcb, SimpleRouteJson};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| {
        eprintln!("usage: srj_bench <problem.srj.json> [effort]");
        std::process::exit(2);
    });
    let effort: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);

    let text = std::fs::read_to_string(&path).expect("read SRJ file");
    let srj: SimpleRouteJson = serde_json::from_str(&text).expect("parse SRJ");
    let pcb = srj_to_pcb(&srj);
    let filter = srj_net_filter(&srj);

    let t0 = Instant::now();
    let r = route_all_with_opts(
        &pcb,
        srj.min_trace_width,
        &filter,
        &RouteOptions {
            effort,
            ..Default::default()
        },
    );
    let elapsed = t0.elapsed();

    let total_len: f64 = r
        .traces
        .iter()
        .map(|t| ((t.end.x - t.start.x).powi(2) + (t.end.y - t.start.y).powi(2)).sqrt())
        .sum();

    println!(
        "{}",
        serde_json::json!({
            "problem": path,
            "layer_count": srj.layer_count,
            "connections": srj.connections.len(),
            "routed_nets": r.routed_nets.len(),
            "unrouted_nets": r.unrouted_nets.len(),
            "routability": r.routability,
            "traces": r.traces.len(),
            "vias": r.vias.len(),
            "total_length_mm": (total_len * 1000.0).round() / 1000.0,
            "duration_ms": elapsed.as_millis(),
            "effort": effort,
        })
    );
    if !r.unrouted_nets.is_empty() {
        eprintln!("unrouted: {:?}", r.unrouted_nets);
    }
}
