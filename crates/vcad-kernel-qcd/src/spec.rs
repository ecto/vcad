//! Simulation spec → run → result: the serde seam a future MCP tool
//! (`simulate_lattice_gauge`) will speak.
//!
//! Fail-closed at the boundary: a spec that cannot produce honest
//! statistics (no thermalization, too few measurements for the
//! requested binning, degenerate lattice) is rejected before any
//! sweep runs — there is no "partial" result.

use serde::{Deserialize, Serialize};

use crate::fields::{snapshot, FieldSnapshot, FluxTubeAccumulator, FluxTubeProfile};
use crate::group::GaugeGroup;
use crate::lattice::Lattice;
use crate::rng::Rng;
use crate::smear::ape_smear_spatial_n;
use crate::stats::{jackknife, Estimate};
use crate::su2::Su2;
use crate::su3::Su3;
use crate::update::{cool_sweep, heatbath_sweep, overrelax_sweep};

/// The gauge group to simulate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Gauge {
    /// SU(2) — quaternion links, exact Kennedy–Pendleton heatbath.
    #[default]
    Su2,
    /// SU(3) — 3×3 complex links, Cabibbo–Marinari subgroup updates.
    Su3,
}

/// APE smearing applied (to a copy) before loop measurements.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SmearSpec {
    /// Smearing weight α ∈ (0,1).
    pub alpha: f64,
    /// Number of passes.
    pub iterations: usize,
}

/// Flux-tube measurement request (M2).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FluxTubeSpec {
    /// Static-pair separation in lattice units, along spatial axis 0.
    pub separation: usize,
}

/// Simulation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimSpec {
    /// Gauge group (default SU(2)).
    #[serde(default)]
    pub gauge: Gauge,
    /// Lattice extents `[n₀, n₁, n₂, n₃]`, each ≥ 2. Direction 3 is
    /// time (shorten it for finite temperature).
    pub dims: [usize; 4],
    /// Inverse coupling β = 2N/g² (Wilson action convention).
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
    /// APE-smear (a copy of) each configuration before loop
    /// measurement; the plaquette stays unsmeared.
    #[serde(default)]
    pub smear: Option<SmearSpec>,
    /// Also measure spatial×temporal Wilson loops (for the static
    /// potential) up to `max_wilson_extent` spatially and `dims[3]/2`
    /// temporally.
    #[serde(default)]
    pub measure_temporal_loops: bool,
    /// Measure the volume-averaged Polyakov loop magnitude ⟨|L|⟩ (the
    /// deconfinement order parameter).
    #[serde(default)]
    pub measure_polyakov: bool,
    /// Accumulate the flux-tube profile for a static pair (M2).
    #[serde(default)]
    pub flux_tube: Option<FluxTubeSpec>,
    /// Export a rendering [`FieldSnapshot`] of the final configuration,
    /// after this many cooling sweeps (0 = raw). `None` = no snapshot.
    #[serde(default)]
    pub snapshot_cooling: Option<usize>,
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
    /// Average plaquette `⟨P⟩ = (1/N)⟨Re Tr U_p⟩` (unsmeared).
    pub plaquette: Estimate,
    /// Planar Wilson loops up to `max_wilson_extent` (smeared if
    /// requested), averaged over all planes.
    pub wilson_loops: Vec<WilsonLoop>,
    /// Spatial×temporal Wilson loops (smeared if requested), when
    /// `measure_temporal_loops` was set.
    #[serde(default)]
    pub temporal_loops: Vec<WilsonLoop>,
    /// ⟨|L|⟩ — Polyakov loop magnitude, when `measure_polyakov`.
    #[serde(default)]
    pub polyakov_abs: Option<Estimate>,
    /// Flux-tube profile, when `flux_tube` was requested.
    #[serde(default)]
    pub flux_tube: Option<FluxTubeProfile>,
    /// Final-configuration rendering snapshot, when requested.
    #[serde(default)]
    pub snapshot: Option<FieldSnapshot>,
    /// Topological charge of the (cooled) snapshot configuration —
    /// SU(2) only at this milestone, present iff a snapshot with
    /// cooling ≥ 1 was taken on an SU(2) run.
    #[serde(default)]
    pub topological_charge: Option<f64>,
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
    /// Smearing weight must be in (0,1) with ≥ 1 iteration.
    BadSmear,
    /// Flux-tube separation must be ≥ 1 and < the axis-0 extent.
    BadFluxTubeSeparation {
        /// Requested separation.
        separation: usize,
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
            SpecError::BadSmear => write!(f, "smear alpha must be in (0,1) with >= 1 iteration"),
            SpecError::BadFluxTubeSeparation { separation } => {
                write!(
                    f,
                    "flux-tube separation {separation} does not fit the lattice"
                )
            }
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
        if let Some(s) = &self.smear {
            if !(s.alpha > 0.0 && s.alpha < 1.0) || s.iterations == 0 {
                return Err(SpecError::BadSmear);
            }
        }
        if let Some(ft) = &self.flux_tube {
            if ft.separation == 0 || ft.separation >= self.dims[0] {
                return Err(SpecError::BadFluxTubeSeparation {
                    separation: ft.separation,
                });
            }
        }
        Ok(())
    }
}

