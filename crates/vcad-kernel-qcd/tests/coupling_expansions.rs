//! Validation against the known SU(2) 4D coupling expansions — the
//! crate's fail-closed oracles.
//!
//! Strong coupling (small β): ⟨P⟩ = β/4 − β³/96 + O(β⁵).
//! Weak coupling (large β):   ⟨P⟩ = 1 − 3/(4β) + O(1/β²).
//!
//! Tolerances are set from the truncation error of each series plus a
//! generous multiple of the Monte Carlo error, so these tests are
//! deterministic (fixed seed) and stable.

use vcad_kernel_qcd::spec::{run, SimSpec};

fn spec(beta: f64, seed: u64) -> SimSpec {
    SimSpec {
        dims: [6, 6, 6, 6],
        beta,
        thermalization_sweeps: 50,
        measurement_sweeps: 100,
        overrelax_per_heatbath: 2,
        bin_size: 10,
        max_wilson_extent: 0,
        seed,
        hot_start: false,
    }
}

#[test]
fn strong_coupling_plaquette() {
    let beta = 0.75;
    let r = run(&spec(beta, 101)).unwrap();
    let series = beta / 4.0 - beta * beta * beta / 96.0;
    let tol = 0.006 + 5.0 * r.plaquette.err; // O(β⁵) truncation + MC
    assert!(
        (r.plaquette.mean - series).abs() < tol,
        "beta={beta}: <P>={} +- {} vs series {series}",
        r.plaquette.mean,
        r.plaquette.err
    );
}

#[test]
fn weak_coupling_plaquette() {
    let beta = 8.0;
    let r = run(&spec(beta, 102)).unwrap();
    let series = 1.0 - 3.0 / (4.0 * beta);
    let tol = 0.01 + 5.0 * r.plaquette.err; // O(1/β²) truncation + MC
    assert!(
        (r.plaquette.mean - series).abs() < tol,
        "beta={beta}: <P>={} +- {} vs series {series}",
        r.plaquette.mean,
        r.plaquette.err
    );
}

#[test]
fn plaquette_is_monotone_in_beta() {
    // ⟨P⟩(β) is strictly increasing — a cheap global sanity sweep.
    let mut last = -1.0;
    for (i, beta) in [0.5, 1.5, 2.5, 4.0].iter().enumerate() {
        let mut s = spec(*beta, 200 + i as u64);
        s.dims = [4, 4, 4, 4];
        s.thermalization_sweeps = 40;
        s.measurement_sweeps = 60;
        s.bin_size = 6;
        let r = run(&s).unwrap();
        assert!(
            r.plaquette.mean > last + 0.02,
            "<P>({beta}) = {} not above previous {last}",
            r.plaquette.mean
        );
        last = r.plaquette.mean;
    }
}
