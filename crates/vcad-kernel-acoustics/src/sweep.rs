//! Frequency sweeps and resonance extraction.
//!
//! Drive the cavity at a source, sweep frequency, and read the response at a
//! probe point — a swept-sine measurement, in silico. Resonances are the
//! peaks of `|p|`, refined by parabolic interpolation on log-magnitude (the
//! same sub-sample peak trick the glockenspiel's FFT verdict path uses). This
//! *is* how the field solver reports mode frequencies: no eigen-decomposition,
//! the same procedure a bench measurement runs.

use crate::cavity::Cavity;
use crate::complex::Cplx;
use crate::helmholtz::{solve_driven, Source};

/// One sample of a swept response.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepPoint {
    /// Frequency, Hz.
    pub f_hz: f64,
    /// Response magnitude `|p|` at the probe, Pa (`NaN` if the solve failed
    /// even after nudging off an exact pole).
    pub value: f64,
}

/// A detected resonance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resonance {
    /// Interpolated peak frequency, Hz.
    pub f_hz: f64,
    /// Response magnitude at the peak, Pa.
    pub value: f64,
}

/// A probe location, millimeters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probe {
    /// Radial position, mm.
    pub r_mm: f64,
    /// Axial position, mm.
    pub z_mm: f64,
}

/// Sweep `n` linearly spaced frequencies in `[f_lo, f_hi]`, recording `|p|`
/// at `probe`. Exact poles (the undamped operator is singular at a
/// resonance) are nudged by a few parts in ten-thousand rather than left as
/// `NaN`, so a peak never falls into a hole.
#[allow(clippy::too_many_arguments)]
pub fn frequency_sweep(
    cavity: &Cavity,
    nr: usize,
    nz: usize,
    source: Source,
    probe: Probe,
    f_lo: f64,
    f_hi: f64,
    n: usize,
) -> Vec<SweepPoint> {
    assert!(n >= 2 && f_hi > f_lo && f_lo > 0.0);
    (0..n)
        .map(|i| {
            let f = f_lo + (f_hi - f_lo) * i as f64 / (n - 1) as f64;
            let value = probe_magnitude(cavity, nr, nz, source, probe, f);
            SweepPoint { f_hz: f, value }
        })
        .collect()
}

/// `|p|` at the probe, nudging off an exact pole if the direct solve is
/// singular.
fn probe_magnitude(
    cavity: &Cavity,
    nr: usize,
    nz: usize,
    source: Source,
    probe: Probe,
    f_hz: f64,
) -> f64 {
    for attempt in 0..3 {
        let f = f_hz * (1.0 + attempt as f64 * 3e-4);
        if let Ok(field) = solve_driven(cavity, nr, nz, f, source) {
            return field.magnitude_at(probe.r_mm, probe.z_mm);
        }
    }
    f64::NAN
}

/// Complex probe response at a single frequency (for reciprocity checks and
/// on-axis response).
pub fn probe_response(
    cavity: &Cavity,
    nr: usize,
    nz: usize,
    source: Source,
    probe: Probe,
    f_hz: f64,
) -> Option<Cplx> {
    solve_driven(cavity, nr, nz, f_hz, source)
        .ok()
        .map(|field| field.pressure_at(probe.r_mm, probe.z_mm))
}

/// Extract up to `max_peaks` resonances from a sweep: interior local maxima,
/// refined by parabolic interpolation on `ln|p|`, sorted by ascending
/// frequency.
pub fn find_peaks(points: &[SweepPoint], max_peaks: usize) -> Vec<Resonance> {
    let mut peaks: Vec<Resonance> = Vec::new();
    for i in 1..points.len().saturating_sub(1) {
        let (a, b, c) = (points[i - 1], points[i], points[i + 1]);
        if !(a.value.is_finite() && b.value.is_finite() && c.value.is_finite()) {
            continue;
        }
        if b.value <= a.value || b.value < c.value {
            continue;
        }
        // Parabolic vertex on log magnitude (sub-sample frequency).
        let la = (a.value.max(1e-300)).ln();
        let lb = (b.value.max(1e-300)).ln();
        let lc = (c.value.max(1e-300)).ln();
        let denom = la - 2.0 * lb + lc;
        let delta = if denom.abs() < 1e-30 {
            0.0
        } else {
            0.5 * (la - lc) / denom
        };
        let df = b.f_hz - a.f_hz;
        peaks.push(Resonance {
            f_hz: b.f_hz + delta * df,
            value: b.value,
        });
    }
    peaks.sort_by(|x, y| y.value.partial_cmp(&x.value).unwrap());
    peaks.truncate(max_peaks);
    peaks.sort_by(|x, y| x.f_hz.partial_cmp(&y.f_hz).unwrap());
    peaks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(f: f64) -> f64 {
        // A synthetic sharp peak at 100 Hz for the peak-picker unit test.
        1.0 / ((f - 100.0).powi(2) + 1.0)
    }

    #[test]
    fn peak_picker_finds_a_synthetic_resonance() {
        let points: Vec<SweepPoint> = (0..201)
            .map(|i| {
                let f = 50.0 + i as f64;
                SweepPoint {
                    f_hz: f,
                    value: tri(f),
                }
            })
            .collect();
        let peaks = find_peaks(&points, 4);
        assert_eq!(peaks.len(), 1);
        assert!(
            (peaks[0].f_hz - 100.0).abs() < 0.5,
            "peak at {}",
            peaks[0].f_hz
        );
    }

    #[test]
    fn peak_picker_ignores_nan_holes() {
        let points = vec![
            SweepPoint {
                f_hz: 1.0,
                value: 1.0,
            },
            SweepPoint {
                f_hz: 2.0,
                value: f64::NAN,
            },
            SweepPoint {
                f_hz: 3.0,
                value: 1.0,
            },
        ];
        assert!(find_peaks(&points, 4).is_empty());
    }
}
