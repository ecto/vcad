//! Component-tolerance yield of a circuit — the adjoint × stackup bridge.
//!
//! `vcad-kernel-tolerance` answers "does this fit, and at what yield?" for
//! dimension chains: a linearized response G = Σ aᵢxᵢ plus per-parameter
//! tolerances in, worst-case / RSS / allocation out. The circuit adjoint
//! (`circuit::adjoint`) produces exactly that linearization for free —
//! d(output)/d(every component value) from one transposed solve. This module
//! plugs the two together:
//!
//! - **coeff aᵢ** = dy/dpᵢ from the adjoint (exact for the network as
//!   discretized, not a finite difference),
//! - **nominal xᵢ** = the device's primary scalar (Ω, F, H, V, A),
//! - **tolerance** = ±frac·|nominal| under a stated σ convention,
//!
//! and hands the resulting [`Stackup`] to the kernel-tolerance machinery for
//! worst-case bounds, RSS σ, and min-cost allocation. Mixed units across
//! contributors are fine: the coefficient carries the unit conversion, and
//! the gap is in output units (V, or dimensionless |H|).
//!
//! **The Monte Carlo is not the linearization.** [`analyze`]'s MC loop
//! re-runs the actual solver (full Newton DC operating point, or the full
//! complex-MNA AC solve) on each sampled circuit, so the linear model is
//! *checked* against nonlinear re-solves rather than trusted. The
//! discrepancy — max and RMS |y_full − y_linear| over the samples, and
//! σ_linear vs σ_MC — is reported on every result. That is the honesty
//! number: at ±1% on a divider it is machine noise; at ±20% on a resonant
//! filter the linearization visibly breaks, and the tests assert that it
//! does.
//!
//! **Fail-closed refusals:**
//!
//! - A toleranced device whose AC sensitivity slot is a deferred placeholder
//!   (diodes at M0 — see [`AcSensitivity::deferred`]) is refused, not
//!   silently treated as zero-sensitivity.
//! - Diodes and motors have no single primary scalar to tolerance; naming
//!   one is an error.
//! - Any MC sample whose re-solve fails (singular, non-convergent) aborts
//!   the analysis rather than skewing the yield by dropping the sample.

use vcad_kernel_tolerance::analysis::{rss, worst_case, ProbabilityEstimate, RssAnalysis};
use vcad_kernel_tolerance::dist::{Distribution, SigmaConvention};
use vcad_kernel_tolerance::rng::Rng;
use vcad_kernel_tolerance::stackup::{Contributor, Requirement, Stackup, StackupError};

use super::ac::{ac_response, AcError};
use super::adjoint::{ac_sensitivities, dc_sensitivities, AcSensitivity};
use super::dc::{operating_point, DcError};
use super::{Circuit, Device};

/// The scalar circuit output whose yield is being priced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CircuitOutput {
    /// DC voltage at a non-ground node (V).
    DcNodeVoltage {
        /// Output node id.
        node: usize,
    },
    /// |H(jω)| at one frequency: magnitude of the complex voltage at
    /// `out_node` per unit amplitude of the device `source` (dimensionless
    /// for a V-source drive).
    AcMagnitude {
        /// Driving source device id.
        source: usize,
        /// Output node id.
        out_node: usize,
        /// Probe frequency (Hz).
        freq_hz: f64,
    },
}

/// A ± fractional tolerance on one device's primary scalar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceTolerance {
    /// Device id in the circuit.
    pub device_id: usize,
    /// ± tolerance as a fraction of the nominal (0.01 = ±1%).
    pub tol_frac: f64,
    /// Tolerance-to-σ convention (recorded, never silently defaulted —
    /// same contract as the stackup engine).
    pub convention: SigmaConvention,
}

impl DeviceTolerance {
    /// ±`tol_frac` under the ±tol = 3σ convention.
    pub fn three_sigma(device_id: usize, tol_frac: f64) -> Self {
        DeviceTolerance {
            device_id,
            tol_frac,
            convention: SigmaConvention::ThreeSigma,
        }
    }
}

/// The spec window on the output. At least one bound is required
/// (fail-closed, inherited from the stackup [`Requirement`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpecWindow {
    /// Minimum acceptable output.
    pub lower: Option<f64>,
    /// Maximum acceptable output.
    pub upper: Option<f64>,
}

impl SpecWindow {
    /// Two-sided window.
    pub fn between(lower: f64, upper: f64) -> Self {
        SpecWindow {
            lower: Some(lower),
            upper: Some(upper),
        }
    }

    fn contains(&self, y: f64) -> bool {
        self.lower.is_none_or(|l| y >= l) && self.upper.is_none_or(|u| y <= u)
    }
}

/// Monte Carlo options for the solver-in-the-loop yield check.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct McOptions {
    /// Number of virtual circuits to build and re-solve.
    pub n: usize,
    /// PRNG seed (xoshiro256++). Same seed + same circuit = bit-identical.
    pub seed: u64,
}

