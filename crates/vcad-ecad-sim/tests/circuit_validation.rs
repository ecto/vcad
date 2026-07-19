//! Circuit-simulation validation ladder — every rung against an exact
//! closed form, in the spirit of the particle crate's analytic gates.
//!
//! Rungs:
//! 1. Voltage divider — exact to machine precision.
//! 2. RC step response vs `V·(1 − e^{−t/RC})`, plus the trapezoidal
//!    convergence-order gate: halving dt must quarter the error (2nd order —
//!    Nagel, SPICE2, UCB ERL-M520, 1975, §4).
//! 3. Series RLC ringdown: damped frequency and Q vs the closed forms
//!    ω_d = ω₀√(1 − 1/4Q²), Q = (1/R)√(L/C).
//! 4. Diode + resistor operating point vs the Lambert-W closed form
//!    (Corless et al., "On the Lambert W function", Adv. Comput. Math. 5,
//!    1996): i = (n·Vt/R)·W((Is·R)/(n·Vt)·e^{(V+Is·R)/(n·Vt)}) − Is.
//! 5. Tellegen power balance: Σ v·i over all devices stays below 1e-9 of
//!    dissipated power at **every** timestep — the energy conscience.

use vcad_ecad_sim::circuit::{
    ac, dc, BjtModel, Circuit, CircuitEnv, Device, DiodeModel, Integrator, MosfetModel, Polarity,
};

/// Lambert W₀ by Halley iteration (Corless et al. 1996, §3). Machine
/// precision in < 10 iterations for x > 0 (our x is always positive).
fn lambert_w0(x: f64) -> f64 {
    assert!(x > 0.0);
    // Initial guess: ln(x) branch for large x, else x·(1 − x) shape.
    let mut w = if x > std::f64::consts::E {
        let lx = x.ln();
        lx - lx.ln()
    } else {
        x / (1.0 + x)
    };
    for _ in 0..50 {
        let ew = w.exp();
        let f = w * ew - x;
        let dw = f / (ew * (w + 1.0) - (w + 2.0) * f / (2.0 * w + 2.0));
        w -= dw;
        if dw.abs() < 1e-15 * (1.0 + w.abs()) {
            break;
        }
    }
    w
}

#[test]
fn rung1_voltage_divider_is_exact() {
    let mut c = Circuit::new();
    let vin = c.node();
    let out = c.node();
    c.add(Device::VSource {
        p: vin,
        n: 0,
        v: 10.0,
    });
    c.add(Device::Resistor {
        p: vin,
        n: out,
        r: 7_500.0,
    });
    c.add(Device::Resistor {
        p: out,
        n: 0,
        r: 2_500.0,
    });
    let sol = dc::operating_point(&c).unwrap();
    assert!((sol.node_voltages[out] - 2.5).abs() < 1e-13);
    assert!(sol.power_balance_w.abs() < 1e-12);
}

/// Max |v_sim − v_analytic| of the trapezoidal RC step over one time
/// constant, at timestep `dt`.
fn rc_step_error(dt: f64) -> f64 {
    let (r, c, v0) = (1_000.0, 1e-6, 5.0);
    let tau = r * c;
    let mut ckt = Circuit::new();
    let vin = ckt.node();
    let out = ckt.node();
    ckt.add(Device::VSource {
        p: vin,
        n: 0,
        v: v0,
    });
    ckt.add(Device::Resistor { p: vin, n: out, r });
    ckt.add(Device::Capacitor { p: out, n: 0, c });
    let mut env = CircuitEnv::new(ckt, dt);
    env.set_integrator(Integrator::Trapezoidal);
    env.reset();

    let steps = (tau / dt).round() as usize;
    let mut worst = 0.0f64;
    for _ in 0..steps {
        let obs = env.step();
        let exact = v0 * (1.0 - (-obs.time / tau).exp());
        worst = worst.max((obs.node_voltages[out] - exact).abs());
    }
    worst
}