/// Run a validated spec to completion.
///
/// Implemented as an unbounded [`SimRun`] drive, so the one-shot and
/// chunked paths are identical by construction.
pub fn run(spec: &SimSpec) -> Result<SimResult, SpecError> {
    let mut run = SimRun::new(spec)?;
    run.advance(usize::MAX);
    run.finish()
}

/// What a bounded [`SimRun::advance`] call concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunProgress {
    /// Budget exhausted; more sweeps remain. Call `advance` again.
    Running,
    /// All sweeps done; take the result with [`SimRun::finish`].
    Done,
}

/// In-flight state of one gauge-group-specific run.
struct RunState<G: GaugeGroup> {
    spec: SimSpec,
    rng: Rng,
    lat: Lattice<G>,
    sym_loops: Vec<(usize, usize)>,
    tmp_loops: Vec<(usize, usize)>,
    plaq_series: Vec<f64>,
    sym_series: Vec<Vec<f64>>,
    tmp_series: Vec<Vec<f64>>,
    poly_series: Vec<f64>,
    flux: Option<FluxTubeAccumulator>,
    /// Compound sweeps (thermalization + measurement) completed so far.
    sweeps_done: usize,
}

impl<G: GaugeGroup> RunState<G> {
    fn new(spec: &SimSpec) -> Self {
        let mut rng = Rng::seeded(spec.seed);
        let lat: Lattice<G> = if spec.hot_start {
            Lattice::hot(spec.dims, &mut rng)
        } else {
            Lattice::cold(spec.dims)
        };
        let sym_loops: Vec<(usize, usize)> = (1..=spec.max_wilson_extent)
            .flat_map(|r| (1..=spec.max_wilson_extent).map(move |t| (r, t)))
            .filter(|&(r, t)| r <= t) // W(r,t) = W(t,r) by plane averaging
            .collect();
        let max_temporal_t = spec.dims[3] / 2;
        let tmp_loops: Vec<(usize, usize)> = if spec.measure_temporal_loops {
            (1..=spec.max_wilson_extent)
                .flat_map(|r| (1..=max_temporal_t).map(move |t| (r, t)))
                .collect()
        } else {
            Vec::new()
        };
        let sym_series = vec![Vec::new(); sym_loops.len()];
        let tmp_series = vec![Vec::new(); tmp_loops.len()];
        let flux = spec
            .flux_tube
            .map(|ft| FluxTubeAccumulator::new(spec.dims, ft.separation));
        RunState {
            spec: spec.clone(),
            rng,
            lat,
            sym_loops,
            tmp_loops,
            plaq_series: Vec::with_capacity(spec.measurement_sweeps),
            sym_series,
            tmp_series,
            poly_series: Vec::new(),
            flux,
            sweeps_done: 0,
        }
    }

    fn total_sweeps(&self) -> usize {
        self.spec.thermalization_sweeps + self.spec.measurement_sweeps
    }

