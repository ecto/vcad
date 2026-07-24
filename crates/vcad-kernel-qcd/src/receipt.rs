//! Predicted-observable claims: `vcad.qcd-claims/1`.
//!
//! Same philosophy as the neutronics and acoustics families: a lattice
//! number without its statistical error and its recipe is not a claim,
//! it is a rumor. Claims here are `basis: predicted` and cap at
//! **Provisional** — nothing in the pure-gauge sector is bench-testable
//! by a vcad user, and no continuum limit is taken, so no claim ever
//! reads Pass. Every claim carries the caveat list in the same JSON
//! object.
//!
//! Fail-closed at construction:
//! - a run whose jackknife had fewer than [`MIN_BINS`] bins mints no
//!   claims (error bars from 2–3 bins are noise about noise);
//! - a non-finite or non-positive error mints no claims;
//! - Creutz ratios are only emitted when every constituent Wilson loop
//!   is resolved from zero by at least [`MIN_SIGNIFICANCE`] sigma —
//!   a log of a statistically-zero number is not a measurement.
//!
//! Registration of the family in `crates/vcad-receipt` + the MCP
//! surface is the flagged follow-up (cross-crate schema + TS codegen),
//! same staging as the particle and neutronics families.

use serde::{Deserialize, Serialize};

use crate::spec::{SimResult, WilsonLoop};

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.qcd-claims/1";

/// Minimum jackknife bins to mint any claim.
pub const MIN_BINS: usize = 5;

/// Minimum |mean|/err for a Wilson loop to enter a Creutz ratio.
pub const MIN_SIGNIFICANCE: f64 = 3.0;

/// Caveats attached to every claim in this family (M0).
pub fn caveats() -> Vec<String> {
    [
        "quenched SU(2) pure gauge — no dynamical fermions, not SU(3)",
        "lattice units at fixed coupling — no continuum extrapolation, no scale setting",
        "finite volume — no infinite-volume extrapolation",
        "statistical errors are binned jackknife; autocorrelation beyond the bin size is not corrected",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// One predicted claim with its statistical uncertainty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Claim name (snake_case).
    pub name: String,
    /// Value (dimensionless, lattice units).
    pub value: f64,
    /// Jackknife standard error.
    pub err: f64,
    /// Human-readable description.
    pub description: String,
}

/// The claim bundle for one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimSet {
    /// Always [`CLAIM_SCHEMA`].
    pub schema: String,
    /// Claims (plaquette, Wilson loops, Creutz ratios where resolvable).
    pub claims: Vec<Claim>,
    /// Caveat list — travels with the claims, always.
    pub caveats: Vec<String>,
    /// Run provenance echoed verbatim.
    pub provenance: crate::spec::Provenance,
}

/// Why a run minted no claims.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClaimError {
    /// Fewer bins than [`MIN_BINS`].
    TooFewBins {
        /// Bins in the run.
        n_bins: usize,
    },
    /// An observable carried a non-finite or non-positive error.
    DegenerateError {
        /// The offending observable name.
        name: String,
    },
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimError::TooFewBins { n_bins } => {
                write!(
                    f,
                    "claims need >= {MIN_BINS} jackknife bins, run has {n_bins}"
                )
            }
            ClaimError::DegenerateError { name } => {
                write!(f, "observable {name} has a degenerate error bar")
            }
        }
    }
}

impl std::error::Error for ClaimError {}

fn find(loops: &[WilsonLoop], r: usize, t: usize) -> Option<&WilsonLoop> {
    // Loops are stored with r <= t (plane-averaged symmetry).
    let (r, t) = if r <= t { (r, t) } else { (t, r) };
    loops.iter().find(|w| w.r == r && w.t == t)
}

fn significant(w: &WilsonLoop) -> bool {
    w.value.err > 0.0 && w.value.mean > 0.0 && w.value.mean / w.value.err >= MIN_SIGNIFICANCE
}

