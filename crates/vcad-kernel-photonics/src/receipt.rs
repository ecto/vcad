//! Predicted-performance claims for the design receipt.
//!
//! Emits a serializable claim set for a 1×2 splitter — transmission per
//! arm, insertion loss, splitting ratio, minimum feature scale — with
//! full solver provenance (grid, cells/λ in vacuum *and in the core
//! material*, Courant, steps, CPML, monitor frequencies, and the
//! solver's own on-axis dispersion error priced from
//! [`crate::dispersion`]), in the spirit of `vcad.receipt/1`: every
//! number carries how it was produced, and nothing is defaulted
//! silently.
//!
//! Construction is **fail-closed**: non-positive input power, NaN
//! anywhere, or an empty spectrum refuses to produce claims rather than
//! producing optimistic ones. These are `basis: "predicted"` claims —
//! binding them to a measured chip is the M6 tape-out pack's `compare()`
//! job. Wiring this family into `crates/vcad-receipt` + the MCP surface
//! is a flagged follow-up (it touches the cross-crate schema and TS
//! codegen).

use serde::{Deserialize, Serialize};

use crate::design::TopologyParam;
use crate::dispersion::fdtd_wavenumber;
use crate::sim::Simulation;

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.photonics-claims/1";

/// How the numbers were produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolverProvenance {
    /// Grid cells `[nx, ny]`.
    pub grid: [usize; 2],
    /// Grid pitch Δ (length units).
    pub delta: f64,
    /// Cells per vacuum wavelength at the design λ.
    pub cells_per_lambda: f64,
    /// Cells per **material** wavelength in the core — the honest
    /// resolution number (λ/n_core/Δ).
    pub cells_per_lambda_core: f64,
    /// Courant factor S (dt = S·Δ/√2).
    pub courant: f64,
    /// Time steps run.
    pub steps: usize,
    /// Simulated time (units of L/c).
    pub run_time: f64,
    /// CPML thicknesses `[x_lo, x_hi, y_lo, y_hi]` in cells.
    pub cpml_cells: [usize; 4],
    /// Monitored frequencies.
    pub monitor_freqs: Vec<f64>,
    /// Polarization ("TM": E out of plane).
    pub polarization: String,
    /// The solver's own error model, priced in: on-axis numerical
    /// dispersion (k_fdtd − k_exact)/k_exact at the design frequency.
    pub dispersion_k_rel_error: f64,
}

impl SolverProvenance {
    /// Assemble provenance from a configured simulation and the design
    /// point (λ₀, core index), after `steps` steps.
    pub fn from_sim(sim: &Simulation, lambda0: f64, n_core: f64, steps: usize) -> Self {
        let g = sim.grid();
        let c = sim.cpml_spec();
        let omega = 2.0 * std::f64::consts::PI / lambda0;
        let k_fdtd = fdtd_wavenumber(omega, g.delta, sim.dt()).unwrap_or(f64::NAN);
        Self {
            grid: [g.nx, g.ny],
            delta: g.delta,
            cells_per_lambda: lambda0 / g.delta,
            cells_per_lambda_core: lambda0 / n_core / g.delta,
            courant: sim.courant(),
            steps,
            run_time: steps as f64 * sim.dt(),
            cpml_cells: [c.x_lo, c.x_hi, c.y_lo, c.y_hi],
            monitor_freqs: Vec::new(),
            polarization: "TM".to_string(),
            dispersion_k_rel_error: (k_fdtd - omega) / omega,
        }
    }
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

/// Per-frequency measured spectrum row for the claim set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpectrumRow {
    /// Frequency (1/λ).
    pub freq: f64,
    /// Arm-A transmission.
    pub t_a: f64,
    /// Arm-B transmission.
    pub t_b: f64,
}

/// The full claim set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimSet {
    /// Schema tag ([`CLAIM_SCHEMA`]).
    pub schema: String,
    /// Solver provenance.
    pub provenance: SolverProvenance,
    /// The claims (at the center frequency).
    pub claims: Vec<Claim>,
    /// The transmission spectrum behind the claims.
    pub spectrum: Vec<SpectrumRow>,
}