    fn sweep(&mut self) {
        heatbath_sweep(&mut self.lat, self.spec.beta, &mut self.rng);
        for _ in 0..self.spec.overrelax_per_heatbath {
            overrelax_sweep(&mut self.lat, &mut self.rng);
        }
    }

    fn advance(&mut self, budget: usize) -> RunProgress {
        let total = self.total_sweeps();
        let mut left = budget.max(1);
        while self.sweeps_done < total && left > 0 {
            self.sweep();
            if self.sweeps_done >= self.spec.thermalization_sweeps {
                self.measure();
            }
            self.sweeps_done += 1;
            left -= 1;
        }
        if self.sweeps_done < total {
            RunProgress::Running
        } else {
            RunProgress::Done
        }
    }

    fn measure(&mut self) {
        self.plaq_series.push(self.lat.average_plaquette());
        let measured: Lattice<G> = match &self.spec.smear {
            Some(s) => ape_smear_spatial_n(&self.lat, s.alpha, s.iterations),
            None => self.lat.clone(),
        };
        for (i, &(r, t)) in self.sym_loops.iter().enumerate() {
            self.sym_series[i].push(measured.wilson_loop(r, t));
        }
        for (i, &(r, t)) in self.tmp_loops.iter().enumerate() {
            self.tmp_series[i].push(measured.wilson_loop_temporal(r, t));
        }
        if self.spec.measure_polyakov {
            // |volume-average L| per configuration: the finite-volume
            // order parameter (⟨L⟩ itself averages to 0 by center
            // symmetry in the confined phase).
            let (re, im) = crate::fields::polyakov_field(&self.lat);
            let n = re.len() as f64;
            let mre = re.iter().sum::<f64>() / n;
            let mim = im.iter().sum::<f64>() / n;
            self.poly_series.push((mre * mre + mim * mim).sqrt());
        }
        if let Some(acc) = self.flux.as_mut() {
            acc.measure(&self.lat);
        }
    }

    fn finish(mut self) -> Result<SimResult, SpecError> {
        let total_sweeps = self.total_sweeps();
        let flux = self.flux.take();
        let spec = &self.spec;
        // validate() guarantees >= 2 bins, so jackknife cannot fail here.
        let jk = |series: &[f64]| jackknife(series, spec.bin_size).expect("validated binning");
        let plaquette = jk(&self.plaq_series);
        let collect = |pairs: &[(usize, usize)], series: &[Vec<f64>]| {
            pairs
                .iter()
                .zip(series)
                .map(|(&(r, t), s)| WilsonLoop { r, t, value: jk(s) })
                .collect::<Vec<_>>()
        };
        let wilson_loops = collect(&self.sym_loops, &self.sym_series);
        let temporal_loops = collect(&self.tmp_loops, &self.tmp_series);
        let polyakov_abs = spec.measure_polyakov.then(|| jk(&self.poly_series));
        let flux_tube = flux.map(|acc| {
            acc.profile(spec.bin_size)
                .expect("validated binning for flux profile")
        });

        let (snapshot_out, topological_charge) = match spec.snapshot_cooling {
            None => (None, None),
            Some(n_cool) => {
                let mut cooled = self.lat.clone();
                for _ in 0..n_cool {
                    cool_sweep(&mut cooled);
                }
                let snap = snapshot(&cooled);
                let q = if n_cool >= 1 {
                    topo_charge_if_su2(&cooled)
                } else {
                    None
                };
                (Some(snap), q)
            }
        };

        Ok(SimResult {
            plaquette,
            wilson_loops,
            temporal_loops,
            polyakov_abs,
            flux_tube,
            snapshot: snapshot_out,
            topological_charge,
            provenance: Provenance {
                spec: self.spec.clone(),
                total_sweeps,
            },
        })
    }
}

enum RunInner {
    Su2(Box<RunState<Su2>>),
    Su3(Box<RunState<Su3>>),
}