#[test]
fn rung2_rc_step_matches_analytic_and_error_scales_second_order() {
    // Absolute accuracy at dt = τ/1000: sub-microvolt on a 5 V step.
    let err_fine = rc_step_error(1e-6);
    assert!(err_fine < 5e-6, "trap error {err_fine} too large");

    // Convergence order: halving dt must quarter the error (ratio ≈ 4).
    let e1 = rc_step_error(4e-6);
    let e2 = rc_step_error(2e-6);
    let e3 = rc_step_error(1e-6);
    let r12 = e1 / e2;
    let r23 = e2 / e3;
    assert!(
        (3.5..4.5).contains(&r12) && (3.5..4.5).contains(&r23),
        "trapezoidal must be 2nd order: ratios {r12:.2}, {r23:.2} (want ~4)"
    );

    // And backward Euler is honestly first order (ratio ≈ 2), so the two
    // integrators bracket the claim.
    let be = |dt: f64| {
        let (r, c, v0) = (1_000.0, 1e-6, 5.0);
        let mut ckt = Circuit::new();
        let vin = ckt.node();
        let out = ckt.node();
        ckt.add(Device::VSource {
            p: vin,
            n: 0,
            v: v0,
        });
        ckt.add(Device::Resistor { p: vin, n: out, r });
        ckt.add(Device::Capacitor { p: out, n: 0, c });
        let mut env = CircuitEnv::new(ckt, dt);
        env.reset();
        let steps = (r * c / dt).round() as usize;
        let mut worst = 0.0f64;
        for _ in 0..steps {
            let obs = env.step();
            let exact = v0 * (1.0 - (-obs.time / (r * c)).exp());
            worst = worst.max((obs.node_voltages[out] - exact).abs());
        }
        worst
    };
    let rbe = be(4e-6) / be(2e-6);
    assert!(
        (1.7..2.3).contains(&rbe),
        "BE should be 1st order, ratio {rbe:.2}"
    );
}

#[test]
fn rung3_rlc_ringdown_frequency_and_q_match_closed_forms() {
    // Series RLC, step-driven, underdamped: R = 20 Ω, L = 1 mH, C = 100 nF.
    // ω₀ = 1/√(LC) = 100 krad/s, Q = (1/R)√(L/C) = 5,
    // ω_d = ω₀√(1 − 1/(4Q²)), envelope decay α = R/2L.
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

    let dt = 2e-8; // ~3000 points per ring period
    let mut env = CircuitEnv::new(ckt, dt);
    env.set_integrator(Integrator::Trapezoidal);
    env.reset();

    // Record y(t) = v_out(t) − 1 (the ringdown about the settled value);
    // extract upward zero crossings (→ ω_d) and positive local maxima
    // (→ envelope decay α).
    let mut samples: Vec<(f64, f64)> = Vec::new();
    let steps = (8.0 * 2.0 * std::f64::consts::PI / omega_d / dt) as usize;
    for _ in 0..steps {
        let obs = env.step();
        samples.push((obs.time, obs.node_voltages[out] - 1.0));
    }

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

    // Damped frequency from the mean upward-crossing period.
    assert!(crossings.len() >= 4, "need several ring periods");
    let n = crossings.len() - 1;
    let period = (crossings[n] - crossings[0]) / n as f64;
    let omega_meas = 2.0 * std::f64::consts::PI / period;
    let rel_err = (omega_meas - omega_d).abs() / omega_d;
    assert!(rel_err < 1e-3, "ring frequency off by {rel_err:.2e}");

    // Envelope decay: successive positive peaks are one period apart and
    // shrink by e^{−α·T}. Skip the first peak (startup transient).
    assert!(peaks.len() >= 4, "need several peaks, got {}", peaks.len());
    let decay = (peaks[1] / peaks[3]).ln() / (2.0 * period);
    let alpha_rel = (decay - alpha).abs() / alpha;
    assert!(alpha_rel < 2e-2, "envelope decay off by {alpha_rel:.2e}");
}

