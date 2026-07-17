//! Fusion cross sections: the Bosch–Hale parameterization for the two D-D
//! branches (Bosch & Hale 1992, Nucl. Fusion 32, 611).
//!
//! σ(E) = S(E) / (E · exp(B_G/√E)), with S a polynomial in the
//! center-of-mass energy E (keV) and B_G the Gamow constant. Valid
//! 0.5–4900 keV; below the validity floor this module returns 0 (the true
//! value there is astronomically small).

/// Gamow constant for D-D, √keV.
const B_G_DD: f64 = 31.397;

/// Millibarn → m².
const MB_TO_M2: f64 = 1e-31;

#[inline]
fn bosch_hale_sigma_mb(e_cm_kev: f64, a: [f64; 5]) -> f64 {
    if e_cm_kev < 0.5 {
        return 0.0;
    }
    let e = e_cm_kev;
    let s = a[0] + e * (a[1] + e * (a[2] + e * (a[3] + e * a[4])));
    s / (e * (B_G_DD / e.sqrt()).exp())
}

/// D(d,n)³He cross section, m², at center-of-mass energy `e_cm_kev`.
///
/// This is the neutron branch (2.45 MeV neutrons) — the one a neutron
/// counter sees.
pub fn dd_n_sigma_m2(e_cm_kev: f64) -> f64 {
    bosch_hale_sigma_mb(
        e_cm_kev,
        [5.3701e4, 3.3027e2, -1.2706e-1, 2.9327e-5, -2.5151e-9],
    ) * MB_TO_M2
}

/// D(d,p)T cross section, m², at center-of-mass energy `e_cm_kev`.
pub fn dd_p_sigma_m2(e_cm_kev: f64) -> f64 {
    bosch_hale_sigma_mb(
        e_cm_kev,
        [5.5576e4, 2.1054e2, -3.2638e-2, 1.4987e-6, 1.8181e-10],
    ) * MB_TO_M2
}

/// Deuteron number density of D₂ gas at `pressure_mtorr` and
/// `temperature_k`, m⁻³ (two deuterons per molecule).
pub fn d2_deuteron_density_m3(pressure_mtorr: f64, temperature_k: f64) -> f64 {
    let pa = pressure_mtorr * 0.133_322;
    2.0 * pa / (1.380_649e-23 * temperature_k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_published_anchor_values() {
        // σ_DDn at E_cm = 50 keV (E_lab = 100 keV) ≈ 16.5 mb.
        let s50 = dd_n_sigma_m2(50.0) / MB_TO_M2;
        assert!((14.0..19.0).contains(&s50), "sigma_ddn(50 keV) = {s50} mb");
        // σ_DDn at E_cm = 15 keV ≈ 1.2 mb.
        let s15 = dd_n_sigma_m2(15.0) / MB_TO_M2;
        assert!((0.9..1.5).contains(&s15), "sigma_ddn(15 keV) = {s15} mb");
        // Both branches are within ~2x of each other at fusor energies.
        let p50 = dd_p_sigma_m2(50.0) / MB_TO_M2;
        assert!(
            (0.5..2.0).contains(&(p50 / s50)),
            "branch ratio off: n {s50} mb vs p {p50} mb"
        );
    }

    #[test]
    fn rises_steeply_through_fusor_energies() {
        let mut last = 0.0;
        for e in [1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 250.0] {
            let s = dd_n_sigma_m2(e);
            assert!(s > last, "sigma must rise with energy at {e} keV");
            last = s;
        }
        // The steepness that makes voltage worth chasing: 15 → 30 keV CM
        // buys more than 4x.
        assert!(dd_n_sigma_m2(30.0) / dd_n_sigma_m2(15.0) > 4.0);
    }

    #[test]
    fn below_validity_floor_is_zero() {
        assert_eq!(dd_n_sigma_m2(0.3), 0.0);
        assert_eq!(dd_p_sigma_m2(0.0), 0.0);
        assert_eq!(dd_n_sigma_m2(-5.0), 0.0);
    }

    #[test]
    fn gas_density_is_physical() {
        // 1 mTorr D₂ at 300 K ≈ 3.2e19 molecules/m³ → 6.4e19 deuterons/m³.
        let n = d2_deuteron_density_m3(1.0, 300.0);
        assert!((6.0e19..7.0e19).contains(&n), "n = {n}");
    }
}
