//! Predicted-performance claims for the design receipt.
//!
//! Emits a serializable claim set — `t_max_c`, per-source
//! `theta_ja_c_per_w`, and the `energy_balance_residual` conscience — with
//! full solver provenance (grid, CG tolerance and iterations, the entire
//! boundary-condition set, anisotropy state, whether the geometry came
//! through the voxelized-part seam), in the spirit of `vcad.receipt/1`:
//! every number carries how it was produced, and nothing is defaulted
//! silently.
//!
//! **Every claim note states the missing physics.** These are conduction
//! predictions priced at a *supplied* film coefficient; the note on every
//! temperature claim says so, names the h values used, and flags that
//! radiation is not modeled. A claim that hides its h is a guess wearing
//! a lab coat.
//!
//! These are `basis: "predicted"` claims. Binding them to thermal-camera
//! and thermocouple measurements (Holds / Violated / Unmeasured) is the
//! M6 measurement pack in this module ([`compare`]). Registering the
//! family in `crates/vcad-receipt` + the MCP surface is the flagged
//! cross-crate follow-up PR (ir:gen exports two crates; names must stay
//! unique), deliberately not done from this branch.

use serde::{Deserialize, Serialize};

use crate::model::{Boundary, ThermalModel};
use crate::solve::{Solution, SolveOptions};

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.thermal-claims/1";

/// How the numbers were produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolverProvenance {
    /// Voxel counts per axis.
    pub grid: [usize; 3],
    /// Voxel edge lengths, mm.
    pub voxel_mm: [f64; 3],
    /// CG relative tolerance requested.
    pub cg_tol: f64,
    /// CG iterations used by the converged solve.
    pub cg_iterations: usize,
    /// Final CG relative residual.
    pub cg_residual_rel: f64,
    /// Human-readable boundary-condition set, one entry per active slot
    /// (`-x … +z`, `exposed`) — the h values every prediction is priced
    /// at live here.
    pub bc_set: Vec<String>,
    /// `"isotropic"` or `"diagonal"` — whether any material splits its
    /// per-axis conductivity.
    pub anisotropy: String,
    /// True when the geometry came through the voxelized-part seam
    /// (`VoxelMaterials`) rather than region painting.
    pub voxelized_materials: bool,
}

/// One predicted claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Claim name (snake_case; per-source θ claims are
    /// `theta_ja_c_per_w:<source>` when multiple sources exist).
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

fn claim(name: String, value: f64, unit: &str, note: String) -> Claim {
    Claim {
        name,
        value,
        unit: unit.to_string(),
        basis: "predicted".to_string(),
        note,
    }
}

fn describe_bc(label: &str, bc: &Boundary) -> Option<String> {
    match bc {
        Boundary::Adiabatic => None,
        Boundary::FixedTemperature { temperature_c } => {
            Some(format!("{label}: fixed {temperature_c} C"))
        }
        Boundary::Convection { h_w_m2k, ambient_c } => Some(format!(
            "{label}: convection h={h_w_m2k} W/m2K, ambient {ambient_c} C"
        )),
    }
}

/// The standing caveat every temperature claim carries.
fn conduction_caveat(model: &ThermalModel) -> String {
    let hs: Vec<String> = model
        .domain_faces
        .iter()
        .chain([&model.exposed])
        .filter_map(|bc| match bc {
            Boundary::Convection { h_w_m2k, .. } => Some(format!("{h_w_m2k}")),
            _ => None,
        })
        .collect();
    let h_part = if hs.is_empty() {
        "no convection surfaces".to_string()
    } else {
        format!(
            "priced at supplied h = {{{}}} W/m2K (not derived)",
            hs.join(", ")
        )
    };
    format!(
        "conduction only; {h_part}; radiation not modeled (~6 W/m2K equivalent at \
         electronics temperatures)"
    )
}

