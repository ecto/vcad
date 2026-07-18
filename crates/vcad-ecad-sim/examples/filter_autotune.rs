//! Filter autotune — the adjoint earns its keep.
//!
//! A series-RLC 2nd-order low-pass starts detuned and is driven by
//! **adjoint gradients** (one transposed complex-MNA solve per frequency —
//! see `circuit::adjoint`) to a 10 kHz Butterworth target (Q = 1/√2).
//! AC response comes from complex MNA (`circuit::ac`), not transient + FFT:
//! it is exact per frequency and ~50 lines, which is why it was preferred.
//!
//! Objective: J(R, L, C) = Σ_k (|H(jω_k)| − |H*(jω_k)|)² over log-spaced
//! probe frequencies, minimized by gradient descent in log-parameter space
//! (components are positive scale quantities) with a backtracking line
//! search and a scale-invariant stopping rule.
//!
//! Run: `cargo run -p vcad-ecad-sim --example filter_autotune`

use vcad_ecad_sim::circuit::adjoint::ac_sensitivities;
use vcad_ecad_sim::circuit::receipt;
use vcad_ecad_sim::circuit::{Circuit, Device};

/// Build the series-RLC low-pass; returns (circuit, source id, output node).
/// Topology: vin —R— mid —L— out —C— gnd, output across the capacitor.
/// |H| depends only on ω₀ = 1/√(LC) and Q = (1/R)√(L/C).
fn build(r: f64, l: f64, c: f64) -> (Circuit, usize, usize) {
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
    ckt.add(Device::Capacitor { p: out, n: 0, c });
    (ckt, src, out)
}

/// Analytic 2nd-order low-pass magnitude for the target.
fn target_mag(f: f64, f0: f64, q: f64) -> f64 {
    let x = f / f0;
    1.0 / ((1.0 - x * x).powi(2) + (x / q).powi(2)).sqrt()
}

fn cutoff_and_q(r: f64, l: f64, c: f64) -> (f64, f64) {
    let f0 = 1.0 / (2.0 * std::f64::consts::PI * (l * c).sqrt());
    let q = (1.0 / r) * (l / c).sqrt();
    (f0, q)
}

/// Objective and gradient (d J / d ln p for p = R, L, C) via the AC adjoint.
fn objective_and_grad(params: [f64; 3], probes: &[f64], targets: &[f64]) -> (f64, [f64; 3]) {
    let [r, l, c] = params;
    let (ckt, src, out) = build(r, l, c);
    let mut j = 0.0;
    let mut grad = [0.0f64; 3];
    for (k, &f) in probes.iter().enumerate() {
        let omega = 2.0 * std::f64::consts::PI * f;
        let sens = ac_sensitivities(&ckt, src, omega, out).expect("AC solve");
        let err = sens.h.abs() - targets[k];
        j += err * err;
        // Devices 1, 2, 3 are R, L, C; chain to log-space: d/dlnp = p·d/dp.
        for (gi, dev) in (1..4).enumerate() {
            grad[gi] += 2.0 * err * sens.d_magnitude(dev) * params[gi];
        }
    }
    (j, grad)
}

fn main() {
    // Butterworth target: f_c = 10 kHz, Q = 1/√2.
    let f_target = 10_000.0;
    let q_target = std::f64::consts::FRAC_1_SQRT_2;

    // 25 log-spaced probes, 1–100 kHz.
    let probes: Vec<f64> = (0..25)
        .map(|i| 1_000.0 * 10f64.powf(2.0 * i as f64 / 24.0))
        .collect();
    let targets: Vec<f64> = probes
        .iter()
        .map(|&f| target_mag(f, f_target, q_target))
        .collect();

    // Detuned start: f0 ≈ 15.9 kHz, Q = 0.5.
    let mut p = [200.0f64, 1e-3, 1e-7];
    let (f0_before, q_before) = cutoff_and_q(p[0], p[1], p[2]);

    println!("== filter autotune: adjoint-driven RLC design ==");
    println!(
        "start:  R = {:8.2} Ω, L = {:9.4e} H, C = {:9.4e} F  →  f0 = {:8.1} Hz, Q = {:.4}",
        p[0], p[1], p[2], f0_before, q_before
    );

    // Gradient descent in log-space with backtracking; scale-invariant stop.
    let (mut j, mut grad) = objective_and_grad(p, &probes, &targets);
    let j0 = j;
    let mut step = 0.25f64;
    let mut iters = 0usize;
    for _ in 0..500 {
        iters += 1;
        // Try a step; backtrack until J decreases.
        let mut accepted = false;
        for _ in 0..40 {
            let trial = [
                p[0] * (-step * grad[0] / (1.0 + grad_norm(&grad))).exp(),
                p[1] * (-step * grad[1] / (1.0 + grad_norm(&grad))).exp(),
                p[2] * (-step * grad[2] / (1.0 + grad_norm(&grad))).exp(),
            ];
            let (jt, gt) = objective_and_grad(trial, &probes, &targets);
            if jt < j {
                let improvement = (j - jt) / j.max(1e-300);
                p = trial;
                j = jt;
                grad = gt;
                step *= 1.5;
                accepted = improvement > 1e-12;
                break;
            }
            step *= 0.5;
        }
        if !accepted || j < 1e-16 {
            break;
        }
    }

    let (f0_after, q_after) = cutoff_and_q(p[0], p[1], p[2]);
    println!(
        "tuned:  R = {:8.2} Ω, L = {:9.4e} H, C = {:9.4e} F  →  f0 = {:8.1} Hz, Q = {:.4}",
        p[0], p[1], p[2], f0_after, q_after
    );
    println!(
        "objective: {:.3e} → {:.3e} in {} gradient iterations (each = 1 fwd + 1 adjoint solve per probe)",
        j0, j, iters
    );

    // Bode table, before vs after vs target.
    let before = [200.0f64, 1e-3, 1e-7];
    println!("\n     f (Hz)   |H| before   |H| tuned   |H| target");
    for &f in probes.iter().step_by(3) {
        let mag = |pp: [f64; 3]| {
            let (ckt, src, out) = build(pp[0], pp[1], pp[2]);
            let omega = 2.0 * std::f64::consts::PI * f;
            vcad_ecad_sim::circuit::ac::ac_response(&ckt, src, omega)
                .unwrap()
                .node_voltages[out]
                .abs()
        };
        println!(
            "  {f:9.1}   {:10.6}   {:9.6}   {:10.6}",
            mag(before),
            mag(p),
            target_mag(f, f_target, q_target)
        );
    }

    // Receipt claims — predicted basis, Provisional rollup.
    let set = receipt::filter_claims(f0_after, q_after, 4, 4);
    let unified = vcad_receipt::DesignReceipt::with_claims(receipt::design_claims(&set));
    println!(
        "\nreceipt: {} claims under {}, rollup = {:?} (predicted basis never rolls up Pass)",
        set.claims.len(),
        receipt::CLAIM_SCHEMA,
        unified.verdict()
    );
    println!("closing instruments: signal generator sweep + $30 USB scope measure |H| directly.");

    let ok = (f0_after - f_target).abs() / f_target < 1e-3
        && (q_after - q_target).abs() / q_target < 1e-3;
    if !ok {
        eprintln!("FAIL: tuned filter missed the target");
        std::process::exit(1);
    }
    println!("\nPASS: cutoff within 0.1% and Q within 0.1% of the Butterworth target.");
}

fn grad_norm(g: &[f64; 3]) -> f64 {
    (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt()
}