impl Default for McOptions {
    fn default() -> Self {
        McOptions {
            n: 20_000,
            seed: 0x5EED_C1AC,
        }
    }
}

/// Solver-in-the-loop Monte Carlo yield, with the linearization checked
/// against the full re-solves.
#[derive(Debug, Clone, PartialEq)]
pub struct McYield {
    /// Sample count.
    pub n: usize,
    /// PRNG seed — reproducibility provenance.
    pub seed: u64,
    /// In-window probability with Agresti–Coull standard error.
    pub yield_est: ProbabilityEstimate,
    /// Sample mean of the full-solve output.
    pub mean: f64,
    /// Bessel-corrected sample σ of the full-solve output.
    pub sigma: f64,
    /// Smallest / largest full-solve output sampled.
    pub min: f64,
    /// Largest full-solve output sampled.
    pub max: f64,
    /// Max |y_full − y_linear| over the samples — the honesty number.
    pub lin_err_max: f64,
    /// RMS |y_full − y_linear| over the samples.
    pub lin_err_rms: f64,
}

/// The full tolerance-yield analysis of one circuit output.
#[derive(Debug, Clone, PartialEq)]
pub struct ToleranceAnalysis {
    /// Nominal output y₀ (all devices at nominal).
    pub nominal_output: f64,
    /// dy/dpᵢ per toleranced device, in `tolerances` order (adjoint-exact).
    pub gradient: Vec<f64>,
    /// Linearized worst-case output bounds (every toleranced device at its
    /// worst drawing limit simultaneously): (min, max).
    pub worst_case_output: (f64, f64),
    /// Largest linearized worst-case deviation from nominal:
    /// max(y₀ − min, max − y₀).
    pub worst_case_deviation: f64,
    /// RSS rollup of the linearized stackup (σ, Φ-yield, Cp/Cpk) — the
    /// `mean_gap` field is in gap space; add `gap_offset` for output space.
    pub rss: RssAnalysis,
    /// Solver-in-the-loop Monte Carlo (the check on everything above).
    pub mc: McYield,
    /// Toleranced device ids whose adjoint gradient is exactly zero: they
    /// are excluded from the linearized stackup (the stackup engine rejects
    /// dead contributors) but still perturbed in the MC, where the full
    /// solver decides whether they matter.
    pub zero_gradient: Vec<usize>,
    /// The linearized stackup handed to the kernel-tolerance machinery, in
    /// gap space (gap = Σ aᵢxᵢ = y − `gap_offset`). Reusable for
    /// sensitivities or allocation.
    pub stackup: Stackup,
    /// y = gap + `gap_offset` (the constant part of the linearization).
    pub gap_offset: f64,
    /// The spec window the yields were computed against.
    pub spec: SpecWindow,
}

/// Everything that can refuse.
#[derive(Debug, Clone, PartialEq)]
pub enum ToleranceError {
    /// DC solve failed.
    Dc(DcError),
    /// AC solve failed.
    Ac(AcError),
    /// The linearized stackup failed kernel-tolerance validation.
    Stackup(StackupError),
    /// No device tolerances were given.
    NoTolerances,
    /// A device id is not in the circuit.
    UnknownDevice(usize),
    /// A device id appears twice in the tolerance list.
    DuplicateDevice(usize),
    /// A tolerance fraction is non-finite or not positive.
    BadTolerance(usize),
    /// The device has no single primary scalar to tolerance (diode, motor),
    /// or its nominal is zero so a fractional tolerance is meaningless.
    NotTolerancable(usize),
    /// The device's AC sensitivity slot is a deferred placeholder (see
    /// [`AcSensitivity::deferred`]); treating it as zero would be a silent
    /// lie, so the analysis refuses.
    DeferredSensitivity(usize),
    /// Every toleranced device has zero adjoint gradient — the linearized
    /// analyses would be vacuous.
    AllZeroGradient,
    /// A Monte Carlo re-solve failed at this sample index; the yield would
    /// be biased by dropping it, so the analysis fails closed.
    McSolveFailed(usize),
}

