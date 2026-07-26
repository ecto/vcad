//! Predicted-performance claims for the design receipt (M2).
//!
//! Emits the `vcad.fea-claims/1` set — `max_von_mises_mpa`,
//! `max_displacement_mm`, `safety_factor`, and the discretization-error
//! conscience claims — with full solver provenance (lattice levels, tet
//! counts, PCG stats, the entire load/support set, material constants).
//! Every note states the missing physics: linear elasticity only, loads
//! and supports idealized as node sets on box regions, staircase boundary
//! at the lattice pitch, constant-strain tets that smear concentrations.
//!
//! These are `basis: "predicted"` claims riding `vcad-receipt`'s open
//! domain vocabulary: a receipt built from them rolls up **Provisional,
//! never Pass** (the thermal/particle contract). Fail-closed on top of
//! that: an *Unverifiable* convergence study yields no predicted claims
//! at all — [`predicted_claims`] refuses, and [`design_claims_unverifiable`]
//! emits a single Unverifiable receipt claim carrying the reasons, so a
//! receipt that includes the analysis can never quietly pass.

use serde::{Deserialize, Serialize};

use crate::convergence::{ConvergedAnalysis, ConvergenceVerdict};
use crate::spec::FeaSpec;

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.fea-claims/1";

/// Domain tag in the unified `vcad.receipt/1` schema.
pub const RECEIPT_DOMAIN: &str = "structure";

/// How the numbers were produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolverProvenance {
    /// Per-level `(resolution, grid, tets, nodes, pcg_iterations,
    /// pcg_residual)` rows, coarsest first, rendered human-readable.
    pub levels: Vec<String>,
    /// Material constants: `E` MPa, ν, optional yield MPa.
    pub material: String,
    /// Load set, one entry per load.
    pub loads: Vec<String>,
    /// Support set, one entry per support.
    pub supports: Vec<String>,
    /// Inter-level relative change of max displacement (the
    /// discretization-error estimate the gate judged).
    pub displacement_change_rel: f64,
    /// Inter-level relative change of max von Mises.
    pub stress_change_rel: f64,
}

/// One predicted claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Claim name, snake_case.
    pub name: String,
    /// Value.
    pub value: f64,
    /// Unit ("1" for dimensionless).
    pub unit: String,
    /// Always `"predicted"` here.
    pub basis: String,
    /// Assumptions and caveats, spelled out.
    pub note: String,
}

/// The full claim set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimSet {
    /// Schema tag ([`CLAIM_SCHEMA`]).
    pub schema: String,
    /// Solver provenance.
    pub provenance: SolverProvenance,
    /// The claims.
    pub claims: Vec<Claim>,
}

/// Why no claims were produced.
#[derive(Debug, Clone, PartialEq)]
pub enum ClaimError {
    /// The convergence study did not pass its gate; reasons attached.
    Unverifiable(Vec<String>),
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimError::Unverifiable(reasons) => write!(
                f,
                "analysis is Unverifiable, no predicted claims emitted: {}",
                reasons.join("; ")
            ),
        }
    }
}

impl std::error::Error for ClaimError {}

fn caveat() -> &'static str {
    "small-displacement linear elasticity of one isotropic material; no plasticity, \
     buckling, contact, or dynamic loads; boundary staircase-approximated at the lattice \
     pitch; constant-strain tets smear stress concentrations (peak stress is a lower bound \
     that tightens with refinement and diverges at sharp re-entrant corners)"
}

fn provenance(study: &ConvergedAnalysis, spec: &FeaSpec) -> SolverProvenance {
    let tw = &study.thin_wall;
    let mut levels: Vec<String> = vec![format!(
        "thinnest section {:.2} mm (measured along {}, {} spans sampled); {:.1} cells through \
         it at the {:.3} mm finest pitch",
        tw.thickness.p05_mm,
        tw.thickness.thin_axis,
        tw.thickness.samples,
        tw.cells_through_section,
        tw.finest_pitch_mm
    )];
    if let Some(advisory) = &tw.advisory {
        levels.push(advisory.clone());
    }
    levels.extend(study.levels.iter().map(|l| {
        format!(
            "grid {}x{}x{} (h {:.3} mm), {} tets / {} nodes, pcg {} iters residual {:.3e}",
            l.grid[0], l.grid[1], l.grid[2], l.h_mm, l.tets, l.nodes, l.iterations, l.residual_rel
        )
    }));
    SolverProvenance {
        levels,
        material: match spec.yield_strength_mpa {
            Some(y) => format!(
                "E {} MPa, nu {}, yield {} MPa",
                spec.youngs_modulus_mpa, spec.poisson, y
            ),
            None => format!("E {} MPa, nu {}", spec.youngs_modulus_mpa, spec.poisson),
        },
        loads: spec
            .loads
            .iter()
            .map(|l| {
                format!(
                    "[{:.2}, {:.2}, {:.2}] N over box {:?}..{:?}",
                    l.force[0], l.force[1], l.force[2], l.region.min, l.region.max
                )
            })
            .collect(),
        supports: spec
            .supports
            .iter()
            .map(|s| {
                format!(
                    "fix {:?} over box {:?}..{:?}",
                    s.fix, s.region.min, s.region.max
                )
            })
            .collect(),
        displacement_change_rel: study.displacement_change_rel,
        stress_change_rel: study.stress_change_rel,
    }
}

