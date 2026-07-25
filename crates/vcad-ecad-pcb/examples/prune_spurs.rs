//! Scratch: remove spur copper (connected dead-end branches) from a board.

use vcad_ecad_symbols::parse_kicad_pcb;
use vcad_ir::ecad::Pcb;

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("usage: prune_spurs <in> <out>");
    let output = args.next().expect("usage: prune_spurs <in> <out>");
    let text = std::fs::read_to_string(&input).expect("read");
    let mut pcb: Pcb = if input.ends_with(".json") {
        serde_json::from_str(&text).expect("parse")
    } else {
        parse_kicad_pcb(&text).expect("parse")
    };
    let (t0, v0) = (pcb.traces.len(), pcb.vias.len());
    let (t, v) = vcad_ecad_pcb::drc::prune_spur_copper(&mut pcb);
    println!("pruned {t} spur traces of {t0}, {v} spur vias of {v0}");
    std::fs::write(&output, serde_json::to_string(&pcb).unwrap()).expect("write");
}
