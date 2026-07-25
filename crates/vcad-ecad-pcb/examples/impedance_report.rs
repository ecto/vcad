//! Per-layer controlled-impedance geometry vs. what a human actually routed.
//!
//! Imports a routed KiCad board, resolves the differential geometry the model
//! derives on every copper layer for a given target impedance, and puts it next
//! to the widths the board's own diff-pair copper uses on that layer. The point
//! is falsifiability: if the model is physical rather than merely plausible,
//! the derived width should land on the human's width where the human is
//! actually running controlled-impedance copper.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example impedance_report -- \
//!     .scratch/CM5RevEng.kicad_pcb [target_zdiff=90]
//! ```

use std::collections::BTreeMap;

use vcad_ecad_pcb::impedance::{
    diff_impedance, diff_pair_geometry_for_layer, layer_em, GeometryBasis, LayerKind,
};
use vcad_ecad_pcb::router::classes::classify_nets;
use vcad_ecad_symbols::parse_kicad_pcb;
use vcad_ir::ecad::{NetClassRules, PcbLayer};

fn seg_len(a: vcad_ir::Vec2, b: vcad_ir::Vec2) -> f64 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| {
        eprintln!("usage: impedance_report <board.kicad_pcb> [target_zdiff]");
        std::process::exit(2);
    });
    let target: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(90.0);

    let text = std::fs::read_to_string(&path).expect("read board");
    let pcb = parse_kicad_pcb(&text).expect("parse board");
    let all_nets: Vec<String> = {
        let mut v: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
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
    let classifier = classify_nets(&all_nets);

    // Every net that is one leg of a detected differential pair.
    let pair_nets: std::collections::HashSet<&String> =
        classifier.pairs.iter().flat_map(|(p, n)| [p, n]).collect();

    // Human evidence: width histogram (by copper length) of pair copper, per
    // layer, plus the modal pair pitch measured between P and N legs.
    let mut human: BTreeMap<PcbLayer, BTreeMap<u64, f64>> = BTreeMap::new();
    for t in &pcb.traces {
        if !pair_nets.contains(&t.net) {
            continue;
        }
        *human
            .entry(t.layer)
            .or_default()
            .entry((t.width * 1000.0).round() as u64)
            .or_default() += seg_len(t.start, t.end);
    }

    // Pair pitch: for each pair, the modal centre-to-centre separation between
    // near-parallel same-layer P/N segments. Gap = pitch − width.
    let pitch = modal_pair_pitch(&pcb, &classifier);
    let gap = (pitch - modal_width(&human)).max(0.05);

    println!("board: {path}");
    println!(
        "pairs detected: {}  |  measured modal pair pitch: {pitch:.3}mm  \
         → gap {gap:.3}mm at the modal width",
        classifier.pairs.len()
    );
    println!("target differential impedance: {target:.0}Ω\n");

    let class = NetClassRules {
        name: "REPORT".into(),
        trace_width: pcb.rules.default_rules.trace_width,
        clearance: pcb.rules.default_rules.clearance,
        via_diameter: pcb.rules.default_rules.via_diameter,
        via_drill: pcb.rules.default_rules.via_drill,
        diff_pair_gap: Some(gap),
        diff_pair_width: pcb.rules.default_rules.diff_pair_width,
        target_impedance: None,
        target_diff_impedance: Some(target),
    };
    let min_w = pcb.rules.default_rules.trace_width.min(0.05);

    println!(
        "{:7} {:10} {:>7} {:>6} {:>10} {:>10} {:>9} {:>9}",
        "layer", "kind", "h(mm)", "t(mm)", "derived_w", "human_w", "human_len", "human_Z"
    );
    for sl in &pcb.stackup.layers {
        let layer = sl.layer;
        let em = layer_em(&pcb.stackup, layer);
        let g = diff_pair_geometry_for_layer(&pcb.stackup, layer, &class, min_w);
        let widths = human.get(&layer);
        let hw = widths.map(modal_of);
        let hlen: f64 = widths.map(|w| w.values().sum()).unwrap_or(0.0);

        let derived = match g.basis {
            GeometryBasis::Derived { achieved } => {
                format!("{:.4} ({achieved:.0}Ω)", g.width)
            }
            GeometryBasis::Declared(why) => format!("— {why:?}"),
        };
        let human_z = match (em, hw) {
            (Some(em), Some(w)) if w > 0.0 => format!("{:.0}Ω", diff_impedance(&em, w, gap)),
            _ => "—".into(),
        };
        println!(
            "{:7} {:10} {:>7} {:>6} {:>10} {:>10} {:>9} {:>9}",
            format!("{layer:?}"),
            em.map(|e| match e.kind {
                LayerKind::Microstrip => "microstrip",
                LayerKind::Stripline => "stripline",
            })
            .unwrap_or("unknown"),
            em.map(|e| format!("{:.3}", e.dielectric_height))
                .unwrap_or_else(|| "—".into()),
            em.map(|e| format!("{:.3}", e.copper_thickness))
                .unwrap_or_else(|| "—".into()),
            derived,
            hw.map(|w| format!("{w:.3}")).unwrap_or_else(|| "—".into()),
            format!("{hlen:.0}mm"),
            human_z,
        );
    }
}