#[test]
fn rung4_diode_resistor_matches_lambert_w() {
    // V — R — diode to ground. Exact current via Lambert W (Corless 1996):
    // i = (nVt/R)·W(Is·R/(nVt)·exp((V + Is·R)/(nVt))) − Is
    let (vsrc, r) = (5.0, 1_000.0);
    let model = DiodeModel::silicon();
    let vte = model.n * 0.025_852; // matches devices::VT
    let x = (model.is * r / vte) * ((vsrc + model.is * r) / vte).exp();
    let i_exact = (vte / r) * lambert_w0(x) - model.is;

    let mut c = Circuit::new();
    let vin = c.node();
    let out = c.node();
    c.add(Device::VSource {
        p: vin,
        n: 0,
        v: vsrc,
    });
    let rid = c.add(Device::Resistor { p: vin, n: out, r });
    c.add(Device::Diode {
        p: out,
        n: 0,
        model,
    });
    let sol = dc::operating_point(&c).unwrap();

    let rel = (sol.device_currents[rid] - i_exact).abs() / i_exact;
    assert!(
        rel < 1e-9,
        "diode op point vs Lambert-W: sim {} exact {} rel {rel:.2e}",
        sol.device_currents[rid],
        i_exact
    );
}

#[test]
fn rung5_tellegen_power_balance_every_timestep() {
    // A deliberately mixed network: source, R's, C, L, diode — stepped for
    // 2000 ticks with both integrators. At every tick, Σ v·i must vanish
    // relative to the dissipated power.
    for integ in [Integrator::BackwardEuler, Integrator::Trapezoidal] {
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

        for step in 0..2000 {
            let obs = env.step();
            // Scale: the source power |V·I| (device 0 is the 5 V source).
            let dissipated = (5.0 * obs.device_currents[0].abs()).max(1e-6);
            let residual = env.power_balance().abs();
            assert!(
                residual / dissipated < 1e-9,
                "{integ:?} step {step}: Tellegen residual {residual:.3e} vs scale {dissipated:.3e}"
            );
        }
    }
}

#[test]
fn rung6_mosfet_saturation_matches_square_law() {
    // Gate and drain held by ideal sources → the drain current must equal
    // the Shichman–Hodges closed form (kp/2)·(vgs − vt0)²·(1 + λ·vds)
    // exactly (SPICE2: Nagel, UCB ERL-M520, 1975, §2).
    let model = MosfetModel {
        kp: 0.05,
        vt0: 1.5,
        lambda: 0.02,
        polarity: Polarity::N,
    };
    let (vgs, vds) = (3.0, 8.0); // vds > vov = 1.5 → saturation
    let mut c = Circuit::new();
    let gate = c.node();
    let drain = c.node();
    c.add(Device::VSource {
        p: gate,
        n: 0,
        v: vgs,
    });
    c.add(Device::VSource {
        p: drain,
        n: 0,
        v: vds,
    });
    let mid = c.add(Device::Mosfet {
        d: drain,
        g: gate,
        s: 0,
        model,
    });
    let sol = dc::operating_point(&c).unwrap();

    let vov = vgs - model.vt0;
    let i_exact = 0.5 * model.kp * vov * vov * (1.0 + model.lambda * vds);
    let rel = (sol.device_currents[mid] - i_exact).abs() / i_exact;
    assert!(
        rel < 1e-9,
        "saturation current {} vs square law {i_exact}, rel {rel:.2e}",
        sol.device_currents[mid]
    );
}

#[test]
fn rung7_common_source_gain_matches_gm_rd_parallel_ro() {
    // Resistor-loaded common-source amplifier: small-signal gain from the
    // AC solve must equal −gm·(Rd ∥ ro) with gm and ro from the same op
    // point — two independent code paths to one closed form.
    let model = MosfetModel {
        kp: 2e-3,
        vt0: 1.5,
        lambda: 0.02,
        polarity: Polarity::N,
    };
    let (vdd, vbias, rd) = (12.0, 2.5, 1_000.0);
    let mut c = Circuit::new();
    let ndd = c.node();
    let gate = c.node();
    let drain = c.node();
    c.add(Device::VSource {
        p: ndd,
        n: 0,
        v: vdd,
    });
    let src = c.add(Device::VSource {
        p: gate,
        n: 0,
        v: vbias,
    });
    c.add(Device::Resistor {
        p: ndd,
        n: drain,
        r: rd,
    });
    c.add(Device::Mosfet {
        d: drain,
        g: gate,
        s: 0,
        model,
    });

    let op = dc::operating_point(&c).unwrap();
    let (vgs, vds) = (vbias, op.node_voltages[drain]);
    assert!(vds > vgs - model.vt0, "stage must bias into saturation");
    let vov = vgs - model.vt0;
    let gm = model.kp * vov * (1.0 + model.lambda * vds);
    let gds = 0.5 * model.kp * vov * vov * model.lambda;
    let gain_exact = -gm / (1.0 / rd + gds); // −gm·(Rd ∥ ro)

    // AC at a low frequency (purely resistive network → gain is real).
    let sol = ac::ac_response(&c, src, 1.0).unwrap();
    let h = sol.node_voltages[drain];
    assert!(h.im.abs() < 1e-12, "resistive stage: gain must be real");
    let rel = (h.re - gain_exact).abs() / gain_exact.abs();
    assert!(
        rel < 1e-9,
        "CS gain {} vs −gm·(Rd∥ro) = {gain_exact}, rel {rel:.2e}",
        h.re
    );
}