/// One monitor reading feeding the claims.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplitterMeasurement {
    /// Frequency.
    pub freq: f64,
    /// Input-arm power (normalization).
    pub p_in: f64,
    /// Output arm A power.
    pub p_arm_a: f64,
    /// Output arm B power.
    pub p_arm_b: f64,
}

/// Refusals — a claim set is never built from unusable numbers.
#[derive(Debug, Clone, PartialEq)]
pub enum ClaimError {
    /// No spectrum rows supplied.
    EmptySpectrum,
    /// Input power non-positive or NaN at some frequency.
    BadInputPower(f64),
    /// An output power was NaN or negative beyond roundoff.
    BadOutputPower(f64),
    /// The requested center frequency is not among the measurements.
    CenterFrequencyMissing(f64),
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimError::EmptySpectrum => write!(f, "no spectrum measurements"),
            ClaimError::BadInputPower(fr) => write!(f, "unusable input power at f = {fr}"),
            ClaimError::BadOutputPower(fr) => write!(f, "unusable output power at f = {fr}"),
            ClaimError::CenterFrequencyMissing(fr) => {
                write!(f, "center frequency {fr} not among measurements")
            }
        }
    }
}

impl std::error::Error for ClaimError {}

fn claim(name: &str, value: f64, unit: &str, note: &str) -> Claim {
    Claim {
        name: name.to_string(),
        value,
        unit: unit.to_string(),
        basis: "predicted".to_string(),
        note: note.to_string(),
    }
}

