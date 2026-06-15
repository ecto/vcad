//! Analytical motor performance evaluation (Tier 1).
//!
//! A clean closed-form evaluator that turns a motor's magnetics + electrical
//! parameters into the headline performance numbers an engineer (or an AI agent)
//! asks for first: the torque constant `Kt`, the back-EMF constant `Ke`, the
//! no-load speed, the stall torque, and a few points on the speed–torque line.
//!
//! It reuses [`crate::magnetics::motor_torque_constant`] for `Kt`, so the air-gap
//! flux that drives torque is the same value the differentiable co-design leaf
//! sees — compute the flux once (e.g. with [`crate::airgap::airgap_flux_density`])
//! and feed it here.
//!
//! # Model (DC / BLDC first order)
//!
//! For an ideal DC-like machine the steady-state voltage balance is
//! `V = i·R + Ke·ω`, and torque is `T = Kt·i`. With `Kt == Ke` in SI units
//! (N·m/A == V·s/rad) this gives the classic linear speed–torque line:
//!
//! ```text
//!   no-load speed   ω0 = V / Ke              (i -> 0, T -> 0)
//!   stall torque    T_s = Kt · V / R         (ω -> 0, i = V/R)
//!   T(ω) = T_s · (1 - ω/ω0)
//! ```
//!
//! Same honesty as the torque-constant leaf it builds on: no slotting, fringing,
//! saturation, iron/friction losses, or inductive dynamics. It is the
//! steady-state envelope, not a transient sim — the dynamics live in the diff
//! crate's rollout and ultimately phyz.

use crate::magnetics::motor_torque_constant;
use serde::{Deserialize, Serialize};

/// Inputs for the analytical motor evaluator.
///
/// Radii in millimetres, resistance in ohms, voltage in volts, flux in tesla.
/// `airgap_flux_tesla` is an input — compute it with
/// [`crate::airgap::airgap_flux_density`] (cored) or
/// [`crate::airgap::aircored_airgap_flux_density`] (coreless) and pass it in.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MotorSpec {
    /// Number of pole pairs `p` (electrical periods per mechanical revolution).
    pub pole_pairs: f64,
    /// Series turns per phase `N`.
    pub turns_per_phase: f64,
    /// Winding factor `kw` (distribution × pitch, typically 0.85–0.96).
    pub winding_factor: f64,
    /// Inner stator (bore) radius, mm.
    pub inner_r_mm: f64,
    /// Outer stator radius, mm.
    pub outer_r_mm: f64,
    /// Per-phase resistance, ohms.
    pub phase_resistance_ohm: f64,
    /// DC supply / bus voltage, volts.
    pub supply_voltage_v: f64,
    /// Air-gap flux density `B_gap`, tesla (see [`crate::airgap`]).
    pub airgap_flux_tesla: f64,
}

/// One point on the steady-state speed–torque line.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatingPoint {
    /// Mechanical angular velocity, rad/s.
    pub speed_rad_s: f64,
    /// Output torque at that speed, N·m.
    pub torque_nm: f64,
}

/// Headline analytical performance of a [`MotorSpec`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MotorPerformance {
    /// Torque constant `Kt`, N·m/A.
    pub kt_nm_per_a: f64,
    /// Back-EMF constant `Ke`, V·s/rad. Equal to `Kt` in SI units.
    pub ke_v_s_per_rad: f64,
    /// No-load speed `ω0 = V / Ke`, rad/s. `INFINITY` when `Ke == 0`.
    pub no_load_speed_rad_s: f64,
    /// Stall torque `T_s = Kt · V / R`, N·m. `0.0` when `R == 0` is treated as
    /// open (no current); a positive `R` gives the finite stall current `V/R`.
    pub stall_torque_nm: f64,
    /// A handful of `(speed, torque)` points along the line from stall (ω=0) to
    /// no-load (ω=ω0), inclusive.
    pub curve: Vec<OperatingPoint>,
}

