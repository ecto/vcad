//! GD&T semantics that move fits: position at MMC with bonus
//! tolerance, virtual condition, and form-tolerance contributor
//! generators (M1).
//!
//! **Scoped subset, stated:** this module owns the position-tolerance
//! fit problem for patterns of cylindrical features (pins into holes —
//! the overwhelmingly common case), plus generators that turn flatness
//! and perpendicularity callouts into gap contributors. It does not
//! simulate datum reference frames, composite frames, or profile
//! tolerances; that is the full-3D-gauge feature set (3DCS/CETOL
//! territory) and a later milestone if ever.
//!
//! ## The honest bonus-tolerance model
//!
//! At MMC (ASME Y14.5 Ⓜ modifier), a feature departing from maximum
//! material condition earns *bonus* position tolerance: allowed zone
//! Ø = stated + |size − MMC|. Three facts the model keeps separate,
//! because conflating them is how stackup spreadsheets lie:
//!
//! 1. **Bonus changes conformance, not physics.** The process's
//!    position scatter ([`PositionedFeature::sigma_pos`]) is a process
//!    property; a big hole doesn't move differently, it just *passes a
//!    gauge* it would otherwise fail — and fits more easily because the
//!    extra clearance is physically real.
//! 2. **Fit is about actual geometry.** The Monte Carlo fit model uses
//!    actual sampled sizes and actual radial misalignments; MMC bonus
//!    is "included" automatically because a larger hole genuinely
//!    clears a worse position error. No bonus bookkeeping can make a
//!    fit analysis more honest than sampling the joint reality.
//! 3. **Inspection truncates.** If parts are 100% gauged, the shipped
//!    population is conditioned on conformance. The classic theorem —
//!    encoded and tested here — is that gauging both parts at MMC
//!    makes fit **guaranteed** whenever the virtual conditions are
//!    compatible (hole VC ≥ pin VC): the gauge *is* the worst-case
//!    counterpart.
//!
//! Position scatter convention (stated, configurable): per-axis
//! position error is Normal(0, σ) with σ = (zone Ø/2)/k under the
//! chosen [`SigmaConvention`] — "the process hits the stated zone
//! radius at kσ per axis." The radial error is then Rayleigh(σ), and
//! every closed form below follows from the Rayleigh CDF
//! 1 − e^(−r²/2σ²).

use serde::{Deserialize, Serialize};

use crate::analysis::ProbabilityEstimate;
use crate::dist::{Distribution, SigmaConvention};
use crate::rng::Rng;
use crate::stackup::{Contributor, StackupError};

/// Internal (hole) or external (pin/boss) feature of size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureOf {
    /// Internal feature — MMC is the smallest size.
    Internal,
    /// External feature — MMC is the largest size.
    External,
}

/// Material-condition modifier on a position tolerance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modifier {
    /// Regardless of feature size: the stated zone, no bonus.
    Rfs,
    /// Maximum material condition: stated zone + |size − MMC| bonus.
    Mmc,
}

/// A cylindrical feature of size with a position tolerance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionedFeature {
    /// Feature name.
    pub name: String,
    /// Hole or pin.
    pub kind: FeatureOf,
    /// Nominal diameter, mm.
    pub size_nominal: f64,
    /// Lower drawing limit on diameter (deviation below nominal ≥ 0), mm.
    pub size_tol_minus: f64,
    /// Upper drawing limit on diameter (deviation above nominal ≥ 0), mm.
    pub size_tol_plus: f64,
    /// Diameter deviation distribution.
    pub size_dist: Distribution,
    /// Position tolerance zone diameter as stated in the feature
    /// control frame (at MMC when `modifier` is Mmc), mm.
    pub zone_dia: f64,
    /// Modifier on the position tolerance.
    pub modifier: Modifier,
    /// Per-axis position-error standard deviation, mm. A process
    /// property — see the module docs for the stated convention.
    pub sigma_pos: f64,
}

