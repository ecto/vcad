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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Boundary, MaterialRegion, PowerSource, Shape, ThermalModel};
    use crate::solve::solve_steady;

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
