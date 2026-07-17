//! Predicted-performance claims for the design receipt.
//!
//! Emits a serializable claim set — interception, recirculation,
//! neutron rate, fusion power, Q, and **distance to Lawson** — with full
//! solver provenance (grid, tolerance, ensemble, censoring, physics
//! channels included), in the spirit of `vcad.receipt/1`: every number
//! carries how it was produced, and nothing is defaulted silently.
//!
//! These are `basis: "predicted"` claims. Binding them to measurements
//! (the cathode ammeter, a calibrated neutron counter) is the experiment
//! pack's job — see `docs/particle-optics-m0.md` M6. Wiring this family
//! into `crates/vcad-receipt` + the MCP surface is the flagged follow-up
//! PR (it touches the cross-crate schema and TS codegen).

use serde::{Deserialize, Serialize};

use crate::fom::EnsembleStats;
use crate::poisson::Solution;
use crate::trace::TraceOptions;

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.particle-claims/1";

/// Energy per D(d,n)³He reaction (both products), joules. 3.27 MeV.
pub const E_DDN_J: f64 = 3.27e6 * crate::constants::ELEMENTARY_CHARGE;
/// Energy per D(d,p)T reaction (both products), joules. 4.03 MeV.
pub const E_DDP_J: f64 = 4.03e6 * crate::constants::ELEMENTARY_CHARGE;

/// The operating point claims are priced at.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OperatingPoint {
    /// Injected ion current, amperes.
    pub ion_current_a: f64,
    /// D₂ background pressure, mTorr.
    pub d2_pressure_mtorr: f64,
    /// Gas temperature, kelvin.
    pub temperature_k: f64,
}

/// How the numbers were produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolverProvenance {
    /// Radial × axial node counts.
    pub grid: [usize; 2],
    /// SOR relative tolerance.
    pub sor_tol: f64,
    /// SOR sweeps used by the converged solve.
    pub sor_sweeps: usize,
    /// Trace ensemble size.
    pub ensemble: usize,
    /// Pass cap (censoring boundary).
    pub max_passes: u32,
    /// Fraction of traces censored at the budget.
    pub survivor_fraction: f64,
    /// Worst per-trace energy drift (integration quality).
    pub max_energy_drift_rel: f64,
    /// Physics channels included in the prediction.
    pub channels: Vec<String>,
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
    /// Solver and ensemble provenance.
    pub provenance: SolverProvenance,
    /// Operating point.
    pub operating_point: OperatingPoint,
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

