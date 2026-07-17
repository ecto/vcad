//! Predicted tolerance claims for the design receipt (M4).
//!
//! Emits a serializable claim set — fit probability (with Monte Carlo
//! standard error), RSS yield, Cp/Cpk, gap moments, worst-case margin —
//! with full provenance (sample count, seed, per-contributor
//! distribution sources and conventions, analyses run), in the spirit
//! of `vcad.receipt/1`: every number carries how it was produced, and
//! nothing is defaulted silently.
//!
//! These are `basis: "predicted"` claims. Binding them to measurements
//! (assembly trials, coupon dimensions from the 3DP
//! print-then-measure loop) is the measurement pack's job (M6), through
//! [`compare`] — fail-closed: an unmeasured receipt never passes, and a
//! measurement matching no claim is a bookkeeping error. Wiring this
//! family into `crates/vcad-receipt` + the MCP surface is the flagged
//! follow-up PR (it touches the cross-crate schema and TS codegen —
//! type names must be unique across `vcad-ir` and `vcad-receipt`).
//!
//! **Completeness is structural:** [`predicted_claims`] requires all
//! three analyses. You cannot emit a claim set without Monte Carlo
//! error bars, and a consistency tripwire rejects analyses that don't
//! describe the same chain (mixing results from different stackups is
//! the receipt-level version of a unit error).

use serde::{Deserialize, Serialize};

use crate::analysis::{MonteCarloAnalysis, RssAnalysis, WorstCaseAnalysis};
use crate::stackup::{Stackup, StackupError};

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.tolerance-claims/1";

/// Schema tag for the comparison report.
pub const COMPARE_SCHEMA: &str = "vcad.tolerance-compare/1";

/// Per-contributor provenance: what the distribution was and where it
/// came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContributorProvenance {
    /// Contributor name.
    pub name: String,
    /// The deviation distribution used.
    pub dist: crate::dist::Distribution,
    /// Assumption or measurement.
    pub source: crate::dist::DistributionSource,
    /// Chain coefficient.
    pub coeff: f64,
    /// Drawing limits (tol_minus, tol_plus), mm.
    pub limits: (f64, f64),
}

/// How the numbers were produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// Monte Carlo sample count.
    pub n_samples: usize,
    /// PRNG seed (xoshiro256++ via SplitMix64).
    pub seed: u64,
    /// Batch count behind the batch-SE cross-check.
    pub batches: usize,
    /// Whether every contributor is normal (RSS yield exact under the
    /// model) or the RSS yield leans on the CLT.
    pub all_normal: bool,
    /// The requirement being priced, as stated.
    pub requirement: crate::stackup::Requirement,
    /// Every contributor's distribution and its source.
    pub contributors: Vec<ContributorProvenance>,
    /// Analyses that fed this claim set.
    pub analyses: Vec<String>,
}

/// One predicted claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Claim name (snake_case).
    pub name: String,
    /// Value.
    pub value: f64,
    /// One-sigma uncertainty on the value, when the producing path has
    /// one (Monte Carlo estimates always do; closed-form RSS values
    /// carry the erf approximation bound instead of pretending to be
    /// exact).
    pub uncertainty: Option<f64>,
    /// Unit ("1" for dimensionless, "mm" for gaps).
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
    /// Full provenance.
    pub provenance: Provenance,
    /// The claims.
    pub claims: Vec<Claim>,
}

fn claim(name: &str, value: f64, uncertainty: Option<f64>, unit: &str, note: &str) -> Claim {
    Claim {
        name: name.to_string(),
        value,
        uncertainty,
        unit: unit.to_string(),
        basis: "predicted".to_string(),
        note: note.to_string(),
    }
}