impl PositionedFeature {
    /// Per-axis σ from the stated zone under a convention: the process
    /// hits the zone *radius* at kσ per axis.
    pub fn sigma_from_zone(zone_dia: f64, convention: SigmaConvention) -> f64 {
        0.5 * zone_dia / convention.k()
    }

    /// Maximum-material size: smallest hole / largest pin, mm.
    pub fn mmc_size(&self) -> f64 {
        match self.kind {
            FeatureOf::Internal => self.size_nominal - self.size_tol_minus,
            FeatureOf::External => self.size_nominal + self.size_tol_plus,
        }
    }

    /// Least-material size, mm.
    pub fn lmc_size(&self) -> f64 {
        match self.kind {
            FeatureOf::Internal => self.size_nominal + self.size_tol_plus,
            FeatureOf::External => self.size_nominal - self.size_tol_minus,
        }
    }

    /// Virtual condition: the fixed gauge boundary (ASME Y14.5).
    /// Internal: MMC − zone; external: MMC + zone. Fit against the
    /// mating part's VC is the worst-case guarantee.
    pub fn virtual_condition(&self) -> f64 {
        match self.kind {
            FeatureOf::Internal => self.mmc_size() - self.zone_dia,
            FeatureOf::External => self.mmc_size() + self.zone_dia,
        }
    }

    /// Allowed position-zone diameter for an actual size: the stated
    /// zone, plus bonus at MMC.
    pub fn allowed_zone_dia(&self, actual_size: f64) -> f64 {
        match self.modifier {
            Modifier::Rfs => self.zone_dia,
            Modifier::Mmc => {
                let departure = match self.kind {
                    FeatureOf::Internal => actual_size - self.mmc_size(),
                    FeatureOf::External => self.mmc_size() - actual_size,
                };
                self.zone_dia + departure.max(0.0)
            }
        }
    }

    /// Sample one made feature: (actual diameter, position error x, y).
    pub fn sample(&self, rng: &mut Rng) -> (f64, f64, f64) {
        let size = self.size_nominal + self.size_dist.sample(rng);
        let ex = self.sigma_pos * rng.next_normal();
        let ey = self.sigma_pos * rng.next_normal();
        (size, ex, ey)
    }

    /// Whether a made feature passes its position gauge (the dynamic,
    /// bonus-widened zone at MMC; the fixed zone at RFS).
    pub fn conforms(&self, actual_size: f64, ex: f64, ey: f64) -> bool {
        let r2 = ex * ex + ey * ey;
        let allowed_r = 0.5 * self.allowed_zone_dia(actual_size);
        // Size must also be in its own band.
        let in_band = actual_size >= self.size_nominal - self.size_tol_minus - 1e-12
            && actual_size <= self.size_nominal + self.size_tol_plus + 1e-12;
        in_band && r2 <= allowed_r * allowed_r
    }

    /// Monte Carlo conformance probability — the honest bonus model:
    /// joint over the size distribution and the 2-D position scatter.
    /// At RFS this converges to the Rayleigh CDF at the fixed zone
    /// radius; at MMC it is strictly higher for any size distribution
    /// with spread (the bonus admits real parts).
    pub fn conformance_probability(&self, n: usize, seed: u64) -> ProbabilityEstimate {
        let mut rng = Rng::new(seed);
        let mut pass = 0usize;
        for _ in 0..n {
            let (size, ex, ey) = self.sample(&mut rng);
            if self.conforms(size, ex, ey) {
                pass += 1;
            }
        }
        ProbabilityEstimate::from_counts(pass, n)
    }

