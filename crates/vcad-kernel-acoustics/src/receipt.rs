//! Predicted-performance claims for the design receipt.
//!
//! Emits a serializable claim set — port tuning, mode frequencies, and
//! on-axis / in-cavity pressure response at named points — with full solver
//! provenance (grid, sweep, medium, model assumptions), in the spirit of
//! `vcad.receipt/1`: every number carries how it was produced, and nothing is
//! defaulted silently.
//!
//! These are `basis: "predicted"` claims. **Closing the loop is a
//! measurement**: a calibrated measurement microphone and a swept-sine
//! excitation — the exact instruments the glockenspiel program used to verify
//! `simulate_strike` to −5 cents, which is this workspace's standing proof
//! that the sim→measurement acoustic loop closes. Binding those measurements
//! is [`compare`]'s job.
//!
//! [`design_claims`] and [`comparison_claims`] translate this family into the
//! unified [`vcad_receipt::DesignReceipt`] schema using the generic claim
//! types (open domain vocabulary — no schema bump, no TS codegen):
//! predictions ride as [`vcad_receipt::ClaimBasis::Predicted`] (a receipt
//! built from them rolls up Provisional, never Pass) and bench comparisons as
//! [`vcad_receipt::ClaimBasis::Measured`] with Holds→Pass / Violated→Fail.

use serde::{Deserialize, Serialize};

use crate::lumped::TuningBand;

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.acoustics-claims/1";

/// Domain tag for acoustics claims in the unified [`vcad_receipt`] schema.
pub const RECEIPT_DOMAIN: &str = "acoustics";

/// How the numbers were produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// Radial × axial node counts.
    pub grid: [usize; 2],
    /// Sweep band `[f_lo, f_hi]`, Hz.
    pub sweep_hz: [f64; 2],
    /// Sweep sample count.
    pub sweep_points: usize,
    /// Sound speed, m/s.
    pub sound_speed_m_s: f64,
    /// Medium density, kg/m³.
    pub density_kg_m3: f64,
    /// Model assumptions in force (e.g. `linear`, `lossless`,
    /// `pressure_release_mouth`).
    pub model: Vec<String>,
}

/// A named on-axis / in-cavity response point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponsePoint {
    /// Point label (e.g. `"on_axis_1m"`, `"port_mouth"`).
    pub label: String,
    /// Frequency the response is reported at, Hz.
    pub f_hz: f64,
    /// Response magnitude `|p|`, Pa.
    pub pressure_pa: f64,
}

/// One predicted claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Claim name (snake_case).
    pub name: String,
    /// Value.
    pub value: f64,
    /// Unit (`"1"` for dimensionless).
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
    /// Solver and medium provenance.
    pub provenance: Provenance,
    /// The claims.
    pub claims: Vec<Claim>,
}

fn claim(name: &str, value: f64, unit: &str, note: &str) -> Claim {
    Claim {
        name: name.to_string(),
        value,
        unit: unit.to_string(),
        basis: "predicted".to_string(),
        note: note.to_string(),
    }
}

/// Build the predicted claim set for a ported-box / resonator prediction.
///
/// `field_tuning_hz` is the field-solved port tuning (bass-reflex resonance);
/// `band` is the lumped end-correction band it should fall in; `modes` are the
/// extracted resonance frequencies (Hz, ascending); `responses` are named
/// pressure readouts.
pub fn predicted_claims(
    provenance: Provenance,
    field_tuning_hz: Option<f64>,
    band: TuningBand,
    modes: &[f64],
    responses: &[ResponsePoint],
) -> ClaimSet {
    let mut claims = Vec::new();

    if let Some(ft) = field_tuning_hz {
        claims.push(claim(
            "tuning_hz",
            ft,
            "Hz",
            "field-solved bass-reflex port tuning (box+port Helmholtz mode); \
             lossless model — Q is optimistic (no radiation/thermoviscous \
             damping)",
        ));
    }
    claims.push(claim(
        "tuning_hz_lumped",
        band.f_nominal_hz,
        "Hz",
        "lumped Helmholtz tuning f=(c/2π)√(S/(V·L_eff)); interior-flanged + \
         exterior-unflanged end correction",
    ));
    claims.push(claim(
        "tuning_band_low_hz",
        band.f_min_hz,
        "Hz",
        "lowest plausible tuning (both ends flanged, longest L_eff)",
    ));
    claims.push(claim(
        "tuning_band_high_hz",
        band.f_max_hz,
        "Hz",
        "highest plausible tuning (interior end only; pressure-release mouth \
         omits exterior radiation mass)",
    ));
    for (i, &m) in modes.iter().enumerate() {
        claims.push(claim(
            &format!("mode_{}_hz", i + 1),
            m,
            "Hz",
            "resonance from a driven frequency sweep (peak of |p| at the probe)",
        ));
    }
    for r in responses {
        claims.push(claim(
            &format!("response_{}_pa", r.label),
            r.pressure_pa,
            "Pa",
            &format!("|p| at {} at {:.2} Hz, unit drive", r.label, r.f_hz),
        ));
    }

    ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance,
        claims,
    }
}

