//! The closed-form route: laminar duct-loss correlations.
//!
//! Both a feature (instant sizing answers, no lattice required) and the
//! field solver's conscience — the receipt carries the gap between the
//! two routes ([`crate::receipt::SolverProvenance::cross_route_residual`]).
//! Every formula here is exact or textbook-validated for developed
//! laminar flow; none of them know about entrance effects, so compare
//! them against developed-core gradients, not port-to-port totals.

/// Reynolds number `Re = ρ·U·D/μ`.
pub fn reynolds(density_kg_m3: f64, speed_m_s: f64, diameter_m: f64, viscosity_pa_s: f64) -> f64 {
    density_kg_m3 * speed_m_s * diameter_m / viscosity_pa_s
}

/// Hydraulic diameter `D_h = 4A/P`.
pub fn hydraulic_diameter_m(area_m2: f64, perimeter_m: f64) -> f64 {
    4.0 * area_m2 / perimeter_m
}

/// Hagen–Poiseuille pressure drop for a circular pipe, Pa:
/// `Δp = 128·μ·L·Q / (π·D⁴)`. Exact for developed laminar flow.
pub fn poiseuille_pipe_dp_pa(
    flow_m3_s: f64,
    diameter_m: f64,
    length_m: f64,
    viscosity_pa_s: f64,
) -> f64 {
    128.0 * viscosity_pa_s * length_m * flow_m3_s / (std::f64::consts::PI * diameter_m.powi(4))
}

/// Developed laminar pressure gradient in a rectangular duct, Pa/m,
/// from the exact series solution for flow `Q` through a duct of
/// cross-section `a × b`:
///
/// `Q = (dp/dx)·a³·b/(12μ) · [1 − Σ 192a/(π⁵b·n⁵)·tanh(nπb/2a)]`
///
/// where the sum runs over odd `n`. Truncated when terms fall below
/// 1e-14 of the running total (a handful of terms; tanh saturates).
pub fn rect_duct_pressure_gradient_pa_m(
    flow_m3_s: f64,
    width_m: f64,
    height_m: f64,
    viscosity_pa_s: f64,
) -> f64 {
    // Use a = the smaller side for series convergence.
    let (a, b) = if width_m <= height_m {
        (width_m, height_m)
    } else {
        (height_m, width_m)
    };
    let mut series = 0.0f64;
    let mut n = 1u32;
    loop {
        let nf = n as f64;
        let term = (192.0 * a) / (std::f64::consts::PI.powi(5) * b * nf.powi(5))
            * (nf * std::f64::consts::PI * b / (2.0 * a)).tanh();
        series += term;
        if term < 1e-14 * series.max(1e-300) || n > 199 {
            break;
        }
        n += 2;
    }
    let coeff = a.powi(3) * b / (12.0 * viscosity_pa_s) * (1.0 - series);
    flow_m3_s / coeff
}

/// Darcy–Weisbach pressure drop, Pa, with the laminar friction factor
/// `f = 64/Re`: `Δp = f·(L/D)·(ρU²/2)`.
pub fn darcy_weisbach_laminar_dp_pa(
    density_kg_m3: f64,
    speed_m_s: f64,
    diameter_m: f64,
    length_m: f64,
    viscosity_pa_s: f64,
) -> f64 {
    let re = reynolds(density_kg_m3, speed_m_s, diameter_m, viscosity_pa_s);
    let f = 64.0 / re;
    f * (length_m / diameter_m) * 0.5 * density_kg_m3 * speed_m_s * speed_m_s
}

/// Borda–Carnot loss for a sudden expansion from area `a1` to `a2 ≥ a1`,
/// Pa: `Δp_loss = ½·ρ·U₁²·(1 − a1/a2)²`.
pub fn borda_carnot_dp_pa(
    density_kg_m3: f64,
    speed1_m_s: f64,
    area1_m2: f64,
    area2_m2: f64,
) -> f64 {
    let ratio = 1.0 - area1_m2 / area2_m2;
    0.5 * density_kg_m3 * speed1_m_s * speed1_m_s * ratio * ratio
}

/// Laminar entrance length, m: `L_e ≈ 0.05·Re·D_h`. Compare correlations
/// only against the developed core beyond this.
pub fn entrance_length_m(re: f64, hydraulic_diameter_m: f64) -> f64 {
    0.05 * re * hydraulic_diameter_m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_and_darcy_weisbach_agree() {
        // Same physics, two forms: for a circular pipe they must match
        // identically.
        let (rho, mu) = (998.2, 1.002e-3);
        let d = 0.01;
        let u = 0.05;
        let l = 0.5;
        let q = u * std::f64::consts::PI * d * d / 4.0;
        let dp1 = poiseuille_pipe_dp_pa(q, d, l, mu);
        let dp2 = darcy_weisbach_laminar_dp_pa(rho, u, d, l, mu);
        assert!((dp1 - dp2).abs() / dp1 < 1e-12);
    }

    #[test]
    fn square_duct_matches_shah_london() {
        // Shah & London: f·Re = 56.91 for a square duct. Recover it from
        // the series solution.
        let (rho, mu) = (1.204, 1.825e-5);
        let w = 0.005;
        let u = 0.1;
        let q = u * w * w;
        let dpdx = rect_duct_pressure_gradient_pa_m(q, w, w, mu);
        let dh = hydraulic_diameter_m(w * w, 4.0 * w);
        let re = reynolds(rho, u, dh, mu);
        let f = dpdx * dh / (0.5 * rho * u * u);
        let f_re = f * re;
        assert!(
            (f_re - 56.91).abs() < 0.1,
            "f*Re = {f_re:.3}, expected 56.91"
        );
    }

    #[test]
    fn borda_carnot_limits() {
        // No expansion -> no loss; infinite expansion -> full dynamic
        // pressure.
        assert_eq!(borda_carnot_dp_pa(1000.0, 2.0, 1.0, 1.0), 0.0);
        let dp = borda_carnot_dp_pa(1000.0, 2.0, 1.0, 1e12);
        assert!((dp - 0.5 * 1000.0 * 4.0).abs() / dp < 1e-6);
    }
}
