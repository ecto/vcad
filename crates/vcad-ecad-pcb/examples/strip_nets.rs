//! Strip all board-level copper (traces + vias) of the named nets from a
//! routed `.pcb.json` — the first half of the DRC fix loop: offenders are
//! stripped here, then re-routed through the session-probed ladder
//! (`cm5_verdict`), which only commits legal copper.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example strip_nets -- in.pcb.json out.pcb.json NET1 NET2 ...
//! ```

use vcad_ir::ecad::Pcb;

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .expect("usage: strip_nets <in.pcb.json> <out.pcb.json> <net>...");
    let output = args
        .next()
        .expect("usage: strip_nets <in.pcb.json> <out.pcb.json> <net>...");
    let nets: Vec<String> = args.collect();
    assert!(!nets.is_empty(), "no nets given");

    let mut pcb: Pcb =
        serde_json::from_str(&std::fs::read_to_string(&input).expect("read")).expect("parse");
    let before = (pcb.traces.len(), pcb.vias.len());
    pcb.traces.retain(|t| !nets.contains(&t.net));
    pcb.vias.retain(|v| !nets.contains(&v.net));
    println!(
        "stripped {} traces, {} vias across {} nets",
        before.0 - pcb.traces.len(),
        before.1 - pcb.vias.len(),
        nets.len()
    );
    std::fs::write(&output, serde_json::to_string(&pcb).expect("serialize")).expect("write");
}
