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
    // Reference-plane discipline: which copper layers hold a large zone
    // (plane), and for each SI-class net, how much of its length rides a
    // layer ADJACENT (stackup-neighbouring) to a plane — the return-path
    // metric — plus its via count (each via is a reference change).
    {
        let mut plane_pos: std::collections::BTreeSet<u8> = Default::default();
        for z in &pcb.zones {
            let area = {
                let v = &z.outline;
                let mut a = 0.0;
                for i in 0..v.len() {
                    let j = (i + 1) % v.len();
                    a += v[i].x * v[j].y - v[j].x * v[i].y;
                }
                (a / 2.0).abs()
            };
            if area > 100.0 {
                if let Some(pz) = z.layer.copper_position() {
                    plane_pos.insert(pz);
                }
            }
        }
        let referenced = |pos: u8| {
            plane_pos.contains(&pos)
                || (pos > 0 && plane_pos.contains(&(pos - 1)))
                || plane_pos.contains(&(pos + 1))
        };
        let si_nets: Vec<&String> = c.pairs.iter().flat_map(|(p, n)| [p, n]).collect();
        let (mut ref_len, mut tot_len) = (0.0f64, 0.0f64);
        let mut vias = 0usize;
        let mut worst_net = (1.0f64, String::new());
        for net in &si_nets {
            let (mut r, mut t) = (0.0f64, 0.0f64);
            for tr in pcb.traces.iter().filter(|t| &t.net == *net) {
                let l = (tr.end - tr.start).length();
                t += l;
                if tr.layer.copper_position().map(referenced).unwrap_or(false) {
                    r += l;
                }
            }
            vias += pcb.vias.iter().filter(|v| &v.net == *net).count();
            ref_len += r;
            tot_len += t;
            if t > 0.0 && r / t < worst_net.0 {
                worst_net = (r / t, (*net).clone());
            }
        }
        if tot_len > 0.0 {
            println!(
                "plane-discipline: {} plane layers; SI nets {:.1}% plane-referenced, {} vias across {} nets, worst {:.0}% ({})",
                plane_pos.len(),
                100.0 * ref_len / tot_len,
                vias,
                si_nets.len(),
                100.0 * worst_net.0,
                worst_net.1
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