/// Stepwise driver for a lattice gauge run: [`SimRun::new`] validates the
/// spec and initializes the lattice (RNG state lives inside, so chunked
/// runs are bit-identical to [`run`]), each [`SimRun::advance`] performs up
/// to a budget of compound sweeps, and [`SimRun::finish`] assembles the
/// [`SimResult`]. A host can surface progress between `advance` calls.
pub struct SimRun {
    inner: RunInner,
}

impl SimRun {
    /// Validate the spec and initialize the run.
    pub fn new(spec: &SimSpec) -> Result<Self, SpecError> {
        spec.validate()?;
        let inner = match spec.gauge {
            Gauge::Su2 => RunInner::Su2(Box::new(RunState::new(spec))),
            Gauge::Su3 => RunInner::Su3(Box::new(RunState::new(spec))),
        };
        Ok(SimRun { inner })
    }

    /// Compound sweeps completed so far.
    pub fn sweeps_done(&self) -> usize {
        match &self.inner {
            RunInner::Su2(s) => s.sweeps_done,
            RunInner::Su3(s) => s.sweeps_done,
        }
    }

    /// Total compound sweeps this run will perform.
    pub fn total_sweeps(&self) -> usize {
        match &self.inner {
            RunInner::Su2(s) => s.total_sweeps(),
            RunInner::Su3(s) => s.total_sweeps(),
        }
    }

    /// Perform up to `budget` compound sweeps (min 1).
    pub fn advance(&mut self, budget: usize) -> RunProgress {
        match &mut self.inner {
            RunInner::Su2(s) => s.advance(budget),
            RunInner::Su3(s) => s.advance(budget),
        }
    }

    /// Assemble the result. Call only after `advance` reported
    /// [`RunProgress::Done`]; finishing early would jackknife a
    /// truncated series, so it is an error.
    pub fn finish(self) -> Result<SimResult, SpecError> {
        let (done, total) = (self.sweeps_done(), self.total_sweeps());
        if done < total {
            return Err(SpecError::StarvedStatistics {
                measurements: done.saturating_sub(match &self.inner {
                    RunInner::Su2(s) => s.spec.thermalization_sweeps,
                    RunInner::Su3(s) => s.spec.thermalization_sweeps,
                }),
                bin_size: match &self.inner {
                    RunInner::Su2(s) => s.spec.bin_size,
                    RunInner::Su3(s) => s.spec.bin_size,
                },
            });
        }
        match self.inner {
            RunInner::Su2(s) => s.finish(),
            RunInner::Su3(s) => s.finish(),
        }
    }
}

