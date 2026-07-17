//! Soft (additive) field sources.
//!
//! A soft source adds `amplitude·profile·s(t)` to the out-of-plane field
//! (Ez in TM, Hz in TE) each step, at the field's native Yee time (E at
//! integer steps, H at half steps). Soft sources are transparent — waves
//! pass through the source cells unscattered — but radiate in both
//! directions; unidirectional total-field/scattered-field injection is
//! the M1 milestone.

use crate::waveform::Waveform;

/// Where a source drives the out-of-plane field.
#[derive(Debug, Clone, PartialEq)]
pub enum SourcePlacement {
    /// A single sample. TM: Ez node `(i·Δ, j·Δ)`. TE: Hz sample
    /// `((i+½)·Δ, (j+½)·Δ)`.
    Point {
        /// Sample index along x.
        i: usize,
        /// Sample index along y.
        j: usize,
    },
    /// A vertical line of samples at column `i`, rows `j0..=j1`, with a
    /// per-row amplitude profile of length `j1 − j0 + 1` (e.g. a slab
    /// eigenmode from [`crate::modes::SlabMode::profile`]).
    ///
    /// Row semantics per polarization: TM drives Ez nodes `y = j·Δ`; TE
    /// drives Hz samples `y = (j+½)·Δ` (and column x = `(i+½)·Δ`).
    VerticalLine {
        /// Sample column.
        i: usize,
        /// First row.
        j0: usize,
        /// Last row (inclusive).
        j1: usize,
        /// Per-row amplitudes, length `j1 − j0 + 1`.
        profile: Vec<f64>,
    },
}

/// A soft source: placement × waveform × overall amplitude.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// Where it drives.
    pub placement: SourcePlacement,
    /// s(t).
    pub waveform: Waveform,
    /// Overall scale (arbitrary units).
    pub amplitude: f64,
}

impl Source {
    /// Point source with unit amplitude.
    pub fn point(i: usize, j: usize, waveform: Waveform) -> Self {
        Self {
            placement: SourcePlacement::Point { i, j },
            waveform,
            amplitude: 1.0,
        }
    }

    /// Uniform vertical line source with unit amplitude.
    pub fn line_uniform(i: usize, j0: usize, j1: usize, waveform: Waveform) -> Self {
        assert!(j1 >= j0);
        Self {
            placement: SourcePlacement::VerticalLine {
                i,
                j0,
                j1,
                profile: vec![1.0; j1 - j0 + 1],
            },
            waveform,
            amplitude: 1.0,
        }
    }

    /// Vertical line source carrying an explicit transverse profile.
    pub fn line_profile(i: usize, j0: usize, profile: Vec<f64>, waveform: Waveform) -> Self {
        assert!(!profile.is_empty());
        let j1 = j0 + profile.len() - 1;
        Self {
            placement: SourcePlacement::VerticalLine { i, j0, j1, profile },
            waveform,
            amplitude: 1.0,
        }
    }

    /// Builder: override the amplitude.
    pub fn with_amplitude(mut self, amplitude: f64) -> Self {
        self.amplitude = amplitude;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_profile_infers_extent() {
        let s = Source::line_profile(4, 10, vec![0.5, 1.0, 0.5], Waveform::gaussian(1.0, 0.2));
        match s.placement {
            SourcePlacement::VerticalLine { j0, j1, .. } => {
                assert_eq!((j0, j1), (10, 12));
            }
            _ => unreachable!(),
        }
    }
}
