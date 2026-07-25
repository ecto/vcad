//! M2 oracle: the Polyakov-pair correlator ⟨ℓ(0)ℓ̄(R)⟩ — the free
//! energy of a static quark–antiquark pair — must decay with
//! separation in the confined phase: F(R) rises, so
//! ⟨ℓℓ̄⟩(1) > ⟨ℓℓ̄⟩(2) > 0. This is the confinement signal the flux-tube
//! viewport demo is built on, asserted at the correlator level where
//! the statistics are solid; the 3D excess profile itself is checked
//! for finiteness and shape (its pointwise signal needs ensembles far
//! beyond CI scale, which is honest to say rather than to fake).

use vcad_kernel_qcd::spec::{run, FluxTubeSpec, Gauge, SimSpec};

fn spec(separation: usize, seed: u64) -> SimSpec {
    SimSpec {
        gauge: Gauge::Su2,
        dims: [6, 6, 6, 4],
        beta: 2.2,
        thermalization_sweeps: 50,
        measurement_sweeps: 120,
        overrelax_per_heatbath: 1,
        bin_size: 12,
        max_wilson_extent: 0,
        seed,
        hot_start: false,
        smear: None,
        measure_temporal_loops: false,
        measure_polyakov: false,
        flux_tube: Some(FluxTubeSpec { separation }),
        snapshot_cooling: None,
    }
}

#[test]
fn pair_correlator_decays_with_separation() {
    let r1 = run(&spec(1, 501)).unwrap();
    let r2 = run(&spec(2, 502)).unwrap();
    let c1 = r1.flux_tube.unwrap().pair_correlator;
    let c2 = r2.flux_tube.unwrap().pair_correlator;
    assert!(
        c1.mean > 0.0,
        "confined pair correlator at R=1 must be positive: {} +- {}",
        c1.mean,
        c1.err
    );
    assert!(
        c1.mean > c2.mean,
        "static-pair free energy must rise with R: C(1)={} !> C(2)={}",
        c1.mean,
        c2.mean
    );
}

#[test]
fn excess_profile_is_finite_and_shaped() {
    let r = run(&spec(2, 503)).unwrap();
    let ft = r.flux_tube.unwrap();
    assert_eq!(ft.spatial_dims, [6, 6, 6]);
    assert_eq!(ft.excess_mean.len(), 216);
    assert_eq!(ft.excess_err.len(), 216);
    assert!(ft.excess_mean.iter().all(|v| v.is_finite()));
    assert!(ft.excess_err.iter().all(|v| v.is_finite() && *v >= 0.0));
}