/// Build the predicted claim set for a 1×2 splitter.
///
/// `center_freq` selects which measurement the headline claims are made
/// at (it must be present in `meas`); `topo` supplies the minimum
/// feature scale if the geometry came from topology optimization.
pub fn splitter_claims(
    meas: &[SplitterMeasurement],
    center_freq: f64,
    mut provenance: SolverProvenance,
    topo: Option<&TopologyParam>,
) -> Result<ClaimSet, ClaimError> {
    if meas.is_empty() {
        return Err(ClaimError::EmptySpectrum);
    }
    for m in meas {
        // NaN-fail-closed: NaN must land in the refusal arm.
        if m.p_in.is_nan() || m.p_in <= 0.0 {
            return Err(ClaimError::BadInputPower(m.freq));
        }
        // Tiny negative flux can appear from monitor interpolation noise;
        // anything materially negative is a refusal, not a clamp.
        if m.p_arm_a.is_nan()
            || m.p_arm_b.is_nan()
            || m.p_arm_a < -1e-6 * m.p_in
            || m.p_arm_b < -1e-6 * m.p_in
        {
            return Err(ClaimError::BadOutputPower(m.freq));
        }
    }
    let center = meas
        .iter()
        .find(|m| (m.freq - center_freq).abs() < 1e-12)
        .ok_or(ClaimError::CenterFrequencyMissing(center_freq))?;

    let t_a = center.p_arm_a / center.p_in;
    let t_b = center.p_arm_b / center.p_in;
    let total = t_a + t_b;
    let mut claims = vec![
        claim(
            "transmission_arm_a",
            t_a,
            "1",
            "P_a/P_in at the center frequency; 2D TM prediction — \
             quantitative for the 2D problem, qualitative for a 3D chip",
        ),
        claim(
            "transmission_arm_b",
            t_b,
            "1",
            "P_b/P_in at the center frequency",
        ),
        claim(
            "transmission_total",
            total,
            "1",
            "1 − total = reflection + radiation + monitor discretization",
        ),
        claim(
            "insertion_loss_db",
            -10.0 * total.max(f64::MIN_POSITIVE).log10(),
            "dB",
            "−10·log₁₀(T_a + T_b); excess loss over an ideal splitter",
        ),
        claim(
            "splitting_ratio",
            t_a / total.max(f64::MIN_POSITIVE),
            "1",
            "T_a/(T_a + T_b); 0.5 is a perfect 50/50 split",
        ),
        claim(
            "arm_a_db",
            -10.0 * t_a.max(f64::MIN_POSITIVE).log10(),
            "dB",
            "per-arm level; the 50/50 target is 3.01 dB",
        ),
        claim(
            "arm_b_db",
            -10.0 * t_b.max(f64::MIN_POSITIVE).log10(),
            "dB",
            "per-arm level; the 50/50 target is 3.01 dB",
        ),
    ];
    if let Some(t) = topo {
        claims.push(claim(
            "min_feature_nm",
            2.0 * t.filter_radius_cells * provenance.delta * 1000.0,
            "nm",
            "cone-filter diameter, assuming 1 length unit = 1 µm; a \
             regularization scale, NOT a geometric guarantee — measured \
             post-binarization linewidth is the tape-out check (M6)",
        ));
    }
    provenance.monitor_freqs = meas.iter().map(|m| m.freq).collect();
    let spectrum = meas
        .iter()
        .map(|m| SpectrumRow {
            freq: m.freq,
            t_a: m.p_arm_a / m.p_in,
            t_b: m.p_arm_b / m.p_in,
        })
        .collect();
    Ok(ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance,
        claims,
        spectrum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prov() -> SolverProvenance {
        SolverProvenance {
            grid: [300, 200],
            delta: 1.55 / 40.0,
            cells_per_lambda: 40.0,
            cells_per_lambda_core: 40.0 / 3.48,
            courant: 0.5,
            steps: 4000,
            run_time: 54.8,
            cpml_cells: [12, 12, 12, 12],
            monitor_freqs: vec![],
            polarization: "TM".into(),
            dispersion_k_rel_error: 3.6e-3,
        }
    }

    #[test]
    fn perfect_split_hits_the_textbook_numbers() {
        let meas = [SplitterMeasurement {
            freq: 0.6452,
            p_in: 2.0,
            p_arm_a: 1.0,
            p_arm_b: 1.0,
        }];
        let cs = splitter_claims(&meas, 0.6452, prov(), None).unwrap();
        let get = |n: &str| cs.claims.iter().find(|c| c.name == n).unwrap().value;
        assert!((get("transmission_total") - 1.0).abs() < 1e-12);
        assert!((get("splitting_ratio") - 0.5).abs() < 1e-12);
        assert!((get("arm_a_db") - 3.0103).abs() < 1e-3);
        assert!((get("insertion_loss_db")).abs() < 1e-9);
        assert!(cs.claims.iter().all(|c| c.basis == "predicted"));
    }

    #[test]
    fn fail_closed_on_unusable_numbers() {
        assert_eq!(
            splitter_claims(&[], 1.0, prov(), None).unwrap_err(),
            ClaimError::EmptySpectrum
        );
        let bad_in = [SplitterMeasurement {
            freq: 1.0,
            p_in: 0.0,
            p_arm_a: 0.1,
            p_arm_b: 0.1,
        }];
        assert!(matches!(
            splitter_claims(&bad_in, 1.0, prov(), None).unwrap_err(),
            ClaimError::BadInputPower(_)
        ));
        let nan_out = [SplitterMeasurement {
            freq: 1.0,
            p_in: 1.0,
            p_arm_a: f64::NAN,
            p_arm_b: 0.1,
        }];
        assert!(matches!(
            splitter_claims(&nan_out, 1.0, prov(), None).unwrap_err(),
            ClaimError::BadOutputPower(_)
        ));
        let ok = [SplitterMeasurement {
            freq: 1.0,
            p_in: 1.0,
            p_arm_a: 0.4,
            p_arm_b: 0.4,
        }];
        assert!(matches!(
            splitter_claims(&ok, 2.0, prov(), None).unwrap_err(),
            ClaimError::CenterFrequencyMissing(_)
        ));
    }

    #[test]
    fn serde_round_trip() {
        let meas = [
            SplitterMeasurement {
                freq: 0.62,
                p_in: 1.0,
                p_arm_a: 0.46,
                p_arm_b: 0.47,
            },
            SplitterMeasurement {
                freq: 0.6452,
                p_in: 1.0,
                p_arm_a: 0.48,
                p_arm_b: 0.48,
            },
        ];
        let cs = splitter_claims(&meas, 0.6452, prov(), None).unwrap();
        let json = serde_json::to_string(&cs).unwrap();
        let back: ClaimSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cs);
        assert_eq!(back.schema, CLAIM_SCHEMA);
        assert_eq!(back.spectrum.len(), 2);
        assert_eq!(back.provenance.monitor_freqs, vec![0.62, 0.6452]);
    }
}
