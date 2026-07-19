//! Predicted-performance claims for the design receipt.
//!
//! Emits a serializable claim set (`vcad.optics-claims/1`) — EFL, back
//! focal distance, f-number, per-field polychromatic RMS spot, chromatic
//! focal shift — with full trace provenance, in the spirit of
//! `vcad.receipt/1`: every number carries how it was produced.
//!
//! These are `basis: "predicted"` claims: [`design_claims`] translates
//! them into the unified [`vcad_receipt::DesignReceipt`] vocabulary as
//! [`vcad_receipt::ClaimBasis::Predicted`] — a receipt built from them
//! **rolls up Provisional, never Pass** (the same contract as the
//! particle/em/thermal families). Binding them to bench measurements
//! (focimeter EFL, beam-profiler spot) is a later milestone.
//!
//! Fail-closed rules encoded here:
//! - a [`crate::spot::SpotAnalysis`] containing TIR or surface-miss rays
//!   **refuses to become claims** — a broken trace never prices a design;
//! - every spot claim carries the Airy radius next to it: RMS spot is a
//!   *geometric* claim, and below the diffraction limit the geometric
//!   number is context, not performance.

use serde::{Deserialize, Serialize};

use crate::paraxial::{first_order, FirstOrder};
use crate::prescription::Prescription;
use crate::spot::{airy_radius_um, SpotAnalysis};

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.optics-claims/1";

/// Domain tag for optics claims in the unified receipt schema.
pub const RECEIPT_DOMAIN: &str = "optics";

/// How the numbers were produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceProvenance {
    /// Number of surfaces in the prescription.
    pub n_surfaces: usize,
    /// Pupil rings per bundle.
    pub pupil_rings: usize,
    /// Rays launched per (field, wavelength) bundle.
    pub rays_per_bundle: usize,
    /// Entrance-pupil radius sampled, mm.
    pub pupil_radius_mm: f64,
    /// Wavelengths traced, µm.
    pub wavelengths_um: Vec<f64>,
    /// Field angles traced, degrees.
    pub fields_deg: Vec<f64>,
    /// Image-plane z (global), mm — the paraxial focus at the reference
    /// wavelength.
    pub image_z_mm: f64,
    /// Worst Snell-invariant residual across every traced ray (exactness
    /// diagnostic; the trace is closed-form, so this sits near 1e-15).
    pub max_snell_residual: f64,
    /// Fraction of launched rays vignetted (reported, never hidden).
    pub vignetted_fraction: f64,
}

/// One predicted claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Claim name (snake_case).
    pub name: String,
    /// Value.
    pub value: f64,
    /// Unit ("1" for dimensionless).
    pub unit: String,
    /// Claim basis — always `"predicted"` here.
    pub basis: String,
    /// Assumptions and caveats, spelled out.
    pub note: String,
}

/// The full claim set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimSet {
    /// Schema tag ([`CLAIM_SCHEMA`]).
    pub schema: String,
    /// Trace provenance.
    pub provenance: TraceProvenance,
    /// The claims.
    pub claims: Vec<Claim>,
}

/// Why a claim set could not be built (fail-closed).
#[derive(Debug, Clone, PartialEq)]
pub enum ClaimError {
    /// The bundle contained rays lost to TIR or surface misses — a
    /// design whose trace breaks is not priced, it is rejected.
    HardRayFailures(usize),
    /// No finite paraxial focus (afocal/diverging system).
    NoParaxialFocus,
    /// A field's polychromatic RMS was unavailable.
    MissingSpot(usize),
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimError::HardRayFailures(n) => {
                write!(
                    f,
                    "{n} rays lost to TIR/miss — refusing to price a broken trace"
                )
            }
            ClaimError::NoParaxialFocus => write!(f, "no finite paraxial focus"),
            ClaimError::MissingSpot(i) => write!(f, "field {i}: no imaged rays"),
        }
    }
}

impl std::error::Error for ClaimError {}

fn claim(name: String, value: f64, unit: &str, note: String) -> Claim {
    Claim {
        name,
        value,
        unit: unit.to_string(),
        basis: "predicted".to_string(),
        note,
    }
}