#[test]
fn rung8_bjt_current_mirror_ratio() {
    // Two matched NPNs, Q1 diode-connected with a reference resistor. The
    // mirror copies I_ref up to the base-current error: with both bases fed
    // from Q1's collector, I_out/I_ref = 1/(1 + 2/βF) exactly for matched
    // devices at equal collector voltage (Ebers–Moll, no Early effect).
    let model = BjtModel::npn();
    let (vcc, rref) = (12.0, 10_000.0);
    let mut c = Circuit::new();
    let ncc = c.node();
    let nref = c.node(); // Q1 collector = both bases
    let nout = c.node(); // Q2 collector
    c.add(Device::VSource {
        p: ncc,
        n: 0,
        v: vcc,
    });
    let rid = c.add(Device::Resistor {
        p: ncc,
        n: nref,
        r: rref,
    });
    c.add(Device::Bjt {
        c: nref,
        b: nref,
        e: 0,
        model,
    });
    let q2 = c.add(Device::Bjt {
        c: nout,
        b: nref,
        e: 0,
        model,
    });
    // Hold Q2's collector at Q1's diode voltage scale so vbc matches too
    // (kills the Early-free reverse-transport mismatch).
    c.add(Device::VSource {
        p: nout,
        n: 0,
        v: 0.65,
    });
    let sol = dc::operating_point(&c).unwrap();

    let i_ref = sol.device_currents[rid];
    let i_out = sol.device_currents[q2];
    let ratio = i_out / i_ref;
    let expected = 1.0 / (1.0 + 2.0 / model.beta_f);
    assert!(
        (ratio - expected).abs() < 5e-3,
        "mirror ratio {ratio} vs {expected} (βF = {})",
        model.beta_f
    );
    assert!(i_ref > 1e-3 / 1.2 && i_ref < 1.2e-3, "I_ref ≈ (Vcc−Vbe)/R");
}

#[test]
fn rung9_tellegen_holds_with_transistors_in_the_network() {
    // The Tellegen gate must keep holding once transistors join: a MOSFET
    // stage and a BJT stage in one network, stepped through a transient
    // with both integrators. Σ v·i (all terminals!) < 1e-9 of source power.
    for integ in [Integrator::BackwardEuler, Integrator::Trapezoidal] {
        let mut ckt = Circuit::new();
        let vdd = ckt.node();
        let gate = ckt.node();
        let drain = ckt.node();
        let base = ckt.node();
        let coll = ckt.node();
        ckt.add(Device::VSource {
            p: vdd,
            n: 0,
            v: 8.0,
        });
        // RC-delayed gate drive → the MOSFET sweeps cutoff → saturation.
        ckt.add(Device::Resistor {
            p: vdd,
            n: gate,
            r: 10_000.0,
        });
        ckt.add(Device::Capacitor {
            p: gate,
            n: 0,
            c: 1e-7,
        });
        ckt.add(Device::Resistor {
            p: vdd,
            n: drain,
            r: 2_200.0,
        });
        ckt.add(Device::Mosfet {
            d: drain,
            g: gate,
            s: 0,
            model: MosfetModel::nmos(),
        });
        // BJT stage biased from the drain node.
        ckt.add(Device::Resistor {
            p: drain,
            n: base,
            r: 47_000.0,
        });
        ckt.add(Device::Resistor {
            p: vdd,
            n: coll,
            r: 4_700.0,
        });
        ckt.add(Device::Bjt {
            c: coll,
            b: base,
            e: 0,
            model: BjtModel::npn(),
        });
        let mut env = CircuitEnv::new(ckt, 1e-6);
        env.set_integrator(integ);
        env.reset();

        for step in 0..2000 {
            let obs = env.step();
            let dissipated = (8.0 * obs.device_currents[0].abs()).max(1e-6);
            let residual = env.power_balance().abs();
            assert!(
                residual / dissipated < 1e-9,
                "{integ:?} step {step}: Tellegen residual {residual:.3e} vs scale {dissipated:.3e}"
            );
        }
    }
}