/// Build the predicted claim set for one solved configuration.
///
/// `opts` must be the options the solution was produced with — they are
/// provenance, and lying to a receipt defeats its purpose.
pub fn predicted_claims(
    model: &ThermalModel,
    solution: &Solution,
    opts: &SolveOptions,
) -> ClaimSet {
    let caveat = conduction_caveat(model);

    let mut claims = vec![claim(
        "t_max_c".into(),
        solution.t_max_c,
        "C",
        format!(
            "hottest solid voxel, at ({:.1}, {:.1}, {:.1}) mm; {caveat}",
            solution.t_max_at_mm[0], solution.t_max_at_mm[1], solution.t_max_at_mm[2]
        ),
    )];

    let single = solution.sources.len() == 1;
    for s in &solution.sources {
        match (s.theta_c_per_w, solution.reference_c) {
            (Some(theta), Some(reference)) => {
                let name = if single {
                    "theta_ja_c_per_w".to_string()
                } else {
                    format!("theta_ja_c_per_w:{}", s.name)
                };
                claims.push(claim(
                    name,
                    theta,
                    "K/W",
                    format!(
                        "(T_src,max - {reference} C) / {} W for source {:?}; {caveat}",
                        s.power_w, s.name
                    ),
                ));
            }
            _ => {
                // A zero-power source has no θ (undefined at P = 0);
                // stated here rather than emitted as NaN.
                claims.push(claim(
                    if single {
                        "theta_ja_undefined".to_string()
                    } else {
                        format!("theta_ja_undefined:{}", s.name)
                    },
                    0.0,
                    "1",
                    format!(
                        "source {:?} has zero power; theta = dT/P is undefined at P = 0 \
                         and is not claimed",
                        s.name
                    ),
                ));
            }
        }
    }

    claims.push(claim(
        "energy_balance_residual".into(),
        solution.energy.residual_rel,
        "1",
        "|P_source - P_boundary_out| / max(|P_source|, gross flow); closes to solver \
         tolerance or the solution is wrong — this number is the audit, not a formality"
            .into(),
    ));

    let mut bc_set = Vec::new();
    let labels = ["-x", "+x", "-y", "+y", "-z", "+z"];
    for (label, bc) in labels.iter().zip(&model.domain_faces) {
        if let Some(d) = describe_bc(label, bc) {
            bc_set.push(d);
        }
    }
    if let Some(d) = describe_bc("exposed", &model.exposed) {
        bc_set.push(d);
    }
    if bc_set.is_empty() {
        bc_set.push("all faces adiabatic".to_string());
    }

    let anisotropy = if model
        .materials
        .iter()
        .all(|m| m.k_w_mk[0] == m.k_w_mk[1] && m.k_w_mk[1] == m.k_w_mk[2])
    {
        "isotropic"
    } else {
        "diagonal"
    };

    ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: SolverProvenance {
            grid: solution.divisions,
            voxel_mm: solution.voxel_mm,
            cg_tol: opts.tol,
            cg_iterations: solution.iterations,
            cg_residual_rel: solution.residual_rel,
            bc_set,
            anisotropy: anisotropy.to_string(),
            voxelized_materials: model.voxel_materials.is_some(),
        },
        claims,
    }
}

/// A bench measurement to bind against a predicted claim (M6).
///
/// The two instruments this pack expects, and their traps:
///
/// - **Thermal camera**: reads radiance, not temperature. A board at
///   ε ≈ 0.9 reads honestly; bare copper or solder at ε ≈ 0.05 reads the
///   *reflection of the room* and shows a hot plane as cold. Paint or
///   tape (known-ε target) the spots you will compare, and record the ε
///   the camera was set to in `instrument`.
/// - **Thermocouple**: reads its own junction, which a contact
///   resistance and a lead that conducts heat away both pull toward
///   ambient — a bare TC pressed on a small hot spot reads *low* by
///   several K. Glue or solder the junction and derate the band
///   accordingly.
///
/// The film coefficient is the third instrument nobody calibrates: a
/// prediction priced at h = 10 compared against a bench with a draft is
/// not a model error. Bands should be set with all three in mind — θ
/// through natural convection honestly carries ±20–30%.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    /// Claim name this measures (must match a claim).
    pub name: String,
    /// Measured value, in the claim's unit.
    pub value: f64,
    /// One-sigma absolute uncertainty, same unit.
    pub uncertainty: f64,
    /// Instrument provenance ("FLIR E8, ε=0.95 tape target", "type-K TC
    /// epoxied, meter s/n …").
    pub instrument: String,
    /// Acceptance band as a multiplicative factor: the claim holds when
    /// the measurement lies in [predicted/band − u, predicted·band + u].
    /// h-dominated claims (θ, T_max under natural convection) warrant a
    /// generous band (1.3+); the energy residual a tight one.
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

