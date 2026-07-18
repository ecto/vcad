//! Filter yield — the tolerance-yield bridge earns its keep.
//!
//! Take the 10 kHz Butterworth low-pass from `filter_autotune` and answer
//! the question the tuner can't: **with real ±% parts, what fraction of
//! built boards meets spec — and which component must be the expensive
//! tight one?**
//!
//! The pipeline is `circuit::tolerance`: the AC adjoint linearizes
//! |H(f₀)| in (R, L, C), `vcad-kernel-tolerance` rolls up worst-case and
//! RSS, and a seeded Monte Carlo re-runs the actual complex-MNA solver on
//! every sampled circuit so the linearization is checked, not trusted —
//! the discrepancy is printed next to every yield.
//!
//! Run: `cargo run -p vcad-ecad-sim --example filter_yield`

use vcad_ecad_sim::circuit::receipt;
use vcad_ecad_sim::circuit::tolerance::{
    allocate_tolerances, analyze, CircuitOutput, DeviceAllocation, DeviceTolerance, McOptions,
    SpecWindow,
};
use vcad_ecad_sim::circuit::{Circuit, Device};
use vcad_kernel_tolerance::allocate::CostModel;

/// The tuned Butterworth: f₀ = 10 kHz, Q = 1/√2 (see `filter_autotune`).
fn build() -> (Circuit, usize, usize, f64) {
    let f0 = 10_000.0;
    let l = 1e-3;
    let c_val = 1.0 / ((2.0 * std::f64::consts::PI * f0).powi(2) * l);
    let r = std::f64::consts::SQRT_2 * (l / c_val).sqrt();
    let mut ckt = Circuit::new();
    let vin = ckt.node();
    let mid = ckt.node();
    let out = ckt.node();
    let src = ckt.add(Device::VSource {
        p: vin,
        n: 0,
        v: 0.0,
    });
    ckt.add(Device::Resistor { p: vin, n: mid, r });
    ckt.add(Device::Inductor { p: mid, n: out, l });
    ckt.add(Device::Capacitor {
        p: out,
        n: 0,
        c: c_val,
    });
    (ckt, src, out, f0)
}

