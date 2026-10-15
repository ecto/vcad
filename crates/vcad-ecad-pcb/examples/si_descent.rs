//! Differentiable pair polish on a routed board (GPU-router charter M5→M6).
//!
//! For each classified differential pair whose legs form single-layer
//! unbranched polylines, run the tang-expr descent (skew² + gap springs +
//! clearance hinges) and commit the optimized geometry ONLY when the exact
//! oracle passes every final segment — fail-closed, per the charter.
//!
//! The implementation lives in `router::descend_board`; this is a driver, so
//! the router's own SI finishing pass and this example cannot drift apart.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example si_descent -- in.pcb.json out.pcb.json [iters] [--adam]
//! ```

use vcad_ecad_pcb::router::descent::{descend_board_with, DescentSolver};
use vcad_ir::ecad::Pcb;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    // `--adam` swaps the SCO default for the original Adam loop (comparison).
    let all: Vec<String> = std::env::args().skip(1).collect();
    let solver = if all.iter().any(|a| a == "--adam") {
        DescentSolver::Adam
    } else {
        DescentSolver::Sco
    };
    let mut args = all.into_iter().filter(|a| a != "--adam");
    let input = args.next().expect("usage: si_descent <in> <out> [iters]");
    let output = args.next().expect("usage: si_descent <in> <out> [iters]");
    let iters: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(2000);

    let mut pcb: Pcb =
        serde_json::from_str(&std::fs::read_to_string(&input).expect("read")).expect("parse");
    let t0 = std::time::Instant::now();
    let r = descend_board_with(&mut pcb, iters, solver);
    eprintln!("solver {solver:?}: {:.2} s", t0.elapsed().as_secs_f64());
    println!(
        "si-descent: tuned {}/{} pairs ({} rejected, {} cuts mined)",
        r.tuned, r.attempted, r.rejected, r.cuts
    );
    std::fs::write(&output, serde_json::to_string(&pcb).expect("serialize")).expect("write");
    eprintln!("wrote {output}");
}
