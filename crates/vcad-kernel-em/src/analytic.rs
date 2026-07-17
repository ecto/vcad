//! Closed-form reference results the validation ladder climbs against.
//!
//! Every formula cites its source and states its regime. These are the
//! *independent* side of each comparison: none of them touch the grid
//! solver, and the elliptic integrals here are a separate implementation
//! from `vcad_kernel_particle::elliptic` (the loop-field cross-check in
//! `tests/validation.rs` deliberately compares two codebases).

use crate::constants::{EPS_0, MU_0};

/// Complete elliptic integrals `(K(m), E(m))` with parameter `m = k²`,
/// by the arithmetic–geometric mean.
///
/// Abramowitz & Stegun, *Handbook of Mathematical Functions*, §17.6
/// (AGM process, eqs. 17.6.3–17.6.4). Valid for `0 ≤ m < 1`.
pub fn ellip_ke(m: f64) -> (f64, f64) {
    assert!(
        (0.0..1.0).contains(&m),
        "elliptic parameter m in [0,1): {m}"
    );
    let mut a = 1.0_f64;
    let mut b = (1.0 - m).sqrt();
    let mut c2_sum = 0.5 * m; // 2^{n−1}·c_n² accumulated, n = 0 term: c₀ = k.
    let mut pow = 0.5;
    for _ in 0..64 {
        let c = 0.5 * (a - b);
        let an = 0.5 * (a + b);
        b = (a * b).sqrt();
        a = an;
        pow *= 2.0;
        c2_sum += pow * c * c;
        if c.abs() < 1e-17 * a {
            break;
        }
    }
    let k = std::f64::consts::PI / (2.0 * a);
    (k, k * (1.0 - c2_sum))
}

/// Inductance per meter of an infinite solenoid of radius `radius_m` with
/// `n_per_m` turns per meter, optionally holding a coaxial linear core of
/// radius `core_radius_m ≤ radius_m` and relative permeability `mu_r`.
///
/// From Ampère's law, `H = n·I` inside regardless of the core (Griffiths,
/// *Introduction to Electrodynamics*, 4th ed., ex. 5.9 + §6.4):
/// `L/ℓ = μ₀ n² π (μ_r·R_c² + (R² − R_c²))`. Exact — the anchor the
/// solver's Neumann-boundary solenoid must hit.
pub fn solenoid_inductance_per_m(
    n_per_m: f64,
    radius_m: f64,
    core_radius_m: f64,
    mu_r: f64,
) -> f64 {
    assert!(core_radius_m <= radius_m);
    let r2 = radius_m * radius_m;
    let rc2 = core_radius_m * core_radius_m;
    MU_0 * n_per_m * n_per_m * std::f64::consts::PI * (mu_r * rc2 + (r2 - rc2))
}

/// Wheeler's 1928 single-layer air-solenoid formula:
/// `L = μ₀ N² π R² / (ℓ + 0.9·R)`.
///
/// H. A. Wheeler, "Simple Inductance Formulas for Radio Coils,"
/// *Proc. IRE* **16**(10), 1398–1400 (1928). Accuracy ~1% for
/// `ℓ > 0.8·R`; treat as a sanity band, not an exact anchor.
pub fn wheeler_solenoid_inductance(radius_m: f64, length_m: f64, turns: f64) -> f64 {
    MU_0 * turns * turns * std::f64::consts::PI * radius_m * radius_m / (length_m + 0.9 * radius_m)
}

/// Mutual inductance of two coaxial circular filaments of radii `r1_m`,
/// `r2_m` separated axially by `d_m` (Maxwell's formula):
/// `M = μ₀ √(r1·r2) · [(2/k − k)·K(k) − (2/k)·E(k)]` with
/// `k² = 4·r1·r2 / ((r1+r2)² + d²)`.
///
/// Smythe, *Static and Dynamic Electricity*, 3rd ed., §8.06; also
/// Jackson, *Classical Electrodynamics*, 3rd ed., §5.17 exercises.
/// Filaments — no conductor thickness.
pub fn loop_mutual_inductance(r1_m: f64, r2_m: f64, d_m: f64) -> f64 {
    let m = 4.0 * r1_m * r2_m / ((r1_m + r2_m).powi(2) + d_m * d_m);
    let k = m.sqrt();
    let (kk, ee) = ellip_ke(m);
    MU_0 * (r1_m * r2_m).sqrt() * ((2.0 / k - k) * kk - (2.0 / k) * ee)
}

