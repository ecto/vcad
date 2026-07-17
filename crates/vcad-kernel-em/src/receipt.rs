//! Predicted-performance claims for the design receipt:
//! `vcad.em-claims/1`.
//!
//! Emits serializable claim sets — inductance, capacitance, force,
//! torque, stored energy — with full solver provenance in the spirit of
//! `vcad.receipt/1`: every number carries how it was produced, nothing
//! is defaulted silently, and this crate's signature discipline — **every
//! quantity extracted two independent ways** — rides along as the
//! `cross_route_residual` (energy vs linkage, charge vs energy, Maxwell
//! stress vs `J×B`).
//!
//! These are `basis: "predicted"` claims. [`compare`] binds bench
//! measurements (an LCR meter, a torque stand, a back-EMF spin-down)
//! with fail-closed verdicts: an unmeasured receipt never passes, a
//! measurement matching no claim is an error, and `Violated` is a
//! publishable result about the model, not a bookkeeping failure —
//! the same contract as `vcad_kernel_particle::receipt`.
//!
//! Registering this family in `crates/vcad-receipt` + the MCP surface is
//! the flagged cross-crate follow-up (ir:gen exports multiple crates;
//! names must stay unique), deliberately not done from this crate.

use serde::{Deserialize, Serialize};

use crate::axisym::{AxisymMagSolution, PicardReport};
use crate::electro::ElectroSolution;
use crate::planar::PlanarMagSolution;

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.em-claims/1";

/// How the numbers were produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolverProvenance {
    /// Formulation that produced the field.
    pub formulation: String,
    /// Node counts `[nx, ny]`.
    pub grid: [usize; 2],
    /// SOR relative tolerance of the final solve.
    pub sor_tol: f64,
    /// SOR sweeps used by the final solve.
    pub sor_sweeps: usize,
    /// The two-independent-routes gap for the claimed quantity
    /// (energy vs linkage, charge vs energy, stress vs J×B) — this
    /// crate's built-in cross-check, carried on the receipt.
    pub cross_route_residual: Option<f64>,
    /// Picard iterations when the solve carried a B–H law (`None` =
    /// linear solve).
    pub nonlinear_iterations: Option<usize>,
}

/// One predicted claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Claim name (snake_case).
    pub name: String,
    /// Value.
    pub value: f64,
    /// Unit ("1" for dimensionless).
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
    /// Solver provenance.
    pub provenance: SolverProvenance,
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

/// Inductance claims for coil `k` of an axisymmetric solve (the coil
/// must be the sole driven one for the self-inductance reading).
///
/// `picard` is the nonlinear convergence report when the solve carried a
/// B–H law — the inductance is then the **secant** value at this drive.
pub fn axisym_inductance_claims(
    solution: &AxisymMagSolution,
    k: usize,
    sor_tol: f64,
    picard: Option<&PicardReport>,
) -> ClaimSet {
    let bal = solution.energy();
    let l = solution.self_inductance(k);
    let secant_note = if picard.is_some() {
        "SECANT inductance at this drive level (B–H law solved); "
    } else {
        ""
    };
    ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: SolverProvenance {
            formulation: "axisym-magnetostatics".to_string(),
            grid: [solution.system.grid.nx, solution.system.grid.ny],
            sor_tol,
            sor_sweeps: solution.sweeps,
            cross_route_residual: Some(bal.residual),
            nonlinear_iterations: picard.map(|p| p.iterations),
        },
        claims: vec![
            claim(
                "inductance_h",
                l,
                "H",
                &format!(
                    "{secant_note}flux-linkage route (identical to 2W/I² by \
                     construction); finite ψ=0 truncation reads low unless the \
                     boundary is far; bindable to an LCR meter at a frequency \
                     where eddy effects are negligible"
                ),
            ),
            claim(
                "stored_energy_j",
                bal.source,
                "J",
                "source-form ½·Σ I·Λ; for nonlinear solves this is the secant \
                 quadratic form ½∫H·B, not ∫H dB",
            ),
        ],
    }
}

