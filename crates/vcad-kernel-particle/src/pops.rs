//! POPS readiness: is the well harmonic enough to phase-lock?
//!
//! POPS (the Periodically Oscillating Plasma Sphere) compresses an IEC
//! core by driving the ion cloud on resonance so every ion collapses to
//! center in phase — 10⁴ transient density in 1D PIC (see the research
//! log). The physics gate is a **harmonic** well: only if the restoring
//! force is ∝ r does every ion bounce at the *same* frequency regardless
//! of amplitude, so one drive frequency phase-locks the whole population.
//! Real IEC wells flatten away from center → bounce frequency falls with
//! amplitude → the cloud decoheres and no coherent compression happens.
//!
//! This module measures that directly: trace on-axis ions from a ladder of
//! amplitudes, recover each one's bounce frequency, and report the
//! **harmonicity** = min/max frequency ∈ (0, 1]. 1.0 is a perfect
//! oscillator (POPS-ready); lower is anharmonic. Harmonicity is exactly
//! the figure of merit an electrode optimizer maximizes — "shape the well
//! to be quadratic" — which is a sentence only a differentiable
//! field-solver can act on.
//!
//! Scope: vacuum well (no space charge, which itself flattens the well —
//! an anharmonicity the neutralized-cloud work would add). Single-particle
//! bounce, on the wire-free axis of a ring device. M0 caveats apply.

use crate::device::Device;
use crate::field::FieldMap;
use crate::poisson::{solve, Solution, SolveError, SolveOptions};
use crate::trace::{TraceOptions, Tracer, DEUTERON};

/// The POPS-readiness report for one device.
#[derive(Debug, Clone, PartialEq)]
pub struct HarmonicityReport {
    /// `(launch z in mm, bounce frequency in Hz)` for each amplitude that
    /// produced a clean oscillation.
    pub frequencies: Vec<(f64, f64)>,
    /// min/max bounce frequency across amplitudes ∈ (0, 1]. 1.0 = harmonic
    /// (POPS-ready); lower = anharmonic (decoheres under a single drive).
    pub harmonicity: f64,
    /// Fractional bounce-frequency droop from the smallest to the largest
    /// amplitude, `1 − f(max)/f(min)` — the physical "well flattens with
    /// amplitude" number (positive when it flattens).
    pub droop: f64,
}

/// Bounce frequency of an on-axis ion launched at rest at `z0_m`, Hz, from
/// its core-crossing count (two core entries per oscillation). `None` if
/// the ion did not complete at least one full oscillation (hit a wire,
/// escaped, or censored before two core passes).
pub fn axial_bounce_frequency(
    device: &Device,
    sol: &Solution,
    topts: &TraceOptions,
    z0_m: f64,
) -> Option<f64> {
    let fields = FieldMap::new(device, sol);
    let tracer = Tracer::new(device, &fields, sol, *topts);
    let out = tracer.trace_from(DEUTERON, [0.0, 0.0, z0_m], [0.0, 0.0, 0.0]);
    if out.core_passes < 2 || out.time_s <= 0.0 {
        return None;
    }
    Some(out.core_passes as f64 / (2.0 * out.time_s))
}

/// Measure the well's harmonicity from a ladder of on-axis launch heights
/// (as fractions of the chamber half-height).
pub fn harmonicity(
    device: &Device,
    nr: usize,
    nz: usize,
    sopts: &SolveOptions,
    topts: &TraceOptions,
    amplitude_fractions: &[f64],
) -> Result<HarmonicityReport, SolveError> {
    let sol = solve(device, nr, nz, sopts)?;
    let half_h = device.chamber_half_height_mm * 1e-3;

    let mut frequencies = Vec::new();
    for &frac in amplitude_fractions {
        let z0 = frac * half_h;
        if let Some(f) = axial_bounce_frequency(device, &sol, topts, z0) {
            frequencies.push((z0 * 1e3, f));
        }
    }

    let (harmonicity, droop) = if frequencies.len() >= 2 {
        let fmin = frequencies
            .iter()
            .map(|(_, f)| *f)
            .fold(f64::INFINITY, f64::min);
        let fmax = frequencies.iter().map(|(_, f)| *f).fold(0.0_f64, f64::max);
        // Droop: smallest-amplitude f vs largest-amplitude f.
        let f_small = frequencies.first().map(|(_, f)| *f).unwrap_or(0.0);
        let f_large = frequencies.last().map(|(_, f)| *f).unwrap_or(0.0);
        let droop = if f_small > 0.0 {
            1.0 - f_large / f_small
        } else {
            0.0
        };
        (fmin / fmax.max(1e-30), droop)
    } else {
        (0.0, 0.0)
    };

    Ok(HarmonicityReport {
        frequencies,
        harmonicity,
        droop,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> TraceOptions {
        TraceOptions {
            max_passes: 20,
            time_budget_factor: 30.0,
            ..TraceOptions::default()
        }
    }

    #[test]
    fn well_is_measurably_anharmonic() {
        // A two-ring IEC well flattens with amplitude: harmonicity < 1 and
        // the droop is positive (bigger swings bounce slower).
        let device = Device::shielded_two_ring(120.0, 40.0, 22.0, 4.0, -20_000.0, 0.0);
        let r = harmonicity(
            &device,
            81,
            161,
            &SolveOptions::default(),
            &budget(),
            &[0.2, 0.4, 0.6, 0.8],
        )
        .unwrap();
        assert!(
            r.frequencies.len() >= 2,
            "must measure at least two amplitudes: {:?}",
            r.frequencies
        );
        assert!(
            r.harmonicity > 0.0 && r.harmonicity < 1.0,
            "a real IEC well is anharmonic: {:.3}",
            r.harmonicity
        );
    }

    #[test]
    fn frequencies_are_physical() {
        // Deuteron bouncing in a ~20 kV well over ~10 cm: ~MHz.
        let device = Device::shielded_two_ring(120.0, 40.0, 22.0, 4.0, -20_000.0, 0.0);
        let r = harmonicity(
            &device,
            81,
            161,
            &SolveOptions::default(),
            &budget(),
            &[0.3, 0.6],
        )
        .unwrap();
        for (_, f) in &r.frequencies {
            assert!(
                *f > 1e5 && *f < 1e9,
                "bounce frequency must be ~MHz: {f:.3e}"
            );
        }
    }
}
