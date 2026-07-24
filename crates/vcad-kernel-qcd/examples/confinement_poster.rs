//! Generates the "confinement, computed" showcase dataset as JSON on
//! stdout: an SU(2) deconfinement β-scan, the static quark potential
//! with its Cornell fit, a flux-tube profile between a static pair, and
//! a cooled field snapshot with its topological charge. Everything a
//! poster needs, from first principles, in about a minute of laptop.
//!
//! ```bash
//! cargo run --release -p vcad-kernel-qcd --example confinement_poster > poster.json
//! ```

use vcad_kernel_qcd::analysis::{creutz_ratios, fit_cornell, static_potential};
use vcad_kernel_qcd::spec::{run, FluxTubeSpec, Gauge, SimSpec, SmearSpec};

fn base(gauge: Gauge, dims: [usize; 4], beta: f64, seed: u64) -> SimSpec {
    SimSpec {
        gauge,
        dims,
        beta,
        thermalization_sweeps: 80,
        measurement_sweeps: 200,
        overrelax_per_heatbath: 2,
        bin_size: 20,
        max_wilson_extent: 0,
        seed,
        hot_start: false,
        smear: None,
        measure_temporal_loops: false,
        measure_polyakov: false,
        flux_tube: None,
        snapshot_cooling: None,
    }
}

fn main() {
    let mut out = serde_json::Map::new();

    // 1. Deconfinement scan, SU(2), N_t = 2 (β_c ≈ 1.88).
    eprintln!("[1/5] SU(2) deconfinement scan…");
    let mut scan = Vec::new();
    for (i, beta) in [1.2, 1.5, 1.7, 1.8, 1.9, 2.0, 2.1, 2.3, 2.6, 3.0]
        .iter()
        .enumerate()
    {
        let mut s = base(Gauge::Su2, [8, 8, 8, 2], *beta, 900 + i as u64);
        s.measure_polyakov = true;
        let r = run(&s).expect("scan run");
        let l = r.polyakov_abs.unwrap();
        scan.push(serde_json::json!({
            "beta": beta, "polyakov_abs": l.mean, "err": l.err,
            "plaquette": r.plaquette.mean,
        }));
    }
    out.insert("su2_deconfinement_nt2".into(), scan.into());

    // 2. SU(3) deconfinement scan, N_t = 2 (β_c ≈ 5.1).
    eprintln!("[2/5] SU(3) deconfinement scan…");
    let mut scan3 = Vec::new();
    for (i, beta) in [4.0, 4.5, 4.8, 5.0, 5.2, 5.4, 5.8, 6.5].iter().enumerate() {
        let mut s = base(Gauge::Su3, [6, 6, 6, 2], *beta, 950 + i as u64);
        s.thermalization_sweeps = 60;
        s.measurement_sweeps = 120;
        s.bin_size = 12;
        s.measure_polyakov = true;
        let r = run(&s).expect("su3 scan run");
        let l = r.polyakov_abs.unwrap();
        scan3.push(serde_json::json!({
            "beta": beta, "polyakov_abs": l.mean, "err": l.err,
        }));
    }
    out.insert("su3_deconfinement_nt2".into(), scan3.into());

    // 3. Static potential + Cornell fit, SU(2) β = 2.3, smeared.
    eprintln!("[3/5] static potential…");
    let mut s = base(Gauge::Su2, [8, 8, 8, 8], 2.3, 977);
    s.max_wilson_extent = 4;
    s.measure_temporal_loops = true;
    s.smear = Some(SmearSpec {
        alpha: 0.5,
        iterations: 3,
    });
    s.measurement_sweeps = 300;
    s.bin_size = 30;
    let r = run(&s).expect("potential run");
    let pot = static_potential(&r.temporal_loops);
    let fit = fit_cornell(&pot);
    let chis = creutz_ratios(&r.wilson_loops);
    out.insert(
        "static_potential".into(),
        serde_json::json!({
            "beta": 2.3,
            "points": pot,
            "cornell": fit,
            "creutz_ratios": chis,
        }),
    );

    // 4. Flux tube, SU(2) β = 2.2 on 8³×4, pair at separation 3.
    eprintln!("[4/5] flux tube…");
    let mut s = base(Gauge::Su2, [8, 8, 8, 4], 2.2, 978);
    s.measurement_sweeps = 300;
    s.bin_size = 30;
    s.flux_tube = Some(FluxTubeSpec { separation: 3 });
    let r = run(&s).expect("flux run");
    out.insert(
        "flux_tube".into(),
        serde_json::to_value(r.flux_tube.unwrap()).unwrap(),
    );

    // 5. Cooled snapshot + topological charge, SU(2) β = 2.4.
    eprintln!("[5/5] cooled snapshot…");
    let mut s = base(Gauge::Su2, [8, 8, 8, 8], 2.4, 979);
    s.measurement_sweeps = 60;
    s.bin_size = 6;
    s.snapshot_cooling = Some(25);
    let r = run(&s).expect("snapshot run");
    out.insert(
        "cooled_snapshot".into(),
        serde_json::json!({
            "topological_charge": r.topological_charge,
            "snapshot": r.snapshot,
        }),
    );
    // Raw (uncooled) companion for the boiling-vacuum contrast.
    let mut s = base(Gauge::Su2, [8, 8, 8, 8], 2.4, 979);
    s.measurement_sweeps = 60;
    s.bin_size = 6;
    s.snapshot_cooling = Some(0);
    let r = run(&s).expect("raw snapshot run");
    out.insert(
        "raw_snapshot".into(),
        serde_json::json!({ "snapshot": r.snapshot }),
    );

    println!("{}", serde_json::Value::Object(out));
}