/// Axial-force claim for coil `k`, `J×B` route, with the Maxwell-stress
/// surface as the cross-route residual when a probe cylinder is given
/// (`(r_mm, z_lo_mm, z_hi_mm, panels)` — must enclose only coil `k`,
/// in vacuum).
pub fn axisym_force_claims(
    solution: &AxisymMagSolution,
    k: usize,
    sor_tol: f64,
    stress_probe: Option<(f64, f64, f64, usize)>,
) -> ClaimSet {
    let f_jxb = solution.axial_force_on_coil(k);
    let cross = stress_probe.map(|(r, z0, z1, n)| {
        let f_stress = solution.axial_force_stress(r, z0, z1, n);
        (f_stress - f_jxb).abs() / f_jxb.abs().max(1e-30)
    });
    ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: SolverProvenance {
            formulation: "axisym-magnetostatics".to_string(),
            grid: [solution.system.grid.nx, solution.system.grid.ny],
            sor_tol,
            sor_sweeps: solution.sweeps,
            cross_route_residual: cross,
            nonlinear_iterations: None,
        },
        claims: vec![claim(
            "force_n",
            f_jxb,
            "N",
            "axial J×B on the coil's own current; valid in non-magnetic \
             surroundings; cross_route_residual compares an independent \
             Maxwell-stress surface when provided",
        )],
    }
}

/// Rotor-torque claim of a planar machine slice: `J×B` on all magnets
/// about `(cx_mm, cy_mm)`, times the stack `depth_m`. The unrolled-slice
/// caveats ride on the claim. When the domain is periodic, the
/// full-period stress line at `stress_line_y_mm` provides the
/// cross-route residual.
pub fn planar_torque_claims(
    solution: &PlanarMagSolution,
    cx_mm: f64,
    cy_mm: f64,
    r_mean_m: f64,
    depth_m: f64,
    sor_tol: f64,
    stress_line_y_mm: Option<f64>,
) -> ClaimSet {
    let n_mag = solution.magnet_sources.len();
    let t_per_m: f64 = if solution.system.grid.periodic_x {
        // Unrolled machine: torque = tangential force × mean radius.
        (0..n_mag)
            .map(|m| solution.force_on_magnet(m).0)
            .sum::<f64>()
            * r_mean_m
    } else {
        (0..n_mag)
            .map(|m| solution.torque_on_magnet(m, cx_mm, cy_mm))
            .sum()
    };
    let value = t_per_m * depth_m;
    let cross = stress_line_y_mm.map(|y| {
        let (fx, _) = solution.force_through_line(y, 4 * solution.system.grid.nx);
        let t_stress = -fx * r_mean_m;
        (t_stress - t_per_m).abs() / t_per_m.abs().max(1e-30)
    });
    ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: SolverProvenance {
            formulation: "planar-magnetostatics".to_string(),
            grid: [solution.system.grid.nx, solution.system.grid.ny],
            sor_tol,
            sor_sweeps: solution.sweeps,
            cross_route_residual: cross,
            nonlinear_iterations: None,
        },
        claims: vec![claim(
            "torque_nm",
            value,
            "N·m",
            "J×B on magnet bound currents × mean radius × stack depth; 2D \
             slice — curvature and radial end fringing NOT modeled; statics; \
             bindable to a torque stand or back-EMF Kt measurement",
        )],
    }
}

/// Capacitance claim of a two-terminal electrostatic solve (electrode
/// `hot` at nonzero potential, everything else grounded).
pub fn capacitance_claims(solution: &ElectroSolution, hot: usize, sor_tol: f64) -> ClaimSet {
    let cap = solution.capacitance_two_terminal(hot);
    ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: SolverProvenance {
            formulation: "electrostatics".to_string(),
            grid: [solution.system.grid.nx, solution.system.grid.ny],
            sor_tol,
            sor_sweeps: solution.sweeps,
            cross_route_residual: Some(cap.mismatch()),
            nonlinear_iterations: None,
        },
        claims: vec![claim(
            "capacitance_f",
            cap.from_charge,
            "F",
            "induced-charge route (cross_route_residual compares the energy \
             route); curved conductors staircase at grid resolution — bracket \
             with a refinement study; bindable to an LCR meter",
        )],
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
    /// Instrument provenance ("LCR meter s/n …", "spin-down rig …").
    pub instrument: String,
    /// Acceptance band as a multiplicative factor: the claim holds when
    /// measured ∈ [predicted/band − u, predicted·band + u].
    pub band_factor: f64,
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
    /// measured / predicted (`None` when unmeasured or predicted = 0).
    pub ratio: Option<f64>,
    /// Verdict.
    pub verdict: Verdict,
}

