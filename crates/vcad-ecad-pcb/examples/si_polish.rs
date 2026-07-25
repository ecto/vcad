//! Pair polish: rip each still-uncoupled differential pair off a finished
//! board and re-route it coupled against the settled copper. Strictly
//! non-regressive per pair (failure restores the original copper).
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example si_polish -- in.pcb.json out.pcb.json [expansions]
//! ```

use vcad_ecad_pcb::router::classes::{apply_classes, classify_nets};
use vcad_ecad_pcb::router::polish_pairs;
use vcad_ir::ecad::Pcb;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("usage: si_polish <in> <out> [exp]");
    let output = args.next().expect("usage: si_polish <in> <out> [exp]");
    let expansions: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000_000);

    let mut pcb: Pcb =
        serde_json::from_str(&std::fs::read_to_string(&input).expect("read")).expect("parse");
    let nets: Vec<String> = {
        let mut v: std::collections::BTreeSet<String> = Default::default();
        for f in &pcb.footprints {
            for pad in &f.pads {
                if let Some(n) = &pad.net {
                    if !n.is_empty() {
                        v.insert(n.clone());
                    }
                }
            }
        }
        v.into_iter().collect()
    };
    let classifier = classify_nets(&nets);
    apply_classes(&mut pcb, &classifier);
    let (polished, attempted) = polish_pairs(&mut pcb, expansions);
    println!("pair-polish: {polished}/{attempted} uncoupled pairs re-routed coupled");
    std::fs::write(&output, serde_json::to_string(&pcb).expect("serialize")).expect("write");
    eprintln!("wrote {output}");
}
