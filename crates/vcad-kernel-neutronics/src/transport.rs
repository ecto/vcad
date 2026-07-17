//! Analog fixed-source Monte Carlo transport.
//!
//! One history: sample the source, fly `-ln(u)/Σt` between events, tally
//! track length into the current region/group, then either absorb
//! (terminate), scatter (sample the outgoing group from the material's
//! transfer row, new direction isotropic), or cross a region boundary.
//! Escaping the outermost boundary is leakage. Analog means no variance
//! reduction and no weights: every history is a physical neutron
//! analogue, so `absorbed + leaked = 1` **exactly** per batch (checked in
//! [`RunResult::balance_max_dev`]) — the books must balance, not merely
//! converge.
//!
//! Batches: `batches` independent RNG streams of `histories_per_batch`
//! histories each; every reported quantity is an [`Estimate`] over batch
//! means. Truncation honesty: histories that exceed `max_collisions`
//! (or the measure-zero parallel-flight-in-void slab pathology) are
//! *counted and reported*, never silently dropped — a nonzero
//! [`RunResult::truncated_histories`] taints the balance and must be
//! zero in any run used for claims.

use crate::dose::group_dose_factors_psv_cm2;
use crate::geometry::{Geometry, GeometryError};
use crate::groups::{N_GROUPS, SOURCE_GROUP, THERMAL_GROUP};
use crate::materials::LIBRARY_VERSION;
use crate::rng::Rng;
use crate::tally::Estimate;

/// Where and how source neutrons are born (all monoenergetic in
/// [`RunConfig::source_group`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Isotropic point source at the center of a [`Geometry::Sphere`].
    IsotropicPoint,
    /// Monodirectional beam entering a [`Geometry::Slab`] at x = 0
    /// along +x (the exponential-attenuation validation source).
    BeamPlusX,
    /// Isotropic emission into the +x hemisphere from the x = 0 face of
    /// a [`Geometry::Slab`] (cosine of an isotropically emitting plane:
    /// μ uniform on (0, 1]).
    IsotropicHalfSpace,
}

/// A complete, reproducible run description.
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// The layered geometry.
    pub geometry: Geometry,
    /// Source kind (must match the geometry family).
    pub source: Source,
    /// Birth energy group (default: the 2.45 MeV D-D source group).
    pub source_group: usize,
    /// Histories per batch.
    pub histories_per_batch: usize,
    /// Number of independent batches (≥ 2 — error bars are mandatory).
    pub batches: usize,
    /// RNG seed; same config + seed ⇒ bit-identical results.
    pub seed: u64,
    /// Collision cap per history (truncations are counted, see module
    /// docs).
    pub max_collisions: u32,
}

impl RunConfig {
    /// A config with the standard source group, 20 batches, and a
    /// 10⁴-collision cap.
    pub fn new(geometry: Geometry, source: Source, histories_per_batch: usize, seed: u64) -> Self {
        RunConfig {
            geometry,
            source,
            source_group: SOURCE_GROUP,
            histories_per_batch,
            batches: 20,
            seed,
            max_collisions: 10_000,
        }
    }
}

