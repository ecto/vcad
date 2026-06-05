//! Golden tests for the transient solver (linear + nonlinear), validated against
//! the analytic behaviour of textbook circuits.

use super::{Circuit, CircuitEnv, Device, DiodeModel, MotorParams};

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

#[test]
fn silicon_diode_forward_drop() {
    // 5 V through 1 kΩ into a forward diode → ~0.65 V drop, ~4.3 mA.
    let mut c = Circuit::new();
    let vin = c.node();
    let a = c.node();
    c.add(Device::VSource {
        p: vin,
        n: 0,
        v: 5.0,
    });
    c.add(Device::Resistor {
        p: vin,
        n: a,
        r: 1_000.0,
    });
    let d = c.add(Device::Diode {
        p: a,
        n: 0,
        model: DiodeModel::silicon(),
    });

    let mut env = CircuitEnv::new(c, 1e-6);
    env.reset();
    for _ in 0..30 {
        env.step();
    }
    let obs = env.observe();
    let vf = obs.node_voltages[a];
    let i = obs.device_currents[d];
    assert!((0.55..0.75).contains(&vf), "Vf = {vf}");
    assert!((3e-3..5e-3).contains(&i), "i = {i}");
}

#[test]
fn led_forward_lights_at_expected_current() {
    // 5 V through 330 Ω into a red LED → Vf ≈ 1.8 V, ~9–10 mA (LED "on").
    let mut c = Circuit::new();
    let vin = c.node();
    let a = c.node();
    c.add(Device::VSource {
        p: vin,
        n: 0,
        v: 5.0,
    });
    c.add(Device::Resistor {
        p: vin,
        n: a,
        r: 330.0,
    });
    let led = c.add(Device::Diode {
        p: a,
        n: 0,
        model: DiodeModel::led(),
    });

    let mut env = CircuitEnv::new(c, 1e-6);
    env.reset();
    for _ in 0..40 {
        env.step();
    }
    let obs = env.observe();
    let vf = obs.node_voltages[a];
    let i = obs.device_currents[led];
    assert!((1.5..2.1).contains(&vf), "LED Vf = {vf}");
    assert!((7e-3..12e-3).contains(&i), "LED current = {i}");
    // Kirchhoff: LED current ≈ resistor current.
    assert!((i - (5.0 - vf) / 330.0).abs() < 1e-4);
}

#[test]
fn diode_blocks_reverse() {
    // Reverse-biased diode passes ~no current; node sits near the rail.
    let mut c = Circuit::new();
    let vin = c.node();
    let a = c.node();
    c.add(Device::VSource {
        p: vin,
        n: 0,
        v: 5.0,
    });
    c.add(Device::Resistor {
        p: vin,
        n: a,
        r: 1_000.0,
    });
    // anode at ground, cathode at `a` → reverse biased by the +5 V rail.
    let d = c.add(Device::Diode {
        p: 0,
        n: a,
        model: DiodeModel::silicon(),
    });

    let mut env = CircuitEnv::new(c, 1e-6);
    env.reset();
    for _ in 0..30 {
        env.step();
    }
    let obs = env.observe();
    assert!(
        obs.node_voltages[a] > 4.9,
        "v(a) = {}",
        obs.node_voltages[a]
    );
    assert!(
        obs.device_currents[d].abs() < 1e-6,
        "i = {}",
        obs.device_currents[d]
    );
}

#[test]
fn motor_spins_up_to_no_load_speed() {
    // 5 V across a small DC motor → rotor accelerates to the analytic no-load
    // speed ω = V / (Ke + R·b/Kt).
    let mp = MotorParams::small_dc();
    let v = 5.0;
    let expected = v / (mp.ke + mp.r * mp.b / mp.kt);

    let mut c = Circuit::new();
    let a = c.node();
    c.add(Device::VSource { p: a, n: 0, v });
    let motor = c.add(Device::Motor {
        p: a,
        n: 0,
        params: mp,
    });

    let mut env = CircuitEnv::new(c, 1e-5);
    env.reset();
    // ~1 s of sim time (mechanical τ ≈ 0.2 s) → well past spin-up.
    for _ in 0..100_000 {
        env.step();
    }
    let obs = env.observe();
    let omega = obs.rotor_speeds[motor];
    let theta = obs.rotor_angles[motor];
    assert!(
        (omega - expected).abs() < 0.03 * expected,
        "ω = {omega}, expected ≈ {expected}"
    );
    assert!(theta > 0.0, "rotor should have turned: θ = {theta}");
    // Armature current at no load balances friction: I = b·ω / Kt (small).
    let i = obs.device_currents[motor].abs();
    assert!(i > 0.0 && i < 0.2, "no-load current = {i}");
}

#[test]
fn motor_slows_under_load() {
    let mut mp = MotorParams::small_dc();
    mp.load = 5e-3; // 5 mN·m opposing torque
    let v = 5.0;

    let mut c = Circuit::new();
    let a = c.node();
    c.add(Device::VSource { p: a, n: 0, v });
    let motor = c.add(Device::Motor {
        p: a,
        n: 0,
        params: mp,
    });

    let mut env = CircuitEnv::new(c, 1e-5);
    env.reset();
    for _ in 0..100_000 {
        env.step();
    }
    let loaded = env.observe().rotor_speeds[motor];

    // Same motor, no load → must spin faster.
    let mut mp0 = MotorParams::small_dc();
    mp0.load = 0.0;
    let mut c2 = Circuit::new();
    let a2 = c2.node();
    c2.add(Device::VSource { p: a2, n: 0, v });
    let m2 = c2.add(Device::Motor {
        p: a2,
        n: 0,
        params: mp0,
    });
    let mut env2 = CircuitEnv::new(c2, 1e-5);
    env2.reset();
    for _ in 0..100_000 {
        env2.step();
    }
    let unloaded = env2.observe().rotor_speeds[m2];

    assert!(
        loaded < unloaded,
        "loaded {loaded} should be < unloaded {unloaded}"
    );
    assert!(loaded > 0.0, "motor should still turn under load: {loaded}");
}
