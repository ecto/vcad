//! The `.vcad` seam: a serde stackup schema with named parameters (M3).
//!
//! A [`StackupSpec`] is the serialization contract between vcad
//! documents and this crate. Every numeric field is a [`ParamValue`]:
//! either a literal, or the **name** of a document parameter supplied
//! at resolve time. Resolution is fail-closed — an unbound name is an
//! error, never a default — and the resolved [`Stackup`] is validated
//! before it is returned, so a spec that resolves to nonsense (negative
//! tolerance via a bad binding, say) also errors instead of analyzing.
//!
//! ## The adapter contract (vcad side of the seam)
//!
//! BRep/document extraction deliberately does not live here — it lands
//! on the vcad side, emitting this schema (the same division of labor
//! as `vcad-kernel-particle::spec` and `document_parameter_gradient`):
//!
//! - **Dimensions come from sketches.** A document's sketch dimensions
//!   and feature parameters are the natural [`ParamValue::Named`]
//!   bindings; the adapter walks the parametric DAG and emits one
//!   [`ContributorSpec`] per chain member, with `nominal` named after
//!   the driving document parameter so the stackup re-prices when the
//!   design moves.
//! - **Requirements come from clearance assertions.** The document's
//!   labeled `check_clearance` assertions (min distance between part
//!   groups, re-verified as Holds/Stale/Violated on the receipt) map
//!   onto [`RequirementSpec`]: the assertion's minimum distance is
//!   `lower_mm` and its label is the requirement name. A tolerance
//!   claim then *prices the probability* that the clearance assertion
//!   holds in production — the two verdicts talk about the same gap.
//!
//! Tolerance-to-σ consistency by construction: the
//! [`DistSpec::NormalFromTol`] variant derives σ from the contributor's
//! own resolved drawing tolerance under a stated convention, so a
//! drawing-driven spec cannot drift its statistical model away from its
//! limits.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::dist::{Distribution, DistributionSource, SigmaConvention};
use crate::stackup::{Contributor, Requirement, Stackup, StackupError};

/// A literal value or the name of a document parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    /// A literal number.
    Literal(f64),
    /// The name of a parameter bound at [`StackupSpec::resolve`] time.
    Named(String),
}

impl From<f64> for ParamValue {
    fn from(v: f64) -> Self {
        ParamValue::Literal(v)
    }
}

impl ParamValue {
    fn resolve(&self, params: &BTreeMap<String, f64>) -> Result<f64, SpecError> {
        match self {
            ParamValue::Literal(v) => Ok(*v),
            ParamValue::Named(name) => params
                .get(name)
                .copied()
                .ok_or_else(|| SpecError::UnknownParameter(name.clone())),
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            ParamValue::Literal(_) => None,
            ParamValue::Named(n) => Some(n),
        }
    }
}

/// Distribution spec with parameterizable fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DistSpec {
    /// Normal with explicit (possibly named) mean and σ.
    Normal {
        /// Centering error, mm.
        mean: ParamValue,
        /// Standard deviation, mm.
        sigma: ParamValue,
    },
    /// σ derived from the contributor's own resolved drawing tolerance
    /// under a stated convention (requires a symmetric ± band —
    /// asymmetric limits with this variant are an error, fail-closed).
    /// The drawing and the statistics cannot drift apart.
    NormalFromTol {
        /// The tolerance-to-σ convention.
        convention: SigmaConvention,
    },
    /// Uniform on [lo, hi] deviations.
    Uniform {
        /// Lower deviation bound, mm.
        lo: ParamValue,
        /// Upper deviation bound, mm.
        hi: ParamValue,
    },
    /// Two-state mix.
    TwoPoint {
        /// First state's deviation, mm.
        a: ParamValue,
        /// Second state's deviation, mm.
        b: ParamValue,
        /// Probability of state `b`.
        p_b: ParamValue,
    },
}

/// One contributor in the spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContributorSpec {
    /// Contributor name.
    pub name: String,
    /// Signed chain coefficient.
    pub coeff: ParamValue,
    /// Nominal dimension, mm — typically named after the driving
    /// document parameter.
    pub nominal: ParamValue,
    /// Lower drawing limit (deviation ≥ 0), mm.
    pub tol_minus: ParamValue,
    /// Upper drawing limit (deviation ≥ 0), mm.
    pub tol_plus: ParamValue,
    /// Deviation distribution.
    pub dist: DistSpec,
    /// Distribution provenance (defaults to assumed 3σ).
    #[serde(default)]
    pub source: DistributionSource,
}

