//! SI tuner: close length-match skew on a routed board. Classifies nets
//! (pairs + match groups), grows short members with clearance-checked
//! meanders on their longest single-layer run, and saves the tuned board.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example si_tune -- in.pcb.json out.pcb.json
//! ```

use vcad_ecad_pcb::router::classes::classify_nets;
use vcad_ecad_pcb::router::length_match::{match_lengths_runs, LengthMatchOptions};
use vcad_ir::ecad::Pcb;

fn tune_group(pcb: &mut Pcb, label: &str, nets: &[String], tolerance: f64) {
    let opts = LengthMatchOptions {
        tolerance,
        ..Default::default()
    };
    let r = match_lengths_runs(pcb, nets, &opts);
    let (mut tuned, mut skipped) = (0usize, 0usize);
    for n in &r.nets {
        if n.tuned {
            tuned += 1;
            pcb.traces.retain(|t| t.net != n.net);
            pcb.traces.extend(n.new_traces.iter().cloned());
        } else if n.skip_reason.is_some() {
            skipped += 1;
        }
    }
    println!(
        "tune {label}: target {:.2} mm, tuned {tuned}, skipped {skipped}, all_matched={}",
        r.target_length, r.all_matched
    );
    for n in r.nets.iter().filter(|n| !n.matched) {
        println!(
            "  unmatched {}: {:.2} -> {:.2} mm ({})",
            n.net,
            n.length_before,
            n.length_after,
            n.skip_reason.as_deref().unwrap_or("meander fell short")
        );
    }
}

fn main() {
    env_logger::init();
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: si_tune <in.pcb.json> <out.pcb.json>");
        std::process::exit(2);
    };
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
    let c = classify_nets(&nets);

    // Bus groups first (coarser tolerance — the human board's own raw-skew
    // discipline), then pairs at tight intra-pair tolerance: pair legs also
    // sit in no bus group, so the two passes never fight.
    for (gname, members) in &c.match_groups {
        tune_group(&mut pcb, gname, members, 1.0);
    }
    for (p, n) in &c.pairs {
        tune_group(&mut pcb, &format!("pair {p}"), &[p.clone(), n.clone()], 0.1);
    }

    std::fs::write(&output, serde_json::to_string(&pcb).expect("serialize")).expect("write");
    eprintln!("wrote {output}");
}