/// Build the predicted claim set for one analyzed prescription.
///
/// `reference_lambda_um` prices EFL/BFD and the Airy context;
/// `analysis` must have been produced at the paraxial image plane of the
/// same prescription.
pub fn predicted_claims(
    presc: &Prescription,
    analysis: &SpotAnalysis,
    reference_lambda_um: f64,
) -> Result<ClaimSet, ClaimError> {
    let hard = analysis.hard_failures();
    if hard > 0 {
        return Err(ClaimError::HardRayFailures(hard));
    }
    let fo: FirstOrder =
        first_order(presc, reference_lambda_um).ok_or(ClaimError::NoParaxialFocus)?;

    let f_number = fo.efl_mm / (2.0 * analysis.pupil_radius_mm);
    let airy = airy_radius_um(reference_lambda_um, f_number);

    let mut claims = vec![
        claim(
            "efl_mm".into(),
            fo.efl_mm,
            "mm",
            format!("paraxial effective focal length at {reference_lambda_um} um"),
        ),
        claim(
            "bfd_mm".into(),
            fo.bfd_mm,
            "mm",
            format!("paraxial back focal distance at {reference_lambda_um} um"),
        ),
        claim(
            "f_number".into(),
            f_number,
            "1",
            "EFL over sampled entrance-pupil diameter".into(),
        ),
        claim(
            "airy_radius_um".into(),
            airy,
            "um",
            "1.22*lambda*N diffraction context — geometric spot claims below \
             this number mean diffraction-limited, not smaller"
                .into(),
        ),
    ];

    for (i, (field, rms)) in analysis
        .fields_deg
        .iter()
        .zip(&analysis.poly_rms_um)
        .enumerate()
    {
        let rms = rms.ok_or(ClaimError::MissingSpot(i))?;
        claims.push(claim(
            format!("rms_spot_um_field_{i}"),
            rms,
            "um",
            format!(
                "polychromatic geometric RMS spot radius at {field} deg field, \
                 wavelengths {:?} um, at the reference-wavelength paraxial focus; \
                 geometric claim only — no diffraction",
                analysis.wavelengths_um
            ),
        ));
    }

    // Chromatic focal shift across the traced wavelength extremes.
    if analysis.wavelengths_um.len() > 1 {
        let mut lams = analysis.wavelengths_um.clone();
        lams.sort_by(f64::total_cmp);
        let (blue, red) = (lams[0], lams[lams.len() - 1]);
        if let (Some(fb), Some(fr)) = (first_order(presc, blue), first_order(presc, red)) {
            claims.push(claim(
                "chromatic_focal_shift_mm".into(),
                fr.bfd_mm - fb.bfd_mm,
                "mm",
                format!("paraxial BFD({red} um) - BFD({blue} um); thin-lens theory gives f/V"),
            ));
        }
    }

    Ok(ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: TraceProvenance {
            n_surfaces: presc.surfaces.len(),
            pupil_rings: analysis.pupil_rings,
            rays_per_bundle: analysis
                .results
                .first()
                .map(|r| r.n_imaged + r.n_vignetted + r.n_tir + r.n_missed)
                .unwrap_or(0),
            pupil_radius_mm: analysis.pupil_radius_mm,
            wavelengths_um: analysis.wavelengths_um.clone(),
            fields_deg: analysis.fields_deg.clone(),
            image_z_mm: analysis.image_z_mm,
            max_snell_residual: analysis.max_snell_residual,
            vignetted_fraction: analysis.vignetted_fraction(),
        },
        claims,
    })
}

/// The oracle reference for this crate's tracer.
pub fn oracle() -> vcad_receipt::OracleRef {
    vcad_receipt::OracleRef::new("vcad-kernel-optics/trace", env!("CARGO_PKG_VERSION"))
}

fn quantity(value: f64, unit: &str) -> vcad_receipt::ClaimQuantity {
    if unit == "1" {
        vcad_receipt::ClaimQuantity::bare(value)
    } else {
        vcad_receipt::ClaimQuantity::new(value, unit)
    }
}

