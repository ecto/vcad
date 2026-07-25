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
use vcad_ecad_pcb::router::si_claims::{si_claims, to_receipt_claims, SiBounds};
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

    // Impedance geometry: differential Z of the declared pair class (w, gap)
    // on each copper layer, from the imported physical stackup. Outer layers
    // model as microstrip (reference = first inner plane), inner layers as
    // stripline. This is the task-24 seam: the numbers that decide per-layer
    // width/gap overrides for a 90/100 ohm target.
    {
        use vcad_ecad_sim::impedance::{diff_microstrip_impedance, diff_stripline_impedance};
        let dp_w = pcb.rules.default_rules.diff_pair_width.unwrap_or(0.2);
        let dp_gap = pcb.rules.default_rules.diff_pair_gap.unwrap_or(0.25);
        let coppers: Vec<&vcad_ir::ecad::StackupLayer> = pcb
            .stackup
            .layers
            .iter()
            .filter(|l| l.layer.is_copper())
            .collect();
        let n = coppers.len();
        for (i, sl) in coppers.iter().enumerate() {
            let t = sl.copper_thickness.unwrap_or(0.035);
            let er = sl.dielectric_er.unwrap_or(4.5);
            // Height to the adjacent copper (the reference in a dense stack).
            let h = if i + 1 < n {
                coppers[i + 1]
                    .dielectric_thickness
                    .or(sl.dielectric_thickness)
            } else {
                sl.dielectric_thickness
            }
            .unwrap_or(0.1);
            let outer = i == 0 || i + 1 == n;
            let zdiff = if outer {
                diff_microstrip_impedance(dp_w, dp_gap, t, h, er)
            } else {
                diff_stripline_impedance(dp_w, dp_gap, t, 2.0 * h, er)
            };
            println!(
                "impedance {:?}: pair w={dp_w} gap={dp_gap} -> Zdiff {:.0} ohm ({})",
                sl.layer,
                zdiff,
                if outer { "microstrip" } else { "stripline" }
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

    // Per-pair detail for the two claims that are worst-case minima/maxima
    // over every routed pair: one bad pair breaks the claim, so the report
    // has to name it, not just score it.
    {
        let gap = pcb.rules.default_rules.diff_pair_gap.unwrap_or(0.25);
        let w = pcb
            .rules
            .default_rules
            .diff_pair_width
            .unwrap_or(pcb.rules.default_rules.trace_width);
        let max_sep = (w + gap) * 1.75;
        let mut rows: Vec<(f64, f64, &str)> = Vec::new();
        for (p, n) in &c.pairs {
            let (lp, ln) = (net_routed_length(&pcb, p), net_routed_length(&pcb, n));
            if lp > 0.0 && ln > 0.0 {
                rows.push((
                    vcad_ecad_pcb::router::pair_coupled_fraction(&pcb, p, n, max_sep),
                    (lp - ln).abs(),
                    p.as_str(),
                ));
            }
        }
        rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let weak = rows.iter().filter(|r| r.0 < 0.5).count();
        println!("pairs below 0.5 coupled fraction: {weak} of {}", rows.len());
        for (frac, skew, net) in rows.iter().take(12) {
            println!("  coupled {frac:.3}  skew {skew:6.3} mm  {net}");
        }
        let mut by_skew = rows.clone();
        by_skew.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        println!("worst intra-pair skews:");
        for (frac, skew, net) in by_skew.iter().take(8) {
            println!("  skew {skew:6.3} mm  coupled {frac:.3}  {net}");
        }
    }

    // The claim set: the machine-checkable verdict this whole report exists
    // to feed. Bounds = the human CM5 envelope.
    let set = si_claims(&pcb, &c, &SiBounds::default());
    println!("\nvcad.si-claims/1 (bounds = human CM5 envelope):");
    for cl in &set.claims {
        println!(
            "  {} {}: {:.3} {} (bound {:.3}) — {}",
            if cl.holds { "HOLDS " } else { "BROKEN" },
            cl.name,
            cl.value,
            cl.unit,
            cl.bound,
            cl.note
        );
    }
    println!(
        "verdict: {}",
        if set.all_hold {
            "ALL HOLD"
        } else {
            "NOT ALL HOLD"
        }
    );

    // Optional second arg: write the unified DesignReceipt JSON.
    if let Some(out) = std::env::args().nth(2) {
        let receipt = vcad_receipt::DesignReceipt::with_claims(to_receipt_claims(&set));
        std::fs::write(
            &out,
            serde_json::to_string_pretty(&receipt).expect("serialize"),
        )
        .expect("write receipt");
        eprintln!("wrote {out} (verdict: {:?})", receipt.verdict());
    }
}
