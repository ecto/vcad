//! The stackup model: contributors, the gap requirement, and fail-closed
//! validation.
//!
//! A [`Stackup`] models a **linear** dimension chain: the gap of
//! interest is
//!
//! ```text
//! G = Σᵢ aᵢ · xᵢ
//! ```
//!
//! where `xᵢ` is contributor *i*'s actual dimension and `aᵢ` its signed
//! sensitivity coefficient (+1 for dimensions that open the gap, −1 for
//! dimensions that consume it, any real value for lever ratios or
//! projected vector-loop legs — see the `loops` module). Linearity is
//! the M0 scope boundary and it is exact for 1-D chains; vector loops
//! are linearized (small-angle) before they get here.
//!
//! Contributors are assumed **statistically independent**. Correlated
//! contributors (two dimensions cut in one fixture setup) violate RSS
//! and Monte Carlo alike; modeling correlation is future work and the
//! docs say so rather than pretending otherwise.

use serde::{Deserialize, Serialize};

use crate::dist::{Distribution, DistributionSource, SigmaConvention};
use crate::rng::Rng;

/// One dimensional contributor in the chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contributor {
    /// Human-readable name (unique within a stackup).
    pub name: String,
    /// Signed sensitivity coefficient `aᵢ` (∂gap/∂dimension, exact for
    /// linear chains): +1 opens the gap, −1 consumes it.
    pub coeff: f64,
    /// Nominal dimension, mm.
    pub nominal: f64,
    /// Lower drawing limit as a deviation: the dimension may be up to
    /// this much *below* nominal (≥ 0), mm.
    pub tol_minus: f64,
    /// Upper drawing limit as a deviation: the dimension may be up to
    /// this much *above* nominal (≥ 0), mm.
    pub tol_plus: f64,
    /// Statistical model of the deviation from nominal. Worst-case
    /// analysis ignores this and uses the drawing limits above.
    pub dist: Distribution,
    /// Where the distribution came from (assumption vs measurement).
    #[serde(default)]
    pub source: DistributionSource,
}

impl Contributor {
    /// Symmetric ± tolerance, centered normal process under `convention`
    /// (σ = tol / k). The standard machined-dimension contributor.
    pub fn normal(
        name: &str,
        coeff: f64,
        nominal: f64,
        tol: f64,
        convention: SigmaConvention,
    ) -> Self {
        Self {
            name: name.to_string(),
            coeff,
            nominal,
            tol_minus: tol,
            tol_plus: tol,
            dist: Distribution::Normal {
                mean: 0.0,
                sigma: tol / convention.k(),
            },
            source: DistributionSource::Assumed { convention },
        }
    }

    /// Asymmetric drawing limits with a uniform deviation spanning the
    /// whole band — the honest vendor-lot model (e.g. bearing width
    /// 15.0 +0/−0.12).
    pub fn uniform(name: &str, coeff: f64, nominal: f64, tol_minus: f64, tol_plus: f64) -> Self {
        Self {
            name: name.to_string(),
            coeff,
            nominal,
            tol_minus,
            tol_plus,
            dist: Distribution::Uniform {
                lo: -tol_minus,
                hi: tol_plus,
            },
            source: DistributionSource::Assumed {
                convention: SigmaConvention::ThreeSigma,
            },
        }
    }

    /// General constructor: explicit drawing limits and an explicit
    /// deviation distribution (e.g. a two-point supplier mix inside a
    /// wider drawing band).
    pub fn with_dist(
        name: &str,
        coeff: f64,
        nominal: f64,
        tol_minus: f64,
        tol_plus: f64,
        dist: Distribution,
    ) -> Self {
        Self {
            name: name.to_string(),
            coeff,
            nominal,
            tol_minus,
            tol_plus,
            dist,
            source: DistributionSource::Assumed {
                convention: SigmaConvention::ThreeSigma,
            },
        }
    }

    /// Mean actual dimension: nominal plus the process centering error.
    pub fn mean_dimension(&self) -> f64 {
        self.nominal + self.dist.mean()
    }
}