/// Build the predicted claim set for one analyzed stackup. Requires
/// all three analyses (completeness is structural) and cross-checks
/// that they describe the same chain.
pub fn predicted_claims(
    s: &Stackup,
    wc: &WorstCaseAnalysis,
    r: &RssAnalysis,
    mc: &MonteCarloAnalysis,
) -> Result<ClaimSet, StackupError> {
    s.validate()?;
    // Tripwire: the three analyses must be of the same stackup. The
    // means of RSS and MC are estimates of the same exact quantity, so
    // an 8-SE disagreement means someone mixed results.
    if (mc.mean_gap - r.mean_gap).abs() > 8.0 * mc.mean_gap_se {
        return Err(StackupError::BadRequirement(format!(
            "analyses disagree on the mean gap ({} vs {} ± {}); \
             were they run on the same stackup?",
            r.mean_gap, mc.mean_gap, mc.mean_gap_se
        )));
    }
    let rss_note = if r.all_normal {
        "Phi-based; exact under the all-normal model and the stated \
         sigma conventions"
    } else {
        "Phi-based via the CLT (chain has non-normal contributors); \
         the Monte Carlo claim is the check"
    };
    let mut claims = vec![
        claim(
            "fit_probability",
            mc.fit.p,
            Some(mc.fit.standard_error),
            "1",
            "Monte Carlo fraction of virtual assemblies meeting the \
             requirement; uncertainty is the Agresti-Coull standard error",
        ),
        claim("rss_yield", r.yield_estimate, Some(1.5e-7), "1", rss_note),
        claim(
            "mean_gap_mm",
            r.mean_gap,
            None,
            "mm",
            "exact under independence: sum of coefficient-weighted mean \
             dimensions",
        ),
        claim(
            "sigma_gap_mm",
            r.sigma_gap,
            None,
            "mm",
            "exact under independence: root-sum-square of \
             coefficient-weighted sigmas",
        ),
        claim(
            "worst_case_margin_mm",
            wc.worst_margin(),
            None,
            "mm",
            "binding margin with every part at its worst drawing limit; \
             negative means worst-case assemblies violate the requirement \
             even when the statistical yield is high",
        ),
    ];
    if let Some(cpk) = r.cpk {
        claims.push(claim(
            "cpk",
            cpk,
            None,
            "1",
            "min distance from mean gap to a limit over 3 sigma_gap",
        ));
    }
    if let Some(cp) = r.cp {
        claims.push(claim(
            "cp",
            cp,
            None,
            "1",
            "requirement width over 6 sigma_gap (two-sided requirements only)",
        ));
    }
    Ok(ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: Provenance {
            n_samples: mc.n,
            seed: mc.seed,
            batches: mc.batches,
            all_normal: r.all_normal,
            requirement: s.requirement.clone(),
            contributors: s
                .contributors
                .iter()
                .map(|c| ContributorProvenance {
                    name: c.name.clone(),
                    dist: c.dist,
                    source: c.source.clone(),
                    coeff: c.coeff,
                    limits: (c.tol_minus, c.tol_plus),
                })
                .collect(),
            analyses: vec![
                "worst_case".to_string(),
                "rss".to_string(),
                "monte_carlo".to_string(),
            ],
        },
        claims,
    })
}

/// A measurement to bind against a predicted claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    /// Claim name this measures (must match a claim).
    pub name: String,
    /// Measured value, in the claim's unit.
    pub value: f64,
    /// One-sigma absolute uncertainty of the measurement, same unit.
    pub uncertainty: f64,
    /// Instrument provenance ("assembly trial n=50", "CMM s/n ...").
    pub instrument: String,
    /// Acceptance half-width, absolute, same unit: the claim holds when
    /// |measured − predicted| ≤ band_abs + measurement uncertainty +
    /// the claim's own uncertainty. Additive (not multiplicative)
    /// because gap margins live near zero and can be negative.
    pub band_abs: f64,
}

/// Verdict for one claim, in the repo's receipt vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// Measurement inside the stated band.
    Holds,
    /// Measurement outside the stated band.
    Violated,
    /// No measurement bound to this claim (fail-closed: unmeasured is
    /// never silently passing).
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
    /// measured − predicted (`None` when unmeasured).
    pub delta: Option<f64>,
    /// Verdict.
    pub verdict: Verdict,
}

