//! M4 integration: claims carry uncertainty + provenance, refuse
//! unbalanced or zero-scored runs, and bind to measurements with
//! fail-closed verdicts.

use std::collections::BTreeMap;

use vcad_kernel_neutronics::receipt::{
    claims_from_run, compare, predicted_claims, ClaimError, Measurement, Verdict, CLAIM_SCHEMA,
};
use vcad_kernel_neutronics::spec::{evaluate, ShieldSpec};

fn spec() -> ShieldSpec {
    serde_json::from_str(
        r#"{
          "layers": [
            {"material": "air", "thickness_mm": 300},
            {"material": "hdpe", "thickness_mm": 100},
            {"material": "air", "thickness_mm": 1800}
          ],
          "source": {"rate_n_per_s": 5.0e6, "energy_ev": 2.45e6},
          "detectors": [{"label": "operator", "radius_mm": 2000}],
          "run": {"histories_per_batch": 2000, "batches": 10, "seed": 11}
        }"#,
    )
    .unwrap()
}

#[test]
fn claims_carry_uncertainty_provenance_and_caveats() {
    let claims = predicted_claims(&spec(), &BTreeMap::new()).unwrap();
    assert_eq!(claims.schema, CLAIM_SCHEMA);
    assert_eq!(claims.provenance.energy_model, "exact-kinematics");
    assert_eq!(claims.provenance.seed, 11);
    assert!(claims.provenance.library.contains("design-estimate"));
    assert!(!claims.caveats.is_empty());
    let dose = claims
        .claims
        .iter()
        .find(|c| c.name == "dose_rate:operator")
        .expect("dose claim");
    assert!(dose.value > 0.0 && dose.rse > 0.0 && dose.rse < 0.1);
    assert_eq!(dose.unit, "uSv/h");
    assert_eq!(dose.basis, "predicted");
    let att = claims
        .claims
        .iter()
        .find(|c| c.name == "attenuation_factor:operator")
        .expect("attenuation claim");
    assert!(att.value > 1.5, "10 cm HDPE attenuation {}", att.value);
    // JSON round-trip: the receipt is a wire object.
    let json = serde_json::to_string_pretty(&claims).unwrap();
    let back: vcad_kernel_neutronics::receipt::ClaimSet = serde_json::from_str(&json).unwrap();
    assert_eq!(claims, back);
}

#[test]
fn truncated_or_zero_scored_runs_refuse_claims() {
    let s = spec();
    let params = BTreeMap::new();
    let resolved = s.resolve(&params).unwrap();
    let (doses, mut result) = evaluate(&s, &params).unwrap();

    // Doctored truncation: the books no longer balance — refused.
    result.truncated_histories = 3;
    assert_eq!(
        claims_from_run(&s, &resolved.detector_regions, &doses, &result, &params).unwrap_err(),
        ClaimError::TruncatedHistories(3)
    );

    // Doctored zero-scored dose: RSE = ∞ — refused by name.
    result.truncated_histories = 0;
    let mut doses2 = doses.clone();
    doses2[0].dose_usv_per_h = vcad_kernel_neutronics::tally::Estimate {
        mean: 0.0,
        rse: f64::INFINITY,
        batches: 10,
    };
    assert!(matches!(
        claims_from_run(&s, &resolved.detector_regions, &doses2, &result, &params).unwrap_err(),
        ClaimError::NothingScored { name } if name == "dose_rate:operator"
    ));
}

#[test]
fn compare_verdicts_fail_closed() {
    let claims = predicted_claims(&spec(), &BTreeMap::new()).unwrap();
    let dose = claims
        .claims
        .iter()
        .find(|c| c.name == "dose_rate:operator")
        .unwrap();

    // No measurements: everything Unmeasured, all_hold = false.
    let empty = compare(&claims, &[]).unwrap();
    assert!(!empty.all_hold);
    assert!(empty
        .entries
        .iter()
        .all(|e| e.verdict == Verdict::Unmeasured));

    // A holding measurement (within band).
    let m_hold = Measurement {
        name: "dose_rate:operator".to_string(),
        value: dose.value * 1.3,
        uncertainty: dose.value * 0.1,
        instrument: "survey meter (test)".to_string(),
        band_factor: 1.5,
    };
    let rep = compare(&claims, &[m_hold]).unwrap();
    let entry = rep
        .entries
        .iter()
        .find(|e| e.name == "dose_rate:operator")
        .unwrap();
    assert_eq!(entry.verdict, Verdict::Holds);
    // all_hold is still false in spirit? No: every *measured* claim
    // holds and one measurement exists — the report passes, with the
    // other claims listed Unmeasured (they must be published as such).
    assert!(rep.all_hold);
    assert!(rep.entries.iter().any(|e| e.verdict == Verdict::Unmeasured));

    // A violating measurement.
    let m_bad = Measurement {
        name: "dose_rate:operator".to_string(),
        value: dose.value * 10.0,
        uncertainty: dose.value * 0.05,
        instrument: "survey meter (test)".to_string(),
        band_factor: 1.5,
    };
    let rep = compare(&claims, &[m_bad]).unwrap();
    assert!(!rep.all_hold);

    // A measurement of nothing is a bookkeeping error.
    let m_ghost = Measurement {
        name: "dose_rate:ghost".to_string(),
        value: 1.0,
        uncertainty: 0.1,
        instrument: "imagination".to_string(),
        band_factor: 2.0,
    };
    assert!(compare(&claims, &[m_ghost]).is_err());
}
