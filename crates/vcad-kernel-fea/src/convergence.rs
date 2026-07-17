//! Mesh-convergence gating (M1): fail-closed refinement study.
//!
//! A single-resolution FE number is an anecdote. This module solves the
//! same case at two (or more) lattice refinements, reports the
//! inter-level relative change of each QoI as its discretization-error
//! estimate, and renders a verdict: **Converged** only when the changes
//! sit inside stated tolerances, otherwise **Unverifiable** with the
//! reason spelled out. Nothing downstream (claims, safety factor) is
//! emitted from an Unverifiable study — the same fail-closed contract as
//! the thermal and particle crates.
//!
//! Displacement is a global, well-behaved QoI (gate default 5%). Max von
//! Mises is pointwise and mesh-sensitive — constant-strain tets smear
//! concentrations, and at a genuinely singular re-entrant corner the true
//! elastic stress is unbounded, so *no* mesh converges there. The stress
//! gate default is a looser 15%; a part that fails it either needs more
//! resolution or has a singularity that linear elasticity cannot price
//! (add a fillet and re-run — which is design feedback, not solver
//! failure).

use serde::{Deserialize, Serialize};
use vcad_kernel_tessellate::TriangleMesh;

use crate::mesh::{tet_fill, MeshError};
use crate::solve::{solve_static, Solution, SolveError, SolveOptions};
use crate::spec::FeaSpec;

/// Controls for the refinement study.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ConvergenceOptions {
    /// Refinement levels to solve, ≥ 2. Level `k` uses resolution
    /// `spec.resolution * 2^k` (capped at 256 by the mesher).
    pub levels: usize,
    /// Relative-change gate on max displacement between the two finest
    /// levels.
    pub displacement_tol: f64,
    /// Relative-change gate on max von Mises stress.
    pub stress_tol: f64,
}

impl Default for ConvergenceOptions {
    fn default() -> Self {
        Self {
            levels: 2,
            displacement_tol: 0.05,
            stress_tol: 0.15,
        }
    }
}

/// Study failures (a *verdict* of Unverifiable is not an error — errors
/// are cases where the study could not run at all).
#[derive(Debug)]
pub enum ConvergenceError {
    /// Meshing failed at some level.
    Mesh(MeshError),
    /// A solve failed at some level.
    Solve(SolveError),
    /// The options are invalid.
    InvalidOptions(String),
}

impl std::fmt::Display for ConvergenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConvergenceError::Mesh(e) => write!(f, "{e}"),
            ConvergenceError::Solve(e) => write!(f, "{e}"),
            ConvergenceError::InvalidOptions(m) => write!(f, "invalid convergence options: {m}"),
        }
    }
}

impl std::error::Error for ConvergenceError {}

impl From<MeshError> for ConvergenceError {
    fn from(e: MeshError) -> Self {
        ConvergenceError::Mesh(e)
    }
}

impl From<SolveError> for ConvergenceError {
    fn from(e: SolveError) -> Self {
        ConvergenceError::Solve(e)
    }
}

/// The gate verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict")]
pub enum ConvergenceVerdict {
    /// Both QoIs stable across the finest refinement step.
    Converged,
    /// Not demonstrated converged; the reasons say which gate failed.
    Unverifiable {
        /// Human-readable reasons, one per failed gate.
        reasons: Vec<String>,
    },
}

/// The full refinement study.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvergedAnalysis {
    /// Per-level solutions, coarsest first. The *finest* level is the
    /// reported answer.
    pub levels: Vec<Solution>,
    /// |Δ max displacement| / finest, between the two finest levels.
    pub displacement_change_rel: f64,
    /// |Δ max von Mises| / finest, between the two finest levels.
    pub stress_change_rel: f64,
    /// The gate verdict.
    pub verdict: ConvergenceVerdict,
    /// Safety factor `yield / max_von_mises` from the finest level —
    /// **only** when the study converged and a yield strength was given.
    pub safety_factor: Option<f64>,
    /// Options the study ran with (provenance).
    pub options: ConvergenceOptions,
}