/// Topological charge for SU(2) lattices; `None` for other groups
/// (clover Q is implemented for SU(2) at this milestone).
fn topo_charge_if_su2<G: GaugeGroup>(lat: &Lattice<G>) -> Option<f64> {
    use std::any::Any;
    let any: &dyn Any = lat;
    any.downcast_ref::<Lattice<Su2>>()
        .map(crate::topology::topological_charge)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn base_spec() -> SimSpec {
        SimSpec {
            gauge: Gauge::Su2,
            dims: [4, 4, 4, 4],
            beta: 2.0,
            thermalization_sweeps: 30,
            measurement_sweeps: 40,
            overrelax_per_heatbath: 1,
            bin_size: 4,
            max_wilson_extent: 2,
            seed: 1,
            hot_start: false,
            smear: None,
            measure_temporal_loops: false,
            measure_polyakov: false,
            flux_tube: None,
            snapshot_cooling: None,
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
        let mut s = base_spec();
        s.smear = Some(SmearSpec {
            alpha: 1.5,
            iterations: 2,
        });
        assert!(matches!(s.validate(), Err(SpecError::BadSmear)));
        let mut s = base_spec();
        s.flux_tube = Some(FluxTubeSpec { separation: 4 });
        assert!(matches!(
            s.validate(),
            Err(SpecError::BadFluxTubeSeparation { .. })
        ));
    }

    #[test]
    fn chunked_run_matches_the_one_shot() {
        // Small odd budget so chunk boundaries land mid-thermalization
        // and mid-measurement; RNG state lives in the run, so the
        // result must be bit-identical to the one-shot path.
        let mut spec = base_spec();
        spec.measure_polyakov = true;
        spec.flux_tube = Some(FluxTubeSpec { separation: 1 });
        let one_shot = run(&spec).unwrap();
        let mut chunked = SimRun::new(&spec).unwrap();
        let mut calls = 0;
        while chunked.advance(7) == RunProgress::Running {
            calls += 1;
            assert!(chunked.sweeps_done() <= chunked.total_sweeps());
        }
        assert!(calls > 2, "budget too generous to exercise chunking");
        assert_eq!(chunked.finish().unwrap(), one_shot);
    }

    #[test]
    fn early_finish_is_refused() {
        let spec = base_spec();
        let mut r = SimRun::new(&spec).unwrap();
        assert_eq!(r.advance(3), RunProgress::Running);
        assert!(r.finish().is_err());
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
    fn optional_measurements_populate() {
        let mut spec = base_spec();
        spec.measurement_sweeps = 24;
        spec.bin_size = 4;
        spec.measure_temporal_loops = true;
        spec.measure_polyakov = true;
        spec.smear = Some(SmearSpec {
            alpha: 0.5,
            iterations: 2,
        });
        spec.flux_tube = Some(FluxTubeSpec { separation: 1 });
        spec.snapshot_cooling = Some(5);
        let r = run(&spec).unwrap();
        assert!(!r.temporal_loops.is_empty());
        assert!(r.polyakov_abs.is_some());
        let ft = r.flux_tube.as_ref().unwrap();
        assert_eq!(ft.spatial_dims, [4, 4, 4]);
        assert!(ft.pair_correlator.mean.is_finite());
        assert!(ft.excess_mean.iter().all(|v| v.is_finite()));
        let snap = r.snapshot.as_ref().unwrap();
        assert_eq!(snap.action_density.len(), 256);
        assert!(r.topological_charge.is_some());
    }

    #[test]
    fn su3_runs_and_serializes() {
        let mut spec = base_spec();
        spec.gauge = Gauge::Su3;
        spec.dims = [3, 3, 3, 3];
        spec.beta = 5.5;
        spec.thermalization_sweeps = 10;
        spec.measurement_sweeps = 12;
        spec.bin_size = 3;
        spec.max_wilson_extent = 1;
        spec.snapshot_cooling = Some(2);
        let r = run(&spec).unwrap();
        assert!(r.plaquette.mean > 0.0 && r.plaquette.mean < 1.0);
        // Clover Q is SU(2)-only at this milestone: the SU(3) run
        // reports no charge rather than a wrong number.
        assert!(r.topological_charge.is_none());
        let json = serde_json::to_string(&r).unwrap();
        let back: SimResult = serde_json::from_str(&json).unwrap();
        // serde_json's default float parse is not ulp-exact; compare
        // structurally with tolerance.
        assert_eq!(back.provenance, r.provenance);
        assert!((back.plaquette.mean - r.plaquette.mean).abs() < 1e-12);
        let (s1, s2) = (r.snapshot.unwrap(), back.snapshot.unwrap());
        assert_eq!(s1.action_density.len(), s2.action_density.len());
        assert!((s1.polyakov_im[0] - s2.polyakov_im[0]).abs() < 1e-12);
    }

    #[test]
    fn old_m0_spec_json_still_parses() {
        // Serde defaults keep the M0 wire format valid.
        let json = r#"{
            "dims": [4,4,4,4], "beta": 2.0,
            "thermalization_sweeps": 5, "measurement_sweeps": 8,
            "overrelax_per_heatbath": 1, "bin_size": 2,
            "max_wilson_extent": 1, "seed": 7, "hot_start": false
        }"#;
        let spec: SimSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.gauge, Gauge::Su2);
        assert!(spec.smear.is_none());
        run(&spec).unwrap();
    }

    #[test]
    fn serde_round_trip() {
        let spec = base_spec();
        let json = serde_json::to_string(&spec).unwrap();
        let back: SimSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
    }
}
