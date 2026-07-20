//! SI report: classify a board's nets (pairs + match groups) and print the
//! routed-length skew tables — for any board, imported `.kicad_pcb` or saved
//! `.pcb.json`. Run it on the human-routed CM5 to get the ground-truth skew
//! discipline our tuner must reach; run it on an autorouted board to score.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example si_report -- board.kicad_pcb
//! ```

use vcad_ecad_pcb::router::classes::classify_nets;
use vcad_ecad_pcb::router::length_match::net_routed_length;
use vcad_ecad_symbols::parse_kicad_pcb;
use vcad_ir::ecad::Pcb;

fn main() {
    let path = std::env::args().nth(1).expect("usage: si_report <board>");
    let text = std::fs::read_to_string(&path).expect("read board");
    let pcb: Pcb = if path.ends_with(".json") {
        serde_json::from_str(&text).expect("parse pcb json")
    } else {
        parse_kicad_pcb(&text).expect("parse kicad_pcb")
    };

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
    let c = classify_nets(&nets);
    println!(
        "classes: {} pairs, {} groups",
        c.pairs.len(),
        c.match_groups.len()
    );

    for (gname, members) in &c.match_groups {
        let lens: Vec<(f64, &str)> = members
            .iter()
            .map(|n| (net_routed_length(&pcb, n), n.as_str()))
            .filter(|(l, _)| *l > 0.0)
            .collect();
        if lens.len() > 1 {
            let max = lens.iter().map(|(l, _)| *l).fold(f64::MIN, f64::max);
            let min = lens.iter().map(|(l, _)| *l).fold(f64::MAX, f64::min);
            println!(
                "group {gname}: {} nets, {min:.2}..{max:.2} mm, skew {:.2} mm",
                lens.len(),
                max - min
            );
        }
    }
    let mut worst: (f64, String) = (0.0, String::new());
    let mut measured = 0usize;
    for (p, n) in &c.pairs {
        let (lp, ln) = (net_routed_length(&pcb, p), net_routed_length(&pcb, n));
        if lp > 0.0 && ln > 0.0 {
            measured += 1;
            let skew = (lp - ln).abs();
            if skew > worst.0 {
                worst = (skew, p.clone());
            }
        }
    }
    println!(
        "pairs: {measured} measured, worst intra-pair skew {:.3} mm ({})",
        worst.0, worst.1
    );
}