    fn check(&self) -> Result<(), StackupError> {
        let bad = |field: &'static str| StackupError::NonFinite {
            contributor: self.name.clone(),
            field,
        };
        if !self.size_nominal.is_finite() {
            return Err(bad("size_nominal"));
        }
        if !self.zone_dia.is_finite() || self.zone_dia < 0.0 {
            return Err(bad("zone_dia"));
        }
        if !self.sigma_pos.is_finite() || self.sigma_pos < 0.0 {
            return Err(bad("sigma_pos"));
        }
        if self.size_tol_minus < 0.0 || self.size_tol_plus < 0.0 {
            return Err(StackupError::NegativeTolerance(self.name.clone()));
        }
        self.size_dist
            .check()
            .map_err(|reason| StackupError::InvalidDistribution {
                contributor: self.name.clone(),
                reason,
            })
    }
}

/// A pattern fit: `n_features` pins (all drawn from the pin spec) must
/// simultaneously enter `n_features` holes (all drawn from the hole
/// spec). Pin and hole position errors are independent per feature —
/// the floating relative-misalignment case; a rigid common shift adds
/// correlation and is future work, stated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatternFit {
    /// The pin (external) spec.
    pub pin: PositionedFeature,
    /// The hole (internal) spec.
    pub hole: PositionedFeature,
    /// Number of features in the pattern (≥ 1).
    pub n_features: usize,
}

/// Worst-case (virtual condition) verdict for a pattern fit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VcFit {
    /// Hole virtual condition (fixed gauge boundary), mm.
    pub hole_vc: f64,
    /// Pin virtual condition, mm.
    pub pin_vc: f64,
    /// hole_vc − pin_vc: ≥ 0 means fit is guaranteed for conforming
    /// parts, mm.
    pub margin: f64,
    /// Whether worst-case fit is guaranteed.
    pub guaranteed: bool,
}

/// Monte Carlo fit result for a pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatternFitResult {
    /// Assembly-level fit probability (all features seat).
    pub fit: ProbabilityEstimate,
    /// Fraction of *assemblies* where every pin conformed to its gauge.
    pub pin_conformance: ProbabilityEstimate,
    /// Fraction of assemblies where every hole conformed.
    pub hole_conformance: ProbabilityEstimate,
    /// Whether sampling was conditioned on conformance (100%
    /// inspection): non-conforming parts re-drawn, so `fit` describes
    /// the shipped population.
    pub inspected: bool,
    /// Sample count (assemblies).
    pub n: usize,
    /// Seed.
    pub seed: u64,
}

impl PatternFit {
    fn check(&self) -> Result<(), StackupError> {
        self.pin.check()?;
        self.hole.check()?;
        if self.pin.kind != FeatureOf::External || self.hole.kind != FeatureOf::Internal {
            return Err(StackupError::BadRequirement(
                "PatternFit needs an external pin and an internal hole".into(),
            ));
        }
        if self.n_features == 0 {
            return Err(StackupError::Empty);
        }
        Ok(())
    }

    /// The worst-case gauge check: fit is guaranteed iff the hole's
    /// virtual condition is at least the pin's (fixed-fastener
    /// formula: H_mmc − T_hole ≥ F_mmc + T_pin, cf. ASME Y14.5
    /// appendix fastener formulas).
    pub fn worst_case(&self) -> Result<VcFit, StackupError> {
        self.check()?;
        let hole_vc = self.hole.virtual_condition();
        let pin_vc = self.pin.virtual_condition();
        let margin = hole_vc - pin_vc;
        Ok(VcFit {
            hole_vc,
            pin_vc,
            margin,
            guaranteed: margin >= 0.0,
        })
    }