/// Requirement spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequirementSpec {
    /// Requirement name — the adapter contract uses the clearance
    /// assertion's label here.
    pub name: String,
    /// Minimum acceptable gap, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower_mm: Option<ParamValue>,
    /// Maximum acceptable gap, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upper_mm: Option<ParamValue>,
}

/// Serializable stackup with named parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackupSpec {
    /// Stackup name.
    pub name: String,
    /// The chain.
    pub contributors: Vec<ContributorSpec>,
    /// The requirement.
    pub requirement: RequirementSpec,
}

/// Resolution failures.
#[derive(Debug, Clone, PartialEq)]
pub enum SpecError {
    /// A named parameter had no binding.
    UnknownParameter(String),
    /// [`DistSpec::NormalFromTol`] on an asymmetric ± band.
    AsymmetricNormalFromTol {
        /// Contributor name.
        contributor: String,
        /// Resolved (tol_minus, tol_plus).
        limits: (f64, f64),
    },
    /// The resolved stackup failed validation.
    Invalid(StackupError),
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::UnknownParameter(name) => {
                write!(f, "unbound stackup parameter: {name:?}")
            }
            SpecError::AsymmetricNormalFromTol {
                contributor,
                limits,
            } => write!(
                f,
                "normal_from_tol requires a symmetric band on {contributor:?}, got {limits:?}"
            ),
            SpecError::Invalid(e) => write!(f, "resolved stackup is invalid: {e}"),
        }
    }
}

impl std::error::Error for SpecError {}

impl From<StackupError> for SpecError {
    fn from(e: StackupError) -> Self {
        SpecError::Invalid(e)
    }
}

impl StackupSpec {
    /// Resolve every field against `params` and validate the result,
    /// fail-closed on unbound names and on invalid resolved models.
    pub fn resolve(&self, params: &BTreeMap<String, f64>) -> Result<Stackup, SpecError> {
        let contributors = self
            .contributors
            .iter()
            .map(|c| {
                let tol_minus = c.tol_minus.resolve(params)?;
                let tol_plus = c.tol_plus.resolve(params)?;
                let dist = match &c.dist {
                    DistSpec::Normal { mean, sigma } => Distribution::Normal {
                        mean: mean.resolve(params)?,
                        sigma: sigma.resolve(params)?,
                    },
                    DistSpec::NormalFromTol { convention } => {
                        if (tol_minus - tol_plus).abs() > 1e-12 {
                            return Err(SpecError::AsymmetricNormalFromTol {
                                contributor: c.name.clone(),
                                limits: (tol_minus, tol_plus),
                            });
                        }
                        Distribution::Normal {
                            mean: 0.0,
                            sigma: tol_plus / convention.k(),
                        }
                    }
                    DistSpec::Uniform { lo, hi } => Distribution::Uniform {
                        lo: lo.resolve(params)?,
                        hi: hi.resolve(params)?,
                    },
                    DistSpec::TwoPoint { a, b, p_b } => Distribution::TwoPoint {
                        a: a.resolve(params)?,
                        b: b.resolve(params)?,
                        p_b: p_b.resolve(params)?,
                    },
                };
                Ok(Contributor {
                    name: c.name.clone(),
                    coeff: c.coeff.resolve(params)?,
                    nominal: c.nominal.resolve(params)?,
                    tol_minus,
                    tol_plus,
                    dist,
                    source: c.source.clone(),
                })
            })
            .collect::<Result<Vec<_>, SpecError>>()?;
        let requirement = Requirement {
            name: self.requirement.name.clone(),
            lower_mm: self
                .requirement
                .lower_mm
                .as_ref()
                .map(|v| v.resolve(params))
                .transpose()?,
            upper_mm: self
                .requirement
                .upper_mm
                .as_ref()
                .map(|v| v.resolve(params))
                .transpose()?,
        };
        let s = Stackup {
            name: self.name.clone(),
            contributors,
            requirement,
        };
        s.validate()?;
        Ok(s)
    }

