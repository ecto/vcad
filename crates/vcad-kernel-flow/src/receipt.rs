//! Predicted-performance claims for the design receipt:
//! `vcad.flow-claims/1`.
//!
//! Emits a serializable claim set — `pressure_drop_pa`, `flow_rate_m3_s`,
//! and the `mass_balance_residual` conscience — with full solver
//! provenance (grid, relaxation time τ, lattice Mach number, steadiness
//! residual, Reynolds number and the envelope it was gated against), in
//! the spirit of `vcad.receipt/1`: every number carries how it was
//! produced, and nothing is defaulted silently.
//!
//! **Two routes or it doesn't ship.** When a lumped-oracle estimate of
//! the same pressure drop is available ([`crate::lumped`]), its gap to
//! the field solve rides in [`SolverProvenance::cross_route_residual`] —
//! the em-crate convention. A field solve whose residual against the
//! textbook correlation is unexplained is a bug report, not a claim.
//!
//! These are `basis: "predicted"` claims: a receipt built from them
//! rolls up Provisional, never Pass. Binding bench measurements
//! (flow-rate loop, manometer/anemometer) is [`compare`]; the printed
//! measurement pack is the M2 milestone.

use serde::{Deserialize, Serialize};

use crate::model::FlowModel;
use crate::solve::{Solution, SolveOptions};

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.flow-claims/1";

/// Domain tag for flow claims in the unified [`vcad_receipt`] schema.
pub const RECEIPT_DOMAIN: &str = "flow";

/// How the numbers were produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolverProvenance {
    /// Voxel counts per axis.
    pub grid: [usize; 3],
    /// Voxel edge, mm.
    pub voxel_mm: f64,
    /// BGK relaxation time τ (dimensionless; stability window is
    /// provenance, not trivia).
    pub tau: f64,
    /// Lattice Mach number — compressibility error is O(Ma²).
    pub mach: f64,
    /// Steps to steadiness.
    pub steps: usize,
    /// Final steadiness residual (relative L∞ velocity change per check
    /// interval).
    pub steady_residual: f64,
    /// The steadiness tolerance the run was asked to meet.
    pub steady_tol: f64,
    /// Inlet Reynolds number, when the model has an inlet.
    pub reynolds: Option<f64>,
    /// The laminar envelope the solve was gated against.
    pub re_envelope: f64,
    /// Relative gap between the field-solved pressure drop and the
    /// lumped-oracle route, when an oracle estimate was supplied:
    /// `|Δp_field − Δp_oracle| / max(|Δp_field|, |Δp_oracle|)`.
    pub cross_route_residual: Option<f64>,
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

fn claim(name: &str, value: f64, unit: &str, note: String) -> Claim {
    Claim {
        name: name.to_string(),
        value,
        unit: unit.to_string(),
        basis: "predicted".to_string(),
        note,
    }
}

/// The standing caveat every flow claim carries.
fn flow_caveat(model: &FlowModel, solution: &Solution) -> String {
    format!(
        "laminar single-phase isothermal LBM (D3Q19 BGK); weakly compressible, pressure \
         noise O(Ma^2) = {:.1e}; walls are voxel staircases at {:.3} mm resolution; no \
         turbulence model — gated at Re <= {:.0}",
        solution.scaling.mach * solution.scaling.mach,
        model.voxel_mm(),
        model.re_envelope,
    )
}

