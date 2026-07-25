//! Pair bail census: run the coupled-pair construction stage on a board with
//! all copper stripped and histogram why each pair did or did not couple.
//!
//! The full CM5 route takes hours; the pair stage sees a near-empty board in
//! round 0 regardless, so censusing it standalone measures the same geometry
//! in seconds. Kill the dominant bail mode, re-census, repeat.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example si_census -- board.kicad_pcb [expansions]
//! ```

use vcad_ecad_pcb::router::census_pairs;
use vcad_ecad_symbols::parse_kicad_pcb;
use vcad_ir::ecad::Pcb;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .format_timestamp_millis()
        .init();
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: si_census <board> [expansions]");
    let expansions: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(400_000);

    let text = std::fs::read_to_string(&path).expect("read board");
    let pcb: Pcb = if path.ends_with(".json") {
        serde_json::from_str(&text).expect("parse pcb json")
    } else {
        parse_kicad_pcb(&text).expect("parse kicad_pcb")
    };

    let t0 = std::time::Instant::now();
    let census = census_pairs(&pcb, expansions);
    let elapsed = t0.elapsed();

    let total = census.rows.len();
    println!(
        "pair census: {total} pairs, {} coupled, {} coupled >=0.5 fraction ({:.1}s)",
        census.coupled(),
        census.coupled_above(0.5),
        elapsed.as_secs_f64()
    );
    println!("\nbail histogram:");
    for (bail, n) in census.histogram() {
        println!("  {n:3}  {}", bail.slug());
    }

    // Successes with a WEAK coupled fraction are the silent failure mode: the
    // pair constructed, so the stage counts it, but the receipt claim does not.
    let mut weak: Vec<&vcad_ecad_pcb::router::PairCensusRow> = census
        .rows
        .iter()
        .filter(|r| r.bail.is_none() && r.coupled_fraction < 0.5)
        .collect();
    weak.sort_by(|a, b| a.coupled_fraction.partial_cmp(&b.coupled_fraction).unwrap());
    if !weak.is_empty() {
        println!("\ncoupled but BELOW 0.5 fraction ({} pairs):", weak.len());
        for r in weak.iter().take(20) {
            println!(
                "  {:.3}  span {:6.2}mm  {}",
                r.coupled_fraction, r.span_mm, r.net_p
            );
        }
    }

    println!("\nbailed pairs (span, reason):");
    let mut bailed: Vec<&vcad_ecad_pcb::router::PairCensusRow> =
        census.rows.iter().filter(|r| r.bail.is_some()).collect();
    bailed.sort_by(|a, b| a.span_mm.partial_cmp(&b.span_mm).unwrap());
    for r in bailed.iter().take(60) {
        println!(
            "  {:6.2}mm  {:26}  {}",
            r.span_mm,
            r.bail.unwrap().slug(),
            r.net_p
        );
    }
}