/// Build the predicted claim set for a **converged** study.
///
/// Refuses (fail-closed) when the study verdict is Unverifiable — an
/// unconverged mesh has no business making claims.
pub fn predicted_claims(study: &ConvergedAnalysis, spec: &FeaSpec) -> Result<ClaimSet, ClaimError> {
    if let ConvergenceVerdict::Unverifiable { reasons } = &study.verdict {
        return Err(ClaimError::Unverifiable(reasons.clone()));
    }
    let fine = study.finest();
    let cav = caveat();
    let mk = |name: &str, value: f64, unit: &str, note: String| Claim {
        name: name.into(),
        value,
        unit: unit.into(),
        basis: "predicted".into(),
        note,
    };
    let mut claims = vec![
        mk(
            "max_von_mises_mpa",
            fine.max_von_mises_mpa,
            "MPa",
            format!(
                "peak element von Mises at ({:.1}, {:.1}, {:.1}) mm, finest level; inter-level \
                 change {:.1}%; {cav}",
                fine.max_stress_at[0],
                fine.max_stress_at[1],
                fine.max_stress_at[2],
                100.0 * study.stress_change_rel
            ),
        ),
        mk(
            "max_displacement_mm",
            fine.max_displacement_mm,
            "mm",
            format!(
                "peak nodal displacement at ({:.1}, {:.1}, {:.1}) mm, finest level; inter-level \
                 change {:.1}%; {cav}",
                fine.max_displacement_at[0],
                fine.max_displacement_at[1],
                fine.max_displacement_at[2],
                100.0 * study.displacement_change_rel
            ),
        ),
        mk(
            "discretization_error_displacement_rel",
            study.displacement_change_rel,
            "1",
            "relative change of max displacement between the two finest lattice levels — the \
             error estimate the convergence gate judged; this number is the audit, not a \
             formality"
                .into(),
        ),
        mk(
            "discretization_error_stress_rel",
            study.stress_change_rel,
            "1",
            "relative change of max von Mises between the two finest lattice levels".into(),
        ),
    ];
    if let (Some(sf), Some(y)) = (study.safety_factor, spec.yield_strength_mpa) {
        claims.push(mk(
            "safety_factor",
            sf,
            "1",
            format!(
                "yield {y} MPa / predicted peak von Mises {:.2} MPa; the peak is a smeared \
                 lower bound, so this factor is optimistic near sharp corners; {cav}",
                fine.max_von_mises_mpa
            ),
        ));
    }
    Ok(ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: provenance(study, spec),
        claims,
    })
}