/// The comparison report: every claim gets a row; measurements that
/// match no claim are an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparisonReport {
    /// Schema tag ([`COMPARE_SCHEMA`]).
    pub schema: String,
    /// Per-claim rows.
    pub entries: Vec<ComparisonEntry>,
    /// True only when every measured claim holds AND at least one
    /// measurement exists — an unmeasured receipt never passes.
    pub all_hold: bool,
}

/// Bind measurements to a claim set, fail-closed.
pub fn compare(
    claims: &ClaimSet,
    measurements: &[Measurement],
) -> Result<ComparisonReport, String> {
    for m in measurements {
        if !claims.claims.iter().any(|c| c.name == m.name) {
            return Err(format!("measurement {:?} matches no claim", m.name));
        }
        let bands_ok = m.uncertainty.is_finite()
            && m.uncertainty >= 0.0
            && m.band_abs.is_finite()
            && m.band_abs >= 0.0;
        if !bands_ok {
            return Err(format!(
                "measurement {:?} has invalid uncertainty/band",
                m.name
            ));
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
                delta: None,
                verdict: Verdict::Unmeasured,
            },
            Some(m) => {
                measured_any = true;
                let delta = m.value - c.value;
                let band = m.band_abs + m.uncertainty + c.uncertainty.unwrap_or(0.0);
                let holds = delta.abs() <= band;
                if !holds {
                    all_hold = false;
                }
                ComparisonEntry {
                    name: c.name.clone(),
                    predicted: c.value,
                    measured: Some(m.value),
                    delta: Some(delta),
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
        schema: COMPARE_SCHEMA.to_string(),
        entries,
        all_hold: all_hold && measured_any,
    })
}

/// Domain tag for tolerance claims in the unified [`vcad_receipt`] schema.
pub const RECEIPT_DOMAIN: &str = "tolerance";

/// The oracle reference for this crate's analysis pipeline.
pub fn oracle() -> vcad_receipt::OracleRef {
    vcad_receipt::OracleRef::new("vcad-kernel-tolerance/analysis", env!("CARGO_PKG_VERSION"))
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
/// analyses ran for real, but they describe parts that have not been
/// measured, so a receipt built from these **rolls up Provisional, never
/// Pass** (the same contract as `predict_physics`/`predict_print`). The
/// computed value rides in `measured` ("what the oracle computed");
/// provenance and per-claim one-sigma uncertainty ride in `details`.
pub fn design_claims(set: &ClaimSet) -> Vec<vcad_receipt::ReceiptClaim> {
    let oracle = oracle();
    let provenance = format!(
        "analyses [{}]; mc n {} seed {} batches {}; all_normal {}",
        set.provenance.analyses.join(", "),
        set.provenance.n_samples,
        set.provenance.seed,
        set.provenance.batches,
        set.provenance.all_normal,
    );
    set.claims
        .iter()
        .map(|c| {
            let details = match c.uncertainty {
                Some(u) => format!("{provenance}; one_sigma {u:.3e}"),
                None => provenance.clone(),
            };
            vcad_receipt::ReceiptClaim::pass(
                format!("tolerance.{}", c.name),
                RECEIPT_DOMAIN,
                c.note.clone(),
                oracle.clone(),
            )
            .with_basis(vcad_receipt::ClaimBasis::Predicted)
            .with_measured(quantity(c.value, &c.unit))
            .with_details(details)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{monte_carlo, rss, worst_case, McOptions};
    use crate::dist::SigmaConvention;
    use crate::stackup::{Contributor, Requirement};

    fn build() -> (Stackup, ClaimSet) {
        let conv = SigmaConvention::ThreeSigma;
        let s = Stackup {
            name: "receipt-chain".into(),
            contributors: vec![
                Contributor::normal("pocket", 1.0, 20.0, 0.15, conv),
                Contributor::uniform("bushing", -1.0, 12.0, 0.1, 0.0),
                Contributor::normal("shim", -1.0, 7.5, 0.05, conv),
            ],
            requirement: Requirement::between("protrusion", 0.35, 0.75),
        };
        let wc = worst_case(&s).unwrap();
        let r = rss(&s).unwrap();
        let mc = monte_carlo(
            &s,
            &McOptions {
                n: 100_000,
                seed: 314,
                batches: 16,
            },
        )
        .unwrap();
        let set = predicted_claims(&s, &wc, &r, &mc).unwrap();
        (s, set)
    }

    fn get<'a>(set: &'a ClaimSet, name: &str) -> &'a Claim {
        set.claims
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("missing claim {name}"))
    }

    #[test]
    fn design_claims_ride_the_unified_receipt_as_provisional() {
        let (_, set) = build();
        let claims = design_claims(&set);
        assert_eq!(claims.len(), set.claims.len());
        for c in &claims {
            assert!(c.id.starts_with("tolerance."));
            assert_eq!(c.domain, RECEIPT_DOMAIN);
            assert_eq!(c.basis, Some(vcad_receipt::ClaimBasis::Predicted));
            assert!(c.measured.is_some());
            assert!(c.details.as_deref().unwrap_or("").contains("monte_carlo"));
        }
        // fit_probability carries its MC standard error into details.
        let fit = claims
            .iter()
            .find(|c| c.id == "tolerance.fit_probability")
            .unwrap();
        assert!(fit.details.as_deref().unwrap().contains("one_sigma"));
        // The fail-closed contract: an all-pass predicted receipt rolls up
        // Provisional, never Pass.
        let receipt = vcad_receipt::DesignReceipt::with_claims(claims);
        assert_eq!(
            receipt.verdict(),
            vcad_receipt::ReceiptVerdict::Provisional,
            "predicted tolerance claims must never read as verified"
        );
    }

    #[test]
    fn claims_are_complete_consistent_and_error_barred() {
        let (_, set) = build();
        assert_eq!(set.schema, CLAIM_SCHEMA);
        // fit_probability structurally carries its MC standard error.
        let fit = get(&set, "fit_probability");
        assert!(fit.uncertainty.unwrap() > 0.0);
        // RSS and MC agree (they're the same chain).
        let rss_y = get(&set, "rss_yield");
        assert!((fit.value - rss_y.value).abs() < 5.0 * fit.uncertainty.unwrap());
        // The worst-case margin is present and negative here (the
        // bushing band makes WC fail while statistics pass) — honesty
        // on the same receipt.
        let wcm = get(&set, "worst_case_margin_mm");
        assert!(wcm.value < 0.0, "wc margin {}", wcm.value);
        assert!(fit.value > 0.95, "fit {}", fit.value);
        // Provenance names every contributor and its source.
        assert_eq!(set.provenance.contributors.len(), 3);
        assert_eq!(set.provenance.n_samples, 100_000);
        assert_eq!(set.provenance.seed, 314);
        assert!(!set.provenance.all_normal);
        assert!(set
            .provenance
            .contributors
            .iter()
            .all(|c| matches!(c.source, crate::dist::DistributionSource::Assumed { .. })));
    }

    #[test]
    fn mismatched_analyses_trip_the_wire() {
        let (s, _) = build();
        let wc = worst_case(&s).unwrap();
        let r = rss(&s).unwrap();
        // MC from a different chain (shifted nominal): the means
        // disagree by far more than 8 SE.
        let mut other = s.clone();
        other.contributors[0].nominal += 0.3;
        let mc = monte_carlo(
            &other,
            &McOptions {
                n: 100_000,
                seed: 315,
                batches: 16,
            },
        )
        .unwrap();
        assert!(predicted_claims(&s, &wc, &r, &mc).is_err());
    }

    #[test]
    fn serializes_with_schema_and_provenance() {
        let (_, set) = build();
        let json = serde_json::to_string_pretty(&set).unwrap();
        assert!(json.contains("vcad.tolerance-claims/1"));
        assert!(json.contains("fit_probability"));
        assert!(json.contains("assumed"));
        assert!(json.contains("three_sigma"));
        let back: ClaimSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema, set.schema);
        assert_eq!(back.claims.len(), set.claims.len());
        for (a, b) in back.claims.iter().zip(&set.claims) {
            assert_eq!(a.name, b.name);
            let scale = b.value.abs().max(1e-300);
            assert!((a.value - b.value).abs() / scale < 1e-12);
        }
    }

    #[test]
    fn compare_binds_measurements_fail_closed() {
        let (_, set) = build();
        // Unmeasured receipt never passes.
        let empty = compare(&set, &[]).unwrap();
        assert!(!empty.all_hold);
        assert!(empty
            .entries
            .iter()
            .all(|e| e.verdict == Verdict::Unmeasured));

        // A measurement of nothing is an error.
        let bogus = Measurement {
            name: "warp_factor".into(),
            value: 9.0,
            uncertainty: 0.1,
            instrument: "vibes".into(),
            band_abs: 1.0,
        };
        assert!(compare(&set, &[bogus]).is_err());

        // An assembly trial agreeing within band → Holds; a wildly
        // off sigma → Violated; the rest stay Unmeasured.
        let fit_pred = get(&set, "fit_probability").value;
        let ok = Measurement {
            name: "fit_probability".into(),
            value: (fit_pred - 0.01).clamp(0.0, 1.0),
            uncertainty: 0.02,
            instrument: "assembly trial n=200".into(),
            band_abs: 0.02,
        };
        let bad = Measurement {
            name: "sigma_gap_mm".into(),
            value: get(&set, "sigma_gap_mm").value * 3.0,
            uncertainty: 0.001,
            instrument: "CMM coupon set".into(),
            band_abs: 0.01,
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
        assert_eq!(verdict("fit_probability"), Verdict::Holds);
        assert_eq!(verdict("sigma_gap_mm"), Verdict::Violated);
        assert_eq!(verdict("cpk"), Verdict::Unmeasured);

        // All-measured-and-holding passes.
        let good = Measurement {
            name: "sigma_gap_mm".into(),
            value: get(&set, "sigma_gap_mm").value * 1.02,
            uncertainty: 0.005,
            instrument: "CMM coupon set".into(),
            band_abs: 0.01,
        };
        let ok2 = Measurement {
            name: "fit_probability".into(),
            value: (fit_pred - 0.01).clamp(0.0, 1.0),
            uncertainty: 0.02,
            instrument: "assembly trial n=200".into(),
            band_abs: 0.02,
        };
        let report = compare(&set, &[ok2, good]).unwrap();
        assert!(report.all_hold, "{report:?}");
    }

    #[test]
    fn negative_margins_and_bands_behave() {
        // Additive bands work at negative predicted values (WC margin).
        let (_, set) = build();
        let wcm = get(&set, "worst_case_margin_mm").value;
        assert!(wcm < 0.0);
        let m = Measurement {
            name: "worst_case_margin_mm".into(),
            value: wcm - 0.005,
            uncertainty: 0.002,
            instrument: "gauge study".into(),
            band_abs: 0.005,
        };
        let report = compare(&set, &[m]).unwrap();
        assert_eq!(report.entries.len(), set.claims.len());
        assert!(report.all_hold);
        // Invalid measurement uncertainty is an error, not a pass.
        let bad = Measurement {
            name: "worst_case_margin_mm".into(),
            value: wcm,
            uncertainty: f64::NAN,
            instrument: "broken".into(),
            band_abs: 0.005,
        };
        assert!(compare(&set, &[bad]).is_err());
    }
}
