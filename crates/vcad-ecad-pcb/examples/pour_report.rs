//! Inspect copper-pour synthesis on a board without routing it: which nets the
//! policy selects, why, on which layers, and how much copper the outlines cover.
//!
//! The policy half of [`vcad_ecad_pcb::pour_synth`] is deliberately separable
//! from the geometry so it can be argued with; this is the window onto it.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example pour_report -- board.kicad_pcb
//! ```

use std::collections::BTreeMap;

use vcad_ecad_pcb::pour_synth::{decide_pours, synthesize_outlines, PourPolicy, PourReason};
use vcad_ir::ecad::Pcb;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: pour_report <board.kicad_pcb|.brd|.pcb.json>");
        std::process::exit(2);
    });
    let text = std::fs::read_to_string(&path).expect("read board file");
    let pcb: Pcb = if path.ends_with(".json") {
        serde_json::from_str(&text).expect("parse pcb json")
    } else if path.ends_with(".brd") {
        vcad_ecad_symbols::parse_eagle_brd(&text).expect("parse eagle brd")
    } else {
        vcad_ecad_symbols::parse_kicad_pcb(&text).expect("parse kicad_pcb")
    };

    let mut existing: BTreeMap<&str, usize> = BTreeMap::new();
    for z in &pcb.zones {
        if z.layer.is_copper() && !z.net.is_empty() {
            *existing.entry(z.net.as_str()).or_default() += 1;
        }
    }
    println!(
        "board: {} copper layers, {} footprints, {} existing copper zones over {} net(s)",
        pcb.stackup
            .layers
            .iter()
            .filter(|l| l.layer.is_copper())
            .count(),
        pcb.footprints.len(),
        pcb.zones.iter().filter(|z| z.layer.is_copper()).count(),
        existing.len(),
    );
    for (net, n) in &existing {
        println!("  existing: {net} ({n} zone(s))");
    }

    let usable = vcad_ecad_pcb::pour_synth::usable_area(&pcb);
    println!(
        "outline: {} vertices, {} cutouts; usable area {:.0} mm^2 in {} region(s)",
        pcb.outline.vertices.len(),
        pcb.outline.cutouts.len(),
        usable.iter().map(|p| p.area()).sum::<f64>(),
        usable.len(),
    );

    let policy = PourPolicy::default();
    let candidates = decide_pours(&pcb, &policy, &[]);
    println!(
        "\npolicy: {} candidate (net, layer) pour(s)",
        candidates.len()
    );
    for c in &candidates {
        let why = match &c.reason {
            PourReason::DeclaredCurrent {
                current_a,
                required_width_mm,
                routed_width_mm,
            } => format!(
                "declared {current_a:.1} A needs {required_width_mm:.2} mm, \
                 routed at {routed_width_mm:.2} mm"
            ),
            PourReason::ClassWidth {
                class_width_mm,
                default_width_mm,
                implied_current_a,
            } => format!(
                "class width {class_width_mm:.2} mm vs default {default_width_mm:.2} mm \
                 (~{implied_current_a:.1} A)"
            ),
            PourReason::PowerRailName {
                pads,
                traced_current_a,
            } => format!("power-rail name, {pads} pads (a trace carries ~{traced_current_a:.1} A)"),
        };
        println!(
            "  {} on {:?} ({} pad(s) on layer) — {why}",
            c.net, c.layer, c.pads_on_layer
        );
    }

    let zones = synthesize_outlines(&pcb, &candidates, &policy);
    println!("\ngeometry: {} synthesized zone(s)", zones.len());
    let mut by_net: BTreeMap<(&str, String), (usize, f64)> = BTreeMap::new();
    for z in &zones {
        let e = by_net
            .entry((z.net.as_str(), format!("{:?}", z.layer)))
            .or_default();
        e.0 += 1;
        e.1 += ring_area(&z.outline);
    }
    for ((net, layer), (n, area)) in &by_net {
        println!("  {net} on {layer}: {n} piece(s), {area:.0} mm^2");
    }
    let dropped: Vec<&str> = candidates
        .iter()
        .filter(|c| !zones.iter().any(|z| z.net == c.net && z.layer == c.layer))
        .map(|c| c.net.as_str())
        .collect();
    if !dropped.is_empty() {
        println!("  dropped (region could not cover every pad): {dropped:?}");
    }
}

/// Absolute shoelace area of a closed ring.
fn ring_area(ring: &[vcad_ir::Vec2]) -> f64 {
    let n = ring.len();
    let mut s = 0.0;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        s += a.x * b.y - b.x * a.y;
    }
    (0.5 * s).abs()
}
