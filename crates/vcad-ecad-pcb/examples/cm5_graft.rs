//! Graft test: copy the HUMAN's routing for our stuck nets onto our routed
//! board and DRC the result. Clean => the channels are open in our world and
//! the failures are search-side. Dirty => our copper occupies their space,
//! and the violations name exactly which of our nets to rip.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example cm5_graft -- \
//!     CM5RevEng.kicad_pcb ours.pcb.json stuck.json
//! ```

use std::collections::BTreeMap;
use vcad_ecad_pcb::drc::{check_drc, DrcRuleType};
use vcad_ecad_symbols::parse_kicad_pcb;
use vcad_ir::ecad::Pcb;

fn main() {
    let mut args = std::env::args().skip(1);
    let human_p = args.next().expect("human.kicad_pcb");
    let ours_p = args.next().expect("ours.pcb.json");
    let stuck_p = args.next().expect("stuck.json");
    let human = parse_kicad_pcb(&std::fs::read_to_string(&human_p).expect("read")).expect("parse");
    let mut ours: Pcb =
        serde_json::from_str(&std::fs::read_to_string(&ours_p).expect("read")).expect("parse");
    let stuck: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string(&stuck_p).expect("read")).expect("parse");
    // Signal nets only: plane-backed nets are served by pours + stitching in
    // our world; grafting their trace copper answers nothing.
    let planes: Vec<&str> = ours.zones.iter().map(|z| z.net.as_str()).collect();
    let targets: Vec<&String> = stuck
        .iter()
        .filter(|n| !planes.contains(&n.as_str()))
        .collect();
    println!(
        "grafting {} signal nets (skipped {} plane-backed)",
        targets.len(),
        stuck.len() - targets.len()
    );

    let mut grafted_traces = 0;
    for t in &human.traces {
        if targets.iter().any(|n| ***n == t.net) {
            ours.traces.push(t.clone());
            grafted_traces += 1;
        }
    }
    let mut grafted_vias = 0;
    for v in &human.vias {
        if targets.iter().any(|n| ***n == v.net) {
            ours.vias.push(v.clone());
            grafted_vias += 1;
        }
    }
    println!("grafted {grafted_traces} traces, {grafted_vias} vias");

    let violations = check_drc(&ours);
    let mut per_net: BTreeMap<String, usize> = BTreeMap::new();
    let mut hard = 0;
    for v in &violations {
        if matches!(v.rule, DrcRuleType::Short | DrcRuleType::Clearance) {
            let involves = targets.iter().any(|n| v.message.contains(n.as_str()));
            if involves {
                hard += 1;
                // crude blame: find OUR net named in the message that is not a target
                per_net.entry(v.message.clone()).or_insert(0);
                if per_net.len() <= 12 && *per_net.get(&v.message).unwrap() == 0 {
                    println!("CONFLICT: {}", &v.message[..v.message.len().min(140)]);
                }
                *per_net.get_mut(&v.message).unwrap() += 1;
            }
        }
    }
    println!("\n== GRAFT VERDICT: {hard} short/clearance violations involving grafted nets ==");
    if hard == 0 {
        println!("channels are OPEN — failures are search-side, not fabric-side");
    }
}