/// Evaluate the closed-form steady-state performance of a motor.
///
/// `Kt` is computed from the magnetics via
/// [`crate::magnetics::motor_torque_constant`]; `Ke == Kt` (SI). The curve is
/// sampled at 5 evenly spaced speeds from stall to no-load.
///
/// First-order steady-state only — no losses, no inductive transient. See the
/// module docs for the modeling assumptions.
pub fn evaluate_motor(spec: &MotorSpec) -> MotorPerformance {
    let kt = motor_torque_constant(
        spec.pole_pairs,
        spec.turns_per_phase,
        spec.winding_factor,
        spec.airgap_flux_tesla,
        spec.inner_r_mm,
        spec.outer_r_mm,
    );
    // SI: torque constant and back-EMF constant are numerically identical.
    let ke = kt;

    let no_load_speed = if ke > 0.0 {
        spec.supply_voltage_v / ke
    } else {
        f64::INFINITY
    };

    // Stall: ω = 0, full stall current i = V/R. R <= 0 -> no current path.
    let stall_torque = if spec.phase_resistance_ohm > 0.0 {
        kt * spec.supply_voltage_v / spec.phase_resistance_ohm
    } else {
        0.0
    };

    // Sample the line T(ω) = T_s · (1 - ω/ω0) from stall to no-load.
    let n = 5usize;
    let mut curve = Vec::with_capacity(n);
    if no_load_speed.is_finite() && no_load_speed > 0.0 {
        for k in 0..n {
            let frac = k as f64 / (n as f64 - 1.0); // 0 .. 1
            let speed = frac * no_load_speed;
            let torque = stall_torque * (1.0 - frac);
            curve.push(OperatingPoint {
                speed_rad_s: speed,
                torque_nm: torque,
            });
        }
    } else {
        // Degenerate (Ke == 0): just report the stall point.
        curve.push(OperatingPoint {
            speed_rad_s: 0.0,
            torque_nm: stall_torque,
        });
    }

    MotorPerformance {
        kt_nm_per_a: kt,
        ke_v_s_per_rad: ke,
        no_load_speed_rad_s: no_load_speed,
        stall_torque_nm: stall_torque,
        curve,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_spec() -> MotorSpec {
        MotorSpec {
            pole_pairs: 6.0,
            turns_per_phase: 60.0,
            winding_factor: 0.866,
            inner_r_mm: 5.0,
            outer_r_mm: 30.0,
            phase_resistance_ohm: 0.5,
            supply_voltage_v: 24.0,
            airgap_flux_tesla: 0.4,
        }
    }

    #[test]
    fn kt_positive_and_equals_ke() {
        let perf = evaluate_motor(&base_spec());
        assert!(perf.kt_nm_per_a > 0.0, "Kt should be positive");
        assert_eq!(
            perf.kt_nm_per_a, perf.ke_v_s_per_rad,
            "Kt and Ke must be equal in SI units"
        );
    }

    #[test]
    fn higher_airgap_flux_gives_higher_kt() {
        let lo = evaluate_motor(&base_spec());
        let mut hi_spec = base_spec();
        hi_spec.airgap_flux_tesla = 0.8;
        let hi = evaluate_motor(&hi_spec);
        assert!(
            hi.kt_nm_per_a > lo.kt_nm_per_a,
            "more air-gap flux -> higher Kt: {} !> {}",
            hi.kt_nm_per_a,
            lo.kt_nm_per_a
        );
    }

    #[test]
    fn stall_torque_is_kt_times_v_over_r() {
        let spec = base_spec();
        let perf = evaluate_motor(&spec);
        let expected = perf.kt_nm_per_a * spec.supply_voltage_v / spec.phase_resistance_ohm;
        assert!(
            (perf.stall_torque_nm - expected).abs() < 1e-12,
            "stall {} vs Kt·V/R {expected}",
            perf.stall_torque_nm
        );
    }

    #[test]
    fn no_load_speed_is_v_over_ke() {
        let spec = base_spec();
        let perf = evaluate_motor(&spec);
        let expected = spec.supply_voltage_v / perf.ke_v_s_per_rad;
        assert!((perf.no_load_speed_rad_s - expected).abs() < 1e-9);
        assert!(perf.no_load_speed_rad_s > 0.0);
    }

    #[test]
    fn curve_runs_from_stall_to_no_load() {
        let perf = evaluate_motor(&base_spec());
        assert_eq!(perf.curve.len(), 5);
        let first = perf.curve.first().unwrap();
        let last = perf.curve.last().unwrap();
        // Stall end: ω = 0, T = stall torque.
        assert!((first.speed_rad_s - 0.0).abs() < 1e-12);
        assert!((first.torque_nm - perf.stall_torque_nm).abs() < 1e-9);
        // No-load end: ω = ω0, T = 0.
        assert!((last.speed_rad_s - perf.no_load_speed_rad_s).abs() < 1e-9);
        assert!(last.torque_nm.abs() < 1e-9);
        // Monotonic: speed up, torque down.
        for w in perf.curve.windows(2) {
            assert!(w[1].speed_rad_s > w[0].speed_rad_s);
            assert!(w[1].torque_nm < w[0].torque_nm + 1e-12);
        }
    }

    #[test]
    fn airgap_to_performance_end_to_end() {
        // Compute B_gap from magnet geometry, feed it straight into the evaluator.
        use crate::airgap::{airgap_flux_density, AirGapSpec};
        let b_gap = airgap_flux_density(&AirGapSpec::ndfeb_default());
        let mut spec = base_spec();
        spec.airgap_flux_tesla = b_gap;
        let perf = evaluate_motor(&spec);
        assert!(perf.kt_nm_per_a > 0.0);
        // Sanity: a realistic NdFeB flux gives a sensible no-load speed.
        assert!(perf.no_load_speed_rad_s.is_finite() && perf.no_load_speed_rad_s > 0.0);
    }
}
