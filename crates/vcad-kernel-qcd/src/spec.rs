//! Simulation spec → run → result: the serde seam a future MCP tool
//! (`simulate_lattice_gauge`) will speak.
//!
//! Fail-closed at the boundary: a spec that cannot produce honest
//! statistics (no thermalization, too few measurements for the
//! requested binning, degenerate lattice) is rejected before any
//! sweep runs — there is no "partial" result.

use serde::{Deserialize, Serialize};

use crate::lattice::Lattice;
use crate::rng::Rng;
use crate::stats::{jackknife, Estimate};
use crate::update::{heatbath_sweep, overrelax_sweep};

/// Simulation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimSpec {
    /// Lattice extents `[n₀, n₁, n₂, n₃]`, each ≥ 2.
    pub dims: [usize; 4],
    /// Inverse coupling β = 4/g² (SU(2) Wilson action convention).
    pub beta: f64,
    /// Discarded thermalization sweeps (heatbath+OR compound), ≥ 1.
    pub thermalization_sweeps: usize,
    /// Measured sweeps after thermalization.
    pub measurement_sweeps: usize,
    /// Overrelaxation sweeps interleaved per heatbath sweep.
    pub overrelax_per_heatbath: usize,
    /// Jackknife bin size in measurements.
    pub bin_size: usize,
    /// Largest square Wilson loop extent to measure (`0` = plaquette
    /// only; `r` measures all `W(i,j)` for `i,j ≤ r`).
    pub max_wilson_extent: usize,
    /// RNG seed (runs are bit-reproducible per seed).
    pub seed: u64,
    /// Start from a hot (random) configuration instead of cold.
    pub hot_start: bool,
}

/// One measured Wilson loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WilsonLoop {
    /// Spatial extent (lattice units).
    pub r: usize,
    /// Temporal extent (lattice units).
    pub t: usize,
    /// `W(r,t)` estimate.
    pub value: Estimate,
}

/// Everything needed to reproduce a run, echoed into results and claims.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// The exact spec that ran.
    pub spec: SimSpec,
    /// Total heatbath sweeps performed.
    pub total_sweeps: usize,
}

/// Simulation result. All observables carry jackknife errors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimResult {
    /// Average plaquette `⟨P⟩ = (1/2)⟨Re Tr U_p⟩`.
    pub plaquette: Estimate,
    /// Planar Wilson loops up to `max_wilson_extent`.
    pub wilson_loops: Vec<WilsonLoop>,
    /// Run provenance.
    pub provenance: Provenance,
}

/// Spec rejection reasons.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SpecError {
    /// A lattice extent is below 2.
    DegenerateLattice {
        /// The offending extents.
        dims: [usize; 4],
    },
    /// β must be positive and finite.
    BadBeta {
        /// The offending value.
        beta: f64,
    },
    /// Thermalization must be at least 1 sweep.
    NoThermalization,
    /// Not enough measurements for ≥ 2 complete jackknife bins.
    StarvedStatistics {
        /// Requested measurement sweeps.
        measurements: usize,
        /// Requested bin size.
        bin_size: usize,
    },
    /// A Wilson-loop extent exceeds half the smallest lattice extent
    /// (the loop would wrap and stop being a loop observable).
    LoopTooLarge {
        /// Requested max extent.
        max_extent: usize,
        /// Largest admissible extent for these dims.
        admissible: usize,
    },
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::DegenerateLattice { dims } => {
                write!(f, "all lattice extents must be >= 2, got {dims:?}")
            }
            SpecError::BadBeta { beta } => {
                write!(f, "beta must be positive and finite, got {beta}")
            }
            SpecError::NoThermalization => write!(f, "thermalization_sweeps must be >= 1"),
            SpecError::StarvedStatistics {
                measurements,
                bin_size,
            } => write!(
                f,
                "need >= 2 complete bins: {measurements} measurements at bin_size {bin_size}"
            ),
            SpecError::LoopTooLarge {
                max_extent,
                admissible,
            } => write!(
                f,
                "max_wilson_extent {max_extent} exceeds admissible {admissible} for these dims"
            ),
        }
    }
}

impl std::error::Error for SpecError {}

impl SimSpec {
    /// Validate the spec without running it.
    pub fn validate(&self) -> Result<(), SpecError> {
        if self.dims.iter().any(|&n| n < 2) {
            return Err(SpecError::DegenerateLattice { dims: self.dims });
        }
        if !(self.beta.is_finite() && self.beta > 0.0) {
            return Err(SpecError::BadBeta { beta: self.beta });
        }
        if self.thermalization_sweeps == 0 {
            return Err(SpecError::NoThermalization);
        }
        if self.bin_size == 0 || self.measurement_sweeps / self.bin_size.max(1) < 2 {
            return Err(SpecError::StarvedStatistics {
                measurements: self.measurement_sweeps,
                bin_size: self.bin_size,
            });
        }
        let admissible = self.dims.iter().copied().min().unwrap_or(0) / 2;
        if self.max_wilson_extent > admissible {
            return Err(SpecError::LoopTooLarge {
                max_extent: self.max_wilson_extent,
                admissible,
            });
        }
        Ok(())
    }
}

