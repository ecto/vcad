//! Step shaper — the transient adjoint earns its keep.
//!
//! A series-RLC 2nd-order low-pass starts badly underdamped (ringing, ~50%
//! overshoot) and is driven by **transient adjoint gradients** (one reverse
//! sweep per iteration — see `circuit::transient_adjoint`) until its step
//! response hits a target: rise time set by f_n = 10 kHz and overshoot
//! ≤ 5% (ζ = 0.7 → 4.6% analytic overshoot).
//!
//! The target waveform is the analytic underdamped 2nd-order step response
//! v(t) = V·(1 − e^{−ζω_n t}(cos ω_d t + ζ/√(1−ζ²)·sin ω_d t)); the
//! objective is J(R, L, C) = Σ_k (v_out(t_k) − v*(t_k))², minimized by
//! gradient descent in log-parameter space (components are positive scale
//! quantities) with a backtracking line search and a scale-invariant
//! stopping rule.
//!
//! Run: `cargo run -p vcad-ecad-sim --example step_shaper`

use vcad_ecad_sim::circuit::transient_adjoint::transient_sensitivities;
use vcad_ecad_sim::circuit::{Circuit, Device, Integrator};

const VSTEP: f64 = 5.0;
const DT: f64 = 1e-7;
const N: usize = 1200; // 120 µs ≈ 1.2 periods of settling at 10 kHz

/// Series RLC low-pass; returns (circuit, device ids of [R, L, C], out node).
fn build(r: f64, l: f64, c: f64) -> (Circuit, [usize; 3], usize) {
    let mut ckt = Circuit::new();
    let vin = ckt.node();
    let mid = ckt.node();
    let out = ckt.node();
    ckt.add(Device::VSource {
        p: vin,
        n: 0,
        v: VSTEP,
    });
    let rid = ckt.add(Device::Resistor { p: vin, n: mid, r });
    let lid = ckt.add(Device::Inductor { p: mid, n: out, l });
    let cid = ckt.add(Device::Capacitor { p: out, n: 0, c });
    (ckt, [rid, lid, cid], out)
}

/// Analytic step response of the 2nd-order target (ζ < 1).
fn target_waveform(fn_hz: f64, zeta: f64) -> Vec<f64> {
    let wn = 2.0 * std::f64::consts::PI * fn_hz;
    let wd = wn * (1.0 - zeta * zeta).sqrt();
    (1..=N)
        .map(|k| {
            let t = k as f64 * DT;
            let env = (-zeta * wn * t).exp();
            VSTEP
                * (1.0
                    - env * ((wd * t).cos() + zeta / (1.0 - zeta * zeta).sqrt() * (wd * t).sin()))
        })
        .collect()
}

/// (10→90% rise time, overshoot fraction) measured from a waveform.
fn rise_and_overshoot(v: &[f64]) -> (f64, f64) {
    let vf = VSTEP;
    let t10 = v.iter().position(|&x| x >= 0.1 * vf).unwrap_or(0);
    let t90 = v.iter().position(|&x| x >= 0.9 * vf).unwrap_or(v.len() - 1);
    let peak = v.iter().cloned().fold(f64::MIN, f64::max);
    ((t90 - t10) as f64 * DT, (peak - vf).max(0.0) / vf)
}

fn evaluate(p: [f64; 3], target: &[f64], weights: &[f64]) -> (f64, [f64; 3], Vec<f64>) {
    let (ckt, ids, out) = build(p[0], p[1], p[2]);
    let s = transient_sensitivities(&ckt, DT, Integrator::Trapezoidal, out, target, weights)
        .expect("transient solve");
    // Log-space gradient: dJ/d ln p = p · dJ/dp.
    let g = [
        p[0] * s.gradient[ids[0]],
        p[1] * s.gradient[ids[1]],
        p[2] * s.gradient[ids[2]],
    ];
    (s.value, g, s.v_out)
}

fn main() {
    let target = target_waveform(10_000.0, 0.7);
    let weights = vec![1.0; N];

    // Start badly underdamped: ζ ≈ 0.16, ringing hard.
    let mut p = [50.0, 2.2e-3, 4.7e-8]; // R (Ω), L (H), C (F)
    let (mut j, mut g, mut v) = evaluate(p, &target, &weights);
    let (tr0, os0) = rise_and_overshoot(&v);
    println!(
        "before: R = {:.1} Ω, L = {:.3e} H, C = {:.3e} F",
        p[0], p[1], p[2]
    );
    println!(
        "        J = {j:.4e}, rise = {:.2} µs, overshoot = {:.1}%",
        tr0 * 1e6,
        os0 * 100.0
    );

    let mut iters = 0usize;
    loop {
        let gnorm = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
        // Scale-invariant stop: log-space gradient small relative to J's scale.
        if gnorm < 1e-9 * (j + 1e-12) / DT.sqrt() || j < 1e-12 || iters >= 500 {
            break;
        }
        // Backtracking line search in log space.
        let mut step = 0.5 / gnorm.max(1e-30);
        loop {
            let p_try = [
                p[0] * (-step * g[0]).exp(),
                p[1] * (-step * g[1]).exp(),
                p[2] * (-step * g[2]).exp(),
            ];
            let (j_try, g_try, v_try) = evaluate(p_try, &target, &weights);
            if j_try < j {
                p = p_try;
                (j, g, v) = (j_try, g_try, v_try);
                break;
            }
            step *= 0.5;
            if step * gnorm < 1e-12 {
                break;
            }
        }
        if step * gnorm < 1e-12 {
            break;
        }
        iters += 1;
    }

    let (tr, os) = rise_and_overshoot(&v);
    println!(
        "after:  R = {:.1} Ω, L = {:.3e} H, C = {:.3e} F",
        p[0], p[1], p[2]
    );
    println!(
        "        J = {j:.4e}, rise = {:.2} µs, overshoot = {:.1}%",
        tr * 1e6,
        os * 100.0
    );
    println!("        {iters} gradient iterations (one reverse sweep each)");

    assert!(os <= 0.05 + 1e-3, "overshoot target missed: {os}");
    println!("target hit: overshoot ≤ 5% with f_n = 10 kHz rise time");
}
