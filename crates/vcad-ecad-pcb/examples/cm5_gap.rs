//! Ground-truth gap analysis: for every net OUR router left unrouted, show
//! exactly how the HUMAN routed it — layers used, via count and spans,
//! copper length, region — so the next architectural bet is made on
//! evidence instead of intuition.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example cm5_gap -- \
//!     CM5RevEng.kicad_pcb routed.pcb.json
//! ```
//!
//! The first argument is the human-routed reference; the second a board
//! saved by `cm5_bench`'s `out.pcb.json` (our routing applied).

use std::collections::{BTreeMap, BTreeSet};

use vcad_ecad_symbols::parse_kicad_pcb;
use vcad_ir::ecad::{Pcb, PcbLayer};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(human_path), Some(ours_path)) = (args.next(), args.next()) else {
        eprintln!("usage: cm5_gap <human.kicad_pcb> <ours.pcb.json>");
        std::process::exit(2);
    };
    let human = parse_kicad_pcb(&std::fs::read_to_string(&human_path).expect("read human"))
        .expect("parse human");
    let ours: Pcb = serde_json::from_str(&std::fs::read_to_string(&ours_path).expect("read ours"))
        .expect("parse ours");

    // Nets with ≥2 pads that the human routed but we did not.
    let mut pad_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for p in ours.footprints.iter().flat_map(|f| f.pads.iter()) {
        if let Some(n) = p.net.as_deref() {
            if !n.is_empty() {
                *pad_counts.entry(n).or_default() += 1;
            }
        }
    }
    let ours_routed: BTreeSet<&str> = ours.traces.iter().map(|t| t.net.as_str()).collect();
    let human_routed: BTreeSet<&str> = human.traces.iter().map(|t| t.net.as_str()).collect();
    let stuck: Vec<&str> = pad_counts
        .iter()
        .filter(|(n, c)| **c >= 2 && human_routed.contains(*n) && !ours_routed.contains(*n))
        .map(|(n, _)| *n)
        .collect();

    println!("stuck nets (human routed, we did not): {}\n", stuck.len());

    let mut layer_hist: BTreeMap<PcbLayer, usize> = BTreeMap::new();
    let mut span_hist: BTreeMap<(PcbLayer, PcbLayer), usize> = BTreeMap::new();
    let mut total_human_vias = 0usize;

    for net in &stuck {
        let segs: Vec<_> = human.traces.iter().filter(|t| t.net == *net).collect();
        let vias: Vec<_> = human.vias.iter().filter(|v| v.net == *net).collect();
        let layers: BTreeSet<PcbLayer> = segs.iter().map(|t| t.layer).collect();
        let len: f64 = segs
            .iter()
            .map(|t| ((t.end.x - t.start.x).powi(2) + (t.end.y - t.start.y).powi(2)).sqrt())
            .sum();
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for t in &segs {
            for p in [t.start, t.end] {
                lo[0] = lo[0].min(p.x);
                lo[1] = lo[1].min(p.y);
                hi[0] = hi[0].max(p.x);
                hi[1] = hi[1].max(p.y);
            }
        }
        for l in &layers {
            *layer_hist.entry(*l).or_default() += 1;
        }
        for v in &vias {
            *span_hist.entry((v.start_layer, v.end_layer)).or_default() += 1;
        }
        total_human_vias += vias.len();
        println!(
            "  {net}: {} segs, {:.1}mm, layers {:?}, {} vias{}, bbox {:.0}x{:.0}mm",
            segs.len(),
            len,
            layers,
            vias.len(),
            if vias.is_empty() {
                String::new()
            } else {
                let spans: BTreeSet<String> = vias
                    .iter()
                    .map(|v| format!("{:?}..{:?}", v.start_layer, v.end_layer))
                    .collect();
                format!(" ({})", spans.into_iter().collect::<Vec<_>>().join(", "))
            },
            hi[0] - lo[0],
            hi[1] - lo[1],
        );
    }

    println!("\n== aggregate: layers the human used for stuck nets ==");
    for (l, c) in &layer_hist {
        println!("  {l:?}: {c} nets");
    }
    println!("\n== aggregate: via spans the human used ==");
    for ((a, b), c) in &span_hist {
        println!("  {a:?}..{b:?}: {c} vias");
    }
    println!("\ntotal human vias on stuck nets: {total_human_vias}");

    // How tightly did the human actually space copper? Probe every stuck
    // net's trace segments against the rest of the board with a huge
    // clearance and report the min air gap observed — the real fab limit,
    // vs. the clearance our importer calibrated.
    let session = vcad_ecad_pcb::session::RouteSession::from_pcb(&human);
    let mut min_gap = f64::INFINITY;
    let mut gaps: Vec<f64> = Vec::new();
    for net in &stuck {
        for t in human.traces.iter().filter(|t| t.net == *net) {
            let pr = session.probe(
                &vcad_ecad_pcb::spatial::CopperGeom::Segment {
                    a: t.start,
                    b: t.end,
                    half_w: t.width / 2.0,
                },
                t.layer,
                net,
                10.0,
            );
            if pr.min_clearance.is_finite() {
                gaps.push(pr.min_clearance);
                min_gap = min_gap.min(pr.min_clearance);
            }
        }
    }
    gaps.sort_by(f64::total_cmp);
    let pct = |q: f64| {
        gaps.get((gaps.len() as f64 * q) as usize)
            .copied()
            .unwrap_or(f64::NAN)
    };
    println!(
        "\nhuman spacing on stuck-net copper: min={:.3}mm p5={:.3} p25={:.3} median={:.3} (our calibrated clearance: {:.3}mm)",
        min_gap,
        pct(0.05),
        pct(0.25),
        pct(0.5),
        human.rules.default_rules.clearance,
    );
}