/// A bench measurement to bind against a predicted claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    /// Claim name this measures (must match a claim).
    pub name: String,
    /// Measured value, in the claim's unit.
    pub value: f64,
    /// One-sigma absolute uncertainty, same unit.
    pub uncertainty: f64,
    /// Instrument provenance ("calibrated measurement mic + swept sine …").
    pub instrument: String,
    /// Acceptance band as a multiplicative factor: the claim holds when
    /// measured / predicted ∈ [1/band, band] after widening by the
    /// uncertainty. A tuning frequency warrants a tight band; a lossless SPL
    /// (Q optimistic) a generous one.
    pub band_factor: f64,
}

/// Verdict for one claim, in the repo's receipt vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// Measurement inside the stated band.
    Holds,
    /// Measurement outside the stated band.
    Violated,
    /// No measurement bound to this claim (fail-closed: unmeasured never
    /// silently passes).
    Unmeasured,
}

/// One row of the predicted-vs-measured comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparisonEntry {
    /// Claim name.
    pub name: String,
    /// Predicted value.
    pub predicted: f64,
    /// Measured value (`None` when unmeasured).
    pub measured: Option<f64>,
    /// measured / predicted (`None` when unmeasured or predicted = 0).
    pub ratio: Option<f64>,
    /// Verdict.
    pub verdict: Verdict,
}

/// The comparison report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparisonReport {
    /// Schema tag.
    pub schema: String,
    /// Per-claim rows.
    pub entries: Vec<ComparisonEntry>,
    /// True only when every measured claim holds AND at least one measurement
    /// exists — an unmeasured receipt never passes.
    pub all_hold: bool,
}

/// Bind measurements to a claim set. A measurement matching no claim is an
/// error (measuring nothing is a bookkeeping bug).
pub fn compare(
    claims: &ClaimSet,
    measurements: &[Measurement],
) -> Result<ComparisonReport, String> {
    for m in measurements {
        if !claims.claims.iter().any(|c| c.name == m.name) {
            return Err(format!("measurement {:?} matches no claim", m.name));
        }
    }
    let mut entries = Vec::with_capacity(claims.claims.len());
    let mut measured_any = false;
    let mut all_hold = true;
    for c in &claims.claims {
        let m = measurements.iter().find(|m| m.name == c.name);
        let entry = match m {
            None => ComparisonEntry {
                name: c.name.clone(),
                predicted: c.value,
                measured: None,
                ratio: None,
                verdict: Verdict::Unmeasured,
            },
            Some(m) => {
                measured_any = true;
                let ratio = if c.value != 0.0 {
                    Some(m.value / c.value)
                } else {
                    None
                };
                let lo = c.value / m.band_factor - m.uncertainty;
                let hi = c.value * m.band_factor + m.uncertainty;
                let holds = (lo..=hi).contains(&m.value);
                if !holds {
                    all_hold = false;
                }
                ComparisonEntry {
                    name: c.name.clone(),
                    predicted: c.value,
                    measured: Some(m.value),
                    ratio,
                    verdict: if holds {
                        Verdict::Holds
                    } else {
                        Verdict::Violated
                    },
                }
            }
        };
        entries.push(entry);
    }
    Ok(ComparisonReport {
        schema: "vcad.acoustics-compare/1".to_string(),
        entries,
        all_hold: all_hold && measured_any,
    })
}

