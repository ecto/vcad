//! The convergence contract: five solver domains price one machine and
//! land in one unified receipt, wired (particle's interception feeds
//! thermal's heat load; particle's neutron rate feeds neutronics' source),
//! and the all-predicted receipt rolls up Provisional — never verified.
//!
//! Coarse settings throughout: this guards the composition contract, not
//! the numbers (the `fusor_codesign` example is the full-fidelity run).

use std::collections::{BTreeMap, BTreeSet};

use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::field::FieldMap;
use vcad_kernel_particle::fom::stats;
use vcad_kernel_particle::poisson::{solve, SolveOptions};
use vcad_kernel_particle::receipt as particle_receipt;
use vcad_kernel_particle::trace::{TraceOptions, Tracer, DEUTERON};
use vcad_receipt::{ClaimBasis, ClaimQuantity, DesignReceipt, OracleRef, ReceiptClaim};

fn unified(domain: &str, oracle: &OracleRef, name: &str, value: f64, unit: &str) -> ReceiptClaim {
    ReceiptClaim::pass(format!("{domain}.{name}"), domain, name, oracle.clone())
        .with_basis(ClaimBasis::Predicted)
        .with_measured(ClaimQuantity::new(value, unit))
}

#[test]
fn five_domains_one_receipt_provisional() {
    let mut all: Vec<ReceiptClaim> = Vec::new();

    // particle (coarse)
    let device = Device::shielded_two_ring(120.0, 40.0, 22.0, 4.0, -20_000.0, 40_000.0);
    let sopts = SolveOptions::default();
    let sol = solve(&device, 61, 121, &sopts).unwrap();
    let fields = FieldMap::new(&device, &sol);
    let topts = TraceOptions {
        max_passes: 8,
        ..TraceOptions::default()
    };
    let tracer = Tracer::new(&device, &fields, &sol, topts);
    let pstats = stats(&tracer.launch_ensemble(DEUTERON, 12));
    let op = particle_receipt::OperatingPoint {
        ion_current_a: 0.010,
        d2_pressure_mtorr: 2.0,
        temperature_k: 300.0,
    };
    let pset = particle_receipt::predicted_claims(
        &pstats,
        &sol,
        &topts,
        sopts.tol,
        device.max_potential_drop_v(),
        &op,
    );
    let neutron_rate = pset
        .claims
        .iter()
        .find(|c| c.name == "ddn_neutron_rate")
        .map(|c| c.value)
        .unwrap();
    all.extend(particle_receipt::design_claims(&pset));

    // em (coarse): the two shield coils.
    use vcad_kernel_em::axisym::{Annulus, AxisymMagnetostatics, Coil};
    let region = |z0: f64| Annulus {
        r_inner_mm: 36.0,
        r_outer_mm: 44.0,
        z_min_mm: z0 - 4.0,
        z_max_mm: z0 + 4.0,
    };
    let mut em_dev = AxisymMagnetostatics::new(120.0, -120.0, 120.0);
    em_dev.coils.push(Coil {
        region: region(22.0),
        turns: 100.0,
        current_a: 400.0,
    });
    em_dev.coils.push(Coil {
        region: region(-22.0),
        turns: 100.0,
        current_a: -400.0,
    });
    let em_opts = vcad_kernel_em::grid::SolveOptions::default();
    let em_sol = em_dev.solve(65, 129, &em_opts).unwrap();
    let em_oracle = OracleRef::new("vcad-kernel-em/axisym", "test");
    let l_set = vcad_kernel_em::receipt::axisym_inductance_claims(&em_sol, 0, em_opts.tol, None);
    for c in &l_set.claims {
        all.push(unified("em", &em_oracle, &c.name, c.value, &c.unit));
    }

    // thermal (coarse): interception heat through a post to a cold flange.
    use vcad_kernel_thermal::model::{
        Axis, FixedTemperature, MaterialRegion, PowerSource, Shape, ThermalModel,
    };
    let intercept_power_w = pstats.interception_fraction * 0.010 * 20_000.0;
    assert!(
        intercept_power_w > 0.0,
        "control config must intercept ions for the thermal wire to exist"
    );
    let ring = Shape::Tube {
        axis: Axis::Z,
        center_mm: [50.0, 50.0],
        inner_radius_mm: 36.0,
        outer_radius_mm: 44.0,
        span_mm: [68.0, 76.0],
    };
    let post = Shape::Box {
        min_mm: [47.0, 84.0, 68.0],
        size_mm: [6.0, 6.0, 32.0],
    };
    let mut tm = ThermalModel::new([0.0, 0.0, 0.0], [100.0, 100.0, 100.0], [40, 40, 40]);
    tm.materials
        .push(MaterialRegion::isotropic(ring.clone(), 390.0));
    tm.materials
        .push(MaterialRegion::isotropic(post.clone(), 16.0));
    tm.sources.push(PowerSource {
        name: "ion-interception".into(),
        shape: ring,
        power_w: intercept_power_w,
    });
    tm.fixed.push(FixedTemperature {
        shape: Shape::Box {
            min_mm: [44.0, 81.0, 95.0],
            size_mm: [12.0, 12.0, 5.0],
        },
        temperature_c: 25.0,
    });
    tm.reference_c = Some(25.0);
    let th_opts = vcad_kernel_thermal::solve::SolveOptions::default();
    let th_sol = vcad_kernel_thermal::solve::solve_steady(&tm, &th_opts).unwrap();
    let th_set = vcad_kernel_thermal::receipt::predicted_claims(&tm, &th_sol, &th_opts);
    let th_oracle = OracleRef::new("vcad-kernel-thermal/steady", "test");
    for c in &th_set.claims {
        all.push(unified("thermal", &th_oracle, &c.name, c.value, &c.unit));
    }
    // The cross-domain wire is literal: the deposited power is the
    // particle result, and the solver's own energy balance reports it.
    let total_source_w: f64 = th_sol.sources.iter().map(|s| s.power_w).sum();
    assert!(
        (total_source_w - intercept_power_w).abs() < 1e-9 * intercept_power_w.max(1.0),
        "thermal heat load must equal interception × beam power"
    );

    // neutronics (coarse): particle's rate is the source term.
    use vcad_kernel_neutronics::spec::{
        DetectorSpec, LayerSpec, ParamValue, RunSpec, ShieldSpec, SourceSpec,
    };
    let shield = ShieldSpec {
        layers: vec![
            LayerSpec {
                material: "air".into(),
                thickness_mm: ParamValue::Literal(500.0),
            },
            LayerSpec {
                material: "hdpe".into(),
                thickness_mm: ParamValue::Literal(50.0),
            },
            LayerSpec {
                material: "air".into(),
                thickness_mm: ParamValue::Literal(600.0),
            },
        ],
        source: SourceSpec {
            rate_n_per_s: ParamValue::Literal(neutron_rate.max(1.0)),
            energy_ev: ParamValue::Literal(2.45e6),
        },
        detectors: vec![DetectorSpec {
            label: "operator".into(),
            radius_mm: ParamValue::Literal(1_000.0),
            half_width_mm: ParamValue::Literal(20.0),
        }],
        run: RunSpec {
            histories_per_batch: 2_000,
            batches: 10,
            seed: 7,
        },
    };
    let n_set =
        vcad_kernel_neutronics::receipt::predicted_claims(&shield, &BTreeMap::new()).unwrap();
    let n_oracle = OracleRef::new("vcad-kernel-neutronics/mc", "test");
    let mut saw_finite_rse = false;
    for c in &n_set.claims {
        assert!(c.rse.is_finite(), "every MC claim carries its uncertainty");
        saw_finite_rse = true;
        all.push(unified("neutronics", &n_oracle, &c.name, c.value, &c.unit));
    }
    assert!(saw_finite_rse);

    // tolerance (fast): the ring-gap chain.
    use vcad_kernel_tolerance::analysis::{monte_carlo, rss, worst_case, McOptions};
    use vcad_kernel_tolerance::dist::SigmaConvention;
    use vcad_kernel_tolerance::stackup::{Contributor, Requirement, Stackup};
    let stack = Stackup {
        name: "cusp-ring-gap".into(),
        contributors: vec![
            Contributor::normal("feedthrough", 1.0, 44.0, 0.3, SigmaConvention::ThreeSigma),
            Contributor::normal("spacer", -1.0, 20.0, 0.2, SigmaConvention::ThreeSigma),
            Contributor::normal("seat", 1.0, 20.0, 0.15, SigmaConvention::ThreeSigma),
        ],
        requirement: Requirement::between("ring-gap", 43.0, 45.0),
    };
    let wc = worst_case(&stack).unwrap();
    let rs = rss(&stack).unwrap();
    let mc = monte_carlo(&stack, &McOptions::default()).unwrap();
    let t_set = vcad_kernel_tolerance::receipt::predicted_claims(&stack, &wc, &rs, &mc).unwrap();
    let t_oracle = OracleRef::new("vcad-kernel-tolerance/stackup", "test");
    for c in &t_set.claims {
        all.push(unified("tolerance", &t_oracle, &c.name, c.value, &c.unit));
    }

    // ── the contract ──────────────────────────────────────────────────
    let receipt = DesignReceipt::with_claims(all);
    let domains: BTreeSet<&str> = receipt.claims.iter().map(|c| c.domain.as_str()).collect();
    assert_eq!(
        domains,
        BTreeSet::from(["particle", "em", "thermal", "neutronics", "tolerance"]),
        "one receipt must span all five domains"
    );
    assert!(
        receipt.claims.len() >= 18,
        "expected a substantive claim set, got {}",
        receipt.claims.len()
    );
    for c in &receipt.claims {
        assert_eq!(c.effective_basis(), ClaimBasis::Predicted);
    }
    assert_eq!(
        receipt.verdict(),
        vcad_receipt::ReceiptVerdict::Provisional,
        "an all-predicted multi-domain receipt must roll up Provisional, never Pass"
    );
}
