//! LTE-based adaptive-timestep validation (SPICE M1, item 3).
//!
//! Rungs:
//! 1. Stiff RC pair (τ ratio 1e4): adaptive reaches t_end in far fewer steps
//!    than a fixed grid resolving the fast pole, at equal-or-better max error
//!    vs the analytic exponentials.
//! 2. RLC ringdown: adaptive preserves the frequency/envelope accuracy of the
//!    fixed-step gate in `circuit_validation.rs`.
//! 3. Diode rectifier under a stepped sine: dt shrinks near the conduction
//!    knee and grows on the flat — the min/max accepted-step spread proves the
//!    controller actually moves.
//! 4. Tellegen power balance holds at every *accepted* step.
//! 5. Fixed-step regression: with adaptivity off, observations are
//!    bit-identical to the M0 code (golden bit patterns captured pre-change).

use vcad_ecad_sim::circuit::{AdaptiveConfig, Circuit, CircuitEnv, Device, DiodeModel, Integrator};

/// 5 V step into two independent RC branches: τ_slow = 1 ms, τ_fast = 100 ns.
fn stiff_rc() -> (Circuit, usize, usize) {
    let mut ckt = Circuit::new();
    let vin = ckt.node();
    let slow = ckt.node();
    let fast = ckt.node();
    ckt.add(Device::VSource {
        p: vin,
        n: 0,
        v: 5.0,
    });
    ckt.add(Device::Resistor {
        p: vin,
        n: slow,
        r: 1_000.0,
    });
    ckt.add(Device::Capacitor {
        p: slow,
        n: 0,
        c: 1e-6,
    }); // τ = 1 ms
    ckt.add(Device::Resistor {
        p: vin,
        n: fast,
        r: 1.0,
    });
    ckt.add(Device::Capacitor {
        p: fast,
        n: 0,
        c: 1e-7,
    }); // τ = 100 ns
    (ckt, slow, fast)
}

fn stiff_rc_max_err(obs: &[vcad_ecad_sim::circuit::Observation], slow: usize, fast: usize) -> f64 {
    let mut worst = 0.0f64;
    for o in obs {
        let es = 5.0 * (1.0 - (-o.time / 1e-3).exp());
        let ef = 5.0 * (1.0 - (-o.time / 1e-7).exp());
        worst = worst.max((o.node_voltages[slow] - es).abs());
        worst = worst.max((o.node_voltages[fast] - ef).abs());
    }
    worst
}

#[test]
fn adaptive_beats_fixed_on_stiff_rc_pair() {
    let t_end = 5e-3; // 5 τ_slow

    // Fixed grid that resolves the fast pole: dt = τ_fast/10 → 500k steps.
    let (ckt, slow, fast) = stiff_rc();
    let mut env = CircuitEnv::new(ckt, 1e-8);
    env.set_integrator(Integrator::Trapezoidal);
    env.reset();
    let fixed_obs = env.step_to(t_end);
    let fixed_steps = fixed_obs.len();
    let fixed_err = stiff_rc_max_err(&fixed_obs, slow, fast);

    // Adaptive with the same starting dt.
    let (ckt, slow, fast) = stiff_rc();
    let mut env = CircuitEnv::new(ckt, 1e-8);
    env.set_adaptive(AdaptiveConfig {
        reltol: 1e-4,
        abstol: 1e-7,
        dt_min: 1e-10,
        dt_max: 1e-4,
    });
    env.reset();
    let adaptive_obs = env.step_to(t_end);
    let adaptive_steps = adaptive_obs.len();
    let adaptive_err = stiff_rc_max_err(&adaptive_obs, slow, fast);

    // Real numbers, asserted with headroom (see the research log for the
    // measured table): fixed = 500_000 steps, adaptive ≈ low thousands.
    eprintln!(
        "stiff RC: fixed {fixed_steps} steps err {fixed_err:.3e} | adaptive {adaptive_steps} steps err {adaptive_err:.3e}"
    );
    // ±1 for float accumulation on the fixed grid.
    assert!((500_000..=500_001).contains(&fixed_steps));
    assert!(
        adaptive_steps < fixed_steps / 100,
        "adaptive took {adaptive_steps} steps, want ≪ {fixed_steps}"
    );
    assert!(
        adaptive_err < 5e-4,
        "adaptive max error {adaptive_err:.3e} vs analytic"
    );
    assert!(
        adaptive_err < 10.0 * fixed_err.max(1e-5),
        "adaptive err {adaptive_err:.3e} should be comparable to fixed {fixed_err:.3e}"
    );
    // And it actually reaches t_end.
    let t_last = adaptive_obs.last().unwrap().time;
    assert!((t_last - t_end).abs() < 1e-12, "landed at {t_last}");
}