/// Build the claim set for a run, fail-closed.
pub fn build_claims(result: &SimResult) -> Result<ClaimSet, ClaimError> {
    if result.plaquette.n_bins < MIN_BINS {
        return Err(ClaimError::TooFewBins {
            n_bins: result.plaquette.n_bins,
        });
    }
    let mut claims = Vec::new();
    let check = |name: &str, mean: f64, err: f64| -> Result<Claim, ClaimError> {
        if !(err.is_finite() && err > 0.0 && mean.is_finite()) {
            return Err(ClaimError::DegenerateError {
                name: name.to_string(),
            });
        }
        Ok(Claim {
            name: name.to_string(),
            value: mean,
            err,
            description: String::new(),
        })
    };

    let mut c = check("plaquette", result.plaquette.mean, result.plaquette.err)?;
    c.description = "average plaquette (1/2)<Re Tr U_p>".to_string();
    claims.push(c);

    for w in &result.wilson_loops {
        let name = format!("wilson_loop_{}x{}", w.r, w.t);
        let mut c = check(&name, w.value.mean, w.value.err)?;
        c.description = format!("planar Wilson loop W({},{})", w.r, w.t);
        claims.push(c);
    }

    // Creutz ratios chi(r,t) = -ln( W(r,t) W(r-1,t-1) / (W(r,t-1) W(r-1,t)) ):
    // the lattice string-tension estimator. Emitted only when all four
    // loops are individually resolved from zero.
    let max_e = result.wilson_loops.iter().map(|w| w.t).max().unwrap_or(0);
    for r in 2..=max_e {
        let (a, b, c1, d) = (
            find(&result.wilson_loops, r, r),
            find(&result.wilson_loops, r - 1, r - 1),
            find(&result.wilson_loops, r, r - 1),
            find(&result.wilson_loops, r - 1, r),
        );
        if let (Some(a), Some(b), Some(c1), Some(d)) = (a, b, c1, d) {
            if r >= 2 && [a, b, c1, d].iter().all(|w| significant(w)) {
                let chi = -((a.value.mean * b.value.mean) / (c1.value.mean * d.value.mean)).ln();
                // First-order error propagation on the log.
                let rel = |w: &WilsonLoop| (w.value.err / w.value.mean).powi(2);
                let err = (rel(a) + rel(b) + rel(c1) + rel(d)).sqrt();
                let mut cl = check(&format!("creutz_ratio_{r}x{r}"), chi, err)?;
                cl.description = format!(
                    "Creutz ratio chi({r},{r}) — lattice string-tension estimator sigma*a^2"
                );
                claims.push(cl);
            }
        }
    }

    Ok(ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        claims,
        caveats: caveats(),
        provenance: result.provenance.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{run, SimSpec};

    fn spec() -> SimSpec {
        SimSpec {
            dims: [4, 4, 4, 4],
            beta: 2.2,
            thermalization_sweeps: 30,
            measurement_sweeps: 60,
            overrelax_per_heatbath: 1,
            bin_size: 6,
            max_wilson_extent: 2,
            seed: 5,
            hot_start: false,
        }
    }

    #[test]
    fn mints_plaquette_and_loop_claims() {
        let r = run(&spec()).unwrap();
        let cs = build_claims(&r).unwrap();
        assert_eq!(cs.schema, CLAIM_SCHEMA);
        assert!(cs.claims.iter().any(|c| c.name == "plaquette"));
        assert!(cs.claims.iter().any(|c| c.name == "wilson_loop_2x2"));
        assert!(!cs.caveats.is_empty());
        for c in &cs.claims {
            assert!(c.err > 0.0 && c.err.is_finite(), "{}", c.name);
        }
    }

    #[test]
    fn refuses_starved_runs() {
        let mut s = spec();
        s.measurement_sweeps = 12;
        s.bin_size = 6; // 2 bins: runnable, but below MIN_BINS for claims
        let r = run(&s).unwrap();
        assert!(matches!(
            build_claims(&r),
            Err(ClaimError::TooFewBins { n_bins: 2 })
        ));
    }

    #[test]
    fn claim_set_serializes() {
        let r = run(&spec()).unwrap();
        let cs = build_claims(&r).unwrap();
        let json = serde_json::to_string(&cs).unwrap();
        assert!(json.contains(CLAIM_SCHEMA));
        let back: ClaimSet = serde_json::from_str(&json).unwrap();
        assert_eq!(cs, back);
    }
}