/// Bind bench measurements to a claim set, fail-closed.
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
        schema: "vcad.thermal-compare/1".to_string(),
        entries,
        all_hold: all_hold && measured_any,
    })
}

/// Build the predicted claim set for one transient run.
///
/// Same fail-closed contract as [`predicted_claims`]: `opts` must be what
/// the run used, the model is the **base** (pre-schedule) model, and every
/// claim carries `basis: "predicted"`. Series data stays on the
/// [`TransientSolution`]; the claims capture the quantities a recipe is
/// judged by — the peak, the endpoint, how far the endpoint sits from
/// steady, and the integrated energy audit.
pub fn transient_claims(
    model: &ThermalModel,
    tsol: &crate::transient::TransientSolution,
    opts: &SolveOptions,
) -> ClaimSet {
    let caveat = conduction_caveat(model);
    let steady = predicted_claims(model, &tsol.final_state, opts);
    let total_time_s = tsol.times_s.last().copied().unwrap_or(0.0);
    let steps = tsol.times_s.len();
    let run = format!("{steps} backward-Euler steps to t = {total_time_s} s");

    let (peak_i, peak) =
        tsol.t_max_c
            .iter()
            .enumerate()
            .fold(
                (0, f64::NEG_INFINITY),
                |acc, (i, &t)| {
                    if t > acc.1 {
                        (i, t)
                    } else {
                        acc
                    }
                },
            );
    let final_t = tsol.t_max_c.last().copied().unwrap_or(f64::NAN);

    let mut claims = vec![
        claim(
            "t_max_peak_c".into(),
            peak,
            "C",
            format!(
                "hottest solid voxel over the run, at t = {} s; {run}; {caveat}",
                tsol.times_s[peak_i]
            ),
        ),
        claim(
            "t_max_final_c".into(),
            final_t,
            "C",
            format!("hottest solid voxel at the end of the run; {run}; {caveat}"),
        ),
    ];
    let single = tsol.final_state.sources.len() == 1;
    for (s, series) in tsol.final_state.sources.iter().zip(&tsol.source_t_max_c) {
        let name = if single {
            "t_source_final_c".to_string()
        } else {
            format!("t_source_final_c:{}", s.name)
        };
        claims.push(claim(
            name,
            series.last().copied().unwrap_or(f64::NAN),
            "C",
            format!(
                "hottest voxel of source {:?} at the end of the run; {caveat}",
                s.name
            ),
        ));
    }
    claims.push(claim(
        "transient_energy_audit_residual".into(),
        tsol.energy_audit_residual_rel,
        "1",
        "|stored-energy change - integrated net injection| over the run, normalized by \
         gross energy traffic; a discrete identity of backward Euler, closes to CG \
         tolerance or the trajectory is wrong"
            .into(),
    ));
    claims.push(claim(
        "distance_from_steady_residual".into(),
        tsol.final_state.energy.residual_rel,
        "1",
        "steady energy-balance residual of the final field: ~solver tolerance means the \
         run has relaxed to steady state; O(1) means the endpoint is still moving"
            .into(),
    ));

    ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: SolverProvenance {
            cg_iterations: tsol.cg_iterations_total,
            ..steady.provenance
        },
        claims,
    }
}