/// Translate a predicted [`ClaimSet`] into unified-receipt claims.
///
/// Every claim lands with [`vcad_receipt::ClaimBasis::Predicted`] — the
/// tracer ran for real, but the claim is about a physical lens that has
/// not been measured, so a receipt built from these **rolls up
/// Provisional, never Pass**.
pub fn design_claims(set: &ClaimSet) -> Vec<vcad_receipt::ReceiptClaim> {
    let oracle = oracle();
    let provenance = format!(
        "{} surfaces, {} rays/bundle over {} rings, pupil r = {} mm, wavelengths {:?} um, \
         fields {:?} deg, image plane z = {:.4} mm, max Snell residual {:.2e}, \
         vignetted fraction {:.4}",
        set.provenance.n_surfaces,
        set.provenance.rays_per_bundle,
        set.provenance.pupil_rings,
        set.provenance.pupil_radius_mm,
        set.provenance.wavelengths_um,
        set.provenance.fields_deg,
        set.provenance.image_z_mm,
        set.provenance.max_snell_residual,
        set.provenance.vignetted_fraction,
    );
    set.claims
        .iter()
        .map(|c| {
            vcad_receipt::ReceiptClaim::pass(
                format!("optics.{}", c.name),
                RECEIPT_DOMAIN,
                c.note.clone(),
                oracle.clone(),
            )
            .with_basis(vcad_receipt::ClaimBasis::Predicted)
            .with_measured(quantity(c.value, &c.unit))
            .with_details(provenance.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glass::Glass;
    use crate::prescription::{Prescription, Surface};
    use crate::spot::analyze;

    fn doublet() -> Prescription {
        Prescription::new(vec![
            Surface::sphere(62.8, 12.7, 4.0, Glass::n_bk7()),
            Surface::sphere(-45.7, 12.7, 2.5, Glass::sf5()),
            Surface::sphere(-128.2, 12.7, 0.0, Glass::Air),
        ])
        .unwrap()
    }

    #[test]
    fn claims_carry_airy_context_and_provenance() {
        let p = doublet();
        let fo = first_order(&p, crate::lines::D).unwrap();
        let a = analyze(
            &p,
            5.0,
            6,
            &[0.0],
            &[crate::lines::F, crate::lines::D, crate::lines::C],
            fo.image_z_mm,
        );
        let set = predicted_claims(&p, &a, crate::lines::D).unwrap();
        assert_eq!(set.schema, CLAIM_SCHEMA);
        let names: Vec<&str> = set.claims.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"efl_mm"));
        assert!(names.contains(&"airy_radius_um"));
        assert!(names.contains(&"rms_spot_um_field_0"));
        assert!(names.contains(&"chromatic_focal_shift_mm"));
        assert!(set.claims.iter().all(|c| c.basis == "predicted"));
        assert!(set.provenance.max_snell_residual < 1e-12);
    }

    #[test]
    fn broken_traces_refuse_to_become_claims() {
        // Steep dense-flint hemisphere: marginal rays TIR at the flat
        // exit face. Claims must refuse, not summarize around the loss.
        let p = Prescription::new(vec![
            Surface::sphere(14.5, 14.0, 10.0, Glass::n_sf11()),
            Surface::sphere(f64::INFINITY, 14.0, 0.0, Glass::Air),
        ])
        .unwrap();
        let a = analyze(&p, 14.0, 8, &[0.0], &[crate::lines::D], 40.0);
        assert!(a.hard_failures() > 0);
        assert!(matches!(
            predicted_claims(&p, &a, crate::lines::D),
            Err(ClaimError::HardRayFailures(_))
        ));
    }

    #[test]
    fn design_claims_are_predicted_basis() {
        let p = doublet();
        let fo = first_order(&p, crate::lines::D).unwrap();
        let a = analyze(&p, 5.0, 6, &[0.0], &[crate::lines::D], fo.image_z_mm);
        let set = predicted_claims(&p, &a, crate::lines::D).unwrap();
        let claims = design_claims(&set);
        assert!(!claims.is_empty());
        for c in &claims {
            assert_eq!(c.basis, Some(vcad_receipt::ClaimBasis::Predicted));
            assert!(c.id.starts_with("optics."));
        }
    }
}
