//! M3 oracles: SU(3) coupling expansions and the deconfinement
//! transition in both gauge groups.
//!
//! SU(3) plaquette series: strong coupling ⟨P⟩ = β/18 + O(β²); weak
//! coupling ⟨P⟩ = 1 − 2/β + O(1/β²) (the SU(N) result (N²−1)/(4β)
//! at N = 3).
//!
//! Deconfinement: at finite temperature (N_t = 2) the Polyakov loop
//! magnitude ⟨|L|⟩ is small in the confined phase and O(1) in the
//! deconfined phase. Known critical couplings: SU(2) N_t = 2 at
//! β_c ≈ 1.88; SU(3) N_t = 2 at β_c ≈ 5.1. We bracket each transition
//! from well inside each phase rather than resolving β_c itself —
//! that scan is physics to do *with* the tool, not a CI gate.

use vcad_kernel_qcd::spec::{run, Gauge, SimSpec};

fn spec(gauge: Gauge, dims: [usize; 4], beta: f64, seed: u64) -> SimSpec {
    SimSpec {
        gauge,
        dims,
        beta,
        thermalization_sweeps: 50,
        measurement_sweeps: 100,
        overrelax_per_heatbath: 1,
        bin_size: 10,
        max_wilson_extent: 0,
        seed,
        hot_start: false,
        smear: None,
        measure_temporal_loops: false,
        measure_polyakov: true,
        flux_tube: None,
        snapshot_cooling: None,
    }
}

#[test]
fn su3_strong_coupling_plaquette() {
    let beta = 1.0;
    let r = run(&spec(Gauge::Su3, [5, 5, 5, 5], beta, 401)).unwrap();
    let series = beta / 18.0;
    let tol = 0.012 + 5.0 * r.plaquette.err; // O(β²) truncation + MC
    assert!(
        (r.plaquette.mean - series).abs() < tol,
        "SU(3) beta={beta}: <P>={} +- {} vs series {series}",
        r.plaquette.mean,
        r.plaquette.err
    );
}

#[test]
fn su3_weak_coupling_plaquette() {
    let beta = 12.0;
    let r = run(&spec(Gauge::Su3, [5, 5, 5, 5], beta, 402)).unwrap();
    let series = 1.0 - 2.0 / beta;
    let tol = 0.02 + 5.0 * r.plaquette.err; // O(1/β²) truncation + MC
    assert!(
        (r.plaquette.mean - series).abs() < tol,
        "SU(3) beta={beta}: <P>={} +- {} vs series {series}",
        r.plaquette.mean,
        r.plaquette.err
    );
}

#[test]
fn su2_deconfinement_transition_brackets() {
    // N_t = 2, β_c ≈ 1.88: β = 1.3 confined, β = 2.5 deconfined.
    let conf = run(&spec(Gauge::Su2, [6, 6, 6, 2], 1.3, 403)).unwrap();
    let deconf = run(&spec(Gauge::Su2, [6, 6, 6, 2], 2.5, 404)).unwrap();
    let lc = conf.polyakov_abs.unwrap();
    let ld = deconf.polyakov_abs.unwrap();
    assert!(
        lc.mean < 0.25,
        "confined <|L|> should be small: {} +- {}",
        lc.mean,
        lc.err
    );
    assert!(
        ld.mean > 0.5,
        "deconfined <|L|> should be O(1): {} +- {}",
        ld.mean,
        ld.err
    );
    assert!(ld.mean > 3.0 * lc.mean, "no transition visible");
}

#[test]
fn su3_deconfinement_transition_brackets() {
    // N_t = 2, β_c ≈ 5.1: β = 4.0 confined, β = 6.5 deconfined.
    let conf = run(&spec(Gauge::Su3, [4, 4, 4, 2], 4.0, 405)).unwrap();
    let deconf = run(&spec(Gauge::Su3, [4, 4, 4, 2], 6.5, 406)).unwrap();
    let lc = conf.polyakov_abs.unwrap();
    let ld = deconf.polyakov_abs.unwrap();
    assert!(
        lc.mean < 0.25,
        "confined <|L|> should be small: {} +- {}",
        lc.mean,
        lc.err
    );
    assert!(
        ld.mean > 0.4,
        "deconfined <|L|> should be O(1): {} +- {}",
        ld.mean,
        ld.err
    );
    assert!(ld.mean > 3.0 * lc.mean, "no transition visible");
}
