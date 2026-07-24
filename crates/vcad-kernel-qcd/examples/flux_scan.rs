//! Static-pair free energy vs separation: Polyakov-pair correlators
//! C(R) on 8³×4 SU(2) at β = 2.2. F(R) = −ln C(R) / N_t rises
//! linearly with R in the confined phase — confinement as a scan.
//!
//! ```bash
//! cargo run --release -p vcad-kernel-qcd --example flux_scan > flux_scan.json
//! ```

use vcad_kernel_qcd::spec::{run, FluxTubeSpec, Gauge, SimSpec};

fn main() {
    let mut rows = Vec::new();
    for sep in 1..=4usize {
        eprintln!("separation {sep}…");
        let spec = SimSpec {
            gauge: Gauge::Su2,
            dims: [8, 8, 8, 4],
            beta: 2.2,
            thermalization_sweeps: 80,
            measurement_sweeps: 400,
            overrelax_per_heatbath: 2,
            bin_size: 40,
            max_wilson_extent: 0,
            seed: 1000 + sep as u64,
            hot_start: false,
            smear: None,
            measure_temporal_loops: false,
            measure_polyakov: false,
            flux_tube: Some(FluxTubeSpec { separation: sep }),
            snapshot_cooling: None,
        };
        let r = run(&spec).expect("run");
        let c = r.flux_tube.unwrap().pair_correlator;
        rows.push(serde_json::json!({
            "separation": sep, "correlator": c.mean, "err": c.err,
        }));
    }
    println!(
        "{}",
        serde_json::json!({ "beta": 2.2, "nt": 4, "rows": rows })
    );
}
