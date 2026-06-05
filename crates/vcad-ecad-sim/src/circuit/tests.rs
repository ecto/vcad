//! Golden tests for the M0 linear transient solver, validated against the
//! analytic behaviour of textbook circuits.

use super::{Circuit, CircuitEnv, Device};

#[test]
fn resistive_voltage_divider() {
    // 5 V across two equal 1 kΩ resistors → midpoint sits at 2.5 V.
    let mut c = Circuit::new();
    let vin = c.node();
    let mid = c.node();
    c.add(Device::VSource {
        p: vin,
        n: 0,
        v: 5.0,
    });
    c.add(Device::Resistor {
        p: vin,
        n: mid,
        r: 1_000.0,
    });
    c.add(Device::Resistor {
        p: mid,
        n: 0,
        r: 1_000.0,
    });

    let mut env = CircuitEnv::new(c, 1e-6);
    env.reset();
    let obs = env.step(); // purely resistive → exact in one step
    assert!((obs.node_voltages[vin] - 5.0).abs() < 1e-9);
    assert!(
        (obs.node_voltages[mid] - 2.5).abs() < 1e-6,
        "vmid = {}",
        obs.node_voltages[mid]
    );
}

#[test]
fn current_source_through_resistor() {
    // 1 mA forced through 1 kΩ → 1 V.
    let mut c = Circuit::new();
    let a = c.node();
    c.add(Device::ISource {
        p: a,
        n: 0,
        i: 1e-3,
    });
    c.add(Device::Resistor {
        p: a,
        n: 0,
        r: 1_000.0,
    });

    let mut env = CircuitEnv::new(c, 1e-6);
    env.reset();
    let obs = env.step();
    assert!(
        (obs.node_voltages[a] - 1.0).abs() < 1e-6,
        "va = {}",
        obs.node_voltages[a]
    );
}

#[test]
fn rc_charging_matches_analytic() {
    // 5 V into R=1 kΩ, C=1 µF → τ = 1 ms. Check 63.2% at τ and ~99% at 5τ.
    let v = 5.0;
    let r = 1_000.0;
    let cap = 1e-6;
    let tau = r * cap;

    let mut c = Circuit::new();
    let vin = c.node();
    let mid = c.node();
    c.add(Device::VSource { p: vin, n: 0, v });
    c.add(Device::Resistor { p: vin, n: mid, r });
    c.add(Device::Capacitor {
        p: mid,
        n: 0,
        c: cap,
    });

    let dt = tau / 1000.0;
    let mut env = CircuitEnv::new(c, dt);
    env.reset();

    // Step to t = τ.
    for _ in 0..1000 {
        env.step();
    }
    let v_tau = env.observe().node_voltages[mid];
    let expected_tau = v * (1.0 - (-1.0f64).exp()); // 0.632·V
    assert!(
        (v_tau - expected_tau).abs() < 0.02 * v,
        "v(τ) = {v_tau}, expected ≈ {expected_tau}"
    );

    // Step to t = 5τ → essentially fully charged.
    for _ in 0..4000 {
        env.step();
    }
    let v_5tau = env.observe().node_voltages[mid];
    assert!(v_5tau > 0.98 * v, "v(5τ) = {v_5tau}");
}

#[test]
fn rl_current_ramp_matches_analytic() {
    // 5 V, R=10 Ω, L=1 mH → τ = 0.1 ms, steady current 0.5 A.
    let v = 5.0;
    let r = 10.0;
    let l = 1e-3;
    let tau = l / r;
    let i_steady = v / r;

    let mut c = Circuit::new();
    let a = c.node();
    let b = c.node();
    c.add(Device::VSource { p: a, n: 0, v });
    c.add(Device::Resistor { p: a, n: b, r });
    let ind = c.add(Device::Inductor { p: b, n: 0, l });

    let dt = tau / 1000.0;
    let mut env = CircuitEnv::new(c, dt);
    env.reset();

    for _ in 0..1000 {
        env.step();
    }
    let i_tau = env.observe().device_currents[ind];
    let expected_tau = i_steady * (1.0 - (-1.0f64).exp());
    assert!(
        (i_tau - expected_tau).abs() < 0.02 * i_steady,
        "i(τ) = {i_tau}, expected ≈ {expected_tau}"
    );

    for _ in 0..4000 {
        env.step();
    }
    let i_5tau = env.observe().device_currents[ind];
    assert!(i_5tau > 0.98 * i_steady, "i(5τ) = {i_5tau}");
}

#[test]
fn reset_returns_to_power_on_state() {
    let mut c = Circuit::new();
    let vin = c.node();
    let mid = c.node();
    c.add(Device::VSource {
        p: vin,
        n: 0,
        v: 5.0,
    });
    c.add(Device::Resistor {
        p: vin,
        n: mid,
        r: 1_000.0,
    });
    c.add(Device::Capacitor {
        p: mid,
        n: 0,
        c: 1e-6,
    });

    let mut env = CircuitEnv::new(c, 1e-6);
    env.reset();
    for _ in 0..500 {
        env.step();
    }
    assert!(env.observe().node_voltages[mid] > 0.1);
    env.reset();
    let obs = env.observe();
    assert_eq!(obs.time, 0.0);
    assert!(obs.node_voltages[mid].abs() < 1e-12);
}