/// Config validation failures (fail-closed: no run without a valid
/// config).
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// Geometry failed validation.
    Geometry(GeometryError),
    /// Source kind incompatible with the geometry family.
    SourceGeometryMismatch,
    /// `source_group` out of range.
    BadSourceGroup(usize),
    /// Fewer than 2 batches or zero histories.
    DegenerateStatistics,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Geometry(e) => write!(f, "invalid geometry: {e}"),
            ConfigError::SourceGeometryMismatch => {
                write!(f, "source kind does not match geometry family")
            }
            ConfigError::BadSourceGroup(g) => write!(f, "source group {g} out of range"),
            ConfigError::DegenerateStatistics => {
                write!(
                    f,
                    "need ≥ 2 batches and ≥ 1 history/batch (error bars are mandatory)"
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Slowing-down observables (sphere point-source runs only).
#[derive(Debug, Clone)]
pub struct Thermalization {
    /// Fraction of histories that scattered into the thermal group.
    pub fraction: Estimate,
    /// Mean collisions at first thermal entry (among thermalized).
    pub mean_collisions: Estimate,
    /// Mean squared radius (cm²) at first thermal entry — the Fermi-age
    /// observable: ⟨r²⟩ = 6τ for a point fast source.
    pub mean_r2_cm2: Estimate,
}

/// Reproducibility provenance carried by every result (and every claim
/// built from one).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RunProvenance {
    /// RNG seed.
    pub seed: u64,
    /// Histories per batch.
    pub histories_per_batch: usize,
    /// Batch count.
    pub batches: usize,
    /// Collision cap.
    pub max_collisions: u32,
    /// Energy-group count.
    pub groups: usize,
    /// Bundled library version tag.
    pub library: String,
}

/// Everything a run reports. Every physical quantity is an [`Estimate`].
#[derive(Debug, Clone)]
pub struct RunResult {
    /// Track-length flux per region per group, n/cm² **per source
    /// neutron** (multiply by source rate for n/cm²/s).
    pub flux_per_source: Vec<[Estimate; N_GROUPS]>,
    /// Ambient dose equivalent per region, pSv **per source neutron**
    /// (flux folded with the ICRP-74-style group factors).
    pub dose_per_source_psv: Vec<Estimate>,
    /// Net outward surface-crossing current on the outer face of each
    /// region, crossings per source neutron per group. The last entry is
    /// the group-wise leakage spectrum.
    pub net_outward_current: Vec<[Estimate; N_GROUPS]>,
    /// Fraction of source neutrons absorbed anywhere.
    pub absorbed: Estimate,
    /// Fraction leaking out of the outer boundary (all groups).
    pub leaked_out: Estimate,
    /// Fraction leaking back out of the x = 0 face (slab only; zero
    /// tally for spheres).
    pub leaked_back: Estimate,
    /// Max over batches of |absorbed + leaked − 1| **excluding truncated
    /// histories** — analog transport must balance exactly (≈1e-15).
    pub balance_max_dev: f64,
    /// Histories killed at the collision cap — must be 0 for claims.
    pub truncated_histories: u64,
    /// Total histories run.
    pub total_histories: u64,
    /// Slowing-down observables (sphere point-source runs).
    pub thermalization: Option<Thermalization>,
    /// Reproducibility provenance.
    pub provenance: RunProvenance,
}

impl RunResult {
    /// Dose rate in a region for a physical source rate, µSv/h.
    pub fn dose_rate_usv_per_h(&self, region: usize, source_n_per_s: f64) -> Estimate {
        // pSv per source-n × n/s = pSv/s; × 3600 s/h × 1e-6 µSv/pSv.
        self.dose_per_source_psv[region].scaled(source_n_per_s * 3600.0 * 1.0e-6)
    }

    /// Flux in a region/group for a physical source rate, n/cm²/s.
    pub fn flux_n_cm2_s(&self, region: usize, group: usize, source_n_per_s: f64) -> Estimate {
        self.flux_per_source[region][group].scaled(source_n_per_s)
    }

    /// Total (group-summed) flux mean in a region per source neutron.
    /// Convenience for buildup comparisons; error propagation across
    /// correlated groups is deliberately not faked — use the per-group
    /// estimates for anything quantitative.
    pub fn total_flux_mean_per_source(&self, region: usize) -> f64 {
        self.flux_per_source[region].iter().map(|e| e.mean).sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeomKind {
    Slab,
    Sphere,
}

#[derive(Debug, Clone, Copy)]
enum Crossing {
    Outward,
    Inward,
}

/// Distance to the current region's boundary along the flight.
fn distance_to_boundary(
    kind: GeomKind,
    b_lo: f64,
    b_hi: f64,
    pos: f64,
    mu: f64,
) -> (f64, Crossing) {
    match kind {
        GeomKind::Slab => {
            if mu > 1.0e-10 {
                ((b_hi - pos) / mu, Crossing::Outward)
            } else if mu < -1.0e-10 {
                ((b_lo - pos) / mu, Crossing::Inward)
            } else {
                // Measure-zero parallel flight; caller truncates if the
                // region is void.
                (f64::INFINITY, Crossing::Outward)
            }
        }
        GeomKind::Sphere => {
            let one_minus_mu2 = (1.0 - mu * mu).max(0.0);
            let disc_out = (b_hi * b_hi - pos * pos * one_minus_mu2).max(0.0);
            let s_out = (-pos * mu + disc_out.sqrt()).max(0.0);
            if b_lo > 0.0 && mu < 0.0 {
                let disc_in = b_lo * b_lo - pos * pos * one_minus_mu2;
                if disc_in > 0.0 {
                    let s_in = -pos * mu - disc_in.sqrt();
                    if s_in > 0.0 && s_in < s_out {
                        return (s_in, Crossing::Inward);
                    }
                }
            }
            (s_out, Crossing::Outward)
        }
    }
}

/// Advance a walker by flight length `s`, updating position and (for
/// spheres) the radial direction cosine.
fn advance(kind: GeomKind, pos: &mut f64, mu: &mut f64, s: f64) {
    match kind {
        GeomKind::Slab => *pos += *mu * s,
        GeomKind::Sphere => {
            let r = *pos;
            let r2 = r * r + s * s + 2.0 * r * s * *mu;
            let r_new = r2.max(0.0).sqrt().max(1.0e-12);
            *mu = ((r * *mu + s) / r_new).clamp(-1.0, 1.0);
            *pos = r_new;
        }
    }
}

/// Sample an outgoing group from a transfer row.
fn sample_row(row: &[f64; N_GROUPS], u: f64) -> usize {
    let mut acc = 0.0;
    let mut last_nonzero = 0;
    for (g, p) in row.iter().enumerate() {
        if *p > 0.0 {
            last_nonzero = g;
            acc += p;
            if u <= acc {
                return g;
            }
        }
    }
    last_nonzero // float-sum guard: land in the last populated bin
}

/// Run the Monte Carlo. Deterministic in `config` (including seed).
pub fn run(config: &RunConfig) -> Result<RunResult, ConfigError> {
    config.geometry.validate().map_err(ConfigError::Geometry)?;
    let kind = match (&config.geometry, config.source) {
        (Geometry::Sphere(_), Source::IsotropicPoint) => GeomKind::Sphere,
        (Geometry::Slab(_), Source::BeamPlusX | Source::IsotropicHalfSpace) => GeomKind::Slab,
        _ => return Err(ConfigError::SourceGeometryMismatch),
    };
    if config.source_group >= N_GROUPS {
        return Err(ConfigError::BadSourceGroup(config.source_group));
    }
    if config.batches < 2 || config.histories_per_batch == 0 {
        return Err(ConfigError::DegenerateStatistics);
    }

    let bounds = config.geometry.boundaries_cm();
    let n_regions = config.geometry.region_count();
    let mats: Vec<_> = config
        .geometry
        .layers()
        .iter()
        .map(|l| &l.material)
        .collect();
    let volumes: Vec<f64> = (0..n_regions)
        .map(|i| config.geometry.region_volume_cc(i))
        .collect();
    let h_factors = group_dose_factors_psv_cm2();
    let hpb = config.histories_per_batch;

    // Per-batch reduced values.
    let nb = config.batches;
    let mut flux_b = vec![vec![[0.0f64; N_GROUPS]; n_regions]; nb];
    let mut dose_b = vec![vec![0.0f64; n_regions]; nb];
    let mut current_b = vec![vec![[0.0f64; N_GROUPS]; n_regions]; nb];
    let mut absorbed_b = vec![0.0f64; nb];
    let mut leak_out_b = vec![0.0f64; nb];
    let mut leak_back_b = vec![0.0f64; nb];
    let mut th_frac_b = vec![0.0f64; nb];
    let mut th_cols_b = vec![0.0f64; nb];
    let mut th_r2_b = vec![0.0f64; nb];
    let mut truncated: u64 = 0;
    let mut balance_max_dev: f64 = 0.0;

    for (batch, ((flux, dose), current)) in flux_b
        .iter_mut()
        .zip(dose_b.iter_mut())
        .zip(current_b.iter_mut())
        .enumerate()
    {
        let mut rng = Rng::stream(config.seed, batch as u64);
        let mut track = vec![[0.0f64; N_GROUPS]; n_regions];
        let mut cur = vec![[0.0f64; N_GROUPS]; n_regions];
        let mut absorbed = 0u64;
        let mut leak_out = 0u64;
        let mut leak_back = 0u64;
        let mut batch_trunc = 0u64;
        let mut th_count = 0u64;
        let mut th_cols = 0u64;
        let mut th_r2 = 0.0f64;

        for _ in 0..hpb {
            // Spawn.
            let (mut region, mut pos, mut mu) = match config.source {
                Source::IsotropicPoint => (0usize, 0.0f64, 1.0f64),
                Source::BeamPlusX => (0usize, 0.0f64, 1.0f64),
                Source::IsotropicHalfSpace => (0usize, 0.0f64, rng.uniform()),
            };
            let mut group = config.source_group;
            let mut collisions = 0u32;
            let mut was_thermal = group == THERMAL_GROUP;

            loop {
                let mat = mats[region];
                let st = mat.sigma_t[group];
                let d_coll = if st > 0.0 {
                    -rng.uniform().ln() / st
                } else {
                    f64::INFINITY
                };
                let (d_b, crossing) =
                    distance_to_boundary(kind, bounds[region], bounds[region + 1], pos, mu);
                if d_coll.is_infinite() && d_b.is_infinite() {
                    // Parallel flight in void (measure-zero): truncate,
                    // never hang.
                    batch_trunc += 1;
                    break;
                }
                if d_coll < d_b {
                    track[region][group] += d_coll;
                    advance(kind, &mut pos, &mut mu, d_coll);
                    if rng.uniform() * st <= mat.sigma_a[group] {
                        absorbed += 1;
                        break;
                    }
                    group = sample_row(&mat.transfer[group], rng.uniform());
                    if group == THERMAL_GROUP && !was_thermal {
                        was_thermal = true;
                        th_count += 1;
                        th_cols += u64::from(collisions) + 1;
                        if kind == GeomKind::Sphere {
                            th_r2 += pos * pos;
                        }
                    }
                    mu = rng.uniform_mu();
                    collisions += 1;
                    if collisions >= config.max_collisions {
                        batch_trunc += 1;
                        break;
                    }
                } else {
                    track[region][group] += d_b;
                    advance(kind, &mut pos, &mut mu, d_b);
                    match crossing {
                        Crossing::Outward => {
                            pos = bounds[region + 1]; // snap: no float drift
                            cur[region][group] += 1.0;
                            if region + 1 == n_regions {
                                leak_out += 1;
                                break;
                            }
                            region += 1;
                        }
                        Crossing::Inward => {
                            pos = bounds[region];
                            if region == 0 {
                                // Slab back-face escape (sphere region 0
                                // has no inner boundary by construction).
                                leak_back += 1;
                                break;
                            }
                            cur[region - 1][group] -= 1.0;
                            region -= 1;
                        }
                    }
                }
            }
        }

        // Reduce the batch.
        let h = hpb as f64;
        for r in 0..n_regions {
            for g in 0..N_GROUPS {
                flux[r][g] = track[r][g] / (volumes[r] * h);
                current[r][g] = cur[r][g] / h;
                dose[r] += h_factors[g] * flux[r][g];
            }
        }
        absorbed_b[batch] = absorbed as f64 / h;
        leak_out_b[batch] = leak_out as f64 / h;
        leak_back_b[batch] = leak_back as f64 / h;
        let live = h - batch_trunc as f64;
        if live > 0.0 {
            let dev = ((absorbed + leak_out + leak_back) as f64 / live - 1.0).abs();
            balance_max_dev = balance_max_dev.max(dev);
        }
        th_frac_b[batch] = th_count as f64 / h;
        th_cols_b[batch] = if th_count > 0 {
            th_cols as f64 / th_count as f64
        } else {
            0.0
        };
        th_r2_b[batch] = if th_count > 0 {
            th_r2 / th_count as f64
        } else {
            0.0
        };
        truncated += batch_trunc;
    }

    // Assemble estimates.
    let mut flux_per_source = Vec::with_capacity(n_regions);
    let mut dose_per_source_psv = Vec::with_capacity(n_regions);
    let mut net_outward_current = Vec::with_capacity(n_regions);
    let mut col = vec![0.0f64; nb];
    for r in 0..n_regions {
        let mut fg: [Estimate; N_GROUPS] = [Estimate::from_batches(&[0.0, 0.0]); N_GROUPS];
        let mut cg: [Estimate; N_GROUPS] = [Estimate::from_batches(&[0.0, 0.0]); N_GROUPS];
        for g in 0..N_GROUPS {
            for b in 0..nb {
                col[b] = flux_b[b][r][g];
            }
            fg[g] = Estimate::from_batches(&col);
            for b in 0..nb {
                col[b] = current_b[b][r][g];
            }
            cg[g] = Estimate::from_batches(&col);
        }
        flux_per_source.push(fg);
        net_outward_current.push(cg);
        for b in 0..nb {
            col[b] = dose_b[b][r];
        }
        dose_per_source_psv.push(Estimate::from_batches(&col));
    }

    let thermalization = if kind == GeomKind::Sphere {
        Some(Thermalization {
            fraction: Estimate::from_batches(&th_frac_b),
            mean_collisions: Estimate::from_batches(&th_cols_b),
            mean_r2_cm2: Estimate::from_batches(&th_r2_b),
        })
    } else {
        None
    };

    Ok(RunResult {
        flux_per_source,
        dose_per_source_psv,
        net_outward_current,
        absorbed: Estimate::from_batches(&absorbed_b),
        leaked_out: Estimate::from_batches(&leak_out_b),
        leaked_back: Estimate::from_batches(&leak_back_b),
        balance_max_dev,
        truncated_histories: truncated,
        total_histories: (config.batches * hpb) as u64,
        thermalization,
        provenance: RunProvenance {
            seed: config.seed,
            histories_per_batch: hpb,
            batches: config.batches,
            max_collisions: config.max_collisions,
            groups: N_GROUPS,
            library: LIBRARY_VERSION.to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Layer;
    use crate::materials::Material;

    #[test]
    fn config_validation_fails_closed() {
        let sphere = Geometry::Sphere(vec![Layer::new(Material::void(), 100.0)]);
        let mut c = RunConfig::new(sphere.clone(), Source::BeamPlusX, 10, 1);
        assert_eq!(run(&c).unwrap_err(), ConfigError::SourceGeometryMismatch);
        c.source = Source::IsotropicPoint;
        c.batches = 1;
        assert_eq!(run(&c).unwrap_err(), ConfigError::DegenerateStatistics);
        c.batches = 2;
        c.source_group = 99;
        assert_eq!(run(&c).unwrap_err(), ConfigError::BadSourceGroup(99));
    }

    #[test]
    fn bit_identical_reproducibility() {
        let g = || {
            Geometry::Sphere(vec![
                Layer::new(crate::materials::water(), 150.0),
                Layer::new(crate::materials::air(), 50.0),
            ])
        };
        let c1 = RunConfig::new(g(), Source::IsotropicPoint, 500, 12345);
        let c2 = RunConfig::new(g(), Source::IsotropicPoint, 500, 12345);
        let (r1, r2) = (run(&c1).unwrap(), run(&c2).unwrap());
        for reg in 0..2 {
            for gr in 0..N_GROUPS {
                assert_eq!(
                    r1.flux_per_source[reg][gr].mean,
                    r2.flux_per_source[reg][gr].mean
                );
            }
        }
        let c3 = RunConfig::new(g(), Source::IsotropicPoint, 500, 54321);
        let r3 = run(&c3).unwrap();
        assert_ne!(r1.flux_per_source[0][0].mean, r3.flux_per_source[0][0].mean);
    }

    #[test]
    fn analog_balance_is_exact() {
        let g = Geometry::Sphere(vec![Layer::new(crate::materials::hdpe(), 200.0)]);
        let c = RunConfig::new(g, Source::IsotropicPoint, 2_000, 7);
        let r = run(&c).unwrap();
        assert_eq!(r.truncated_histories, 0);
        assert!(
            r.balance_max_dev < 1.0e-12,
            "absorbed + leaked must equal 1 exactly (dev {})",
            r.balance_max_dev
        );
    }
}
