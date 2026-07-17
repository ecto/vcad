//! Exact sensitivities: the design compass.
//!
//! For a linear chain G = Σ aᵢxᵢ these are **closed forms, not
//! adjoints, not finite differences** — linearity hands us every
//! derivative exactly, and we take the win:
//!
//! - ∂G/∂nominalᵢ = aᵢ
//! - ∂σ_G/∂σᵢ = aᵢ²σᵢ/σ_G          (from σ_G² = Σ aᵢ²σᵢ²)
//! - variance share = aᵢ²σᵢ²/σ_G²   (Σ over i = 1, the ranking metric)
//! - ∂Y/∂μ_G = [φ(z_L) − φ(z_U)]/σ_G, chained: ∂Y/∂nominalᵢ = aᵢ·∂Y/∂μ_G
//! - ∂Y/∂σ_G = [z_L·φ(z_L) − z_U·φ(z_U)]/σ_G, chained through ∂σ_G/∂σᵢ
//!
//! where Y = Φ(z_U) − Φ(z_L), z = (limit − μ_G)/σ_G, φ the standard
//! normal PDF (absent limits contribute 0). The yield derivatives
//! inherit the RSS normality assumption; the moment derivatives do not.
//!
//! Rows are ranked by variance share: the first row is the dimension
//! that is killing the yield.

use serde::{Deserialize, Serialize};

use crate::capability::phi_pdf;
use crate::stackup::{Stackup, StackupError};

/// Exact sensitivities for one contributor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensitivityRow {
    /// Contributor name.
    pub name: String,
    /// ∂gap/∂nominal = the chain coefficient aᵢ (mm/mm). Exact.
    pub d_gap_d_nominal: f64,
    /// Contributor standard deviation σᵢ, mm.
    pub sigma: f64,
    /// ∂σ_G/∂σᵢ = aᵢ²σᵢ/σ_G (mm/mm). Exact.
    pub d_sigma_gap_d_sigma: f64,
    /// Share of gap variance: aᵢ²σᵢ²/σ_G², in [0, 1]. The ranking
    /// metric.
    pub variance_share: f64,
    /// ∂yield/∂nominalᵢ (per mm). Exact under the RSS normal-gap model.
    /// The re-centering compass: move nominals down its gradient.
    pub d_yield_d_nominal: f64,
    /// ∂yield/∂σᵢ (per mm of σ). Exact under the RSS normal-gap model;
    /// ≤ 0 always (scatter never helps). The allocation compass.
    pub d_yield_d_sigma: f64,
    /// Worst-case span |aᵢ|·(tol₋ + tol₊): this contributor's share of
    /// the worst-case interval width, mm.
    pub wc_span: f64,
}