impl std::fmt::Display for ToleranceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToleranceError::Dc(e) => write!(f, "DC solve failed: {e:?}"),
            ToleranceError::Ac(e) => write!(f, "AC solve failed: {e:?}"),
            ToleranceError::Stackup(e) => write!(f, "stackup validation failed: {e}"),
            ToleranceError::NoTolerances => write!(f, "no device tolerances given"),
            ToleranceError::UnknownDevice(id) => write!(f, "no device with id {id}"),
            ToleranceError::DuplicateDevice(id) => write!(f, "device {id} toleranced twice"),
            ToleranceError::BadTolerance(id) => {
                write!(f, "device {id}: tolerance fraction must be finite and > 0")
            }
            ToleranceError::NotTolerancable(id) => {
                write!(f, "device {id} has no positive primary scalar to tolerance")
            }
            ToleranceError::DeferredSensitivity(id) => write!(
                f,
                "device {id}'s AC sensitivity is a deferred placeholder (not computed at M0); \
                 refusing to treat it as zero"
            ),
            ToleranceError::AllZeroGradient => {
                write!(f, "every toleranced device has zero output sensitivity")
            }
            ToleranceError::McSolveFailed(i) => {
                write!(f, "Monte Carlo re-solve failed at sample {i}")
            }
        }
    }
}

impl std::error::Error for ToleranceError {}

impl From<StackupError> for ToleranceError {
    fn from(e: StackupError) -> Self {
        ToleranceError::Stackup(e)
    }
}

/// Solve the output at nominal and return (y₀, dy/dp per device id).
fn output_and_gradient(
    circuit: &Circuit,
    output: CircuitOutput,
) -> Result<(f64, Vec<f64>, Option<AcSensitivity>), ToleranceError> {
    match output {
        CircuitOutput::DcNodeVoltage { node } => {
            let s = dc_sensitivities(circuit, node).map_err(ToleranceError::Dc)?;
            Ok((s.value, s.gradient, None))
        }
        CircuitOutput::AcMagnitude {
            source,
            out_node,
            freq_hz,
        } => {
            let omega = 2.0 * std::f64::consts::PI * freq_hz;
            let s =
                ac_sensitivities(circuit, source, omega, out_node).map_err(ToleranceError::Ac)?;
            let grad = (0..circuit.devices.len())
                .map(|i| s.d_magnitude(i))
                .collect();
            Ok((s.h.abs(), grad, Some(s)))
        }
    }
}

/// Re-solve the full (non-linearized) output for a perturbed circuit.
fn solve_output(circuit: &Circuit, output: CircuitOutput) -> Result<f64, ()> {
    match output {
        CircuitOutput::DcNodeVoltage { node } => operating_point(circuit)
            .map(|s| s.node_voltages[node])
            .map_err(|_| ()),
        CircuitOutput::AcMagnitude {
            source,
            out_node,
            freq_hz,
        } => {
            let omega = 2.0 * std::f64::consts::PI * freq_hz;
            ac_response(circuit, source, omega)
                .map(|s| s.node_voltages[out_node].abs())
                .map_err(|_| ())
        }
    }
}

fn contributor_name(circuit: &Circuit, id: usize) -> String {
    let kind = match circuit.devices[id] {
        Device::Resistor { .. } => "R",
        Device::Capacitor { .. } => "C",
        Device::Inductor { .. } => "L",
        Device::VSource { .. } => "V",
        Device::ISource { .. } => "I",
        Device::Diode { .. } => "D",
        Device::Motor { .. } => "M",
        Device::Mosfet { .. } => "Q",
        Device::Bjt { .. } => "Q",
    };
    format!("{kind}{id}")
}