/// Domain tag for thermal claims in the unified [`vcad_receipt`] schema.
pub const RECEIPT_DOMAIN: &str = "thermal";

/// The oracle reference for this crate's steady-conduction solver.
pub fn oracle() -> vcad_receipt::OracleRef {
    vcad_receipt::OracleRef::new("vcad-kernel-thermal/solve", env!("CARGO_PKG_VERSION"))
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
/// solver ran for real, but the claims describe hardware that has not
/// been measured, so a receipt built from these **rolls up Provisional,
/// never Pass** (the same contract as `predict_physics`/`predict_print`).
/// The computed value rides in `measured` ("what the oracle computed");
/// solver provenance rides in `details`.
pub fn design_claims(set: &ClaimSet) -> Vec<vcad_receipt::ReceiptClaim> {
    let oracle = oracle();
    let provenance = format!(
        "grid {}x{}x{}, voxel {:.3}x{:.3}x{:.3} mm, cg tol {:.1e} iters {} residual {:.3e}, {}, bc [{}]",
        set.provenance.grid[0],
        set.provenance.grid[1],
        set.provenance.grid[2],
        set.provenance.voxel_mm[0],
        set.provenance.voxel_mm[1],
        set.provenance.voxel_mm[2],
        set.provenance.cg_tol,
        set.provenance.cg_iterations,
        set.provenance.cg_residual_rel,
        set.provenance.anisotropy,
        set.provenance.bc_set.join("; "),
    );
    set.claims
        .iter()
        .map(|c| {
            vcad_receipt::ReceiptClaim::pass(
                format!("thermal.{}", c.name),
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
    use crate::model::{Boundary, MaterialRegion, PowerSource, Shape, ThermalModel};
    use crate::solve::solve_steady;

    #[test]
    fn transient_claims_capture_peak_endpoint_and_audits() {
        let mut m = chip_model();
        m.materials[0].heat_capacity_j_m3k = Some(1.8e6);
        let opts = crate::solve::SolveOptions::default();
        let tsol = crate::transient::solve_transient(
            &m,
            &opts,
            &crate::transient::TransientOptions {
                dt_s: 5.0,
                steps: 20,
                initial_c: 25.0,
                snapshot_every: 0,
            },
        )
        .unwrap();
        let set = transient_claims(&m, &tsol, &opts);
        assert_eq!(set.schema, CLAIM_SCHEMA);
        let names: Vec<&str> = set.claims.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"t_max_peak_c"));
        assert!(names.contains(&"t_max_final_c"));
        assert!(names.contains(&"t_source_final_c"));
        assert!(names.contains(&"transient_energy_audit_residual"));
        assert!(names.contains(&"distance_from_steady_residual"));
        assert!(set.claims.iter().all(|c| c.basis == "predicted"));
        assert_eq!(set.provenance.cg_iterations, tsol.cg_iterations_total);
        // Heating run: the peak is the endpoint.
        let peak = set
            .claims
            .iter()
            .find(|c| c.name == "t_max_peak_c")
            .unwrap();
        let fin = set
            .claims
            .iter()
            .find(|c| c.name == "t_max_final_c")
            .unwrap();
        assert!((peak.value - fin.value).abs() < 1e-12);
        // And it rides the unified receipt as Provisional like the rest.
        let rc = design_claims(&set);
        assert_eq!(rc.len(), set.claims.len());
        assert!(rc
            .iter()
            .all(|c| c.basis == Some(vcad_receipt::ClaimBasis::Predicted)));
    }

    fn chip_model() -> ThermalModel {
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [60.0, 60.0, 2.0], [30, 30, 2]);
        m.materials.push(MaterialRegion::anisotropic(
            Shape::Box {
                min_mm: [0.0, 0.0, 0.0],
                size_mm: [60.0, 60.0, 2.0],
            },
            [15.0, 15.0, 0.5],
        ));
        m.sources.push(PowerSource {
            name: "die".into(),
            shape: Shape::Box {
                min_mm: [25.0, 25.0, 0.0],
                size_mm: [10.0, 10.0, 2.0],
            },
            power_w: 2.0,
        });
        let conv = Boundary::Convection {
            h_w_m2k: 15.0,
            ambient_c: 25.0,
        };
        m.domain_faces[4] = conv;
        m.domain_faces[5] = conv;
        m
    }

    fn get(set: &ClaimSet, name: &str) -> f64 {
        set.claims
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("missing claim {name}"))
            .value
    }

    #[test]
    fn design_claims_ride_the_unified_receipt_as_provisional() {
        let m = chip_model();
        let opts = SolveOptions::default();
        let sol = solve_steady(&m, &opts).unwrap();
        let set = predicted_claims(&m, &sol, &opts);
        let claims = design_claims(&set);
        assert_eq!(claims.len(), set.claims.len());
        for c in &claims {
            assert!(c.id.starts_with("thermal."));
            assert_eq!(c.domain, RECEIPT_DOMAIN);
            assert_eq!(c.basis, Some(vcad_receipt::ClaimBasis::Predicted));
            assert!(c.measured.is_some());
            assert!(c.details.as_deref().unwrap_or("").contains("grid 30x30x2"));
        }
        let receipt = vcad_receipt::DesignReceipt::with_claims(claims);
        assert_eq!(
            receipt.verdict(),
            vcad_receipt::ReceiptVerdict::Provisional,
            "predicted thermal claims must never read as verified"
        );
    }

    #[test]
    fn claims_carry_provenance_and_caveats() {
        let m = chip_model();
        let opts = SolveOptions::default();
        let sol = solve_steady(&m, &opts).unwrap();
        let set = predicted_claims(&m, &sol, &opts);

        assert_eq!(set.schema, CLAIM_SCHEMA);
        assert!((get(&set, "t_max_c") - sol.t_max_c).abs() < 1e-12);
        assert!(get(&set, "theta_ja_c_per_w") > 0.0);
        assert!(get(&set, "energy_balance_residual") < 1e-6);

        // Every temperature claim states the missing physics and its h.
        for c in set
            .claims
            .iter()
            .filter(|c| c.unit == "C" || c.unit == "K/W")
        {
            assert!(
                c.note.contains("conduction only"),
                "note lacks scope: {}",
                c.note
            );
            assert!(c.note.contains("h = {15"), "note lacks its h: {}", c.note);
            assert!(
                c.note.contains("radiation"),
                "note lacks radiation: {}",
                c.note
            );
            assert_eq!(c.basis, "predicted");
        }

        // Provenance is truthful.
        assert_eq!(set.provenance.grid, [30, 30, 2]);
        assert_eq!(set.provenance.anisotropy, "diagonal");
        assert!(!set.provenance.voxelized_materials);
        assert_eq!(set.provenance.cg_tol, opts.tol);
        assert!(set.provenance.cg_iterations > 0);
        assert!(set
            .provenance
            .bc_set
            .iter()
            .any(|s| s.starts_with("-z: convection")));

        // Round trip.
        let json = serde_json::to_string_pretty(&set).unwrap();
        assert!(json.contains("vcad.thermal-claims/1"));
        let back: ClaimSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema, set.schema);
        assert_eq!(back.claims.len(), set.claims.len());
        // No NaN sneaks into a receipt.
        for c in &back.claims {
            assert!(c.value.is_finite(), "non-finite claim {}", c.name);
        }
    }

    #[test]
    fn compare_binds_measurements_fail_closed() {
        let m = chip_model();
        let opts = SolveOptions::default();
        let sol = solve_steady(&m, &opts).unwrap();
        let set = predicted_claims(&m, &sol, &opts);

        // Unmeasured receipt never passes.
        let empty = compare(&set, &[]).unwrap();
        assert!(!empty.all_hold);
        assert!(empty
            .entries
            .iter()
            .all(|e| e.verdict == Verdict::Unmeasured));

        // A measurement of nothing is an error.
        let bogus = Measurement {
            name: "vibes_c".into(),
            value: 42.0,
            uncertainty: 0.1,
            instrument: "gut".into(),
            band_factor: 2.0,
        };
        assert!(compare(&set, &[bogus]).is_err());

        // Camera within band → Holds; a wildly wrong theta → Violated;
        // everything else stays Unmeasured.
        let t_pred = get(&set, "t_max_c");
        let camera = Measurement {
            name: "t_max_c".into(),
            value: t_pred * 1.08,
            uncertainty: 2.0,
            instrument: "thermal camera, eps=0.95 tape target".into(),
            band_factor: 1.25,
        };
        let bad_theta = Measurement {
            name: "theta_ja_c_per_w".into(),
            value: get(&set, "theta_ja_c_per_w") * 3.0,
            uncertainty: 0.5,
            instrument: "type-K TC epoxied to die".into(),
            band_factor: 1.3,
        };
        let report = compare(&set, &[camera, bad_theta]).unwrap();
        assert!(!report.all_hold);
        let verdict = |name: &str| {
            report
                .entries
                .iter()
                .find(|e| e.name == name)
                .unwrap()
                .verdict
        };
        assert_eq!(verdict("t_max_c"), Verdict::Holds);
        assert_eq!(verdict("theta_ja_c_per_w"), Verdict::Violated);
        assert_eq!(verdict("energy_balance_residual"), Verdict::Unmeasured);

        // All measured and holding → passes.
        let good_theta = Measurement {
            name: "theta_ja_c_per_w".into(),
            value: get(&set, "theta_ja_c_per_w") * 1.15,
            uncertainty: 0.5,
            instrument: "type-K TC epoxied to die".into(),
            band_factor: 1.3,
        };
        let camera2 = Measurement {
            name: "t_max_c".into(),
            value: t_pred * 0.95,
            uncertainty: 2.0,
            instrument: "thermal camera, eps=0.95 tape target".into(),
            band_factor: 1.25,
        };
        let resid = Measurement {
            name: "energy_balance_residual".into(),
            value: get(&set, "energy_balance_residual"),
            uncertainty: 1e-9,
            instrument: "solver self-audit".into(),
            band_factor: 10.0,
        };
        let ok = compare(&set, &[good_theta, camera2, resid]).unwrap();
        assert!(ok.all_hold);
        // Round trip: structural with a float tolerance — JSON shortest-
        // form printing can move the last ULP (the particle-crate gotcha).
        let json = serde_json::to_string(&ok).unwrap();
        assert!(json.contains("vcad.thermal-compare/1"));
        let back: ComparisonReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema, ok.schema);
        assert_eq!(back.all_hold, ok.all_hold);
        assert_eq!(back.entries.len(), ok.entries.len());
        for (a, b) in back.entries.iter().zip(&ok.entries) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.verdict, b.verdict);
            let scale = b.predicted.abs().max(1e-300);
            assert!((a.predicted - b.predicted).abs() / scale < 1e-12);
        }
    }

    #[test]
    fn multi_source_thetas_are_suffixed_and_zero_power_is_stated() {
        let mut m = chip_model();
        m.sources.push(PowerSource {
            name: "ldo".into(),
            shape: Shape::Box {
                min_mm: [5.0, 5.0, 0.0],
                size_mm: [6.0, 6.0, 2.0],
            },
            power_w: 0.0,
        });
        let opts = SolveOptions::default();
        let sol = solve_steady(&m, &opts).unwrap();
        let set = predicted_claims(&m, &sol, &opts);
        assert!(set.claims.iter().any(|c| c.name == "theta_ja_c_per_w:die"));
        // The zero-power source is stated, not silently dropped and not NaN.
        let undef = set
            .claims
            .iter()
            .find(|c| c.name == "theta_ja_undefined:ldo")
            .expect("stated undefined theta");
        assert!(undef.note.contains("undefined at P = 0"));
        assert!(set.claims.iter().all(|c| c.value.is_finite()));
    }
}