/// Compute exact sensitivities, ranked by variance share (descending).
pub fn sensitivities(s: &Stackup) -> Result<Vec<SensitivityRow>, StackupError> {
    s.validate()?;
    let var_g = s.variance_gap();
    if var_g == 0.0 {
        return Err(StackupError::DegenerateChain);
    }
    let sigma_g = var_g.sqrt();
    let mean_g = s.mean_gap();

    // ∂Y/∂μ_G and ∂Y/∂σ_G from the Φ-based yield.
    let z_l = s.requirement.lower_mm.map(|l| (l - mean_g) / sigma_g);
    let z_u = s.requirement.upper_mm.map(|u| (u - mean_g) / sigma_g);
    let pdf_l = z_l.map_or(0.0, phi_pdf);
    let pdf_u = z_u.map_or(0.0, phi_pdf);
    let dy_dmean = (pdf_l - pdf_u) / sigma_g;
    let dy_dsigma_g =
        (z_l.map_or(0.0, |z| z * phi_pdf(z)) - z_u.map_or(0.0, |z| z * phi_pdf(z))) / sigma_g;

    let mut rows: Vec<SensitivityRow> = s
        .contributors
        .iter()
        .map(|c| {
            let sigma_i = c.dist.sigma();
            let a2 = c.coeff * c.coeff;
            let d_sigma_gap_d_sigma = a2 * sigma_i / sigma_g;
            SensitivityRow {
                name: c.name.clone(),
                d_gap_d_nominal: c.coeff,
                sigma: sigma_i,
                d_sigma_gap_d_sigma,
                variance_share: a2 * sigma_i * sigma_i / var_g,
                d_yield_d_nominal: c.coeff * dy_dmean,
                d_yield_d_sigma: dy_dsigma_g * d_sigma_gap_d_sigma,
                wc_span: c.coeff.abs() * (c.tol_minus + c.tol_plus),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.variance_share
            .partial_cmp(&a.variance_share)
            .expect("variance shares are finite")
    });
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::rss;
    use crate::dist::SigmaConvention;
    use crate::stackup::{Contributor, Requirement};

    fn chain() -> Stackup {
        Stackup {
            name: "s".into(),
            contributors: vec![
                Contributor::normal("big", 1.0, 50.0, 0.3, SigmaConvention::ThreeSigma),
                Contributor::normal("small", -1.0, 20.0, 0.1, SigmaConvention::ThreeSigma),
                Contributor::normal("lever", 2.0, 5.0, 0.06, SigmaConvention::ThreeSigma),
            ],
            requirement: Requirement::between("gap", 39.5, 40.5),
        }
    }

    #[test]
    fn closed_forms_are_exact() {
        let s = chain();
        let rows = sensitivities(&s).unwrap();
        // σᵢ: 0.1, 0.0333.., 0.02; aᵢ²σᵢ²: 0.01, 0.001111, 0.0016.
        let var_g: f64 = 0.01 + 0.1f64 / 3.0 * (0.1 / 3.0) + 4.0 * 0.02 * 0.02;
        let sigma_g = var_g.sqrt();
        // Ranked: big, lever, small.
        assert_eq!(rows[0].name, "big");
        assert_eq!(rows[1].name, "lever");
        assert_eq!(rows[2].name, "small");
        assert!((rows[0].variance_share - 0.01 / var_g).abs() < 1e-12);
        assert!((rows[1].variance_share - 0.0016 / var_g).abs() < 1e-12);
        // Shares sum to 1.
        let total: f64 = rows.iter().map(|r| r.variance_share).sum();
        assert!((total - 1.0).abs() < 1e-12);
        // ∂σ_G/∂σᵢ exact.
        assert!((rows[0].d_sigma_gap_d_sigma - 0.1 / sigma_g).abs() < 1e-12);
        assert!((rows[1].d_sigma_gap_d_sigma - 4.0 * 0.02 / sigma_g).abs() < 1e-12);
        // ∂gap/∂nominal is the coefficient.
        assert_eq!(rows[1].d_gap_d_nominal, 2.0);
        // WC spans.
        assert!((rows[1].wc_span - 2.0 * 0.12).abs() < 1e-12);
    }

    #[test]
    fn yield_derivatives_match_finite_differences() {
        // The exact forms must agree with FD on the Φ-based yield —
        // this validates the chain rule, not an approximation of it.
        let s = chain();
        let rows = sensitivities(&s).unwrap();
        let base = rss(&s).unwrap().yield_estimate;
        let h = 1e-6;
        for row in &rows {
            // Nominal FD.
            let mut sp = s.clone();
            let c = sp
                .contributors
                .iter_mut()
                .find(|c| c.name == row.name)
                .unwrap();
            c.nominal += h;
            let fd = (rss(&sp).unwrap().yield_estimate - base) / h;
            assert!(
                (fd - row.d_yield_d_nominal).abs() < 1e-4 * (1.0 + fd.abs()),
                "{}: fd {fd} vs exact {}",
                row.name,
                row.d_yield_d_nominal
            );
            // Sigma FD.
            let mut sp = s.clone();
            let c = sp
                .contributors
                .iter_mut()
                .find(|c| c.name == row.name)
                .unwrap();
            if let crate::dist::Distribution::Normal { sigma, .. } = &mut c.dist {
                *sigma += h;
            }
            let fd = (rss(&sp).unwrap().yield_estimate - base) / h;
            assert!(
                (fd - row.d_yield_d_sigma).abs() < 1e-4 * (1.0 + fd.abs()),
                "{}: fd {fd} vs exact {}",
                row.name,
                row.d_yield_d_sigma
            );
            // Scatter never helps.
            assert!(row.d_yield_d_sigma <= 0.0);
        }
    }

    #[test]
    fn one_sided_requirement_yields_signed_compass() {
        // Clearance-only requirement: increasing the gap-opening nominal
        // must increase yield; increasing a consuming nominal must
        // decrease it.
        let mut s = chain();
        s.requirement = Requirement::at_least("clearance", 39.9);
        let rows = sensitivities(&s).unwrap();
        let big = rows.iter().find(|r| r.name == "big").unwrap();
        let small = rows.iter().find(|r| r.name == "small").unwrap();
        assert!(big.d_yield_d_nominal > 0.0);
        assert!(small.d_yield_d_nominal < 0.0);
    }
}
