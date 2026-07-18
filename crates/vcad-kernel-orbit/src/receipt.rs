//! Orbit claims for the design receipt — `vcad.orbit-claims/1`.
//!
//! Two kinds of claim, mirroring the particle crate's contract but with
//! one first: because the ground truth here is public ephemeris data, the
//! position-error claim is born **measured** — the first claim family in
//! the workspace whose Measured basis is fed by real sky data instead of
//! bench hardware.
//!
//! - **Predicted claims** ([`predicted_claims`]): orbital period, J2
//!   secular rates, pass count/first-pass window. `basis: "predicted"` —
//!   a receipt built from them rolls up Provisional, never Pass.
//! - **The sky-measured claim** ([`sky_comparison_claim`]): propagated
//!   position error against a checked-in ephemeris at a stated horizon,
//!   judged against a stated error budget. Holds → Pass, exceeds → Fail,
//!   `basis: measured`. Fail-closed: no ephemeris, no Pass.

use serde::{Deserialize, Serialize};

use crate::pass::Pass;
use crate::secular::{apsidal_rate_deg_per_day, nodal_rate_deg_per_day};
use crate::state::OrbitalElements;

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.orbit-claims/1";

/// Domain tag in the unified `vcad.receipt/1` schema.
pub const RECEIPT_DOMAIN: &str = "orbit";

/// How the numbers were produced — the frame/time/force honesty block
/// that rides on every claim set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolverProvenance {
    /// Integrator ("rk4-fixed") and step, seconds.
    pub integrator: String,
    /// RK4 step size, seconds.
    pub step_s: f64,
    /// Force model ("two-body+J2").
    pub force_model: String,
    /// Frame statement (ICRF treated as inertial Earth-equator; GMST-only
    /// rotation; TDB−UTC constant offset). Spelled out, never defaulted.
    pub frame_note: String,
    /// Ephemeris fixture identifier, when a sky comparison was run.
    pub ephemeris_id: Option<String>,
}

/// One claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Claim name (snake_case).
    pub name: String,
    /// Value.
    pub value: f64,
    /// Unit ("1" for dimensionless).
    pub unit: String,
    /// "predicted" or "measured".
    pub basis: String,
    /// Assumptions and caveats, spelled out.
    pub note: String,
}

/// The full claim set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimSet {
    /// Schema tag ([`CLAIM_SCHEMA`]).
    pub schema: String,
    /// Provenance.
    pub provenance: SolverProvenance,
    /// The claims.
    pub claims: Vec<Claim>,
}

fn claim(name: &str, value: f64, unit: &str, basis: &str, note: &str) -> Claim {
    Claim {
        name: name.to_string(),
        value,
        unit: unit.to_string(),
        basis: basis.to_string(),
        note: note.to_string(),
    }
}

/// Build the predicted claim set for an orbit and (optionally) a pass
/// forecast over one site.
pub fn predicted_claims(
    el: &OrbitalElements,
    passes: Option<&[Pass]>,
    provenance: SolverProvenance,
) -> ClaimSet {
    let mut claims = vec![
        claim(
            "period_s",
            el.period_s(),
            "s",
            "predicted",
            "osculating two-body period 2π√(a³/μ); J2 nodal/anomalistic \
             period corrections are not applied at M0",
        ),
        claim(
            "nodal_rate_deg_per_day",
            nodal_rate_deg_per_day(el),
            "deg/day",
            "predicted",
            "first-order J2 secular theory (Vallado Eq. 9-38); validated \
             against the numeric propagator in-crate",
        ),
        claim(
            "apsidal_rate_deg_per_day",
            apsidal_rate_deg_per_day(el),
            "deg/day",
            "predicted",
            "first-order J2 secular theory (Vallado Eq. 9-39)",
        ),
    ];
    if let Some(passes) = passes {
        claims.push(claim(
            "pass_count",
            passes.len() as f64,
            "passes",
            "predicted",
            "complete rise→set windows above the stated mask in the \
             forecast window; J2-only + GMST-only ⇒ times good to \
             ±minutes, not ±seconds",
        ));
        if let Some(first) = passes.first() {
            claims.push(claim(
                "first_pass_rise_jd_utc",
                first.rise_jd_utc,
                "jd",
                "predicted",
                "rise time of the first predicted pass (JD UTC)",
            ));
            claims.push(claim(
                "first_pass_max_elevation_deg",
                first.max_elevation_rad.to_degrees(),
                "deg",
                "predicted",
                "maximum elevation of the first predicted pass",
            ));
        }
    }
    ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance,
        claims,
    }
}

/// The sky comparison: propagated-vs-ephemeris position error at a
/// horizon, judged against a stated budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkyComparison {
    /// Comparison horizon, hours after the ephemeris initial state.
    pub horizon_h: f64,
    /// Propagated-minus-ephemeris position error at the horizon, km.
    pub position_error_km: f64,
    /// Stated model-gap budget, km. Exceeding it fails the claim.
    pub budget_km: f64,
    /// Fixture identifier (file name of the checked-in ephemeris).
    pub ephemeris_id: String,
}