    /// A literal (parameter-free) spec mirroring `stackup` — the
    /// round-trip starting point for documents that don't parameterize.
    pub fn from_stackup(s: &Stackup) -> Self {
        Self {
            name: s.name.clone(),
            contributors: s
                .contributors
                .iter()
                .map(|c| ContributorSpec {
                    name: c.name.clone(),
                    coeff: c.coeff.into(),
                    nominal: c.nominal.into(),
                    tol_minus: c.tol_minus.into(),
                    tol_plus: c.tol_plus.into(),
                    dist: match c.dist {
                        Distribution::Normal { mean, sigma } => DistSpec::Normal {
                            mean: mean.into(),
                            sigma: sigma.into(),
                        },
                        Distribution::Uniform { lo, hi } => DistSpec::Uniform {
                            lo: lo.into(),
                            hi: hi.into(),
                        },
                        Distribution::TwoPoint { a, b, p_b } => DistSpec::TwoPoint {
                            a: a.into(),
                            b: b.into(),
                            p_b: p_b.into(),
                        },
                    },
                    source: c.source.clone(),
                })
                .collect(),
            requirement: RequirementSpec {
                name: s.requirement.name.clone(),
                lower_mm: s.requirement.lower_mm.map(Into::into),
                upper_mm: s.requirement.upper_mm.map(Into::into),
            },
        }
    }

    /// Every named parameter referenced by this spec — what the
    /// document must supply.
    pub fn parameter_names(&self) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let mut put = |v: &ParamValue| {
            if let Some(n) = v.name() {
                names.insert(n.to_string());
            }
        };
        for c in &self.contributors {
            put(&c.coeff);
            put(&c.nominal);
            put(&c.tol_minus);
            put(&c.tol_plus);
            match &c.dist {
                DistSpec::Normal { mean, sigma } => {
                    put(mean);
                    put(sigma);
                }
                DistSpec::NormalFromTol { .. } => {}
                DistSpec::Uniform { lo, hi } => {
                    put(lo);
                    put(hi);
                }
                DistSpec::TwoPoint { a, b, p_b } => {
                    put(a);
                    put(b);
                    put(p_b);
                }
            }
        }
        if let Some(v) = &self.requirement.lower_mm {
            put(v);
        }
        if let Some(v) = &self.requirement.upper_mm {
            put(v);
        }
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::rss;
    use crate::dist::SigmaConvention;

    fn params(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn json_round_trip_with_named_parameters() {
        let json = r#"{
            "name": "pocket-stack",
            "contributors": [
                {
                    "name": "pocket",
                    "coeff": 1.0,
                    "nominal": "pocket_depth",
                    "tol_minus": 0.15,
                    "tol_plus": 0.15,
                    "dist": { "type": "normal_from_tol",
                              "convention": { "type": "three_sigma" } }
                },
                {
                    "name": "bushing",
                    "coeff": -1.0,
                    "nominal": "bushing_len",
                    "tol_minus": 0.10,
                    "tol_plus": 0.10,
                    "dist": { "type": "normal",
                              "mean": 0.0, "sigma": "bushing_sigma" }
                }
            ],
            "requirement": {
                "name": "protrusion",
                "lower_mm": 0.2,
                "upper_mm": "max_protrusion"
            }
        }"#;
        let spec: StackupSpec = serde_json::from_str(json).expect("parse");
        assert_eq!(
            spec.parameter_names(),
            [
                "pocket_depth",
                "bushing_len",
                "bushing_sigma",
                "max_protrusion"
            ]
            .iter()
            .map(|s| s.to_string())
            .collect()
        );
        let s = spec
            .resolve(&params(&[
                ("pocket_depth", 20.0),
                ("bushing_len", 19.5),
                ("bushing_sigma", 0.03),
                ("max_protrusion", 0.8),
            ]))
            .expect("resolve");
        assert!((s.nominal_gap() - 0.5).abs() < 1e-12);
        // NormalFromTol derived σ = 0.15/3 = 0.05.
        assert!((s.contributors[0].dist.sigma() - 0.05).abs() < 1e-12);
        // The resolved model analyzes.
        let r = rss(&s).unwrap();
        assert!(r.yield_estimate > 0.9);

        // Serialize → parse: identical spec.
        let round = serde_json::to_string(&spec).expect("serialize");
        let spec2: StackupSpec = serde_json::from_str(&round).expect("reparse");
        assert_eq!(spec, spec2);
    }