/// ISO 2768-1 general tolerance class (linear dimensions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Iso2768Class {
    /// Fine.
    F,
    /// Medium — the default on most machine-shop drawings.
    M,
    /// Coarse.
    C,
}

/// ISO 2768-1:1989 Table 1 general tolerances for linear dimensions:
/// the ± tolerance for a nominal dimension under the given class.
/// Returns `None` outside the table's domain (below 0.5 mm the
/// standard requires an explicit tolerance; above 4000 mm it is
/// silent) — fail-closed, never an extrapolation.
pub fn iso2768(nominal_mm: f64, class: Iso2768Class) -> Option<f64> {
    if !nominal_mm.is_finite() || nominal_mm < 0.5 {
        return None;
    }
    // Band upper edges and ± values per class (f, m, c).
    const BANDS: [(f64, f64, f64, f64); 8] = [
        (3.0, 0.05, 0.1, 0.2),
        (6.0, 0.05, 0.1, 0.3),
        (30.0, 0.1, 0.2, 0.5),
        (120.0, 0.15, 0.3, 0.8),
        (400.0, 0.2, 0.5, 1.2),
        (1000.0, 0.3, 0.8, 2.0),
        (2000.0, 0.5, 1.2, 3.0),
        (4000.0, f64::NAN, 2.0, 4.0), // class f is unspecified here
    ];
    for (hi, f, m, c) in BANDS {
        if nominal_mm <= hi {
            let t = match class {
                Iso2768Class::F => f,
                Iso2768Class::M => m,
                Iso2768Class::C => c,
            };
            return if t.is_nan() { None } else { Some(t) };
        }
    }
    None
}

/// The gap requirement: at least one of the bounds must be present
/// (fail-closed — a stackup with nothing to check is a modeling error,
/// not a trivially passing one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Requirement {
    /// What the gap means ("axial play", "radial clearance").
    pub name: String,
    /// Minimum acceptable gap, mm (e.g. 0.05 so parts never jam).
    pub lower_mm: Option<f64>,
    /// Maximum acceptable gap, mm (e.g. 0.75 so the shaft can't rattle).
    pub upper_mm: Option<f64>,
}

impl Requirement {
    /// Two-sided requirement.
    pub fn between(name: &str, lower_mm: f64, upper_mm: f64) -> Self {
        Self {
            name: name.to_string(),
            lower_mm: Some(lower_mm),
            upper_mm: Some(upper_mm),
        }
    }

    /// One-sided minimum-clearance requirement.
    pub fn at_least(name: &str, lower_mm: f64) -> Self {
        Self {
            name: name.to_string(),
            lower_mm: Some(lower_mm),
            upper_mm: None,
        }
    }
}

/// A complete stackup: the chain plus its requirement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stackup {
    /// Assembly-level name.
    pub name: String,
    /// The dimension chain.
    pub contributors: Vec<Contributor>,
    /// The gap requirement.
    pub requirement: Requirement,
}

/// Validation failures. Every analysis validates first; a malformed
/// stackup never produces a number.
#[derive(Debug, Clone, PartialEq)]
pub enum StackupError {
    /// The chain has no contributors.
    Empty,
    /// A contributor name is empty or duplicated.
    BadName(String),
    /// A numeric field is NaN or infinite.
    NonFinite {
        /// Contributor name.
        contributor: String,
        /// Field description.
        field: &'static str,
    },
    /// A coefficient is exactly zero — a dead contributor is a modeling
    /// bug, not a harmless no-op.
    ZeroCoefficient(String),
    /// A drawing tolerance is negative.
    NegativeTolerance(String),
    /// A distribution's parameters are structurally invalid.
    InvalidDistribution {
        /// Contributor name.
        contributor: String,
        /// Reason from the distribution check.
        reason: String,
    },
    /// A bounded distribution puts mass outside the drawing limits: the
    /// statistical model contradicts the drawing.
    DistributionExceedsLimits {
        /// Contributor name.
        contributor: String,
        /// Offending support bound (deviation, mm).
        support: (f64, f64),
        /// Drawing limits as deviations (−tol_minus, +tol_plus), mm.
        limits: (f64, f64),
    },
    /// The requirement has neither a lower nor an upper bound.
    UnboundedRequirement,
    /// The requirement bounds are inverted or non-finite.
    BadRequirement(String),
    /// Every contributor has zero variance — statistical analysis is
    /// meaningless (worst-case still works; use that).
    DegenerateChain,
    /// Monte Carlo sample count too small for meaningful error bars.
    TooFewSamples {
        /// Requested sample count.
        n: usize,
        /// Minimum accepted.
        min: usize,
    },
    /// A tolerance-allocation target yield cannot be met even with
    /// every allocatable tolerance at its tightest.
    Infeasible {
        /// The requested yield floor.
        target_yield: f64,
        /// The best yield achievable inside the boxes.
        best_yield: f64,
    },
    /// A contributor named for allocation cannot be allocated (not an
    /// assumed-normal contributor, or not in the chain).
    NotAllocatable(String),
}