/// Validate the tolerance list against the circuit; returns per-device
/// nominals in list order.
fn validate_tolerances(
    circuit: &Circuit,
    tolerances: &[DeviceTolerance],
    ac: Option<&AcSensitivity>,
) -> Result<Vec<f64>, ToleranceError> {
    if tolerances.is_empty() {
        return Err(ToleranceError::NoTolerances);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut nominals = Vec::with_capacity(tolerances.len());
    for t in tolerances {
        let dev = circuit
            .devices
            .get(t.device_id)
            .ok_or(ToleranceError::UnknownDevice(t.device_id))?;
        if !seen.insert(t.device_id) {
            return Err(ToleranceError::DuplicateDevice(t.device_id));
        }
        if !t.tol_frac.is_finite() || t.tol_frac <= 0.0 {
            return Err(ToleranceError::BadTolerance(t.device_id));
        }
        if matches!(
            dev,
            Device::Diode { .. }
                | Device::Motor { .. }
                | Device::Mosfet { .. }
                | Device::Bjt { .. }
        ) || dev.primary() == 0.0
        {
            return Err(ToleranceError::NotTolerancable(t.device_id));
        }
        if let Some(s) = ac {
            if s.is_deferred(t.device_id) {
                return Err(ToleranceError::DeferredSensitivity(t.device_id));
            }
        }
        nominals.push(dev.primary());
    }
    Ok(nominals)
}

/// Build the linearized stackup in gap space. Zero-gradient devices are
/// excluded (returned separately); the gap offset makes y = gap + offset.
fn build_stackup(
    circuit: &Circuit,
    tolerances: &[DeviceTolerance],
    nominals: &[f64],
    gradient_full: &[f64],
    y0: f64,
    spec: SpecWindow,
) -> Result<(Stackup, f64, Vec<usize>), ToleranceError> {
    let mut contributors = Vec::new();
    let mut zero_gradient = Vec::new();
    let mut linear_at_nominal = 0.0;
    for (t, &p) in tolerances.iter().zip(nominals) {
        let g = gradient_full[t.device_id];
        if g == 0.0 {
            zero_gradient.push(t.device_id);
            continue;
        }
        let tol = t.tol_frac * p.abs();
        contributors.push(Contributor {
            name: contributor_name(circuit, t.device_id),
            coeff: g,
            nominal: p,
            tol_minus: tol,
            tol_plus: tol,
            dist: Distribution::Normal {
                mean: 0.0,
                sigma: tol / t.convention.k(),
            },
            source: vcad_kernel_tolerance::dist::DistributionSource::Assumed {
                convention: t.convention,
            },
        });
        linear_at_nominal += g * p;
    }
    if contributors.is_empty() {
        return Err(ToleranceError::AllZeroGradient);
    }
    let gap_offset = y0 - linear_at_nominal;
    let stackup = Stackup {
        name: "circuit output".into(),
        contributors,
        requirement: Requirement {
            name: "spec window".into(),
            lower_mm: spec.lower.map(|l| l - gap_offset),
            upper_mm: spec.upper.map(|u| u - gap_offset),
        },
    };
    stackup.validate()?;
    Ok((stackup, gap_offset, zero_gradient))
}

/// Full tolerance-yield analysis: adjoint-linearized worst case and RSS via
/// the kernel-tolerance stackup engine, checked by a seeded Monte Carlo that
/// re-runs the actual solver on every sample.
pub fn analyze(
    circuit: &Circuit,
    output: CircuitOutput,
    tolerances: &[DeviceTolerance],
    spec: SpecWindow,
    mc: McOptions,
) -> Result<ToleranceAnalysis, ToleranceError> {
    let (y0, gradient_full, ac) = output_and_gradient(circuit, output)?;
    let nominals = validate_tolerances(circuit, tolerances, ac.as_ref())?;
    let (stackup, gap_offset, zero_gradient) =
        build_stackup(circuit, tolerances, &nominals, &gradient_full, y0, spec)?;

    let wc = worst_case(&stackup)?;
    let rss_a = rss(&stackup)?;
    let wc_min = wc.min_gap + gap_offset;
    let wc_max = wc.max_gap + gap_offset;
    let worst_case_deviation = (y0 - wc_min).max(wc_max - y0);

    // Monte Carlo: perturb every toleranced device (zero-gradient ones
    // included — the full solver, not the linearization, decides), re-solve
    // the actual network, and compare against the linear prediction.
    let mut rng = Rng::new(mc.seed);
    let sigmas: Vec<f64> = tolerances
        .iter()
        .zip(&nominals)
        .map(|(t, &p)| t.tol_frac * p.abs() / t.convention.k())
        .collect();
    let mut mean = 0.0f64;
    let mut m2 = 0.0f64;
    let (mut min_s, mut max_s) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut fits = 0usize;
    let mut err_max = 0.0f64;
    let mut err_sq_sum = 0.0f64;
    for i in 0..mc.n {
        let mut perturbed = circuit.clone();
        let mut y_lin = y0;
        for ((t, &p), &s) in tolerances.iter().zip(&nominals).zip(&sigmas) {
            let dev = s * rng.next_normal();
            perturbed.devices[t.device_id].set_primary(p + dev);
            y_lin += gradient_full[t.device_id] * dev;
        }
        let y = solve_output(&perturbed, output).map_err(|_| ToleranceError::McSolveFailed(i))?;
        let k = (i + 1) as f64;
        let delta = y - mean;
        mean += delta / k;
        m2 += delta * (y - mean);
        min_s = min_s.min(y);
        max_s = max_s.max(y);
        if spec.contains(y) {
            fits += 1;
        }
        let e = (y - y_lin).abs();
        err_max = err_max.max(e);
        err_sq_sum += e * e;
    }
    let n_f = mc.n as f64;
    let sigma_mc = (m2 / (n_f - 1.0)).sqrt();

    Ok(ToleranceAnalysis {
        nominal_output: y0,
        gradient: tolerances
            .iter()
            .map(|t| gradient_full[t.device_id])
            .collect(),
        worst_case_output: (wc_min, wc_max),
        worst_case_deviation,
        rss: rss_a,
        mc: McYield {
            n: mc.n,
            seed: mc.seed,
            yield_est: ProbabilityEstimate::from_counts(fits, mc.n),
            mean,
            sigma: sigma_mc,
            min: min_s,
            max: max_s,
            lin_err_max: err_max,
            lin_err_rms: (err_sq_sum / n_f).sqrt(),
        },
        zero_gradient,
        stackup,
        gap_offset,
        spec,
    })
}

/// One allocatable device for [`allocate_tolerances`]: its cost curve and
/// the fractional-tolerance box the process allows.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceAllocation {
    /// Device id in the circuit.
    pub device_id: usize,
    /// Cost vs ± *fractional* tolerance (so a 1%-vs-10% resistor price list
    /// plugs in directly; converted to absolute units internally).
    pub cost: vcad_kernel_tolerance::allocate::CostModel,
    /// Tightest fractional tolerance the part family offers (> 0).
    pub tol_frac_min: f64,
    /// Loosest fractional tolerance allowed (≥ min).
    pub tol_frac_max: f64,
}

