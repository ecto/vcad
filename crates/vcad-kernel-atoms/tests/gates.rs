//! Milestone acceptance gates. Each test *is* the gate for a milestone claim.

use vcad_kernel_atoms::inverse::{optimize, DesignProblem, InverseOptions};
use vcad_kernel_atoms::potential::{min_image, Coulomb, HarmonicAngles, HarmonicBonds};
use vcad_kernel_atoms::{
    builder, fd, inspect, io, AtomSystem, Integrator, LennardJones, MinimizeOptions, SimReceipt,
    Species, Sum, Thermostat,
};

const WATER_XYZ: &str = "3\nwater molecule\nO 0.000000 0.000000 0.000000\nH 0.757000 0.586000 0.000000\nH -0.757000 0.586000 0.000000\n";

// ---------------------------------------------------------------------------
// M0 — structure I/O + inspect
// ---------------------------------------------------------------------------

#[test]
fn m0_xyz_parse_formula_bonds_rg() {
    let mol = io::parse_xyz(WATER_XYZ).expect("parse water");
    assert_eq!(mol.len(), 3);
    let rep = inspect::report(&mol);
    assert_eq!(rep.formula, "H2O");
    assert_eq!(rep.atom_count, 3);
    // O-H, O-H bonds perceived; H-H not.
    assert_eq!(rep.bond_count, 2, "expected 2 O-H bonds");
    assert!(rep.radius_of_gyration > 0.0 && rep.radius_of_gyration < 1.0);
    assert!(rep.mass_amu > 17.0 && rep.mass_amu < 19.0);
}

#[test]
fn m0_xyz_roundtrip_through_document() {
    let mol = io::parse_xyz(WATER_XYZ).expect("parse");
    // Round-trip through the IR Document as the molecule domain field.
    let mut doc = vcad_ir::Document::new();
    doc.molecule = Some(mol.clone());
    let json = doc.to_json().expect("serialize");
    let back = vcad_ir::Document::from_json(&json).expect("deserialize");
    assert_eq!(back.molecule.as_ref().unwrap(), &mol);
}

#[test]
fn m0_extended_xyz_lattice() {
    let text = "1\nLattice=\"4.0 0 0 0 4.0 0 0 0 4.0\"\nAr 0 0 0\n";
    let mol = io::parse_xyz(text).expect("parse");
    let cell = mol.cell.expect("cell parsed");
    assert_eq!(cell.a, [4.0, 0.0, 0.0]);
    assert_eq!(cell.c, [0.0, 0.0, 4.0]);
}

// ---------------------------------------------------------------------------
// M2 — force fields validated against FD; dynamics; minimization
// ---------------------------------------------------------------------------

fn argon_cluster() -> AtomSystem {
    // A few argon atoms in a generic (non-symmetric) arrangement so forces are
    // nonzero along every axis.
    let mol = builder::fcc("Ar", 5.26, 1, 1, 1);
    let mut sys = AtomSystem::from_ir(&mol).unwrap();
    sys.cell = None; // treat as a cluster for the FD check
                     // Perturb slightly to break symmetry.
    for (i, p) in sys.positions.iter_mut().enumerate() {
        p[0] += 0.13 * (i as f64 + 1.0).sin();
        p[1] += 0.11 * (i as f64 + 2.0).cos();
        p[2] += 0.09 * (i as f64).sin();
    }
    sys
}

#[test]
fn m2_lennard_jones_matches_fd() {
    let sys = argon_cluster();
    let lj = LennardJones::monatomic(0.0103, 3.4, 8.0);
    let rep = fd::check_forces(&lj, &sys, 1e-6);
    assert!(
        rep.max_abs_error < 1e-6,
        "LJ FD mismatch {:?} at {:?}",
        rep.max_abs_error,
        rep.worst
    );
}

#[test]
fn m2_harmonic_bonds_match_fd() {
    let mol = builder::diatomic("O", 1.4); // stretched from r0
    let sys = AtomSystem::from_ir(&mol).unwrap();
    let bonds = HarmonicBonds::uniform(30.0, 1.2);
    let rep = fd::check_forces(&bonds, &sys, 1e-6);
    assert!(rep.max_abs_error < 1e-6, "bond FD mismatch {rep:?}");
}

#[test]
fn m2_harmonic_angles_match_fd() {
    // Water-like triple, apex atom index 0.
    let mut sys = AtomSystem::from_ir(&io::parse_xyz(WATER_XYZ).unwrap()).unwrap();
    // Move off the equilibrium angle a bit.
    sys.positions[1][2] += 0.2;
    let angles = HarmonicAngles {
        triples: vec![(1, 0, 2)],
        k: 3.0,
        theta0: 1.4, // deliberately far from the actual angle for a strong signal
        // phyz-md grew per-triple (k, theta0) overrides; empty keeps the
        // uniform pair above, which is what this FD gate exercises.
        per_angle: Vec::new(),
    };
    let rep = fd::check_forces(&angles, &sys, 1e-6);
    assert!(rep.max_abs_error < 1e-5, "angle FD mismatch {rep:?}");
}