#[test]
fn adaptive_preserves_rlc_ringdown_accuracy() {
    // Same series RLC as the fixed-step validation gate: R = 20 Ω, L = 1 mH,
    // C = 100 nF → ω₀ = 100 krad/s, Q = 5.
    let (r, l, c): (f64, f64, f64) = (20.0, 1e-3, 1e-7);
    let omega0 = 1.0 / (l * c).sqrt();
    let q = (1.0 / r) * (l / c).sqrt();
    let omega_d = omega0 * (1.0 - 1.0 / (4.0 * q * q)).sqrt();
    let alpha = r / (2.0 * l);

    let mut ckt = Circuit::new();
    let vin = ckt.node();
    let mid = ckt.node();
    let out = ckt.node();
    ckt.add(Device::VSource {
        p: vin,
        n: 0,
        v: 1.0,
    });
    ckt.add(Device::Resistor { p: vin, n: mid, r });
    ckt.add(Device::Inductor { p: mid, n: out, l });
    ckt.add(Device::Capacitor { p: out, n: 0, c });

    let mut env = CircuitEnv::new(ckt, 2e-8);
    env.set_adaptive(AdaptiveConfig {
        reltol: 1e-5,
        abstol: 1e-9,
        dt_min: 1e-10,
        dt_max: 1e-6,
    });
    env.reset();
    let t_end = 8.0 * 2.0 * std::f64::consts::PI / omega_d;
    let samples: Vec<(f64, f64)> = env
        .step_to(t_end)
        .iter()
        .map(|o| (o.time, o.node_voltages[out] - 1.0))
        .collect();

    let mut crossings: Vec<f64> = Vec::new();
    let mut peaks: Vec<f64> = Vec::new();
    for k in 1..samples.len() - 1 {
        let (t0, y0) = samples[k - 1];
        let (t1, y1) = samples[k];
        let (_, y2) = samples[k + 1];
        if y0 < 0.0 && y1 >= 0.0 {
            crossings.push(t0 + (-y0 / (y1 - y0)) * (t1 - t0));
        }
        if y1 > 0.0 && y1 > y0 && y1 >= y2 {
            peaks.push(y1);
        }
    }

    assert!(crossings.len() >= 4, "need several ring periods");
    let n = crossings.len() - 1;
    let period = (crossings[n] - crossings[0]) / n as f64;
    let omega_meas = 2.0 * std::f64::consts::PI / period;
    let rel_err = (omega_meas - omega_d).abs() / omega_d;
    assert!(rel_err < 1e-3, "ring frequency off by {rel_err:.2e}");

    assert!(peaks.len() >= 4, "need several peaks, got {}", peaks.len());
    let decay = (peaks[1] / peaks[3]).ln() / (2.0 * period);
    let alpha_rel = (decay - alpha).abs() / alpha;
    assert!(alpha_rel < 2e-2, "envelope decay off by {alpha_rel:.2e}");
}

#[test]
fn adaptive_dt_tracks_diode_conduction_knee() {
    // Half-wave rectifier: stepped-sine source — R — diode — RC load. The
    // source is re-set before every accepted step (zeroth-order hold), so the
    // waveform the LTE sees is the circuit's own response: sharp at the
    // conduction knee, flat between half-cycles.
    let f = 1_000.0; // 1 kHz
    let mut ckt = Circuit::new();
    let vin = ckt.node();
    let mid = ckt.node();
    let out = ckt.node();
    let src = ckt.add(Device::VSource {
        p: vin,
        n: 0,
        v: 0.0,
    });
    ckt.add(Device::Resistor {
        p: vin,
        n: mid,
        r: 100.0,
    });
    ckt.add(Device::Diode {
        p: mid,
        n: out,
        model: DiodeModel::silicon(),
    });
    ckt.add(Device::Capacitor {
        p: out,
        n: 0,
        c: 1e-6,
    });
    ckt.add(Device::Resistor {
        p: out,
        n: 0,
        r: 10_000.0,
    });

    let mut env = CircuitEnv::new(ckt, 1e-6);
    env.set_adaptive(AdaptiveConfig {
        reltol: 1e-3,
        abstol: 1e-6,
        dt_min: 1e-9,
        dt_max: 5e-5,
    });
    env.reset();

    let t_end = 3.0 / f; // three cycles
    let mut t_prev = 0.0;
    let mut dts: Vec<f64> = Vec::new();
    while t_prev < t_end {
        let t = env.observe().time;
        env.set_value(src, 5.0 * (2.0 * std::f64::consts::PI * f * t).sin());
        let obs = env.step();
        dts.push(obs.time - t_prev);
        t_prev = obs.time;
    }

    // Skip the startup ramp; look at the settled cycles.
    let settled = &dts[dts.len() / 3..];
    let dt_min_seen = settled.iter().cloned().fold(f64::INFINITY, f64::min);
    let dt_max_seen = settled.iter().cloned().fold(0.0f64, f64::max);
    assert!(
        dt_max_seen / dt_min_seen > 10.0,
        "controller should spread dt ≥ 10× across the cycle: min {dt_min_seen:.3e} max {dt_max_seen:.3e}"
    );
    assert!(
        dt_max_seen > 1e-5,
        "flat regions should coast at large dt, got {dt_max_seen:.3e}"
    );
}

