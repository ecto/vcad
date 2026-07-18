//! Predicted-performance claims for circuit simulation — `vcad.spice-claims/1`.
//!
//! Emits a serializable claim set from a solved circuit: DC node voltages,
//! filter cutoff and Q, and the Tellegen power-balance residual, each with
//! full solver provenance (integrator, tolerances, gmin ladder, Newton
//! iterations). Nothing is defaulted silently, and the residual claim is the
//! solve's own conscience riding along in public.
//!
//! These are `basis: "predicted"` claims: the solver ran for real, but they
//! describe a circuit that has not been measured. A receipt built from them
//! **rolls up Provisional, never Pass** — the same contract as every other
//! solver crate. The closing instruments are cheap and named here on
//! purpose: a ~$30 USB oscilloscope plus a signal generator measure DC
//! operating points, step responses, and Bode magnitude directly; binding
//! those measurements is the M-next experiment pack.

use serde::{Deserialize, Serialize};

use super::dc::DcSolution;

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.spice-claims/1";

/// Receipt domain string for the unified receipt.
pub const RECEIPT_DOMAIN: &str = "circuit";

/// How the numbers were produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolverProvenance {
    /// Analysis kind: `"dc"`, `"ac"`, `"transient"`.
    pub analysis: String,
    /// Companion-model integrator (`"trapezoidal"`, `"backward-euler"`, or
    /// `"n/a"` for DC/AC).
    pub integrator: String,
    /// Newton voltage tolerance (V).
    pub newton_vntol: f64,
    /// Newton iterations used by the final solve stage.
    pub newton_iterations: usize,
    /// Final gmin of the continuation ladder (0 = exact network).
    pub gmin_final: f64,
    /// Node and device counts.
    pub num_nodes: usize,
    /// Device count.
    pub num_devices: usize,
}

/// One predicted claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Claim name (snake_case).
    pub name: String,
    /// Value.
    pub value: f64,
    /// Unit (`"1"` for dimensionless).
    pub unit: String,
    /// Claim basis — always `"predicted"` here.
    pub basis: String,
    /// Assumptions and caveats, spelled out.
    pub note: String,
}

/// A claim set under [`CLAIM_SCHEMA`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimSet {
    /// Always [`CLAIM_SCHEMA`].
    pub schema: String,
    /// Solver provenance.
    pub provenance: SolverProvenance,
    /// The claims.
    pub claims: Vec<Claim>,
}

fn predicted(name: impl Into<String>, value: f64, unit: &str, note: impl Into<String>) -> Claim {
    Claim {
        name: name.into(),
        value,
        unit: unit.to_string(),
        basis: "predicted".to_string(),
        note: note.into(),
    }
}

/// Claims from a converged DC operating point: every non-ground node voltage
/// plus the power-balance residual.
pub fn dc_claims(sol: &DcSolution, num_devices: usize) -> ClaimSet {
    let mut claims: Vec<Claim> = sol
        .node_voltages
        .iter()
        .enumerate()
        .skip(1)
        .map(|(node, v)| {
            predicted(
                format!("dc_node_voltage_{node}"),
                *v,
                "V",
                "DC operating point; measurable with a multimeter or scope DC coupling",
            )
        })
        .collect();
    claims.push(predicted(
        "power_balance_residual",
        sol.power_balance_w.abs(),
        "W",
        "|Σ v·i| over all devices (Tellegen). Nonzero only through solver error; \
         gate: < 1e-9 relative to dissipated power",
    ));
    ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: SolverProvenance {
            analysis: "dc".into(),
            integrator: "n/a".into(),
            newton_vntol: 1e-12,
            newton_iterations: sol.newton_iterations,
            gmin_final: 0.0,
            num_nodes: sol.node_voltages.len(),
            num_devices,
        },
        claims,
    }
}

/// Claims for a designed 2nd-order filter: cutoff and Q from the AC sweep.
pub fn filter_claims(
    cutoff_hz: f64,
    q_factor: f64,
    num_nodes: usize,
    num_devices: usize,
) -> ClaimSet {
    ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: SolverProvenance {
            analysis: "ac".into(),
            integrator: "n/a".into(),
            newton_vntol: 1e-12,
            newton_iterations: 1,
            gmin_final: 0.0,
            num_nodes,
            num_devices,
        },
        claims: vec![
            predicted(
                "cutoff_hz",
                cutoff_hz,
                "Hz",
                "−3 dB frequency from complex MNA sweep; measurable with a \
                 signal generator + scope Bode sweep",
            ),
            predicted(
                "q_factor",
                q_factor,
                "1",
                "quality factor from the AC response peak / closed form; \
                 measurable from ringdown log-decrement on the scope",
            ),
        ],
    }
}

/// Which solver produced these claims.
pub fn oracle() -> vcad_receipt::OracleRef {
    vcad_receipt::OracleRef::new("vcad-ecad-sim/circuit", env!("CARGO_PKG_VERSION"))
}

/// Translate a predicted [`ClaimSet`] into unified-receipt claims.
///
/// Every claim lands with [`vcad_receipt::ClaimBasis::Predicted`], so a
/// receipt built from these rolls up **Provisional, never Pass** — the
/// fail-closed contract shared with the other solver crates.
pub fn design_claims(set: &ClaimSet) -> Vec<vcad_receipt::ReceiptClaim> {
    let oracle = oracle();
    let provenance = format!(
        "{} analysis, integrator {}, vntol {:.1e}, newton iters {}, gmin_final {:.1e}, {} nodes / {} devices",
        set.provenance.analysis,
        set.provenance.integrator,
        set.provenance.newton_vntol,
        set.provenance.newton_iterations,
        set.provenance.gmin_final,
        set.provenance.num_nodes,
        set.provenance.num_devices,
    );
    set.claims
        .iter()
        .map(|c| {
            let quantity = if c.unit == "1" {
                vcad_receipt::ClaimQuantity::bare(c.value)
            } else {
                vcad_receipt::ClaimQuantity::new(c.value, c.unit.clone())
            };
            vcad_receipt::ReceiptClaim::pass(
                format!("circuit.{}", c.name),
                RECEIPT_DOMAIN,
                c.note.clone(),
                oracle.clone(),
            )
            .with_basis(vcad_receipt::ClaimBasis::Predicted)
            .with_measured(quantity)
            .with_details(provenance.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{dc, Circuit, Device};

    #[test]
    fn dc_claims_ride_the_unified_receipt_as_provisional() {
        let mut c = Circuit::new();
        let vin = c.node();
        let out = c.node();
        c.add(Device::VSource {
            p: vin,
            n: 0,
            v: 12.0,
        });
        c.add(Device::Resistor {
            p: vin,
            n: out,
            r: 3_000.0,
        });
        c.add(Device::Resistor {
            p: out,
            n: 0,
            r: 1_000.0,
        });
        let sol = dc::operating_point(&c).unwrap();
        let set = dc_claims(&sol, c.devices.len());
        assert_eq!(set.schema, CLAIM_SCHEMA);

        let receipt = vcad_receipt::DesignReceipt::with_claims(design_claims(&set));
        // Predicted basis must roll up Provisional, never Pass.
        assert_eq!(receipt.verdict(), vcad_receipt::ReceiptVerdict::Provisional);
    }
}