impl std::fmt::Display for StackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StackupError::Empty => write!(f, "stackup has no contributors"),
            StackupError::BadName(n) => write!(f, "bad contributor name: {n:?}"),
            StackupError::NonFinite { contributor, field } => {
                write!(f, "non-finite {field} on contributor {contributor:?}")
            }
            StackupError::ZeroCoefficient(n) => {
                write!(f, "contributor {n:?} has zero coefficient")
            }
            StackupError::NegativeTolerance(n) => {
                write!(f, "contributor {n:?} has a negative drawing tolerance")
            }
            StackupError::InvalidDistribution {
                contributor,
                reason,
            } => write!(f, "invalid distribution on {contributor:?}: {reason}"),
            StackupError::DistributionExceedsLimits {
                contributor,
                support,
                limits,
            } => write!(
                f,
                "distribution support {support:?} exceeds drawing limits {limits:?} on {contributor:?}"
            ),
            StackupError::UnboundedRequirement => {
                write!(f, "requirement needs at least one bound")
            }
            StackupError::BadRequirement(r) => write!(f, "bad requirement: {r}"),
            StackupError::DegenerateChain => {
                write!(f, "every contributor has zero variance; statistical analysis is meaningless")
            }
            StackupError::TooFewSamples { n, min } => {
                write!(f, "monte carlo needs at least {min} samples, got {n}")
            }
            StackupError::Infeasible {
                target_yield,
                best_yield,
            } => write!(
                f,
                "target yield {target_yield} is infeasible; best achievable is {best_yield}"
            ),
            StackupError::NotAllocatable(name) => {
                write!(f, "contributor {name:?} is not allocatable")
            }
        }
    }
}

impl std::error::Error for StackupError {}

impl Stackup {
    /// Validate the model, fail-closed. Called by every analysis.
    pub fn validate(&self) -> Result<(), StackupError> {
        if self.contributors.is_empty() {
            return Err(StackupError::Empty);
        }
        let mut seen = std::collections::BTreeSet::new();
        for c in &self.contributors {
            if c.name.is_empty() || !seen.insert(c.name.as_str()) {
                return Err(StackupError::BadName(c.name.clone()));
            }
            for (v, field) in [
                (c.coeff, "coeff"),
                (c.nominal, "nominal"),
                (c.tol_minus, "tol_minus"),
                (c.tol_plus, "tol_plus"),
            ] {
                if !v.is_finite() {
                    return Err(StackupError::NonFinite {
                        contributor: c.name.clone(),
                        field,
                    });
                }
            }
            if c.coeff == 0.0 {
                return Err(StackupError::ZeroCoefficient(c.name.clone()));
            }
            if c.tol_minus < 0.0 || c.tol_plus < 0.0 {
                return Err(StackupError::NegativeTolerance(c.name.clone()));
            }
            c.dist
                .check()
                .map_err(|reason| StackupError::InvalidDistribution {
                    contributor: c.name.clone(),
                    reason,
                })?;
            if let Some((lo, hi)) = c.dist.support() {
                // Small slack so exactly-at-the-limit supports pass in
                // the presence of float noise.
                let eps = 1e-9;
                if lo < -c.tol_minus - eps || hi > c.tol_plus + eps {
                    return Err(StackupError::DistributionExceedsLimits {
                        contributor: c.name.clone(),
                        support: (lo, hi),
                        limits: (-c.tol_minus, c.tol_plus),
                    });
                }
            }
        }
        match (self.requirement.lower_mm, self.requirement.upper_mm) {
            (None, None) => return Err(StackupError::UnboundedRequirement),
            (Some(l), Some(u)) => {
                if !l.is_finite() || !u.is_finite() {
                    return Err(StackupError::BadRequirement("non-finite bound".into()));
                }
                if l >= u {
                    return Err(StackupError::BadRequirement(format!(
                        "lower {l} must be < upper {u}"
                    )));
                }
            }
            (Some(b), None) | (None, Some(b)) => {
                if !b.is_finite() {
                    return Err(StackupError::BadRequirement("non-finite bound".into()));
                }
            }
        }
        Ok(())
    }