/// Modal width (mm) of the busiest layer's histogram.
fn modal_width(human: &BTreeMap<PcbLayer, BTreeMap<u64, f64>>) -> f64 {
    human
        .values()
        .max_by(|a, b| {
            let (sa, sb): (f64, f64) = (a.values().sum(), b.values().sum());
            sa.total_cmp(&sb)
        })
        .map(modal_of)
        .unwrap_or(0.1)
}

/// Length-weighted modal width (mm) of one histogram.
fn modal_of(w: &BTreeMap<u64, f64>) -> f64 {
    w.iter()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(um, _)| *um as f64 / 1000.0)
        .unwrap_or(0.0)
}

/// Modal centre-to-centre separation between the legs of the board's pairs.
fn modal_pair_pitch(
    pcb: &vcad_ir::ecad::Pcb,
    classifier: &vcad_ecad_pcb::router::classes::NetClassifier,
) -> f64 {
    let mut hist: BTreeMap<u64, usize> = BTreeMap::new();
    for (p, n) in &classifier.pairs {
        let ps: Vec<_> = pcb.traces.iter().filter(|t| &t.net == p).collect();
        let ns: Vec<_> = pcb.traces.iter().filter(|t| &t.net == n).collect();
        for a in &ps {
            let da = (a.end.x - a.start.x, a.end.y - a.start.y);
            let la = (da.0 * da.0 + da.1 * da.1).sqrt();
            if la < 0.5 {
                continue;
            }
            for b in &ns {
                if b.layer != a.layer {
                    continue;
                }
                let db = (b.end.x - b.start.x, b.end.y - b.start.y);
                let lb = (db.0 * db.0 + db.1 * db.1).sqrt();
                if lb < 0.5 {
                    continue;
                }
                // Near-parallel only (|sin θ| < 0.09 ≈ 5°).
                let cross = (da.0 * db.1 - da.1 * db.0).abs() / (la * lb);
                if cross > 0.09 {
                    continue;
                }
                // Perpendicular offset of b's midpoint from a's line.
                let mid = ((b.start.x + b.end.x) / 2.0, (b.start.y + b.end.y) / 2.0);
                let d = ((mid.0 - a.start.x) * da.1 - (mid.1 - a.start.y) * da.0).abs() / la;
                if (0.02..=1.0).contains(&d) {
                    *hist.entry((d * 100.0).round() as u64).or_default() += 1;
                }
            }
        }
    }
    hist.iter()
        .max_by_key(|(_, c)| **c)
        .map(|(b, _)| *b as f64 / 100.0)
        .unwrap_or(0.25)
}