/// `dM/dd` of [`loop_mutual_inductance`] by Richardson-extrapolated central
/// differences **of the closed form** (a smooth analytic function — this is
/// not finite-differencing a discretized solver, so no discretization is
/// being hidden in the step).
pub fn loop_mutual_gradient(r1_m: f64, r2_m: f64, d_m: f64) -> f64 {
    let h = 1e-4 * d_m.abs().max(1e-3);
    let d1 = (loop_mutual_inductance(r1_m, r2_m, d_m + h)
        - loop_mutual_inductance(r1_m, r2_m, d_m - h))
        / (2.0 * h);
    let d2 = (loop_mutual_inductance(r1_m, r2_m, d_m + 0.5 * h)
        - loop_mutual_inductance(r1_m, r2_m, d_m - 0.5 * h))
        / h;
    (4.0 * d2 - d1) / 3.0
}

/// Axial force between two coaxial filament loops carrying `i1_a`, `i2_a`
/// at separation `d_m > 0`: `F_z = i1·i2·dM/dd`, the force **on the upper
/// loop**; negative = attraction (currents circulating the same way).
///
/// Constant-current virtual work on the coenergy, e.g. Smythe §8.08.
pub fn loop_axial_force(r1_m: f64, r2_m: f64, d_m: f64, i1_a: f64, i2_a: f64) -> f64 {
    i1_a * i2_a * loop_mutual_gradient(r1_m, r2_m, d_m)
}

/// On-axis field of a circular filament loop: `B_z = μ₀ I R² / (2·(R²+z²)^{3/2})`.
/// Any EM text; Griffiths 4th ed., ex. 5.6.
pub fn loop_b_axis(radius_m: f64, current_a: f64, z_m: f64) -> f64 {
    let r2 = radius_m * radius_m;
    MU_0 * current_a * r2 / (2.0 * (r2 + z_m * z_m).powf(1.5))
}

/// Capacitance per meter of a coaxial line, inner radius `a_m`, outer
/// radius `b_m`, dielectric `eps_r`: `C′ = 2π·ε₀·ε_r / ln(b/a)`.
/// Griffiths 4th ed., ex. 2.39.
pub fn coax_capacitance_per_m(a_m: f64, b_m: f64, eps_r: f64) -> f64 {
    assert!(b_m > a_m && a_m > 0.0);
    2.0 * std::f64::consts::PI * EPS_0 * eps_r / (b_m / a_m).ln()
}

/// Capacitance of concentric spheres, radii `a_m < b_m`:
/// `C = 4π·ε₀·a·b/(b−a)`. Griffiths 4th ed., ex. 2.40.
pub fn concentric_spheres_capacitance(a_m: f64, b_m: f64) -> f64 {
    assert!(b_m > a_m && a_m > 0.0);
    4.0 * std::f64::consts::PI * EPS_0 * a_m * b_m / (b_m - a_m)
}

/// Capacitance per meter of depth of an ideal parallel-plate pair of width
/// `w_m` and gap `d_m`, fringing-free: `C′ = ε₀·ε_r·w/d`.
pub fn parallel_plate_capacitance_per_m(w_m: f64, d_m: f64, eps_r: f64) -> f64 {
    EPS_0 * eps_r * w_m / d_m
}

/// Inductance per meter of depth of a wide sheet pair (parallel-plate
/// transmission line), sheet width `w_m`, separation `d_m`, carrying equal
/// and opposite currents: `L′ = μ₀·d/w`. Exact in the fringing-free limit
/// `w ≫ d` (uniform field between the sheets) — and exactly realized by
/// the solver's Neumann-side configuration.
pub fn sheet_pair_inductance_per_m(w_m: f64, d_m: f64) -> f64 {
    MU_0 * d_m / w_m
}