    /// Nominal gap: Σ aᵢ·nominalᵢ, mm.
    pub fn nominal_gap(&self) -> f64 {
        self.contributors.iter().map(|c| c.coeff * c.nominal).sum()
    }

    /// Mean gap including process centering errors: Σ aᵢ·(nominalᵢ + μᵢ), mm.
    pub fn mean_gap(&self) -> f64 {
        self.contributors
            .iter()
            .map(|c| c.coeff * c.mean_dimension())
            .sum()
    }

    /// Gap variance under independence: Σ aᵢ²·σᵢ², mm².
    pub fn variance_gap(&self) -> f64 {
        self.contributors
            .iter()
            .map(|c| c.coeff * c.coeff * c.dist.variance())
            .sum()
    }

    /// Draw one virtual assembly's gap.
    pub fn sample_gap(&self, rng: &mut Rng) -> f64 {
        self.contributors
            .iter()
            .map(|c| c.coeff * (c.nominal + c.dist.sample(rng)))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> Stackup {
        Stackup {
            name: "test".into(),
            contributors: vec![
                Contributor::normal("housing", 1.0, 62.0, 0.3, SigmaConvention::ThreeSigma),
                Contributor::uniform("bearing", -1.0, 15.0, 0.12, 0.0),
                Contributor::with_dist(
                    "spacer",
                    -1.0,
                    12.0,
                    0.1,
                    0.1,
                    Distribution::TwoPoint {
                        a: -0.03,
                        b: 0.04,
                        p_b: 0.4,
                    },
                ),
            ],
            requirement: Requirement::between("gap", 0.05, 0.75),
        }
    }

    #[test]
    fn valid_chain_validates_and_computes_moments() {
        let s = chain();
        s.validate().expect("valid");
        assert!((s.nominal_gap() - 35.0).abs() < 1e-12);
        // mean = 35 + 0 − (−0.06) − (−0.03·0.6 + 0.04·0.4) = 35.06 + 0.002
        assert!((s.mean_gap() - 35.062).abs() < 1e-12, "{}", s.mean_gap());
        let var = 0.01 + 0.12 * 0.12 / 12.0 + 0.07 * 0.07 * 0.4 * 0.6;
        assert!((s.variance_gap() - var).abs() < 1e-15);
    }

    #[test]
    fn validation_is_fail_closed() {
        let mut s = chain();
        s.contributors.clear();
        assert_eq!(s.validate(), Err(StackupError::Empty));

        let mut s = chain();
        s.contributors[1].name = "housing".into();
        assert!(matches!(s.validate(), Err(StackupError::BadName(_))));

        let mut s = chain();
        s.contributors[0].coeff = 0.0;
        assert!(matches!(
            s.validate(),
            Err(StackupError::ZeroCoefficient(_))
        ));

        let mut s = chain();
        s.contributors[0].tol_plus = -0.1;
        assert!(matches!(
            s.validate(),
            Err(StackupError::NegativeTolerance(_))
        ));

        let mut s = chain();
        s.contributors[0].nominal = f64::INFINITY;
        assert!(matches!(s.validate(), Err(StackupError::NonFinite { .. })));

        let mut s = chain();
        s.requirement.lower_mm = None;
        s.requirement.upper_mm = None;
        assert_eq!(s.validate(), Err(StackupError::UnboundedRequirement));

        let mut s = chain();
        s.requirement.lower_mm = Some(1.0);
        s.requirement.upper_mm = Some(0.5);
        assert!(matches!(s.validate(), Err(StackupError::BadRequirement(_))));
    }

    #[test]
    fn drawing_limits_must_contain_bounded_support() {
        // A uniform wider than the drawing band: the model contradicts
        // the drawing — error, never a silent clip.
        let mut s = chain();
        s.contributors[1].dist = Distribution::Uniform { lo: -0.2, hi: 0.0 };
        assert!(matches!(
            s.validate(),
            Err(StackupError::DistributionExceedsLimits { .. })
        ));
        // Normal is unbounded by design and passes (documented).
        let mut s = chain();
        s.contributors[0].dist = Distribution::Normal {
            mean: 0.0,
            sigma: 10.0,
        };
        s.validate()
            .expect("normal tails are not a limits violation");
    }

    #[test]
    fn sampling_matches_moments() {
        let s = chain();
        let mut rng = Rng::new(11);
        let n = 100_000;
        let mut sum = 0.0;
        let mut sum_sq = 0.0;
        for _ in 0..n {
            let g = s.sample_gap(&mut rng);
            sum += g;
            sum_sq += g * g;
        }
        let mean = sum / n as f64;
        let var = sum_sq / n as f64 - mean * mean;
        let se = (s.variance_gap() / n as f64).sqrt();
        assert!((mean - s.mean_gap()).abs() < 5.0 * se);
        assert!((var - s.variance_gap()).abs() / s.variance_gap() < 0.05);
    }

    #[test]
    fn iso2768_table_values() {
        // Spot checks against ISO 2768-1:1989 Table 1.
        assert_eq!(iso2768(25.0, Iso2768Class::M), Some(0.2));
        assert_eq!(iso2768(62.0, Iso2768Class::M), Some(0.3));
        assert_eq!(iso2768(62.0, Iso2768Class::F), Some(0.15));
        assert_eq!(iso2768(62.0, Iso2768Class::C), Some(0.8));
        assert_eq!(iso2768(1.6, Iso2768Class::M), Some(0.1));
        assert_eq!(iso2768(350.0, Iso2768Class::M), Some(0.5));
        assert_eq!(iso2768(3000.0, Iso2768Class::M), Some(2.0));
        // Fail-closed outside the domain.
        assert_eq!(iso2768(0.4, Iso2768Class::M), None);
        assert_eq!(iso2768(3000.0, Iso2768Class::F), None);
        assert_eq!(iso2768(5000.0, Iso2768Class::C), None);
        assert_eq!(iso2768(f64::NAN, Iso2768Class::M), None);
    }

    #[test]
    fn serde_round_trip() {
        let s = chain();
        let json = serde_json::to_string_pretty(&s).unwrap();
        let back: Stackup = serde_json::from_str(&json).unwrap();
        // serde_json's default float parse can move the last ULP (the
        // `float_roundtrip` feature trades speed for exactness) — same
        // lesson as vcad-kernel-particle: compare structurally with a
        // relative tolerance, not bitwise.
        assert_eq!(back.name, s.name);
        assert_eq!(back.requirement, s.requirement);
        assert_eq!(back.contributors.len(), s.contributors.len());
        for (a, b) in back.contributors.iter().zip(&s.contributors) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.source, b.source);
            let close = |x: f64, y: f64| (x - y).abs() <= 1e-12 * (1.0 + y.abs());
            assert!(close(a.coeff, b.coeff));
            assert!(close(a.nominal, b.nominal));
            assert!(close(a.dist.mean(), b.dist.mean()));
            assert!(
                close(a.dist.sigma(), b.dist.sigma()),
                "{} sigma drifted: {} vs {}",
                a.name,
                a.dist.sigma(),
                b.dist.sigma()
            );
        }
    }
}
