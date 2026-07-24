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

use crate::spec::{Gauge, SimResult};

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.qcd-claims/1";

/// Minimum jackknife bins to mint any claim.
pub const MIN_BINS: usize = 5;

/// Minimum |mean|/err for a Wilson loop to enter a Creutz ratio
/// (enforced in [`crate::analysis`]).
pub const MIN_SIGNIFICANCE: f64 = crate::analysis::MIN_SIGNIFICANCE;

/// Caveats attached to every claim in this family.
pub fn caveats(gauge: Gauge) -> Vec<String> {
    let group = match gauge {
        Gauge::Su2 => {
            "quenched SU(2) pure gauge — no dynamical fermions, SU(2) is not physical QCD's SU(3)"
        }
        Gauge::Su3 => "quenched SU(3) pure gauge — no dynamical fermions (no sea quarks)",
    };
    [
        group,
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

    // Creutz ratios (the lattice string-tension estimator), via the
    // analysis module — only ratios whose four loops are all resolved
    // ≥ 3σ from zero are emitted.
    let chis = crate::analysis::creutz_ratios(&result.wilson_loops);
    for chi in &chis {
        let mut cl = check(
            &format!("creutz_ratio_{}x{}", chi.r, chi.r),
            chi.chi,
            chi.err,
        )?;
        cl.description = format!(
            "Creutz ratio chi({},{}) — lattice string-tension estimator sigma*a^2",
            chi.r, chi.r
        );
        claims.push(cl);
    }
    // The headline string tension: the largest resolvable ratio (least
    // contaminated by the Coulomb term).
    if let Some(chi) = chis.last() {
        let mut cl = check("string_tension", chi.chi, chi.err)?;
        cl.description = format!(
            "string tension sigma*a^2 from chi({},{}) (lattice units)",
            chi.r, chi.r
        );
        claims.push(cl);
    }

    // Static potential points from temporal loops (M1).
    for p in crate::analysis::static_potential(&result.temporal_loops) {
        let mut cl = check(&format!("static_potential_r{}", p.r), p.v, p.err)?;
        cl.description = format!(
            "static quark potential V({})·a from W({},t) at t={}",
            p.r, p.r, p.t
        );
        claims.push(cl);
    }

    // Polyakov-loop magnitude (deconfinement order parameter, M3).
    if let Some(l) = &result.polyakov_abs {
        let mut cl = check("polyakov_abs", l.mean, l.err)?;
        cl.description =
            "volume-averaged Polyakov loop magnitude <|L|> (deconfinement order parameter)"
                .to_string();
        claims.push(cl);
    }

    Ok(ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        claims,
        caveats: caveats(result.provenance.spec.gauge),
        provenance: result.provenance.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{run, SimSpec};

    fn spec() -> SimSpec {
        SimSpec {
            beta: 2.2,
            thermalization_sweeps: 30,
            measurement_sweeps: 60,
            bin_size: 6,
            seed: 5,
            measure_polyakov: true,
            ..crate::spec::tests::base_spec()
        }
    }

    #[test]
    fn mints_polyakov_and_string_tension_claims() {
        let cs = build_claims(&run(&spec()).unwrap()).unwrap();
        assert!(cs.claims.iter().any(|c| c.name == "polyakov_abs"));
        // string_tension present iff a creutz ratio resolved; at β=2.2
        // on 4⁴ with 60 sweeps the 2x2 quad resolves.
        assert!(cs.claims.iter().any(|c| c.name == "string_tension"));
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