/// The oracle reference for this crate's static solver.
pub fn oracle() -> vcad_receipt::OracleRef {
    vcad_receipt::OracleRef::new("vcad-kernel-fea/solve", env!("CARGO_PKG_VERSION"))
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
/// solver ran for real, but the part has not been load-tested, so a
/// receipt built from these **rolls up Provisional, never Pass**.
pub fn design_claims(set: &ClaimSet) -> Vec<vcad_receipt::ReceiptClaim> {
    let oracle = oracle();
    let provenance = format!(
        "levels [{}]; {}; loads [{}]; supports [{}]; d(disp) {:.3e}, d(vm) {:.3e}",
        set.provenance.levels.join(" | "),
        set.provenance.material,
        set.provenance.loads.join("; "),
        set.provenance.supports.join("; "),
        set.provenance.displacement_change_rel,
        set.provenance.stress_change_rel,
    );
    set.claims
        .iter()
        .map(|c| {
            vcad_receipt::ReceiptClaim::pass(
                format!("structure.{}", c.name),
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

/// The unified-receipt claim for an **Unverifiable** study: one claim,
/// verdict Unverifiable, reasons attached — so a receipt that includes
/// the analysis can never quietly pass.
pub fn design_claims_unverifiable(reasons: &[String]) -> Vec<vcad_receipt::ReceiptClaim> {
    vec![vcad_receipt::ReceiptClaim::unverifiable(
        "structure.convergence",
        RECEIPT_DOMAIN,
        "static FEA mesh-convergence gate",
        oracle(),
        format!(
            "refinement study did not converge — no structural QoI is claimed: {}",
            reasons.join("; ")
        ),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convergence::{analyze_converged, ConvergenceOptions};
    use crate::mesh::box_mesh;
    use crate::solve::SolveOptions;
    use crate::spec::{FeaSpec, Load, RegionBox, Support};

    fn cantilever() -> FeaSpec {
        FeaSpec {
            resolution: 32,
            youngs_modulus_mpa: 69_000.0,
            poisson: 0.33,
            yield_strength_mpa: Some(276.0),
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

    fn converged_study() -> (ConvergedAnalysis, FeaSpec) {
        let surface = box_mesh([0.0; 3], [80.0, 10.0, 10.0]);
        let spec = cantilever();
        let conv = ConvergenceOptions {
            levels: 2,
            displacement_tol: 0.25,
            stress_tol: 0.50,
            ..Default::default()
        };
        let study = analyze_converged(&surface, &spec, &conv, &SolveOptions::default()).unwrap();
        assert!(study.converged(), "{:?}", study.verdict);
        (study, spec)
    }

    #[test]
    fn claims_carry_provenance_and_caveats() {
        let (study, spec) = converged_study();
        let set = predicted_claims(&study, &spec).unwrap();
        assert_eq!(set.schema, CLAIM_SCHEMA);
        let names: Vec<&str> = set.claims.iter().map(|c| c.name.as_str()).collect();
        for want in [
            "max_von_mises_mpa",
            "max_displacement_mm",
            "discretization_error_displacement_rel",
            "discretization_error_stress_rel",
            "safety_factor",
        ] {
            assert!(names.contains(&want), "missing claim {want}");
        }
        for c in &set.claims {
            assert_eq!(c.basis, "predicted");
            assert!(c.value.is_finite(), "non-finite claim {}", c.name);
        }
        // Physical claims state the missing physics.
        for c in set.claims.iter().filter(|c| c.unit != "1") {
            assert!(c.note.contains("linear elasticity"), "note: {}", c.note);
            assert!(c.note.contains("staircase"), "note: {}", c.note);
        }
        // One provenance row per lattice level, preceded by the measured
        // thin-wall row (the cell arithmetic, on the record).
        assert_eq!(set.provenance.levels.len(), 3);
        assert!(
            set.provenance.levels[0].contains("cells through"),
            "provenance must state cells-through-section: {:?}",
            set.provenance.levels[0]
        );
        assert!(set.provenance.material.contains("yield 276"));
        assert_eq!(set.provenance.loads.len(), 1);
        // Round trip.
        let json = serde_json::to_string(&set).unwrap();
        assert!(json.contains("vcad.fea-claims/1"));
        let back: ClaimSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back.claims.len(), set.claims.len());
    }

    #[test]
    fn design_claims_ride_the_unified_receipt_as_provisional() {
        let (study, spec) = converged_study();
        let set = predicted_claims(&study, &spec).unwrap();
        let claims = design_claims(&set);
        assert_eq!(claims.len(), set.claims.len());
        for c in &claims {
            assert!(c.id.starts_with("structure."));
            assert_eq!(c.domain, RECEIPT_DOMAIN);
            assert_eq!(c.basis, Some(vcad_receipt::ClaimBasis::Predicted));
            assert!(c.measured.is_some());
        }
        let receipt = vcad_receipt::DesignReceipt::with_claims(claims);
        assert_eq!(
            receipt.verdict(),
            vcad_receipt::ReceiptVerdict::Provisional,
            "predicted structural claims must never read as verified"
        );
    }

    #[test]
    fn unverifiable_study_claims_nothing_and_poisons_the_receipt() {
        let surface = box_mesh([0.0; 3], [80.0, 10.0, 10.0]);
        let mut spec = cantilever();
        spec.resolution = 8;
        let conv = ConvergenceOptions {
            levels: 2,
            displacement_tol: 1e-6,
            stress_tol: 1e-6,
            ..Default::default()
        };
        let study = analyze_converged(&surface, &spec, &conv, &SolveOptions::default()).unwrap();
        let reasons = match &study.verdict {
            ConvergenceVerdict::Unverifiable { reasons } => reasons.clone(),
            v => panic!("expected Unverifiable, got {v:?}"),
        };
        assert!(matches!(
            predicted_claims(&study, &spec),
            Err(ClaimError::Unverifiable(_))
        ));
        let claims = design_claims_unverifiable(&reasons);
        assert_eq!(claims.len(), 1);
        let receipt = vcad_receipt::DesignReceipt::with_claims(claims);
        assert_ne!(
            receipt.verdict(),
            vcad_receipt::ReceiptVerdict::Pass,
            "an unverifiable analysis must never let a receipt pass"
        );
    }
}