/// The comparison report: every claim gets a row; a measurement matching
/// no claim is an error (a measurement of nothing is a bookkeeping bug).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparisonReport {
    /// Schema tag.
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
        schema: "vcad.em-compare/1".to_string(),
        entries,
        all_hold: all_hold && measured_any,
    })
}

/// Domain tag for EM claims in the unified [`vcad_receipt`] schema.
pub const RECEIPT_DOMAIN: &str = "em";

/// The oracle reference for this crate's finite-volume field solver.
pub fn oracle() -> vcad_receipt::OracleRef {
    vcad_receipt::OracleRef::new("vcad-kernel-em/fv", env!("CARGO_PKG_VERSION"))
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
/// field solve ran for real, but the claims describe hardware that has
/// not been measured, so a receipt built from these **rolls up
/// Provisional, never Pass** (the same contract as
/// `predict_physics`/`predict_print`). The computed value rides in
/// `measured` ("what the oracle computed"); formulation, grid, and the
/// two-independent-routes residual ride in `details`.
pub fn design_claims(set: &ClaimSet) -> Vec<vcad_receipt::ReceiptClaim> {
    let oracle = oracle();
    let mut provenance = format!(
        "{}, grid {}x{}, sor tol {:.1e} sweeps {}",
        set.provenance.formulation,
        set.provenance.grid[0],
        set.provenance.grid[1],
        set.provenance.sor_tol,
        set.provenance.sor_sweeps,
    );
    if let Some(r) = set.provenance.cross_route_residual {
        provenance.push_str(&format!("; cross_route_residual {r:.3e}"));
    }
    if let Some(n) = set.provenance.nonlinear_iterations {
        provenance.push_str(&format!("; picard_iterations {n}"));
    }
    set.claims
        .iter()
        .map(|c| {
            vcad_receipt::ReceiptClaim::pass(
                format!("em.{}", c.name),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axisym::{Annulus, AxisymMagnetostatics, Coil};
    use crate::grid::{Bc, SolveOptions};

    fn solenoid_claims() -> ClaimSet {
        let mut dev = AxisymMagnetostatics::new(40.0, 0.0, 100.0);
        dev.bc_r_outer = Bc::Neumann;
        dev.bc_z_low = Bc::Neumann;
        dev.bc_z_high = Bc::Neumann;
        dev.coils.push(Coil {
            region: Annulus {
                r_inner_mm: 20.0,
                r_outer_mm: 22.0,
                z_min_mm: 0.0,
                z_max_mm: 100.0,
            },
            turns: 1000.0,
            current_a: 1.0,
        });
        let opts = SolveOptions::default();
        let sol = dev.solve(41, 7, &opts).unwrap();
        axisym_inductance_claims(&sol, 0, opts.tol, None)
    }

    #[test]
    fn design_claims_ride_the_unified_receipt_as_provisional() {
        let set = solenoid_claims();
        let claims = design_claims(&set);
        assert_eq!(claims.len(), set.claims.len());
        for c in &claims {
            assert!(c.id.starts_with("em."));
            assert_eq!(c.domain, RECEIPT_DOMAIN);
            assert_eq!(c.basis, Some(vcad_receipt::ClaimBasis::Predicted));
            assert!(c.measured.is_some());
            let details = c.details.as_deref().unwrap_or("");
            assert!(details.contains("axisym-magnetostatics"));
            assert!(details.contains("cross_route_residual"));
        }
        let receipt = vcad_receipt::DesignReceipt::with_claims(claims);
        assert_eq!(
            receipt.verdict(),
            vcad_receipt::ReceiptVerdict::Provisional,
            "predicted em claims must never read as verified"
        );
    }

    #[test]
    fn serializes_with_schema_and_provenance() {
        let set = solenoid_claims();
        let json = serde_json::to_string_pretty(&set).unwrap();
        assert!(json.contains("vcad.em-claims/1"));
        assert!(json.contains("inductance_h"));
        assert!(json.contains("cross_route_residual"));
        let back: ClaimSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema, set.schema);
        assert_eq!(back.provenance.grid, [41, 7]);
        // The two-route residual is real and small.
        let r = back.provenance.cross_route_residual.unwrap();
        assert!(r < 1e-6, "cross-route residual {r:.2e}");
        // And the inductance is physical for this solenoid:
        // μ₀·n²·πR²·ℓ ≈ 17 mH.
        let l = back
            .claims
            .iter()
            .find(|c| c.name == "inductance_h")
            .unwrap();
        assert!(l.value > 1e-2 && l.value < 3e-2, "L = {}", l.value);
    }

    #[test]
    fn compare_binds_measurements_fail_closed() {
        let set = solenoid_claims();
        // Unmeasured receipt never passes.
        let empty = compare(&set, &[]).unwrap();
        assert!(!empty.all_hold);
        assert!(empty
            .entries
            .iter()
            .all(|e| e.verdict == Verdict::Unmeasured));

        // A measurement of nothing is an error.
        assert!(compare(
            &set,
            &[Measurement {
                name: "warp_factor".into(),
                value: 9.0,
                uncertainty: 0.1,
                instrument: "vibes".into(),
                band_factor: 2.0,
            }]
        )
        .is_err());

        // LCR meter within band → Holds; a wildly wrong energy → Violated.
        let l_pred = set
            .claims
            .iter()
            .find(|c| c.name == "inductance_h")
            .unwrap()
            .value;
        let ok = Measurement {
            name: "inductance_h".into(),
            value: l_pred * 1.05,
            uncertainty: 1e-6,
            instrument: "LCR meter".into(),
            band_factor: 1.2,
        };
        let bad = Measurement {
            name: "stored_energy_j".into(),
            value: 1e6,
            uncertainty: 1.0,
            instrument: "calorimeter of lies".into(),
            band_factor: 2.0,
        };
        let report = compare(&set, &[ok, bad]).unwrap();
        assert!(!report.all_hold);
        let verdict = |n: &str| report.entries.iter().find(|e| e.name == n).unwrap().verdict;
        assert_eq!(verdict("inductance_h"), Verdict::Holds);
        assert_eq!(verdict("stored_energy_j"), Verdict::Violated);

        // All-measured, all-holding passes.
        let ok2 = Measurement {
            name: "stored_energy_j".into(),
            value: set
                .claims
                .iter()
                .find(|c| c.name == "stored_energy_j")
                .unwrap()
                .value,
            uncertainty: 0.0,
            instrument: "the same solver, honestly".into(),
            band_factor: 1.01,
        };
        let ok1 = Measurement {
            name: "inductance_h".into(),
            value: l_pred,
            uncertainty: 0.0,
            instrument: "LCR meter".into(),
            band_factor: 1.01,
        };
        let pass = compare(&set, &[ok1, ok2]).unwrap();
        assert!(pass.all_hold);
    }

    #[test]
    fn torque_claim_carries_the_stress_cross_route() {
        use crate::planar::{Conductor, MagnetBlock, PlanarMagnetostatics, Rect};
        // A small periodic machine with a real cross-route check.
        let mut dev = PlanarMagnetostatics::new(0.0, 60.0, 0.0, 24.0);
        dev.periodic_x = true;
        for (k, sign) in [(0usize, 1.0), (1, -1.0)] {
            dev.magnets.push(MagnetBlock {
                region: Rect {
                    x_min_mm: 6.0 + 30.0 * k as f64,
                    x_max_mm: 24.0 + 30.0 * k as f64,
                    y_min_mm: 14.0,
                    y_max_mm: 18.0,
                },
                br_x_t: 0.0,
                br_y_t: sign * 0.8,
                mu_r: 1.05,
            });
        }
        for (k, i) in [(0usize, 4.0), (1, -1.0), (2, -3.0), (3, 2.0)] {
            dev.conductors.push(Conductor {
                region: Rect {
                    x_min_mm: 3.0 + 15.0 * k as f64,
                    x_max_mm: 9.0 + 15.0 * k as f64,
                    y_min_mm: 4.0,
                    y_max_mm: 7.0,
                },
                total_current_a: i,
            });
        }
        let opts = SolveOptions::default();
        let sol = dev.solve(120, 49, &opts).unwrap();
        let set = planar_torque_claims(&sol, 0.0, 0.0, 0.0225, 0.015, opts.tol, Some(10.5));
        let t = &set.claims[0];
        assert_eq!(t.unit, "N·m");
        assert!(t.value.abs() > 0.0);
        let cross = set.provenance.cross_route_residual.unwrap();
        assert!(cross < 0.05, "stress vs J×B gap {cross:.3e}");
    }
}