/// The oracle reference for this crate's field solver.
pub fn oracle() -> vcad_receipt::OracleRef {
    vcad_receipt::OracleRef::new("vcad-kernel-acoustics/helmholtz", env!("CARGO_PKG_VERSION"))
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
/// Every claim lands with [`vcad_receipt::ClaimBasis::Predicted`] — the solver
/// ran for real, but the claim is about a physical enclosure that has not been
/// measured, so a receipt built from these **rolls up Provisional, never
/// Pass** (the same contract as `predict_physics`/`predict_print`).
pub fn design_claims(set: &ClaimSet) -> Vec<vcad_receipt::ReceiptClaim> {
    let oracle = oracle();
    let provenance = format!(
        "grid {}x{}, sweep {:.1}-{:.1} Hz ({} pts), c={:.1} m/s, rho={:.3}; model [{}]",
        set.provenance.grid[0],
        set.provenance.grid[1],
        set.provenance.sweep_hz[0],
        set.provenance.sweep_hz[1],
        set.provenance.sweep_points,
        set.provenance.sound_speed_m_s,
        set.provenance.density_kg_m3,
        set.provenance.model.join(", "),
    );
    set.claims
        .iter()
        .map(|c| {
            vcad_receipt::ReceiptClaim::pass(
                format!("acoustics.{}", c.name),
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

/// Translate a bench [`ComparisonReport`] into unified-receipt claims.
/// Holds → Pass, Violated → Fail, both [`vcad_receipt::ClaimBasis::Measured`].
pub fn comparison_claims(
    report: &ComparisonReport,
    set: &ClaimSet,
    measurements: &[Measurement],
) -> Vec<vcad_receipt::ReceiptClaim> {
    let oracle = oracle();
    report
        .entries
        .iter()
        .filter_map(|e| {
            let measured = e.measured?;
            let m = measurements.iter().find(|m| m.name == e.name)?;
            let unit = set
                .claims
                .iter()
                .find(|c| c.name == e.name)
                .map(|c| c.unit.clone())
                .unwrap_or_else(|| "1".to_string());
            let base = match e.verdict {
                Verdict::Holds => vcad_receipt::ReceiptClaim::pass(
                    format!("acoustics.{}", e.name),
                    RECEIPT_DOMAIN,
                    format!(
                        "bench measurement of {} within band {}",
                        e.name, m.band_factor
                    ),
                    oracle.clone(),
                ),
                Verdict::Violated => vcad_receipt::ReceiptClaim::fail(
                    format!("acoustics.{}", e.name),
                    RECEIPT_DOMAIN,
                    format!(
                        "bench measurement of {} outside band {}",
                        e.name, m.band_factor
                    ),
                    oracle.clone(),
                ),
                Verdict::Unmeasured => return None,
            };
            Some(
                base.with_basis(vcad_receipt::ClaimBasis::Measured)
                    .with_predicted(quantity(e.predicted, &unit))
                    .with_measured(quantity(measured, &unit))
                    .with_details(format!("instrument: {}", m.instrument)),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::medium::Medium;

    fn build() -> ClaimSet {
        let air = Medium::air(20.0);
        let band = crate::lumped::ported_box_tuning_mm(&air, 20e6, 25.0, 120.0);
        predicted_claims(
            Provenance {
                grid: [40, 140],
                sweep_hz: [20.0, 120.0],
                sweep_points: 60,
                sound_speed_m_s: air.c,
                density_kg_m3: air.rho,
                model: vec![
                    "linear".into(),
                    "lossless".into(),
                    "pressure_release_mouth".into(),
                ],
            },
            Some(band.f_nominal_hz * 1.03),
            band,
            &[band.f_nominal_hz * 1.03, 180.0],
            &[ResponsePoint {
                label: "port_mouth".into(),
                f_hz: band.f_nominal_hz,
                pressure_pa: 2.5,
            }],
        )
    }

    fn get(set: &ClaimSet, name: &str) -> f64 {
        set.claims.iter().find(|c| c.name == name).unwrap().value
    }

    #[test]
    fn tuning_claims_are_ordered_and_present() {
        let set = build();
        let lo = get(&set, "tuning_band_low_hz");
        let hi = get(&set, "tuning_band_high_hz");
        let nom = get(&set, "tuning_hz_lumped");
        assert!(lo < nom && nom < hi);
        assert!(get(&set, "tuning_hz") > 0.0);
        assert!(get(&set, "mode_1_hz") > 0.0);
        assert!(get(&set, "response_port_mouth_pa") > 0.0);
    }

    #[test]
    fn serializes_with_schema_and_provenance() {
        let set = build();
        let json = serde_json::to_string_pretty(&set).unwrap();
        assert!(json.contains("vcad.acoustics-claims/1"));
        assert!(json.contains("tuning_hz"));
        assert!(json.contains("pressure_release_mouth"));
        let back: ClaimSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema, set.schema);
        assert_eq!(back.claims.len(), set.claims.len());
    }

    #[test]
    fn compare_binds_measurements_fail_closed() {
        let set = build();
        // Unmeasured receipt never passes.
        let empty = compare(&set, &[]).unwrap();
        assert!(!empty.all_hold);
        assert!(empty
            .entries
            .iter()
            .all(|e| e.verdict == Verdict::Unmeasured));

        // A measurement of nothing is an error.
        let bogus = Measurement {
            name: "warp_hz".into(),
            value: 9.0,
            uncertainty: 0.1,
            instrument: "vibes".into(),
            band_factor: 2.0,
        };
        assert!(compare(&set, &[bogus]).is_err());

        // A mic that agrees on tuning within band → Holds; a wild mode → Violated.
        let ft = get(&set, "tuning_hz");
        let ok = Measurement {
            name: "tuning_hz".into(),
            value: ft * 1.02,
            uncertainty: 0.5,
            instrument: "calibrated mic + swept sine".into(),
            band_factor: 1.1,
        };
        let bad = Measurement {
            name: "mode_1_hz".into(),
            value: get(&set, "mode_1_hz") * 3.0,
            uncertainty: 0.5,
            instrument: "calibrated mic + swept sine".into(),
            band_factor: 1.1,
        };
        let report = compare(&set, &[ok, bad]).unwrap();
        assert!(!report.all_hold);
        let verdict = |name: &str| {
            report
                .entries
                .iter()
                .find(|e| e.name == name)
                .unwrap()
                .verdict
        };
        assert_eq!(verdict("tuning_hz"), Verdict::Holds);
        assert_eq!(verdict("mode_1_hz"), Verdict::Violated);
        assert_eq!(verdict("tuning_hz_lumped"), Verdict::Unmeasured);
    }

    #[test]
    fn design_claims_ride_the_unified_receipt_as_provisional() {
        let set = build();
        let claims = design_claims(&set);
        assert_eq!(claims.len(), set.claims.len());
        for c in &claims {
            assert!(c.id.starts_with("acoustics."));
            assert_eq!(c.domain, RECEIPT_DOMAIN);
            assert_eq!(c.basis, Some(vcad_receipt::ClaimBasis::Predicted));
            assert!(c.measured.is_some());
        }
        let receipt = vcad_receipt::DesignReceipt::with_claims(claims);
        assert_eq!(
            receipt.verdict(),
            vcad_receipt::ReceiptVerdict::Provisional,
            "predicted acoustics claims must never read as verified"
        );
    }

    #[test]
    fn comparison_claims_map_verdicts_with_measured_basis() {
        let set = build();
        let ft = get(&set, "tuning_hz");
        let measurements = vec![Measurement {
            name: "tuning_hz".into(),
            value: ft * 1.02,
            uncertainty: 0.5,
            instrument: "calibrated mic + swept sine".into(),
            band_factor: 1.1,
        }];
        let report = compare(&set, &measurements).unwrap();
        let claims = comparison_claims(&report, &set, &measurements);
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].verdict, vcad_receipt::ClaimVerdict::Pass);
        assert_eq!(claims[0].basis, Some(vcad_receipt::ClaimBasis::Measured));
        assert!(claims[0].predicted.is_some() && claims[0].measured.is_some());
    }
}
