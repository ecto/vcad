//! Planar-magnetics leaves: spiral inductance and motor torque/back-EMF
//! constant.
//!
//! `Scalar`-generic so `tang_expr::ExprId` flows through for symbolic gradients
//! — the differentiable geometry→performance leaves that motor co-design
//! consumes. The dynamics integration (spinning the machine, backprop through
//! time, plant/controller co-design) lives in the phyz repo and is *not* here;
//! this module is the geometry→constant bridge those will build on.

use std::f64::consts::PI;
use tang::Scalar;

/// Modified-Wheeler planar-spiral inductance in henries (Mohan et al. 1999).
/// Radii in mm; `k1`/`k2` are the shape coefficients (circular: 2.25, 3.55).
pub fn coil_inductance_henry<S: Scalar>(
    turns: S,
    inner_r_mm: S,
    outer_r_mm: S,
    k1: f64,
    k2: f64,
) -> S {
    let mu0 = S::from_f64(4.0 * PI * 1e-7);
    let d_avg = (inner_r_mm + outer_r_mm) * S::from_f64(1e-3); // (d_in + d_out)/2 in m
    let fill = (outer_r_mm - inner_r_mm) / (outer_r_mm + inner_r_mm); // fill ratio ρ
    S::from_f64(k1) * mu0 * turns * turns * d_avg / (S::from_f64(1.0) + S::from_f64(k2) * fill)
}

/// Simplified analytical motor torque / back-EMF constant (N·m/A ≈ V·s/rad):
/// `Kt = kw · N · p · B_g · A_pole`, with the pole face area taken as the
/// stator annulus divided by the pole count.
///
/// This is a first-order estimate — its *gradient* (verified against finite
/// difference) is what co-design needs; the physical fidelity is intentionally
/// coarse (no slotting, fringing, or saturation).
pub fn motor_torque_constant<S: Scalar>(
    pole_pairs: S,
    turns_per_phase: S,
    winding_factor: S,
    airgap_flux_tesla: S,
    inner_r_mm: S,
    outer_r_mm: S,
) -> S {
    let rin = inner_r_mm * S::from_f64(1e-3);
    let rout = outer_r_mm * S::from_f64(1e-3);
    let annulus = S::from_f64(PI) * (rout * rout - rin * rin);
    let poles = S::from_f64(2.0) * pole_pairs;
    let pole_area = annulus / poles;
    winding_factor * turns_per_phase * pole_pairs * airgap_flux_tesla * pole_area
}

#[cfg(test)]
mod tests {
    use super::*;
    use tang_expr::{trace, ExprId};

    // Circular spiral coefficients.
    const K1: f64 = 2.25;
    const K2: f64 = 3.55;

    #[test]
    fn inductance_gradient_matches_finite_difference() {
        let (rin, rout) = (2.0_f64, 6.0_f64);
        let n0 = 10.0_f64;
        // Trace L(turns) with var(0) = turns; radii baked.
        let (mut g, expr) = trace(|| {
            coil_inductance_henry(
                ExprId::var(0),
                ExprId::from_f64(rin),
                ExprId::from_f64(rout),
                K1,
                K2,
            )
        });
        let direct = coil_inductance_henry(n0, rin, rout, K1, K2);
        assert!((g.eval(expr, &[n0]) - direct).abs() < 1e-9 * (1.0 + direct.abs()));

        let dexpr = g.diff(expr, 0);
        let grad = g.eval(dexpr, &[n0]);
        let eps = 1e-4;
        let fd = (coil_inductance_henry(n0 + eps, rin, rout, K1, K2)
            - coil_inductance_henry(n0 - eps, rin, rout, K1, K2))
            / (2.0 * eps);
        assert!((grad - fd).abs() < 1e-6 * (1.0 + fd.abs()), "dL/dn {grad} vs fd {fd}");
        assert!(grad > 0.0, "more turns -> more inductance");
    }

    #[test]
    fn torque_constant_gradient_matches_finite_difference() {
        // dKt/d(outer_radius) symbolic vs FD; var(0) = outer_radius_mm.
        let kt = |rout: f64| motor_torque_constant(6.0, 60.0, 0.866, 0.4, 5.0, rout);
        let r0 = 30.0_f64;
        let (mut g, expr) = trace(|| {
            motor_torque_constant(
                ExprId::from_f64(6.0),
                ExprId::from_f64(60.0),
                ExprId::from_f64(0.866),
                ExprId::from_f64(0.4),
                ExprId::from_f64(5.0),
                ExprId::var(0),
            )
        });
        assert!((g.eval(expr, &[r0]) - kt(r0)).abs() < 1e-9 * (1.0 + kt(r0).abs()));

        let dexpr = g.diff(expr, 0);
        let grad = g.eval(dexpr, &[r0]);
        let eps = 1e-3;
        let fd = (kt(r0 + eps) - kt(r0 - eps)) / (2.0 * eps);
        assert!((grad - fd).abs() < 1e-6 * (1.0 + fd.abs()), "dKt/dr {grad} vs fd {fd}");
        assert!(grad > 0.0, "larger stator -> higher torque constant");
    }
}
