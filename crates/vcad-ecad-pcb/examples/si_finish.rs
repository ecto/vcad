//! Signal-integrity finishing pass on an already-routed board: re-couple
//! uncoupled pairs, descend residual skew, then meander out what is left.
//!
//! Every stage is non-regressive and oracle-gated, so running this on a board
//! is safe and idempotent-ish — a second run only works whatever the first
//! could not close.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example si_finish -- in.pcb.json out.pcb.json [expansions] [iters]
//! ```

use vcad_ecad_pcb::router::si_finish;
use vcad_ir::ecad::Pcb;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("usage: si_finish <in> <out> [exp] [iters]");
    let output = args.next().expect("usage: si_finish <in> <out> [exp] [iters]");
    let expansions: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000_000);
    let iters: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(2000);

    let mut pcb: Pcb =
        serde_json::from_str(&std::fs::read_to_string(&input).expect("read")).expect("parse");
    let r = si_finish(&mut pcb, expansions, iters);
    println!(
        "si-finish: {}/{} re-coupled, {}/{} descended ({} rejected), {} meandered, {} still over tolerance",
        r.polished,
        r.polish_attempted,
        r.descent.tuned,
        r.descent.attempted,
        r.descent.rejected,
        r.meandered,
        r.over_tolerance,
    );
    std::fs::write(&output, serde_json::to_string(&pcb).expect("serialize")).expect("write");
    eprintln!("wrote {output}");
}