    /// Monte Carlo over the exact joint model. `inspected` conditions
    /// each part on passing its position gauge (rejection sampling) —
    /// the shipped-population model. A feature seats iff the relative
    /// radial misalignment ≤ (hole − pin)/2 for its sampled sizes.
    pub fn monte_carlo(
        &self,
        n: usize,
        seed: u64,
        inspected: bool,
    ) -> Result<PatternFitResult, StackupError> {
        self.check()?;
        if n < crate::analysis::MIN_MC_SAMPLES {
            return Err(StackupError::TooFewSamples {
                n,
                min: crate::analysis::MIN_MC_SAMPLES,
            });
        }
        let mut rng = Rng::new(seed);
        let mut fit_count = 0usize;
        let mut pin_conf_count = 0usize;
        let mut hole_conf_count = 0usize;
        // Rejection cap: if conformance is below ~1%, inspected
        // sampling would spin; fail loudly instead of silently looping.
        let max_draws = 1000usize;
        for _ in 0..n {
            let mut all_fit = true;
            let mut all_pin_conf = true;
            let mut all_hole_conf = true;
            for _ in 0..self.n_features {
                let (pin_d, pex, pey, pin_conf) = {
                    let mut draws = 0;
                    loop {
                        let (d, x, y) = self.pin.sample(&mut rng);
                        let conf = self.pin.conforms(d, x, y);
                        if !inspected || conf {
                            break (d, x, y, conf);
                        }
                        draws += 1;
                        if draws > max_draws {
                            return Err(StackupError::BadRequirement(format!(
                                "inspected sampling: pin {:?} conformance too low \
                                 (>{max_draws} rejections)",
                                self.pin.name
                            )));
                        }
                    }
                };
                let (hole_d, hex, hey, hole_conf) = {
                    let mut draws = 0;
                    loop {
                        let (d, x, y) = self.hole.sample(&mut rng);
                        let conf = self.hole.conforms(d, x, y);
                        if !inspected || conf {
                            break (d, x, y, conf);
                        }
                        draws += 1;
                        if draws > max_draws {
                            return Err(StackupError::BadRequirement(format!(
                                "inspected sampling: hole {:?} conformance too low \
                                 (>{max_draws} rejections)",
                                self.hole.name
                            )));
                        }
                    }
                };
                all_pin_conf &= pin_conf;
                all_hole_conf &= hole_conf;
                // Relative misalignment vs actual radial clearance.
                let dx = hex - pex;
                let dy = hey - pey;
                let c = 0.5 * (hole_d - pin_d);
                if c < 0.0 || dx * dx + dy * dy > c * c {
                    all_fit = false;
                }
            }
            if all_fit {
                fit_count += 1;
            }
            if all_pin_conf {
                pin_conf_count += 1;
            }
            if all_hole_conf {
                hole_conf_count += 1;
            }
        }
        Ok(PatternFitResult {
            fit: ProbabilityEstimate::from_counts(fit_count, n),
            pin_conformance: ProbabilityEstimate::from_counts(pin_conf_count, n),
            hole_conformance: ProbabilityEstimate::from_counts(hole_conf_count, n),
            inspected,
            n,
            seed,
        })
    }
}

/// Flatness callout → gap contributor at a clamped interface.
///
/// Assumption (stated): high spots hold the mating faces apart, so a
/// flatness error of up to `t` consumes 0..t of gap, modeled Uniform(0,
/// t) — nothing is known about where in the band a surface lands, and
/// the error is one-sided by construction. `coeff` is +1 if the
/// separation opens the measured gap, −1 if it consumes it.
pub fn flatness_contributor(name: &str, coeff: f64, t: f64) -> Contributor {
    Contributor {
        name: name.to_string(),
        coeff,
        nominal: 0.0,
        tol_minus: 0.0,
        tol_plus: t,
        dist: Distribution::Uniform { lo: 0.0, hi: t },
        source: crate::dist::DistributionSource::Assumed {
            convention: SigmaConvention::ThreeSigma,
        },
    }
}

