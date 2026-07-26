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

use crate::mesh::{diagnose_thin_wall, tet_fill, wall_thickness, MeshError, ThinWallDiagnosis};
use crate::solve::{solve_static_full, NodeFields, Solution, SolveError, SolveOptions};
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
    /// The largest coarse-level resolution the caller's tier allows. Used
    /// only to tell a thin-wall diagnosis whether raising `resolution`
    /// could ever resolve the section, or whether nothing will.
    #[serde(default = "default_resolution_cap")]
    pub resolution_cap: usize,
}

fn default_resolution_cap() -> usize {
    256
}

impl Default for ConvergenceOptions {
    fn default() -> Self {
        Self {
            levels: 2,
            displacement_tol: 0.05,
            stress_tol: 0.15,
            resolution_cap: default_resolution_cap(),
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
    /// The part is thin-walled: the lattice could not be built at all at
    /// this pitch, and the measured diagnosis says why and what to do
    /// instead. Failing closed is right; failing without a route forward is
    /// what costs the caller an afternoon, so the route travels with the
    /// error.
    ThinWalled(Box<ThinWallDiagnosis>),
}

impl std::fmt::Display for ConvergenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConvergenceError::Mesh(e) => write!(f, "{e}"),
            ConvergenceError::Solve(e) => write!(f, "{e}"),
            ConvergenceError::InvalidOptions(m) => write!(f, "invalid convergence options: {m}"),
            ConvergenceError::ThinWalled(d) => write!(
                f,
                "{}",
                d.blocking_advice
                    .as_deref()
                    .unwrap_or("part is thinner than the lattice pitch")
            ),
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
    /// Measured wall thickness against the lattice pitch. When its
    /// `blocking_advice` is set, the verdict is Unverifiable regardless of
    /// what the QoIs did between levels — a study that never resolved the
    /// thin section can agree with itself and still be wrong.
    pub thin_wall: ThinWallDiagnosis,
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
    analyze_converged_fields(surface, spec, conv, solve_opts).map(|(a, _)| a)
}

/// Run the refinement study, also returning the finest level's per-node
/// fields (for viewport coloring). Fields are returned regardless of the
/// verdict — consumers must gate any *claims* on `converged()` themselves,
/// but an Unverifiable field picture is still useful diagnostic feedback.
pub fn analyze_converged_fields(
    surface: &TriangleMesh,
    spec: &FeaSpec,
    conv: &ConvergenceOptions,
    solve_opts: &SolveOptions,
) -> Result<(ConvergedAnalysis, NodeFields), ConvergenceError> {
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
    // Measure the part before solving it: a lattice that never puts enough
    // cells through the thinnest section is not converged, it is describing
    // a different part. The diagnosis carries the cell arithmetic and a
    // route forward so the caller never has to derive either.
    let finest_resolution = (spec.resolution << (conv.levels - 1)).min(256);
    let thin_wall = diagnose_thin_wall(
        wall_thickness(surface, 32)?,
        finest_resolution,
        conv.resolution_cap,
    );

    let mut levels = Vec::with_capacity(conv.levels);
    let mut finest_fields = None;
    for k in 0..conv.levels {
        let res = (spec.resolution << k).min(256);
        // When the lattice cannot even be built, the thin-wall diagnosis is
        // the useful error — not "no interior cells".
        let tm = tet_fill(surface, res).map_err(|e| {
            if thin_wall.blocking_advice.is_some() {
                ConvergenceError::ThinWalled(Box::new(thin_wall.clone()))
            } else {
                ConvergenceError::Mesh(e)
            }
        })?;
        let full = solve_static_full(&tm, spec, solve_opts)?;
        levels.push(full.summary);
        finest_fields = Some(full.fields);
    }
    let finest_fields = finest_fields.expect("levels >= 2");
    let fine = &levels[levels.len() - 1];
    let coarse = &levels[levels.len() - 2];
    let rel = |a: f64, b: f64| {
        let scale = b.abs().max(1e-300);
        (a - b).abs() / scale
    };
    let displacement_change_rel = rel(coarse.max_displacement_mm, fine.max_displacement_mm);
    let stress_change_rel = rel(coarse.max_von_mises_mpa, fine.max_von_mises_mpa);

    let mut reasons = Vec::new();
    if let Some(advice) = &thin_wall.blocking_advice {
        reasons.push(advice.clone());
    }
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
    Ok((
        ConvergedAnalysis {
            levels,
            displacement_change_rel,
            stress_change_rel,
            verdict,
            safety_factor,
            thin_wall,
            options: *conv,
        },
        finest_fields,
    ))
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        };
        let study = analyze_converged(&surface, &spec, &conv, &SolveOptions::default()).unwrap();
        assert!(study.converged(), "verdict {:?}", study.verdict);
        let sf = study.safety_factor.expect("safety factor");
        let expect = 276.0 / study.finest().max_von_mises_mpa;
        assert!((sf - expect).abs() < 1e-12);
        assert!(sf > 1.0, "aluminum cantilever at 100 N should not yield");
    }

    #[test]
    fn thin_walled_part_is_unverifiable_with_a_diagnosis_not_a_bare_refusal() {
        // A 200x60x6 mm plate at resolution 48: the lattice DOES fill
        // (2.9 cells through the 6 mm thickness at the finest level), so
        // the QoIs are real numbers that could agree with each other — and
        // the study must still refuse, with the arithmetic and a route
        // forward attached rather than leaving the caller to derive why.
        let surface = box_mesh([0.0, 0.0, 0.0], [200.0, 60.0, 6.0]);
        let mut spec = cantilever(48, Some(276.0));
        spec.loads[0].region = RegionBox {
            min: [200.0, 0.0, 0.0],
            max: [200.0, 60.0, 6.0],
        };
        spec.supports[0].region = RegionBox {
            min: [0.0, 0.0, 0.0],
            max: [0.0, 60.0, 6.0],
        };
        let conv = ConvergenceOptions {
            levels: 2,
            // Deliberately generous gates: the QoIs could agree with
            // themselves and the study must still refuse.
            displacement_tol: 10.0,
            stress_tol: 10.0,
            resolution_cap: 160,
        };
        let study = analyze_converged(&surface, &spec, &conv, &SolveOptions::default()).unwrap();
        assert!(
            (study.thin_wall.thickness.p05_mm - 6.0).abs() < 1e-6,
            "measured {:?}",
            study.thin_wall.thickness
        );
        assert!(study.thin_wall.cells_through_section < 4.0);
        assert!(!study.thin_wall.reachable, "{:?}", study.thin_wall);
        match &study.verdict {
            ConvergenceVerdict::Unverifiable { reasons } => {
                let joined = reasons.join(" ");
                assert!(joined.contains("THIN-WALLED"), "{joined}");
                assert!(joined.contains("beam_check"), "no route forward: {joined}");
            }
            v => panic!("a 2 mm wall at 1 cell must never read as converged: {v:?}"),
        }
        assert_eq!(study.safety_factor, None);
    }

    #[test]
    fn unfillable_thin_plate_errors_with_the_diagnosis_not_no_interior_cells() {
        // 200x60x2 mm at resolution 24: pitch 8.3 mm, no cell center lands
        // inside the plate at all. The old failure was a bare "no interior
        // cells — raise the resolution", which is advice that cannot work.
        let surface = box_mesh([0.0, 0.0, 0.0], [200.0, 60.0, 2.0]);
        let spec = cantilever(24, Some(276.0));
        let conv = ConvergenceOptions {
            resolution_cap: 160,
            ..Default::default()
        };
        let err = analyze_converged(&surface, &spec, &conv, &SolveOptions::default())
            .expect_err("a 2 mm plate cannot be latticed at an 8 mm pitch");
        let d = match &err {
            ConvergenceError::ThinWalled(d) => d.clone(),
            e => panic!("expected ThinWalled, got {e:?}"),
        };
        assert!(!d.reachable);
        let text = err.to_string();
        assert!(text.contains("THIN-WALLED"), "{text}");
        assert!(text.contains("beam_check"), "no route forward: {text}");
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