impl ConvergedAnalysis {
    /// The finest-level solution (the reported answer).
    pub fn finest(&self) -> &Solution {
        self.levels.last().expect("study has >= 2 levels")
    }

    /// True when the gate passed.
    pub fn converged(&self) -> bool {
        self.verdict == ConvergenceVerdict::Converged
    }
}

/// Run the refinement study on a part's surface mesh.
pub fn analyze_converged(
    surface: &TriangleMesh,
    spec: &FeaSpec,
    conv: &ConvergenceOptions,
    solve_opts: &SolveOptions,
) -> Result<ConvergedAnalysis, ConvergenceError> {
    if conv.levels < 2 {
        return Err(ConvergenceError::InvalidOptions(
            "levels must be >= 2 (one solve is an anecdote, not a study)".into(),
        ));
    }
    if !(conv.displacement_tol > 0.0 && conv.stress_tol > 0.0) {
        return Err(ConvergenceError::InvalidOptions(
            "tolerances must be positive".into(),
        ));
    }
    let mut levels = Vec::with_capacity(conv.levels);
    for k in 0..conv.levels {
        let res = (spec.resolution << k).min(256);
        let tm = tet_fill(surface, res)?;
        levels.push(solve_static(&tm, spec, solve_opts)?);
    }
    let fine = &levels[levels.len() - 1];
    let coarse = &levels[levels.len() - 2];
    let rel = |a: f64, b: f64| {
        let scale = b.abs().max(1e-300);
        (a - b).abs() / scale
    };
    let displacement_change_rel = rel(coarse.max_displacement_mm, fine.max_displacement_mm);
    let stress_change_rel = rel(coarse.max_von_mises_mpa, fine.max_von_mises_mpa);

    let mut reasons = Vec::new();
    if displacement_change_rel > conv.displacement_tol {
        reasons.push(format!(
            "max displacement changed {:.1}% between the two finest levels (gate {:.1}%) — \
             raise resolution",
            100.0 * displacement_change_rel,
            100.0 * conv.displacement_tol
        ));
    }
    if stress_change_rel > conv.stress_tol {
        reasons.push(format!(
            "max von Mises changed {:.1}% between the two finest levels (gate {:.1}%) — raise \
             resolution, or the peak sits at a stress singularity (sharp re-entrant corner) \
             that linear elasticity cannot price; fillet it",
            100.0 * stress_change_rel,
            100.0 * conv.stress_tol
        ));
    }
    let verdict = if reasons.is_empty() {
        ConvergenceVerdict::Converged
    } else {
        ConvergenceVerdict::Unverifiable { reasons }
    };
    let safety_factor = match (&verdict, spec.yield_strength_mpa) {
        (ConvergenceVerdict::Converged, Some(y)) if fine.max_von_mises_mpa > 0.0 => {
            Some(y / fine.max_von_mises_mpa)
        }
        _ => None,
    };
    Ok(ConvergedAnalysis {
        levels,
        displacement_change_rel,
        stress_change_rel,
        verdict,
        safety_factor,
        options: *conv,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::box_mesh;
    use crate::spec::{Load, RegionBox, Support};

    fn cantilever(res: usize, yield_mpa: Option<f64>) -> FeaSpec {
        FeaSpec {
            resolution: res,
            youngs_modulus_mpa: 69_000.0,
            poisson: 0.33,
            yield_strength_mpa: yield_mpa,
            loads: vec![Load {
                region: RegionBox {
                    min: [80.0, 0.0, 0.0],
                    max: [80.0, 10.0, 10.0],
                },
                force: [0.0, 0.0, -100.0],
            }],
            supports: vec![Support {
                region: RegionBox {
                    min: [0.0, 0.0, 0.0],
                    max: [0.0, 10.0, 10.0],
                },
                fix: [true, true, true],
            }],
        }
    }

    #[test]
    fn cantilever_converges_toward_beam_theory() {
        // 80×10×10 mm aluminum cantilever, 100 N tip load.
        // Timoshenko: δ = FL³/(3EI) + FL/(κGA) ≈ 0.297 + 0.004 ≈ 0.301 mm.
        // The fully-clamped root face and staircase-free box geometry make
        // this the cleanest closed-form check; constant-strain tets
        // converge from below (too stiff), so require monotone approach
        // and a finest-level value within a modest band.
        let surface = box_mesh([0.0; 3], [80.0, 10.0, 10.0]);
        let spec = cantilever(24, Some(276.0));
        let conv = ConvergenceOptions {
            levels: 3,
            displacement_tol: 0.10,
            stress_tol: 0.35,
        };
        let study = analyze_converged(&surface, &spec, &conv, &SolveOptions::default()).unwrap();
        assert_eq!(study.levels.len(), 3);
        let d: Vec<f64> = study.levels.iter().map(|l| l.max_displacement_mm).collect();
        assert!(d[0] < d[1] && d[1] < d[2], "not monotone from below: {d:?}");
        let timoshenko = 0.301;
        let rel = (study.finest().max_displacement_mm - timoshenko).abs() / timoshenko;
        assert!(
            rel < 0.12,
            "finest tip {} vs Timoshenko {timoshenko}, rel {rel}",
            study.finest().max_displacement_mm
        );
        // Root bending stress sigma = Mc/I ≈ 48 MPa; smeared estimate in band.
        let vm = study.finest().max_von_mises_mpa;
        assert!(vm > 25.0 && vm < 90.0, "root vm {vm}");
        // Max deflection at the loaded tip, max stress near the root.
        assert!(study.finest().max_displacement_at[0] > 70.0);
        assert!(study.finest().max_stress_at[0] < 20.0);
    }

    #[test]
    fn unconverged_study_is_unverifiable_and_claims_nothing() {
        // Deliberately absurd coarse start: resolution 2 halves nothing.
        let surface = box_mesh([0.0; 3], [80.0, 10.0, 10.0]);
        let spec = cantilever(8, Some(276.0));
        let conv = ConvergenceOptions {
            levels: 2,
            displacement_tol: 0.01, // unreachably tight for this pair
            stress_tol: 0.01,
        };
        let study = analyze_converged(&surface, &spec, &conv, &SolveOptions::default()).unwrap();
        match &study.verdict {
            ConvergenceVerdict::Unverifiable { reasons } => {
                assert!(!reasons.is_empty());
            }
            v => panic!("expected Unverifiable, got {v:?}"),
        }
        assert_eq!(
            study.safety_factor, None,
            "no safety factor without convergence"
        );
    }

    #[test]
    fn converged_study_reports_safety_factor() {
        let surface = box_mesh([0.0; 3], [80.0, 10.0, 10.0]);
        let spec = cantilever(32, Some(276.0));
        let conv = ConvergenceOptions {
            levels: 2,
            displacement_tol: 0.25,
            stress_tol: 0.40,
        };
        let study = analyze_converged(&surface, &spec, &conv, &SolveOptions::default()).unwrap();
        assert!(study.converged(), "verdict {:?}", study.verdict);
        let sf = study.safety_factor.expect("safety factor");
        let expect = 276.0 / study.finest().max_von_mises_mpa;
        assert!((sf - expect).abs() < 1e-12);
        assert!(sf > 1.0, "aluminum cantilever at 100 N should not yield");
    }

    #[test]
    fn options_validated() {
        let surface = box_mesh([0.0; 3], [10.0; 3]);
        let spec = cantilever(8, None);
        let bad = ConvergenceOptions {
            levels: 1,
            ..Default::default()
        };
        assert!(matches!(
            analyze_converged(&surface, &spec, &bad, &SolveOptions::default()),
            Err(ConvergenceError::InvalidOptions(_))
        ));
    }
}