#[test]
fn m2_coulomb_matches_fd() {
    let mol = builder::diatomic("Na", 3.0);
    let mut sys = AtomSystem::from_ir(&mol).unwrap();
    sys.charges = vec![1.0, -1.0];
    // break axis-alignment
    sys.positions[1] = [2.4, 1.1, 0.7];
    let coul = Coulomb { cutoff: 20.0 };
    let rep = fd::check_forces(&coul, &sys, 1e-6);
    assert!(rep.max_abs_error < 1e-6, "coulomb FD mismatch {rep:?}");
}

#[test]
fn m2_nve_energy_conservation() {
    let mol = builder::fcc("Ar", 5.26, 2, 2, 2); // 32 atoms, periodic
    let mut sys = AtomSystem::from_ir(&mol).unwrap();
    // Seed deterministic small velocities (Å/fs).
    for (i, v) in sys.velocities.iter_mut().enumerate() {
        v[0] = 1e-3 * (i as f64 + 1.0).sin();
        v[1] = 1e-3 * (i as f64 + 1.0).cos();
        v[2] = 1e-3 * (2.0 * i as f64 + 1.0).sin();
    }
    let lj = LennardJones::monatomic(0.0103, 3.4, 8.0);
    let mut integ = Integrator::new(&lj, &sys, 2.0); // 2 fs
    let e0 = integ.total_energy(&sys);
    let mut max_dev = 0.0_f64;
    for _ in 0..2000 {
        integ.step(&mut sys);
        let e = integ.total_energy(&sys);
        max_dev = max_dev.max((e - e0).abs());
    }
    // Energy scale of the system.
    let scale = e0.abs().max(1e-3);
    assert!(
        max_dev / scale < 1e-3,
        "NVE energy drift too large: {max_dev} (scale {scale})"
    );
}

#[test]
fn m2_minimize_finds_lj_minimum() {
    // Two argon atoms; LJ minimum at r = 2^(1/6) sigma.
    let mol = builder::diatomic("Ar", 4.6);
    let mut sys = AtomSystem::from_ir(&mol).unwrap();
    let lj = LennardJones::monatomic(0.0103, 3.4, 12.0);
    let res = vcad_kernel_atoms::minimize(&lj, &mut sys, &MinimizeOptions::default());
    assert!(res.converged, "minimize did not converge: {res:?}");
    let r = {
        use vcad_kernel_atoms::vec3;
        vec3::norm(vec3::sub(sys.positions[0], sys.positions[1]))
    };
    let expected = 2f64.powf(1.0 / 6.0) * 3.4;
    assert!(
        (r - expected).abs() < 1e-2,
        "minimized distance {r}, expected {expected}"
    );
}

#[test]
fn m2_thermostat_drives_temperature() {
    let mol = builder::fcc("Ar", 5.26, 2, 2, 2);
    let mut sys = AtomSystem::from_ir(&mol).unwrap();
    for (i, v) in sys.velocities.iter_mut().enumerate() {
        v[0] = 2e-3 * (i as f64 + 1.0).sin();
        v[1] = 2e-3 * (i as f64 + 1.0).cos();
    }
    let lj = LennardJones::monatomic(0.0103, 3.4, 8.0);
    let mut integ = Integrator::new(&lj, &sys, 2.0).with_thermostat(Thermostat {
        target_k: 50.0,
        tau_fs: 20.0,
    });
    for _ in 0..3000 {
        integ.step(&mut sys);
    }
    let t = sys.temperature();
    assert!(
        (t - 50.0).abs() < 15.0,
        "thermostat failed to reach ~50 K, got {t}"
    );
}

// ---------------------------------------------------------------------------
// M3 — MLIP adapter (stub backend) runs through the same contract
// ---------------------------------------------------------------------------

#[test]
fn m3_mlip_stub_matches_fd_and_minimizes() {
    use vcad_kernel_atoms::mlip::{MlipPotential, PairwiseStubBackend};
    let sys = argon_cluster();
    let pot = MlipPotential::new(PairwiseStubBackend::default());
    let rep = fd::check_forces(&pot, &sys, 1e-6);
    assert!(rep.max_abs_error < 1e-5, "MLIP-stub FD mismatch {rep:?}");

    // And it drives a minimization (graph rebuilt each step).
    let mut s2 = sys.clone();
    let res = vcad_kernel_atoms::minimize(&pot, &mut s2, &MinimizeOptions::default());
    assert!(
        res.max_force <= 1e-3 || res.converged,
        "MLIP minimize {res:?}"
    );
}

