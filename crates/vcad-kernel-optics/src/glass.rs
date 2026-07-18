//! Refractive-index models for stock optical glasses.
//!
//! Primary model: the three-term Sellmeier equation
//! n²(λ) = 1 + Σᵢ Bᵢ λ² / (λ² − Cᵢ), λ in **micrometers** — the form used
//! by every Schott datasheet. Coefficients here are the published Schott
//! catalog values (see per-glass citations); the unit tests validate the
//! derived n_d and Abbe number V_d against the catalog headline numbers,
//! so a transcription error in a coefficient fails loudly rather than
//! silently skewing every downstream trace.
//!
//! A [`Glass::Constant`] fallback carries a catalog n_d for glasses whose
//! Sellmeier coefficients are not bundled (usable for single-wavelength
//! paraxial checks only — asking it for dispersion is honest: it has none).

use serde::{Deserialize, Serialize};

use crate::lines;

/// A dispersive medium.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Glass {
    /// Vacuum / air (n = 1 exactly; air's 1.0003 is ignored at M0 — the
    /// convention of catalog prescriptions, which quote n relative to air).
    Air,
    /// Three-term Sellmeier: `b` and `c` per the Schott datasheet form.
    Sellmeier {
        /// Display name (e.g. "N-BK7").
        name: String,
        /// B₁..B₃ (dimensionless).
        b: [f64; 3],
        /// C₁..C₃ (µm²).
        c: [f64; 3],
    },
    /// Constant catalog index (no dispersion — single-wavelength use only).
    Constant {
        /// Display name.
        name: String,
        /// Catalog n_d.
        nd: f64,
    },
}

impl Glass {
    /// Refractive index at wavelength `lambda_um` (micrometers).
    pub fn index(&self, lambda_um: f64) -> f64 {
        match self {
            Glass::Air => 1.0,
            Glass::Sellmeier { b, c, .. } => {
                let l2 = lambda_um * lambda_um;
                let n2 = 1.0
                    + b[0] * l2 / (l2 - c[0])
                    + b[1] * l2 / (l2 - c[1])
                    + b[2] * l2 / (l2 - c[2]);
                n2.sqrt()
            }
            Glass::Constant { nd, .. } => *nd,
        }
    }

    /// Abbe number V_d = (n_d − 1)/(n_F − n_C).
    ///
    /// For [`Glass::Constant`] this is `f64::INFINITY` (no dispersion) —
    /// deliberately absurd rather than silently plausible.
    pub fn abbe_number(&self) -> f64 {
        let nd = self.index(lines::D);
        let nf = self.index(lines::F);
        let nc = self.index(lines::C);
        if nf == nc {
            f64::INFINITY
        } else {
            (nd - 1.0) / (nf - nc)
        }
    }

    /// Display name.
    pub fn name(&self) -> &str {
        match self {
            Glass::Air => "air",
            Glass::Sellmeier { name, .. } | Glass::Constant { name, .. } => name,
        }
    }

    /// Schott N-BK7 (the workhorse crown).
    ///
    /// Sellmeier coefficients from the Schott N-BK7 datasheet
    /// (catalog headline: n_d = 1.5168, V_d = 64.17).
    pub fn n_bk7() -> Glass {
        Glass::Sellmeier {
            name: "N-BK7".to_string(),
            b: [1.039_612_12, 0.231_792_344, 1.010_469_45],
            c: [0.006_000_698_67, 0.020_017_914_4, 103.560_653],
        }
    }

    /// Schott F2 (classic flint).
    ///
    /// Sellmeier coefficients from the Schott F2 datasheet
    /// (catalog headline: n_d = 1.62004, V_d = 36.37).
    pub fn f2() -> Glass {
        Glass::Sellmeier {
            name: "F2".to_string(),
            b: [1.345_333_59, 0.209_073_176, 0.937_357_162],
            c: [0.009_977_438_71, 0.047_045_076_7, 111.886_764],
        }
    }

    /// Schott N-SF11 (dense flint).
    ///
    /// Sellmeier coefficients from the Schott N-SF11 datasheet
    /// (catalog headline: n_d = 1.78472, V_d = 25.68).
    pub fn n_sf11() -> Glass {
        Glass::Sellmeier {
            name: "N-SF11".to_string(),
            b: [1.737_596_95, 0.313_747_346, 1.898_781_01],
            c: [0.013_188_707, 0.062_306_814_2, 155.236_29],
        }
    }

    /// Schott SF5 (classic lead flint, used in Thorlabs "-A" achromats).
    ///
    /// Sellmeier coefficients from the Schott SF5 datasheet
    /// (catalog headline: n_d = 1.67271, V_d = 32.21).
    pub fn sf5() -> Glass {
        Glass::Sellmeier {
            name: "SF5".to_string(),
            b: [1.524_818_89, 0.187_085_527, 1.427_290_15],
            c: [0.011_254_756, 0.058_899_539_2, 129.141_675],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Catalog headline values gate the bundled Sellmeier coefficients:
    /// a transcription error in any coefficient shifts n_d or V_d and
    /// fails here, rather than silently mis-dispersing every trace.
    #[test]
    fn sellmeier_recovers_catalog_nd_and_abbe() {
        for (glass, nd, vd) in [
            (Glass::n_bk7(), 1.5168, 64.17),
            (Glass::f2(), 1.620_04, 36.37),
            (Glass::n_sf11(), 1.784_72, 25.68),
            (Glass::sf5(), 1.672_71, 32.21),
        ] {
            let got_nd = glass.index(lines::D);
            let got_vd = glass.abbe_number();
            assert!(
                (got_nd - nd).abs() < 3e-4,
                "{}: n_d = {got_nd}, catalog {nd}",
                glass.name()
            );
            assert!(
                (got_vd - vd).abs() < 0.3,
                "{}: V_d = {got_vd}, catalog {vd}",
                glass.name()
            );
        }
    }

    #[test]
    fn normal_dispersion_holds() {
        // n_F > n_d > n_C for every bundled glass (normal dispersion in
        // the visible).
        for glass in [Glass::n_bk7(), Glass::f2(), Glass::n_sf11(), Glass::sf5()] {
            let (nf, nd, nc) = (
                glass.index(lines::F),
                glass.index(lines::D),
                glass.index(lines::C),
            );
            assert!(nf > nd && nd > nc, "{}: {nf} {nd} {nc}", glass.name());
        }
    }

    #[test]
    fn air_is_exactly_one_and_dispersionless() {
        assert_eq!(Glass::Air.index(0.4), 1.0);
        assert_eq!(Glass::Air.index(0.7), 1.0);
    }

    #[test]
    fn constant_glass_reports_infinite_abbe() {
        let g = Glass::Constant {
            name: "X".into(),
            nd: 1.6,
        };
        assert_eq!(g.index(0.5), 1.6);
        assert!(g.abbe_number().is_infinite());
    }
}