/// Build the predicted claim set for one solved configuration.
///
/// `opts` must be the options the solution was produced with — they are
/// provenance, and lying to a receipt defeats its purpose.
/// `oracle_dp_pa` is the lumped route's estimate of the same pressure
/// drop when one exists; supplying it populates `cross_route_residual`.
pub fn predicted_claims(
    model: &FlowModel,
    solution: &Solution,
    opts: &SolveOptions,
    oracle_dp_pa: Option<f64>,
) -> ClaimSet {
    let caveat = flow_caveat(model, solution);
    let has_ports = solution.inlet_flow_m3_s != 0.0 || solution.outlet_flow_m3_s != 0.0;

    let mut claims = Vec::new();
    if has_ports {
        claims.push(claim(
            "pressure_drop_pa",
            solution.pressure_drop_pa,
            "Pa",
            format!(
                "mean gauge pressure of fluid adjacent to inlet minus outlet; includes \
                 entrance effects, not just the developed core; {caveat}"
            ),
        ));
        claims.push(claim(
            "flow_rate_m3_s",
            solution.outlet_flow_m3_s,
            "m3/s",
            format!(
                "volumetric flow measured at the outlet (inlet link-realized {:.3e} m3/s); {caveat}",
                solution.inlet_flow_m3_s
            ),
        ));
        claims.push(claim(
            "mass_balance_residual",
            solution.mass_balance_residual,
            "1",
            "|Q_in - Q_out| / max(|Q_in|, |Q_out|); closes to solver tolerance or the \
             solution is wrong — this number is the audit, not a formality"
                .into(),
        ));
    }
    if let Some(t_out) = solution.outlet_temp_c {
        claims.push(claim(
            "outlet_temp_c",
            t_out,
            "C",
            format!("flux-weighted mean fluid temperature at the outlet; {caveat}"),
        ));
    }
    if let Some(q) = solution.heat_pickup_w {
        claims.push(claim(
            "heat_pickup_w",
            q,
            "W",
            format!("rho*c_p*(outlet enthalpy flux - inlet enthalpy flux); {caveat}"),
        ));
    }
    if let (Some(q), Some(w)) = (solution.heat_pickup_w, solution.wall_heat_w) {
        let denom = q.abs().max(w.abs());
        if denom > f64::MIN_POSITIVE {
            claims.push(claim(
                "thermal_energy_residual",
                (q - w).abs() / denom,
                "1",
                "|heat picked up by fluid - heat in through isothermal walls| / max; the \
                 thermal audit — closes at steady state or the scalar transport is wrong"
                    .into(),
            ));
        }
    }
    claims.push(claim(
        "max_speed_m_s",
        solution.max_speed_m_s,
        "m/s",
        format!("largest fluid speed in the domain; {caveat}"),
    ));

    let cross = oracle_dp_pa.map(|o| {
        let a = solution.pressure_drop_pa.abs();
        let b = o.abs();
        (solution.pressure_drop_pa - o).abs() / a.max(b).max(f64::MIN_POSITIVE)
    });

    ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: SolverProvenance {
            grid: model.divisions,
            voxel_mm: model.voxel_mm(),
            tau: solution.scaling.tau,
            mach: solution.scaling.mach,
            steps: solution.steps,
            steady_residual: solution.steady_residual,
            steady_tol: opts.steady_tol,
            reynolds: model.reynolds(),
            re_envelope: model.re_envelope,
            cross_route_residual: cross,
        },
        claims,
    }
}

/// A bench measurement to bind against a predicted claim (M2 pack).
///
/// The instruments this pack expects, and their traps:
///
/// - **Flow**: a printed orifice plate + manometer, or a hot-wire
///   anemometer. Anemometers read point speed, not volumetric flow —
///   traverse the duct or derate the band.
/// - **Pressure**: micromanometers drift; zero them and record the
///   zero-offset procedure in `instrument`. Laminar Δp at maker scale is
///   pascals — tubing leaks are the dominant error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    /// Claim name this measures (must match a claim).
    pub name: String,
    /// Measured value, in the claim's unit.
    pub value: f64,
    /// One-sigma absolute uncertainty, same unit.
    pub uncertainty: f64,
    /// Instrument provenance.
    pub instrument: String,
    /// Acceptance band as a multiplicative factor: the claim holds when
    /// the measurement lies in [predicted/band − u, predicted·band + u].
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
/// no claim is an error.
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
        schema: "vcad.flow-compare/1".to_string(),
        entries,
        all_hold: all_hold && measured_any,
    })
}