/// Oracle reference for this crate's propagator.
pub fn oracle() -> vcad_receipt::OracleRef {
    vcad_receipt::OracleRef::new("vcad-kernel-orbit/propagate", env!("CARGO_PKG_VERSION"))
}

fn quantity(value: f64, unit: &str) -> vcad_receipt::ClaimQuantity {
    if unit == "1" {
        vcad_receipt::ClaimQuantity::bare(value)
    } else {
        vcad_receipt::ClaimQuantity::new(value, unit)
    }
}

/// Translate predicted claims into the unified receipt. All land with
/// `ClaimBasis::Predicted`; a receipt built only from them rolls up
/// **Provisional, never Pass** (same contract as every solver crate).
pub fn design_claims(set: &ClaimSet) -> Vec<vcad_receipt::ReceiptClaim> {
    let oracle = oracle();
    let provenance = format!(
        "{} step {} s, {}; {}",
        set.provenance.integrator,
        set.provenance.step_s,
        set.provenance.force_model,
        set.provenance.frame_note
    );
    set.claims
        .iter()
        .map(|c| {
            vcad_receipt::ReceiptClaim::pass(
                format!("orbit.{}", c.name),
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

/// Translate the sky comparison into a unified-receipt claim with
/// `ClaimBasis::Measured`: the ephemeris is real observation-derived sky
/// data, so this is a measured verdict — Pass when the error is inside
/// the stated budget, Fail when it is not. There is no third outcome.
pub fn sky_comparison_claim(cmp: &SkyComparison) -> vcad_receipt::ReceiptClaim {
    let oracle = oracle();
    let id = format!("orbit.position_error_km_at_{:.0}h", cmp.horizon_h);
    let note = format!(
        "propagated (two-body+J2) vs {} at {:.0} h; budget {} km states the \
         accepted model gap (drag/harmonics unmodeled at M0)",
        cmp.ephemeris_id, cmp.horizon_h, cmp.budget_km
    );
    let base = if cmp.position_error_km <= cmp.budget_km {
        vcad_receipt::ReceiptClaim::pass(id, RECEIPT_DOMAIN, note, oracle)
    } else {
        vcad_receipt::ReceiptClaim::fail(id, RECEIPT_DOMAIN, note, oracle)
    };
    base.with_basis(vcad_receipt::ClaimBasis::Measured)
        .with_predicted(quantity(cmp.budget_km, "km"))
        .with_measured(quantity(cmp.position_error_km, "km"))
        .with_details(format!(
            "sky truth: JPL Horizons fixture {}, geocentric ICRF, TDB \
             timestamps",
            cmp.ephemeris_id
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iss_elements() -> OrbitalElements {
        OrbitalElements {
            a: 6795.0,
            e: 0.0007,
            i: 51.63_f64.to_radians(),
            raan: 2.6,
            argp: 5.3,
            nu: 0.0,
        }
    }

    fn provenance() -> SolverProvenance {
        SolverProvenance {
            integrator: "rk4-fixed".into(),
            step_s: 10.0,
            force_model: "two-body+J2".into(),
            frame_note: "ICRF≈inertial equator; GMST-only; TDB−UTC 69.184 s".into(),
            ephemeris_id: Some("horizons_iss_2026-07-17_72h.txt".into()),
        }
    }

    #[test]
    fn claim_set_serializes_with_schema_and_provenance() {
        let set = predicted_claims(&iss_elements(), None, provenance());
        let json = serde_json::to_string_pretty(&set).unwrap();
        assert!(json.contains("vcad.orbit-claims/1"));
        assert!(json.contains("nodal_rate_deg_per_day"));
        assert!(json.contains("GMST-only"));
        let back: ClaimSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back, set);
    }

    #[test]
    fn predicted_claims_roll_up_provisional_never_pass() {
        let set = predicted_claims(&iss_elements(), None, provenance());
        let claims = design_claims(&set);
        assert_eq!(claims.len(), set.claims.len());
        for c in &claims {
            assert!(c.id.starts_with("orbit."));
            assert_eq!(c.basis, Some(vcad_receipt::ClaimBasis::Predicted));
        }
        let receipt = vcad_receipt::DesignReceipt::with_claims(claims);
        assert_eq!(
            receipt.verdict(),
            vcad_receipt::ReceiptVerdict::Provisional,
            "predicted orbit claims must never read as verified"
        );
    }

    #[test]
    fn sky_claim_is_measured_and_judged_by_the_budget() {
        let inside = SkyComparison {
            horizon_h: 24.0,
            position_error_km: 40.0,
            budget_km: 150.0,
            ephemeris_id: "horizons_iss_2026-07-17_72h.txt".into(),
        };
        let c = sky_comparison_claim(&inside);
        assert_eq!(c.verdict, vcad_receipt::ClaimVerdict::Pass);
        assert_eq!(c.basis, Some(vcad_receipt::ClaimBasis::Measured));
        assert!(c.id.contains("24h"));

        let outside = SkyComparison {
            position_error_km: 400.0,
            ..inside
        };
        let c = sky_comparison_claim(&outside);
        assert_eq!(
            c.verdict,
            vcad_receipt::ClaimVerdict::Fail,
            "an exceeded budget must fail, not warn"
        );
    }
}