fn main() {
    let (ckt, src, out, f0) = build();
    let output = CircuitOutput::AcMagnitude {
        source: src,
        out_node: out,
        freq_hz: f0,
    };
    // Spec: |H(f₀)| within ±4% of the Butterworth 1/√2.
    let nominal = std::f64::consts::FRAC_1_SQRT_2;
    let spec = SpecWindow::between(nominal * 0.96, nominal * 1.04);
    let mc = McOptions {
        n: 20_000,
        seed: 0x5EED_C1AC,
    };

    println!("== filter yield: tolerance bridge on the 10 kHz Butterworth ==");
    println!(
        "spec: |H(f0)| in [{:.4}, {:.4}] (nominal {:.4})\n",
        nominal * 0.96,
        nominal * 1.04,
        nominal
    );

    // Table 1: uniform part tolerance vs yield, with the honesty column.
    println!("  tol (all)   WC dev     sigma_lin   sigma_MC    yield_RSS   yield_MC (±SE)      lin err max");
    let mut last = None;
    for tol in [0.01, 0.02, 0.05, 0.10] {
        let tols = [
            DeviceTolerance::three_sigma(1, tol),
            DeviceTolerance::three_sigma(2, tol),
            DeviceTolerance::three_sigma(3, tol),
        ];
        let a = analyze(&ckt, output, &tols, spec, mc).expect("analysis");
        println!(
            "  ±{:>4.1}%     {:.4}     {:.5}     {:.5}     {:8.5}    {:8.5} ±{:.5}   {:.2e}",
            tol * 100.0,
            a.worst_case_deviation,
            a.rss.sigma_gap,
            a.mc.sigma,
            a.rss.yield_estimate,
            a.mc.yield_est.p,
            a.mc.yield_est.standard_error,
            a.mc.lin_err_max,
        );
        last = Some(a);
    }
    let a10 = last.expect("loop ran");

    // Table 2: who dominates? Per-device variance share at ±5%.
    let tols5 = [
        DeviceTolerance::three_sigma(1, 0.05),
        DeviceTolerance::three_sigma(2, 0.05),
        DeviceTolerance::three_sigma(3, 0.05),
    ];
    let a5 = analyze(&ckt, output, &tols5, spec, mc).expect("analysis");
    let sigma_sq = a5.rss.sigma_gap * a5.rss.sigma_gap;
    println!("\n  dominance at ±5% (share of output variance):");
    for c in &a5.stackup.contributors {
        println!(
            "    {:<4}  |dH/dp|·p = {:.4}   share = {:5.1}%",
            c.name,
            (c.coeff * c.nominal).abs(),
            100.0 * c.coeff * c.coeff * c.dist.variance() / sigma_sq
        );
    }

    // Min-cost allocation: hit 99% yield, buying tightness where it's cheap.
    // Fractional cost curves from a caricature price list: resistors are
    // cheap to tighten, inductors expensive, capacitors in between.
    let vars = vec![
        DeviceAllocation {
            device_id: 1,
            cost: CostModel::Reciprocal { a: 0.01, b: 0.0002 },
            tol_frac_min: 0.0005,
            tol_frac_max: 0.10,
        },
        DeviceAllocation {
            device_id: 2,
            cost: CostModel::Reciprocal { a: 0.20, b: 0.004 },
            tol_frac_min: 0.005,
            tol_frac_max: 0.20,
        },
        DeviceAllocation {
            device_id: 3,
            cost: CostModel::Reciprocal { a: 0.05, b: 0.001 },
            tol_frac_min: 0.001,
            tol_frac_max: 0.20,
        },
    ];
    let target = 0.99;
    let start_tols = [
        DeviceTolerance::three_sigma(1, 0.05),
        DeviceTolerance::three_sigma(2, 0.05),
        DeviceTolerance::three_sigma(3, 0.05),
    ];
    let alloc =
        allocate_tolerances(&ckt, output, &start_tols, spec, &vars, target).expect("allocation");

    println!(
        "\n  min-cost allocation for {:.0}% RSS yield (KKT water-filling, kernel-tolerance):",
        target * 100.0
    );
    println!("    device   tolerance   cost      variance share");
    let names = ["R1", "L2", "C3"];
    let mut dominant = 0usize;
    for (i, &(id, frac, cost)) in alloc.allocations.iter().enumerate() {
        if alloc.variance_share[i] > alloc.variance_share[dominant] {
            dominant = i;
        }
        println!(
            "    {:<6}   ±{:5.2}%     ${:.3}    {:5.1}%",
            names[id - 1],
            frac * 100.0,
            cost,
            100.0 * alloc.variance_share[i]
        );
    }
    println!(
        "    total ${:.3} (proportional one-knob baseline ${:.3}); RSS yield {:.4}",
        alloc.cost, alloc.cost_proportional_baseline, alloc.predicted_yield
    );
    let tightest = alloc
        .allocations
        .iter()
        .enumerate()
        .min_by(|a, b| a.1 .1.total_cmp(&b.1 .1))
        .expect("nonempty");
    println!(
        "    → the tight part is {} at ±{:.2}%; the loosest budget goes to the \
         expensive-to-tighten device.",
        names[tightest.1 .0 - 1],
        tightest.1 .1 * 100.0
    );

    // Verify the allocation with the full solver, not the linearization.
    let alloc_tols: Vec<DeviceTolerance> = alloc
        .allocations
        .iter()
        .map(|&(id, frac, _)| DeviceTolerance::three_sigma(id, frac))
        .collect();
    let check = analyze(&ckt, output, &alloc_tols, spec, mc).expect("check");
    println!(
        "    solver-in-the-loop check: MC yield {:.4} ±{:.4} (lin err max {:.2e})",
        check.mc.yield_est.p, check.mc.yield_est.standard_error, check.mc.lin_err_max
    );

    // Receipt: predicted basis, Provisional rollup, seed in the note.
    let set = receipt::yield_claims(&check, ckt.num_nodes, ckt.devices.len());
    let unified = vcad_receipt::DesignReceipt::with_claims(receipt::design_claims(&set));
    println!(
        "\nreceipt: {} claims under {}, rollup = {:?} (predicted basis never rolls up Pass)",
        set.claims.len(),
        receipt::CLAIM_SCHEMA,
        unified.verdict()
    );
    println!("closing instrument: build N boards, sweep |H| with a signal generator + scope.");

    let ok = check.mc.yield_est.p >= target - 5.0 * check.mc.yield_est.standard_error - 0.01
        && a10.mc.yield_est.p < a5.mc.yield_est.p + 1e-9;
    if !ok {
        eprintln!("FAIL: yield numbers inconsistent");
        std::process::exit(1);
    }
    println!("\nPASS: allocation meets the yield target under full-solver Monte Carlo.");
}
