//! Per-configuration field exports — the visualization seam (M2).
//!
//! A [`FieldSnapshot`] is what the viewport (or an MCP tool) consumes
//! to draw the vacuum: the local action density per site (the "boiling
//! vacuum" and, cooled, the instanton lumps) and the Polyakov-loop
//! field per spatial site (the confinement order-parameter texture).
//! Both are plain row-major `Vec<f64>` over the lattice dims, serde
//! all the way down.
//!
//! [`FluxTubeAccumulator`] measures the chromoelectric flux tube
//! between a static quark–antiquark pair: the connected correlator of
//! a Polyakov-loop pair at spatial separation `R` (along axis 0) with
//! the action density, translation-averaged, as a 3D profile over
//! displacements from the quark. This is the "drag the quarks apart
//! and watch the tube stretch" demo's data source. It also tallies the
//! pair correlator `⟨ℓ(0)ℓ(R)⟩` itself, whose decay with `R` *is* the
//! static potential — the confinement signal in one number per
//! separation.

use serde::{Deserialize, Serialize};

use crate::group::GaugeGroup;
use crate::lattice::{Lattice, ND, TIME_DIR};
use crate::stats::{jackknife, Estimate, StatsError};

/// Per-configuration scalar fields for rendering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSnapshot {
    /// Lattice extents.
    pub dims: [usize; 4],
    /// Local action density `s(x) = Σ_{μ<ν}(1 − (1/N)Re Tr U_μν(x))`
    /// per site, row-major over `dims`. ≥ 0, larger = hotter vacuum.
    pub action_density: Vec<f64>,
    /// Polyakov loop real part per spatial site, row-major over
    /// `dims[0..3]`.
    pub polyakov_re: Vec<f64>,
    /// Polyakov loop imaginary part per spatial site (identically 0
    /// for SU(2); the Z₃ phase texture for SU(3)).
    pub polyakov_im: Vec<f64>,
}

/// Take a rendering snapshot of the current configuration.
pub fn snapshot<G: GaugeGroup>(lat: &Lattice<G>) -> FieldSnapshot {
    FieldSnapshot {
        dims: lat.dims,
        action_density: action_density(lat),
        polyakov_re: polyakov_field(lat).0,
        polyakov_im: polyakov_field(lat).1,
    }
}

/// Local action density per site (row-major over dims).
pub fn action_density<G: GaugeGroup>(lat: &Lattice<G>) -> Vec<f64> {
    let mut out = Vec::with_capacity(lat.volume());
    for site in 0..lat.volume() {
        let mut s = 0.0;
        for mu in 0..ND {
            for nu in (mu + 1)..ND {
                s += 1.0 - lat.plaquette(site, mu, nu).norm_trace();
            }
        }
        out.push(s);
    }
    out
}

/// Polyakov loop field per spatial site: (re, im) vectors, row-major
/// over the spatial dims.
pub fn polyakov_field<G: GaugeGroup>(lat: &Lattice<G>) -> (Vec<f64>, Vec<f64>) {
    let sd = [lat.dims[0], lat.dims[1], lat.dims[2]];
    let mut re = Vec::with_capacity(sd.iter().product());
    let mut im = Vec::with_capacity(sd.iter().product());
    for i in 0..sd[0] {
        for j in 0..sd[1] {
            for k in 0..sd[2] {
                let (r, m) = polyakov_complex(lat, [i, j, k]);
                re.push(r);
                im.push(m);
            }
        }
    }
    (re, im)
}

/// Complex Polyakov loop `(1/N)Tr ∏_t U_t` at a spatial site.
pub fn polyakov_complex<G: GaugeGroup>(lat: &Lattice<G>, xyz: [usize; 3]) -> (f64, f64) {
    let c = [xyz[0], xyz[1], xyz[2], 0];
    let (u, _) = lat.line(c, TIME_DIR, lat.dims[TIME_DIR]);
    (u.norm_trace(), u.norm_trace_im())
}

/// The measured flux-tube profile between a static pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FluxTubeProfile {
    /// Pair separation in lattice units (along spatial axis 0).
    pub separation: usize,
    /// Spatial dims of the displacement grid.
    pub spatial_dims: [usize; 3],
    /// Connected action-density excess per displacement from the
    /// quark, row-major over `spatial_dims`:
    /// `⟨ℓℓ̄ s(d)⟩/⟨ℓℓ̄⟩ − ⟨s⟩`, jackknife mean.
    pub excess_mean: Vec<f64>,
    /// Jackknife error per displacement.
    pub excess_err: Vec<f64>,
    /// The Polyakov pair correlator `⟨ℓ(0)ℓ̄(R)⟩` — decays as
    /// `exp(−V(R)·N_t)`: the static potential in one number.
    pub pair_correlator: Estimate,
}

/// Accumulates flux-tube statistics over an ensemble of
/// configurations. Feed each measured configuration with
/// [`FluxTubeAccumulator::measure`], finish with
/// [`FluxTubeAccumulator::profile`].
#[derive(Debug, Clone)]
pub struct FluxTubeAccumulator {
    separation: usize,
    spatial_dims: [usize; 3],
    /// Per-config translation-averaged weighted density per displacement.
    weighted: Vec<Vec<f64>>,
    /// Per-config pair correlator.
    pair: Vec<f64>,
    /// Per-config plain mean action density.
    plain: Vec<f64>,
}