/// The oracle reference for this crate's LBM solver.
pub fn oracle() -> vcad_receipt::OracleRef {
    vcad_receipt::OracleRef::new("vcad-kernel-flow/solve", env!("CARGO_PKG_VERSION"))
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
/// never Pass**. The computed value rides in `measured` ("what the
/// oracle computed"); solver provenance, including the cross-route
/// residual, rides in `details`.
pub fn design_claims(set: &ClaimSet) -> Vec<vcad_receipt::ReceiptClaim> {
    let oracle = oracle();
    let cross = match set.provenance.cross_route_residual {
        Some(r) => format!("cross-route residual vs lumped oracle {r:.3e}"),
        None => "no lumped-oracle cross-check supplied".to_string(),
    };
    let provenance = format!(
        "grid {}x{}x{}, voxel {:.3} mm, tau {:.4}, Ma {:.3}, {} steps, steady residual \
         {:.3e}, Re {} (envelope {:.0}), {}",
        set.provenance.grid[0],
        set.provenance.grid[1],
        set.provenance.grid[2],
        set.provenance.voxel_mm,
        set.provenance.tau,
        set.provenance.mach,
        set.provenance.steps,
        set.provenance.steady_residual,
        set.provenance
            .reynolds
            .map(|r| format!("{r:.0}"))
            .unwrap_or_else(|| "n/a (body-force drive)".to_string()),
        set.provenance.re_envelope,
        cross,
    );
    set.claims
        .iter()
        .map(|c| {
            vcad_receipt::ReceiptClaim::pass(
                format!("flow.{}", c.name),
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
    use crate::model::{Cell, FlowModel};
    use crate::solve::{solve_steady, SolveOptions};

    fn solved_duct() -> (FlowModel, Solution) {
        let (nx, ny, nz) = (24usize, 7usize, 7usize);
        let mut m = FlowModel::new([0.0; 3], [24.0, 7.0, 7.0], [nx, ny, nz]);
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let x = m.index(i, j, k);
                    m.cells[x] = if i == 0 {
                        Cell::Inlet
                    } else if i == nx - 1 {
                        Cell::Outlet
                    } else {
                        Cell::Fluid
                    };
                }
            }
        }
        m.inlet_velocity_m_s = [0.08, 0.0, 0.0];
        let sol = solve_steady(&m, &SolveOptions::default()).expect("duct");
        (m, sol)
    }

    #[test]
    fn claims_carry_provenance_and_cross_route() {
        let (m, sol) = solved_duct();
        let opts = SolveOptions::default();
        let set = predicted_claims(&m, &sol, &opts, Some(sol.pressure_drop_pa * 1.05));
        assert_eq!(set.schema, CLAIM_SCHEMA);
        assert!(set.claims.iter().all(|c| c.basis == "predicted"));
        assert!(set.claims.iter().any(|c| c.name == "pressure_drop_pa"));
        assert!(set.claims.iter().any(|c| c.name == "mass_balance_residual"));
        let cross = set.provenance.cross_route_residual.unwrap();
        assert!((cross - 0.05 / 1.05).abs() < 1e-9, "cross = {cross}");
        assert!(set.provenance.reynolds.unwrap() > 0.0);
    }

    #[test]
    fn design_claims_ride_the_unified_receipt_as_provisional() {
        let (m, sol) = solved_duct();
        let set = predicted_claims(&m, &sol, &SolveOptions::default(), None);
        let claims = design_claims(&set);
        assert!(!claims.is_empty());
        for c in &claims {
            assert_eq!(c.domain, RECEIPT_DOMAIN);
            assert_eq!(c.basis, Some(vcad_receipt::ClaimBasis::Predicted));
            assert!(c.details.as_deref().unwrap().contains("tau"));
        }
        let receipt = vcad_receipt::DesignReceipt::with_claims(claims);
        assert_eq!(receipt.verdict(), vcad_receipt::ReceiptVerdict::Provisional);
    }

    #[test]
    fn compare_is_fail_closed() {
        let (m, sol) = solved_duct();
        let set = predicted_claims(&m, &sol, &SolveOptions::default(), None);
        // No measurements: nothing holds.
        let report = compare(&set, &[]).unwrap();
        assert!(!report.all_hold);
        assert!(report
            .entries
            .iter()
            .all(|e| e.verdict == Verdict::Unmeasured));
        // A measurement of nothing is an error.
        assert!(compare(
            &set,
            &[Measurement {
                name: "not_a_claim".into(),
                value: 1.0,
                uncertainty: 0.1,
                instrument: "imagination".into(),
                band_factor: 1.2,
            }]
        )
        .is_err());
        // A measurement inside the band holds.
        let dp = set
            .claims
            .iter()
            .find(|c| c.name == "pressure_drop_pa")
            .unwrap()
            .value;
        let report = compare(
            &set,
            &[Measurement {
                name: "pressure_drop_pa".into(),
                value: dp * 1.1,
                uncertainty: dp.abs() * 0.05,
                instrument: "micromanometer, zeroed".into(),
                band_factor: 1.25,
            }],
        )
        .unwrap();
        assert!(report.all_hold);
    }
}
