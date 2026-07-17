//! 2D/3D vector loops with linearized closure.
//!
//! A [`VectorLoop`] is a closed chain of dimensioned legs in 2D/3D; the
//! gap of interest is the loop-closure error **projected onto a measure
//! direction** d̂. For a leg with unit direction v̂ᵢ and scalar length
//! xᵢ, the projected contribution is (v̂ᵢ·d̂)·xᵢ — so projection turns a
//! vector loop into an exactly linear [`Stackup`] with coefficients
//! aᵢ = v̂ᵢ·d̂. This is **exact for translational legs** (the directions
//! themselves carry no tolerance) and **first-order for rotational
//! contributors**, which enter as displacement ≈ r·θ along the motion
//! direction.
//!
//! **Small-angle validity bound:** linearizing a rotation drops the
//! second-order transverse term r·(1 − cos θ) ≈ r·θ²/2 — a relative
//! error of θ/2 against the first-order term. For |θ| ≤ 2° (35 mrad)
//! the dropped term is ≤ 1.7% of the kept one; beyond ~5° you should
//! stop trusting linearized loops and model the mechanism.
//!
//! **What projection cannot see:** a leg perpendicular to d̂ contributes
//! zero *to first order* and is dropped from the projected chain. For
//! genuinely radial (2-D magnitude) requirements — pin in hole — the
//! magnitude of a 2-D error is Rayleigh-, not normal-, distributed, and
//! a single projection is optimistic. `tests/bolt_circle.rs` quantifies
//! this against the exact closed form; the GD&T module (M1) owns the
//! radial fit problem.

use serde::{Deserialize, Serialize};

use crate::dist::{Distribution, DistributionSource};
use crate::stackup::{Contributor, Requirement, Stackup, StackupError};

/// One leg of a vector loop: a dimension along a fixed direction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopLeg {
    /// Leg name (unique within the loop).
    pub name: String,
    /// Direction of the dimension (need not be unit; normalized at
    /// projection time).
    pub direction: [f64; 3],
    /// Nominal length along `direction`, mm.
    pub nominal: f64,
    /// Lower drawing limit (deviation below nominal, ≥ 0), mm.
    pub tol_minus: f64,
    /// Upper drawing limit (deviation above nominal, ≥ 0), mm.
    pub tol_plus: f64,
    /// Deviation distribution along the leg direction.
    pub dist: Distribution,
    /// Distribution provenance.
    #[serde(default)]
    pub source: DistributionSource,
}

impl LoopLeg {
    /// A rotational contributor, linearized: a joint/feature at lever
    /// arm `lever_arm_mm` whose angular error `angle_dist` (radians)
    /// displaces the point of interest by ≈ r·θ along
    /// `motion_direction`. Small-angle: see the module docs for the
    /// validity bound. `tol_rad` are the drawing limits on the angle.
    pub fn from_rotation(
        name: &str,
        lever_arm_mm: f64,
        motion_direction: [f64; 3],
        angle_dist: Distribution,
        tol_minus_rad: f64,
        tol_plus_rad: f64,
    ) -> Self {
        // Scale the angular distribution by the lever arm to get mm.
        let dist = match angle_dist {
            Distribution::Normal { mean, sigma } => Distribution::Normal {
                mean: mean * lever_arm_mm,
                sigma: sigma * lever_arm_mm,
            },
            Distribution::Uniform { lo, hi } => Distribution::Uniform {
                lo: lo * lever_arm_mm,
                hi: hi * lever_arm_mm,
            },
            Distribution::TwoPoint { a, b, p_b } => Distribution::TwoPoint {
                a: a * lever_arm_mm,
                b: b * lever_arm_mm,
                p_b,
            },
        };
        Self {
            name: name.to_string(),
            direction: motion_direction,
            nominal: 0.0,
            tol_minus: tol_minus_rad * lever_arm_mm,
            tol_plus: tol_plus_rad * lever_arm_mm,
            dist,
            source: DistributionSource::default(),
        }
    }
}

/// A vector loop: legs, a measure direction, and the requirement on the
/// projected gap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorLoop {
    /// Loop name.
    pub name: String,
    /// The legs.
    pub legs: Vec<LoopLeg>,
    /// Direction along which the gap is measured (normalized at
    /// projection time).
    pub measure_direction: [f64; 3],
    /// Requirement on the projected gap, mm.
    pub requirement: Requirement,
}