/// Perpendicularity callout → gap contributor for a feature engaged
/// over part of its control length.
///
/// A perpendicularity zone of width `t` over control length `l`
/// permits a tilt of up to t/l; at engagement height `h` the lateral
/// displacement is up to `t·h/l`, modeled Uniform(0, t·h/l), one-sided
/// (tilt direction unknown but displacement magnitude is what a gap
/// chain consumes). First-order small-angle, same bound as the loops
/// module.
pub fn perpendicularity_contributor(
    name: &str,
    coeff: f64,
    t: f64,
    control_len: f64,
    engagement: f64,
) -> Contributor {
    let span = t * engagement / control_len;
    Contributor {
        name: name.to_string(),
        coeff,
        nominal: 0.0,
        tol_minus: 0.0,
        tol_plus: span,
        dist: Distribution::Uniform { lo: 0.0, hi: span },
        source: crate::dist::DistributionSource::Assumed {
            convention: SigmaConvention::ThreeSigma,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin() -> PositionedFeature {
        PositionedFeature {
            name: "pin".into(),
            kind: FeatureOf::External,
            size_nominal: 6.0,
            size_tol_minus: 0.02,
            size_tol_plus: 0.0,
            size_dist: Distribution::Uniform { lo: -0.02, hi: 0.0 },
            zone_dia: 0.05,
            modifier: Modifier::Mmc,
            sigma_pos: PositionedFeature::sigma_from_zone(0.05, SigmaConvention::ThreeSigma),
        }
    }

    fn hole() -> PositionedFeature {
        PositionedFeature {
            name: "hole".into(),
            kind: FeatureOf::Internal,
            size_nominal: 6.12,
            size_tol_minus: 0.02,
            size_tol_plus: 0.03,
            size_dist: Distribution::Uniform {
                lo: -0.02,
                hi: 0.03,
            },
            zone_dia: 0.1,
            modifier: Modifier::Mmc,
            sigma_pos: PositionedFeature::sigma_from_zone(0.1, SigmaConvention::ThreeSigma),
        }
    }

    #[test]
    fn mmc_lmc_and_virtual_condition_arithmetic() {
        let p = pin();
        let h = hole();
        assert!((p.mmc_size() - 6.0).abs() < 1e-12);
        assert!((p.lmc_size() - 5.98).abs() < 1e-12);
        assert!((h.mmc_size() - 6.10).abs() < 1e-12);
        assert!((h.lmc_size() - 6.15).abs() < 1e-12);
        // VC: pin 6.00 + 0.05 = 6.05; hole 6.10 − 0.10 = 6.00.
        assert!((p.virtual_condition() - 6.05).abs() < 1e-12);
        assert!((h.virtual_condition() - 6.00).abs() < 1e-12);
        // Fixed-fastener margin: 6.00 − 6.05 = −0.05 → not guaranteed.
        let fit = PatternFit {
            pin: p,
            hole: h,
            n_features: 4,
        };
        let wc = fit.worst_case().unwrap();
        assert!((wc.margin + 0.05).abs() < 1e-12);
        assert!(!wc.guaranteed);
    }

    #[test]
    fn bonus_tolerance_arithmetic() {
        let h = hole();
        // At MMC size: no bonus. At LMC: bonus = 0.05.
        assert!((h.allowed_zone_dia(6.10) - 0.1).abs() < 1e-12);
        assert!((h.allowed_zone_dia(6.15) - 0.15).abs() < 1e-12);
        // RFS: never a bonus.
        let mut rfs = h.clone();
        rfs.modifier = Modifier::Rfs;
        assert!((rfs.allowed_zone_dia(6.15) - 0.1).abs() < 1e-12);
        // A pin's bonus grows as it shrinks.
        let p = pin();
        assert!((p.allowed_zone_dia(6.0) - 0.05).abs() < 1e-12);
        assert!((p.allowed_zone_dia(5.98) - 0.07).abs() < 1e-12);
    }

    #[test]
    fn rfs_conformance_matches_rayleigh_closed_form() {
        // At RFS with a fixed zone, conformance = Rayleigh CDF at the
        // zone radius: 1 − e^(−r²/2σ²); with r = 3σ per the
        // convention, that's 1 − e^(−4.5) = 0.98889.
        let mut f = hole();
        f.modifier = Modifier::Rfs;
        let p = f.conformance_probability(200_000, 77);
        let want = 1.0 - (-4.5f64).exp();
        assert!(
            (p.p - want).abs() < 4.0 * p.standard_error,
            "MC {} ± {} vs Rayleigh {}",
            p.p,
            p.standard_error,
            want
        );
    }

    #[test]
    fn mmc_bonus_raises_conformance_by_the_predicted_amount() {
        // MMC conformance = E over size of Rayleigh CDF at the
        // bonus-widened radius. Uniform size across [MMC, LMC] with
        // zone 0.1 and bonus up to 0.05: integrate numerically and
        // compare with MC — the honest model, checked both ways.
        let h = hole();
        let mc = h.conformance_probability(200_000, 78);
        let m = 2000;
        let sigma = h.sigma_pos;
        let mut acc = 0.0;
        for i in 0..m {
            let size = 6.10 + 0.05 * (i as f64 + 0.5) / m as f64;
            let r = 0.5 * h.allowed_zone_dia(size);
            acc += 1.0 - (-r * r / (2.0 * sigma * sigma)).exp();
        }
        let want = acc / m as f64;
        assert!(
            (mc.p - want).abs() < 4.0 * mc.standard_error,
            "MC {} ± {} vs integral {}",
            mc.p,
            mc.standard_error,
            want
        );
        // And the bonus is worth something real vs RFS.
        let mut rfs = h.clone();
        rfs.modifier = Modifier::Rfs;
        let rfs_p = rfs.conformance_probability(200_000, 79);
        assert!(
            mc.p > rfs_p.p + 3.0 * (mc.standard_error + rfs_p.standard_error),
            "bonus must raise conformance: MMC {} vs RFS {}",
            mc.p,
            rfs_p.p
        );
    }

    #[test]
    fn mmc_gauging_guarantees_fit_when_virtual_conditions_are_compatible() {
        // The Y14.5 theorem, reproduced by simulation: shrink the pin's
        // zone so hole VC ≥ pin VC, inspect both parts at MMC, and NO
        // assembly can fail — the gauge is the worst-case counterpart.
        let mut p = pin();
        p.zone_dia = 0.05;
        let mut h = hole();
        h.zone_dia = 0.05; // hole VC = 6.05 = pin VC → margin 0
                           // Widen position scatter so plenty of non-conforming parts are
                           // made (the theorem is about the shipped population).
        p.sigma_pos = 0.05;
        h.sigma_pos = 0.05;
        let fit = PatternFit {
            pin: p,
            hole: h,
            n_features: 4,
        };
        let wc = fit.worst_case().unwrap();
        assert!(wc.guaranteed, "margin {}", wc.margin);
        let r = fit.monte_carlo(50_000, 5150, true).unwrap();
        assert_eq!(
            r.fit.successes, r.n,
            "inspected + compatible VCs must fit every single time"
        );
        // Sanity: the theorem needed inspection — uninspected, with
        // this much scatter, some assemblies genuinely fail.
        let raw = fit.monte_carlo(50_000, 5151, false).unwrap();
        assert!(
            raw.fit.p < 0.999,
            "uninspected population should show failures: {}",
            raw.fit.p
        );
    }

    #[test]
    fn pattern_fit_monte_carlo_brackets_the_m0_bolt_circle() {
        // RFS, no inspection, zone Ø0.15: the same fixture as
        // tests/bolt_circle.rs — the API path must land on the same
        // ~90% assembly fit rate (cross-module consistency).
        let p = PositionedFeature {
            name: "pin".into(),
            kind: FeatureOf::External,
            size_nominal: 6.0,
            size_tol_minus: 0.02,
            size_tol_plus: 0.0,
            size_dist: Distribution::Uniform { lo: -0.02, hi: 0.0 },
            zone_dia: 0.0,
            modifier: Modifier::Rfs,
            sigma_pos: 0.0, // fixture: pins perfectly positioned
        };
        let h = PositionedFeature {
            name: "hole".into(),
            kind: FeatureOf::Internal,
            size_nominal: 6.10,
            size_tol_minus: 0.0,
            size_tol_plus: 0.05,
            size_dist: Distribution::Uniform { lo: 0.0, hi: 0.05 },
            zone_dia: 0.15,
            modifier: Modifier::Rfs,
            sigma_pos: PositionedFeature::sigma_from_zone(0.15, SigmaConvention::ThreeSigma),
        };
        let sigma = h.sigma_pos;
        let fit = PatternFit {
            pin: p,
            hole: h,
            n_features: 4,
        };
        let r = fit.monte_carlo(200_000, 90210, false).unwrap();
        // Reference: per-pin fit = E over both size bands of the
        // Rayleigh CDF at c = (hole − pin)/2, to the fourth power —
        // the same integral tests/bolt_circle.rs validates. (Note it
        // is the mean of the CDF, not the CDF at the mean clearance:
        // Jensen's inequality costs ~3 points here.)
        let m = 400;
        let mut acc = 0.0;
        for i in 0..m {
            let hole_d = 6.10 + 0.05 * (i as f64 + 0.5) / m as f64;
            for j in 0..m {
                let pin_d = 5.98 + 0.02 * (j as f64 + 0.5) / m as f64;
                let c = 0.5 * (hole_d - pin_d);
                acc += 1.0 - (-c * c / (2.0 * sigma * sigma)).exp();
            }
        }
        let want = (acc / (m * m) as f64).powi(4);
        assert!(
            (r.fit.p - want).abs() < 4.0 * r.fit.standard_error,
            "API path {} ± {} vs closed-form {}",
            r.fit.p,
            r.fit.standard_error,
            want
        );
    }

    #[test]
    fn form_generators_have_stated_moments_and_compose() {
        let f = flatness_contributor("face flatness", -1.0, 0.05);
        assert_eq!(f.dist.support(), Some((0.0, 0.05)));
        assert!((f.dist.mean() - 0.025).abs() < 1e-15);
        assert_eq!(f.tol_plus, 0.05);
        assert_eq!(f.tol_minus, 0.0);

        // Perpendicularity 0.1 over 20 mm, engaged 8 mm: span 0.04.
        let p = perpendicularity_contributor("post tilt", -1.0, 0.1, 20.0, 8.0);
        assert_eq!(p.dist.support(), Some((0.0, 0.04)));

        // They validate inside a stackup.
        let s = crate::stackup::Stackup {
            name: "with-form".into(),
            contributors: vec![
                Contributor::normal("depth", 1.0, 10.0, 0.1, SigmaConvention::ThreeSigma),
                f,
                p,
            ],
            requirement: crate::stackup::Requirement::at_least("gap", 9.8),
        };
        s.validate().unwrap();
        let r = crate::analysis::rss(&s).unwrap();
        assert!(r.mean_gap < 10.0, "form errors consume gap: {}", r.mean_gap);
    }

    #[test]
    fn fail_closed_paths() {
        let mut f = PatternFit {
            pin: pin(),
            hole: hole(),
            n_features: 0,
        };
        assert!(f.worst_case().is_err(), "zero features");
        f.n_features = 4;
        std::mem::swap(&mut f.pin.kind, &mut f.hole.kind);
        assert!(f.worst_case().is_err(), "swapped kinds");

        let mut g = PatternFit {
            pin: pin(),
            hole: hole(),
            n_features: 1,
        };
        g.pin.zone_dia = f64::NAN;
        assert!(g.worst_case().is_err(), "NaN zone");

        // Inspected sampling with hopeless conformance errors loudly.
        let mut h = PatternFit {
            pin: pin(),
            hole: hole(),
            n_features: 1,
        };
        h.hole.sigma_pos = 50.0; // conformance ~1e-6
        assert!(
            h.monte_carlo(1_000, 1, true).is_err(),
            "rejection cap must fire"
        );
    }
}