// ---------------------------------------------------------------------------
// M4 — inverse design: drive a lattice constant to a target NN distance
// ---------------------------------------------------------------------------

fn nearest_neighbor_distance(mol: &vcad_kernel_atoms::MoleculeSystem) -> f64 {
    use vcad_kernel_atoms::vec3;
    let mut min = f64::INFINITY;
    for i in 0..mol.len() {
        for j in (i + 1)..mol.len() {
            let d = min_image(vec3::sub(mol.positions[i], mol.positions[j]), &mol.cell);
            min = min.min(vec3::norm(d));
        }
    }
    min
}

#[test]
fn m4_inverse_design_hits_target() {
    // FCC nearest-neighbor distance is a/sqrt(2). Target 2.7 Å -> a = 3.818 Å.
    let target = 2.7;
    let problem = DesignProblem {
        build: Box::new(|theta: &[f64]| builder::fcc("Ar", theta[0], 1, 1, 1)),
        property: Box::new(nearest_neighbor_distance),
        target,
    };
    let opts = InverseOptions {
        bounds: Some(vec![[2.0, 8.0]]),
        step0: 5.0,
        ..Default::default()
    };
    let res = optimize(&problem, &[5.5], &opts);
    let expected_a = target * 2f64.sqrt();
    assert!(
        (res.theta[0] - expected_a).abs() < 1e-2,
        "inverse design converged to a={}, expected {}",
        res.theta[0],
        expected_a
    );
    assert!((res.property - target).abs() < 1e-3);
}

#[test]
fn m4_gradient_matches_analytic() {
    // Closed-form property p(a) = a/sqrt(2); objective 0.5 (p - target)^2.
    // Analytic dObj/da = (a/sqrt2 - target) * (1/sqrt2).
    let target = 2.7;
    let problem = DesignProblem {
        build: Box::new(|theta: &[f64]| {
            // Two-atom system at distance theta/sqrt(2) so
            // nearest_neighbor_distance == theta/sqrt(2).
            builder::diatomic("Ar", theta[0] / 2f64.sqrt())
        }),
        property: Box::new(nearest_neighbor_distance),
        target,
    };
    let a = 5.5;
    let g_fd = problem.grad_fd(&[a], 1e-5);
    let analytic = (a / 2f64.sqrt() - target) * (1.0 / 2f64.sqrt());
    assert!(
        (g_fd[0] - analytic).abs() < 1e-4,
        "grad {} vs analytic {}",
        g_fd[0],
        analytic
    );
}

// ---------------------------------------------------------------------------
// M6 — reproducibility receipts
// ---------------------------------------------------------------------------

#[test]
fn m6_receipt_reproducible_and_tamper_evident() {
    let mol = builder::diatomic("Ar", 4.6);
    let run = |m: &vcad_kernel_atoms::MoleculeSystem| -> SimReceipt {
        let mut sys = AtomSystem::from_ir(m).unwrap();
        let lj = LennardJones::monatomic(0.0103, 3.4, 12.0);
        let res = vcad_kernel_atoms::minimize(&lj, &mut sys, &MinimizeOptions::default());
        SimReceipt::build(
            m,
            "LJ(argon)",
            "minimize",
            serde_json::json!({"force_tol": 1e-4}),
            vec![
                ("energy".into(), res.energy),
                ("max_force".into(), res.max_force),
            ],
        )
    };
    let r1 = run(&mol);
    let r2 = run(&mol);
    assert!(r1.verify_self());
    assert!(r2.verify_against(&r1), "identical runs must reproduce");

    // Tampering with an output breaks the digest.
    let mut tampered = r1.clone();
    tampered.outputs[0].1 += 1.0;
    assert!(!tampered.verify_self(), "tamper must be detected");
}

// Species is re-exported; touch it so the import is meaningful.
#[allow(dead_code)]
fn _species_smoke() -> Species {
    Species {
        element: "C".into(),
        atomic_number: 6,
        mass: 12.011,
        charge: 0.0,
        label: None,
        radius: None,
        color: None,
    }
}

// Sum is re-exported and used to combine terms.
#[test]
fn m2_sum_combines_terms() {
    let mol = builder::diatomic("O", 1.4);
    let sys = AtomSystem::from_ir(&mol).unwrap();
    let sum = Sum::new(vec![
        Box::new(HarmonicBonds::uniform(30.0, 1.2)),
        Box::new(LennardJones::monatomic(0.005, 3.0, 8.0)),
    ]);
    let rep = fd::check_forces(&sum, &sys, 1e-6);
    assert!(rep.max_abs_error < 1e-5, "sum FD mismatch {rep:?}");
}