/// Run a validated spec to completion.
pub fn run(spec: &SimSpec) -> Result<SimResult, SpecError> {
    spec.validate()?;
    let mut rng = Rng::seeded(spec.seed);
    let mut lat = if spec.hot_start {
        Lattice::hot(spec.dims, &mut rng)
    } else {
        Lattice::cold(spec.dims)
    };

    let sweep = |lat: &mut Lattice, rng: &mut Rng| {
        heatbath_sweep(lat, spec.beta, rng);
        for _ in 0..spec.overrelax_per_heatbath {
            overrelax_sweep(lat, rng);
        }
    };

    for _ in 0..spec.thermalization_sweeps {
        sweep(&mut lat, &mut rng);
    }

    let n_loops: Vec<(usize, usize)> = (1..=spec.max_wilson_extent)
        .flat_map(|r| (1..=spec.max_wilson_extent).map(move |t| (r, t)))
        .filter(|&(r, t)| r <= t) // W(r,t) = W(t,r) by plane averaging
        .collect();

    let mut plaq_series = Vec::with_capacity(spec.measurement_sweeps);
    let mut loop_series: Vec<Vec<f64>> = vec![Vec::new(); n_loops.len()];
    for _ in 0..spec.measurement_sweeps {
        sweep(&mut lat, &mut rng);
        plaq_series.push(lat.average_plaquette());
        for (i, &(r, t)) in n_loops.iter().enumerate() {
            loop_series[i].push(lat.wilson_loop(r, t));
        }
    }

    // validate() guarantees >= 2 bins, so jackknife cannot fail here.
    let plaquette = jackknife(&plaq_series, spec.bin_size).expect("validated binning");
    let wilson_loops = n_loops
        .iter()
        .zip(&loop_series)
        .map(|(&(r, t), series)| WilsonLoop {
            r,
            t,
            value: jackknife(series, spec.bin_size).expect("validated binning"),
        })
        .collect();

    let total_sweeps = spec.thermalization_sweeps + spec.measurement_sweeps;
    Ok(SimResult {
        plaquette,
        wilson_loops,
        provenance: Provenance {
            spec: spec.clone(),
            total_sweeps,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_spec() -> SimSpec {
        SimSpec {
            dims: [4, 4, 4, 4],
            beta: 2.0,
            thermalization_sweeps: 30,
            measurement_sweeps: 40,
            overrelax_per_heatbath: 1,
            bin_size: 4,
            max_wilson_extent: 2,
            seed: 1,
            hot_start: false,
        }
    }

    #[test]
    fn rejects_bad_specs() {
        let mut s = base_spec();
        s.dims = [1, 4, 4, 4];
        assert!(matches!(
            s.validate(),
            Err(SpecError::DegenerateLattice { .. })
        ));
        let mut s = base_spec();
        s.beta = -1.0;
        assert!(matches!(s.validate(), Err(SpecError::BadBeta { .. })));
        let mut s = base_spec();
        s.thermalization_sweeps = 0;
        assert!(matches!(s.validate(), Err(SpecError::NoThermalization)));
        let mut s = base_spec();
        s.measurement_sweeps = 3;
        assert!(matches!(
            s.validate(),
            Err(SpecError::StarvedStatistics { .. })
        ));
        let mut s = base_spec();
        s.max_wilson_extent = 3;
        assert!(matches!(s.validate(), Err(SpecError::LoopTooLarge { .. })));
    }

    #[test]
    fn deterministic_per_seed() {
        let spec = base_spec();
        let a = run(&spec).unwrap();
        let b = run(&spec).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn wilson_1x1_matches_plaquette_mean() {
        let r = run(&base_spec()).unwrap();
        let w11 = r
            .wilson_loops
            .iter()
            .find(|w| w.r == 1 && w.t == 1)
            .unwrap();
        assert!((w11.value.mean - r.plaquette.mean).abs() < 1e-10);
    }

    #[test]
    fn area_law_ordering_in_confined_phase() {
        // At β = 2.0 (confined, coarse) larger loops must be smaller:
        // W(1,1) > W(1,2) > W(2,2) > 0 within errors.
        let mut spec = base_spec();
        spec.beta = 2.0;
        spec.measurement_sweeps = 60;
        let r = run(&spec).unwrap();
        let get = |rr, tt| {
            r.wilson_loops
                .iter()
                .find(|w| w.r == rr && w.t == tt)
                .unwrap()
                .value
                .mean
        };
        assert!(get(1, 1) > get(1, 2));
        assert!(get(1, 2) > get(2, 2));
        assert!(get(2, 2) > 0.0);
    }

    #[test]
    fn serde_round_trip() {
        let spec = base_spec();
        let json = serde_json::to_string(&spec).unwrap();
        let back: SimSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
    }
}