/// Build the predicted claim set for one solved-and-traced configuration.
///
/// `drop_v` is the accelerating potential drop (volts, absolute) used to
/// price input power; `stats` must come from the same trace options.
pub fn predicted_claims(
    stats: &EnsembleStats,
    solution: &Solution,
    trace_opts: &TraceOptions,
    sor_tol: f64,
    drop_v: f64,
    op: &OperatingPoint,
) -> ClaimSet {
    let n_d = crate::xsection::d2_deuteron_density_m3(op.d2_pressure_mtorr, op.temperature_k);
    let ions_per_s = op.ion_current_a / crate::constants::ELEMENTARY_CHARGE;

    let mut channels = vec!["beam_on_background".to_string()];
    let neutron_rate = if trace_opts.cx.is_some() {
        channels.push("cx_survival".to_string());
        channels.push("fast_neutral".to_string());
        ions_per_s * (stats.mean_neutrons_ion_channel + stats.mean_neutrons_cx_channel)
    } else {
        ions_per_s * n_d * stats.mean_ddn_sigma_v_m3
    };

    // Fusion power from both branches (beam-on-background integrals; the
    // CX chain and beam-beam channels are not priced — floor).
    let rate_n = ions_per_s * n_d * stats.mean_ddn_sigma_v_m3;
    let rate_p = ions_per_s * n_d * stats.mean_ddp_sigma_v_m3;
    let p_fus = rate_n * E_DDN_J + rate_p * E_DDP_J;
    let p_in = op.ion_current_a * drop_v;
    let q = if p_in > 0.0 { p_fus / p_in } else { 0.0 };
    let distance_orders = if q > 0.0 {
        (1.0 / q).log10().min(99.0)
    } else {
        99.0
    };

    let claims = vec![
        claim(
            "interception_fraction",
            stats.interception_fraction,
            "1",
            "fraction of traced ions ending on cathode wire; the hardware \
             observable is cathode interception current",
        ),
        claim(
            "mean_core_passes",
            stats.mean_passes,
            "passes",
            "recirculation count, censored at max_passes",
        ),
        claim(
            "effective_transparency",
            stats.effective_transparency,
            "1",
            "geometric-survival estimate m/(m+1); lower bound under censoring",
        ),
        claim(
            "ddn_neutron_rate",
            neutron_rate,
            "n/s",
            "D(d,n)3He at the stated operating point; channels as listed in \
             provenance; CX chain and beam-beam are NOT included (floor)",
        ),
        claim(
            "fusion_power",
            p_fus,
            "W",
            "both D-D branches, beam-on-background integrals only",
        ),
        claim("input_power", p_in, "W", "ion current x potential drop"),
        claim(
            "q_estimate",
            q,
            "1",
            "fusion power / input power at the stated operating point",
        ),
        claim(
            "distance_to_lawson",
            distance_orders,
            "orders",
            "log10(1/Q); breakeven at 0; capped at 99 when no fusion is \
             predicted",
        ),
    ];

    ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: SolverProvenance {
            grid: [solution.nr, solution.nz],
            sor_tol,
            sor_sweeps: solution.sweeps,
            ensemble: stats.n,
            max_passes: trace_opts.max_passes,
            survivor_fraction: stats.survivor_fraction,
            max_energy_drift_rel: stats.max_energy_drift_rel,
            channels,
        },
        operating_point: *op,
        claims,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Device;
    use crate::field::FieldMap;
    use crate::fom::stats;
    use crate::poisson::{solve, SolveOptions};
    use crate::trace::{TraceOptions, Tracer, DEUTERON};

    fn build() -> ClaimSet {
        let device = Device::classic_fusor(120.0, 40.0, 5, 1.0, -30_000.0);
        let sopts = SolveOptions::default();
        let sol = solve(&device, 81, 161, &sopts).unwrap();
        let fields = FieldMap::new(&device, &sol);
        let topts = TraceOptions {
            max_passes: 10,
            ..TraceOptions::default()
        };
        let tracer = Tracer::new(&device, &fields, &sol, topts);
        let s = stats(&tracer.launch_ensemble(DEUTERON, 12));
        predicted_claims(
            &s,
            &sol,
            &topts,
            sopts.tol,
            30_000.0,
            &OperatingPoint {
                ion_current_a: 0.010,
                d2_pressure_mtorr: 2.0,
                temperature_k: 300.0,
            },
        )
    }

    fn get(set: &ClaimSet, name: &str) -> f64 {
        set.claims
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("missing claim {name}"))
            .value
    }

    #[test]
    fn q_arithmetic_is_consistent_and_honest() {
        let set = build();
        let q = get(&set, "q_estimate");
        let p_fus = get(&set, "fusion_power");
        let p_in = get(&set, "input_power");
        assert!((q - p_fus / p_in).abs() < 1e-15);
        assert!((p_in - 300.0).abs() < 1e-9, "10 mA x 30 kV = 300 W");
        // A fusor is microwatts against hundreds of watts: Q around 1e-9,
        // distance to Lawson around 9 orders. Honesty is load-bearing.
        assert!(q > 1e-12 && q < 1e-6, "q = {q:.3e}");
        let d = get(&set, "distance_to_lawson");
        assert!((d - (1.0 / q).log10()).abs() < 1e-9);
        assert!((6.0..12.0).contains(&d), "distance = {d}");
    }

    #[test]
    fn serializes_with_schema_and_provenance() {
        let set = build();
        let json = serde_json::to_string_pretty(&set).expect("serialize");
        assert!(json.contains("vcad.particle-claims/1"));
        assert!(json.contains("distance_to_lawson"));
        assert!(json.contains("beam_on_background"));
        let back: ClaimSet = serde_json::from_str(&json).expect("round trip");
        // Compare structurally with a float tolerance (JSON float printing
        // can move the last ULP).
        assert_eq!(back.schema, set.schema);
        assert_eq!(back.claims.len(), set.claims.len());
        for (a, b) in back.claims.iter().zip(&set.claims) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.unit, b.unit);
            let scale = b.value.abs().max(1e-300);
            assert!(
                (a.value - b.value).abs() / scale < 1e-12,
                "claim {} drifted: {} vs {}",
                a.name,
                a.value,
                b.value
            );
        }
        // Provenance is present and truthful.
        assert_eq!(back.provenance.grid, [81, 161]);
        assert_eq!(back.provenance.ensemble, 12);
    }

    #[test]
    fn no_fusion_is_fail_closed_not_flattering() {
        // A 100 V device fuses nothing; the receipt must say "99 orders",
        // not divide by zero or omit the claim.
        let device = Device::classic_fusor(120.0, 40.0, 3, 1.0, -100.0);
        let sopts = SolveOptions::default();
        let sol = solve(&device, 41, 81, &sopts).unwrap();
        let fields = FieldMap::new(&device, &sol);
        let topts = TraceOptions {
            max_passes: 4,
            ..TraceOptions::default()
        };
        let tracer = Tracer::new(&device, &fields, &sol, topts);
        let s = stats(&tracer.launch_ensemble(DEUTERON, 6));
        let set = predicted_claims(
            &s,
            &sol,
            &topts,
            sopts.tol,
            100.0,
            &OperatingPoint {
                ion_current_a: 0.010,
                d2_pressure_mtorr: 2.0,
                temperature_k: 300.0,
            },
        );
        let d = get(&set, "distance_to_lawson");
        assert!(d > 20.0, "no-fusion distance must be huge: {d}");
    }
}