/// The allocation answer: which device must be the expensive tight part.
#[derive(Debug, Clone, PartialEq)]
pub struct AllocationOutcome {
    /// (device_id, allocated ± fraction, cost) in input order.
    pub allocations: Vec<(usize, f64, f64)>,
    /// Total cost at the allocation.
    pub cost: f64,
    /// Cost of the proportional one-knob baseline (see the kernel allocator).
    pub cost_proportional_baseline: f64,
    /// RSS yield of the allocated circuit (≥ target when feasible).
    pub predicted_yield: f64,
    /// The requested yield floor.
    pub target_yield: f64,
    /// Per-device share of the allocated output variance
    /// (aᵢ²σᵢ² / σ_G²), same order as `allocations` — the dominance table.
    pub variance_share: Vec<f64>,
}

/// Min-cost tolerance allocation on the linearized circuit response:
/// which device must be the expensive tight part to hit `target_yield`?
///
/// Reuses the kernel-tolerance KKT allocator verbatim; this wrapper only
/// converts fractional boxes/costs to the stackup's absolute units and back.
/// The yield is the RSS (linearized) yield — check the answer with
/// [`analyze`]'s solver-in-the-loop MC before buying parts.
pub fn allocate_tolerances(
    circuit: &Circuit,
    output: CircuitOutput,
    tolerances: &[DeviceTolerance],
    spec: SpecWindow,
    vars: &[DeviceAllocation],
    target_yield: f64,
) -> Result<AllocationOutcome, ToleranceError> {
    use vcad_kernel_tolerance::allocate::{allocate, AllocationVar, CostModel};

    let (y0, gradient_full, ac) = output_and_gradient(circuit, output)?;
    let nominals = validate_tolerances(circuit, tolerances, ac.as_ref())?;
    let (stackup, _offset, _zero) =
        build_stackup(circuit, tolerances, &nominals, &gradient_full, y0, spec)?;

    // Map fractional boxes and cost curves into the stackup's absolute
    // units: t_abs = frac·|p|, and C(t_abs) = C_frac(t_abs/|p|) by scaling
    // the model coefficients.
    let mut kernel_vars = Vec::with_capacity(vars.len());
    for v in vars {
        let idx = tolerances
            .iter()
            .position(|t| t.device_id == v.device_id)
            .ok_or(ToleranceError::UnknownDevice(v.device_id))?;
        let p = nominals[idx].abs();
        let cost = match v.cost {
            CostModel::Reciprocal { a, b } => CostModel::Reciprocal { a, b: b * p },
            CostModel::ReciprocalSquared { a, b } => {
                CostModel::ReciprocalSquared { a, b: b * p * p }
            }
            CostModel::Exponential { a, b, tau } => CostModel::Exponential { a, b, tau: tau * p },
        };
        kernel_vars.push(AllocationVar {
            contributor: contributor_name(circuit, v.device_id),
            cost,
            t_min: v.tol_frac_min * p,
            t_max: v.tol_frac_max * p,
        });
    }

    let r = allocate(&stackup, &kernel_vars, target_yield)?;

    let mut allocations = Vec::with_capacity(vars.len());
    let sigma_sq = r.sigma_gap * r.sigma_gap;
    let mut variance_share = Vec::with_capacity(vars.len());
    for (v, (name, t_abs, cost)) in vars.iter().zip(&r.tolerances) {
        let idx = tolerances
            .iter()
            .position(|t| t.device_id == v.device_id)
            .expect("validated above");
        let p = nominals[idx].abs();
        allocations.push((v.device_id, t_abs / p, *cost));
        let c = r
            .stackup
            .contributors
            .iter()
            .find(|c| &c.name == name)
            .expect("allocated contributor exists");
        variance_share.push(c.coeff * c.coeff * c.dist.variance() / sigma_sq);
    }

    Ok(AllocationOutcome {
        allocations,
        cost: r.cost,
        cost_proportional_baseline: r.cost_proportional_baseline,
        predicted_yield: r.predicted_yield,
        target_yield,
        variance_share,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::DiodeModel;

    /// 10 V divider: R1 = 3 kΩ over R2 = 1 kΩ, Vout = 2.5 V.
    fn divider() -> (Circuit, usize, usize, usize) {
        let mut c = Circuit::new();
        let vin = c.node();
        let out = c.node();
        c.add(Device::VSource {
            p: vin,
            n: 0,
            v: 10.0,
        });
        let r1 = c.add(Device::Resistor {
            p: vin,
            n: out,
            r: 3_000.0,
        });
        let r2 = c.add(Device::Resistor {
            p: out,
            n: 0,
            r: 1_000.0,
        });
        (c, r1, r2, out)
    }

    /// Butterworth 10 kHz low-pass (Q = 1/√2): vin —R— mid —L— out —C— gnd.
    fn butterworth() -> (Circuit, usize, f64) {
        let f0 = 10_000.0;
        let l = 1e-3;
        let c_val = 1.0 / ((2.0 * std::f64::consts::PI * f0).powi(2) * l);
        let r = std::f64::consts::SQRT_2 * (l / c_val).sqrt();
        let mut ckt = Circuit::new();
        let vin = ckt.node();
        let mid = ckt.node();
        let out = ckt.node();
        let src = ckt.add(Device::VSource {
            p: vin,
            n: 0,
            v: 0.0,
        });
        ckt.add(Device::Resistor { p: vin, n: mid, r });
        ckt.add(Device::Inductor { p: mid, n: out, l });
        ckt.add(Device::Capacitor {
            p: out,
            n: 0,
            c: c_val,
        });
        let _ = src;
        (ckt, out, f0)
    }

    fn butterworth_output(out: usize, f0: f64) -> CircuitOutput {
        CircuitOutput::AcMagnitude {
            source: 0,
            out_node: out,
            freq_hz: f0,
        }
    }

    #[test]
    fn divider_wc_and_rss_match_hand_closed_form() {
        // Vout = V·R2/(R1+R2); dV/dR1 = −V·R2/(R1+R2)², dV/dR2 = V·R1/(R1+R2)².
        let (c, r1, r2, out) = divider();
        let (v, ra, rb) = (10.0, 3_000.0, 1_000.0);
        let s = ra + rb;
        let g1 = -v * rb / (s * s);
        let g2 = v * ra / (s * s);

        let tols = [
            DeviceTolerance::three_sigma(r1, 0.01),
            DeviceTolerance::three_sigma(r2, 0.01),
        ];
        let a = analyze(
            &c,
            CircuitOutput::DcNodeVoltage { node: out },
            &tols,
            SpecWindow::between(2.45, 2.55),
            McOptions {
                n: 5_000,
                ..Default::default()
            },
        )
        .unwrap();

        assert!((a.nominal_output - 2.5).abs() < 1e-12);
        assert!((a.gradient[0] - g1).abs() < 1e-12 * g1.abs());
        assert!((a.gradient[1] - g2).abs() < 1e-12 * g2.abs());

        // Linearized WC: y0 ± (|g1|·0.01·R1 + |g2|·0.01·R2), exact.
        let half_width = g1.abs() * 0.01 * ra + g2.abs() * 0.01 * rb;
        assert!((a.worst_case_output.0 - (2.5 - half_width)).abs() < 1e-12);
        assert!((a.worst_case_output.1 - (2.5 + half_width)).abs() < 1e-12);
        assert!((a.worst_case_deviation - half_width).abs() < 1e-12);

        // RSS σ: √(g1²σ1² + g2²σ2²) with σ = tol/3, exact.
        let sig = ((g1 * 0.01 * ra / 3.0).powi(2) + (g2 * 0.01 * rb / 3.0).powi(2)).sqrt();
        assert!((a.rss.sigma_gap - sig).abs() < 1e-12 * sig);

        // A divider is mildly nonlinear in R; at ±1% the linearization is
        // near-exact and the MC agrees tightly.
        assert!(a.mc.lin_err_max < 1e-3, "{}", a.mc.lin_err_max);
        assert!((a.mc.sigma - sig).abs() / sig < 0.05);
        assert!(a.zero_gradient.is_empty());
    }

    #[test]
    fn rlc_linearized_sigma_agrees_with_mc_until_it_honestly_does_not() {
        // At the Butterworth cutoff |H(f₀)| = 1/(ω₀CR): mildly nonlinear,
        // and the linearization holds surprisingly far (the flagship example
        // shows σ agreement even at ±10%). A *high-Q* filter probed at its
        // resonance is a different animal: an L or C shift moves the peak
        // off the probe frequency and |H| collapses nonlinearly. That is
        // where the linearized σ must honestly break — a negative result
        // worth asserting.
        let f0 = 10_000.0;
        let l = 1e-3;
        let c_val = 1.0 / ((2.0 * std::f64::consts::PI * f0).powi(2) * l);
        let q = 5.0;
        let r = (l / c_val).sqrt() / q;
        let mut ckt = Circuit::new();
        let vin = ckt.node();
        let mid = ckt.node();
        let out = ckt.node();
        ckt.add(Device::VSource {
            p: vin,
            n: 0,
            v: 0.0,
        });
        ckt.add(Device::Resistor { p: vin, n: mid, r });
        ckt.add(Device::Inductor { p: mid, n: out, l });
        ckt.add(Device::Capacitor {
            p: out,
            n: 0,
            c: c_val,
        });
        let spec = SpecWindow::between(0.9 * q, 1.1 * q);
        let run = |tol: f64| {
            let tols = [
                DeviceTolerance::three_sigma(1, tol),
                DeviceTolerance::three_sigma(2, tol),
                DeviceTolerance::three_sigma(3, tol),
            ];
            analyze(
                &ckt,
                butterworth_output(out, f0),
                &tols,
                spec,
                McOptions { n: 20_000, seed: 7 },
            )
            .unwrap()
        };

        // Stated bound: σ_lin vs σ_MC within 5% at part tolerances up to
        // ±5% (measured: 0.04%, 0.13%, 4.5% at ±0.5/1/5%).
        let a1 = run(0.01);
        let rel1 = (a1.rss.sigma_gap - a1.mc.sigma).abs() / a1.mc.sigma;
        assert!(rel1 < 0.05, "±1%: σ_lin vs σ_MC rel err {rel1}");
        let a5 = run(0.05);
        let rel5 = (a5.rss.sigma_gap - a5.mc.sigma).abs() / a5.mc.sigma;
        assert!(rel5 < 0.05, "±5%: σ_lin vs σ_MC rel err {rel5}");

        // ±20%: nonlinearity bites — the 5% bound must FAIL by a wide
        // margin (measured 18%), and the reported linearization error is no
        // longer small next to σ (measured lin_err_max ≈ 6σ).
        let a20 = run(0.20);
        let rel20 = (a20.rss.sigma_gap - a20.mc.sigma).abs() / a20.mc.sigma;
        assert!(
            rel20 > 0.10,
            "±20% should break the linearization: rel err only {rel20}"
        );
        assert!(
            a20.mc.lin_err_max > 0.5 * a20.mc.sigma,
            "lin_err_max {} should be comparable to σ {}",
            a20.mc.lin_err_max,
            a20.mc.sigma
        );
        // And the honesty numbers grow monotonically with tolerance.
        assert!(a1.mc.lin_err_rms < a5.mc.lin_err_rms);
        assert!(a5.mc.lin_err_rms < a20.mc.lin_err_rms);
    }

    #[test]
    fn yield_decreases_monotonically_as_tolerances_widen() {
        let (ckt, out, f0) = butterworth();
        // ±4% window around |H(f₀)| = 1/√2, same as the flagship example.
        let nom = std::f64::consts::FRAC_1_SQRT_2;
        let spec = SpecWindow::between(nom * 0.96, nom * 1.04);
        let mut prev_mc = f64::INFINITY;
        let mut prev_rss = f64::INFINITY;
        for tol in [0.01, 0.02, 0.05, 0.10] {
            let tols = [
                DeviceTolerance::three_sigma(1, tol),
                DeviceTolerance::three_sigma(2, tol),
                DeviceTolerance::three_sigma(3, tol),
            ];
            let a = analyze(
                &ckt,
                butterworth_output(out, f0),
                &tols,
                spec,
                McOptions {
                    n: 20_000,
                    seed: 11,
                },
            )
            .unwrap();
            assert!(
                a.mc.yield_est.p <= prev_mc,
                "MC yield must not increase: {} after {}",
                a.mc.yield_est.p,
                prev_mc
            );
            assert!(a.rss.yield_estimate < prev_rss);
            prev_mc = a.mc.yield_est.p;
            prev_rss = a.rss.yield_estimate;
        }
        assert!(prev_mc < 0.9, "±10% on a resonant spec should hurt");
    }

    #[test]
    fn deferred_ac_diode_sensitivity_is_refused_not_zeroed() {
        let mut c = Circuit::new();
        let vin = c.node();
        let out = c.node();
        c.add(Device::VSource {
            p: vin,
            n: 0,
            v: 0.0,
        });
        let rid = c.add(Device::Resistor {
            p: vin,
            n: out,
            r: 1_000.0,
        });
        let did = c.add(Device::Diode {
            p: out,
            n: 0,
            model: DiodeModel::silicon(),
        });
        let output = CircuitOutput::AcMagnitude {
            source: 0,
            out_node: out,
            freq_hz: 1_000.0,
        };
        // Tolerancing the diode: refused (deferred slot / no primary).
        let err = analyze(
            &c,
            output,
            &[DeviceTolerance::three_sigma(did, 0.05)],
            SpecWindow::between(0.0, 1.0),
            McOptions {
                n: 200,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                ToleranceError::NotTolerancable(id) | ToleranceError::DeferredSensitivity(id)
                    if id == did
            ),
            "got {err:?}"
        );
        // Tolerancing only the resistor is fine — the diode stays nominal.
        analyze(
            &c,
            output,
            &[DeviceTolerance::three_sigma(rid, 0.05)],
            SpecWindow::between(0.0, 1.0),
            McOptions {
                n: 200,
                ..Default::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn fail_closed_validation() {
        let (c, r1, _r2, out) = divider();
        let output = CircuitOutput::DcNodeVoltage { node: out };
        let spec = SpecWindow::between(2.4, 2.6);
        let mc = McOptions {
            n: 100,
            ..Default::default()
        };
        assert_eq!(
            analyze(&c, output, &[], spec, mc).unwrap_err(),
            ToleranceError::NoTolerances
        );
        assert_eq!(
            analyze(
                &c,
                output,
                &[DeviceTolerance::three_sigma(99, 0.01)],
                spec,
                mc
            )
            .unwrap_err(),
            ToleranceError::UnknownDevice(99)
        );
        assert_eq!(
            analyze(
                &c,
                output,
                &[
                    DeviceTolerance::three_sigma(r1, 0.01),
                    DeviceTolerance::three_sigma(r1, 0.02)
                ],
                spec,
                mc
            )
            .unwrap_err(),
            ToleranceError::DuplicateDevice(r1)
        );
        assert_eq!(
            analyze(
                &c,
                output,
                &[DeviceTolerance::three_sigma(r1, -0.01)],
                spec,
                mc
            )
            .unwrap_err(),
            ToleranceError::BadTolerance(r1)
        );
    }

    #[test]
    fn allocation_finds_the_dominant_device_on_the_butterworth() {
        // Which component must be the expensive 1% part? The variance-share
        // table must point at the same device the raw sensitivities do.
        use vcad_kernel_tolerance::allocate::CostModel;
        let (ckt, out, f0) = butterworth();
        let output = butterworth_output(out, f0);
        let tols = [
            DeviceTolerance::three_sigma(1, 0.05),
            DeviceTolerance::three_sigma(2, 0.05),
            DeviceTolerance::three_sigma(3, 0.05),
        ];
        let spec = SpecWindow::between(0.68, 0.735);
        let vars: Vec<DeviceAllocation> = [1usize, 2, 3]
            .iter()
            .map(|&id| DeviceAllocation {
                device_id: id,
                cost: CostModel::Reciprocal { a: 0.02, b: 0.001 },
                tol_frac_min: 0.001,
                tol_frac_max: 0.20,
            })
            .collect();
        let r = allocate_tolerances(&ckt, output, &tols, spec, &vars, 0.99).unwrap();
        assert!(r.predicted_yield >= 0.99 - 1e-9);
        // Equal cost curves ⇒ equal fractional sensitivity |g·p| pulls the
        // allocation; shares sum to ~1 over the allocated devices.
        let share_sum: f64 = r.variance_share.iter().sum();
        assert!((share_sum - 1.0).abs() < 1e-6, "shares sum to {share_sum}");
        // Every allocation is inside its box and priced.
        for &(_, frac, cost) in &r.allocations {
            assert!((0.001..=0.20).contains(&frac));
            assert!(cost > 0.0);
        }
        // Optimizer never loses to the proportional one-knob baseline.
        assert!(r.cost <= r.cost_proportional_baseline + 1e-9);

        // Cross-check the allocated tolerances with the full solver MC.
        let alloc_tols: Vec<DeviceTolerance> = r
            .allocations
            .iter()
            .map(|&(id, frac, _)| DeviceTolerance::three_sigma(id, frac))
            .collect();
        let check = analyze(
            &ckt,
            output,
            &alloc_tols,
            spec,
            McOptions { n: 20_000, seed: 3 },
        )
        .unwrap();
        // Solver-in-the-loop yield confirms the linearized promise within
        // a few standard errors plus linearization slack.
        assert!(
            check.mc.yield_est.p > 0.97,
            "MC yield {} too far below the 0.99 RSS promise",
            check.mc.yield_est.p
        );
    }

    #[test]
    fn mc_is_seeded_and_reproducible() {
        let (c, r1, r2, out) = divider();
        let tols = [
            DeviceTolerance::three_sigma(r1, 0.01),
            DeviceTolerance::three_sigma(r2, 0.01),
        ];
        let run = |seed| {
            analyze(
                &c,
                CircuitOutput::DcNodeVoltage { node: out },
                &tols,
                SpecWindow::between(2.45, 2.55),
                McOptions { n: 1_000, seed },
            )
            .unwrap()
        };
        let a = run(42);
        let b = run(42);
        assert_eq!(a, b, "same seed must be bit-identical");
        let c2 = run(43);
        assert_ne!(a.mc.mean.to_bits(), c2.mc.mean.to_bits());
    }
}