#[test]
fn dc_and_transient_agree_at_steady_state() {
    // The transient sim, run to steady state, must land on the DC operating
    // point — two independent paths to the same answer.
    let mut ckt = Circuit::new();
    let vin = ckt.node();
    let out = ckt.node();
    ckt.add(Device::VSource {
        p: vin,
        n: 0,
        v: 5.0,
    });
    ckt.add(Device::Resistor {
        p: vin,
        n: out,
        r: 1_000.0,
    });
    ckt.add(Device::Diode {
        p: out,
        n: 0,
        model: DiodeModel::silicon(),
    });
    ckt.add(Device::Capacitor {
        p: out,
        n: 0,
        c: 1e-7,
    });

    let dcsol = dc::operating_point(&ckt).unwrap();

    let mut env = CircuitEnv::new(ckt, 1e-6);
    env.set_integrator(Integrator::Trapezoidal);
    env.reset();
    let mut last = 0.0;
    for _ in 0..5000 {
        last = env.step().node_voltages[out];
    }
    assert!(
        (last - dcsol.node_voltages[out]).abs() < 1e-6,
        "transient settled {last} vs DC {}",
        dcsol.node_voltages[out]
    );
}

#[test]
fn transient_adjoint_matches_fd_on_rlc_both_integrators() {
    // Transient adjoint rung: dJ/dp of a full-transient tracking objective
    // vs central finite differences with the discretization frozen across
    // probes (same dt, step count, integrator) — every element kind, both
    // integrators. The module's own tests cover this in depth; this rung
    // keeps it in the validation ladder alongside the DC/AC adjoint rungs.
    use vcad_ecad_sim::circuit::transient_adjoint::transient_sensitivities;

    let build = |r: f64, l: f64, cv: f64| {
        let mut ckt = Circuit::new();
        let vin = ckt.node();
        let mid = ckt.node();
        let out = ckt.node();
        ckt.add(Device::VSource {
            p: vin,
            n: 0,
            v: 5.0,
        });
        ckt.add(Device::Resistor { p: vin, n: mid, r });
        ckt.add(Device::Inductor { p: mid, n: out, l });
        ckt.add(Device::Capacitor {
            p: out,
            n: 0,
            c: cv,
        });
        (ckt, out)
    };

    let (dt, n) = (2e-7, 300);
    for integ in [Integrator::BackwardEuler, Integrator::Trapezoidal] {
        let (ckt, out) = build(50.0, 1e-3, 1e-7);
        let weights = vec![1.0; n];
        // Track 60% of the network's own response: every step contributes.
        let free = transient_sensitivities(&ckt, dt, integ, out, &vec![0.0; n], &weights).unwrap();
        let targets: Vec<f64> = free.v_out.iter().map(|v| 0.6 * v).collect();
        let sens = transient_sensitivities(&ckt, dt, integ, out, &targets, &weights).unwrap();

        for id in 0..ckt.devices.len() {
            let base = ckt.devices[id].primary();
            let h = base.abs() * 1e-6;
            let mut lo = ckt.clone();
            let mut hi = ckt.clone();
            lo.devices[id].set_primary(base - h);
            hi.devices[id].set_primary(base + h);
            let jlo = transient_sensitivities(&lo, dt, integ, out, &targets, &weights)
                .unwrap()
                .value;
            let jhi = transient_sensitivities(&hi, dt, integ, out, &targets, &weights)
                .unwrap()
                .value;
            let fd = (jhi - jlo) / (2.0 * h);
            let ad = sens.gradient[id];
            let scale = fd.abs().max(ad.abs()).max(1e-12);
            assert!(
                (fd - ad).abs() / scale < 1e-5,
                "{integ:?} device {id}: adjoint {ad} vs FD {fd}"
            );
        }
    }
}