impl FluxTubeAccumulator {
    /// New accumulator for pair separation `separation` (lattice units,
    /// along spatial axis 0).
    pub fn new(dims: [usize; 4], separation: usize) -> FluxTubeAccumulator {
        FluxTubeAccumulator {
            separation,
            spatial_dims: [dims[0], dims[1], dims[2]],
            weighted: Vec::new(),
            pair: Vec::new(),
            plain: Vec::new(),
        }
    }

    /// Measure one configuration.
    pub fn measure<G: GaugeGroup>(&mut self, lat: &Lattice<G>) {
        let sd = self.spatial_dims;
        let vs: usize = sd.iter().product();
        let (p_re, p_im) = polyakov_field(lat);
        let dens = action_density(lat);
        // Time-average the action density onto spatial sites.
        let nt = lat.dims[TIME_DIR];
        let mut s3 = vec![0.0; vs];
        for (spatial, s) in s3.iter_mut().enumerate() {
            let i = spatial / (sd[1] * sd[2]);
            let j = (spatial / sd[2]) % sd[1];
            let k = spatial % sd[2];
            for t in 0..nt {
                let site = ((i * lat.dims[1] + j) * lat.dims[2] + k) * lat.dims[3] + t;
                *s += dens[site];
            }
            *s /= nt as f64;
        }
        let idx = |i: usize, j: usize, k: usize| (i * sd[1] + j) * sd[2] + k;
        let mut w_num = vec![0.0; vs];
        let mut pair_sum = 0.0;
        let mut count = 0usize;
        for i in 0..sd[0] {
            for j in 0..sd[1] {
                for k in 0..sd[2] {
                    let a = idx(i, j, k);
                    let b = idx((i + self.separation) % sd[0], j, k);
                    // ℓ(x)·conj(ℓ(x+R)) — real part (the correlator is
                    // real on average by charge symmetry).
                    let ll = p_re[a] * p_re[b] + p_im[a] * p_im[b];
                    pair_sum += ll;
                    count += 1;
                    // Bin density by displacement from the quark at x.
                    for di in 0..sd[0] {
                        for dj in 0..sd[1] {
                            for dk in 0..sd[2] {
                                let x = idx((i + di) % sd[0], (j + dj) % sd[1], (k + dk) % sd[2]);
                                w_num[idx(di, dj, dk)] += ll * s3[x];
                            }
                        }
                    }
                }
            }
        }
        let n = count as f64;
        self.pair.push(pair_sum / n);
        self.plain.push(s3.iter().sum::<f64>() / vs as f64);
        self.weighted.push(w_num.iter().map(|w| w / n).collect());
    }

    /// Jackknife the accumulated ensemble into a profile. The connected
    /// excess is formed at the ensemble level (ratio of means, the
    /// standard estimator), with jackknife errors from bin deletion.
    pub fn profile(&self, bin_size: usize) -> Result<FluxTubeProfile, StatsError> {
        let vs: usize = self.spatial_dims.iter().product();
        let pair = jackknife(&self.pair, bin_size)?;
        let plain = jackknife(&self.plain, bin_size)?;
        let mut excess_mean = Vec::with_capacity(vs);
        let mut excess_err = Vec::with_capacity(vs);
        for d in 0..vs {
            let series: Vec<f64> = self.weighted.iter().map(|w| w[d]).collect();
            let w = jackknife(&series, bin_size)?;
            // First-order propagation for w/pair − plain.
            let ratio = w.mean / pair.mean;
            excess_mean.push(ratio - plain.mean);
            let rel = (w.err / w.mean).abs().min(10.0);
            let relp = (pair.err / pair.mean).abs().min(10.0);
            let err = (ratio * (rel * rel + relp * relp).sqrt()).hypot(plain.err);
            excess_err.push(err.abs());
        }
        Ok(FluxTubeProfile {
            separation: self.separation,
            spatial_dims: self.spatial_dims,
            excess_mean,
            excess_err,
            pair_correlator: pair,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::su2::Su2;

    #[test]
    fn cold_lattice_fields() {
        let l: Lattice<Su2> = Lattice::cold([3, 3, 3, 2]);
        let s = snapshot(&l);
        assert_eq!(s.action_density.len(), l.volume());
        assert!(s.action_density.iter().all(|&d| d.abs() < 1e-12));
        assert_eq!(s.polyakov_re.len(), 27);
        assert!(s.polyakov_re.iter().all(|&p| (p - 1.0).abs() < 1e-12));
        assert!(s.polyakov_im.iter().all(|&p| p.abs() < 1e-12));
    }

    #[test]
    fn action_density_sums_to_global_action() {
        let mut rng = crate::rng::Rng::seeded(51);
        let l: Lattice<Su2> = Lattice::hot([3, 3, 3, 3], &mut rng);
        let total: f64 = action_density(&l).iter().sum();
        let expect = (1.0 - l.average_plaquette()) * (l.volume() * 6) as f64;
        assert!((total - expect).abs() < 1e-8, "{total} vs {expect}");
    }
}