    #[test]
    fn resolution_is_fail_closed_on_unbound_names() {
        let spec = StackupSpec {
            name: "s".into(),
            contributors: vec![ContributorSpec {
                name: "a".into(),
                coeff: 1.0.into(),
                nominal: ParamValue::Named("missing".into()),
                tol_minus: 0.1.into(),
                tol_plus: 0.1.into(),
                dist: DistSpec::NormalFromTol {
                    convention: SigmaConvention::ThreeSigma,
                },
                source: DistributionSource::default(),
            }],
            requirement: RequirementSpec {
                name: "gap".into(),
                lower_mm: Some(0.0.into()),
                upper_mm: None,
            },
        };
        assert_eq!(
            spec.resolve(&BTreeMap::new()).unwrap_err(),
            SpecError::UnknownParameter("missing".into())
        );
    }

    #[test]
    fn resolved_nonsense_is_fail_closed_too() {
        // A binding that makes a tolerance negative: resolution
        // succeeds numerically but validation must reject it.
        let mut spec = StackupSpec {
            name: "s".into(),
            contributors: vec![ContributorSpec {
                name: "a".into(),
                coeff: 1.0.into(),
                nominal: 10.0.into(),
                tol_minus: ParamValue::Named("t".into()),
                tol_plus: ParamValue::Named("t".into()),
                dist: DistSpec::NormalFromTol {
                    convention: SigmaConvention::ThreeSigma,
                },
                source: DistributionSource::default(),
            }],
            requirement: RequirementSpec {
                name: "gap".into(),
                lower_mm: Some(9.0.into()),
                upper_mm: None,
            },
        };
        let err = spec.resolve(&params(&[("t", -0.1)])).unwrap_err();
        assert!(matches!(err, SpecError::Invalid(_)), "{err:?}");

        // NormalFromTol on an asymmetric band is its own loud error.
        spec.contributors[0].tol_minus = 0.2.into();
        spec.contributors[0].tol_plus = 0.1.into();
        let err = spec.resolve(&BTreeMap::new()).unwrap_err();
        assert!(
            matches!(err, SpecError::AsymmetricNormalFromTol { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn from_stackup_round_trips_and_analyzes() {
        let conv = SigmaConvention::ThreeSigma;
        let s = Stackup {
            name: "rt".into(),
            contributors: vec![
                Contributor::normal("a", 1.0, 30.0, 0.3, conv),
                Contributor::uniform("b", -1.0, 29.2, 0.1, 0.0),
            ],
            requirement: Requirement::between("gap", 0.3, 1.4),
        };
        let spec = StackupSpec::from_stackup(&s);
        assert!(spec.parameter_names().is_empty());
        let resolved = spec.resolve(&BTreeMap::new()).expect("literal resolve");
        assert_eq!(resolved, s);
        // And it still analyzes.
        assert!(rss(&resolved).unwrap().yield_estimate > 0.5);
    }

    #[test]
    fn one_parameter_drives_many_fields() {
        // The point of the seam: one document parameter re-prices the
        // whole stackup. A shared "wall_tol" drives two contributors'
        // limits (and, via NormalFromTol, their σ).
        let spec = StackupSpec {
            name: "shared".into(),
            contributors: vec![
                ContributorSpec {
                    name: "wall_a".into(),
                    coeff: 1.0.into(),
                    nominal: 10.0.into(),
                    tol_minus: ParamValue::Named("wall_tol".into()),
                    tol_plus: ParamValue::Named("wall_tol".into()),
                    dist: DistSpec::NormalFromTol {
                        convention: SigmaConvention::ThreeSigma,
                    },
                    source: DistributionSource::default(),
                },
                ContributorSpec {
                    name: "wall_b".into(),
                    coeff: (-1.0).into(),
                    nominal: 9.4.into(),
                    tol_minus: ParamValue::Named("wall_tol".into()),
                    tol_plus: ParamValue::Named("wall_tol".into()),
                    dist: DistSpec::NormalFromTol {
                        convention: SigmaConvention::ThreeSigma,
                    },
                    source: DistributionSource::default(),
                },
            ],
            requirement: RequirementSpec {
                name: "gap".into(),
                lower_mm: Some(0.3.into()),
                upper_mm: Some(0.9.into()),
            },
        };
        let tight = spec.resolve(&params(&[("wall_tol", 0.1)])).unwrap();
        let loose = spec.resolve(&params(&[("wall_tol", 0.3)])).unwrap();
        let y_tight = rss(&tight).unwrap().yield_estimate;
        let y_loose = rss(&loose).unwrap().yield_estimate;
        assert!(
            y_tight > y_loose + 0.01,
            "tighter walls must raise yield: {y_tight} vs {y_loose}"
        );
    }
}
