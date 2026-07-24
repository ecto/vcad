//! Remove dangling copper (islands touching no pad/pour of their net) from a
//! routed `.pcb.json`. See [`vcad_ecad_pcb::drc::prune_dangling_copper`].
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example prune_copper -- in.pcb.json out.pcb.json
//! ```

use vcad_ir::ecad::Pcb;

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("usage: prune_copper <in> <out>");
    let output = args.next().expect("usage: prune_copper <in> <out>");
    let mut pcb: Pcb =
        serde_json::from_str(&std::fs::read_to_string(&input).expect("read")).expect("parse");
    let (t, v) = vcad_ecad_pcb::drc::prune_dangling_copper(&mut pcb);
    println!("pruned {t} dangling traces, {v} dangling vias");
    std::fs::write(&output, serde_json::to_string(&pcb).expect("ser")).expect("write");
}