/// First-order axial-flux PM motor torque constant
/// `Kt = k_w · N_phase · p · B_gap · A_pole`, `A_pole = π(R_o²−R_i²)/(2p)`
/// — the same formula as `vcad-ecad-sim::magnetics::motor_torque_constant`
/// (crates/vcad-ecad-sim/src/magnetics.rs), reproduced here so the field
/// solution can be compared against the repo's incumbent estimate without
/// a cross-crate dependency. No slotting, no fringing, no end effects.
pub fn motor_kt_first_order(
    pole_pairs: f64,
    turns_per_phase: f64,
    winding_factor: f64,
    airgap_flux_t: f64,
    inner_radius_m: f64,
    outer_radius_m: f64,
) -> f64 {
    let annulus =
        std::f64::consts::PI * (outer_radius_m * outer_radius_m - inner_radius_m * inner_radius_m);
    let pole_area = annulus / (2.0 * pole_pairs);
    winding_factor * turns_per_phase * pole_pairs * airgap_flux_t * pole_area
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elliptic_integrals_match_abramowitz_stegun() {
        // A&S Table 17.1 (m = k²): K(0.5) = 1.85407468, E(0.5) = 1.35064388.
        let (k, e) = ellip_ke(0.5);
        assert!((k - 1.854_074_68).abs() < 1e-7, "K(0.5) = {k}");
        assert!((e - 1.350_643_88).abs() < 1e-7, "E(0.5) = {e}");
        // m = 0: both π/2.
        let (k0, e0) = ellip_ke(0.0);
        assert!((k0 - std::f64::consts::FRAC_PI_2).abs() < 1e-15);
        assert!((e0 - std::f64::consts::FRAC_PI_2).abs() < 1e-15);
        // K diverges toward m = 1, E → 1.
        let (k9, e9) = ellip_ke(0.999_999);
        assert!(k9 > 7.0);
        assert!((e9 - 1.0).abs() < 1e-2);
    }

    #[test]
    fn wheeler_approaches_the_infinite_solenoid_when_long() {
        // ℓ = 20 R: end effects are ~4%, and Wheeler must sit close to the
        // ideal μ₀ n² π R² ℓ.
        let (r, l, n) = (0.01, 0.2, 200.0);
        let ideal = solenoid_inductance_per_m(n / l, r, 0.0, 1.0) * l;
        let wheeler = wheeler_solenoid_inductance(r, l, n);
        let ratio = wheeler / ideal;
        assert!(
            (0.93..1.0).contains(&ratio),
            "wheeler/ideal = {ratio} (wheeler {wheeler:.3e}, ideal {ideal:.3e})"
        );
    }

    #[test]
    fn loop_mutual_has_the_right_asymptotics() {
        // Far apart: M → μ₀ π R₁² R₂² / (2 d³) (coaxial dipole coupling).
        let (r1, r2, d) = (0.03, 0.02, 0.5);
        let m = loop_mutual_inductance(r1, r2, d);
        let dipole = MU_0 * std::f64::consts::PI * r1 * r1 * r2 * r2 / (2.0 * d.powi(3));
        assert!(
            ((m - dipole) / dipole).abs() < 0.01,
            "far-field mutual {m:.4e} vs dipole {dipole:.4e}"
        );
        // And attraction strengthens as loops approach.
        assert!(loop_mutual_inductance(r1, r2, 0.01) > loop_mutual_inductance(r1, r2, 0.02));
    }

    #[test]
    fn loop_force_sign_and_gradient_stability() {
        // Equal currents, same sense: attraction (force on upper loop is
        // −z, i.e. negative at positive separation).
        let f = loop_axial_force(0.03, 0.03, 0.02, 10.0, 10.0);
        assert!(f < 0.0, "same-sense loops must attract: {f}");
        // Richardson gradient consistent with a tiny plain central diff.
        let g = loop_mutual_gradient(0.03, 0.03, 0.02);
        let h = 1e-6;
        let plain = (loop_mutual_inductance(0.03, 0.03, 0.02 + h)
            - loop_mutual_inductance(0.03, 0.03, 0.02 - h))
            / (2.0 * h);
        assert!(((g - plain) / plain).abs() < 1e-6);
    }

    #[test]
    fn capacitance_formulas_limits() {
        // Spheres: b → ∞ recovers the isolated sphere 4πε₀a.
        let iso = concentric_spheres_capacitance(0.01, 1e6);
        assert!(((iso - 4.0 * std::f64::consts::PI * EPS_0 * 0.01) / iso).abs() < 1e-4);
        // Coax grows as the gap narrows.
        assert!(
            coax_capacitance_per_m(0.004, 0.005, 1.0) > coax_capacitance_per_m(0.002, 0.005, 1.0)
        );
    }
}