fn norm(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// Threshold below which a projected coefficient is treated as zero
/// (the leg is perpendicular to the measure direction to first order).
pub const PERPENDICULAR_EPS: f64 = 1e-12;

impl VectorLoop {
    /// Project the loop onto the measure direction, producing an
    /// exactly linear [`Stackup`] with aᵢ = v̂ᵢ·d̂. Perpendicular legs
    /// (|aᵢ| < [`PERPENDICULAR_EPS`]) contribute zero to first order
    /// and are dropped — see the module docs for what that misses.
    pub fn project(&self) -> Result<Stackup, StackupError> {
        let dn = norm(self.measure_direction);
        if !dn.is_finite() || dn == 0.0 {
            return Err(StackupError::BadRequirement(
                "measure_direction must be a nonzero finite vector".into(),
            ));
        }
        let d = self.measure_direction.map(|c| c / dn);
        let mut contributors = Vec::new();
        for leg in &self.legs {
            let vn = norm(leg.direction);
            if !vn.is_finite() || vn == 0.0 {
                return Err(StackupError::NonFinite {
                    contributor: leg.name.clone(),
                    field: "direction",
                });
            }
            let v = leg.direction.map(|c| c / vn);
            let coeff = v[0] * d[0] + v[1] * d[1] + v[2] * d[2];
            if coeff.abs() < PERPENDICULAR_EPS {
                continue; // perpendicular: zero to first order
            }
            contributors.push(Contributor {
                name: leg.name.clone(),
                coeff,
                nominal: leg.nominal,
                tol_minus: leg.tol_minus,
                tol_plus: leg.tol_plus,
                dist: leg.dist,
                source: leg.source.clone(),
            });
        }
        let s = Stackup {
            name: self.name.clone(),
            contributors,
            requirement: self.requirement.clone(),
        };
        s.validate()?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{monte_carlo, rss, McOptions};
    use crate::dist::SigmaConvention;
    use crate::stackup::Contributor as C;

    /// An L-bracket loop: horizontal leg, vertical leg, and a diagonal
    /// measured horizontally — coefficients are direction cosines.
    #[test]
    fn projection_reduces_to_linear_chain_with_direction_cosines() {
        let vl = VectorLoop {
            name: "L".into(),
            legs: vec![
                LoopLeg {
                    name: "base".into(),
                    direction: [1.0, 0.0, 0.0],
                    nominal: 40.0,
                    tol_minus: 0.2,
                    tol_plus: 0.2,
                    dist: Distribution::Normal {
                        mean: 0.0,
                        sigma: 0.2 / 3.0,
                    },
                    source: DistributionSource::default(),
                },
                LoopLeg {
                    name: "upright".into(),
                    direction: [0.0, 0.0, 1.0],
                    nominal: 30.0,
                    tol_minus: 0.2,
                    tol_plus: 0.2,
                    dist: Distribution::Normal {
                        mean: 0.0,
                        sigma: 0.2 / 3.0,
                    },
                    source: DistributionSource::default(),
                },
                LoopLeg {
                    name: "diagonal".into(),
                    direction: [-3.0, 0.0, -4.0], // unit: (−0.6, 0, −0.8)
                    nominal: 50.0,
                    tol_minus: 0.3,
                    tol_plus: 0.3,
                    dist: Distribution::Normal {
                        mean: 0.0,
                        sigma: 0.1,
                    },
                    source: DistributionSource::default(),
                },
            ],
            measure_direction: [1.0, 0.0, 0.0],
            requirement: Requirement::between("closure-x", -1.0, 1.0),
        };
        let s = vl.project().unwrap();
        // upright ⟂ x drops; base coeff 1; diagonal coeff −0.6.
        assert_eq!(s.contributors.len(), 2);
        assert_eq!(s.contributors[0].name, "base");
        assert!((s.contributors[0].coeff - 1.0).abs() < 1e-15);
        assert_eq!(s.contributors[1].name, "diagonal");
        assert!((s.contributors[1].coeff + 0.6).abs() < 1e-15);
        // Nominal closure: 40 − 0.6·50 = 10.
        assert!((s.nominal_gap() - 10.0).abs() < 1e-12);
        // σ_G² = (1·0.0667)² + (0.6·0.1)².
        let want = ((0.2f64 / 3.0).powi(2) + 0.06f64.powi(2)).sqrt();
        assert!((rss(&s).unwrap().sigma_gap - want).abs() < 1e-12);
    }

    #[test]
    fn projected_chain_agrees_with_direct_3d_monte_carlo() {
        // Sample the loop in full 3D (sum vector legs, project the
        // closure), and compare with the projected chain's MC: they are
        // the same model, so they must agree within error bars.
        let vl = VectorLoop {
            name: "loop".into(),
            legs: vec![
                LoopLeg {
                    name: "a".into(),
                    direction: [1.0, 0.0, 0.0],
                    nominal: 20.0,
                    tol_minus: 0.15,
                    tol_plus: 0.15,
                    dist: Distribution::Normal {
                        mean: 0.0,
                        sigma: 0.05,
                    },
                    source: DistributionSource::default(),
                },
                LoopLeg {
                    name: "b".into(),
                    direction: [0.6, 0.8, 0.0],
                    nominal: 25.0,
                    tol_minus: 0.15,
                    tol_plus: 0.15,
                    dist: Distribution::Uniform {
                        lo: -0.15,
                        hi: 0.15,
                    },
                    source: DistributionSource::default(),
                },
            ],
            measure_direction: [1.0, 0.0, 0.0],
            requirement: Requirement::between("x-closure", 34.0, 36.0),
        };
        let s = vl.project().unwrap();
        let mc = monte_carlo(
            &s,
            &McOptions {
                n: 60_000,
                seed: 7,
                batches: 16,
            },
        )
        .unwrap();

        // Direct 3D sampling with an independent stream.
        let mut rng = crate::rng::Rng::new(1234);
        let n = 60_000;
        let mut mean = 0.0;
        for i in 0..n {
            let mut x = 0.0;
            for leg in &vl.legs {
                let vn = norm(leg.direction);
                let len = leg.nominal + leg.dist.sample(&mut rng);
                x += leg.direction[0] / vn * len;
            }
            mean += (x - mean) / (i + 1) as f64;
        }
        assert!(
            (mean - mc.mean_gap).abs() < 5.0 * mc.mean_gap_se + 5.0 * mc.mean_gap_se,
            "3D mean {mean} vs projected {}",
            mc.mean_gap
        );
    }

    #[test]
    fn rotation_leg_linearizes_with_lever_arm() {
        // 100 mm lever, ±0.5° (8.727 mrad) angular tolerance, normal 3σ.
        let t = 0.5f64.to_radians();
        let leg = LoopLeg::from_rotation(
            "tilt",
            100.0,
            [1.0, 0.0, 0.0],
            Distribution::Normal {
                mean: 0.0,
                sigma: t / 3.0,
            },
            t,
            t,
        );
        // Displacement tolerance = r·θ = 0.8727 mm; σ = that / 3.
        assert!((leg.tol_plus - 100.0 * t).abs() < 1e-12);
        assert!((leg.dist.sigma() - 100.0 * t / 3.0).abs() < 1e-12);
        // And it composes into a stackup like any other leg.
        let vl = VectorLoop {
            name: "arm".into(),
            legs: vec![leg],
            measure_direction: [1.0, 0.0, 0.0],
            requirement: Requirement::between("tip-x", -1.0, 1.0),
        };
        let s = vl.project().unwrap();
        assert_eq!(s.contributors.len(), 1);
        let r = rss(&s).unwrap();
        assert!((r.sigma_gap - 100.0 * t / 3.0).abs() < 1e-12);
    }

    #[test]
    fn projection_composes_with_plain_contributors() {
        // A projected loop stackup can be extended with plain 1-D
        // contributors (mixed modeling is the common real case).
        let vl = VectorLoop {
            name: "mixed".into(),
            legs: vec![LoopLeg {
                name: "strut".into(),
                direction: [0.0, 0.6, 0.8],
                nominal: 50.0,
                tol_minus: 0.2,
                tol_plus: 0.2,
                dist: Distribution::Normal {
                    mean: 0.0,
                    sigma: 0.2 / 3.0,
                },
                source: DistributionSource::default(),
            }],
            measure_direction: [0.0, 0.0, 1.0],
            requirement: Requirement::at_least("z-gap", 39.0),
        };
        let mut s = vl.project().unwrap();
        s.contributors.push(C::normal(
            "shim",
            1.0,
            0.5,
            0.05,
            SigmaConvention::ThreeSigma,
        ));
        s.validate().unwrap();
        assert!((s.nominal_gap() - (0.8 * 50.0 + 0.5)).abs() < 1e-12);
    }

    #[test]
    fn bad_directions_fail_closed() {
        let mut vl = VectorLoop {
            name: "bad".into(),
            legs: vec![LoopLeg {
                name: "z".into(),
                direction: [0.0, 0.0, 0.0],
                nominal: 1.0,
                tol_minus: 0.1,
                tol_plus: 0.1,
                dist: Distribution::Normal {
                    mean: 0.0,
                    sigma: 0.03,
                },
                source: DistributionSource::default(),
            }],
            measure_direction: [1.0, 0.0, 0.0],
            requirement: Requirement::at_least("g", 0.0),
        };
        assert!(vl.project().is_err(), "zero leg direction");
        vl.legs[0].direction = [1.0, 0.0, 0.0];
        vl.measure_direction = [0.0, 0.0, 0.0];
        assert!(vl.project().is_err(), "zero measure direction");
    }
}