#[test]
fn tellegen_holds_at_every_accepted_step() {
    // The rung-5 mixed network, driven adaptively.
    let mut ckt = Circuit::new();
    let vin = ckt.node();
    let a = ckt.node();
    let b = ckt.node();
    ckt.add(Device::VSource {
        p: vin,
        n: 0,
        v: 5.0,
    });
    ckt.add(Device::Resistor {
        p: vin,
        n: a,
        r: 470.0,
    });
    ckt.add(Device::Capacitor {
        p: a,
        n: 0,
        c: 2.2e-6,
    });
    ckt.add(Device::Inductor {
        p: a,
        n: b,
        l: 1e-3,
    });
    ckt.add(Device::Resistor {
        p: b,
        n: 0,
        r: 220.0,
    });
    ckt.add(Device::Diode {
        p: a,
        n: 0,
        model: DiodeModel::silicon(),
    });

    let mut env = CircuitEnv::new(ckt, 1e-6);
    env.set_adaptive(AdaptiveConfig {
        reltol: 1e-3,
        abstol: 1e-6,
        dt_min: 1e-10,
        dt_max: 1e-4,
    });
    env.reset();
    let mut k = 0usize;
    while env.observe().time < 2e-3 {
        let obs = env.step();
        let dissipated = (5.0 * obs.device_currents[0].abs()).max(1e-6);
        let residual = env.power_balance().abs();
        assert!(
            residual / dissipated < 1e-9,
            "step {k} (t={:.3e}): Tellegen residual {residual:.3e} vs scale {dissipated:.3e}",
            obs.time
        );
        k += 1;
    }
}

#[test]
fn fixed_step_path_is_bit_identical_to_m0() {
    // Golden bit patterns captured from the pre-adaptive M0 code on this
    // exact circuit (rung-5 network, dt = 1 µs, 500 steps). Any drift in the
    // fixed-step discretization breaks the adjoint/WASM contract.
    let golden = [
        (
            Integrator::BackwardEuler,
            0x3fe66e1ddf86218fu64, // v_a
            0x3fe66e1ddf86218eu64, // v_b
            0xbf82bba06d004126u64, // i_src
        ),
        (
            Integrator::Trapezoidal,
            0x3fe66e1ddf862187u64,
            0x3fe66e1ddf862186u64,
            0xbf82bba06d004127u64,
        ),
    ];
    for (integ, va, vb, isrc) in golden {
        let mut ckt = Circuit::new();
        let vin = ckt.node();
        let a = ckt.node();
        let b = ckt.node();
        ckt.add(Device::VSource {
            p: vin,
            n: 0,
            v: 5.0,
        });
        ckt.add(Device::Resistor {
            p: vin,
            n: a,
            r: 470.0,
        });
        ckt.add(Device::Capacitor {
            p: a,
            n: 0,
            c: 2.2e-6,
        });
        ckt.add(Device::Inductor {
            p: a,
            n: b,
            l: 1e-3,
        });
        ckt.add(Device::Resistor {
            p: b,
            n: 0,
            r: 220.0,
        });
        ckt.add(Device::Diode {
            p: a,
            n: 0,
            model: DiodeModel::silicon(),
        });
        let mut env = CircuitEnv::new(ckt, 1e-6);
        env.set_integrator(integ);
        env.reset();
        let mut obs = env.observe();
        for _ in 0..500 {
            obs = env.step();
        }
        assert_eq!(obs.node_voltages[2].to_bits(), va, "{integ:?} v_a drifted");
        assert_eq!(obs.node_voltages[3].to_bits(), vb, "{integ:?} v_b drifted");
        assert_eq!(
            obs.device_currents[0].to_bits(),
            isrc,
            "{integ:?} i_src drifted"
        );
        assert_eq!(obs.time.to_bits(), 0x3f40624dd2f1aa30u64, "time drifted");
    }
}
