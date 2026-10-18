//! Flow router M3: plane access as an exact sub-market on a stripped board —
//! per-iteration duality gap and the resulting via mix.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example flow_m3 -- board.pcb.json
//! ```

use std::collections::BTreeMap;
use std::time::Instant;

use vcad_ecad_pcb::router::flow::access::AccessMarket;
use vcad_ir::ecad::Pcb;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: flow_m3 <board.pcb.json>");
    let mut pcb: Pcb =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    let mut human: BTreeMap<String, usize> = BTreeMap::new();
    for v in &pcb.vias {
        *human
            .entry(format!("{:?}-{:?}", v.start_layer, v.end_layer))
            .or_default() += 1;
    }
    pcb.traces.clear();
    pcb.trace_arcs.clear();
    pcb.vias.clear();
    let t0 = Instant::now();
    let mk = AccessMarket::build(&pcb, 1.0, 2.0);
    let build = t0.elapsed().as_secs_f64();
    let no_opt = mk.options.iter().filter(|o| o.is_empty()).count();
    println!(
        "access: {} pads need a via ({} already on their pour, {} with no legal option), {} sites; build {build:.1} s",
        mk.pads.len(),
        mk.served_by_pour,
        no_opt,
        mk.sites.len()
    );
    let t1 = Instant::now();
    let (choice, stats) = mk.solve(200, 2.0);
    println!("iter  conflicts    cost_mm      LB_mm      UB_mm     gap");
    for s in stats
        .iter()
        .filter(|s| s.iter < 10 || s.iter % 10 == 0 || s.conflicts == 0)
    {
        println!(
            "{:>4} {:>10} {:>10.2} {:>10.2} {:>10.2} {:>7.4}",
            s.iter, s.conflicts, s.cost, s.lower_bound, s.upper_bound, s.gap
        );
    }
    let last = stats.last().unwrap();
    println!(
        "solved in {} iterations, {:.2} s; final gap {:.4}",
        stats.len(),
        t1.elapsed().as_secs_f64(),
        last.gap
    );
    let vias = mk.vias(&choice);
    let mut mix: BTreeMap<String, usize> = BTreeMap::new();
    let mut in_pad = 0usize;
    for v in &vias {
        *mix.entry(format!("{:?}-{:?}", v.span.0, v.span.1))
            .or_default() += 1;
        let c = mk.pads[v.pad].center;
        if (c[0] - v.pos[0]).abs() < 1e-9 && (c[1] - v.pos[1]).abs() < 1e-9 {
            in_pad += 1;
        }
    }
    println!(
        "access vias: {} ({} in pad, {} dog-bone) by span {mix:?}",
        vias.len(),
        in_pad,
        vias.len() - in_pad
    );
    println!("human vias by span (all nets): {human:?}");
}
