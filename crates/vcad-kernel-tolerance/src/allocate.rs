//! Tolerance allocation (M2): spend tolerance where it's cheap, save
//! it where it's expensive.
//!
//! The problem: choose drawing tolerances tᵢ for the allocatable
//! contributors to **minimize manufacturing cost subject to a yield
//! floor**,
//!
//! ```text
//! min Σ Cᵢ(tᵢ)   s.t.   yield(σ_G) ≥ Y_target,   tᵢ ∈ [tᵢ_min, tᵢ_max]
//! ```
//!
//! with σ_G² = Σ_alloc (aᵢ/kᵢ)²tᵢ² + Σ_fixed aⱼ²σⱼ² under each
//! contributor's stated tolerance-to-σ convention.
//!
//! Cost-vs-tolerance models are the classical families from the
//! allocation literature (Chase & Greenwood, *Manufacturing Review*
//! 1(1), 1988, survey the lineage): **reciprocal** and
//! **reciprocal-squared** (Spotts, *J. Eng. Industry* 95, 1973)
//! `C = a + b/tᵖ`, and **exponential** (Speckhart, *J. Eng. Industry*
//! 94, 1972) `C = a + b·e^(−t/τ)`. `vcad-kernel-cost` models
//! process-level cost (material + machine minutes + setup), not
//! cost-vs-tolerance, so these curves live here and take their
//! coefficients from quotes or shop data.
//!
//! The exact gradients make the optimization clean: yield is monotone
//! in σ_G, so the constraint is σ_G ≤ σ_max (σ_max found by bisection
//! on the exact Φ yield), and the KKT stationarity condition
//!
//! ```text
//! dCᵢ/dtᵢ + λ · 2wᵢtᵢ = 0,   wᵢ = (aᵢ/kᵢ)²
//! ```
//!
//! has per-contributor **closed forms** for the reciprocal families
//! (tᵢ = (bᵢ/(2λwᵢ))^(1/3), (bᵢ/(λwᵢ))^(1/4)) and a monotone 1-D root
//! for the exponential. The outer loop is a single bisection on λ with
//! box clamping (water-filling style); the solution is exact to
//! bisection tolerance, deterministic, and dependency-free. Costs
//! decrease in t, so the yield constraint is active at the optimum
//! unless every tolerance hits its box maximum first.

use serde::{Deserialize, Serialize};

use crate::analysis::rss;
use crate::capability::yield_within;
use crate::dist::{Distribution, DistributionSource};
use crate::stackup::{Stackup, StackupError};

/// A cost-vs-tolerance model, dollars (or any consistent unit) per
/// part as a function of the ± tolerance in mm.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CostModel {
    /// C(t) = a + b/t — the workhorse reciprocal law.
    Reciprocal {
        /// Fixed cost floor.
        a: f64,
        /// Tightness cost scale (> 0).
        b: f64,
    },
    /// C(t) = a + b/t² (Spotts 1973) — steeper penalty for precision
    /// processes.
    ReciprocalSquared {
        /// Fixed cost floor.
        a: f64,
        /// Tightness cost scale (> 0).
        b: f64,
    },
    /// C(t) = a + b·e^(−t/τ) (Speckhart 1972) — saturating cost with a
    /// characteristic tolerance τ.
    Exponential {
        /// Fixed cost floor.
        a: f64,
        /// Cost at t = 0 above the floor (> 0).
        b: f64,
        /// Characteristic tolerance, mm (> 0).
        tau: f64,
    },
}

impl CostModel {
    /// Cost at tolerance `t` (> 0).
    pub fn cost(&self, t: f64) -> f64 {
        match *self {
            CostModel::Reciprocal { a, b } => a + b / t,
            CostModel::ReciprocalSquared { a, b } => a + b / (t * t),
            CostModel::Exponential { a, b, tau } => a + b * (-t / tau).exp(),
        }
    }

    /// Exact dC/dt (< 0: looser is always cheaper in these families).
    pub fn d_cost_d_tol(&self, t: f64) -> f64 {
        match *self {
            CostModel::Reciprocal { b, .. } => -b / (t * t),
            CostModel::ReciprocalSquared { b, .. } => -2.0 * b / (t * t * t),
            CostModel::Exponential { b, tau, .. } => -(b / tau) * (-t / tau).exp(),
        }
    }

    fn check(&self) -> Result<(), String> {
        let ok = match *self {
            CostModel::Reciprocal { a, b } | CostModel::ReciprocalSquared { a, b } => {
                a.is_finite() && b.is_finite() && b > 0.0
            }
            CostModel::Exponential { a, b, tau } => {
                a.is_finite() && b.is_finite() && tau.is_finite() && b > 0.0 && tau > 0.0
            }
        };
        if ok {
            Ok(())
        } else {
            Err(format!("invalid cost model {self:?}"))
        }
    }

    /// Solve dC/dt + 2λwt = 0 for t > 0 (the KKT stationarity point at
    /// multiplier λ > 0, weight w > 0). Closed form for the reciprocal
    /// families; monotone bisection for the exponential.
    fn stationary_t(&self, lambda: f64, w: f64) -> f64 {
        match *self {
            CostModel::Reciprocal { b, .. } => (b / (2.0 * lambda * w)).cbrt(),
            CostModel::ReciprocalSquared { b, .. } => (b / (lambda * w)).powf(0.25),
            CostModel::Exponential { b, tau, .. } => {
                // g(t) = (b/τ)e^(−t/τ) − 2λwt: strictly decreasing,
                // g(0) > 0 → unique root; bracket by doubling.
                let g = |t: f64| (b / tau) * (-t / tau).exp() - 2.0 * lambda * w * t;
                let mut hi = tau.max(1e-9);
                while g(hi) > 0.0 {
                    hi *= 2.0;
                    if hi > 1e12 {
                        return hi; // λ→0 pathology; caller clamps to box
                    }
                }
                let mut lo = 0.0;
                for _ in 0..200 {
                    let mid = 0.5 * (lo + hi);
                    if g(mid) > 0.0 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                0.5 * (lo + hi)
            }
        }
    }
}

/// One allocatable contributor: which chain member, its cost curve,
/// and the box its tolerance may move in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllocationVar {
    /// Name of the contributor in the stackup (must be a centered-or-
    /// shifted **normal** contributor with an `Assumed` convention —
    /// allocating a vendor-band uniform or a measured distribution is
    /// a category error and fails closed).
    pub contributor: String,
    /// Cost-vs-tolerance model for this dimension.
    pub cost: CostModel,
    /// Tightest tolerance the process can hold, mm (> 0).
    pub t_min: f64,
    /// Loosest tolerance the function allows, mm (≥ t_min).
    pub t_max: f64,
}

/// The allocation result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllocationResult {
    /// Allocated ± tolerances, in `vars` order: (contributor, t, cost).
    pub tolerances: Vec<(String, f64, f64)>,
    /// Total cost at the allocation.
    pub cost: f64,
    /// Baseline: cost of the idealized proportional policy (scale all
    /// allocatable tolerances by one factor to hit the same σ target,
    /// no boxes). The allocator must never lose to this inside the
    /// same boxes; the gap is what optimization bought.
    pub cost_proportional_baseline: f64,
    /// KKT multiplier at the solution (0 ⇒ yield constraint inactive:
    /// everything at t_max already meets the target).
    pub lambda: f64,
    /// σ_G of the allocated chain, mm.
    pub sigma_gap: f64,
    /// The σ_G ceiling that enforces the yield target, mm.
    pub sigma_max: f64,
    /// RSS yield of the allocated chain (≥ target to bisection
    /// tolerance when the constraint binds).
    pub predicted_yield: f64,
    /// The requested yield floor.
    pub target_yield: f64,
    /// The allocated stackup: drawing limits and σ updated together
    /// under each contributor's stated convention.
    pub stackup: Stackup,
}

/// Find σ_max such that the RSS yield equals `target` at fixed mean
/// (yield is strictly decreasing in σ when the mean is inside the
/// limits). Fail-closed when the target is unreachable even at σ → 0.
fn sigma_for_yield(
    mean: f64,
    lower: Option<f64>,
    upper: Option<f64>,
    target: f64,
) -> Result<f64, StackupError> {
    if !(0.0..1.0).contains(&target) {
        return Err(StackupError::BadRequirement(format!(
            "target yield must be in [0, 1), got {target}"
        )));
    }
    // σ → 0 limit: indicator of the mean being inside the limits.
    let best = yield_within(mean, 0.0, lower, upper);
    if best < target {
        return Err(StackupError::Infeasible {
            target_yield: target,
            best_yield: best,
        });
    }
    let mut lo = 0.0f64;
    // Bracket: grow until yield < target.
    let scale = (upper.unwrap_or(mean) - lower.unwrap_or(mean))
        .abs()
        .max(1.0);
    let mut hi = 1e-6 * scale;
    while yield_within(mean, hi, lower, upper) >= target {
        hi *= 2.0;
        if hi > 1e12 * scale {
            // Target ≤ the asymptotic yield (e.g. 0 for two-sided) —
            // effectively unconstrained.
            return Ok(hi);
        }
    }
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if yield_within(mean, mid, lower, upper) >= target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(0.5 * (lo + hi))
}

/// Allocate tolerances: minimize Σ cost subject to RSS yield ≥ target.
///
/// Nominals are held fixed (re-centering is a separate, free move —
/// see the sensitivity module); only the named normal contributors'
/// tolerances move, each keeping its stated σ convention so drawing
/// limits and process σ stay consistent.
pub fn allocate(
    s: &Stackup,
    vars: &[AllocationVar],
    target_yield: f64,
) -> Result<AllocationResult, StackupError> {
    s.validate()?;
    if vars.is_empty() {
        return Err(StackupError::Empty);
    }

    // Resolve each var to (index, weight w = (a/k)², cost model, box).
    struct V {
        idx: usize,
        w: f64,
        k: f64,
        cost: CostModel,
        t_min: f64,
        t_max: f64,
    }
    let mut resolved: Vec<V> = Vec::with_capacity(vars.len());
    let mut seen = std::collections::BTreeSet::new();
    for v in vars {
        if !seen.insert(v.contributor.as_str()) {
            return Err(StackupError::BadName(v.contributor.clone()));
        }
        let idx = s
            .contributors
            .iter()
            .position(|c| c.name == v.contributor)
            .ok_or_else(|| {
                StackupError::NotAllocatable(format!("no contributor named {:?}", v.contributor))
            })?;
        let c = &s.contributors[idx];
        let k = match (&c.dist, &c.source) {
            (Distribution::Normal { .. }, DistributionSource::Assumed { convention }) => {
                convention.k()
            }
            _ => {
                return Err(StackupError::NotAllocatable(format!(
                    "{:?} is not an assumed-normal contributor (vendor bands and \
                     measured distributions are not yours to re-spec)",
                    v.contributor
                )))
            }
        };
        v.cost
            .check()
            .map_err(|reason| StackupError::InvalidDistribution {
                contributor: v.contributor.clone(),
                reason,
            })?;
        if !(v.t_min.is_finite() && v.t_max.is_finite()) || v.t_min <= 0.0 || v.t_max < v.t_min {
            return Err(StackupError::BadRequirement(format!(
                "bad tolerance box [{}, {}] on {:?}",
                v.t_min, v.t_max, v.contributor
            )));
        }
        resolved.push(V {
            idx,
            w: (c.coeff / k) * (c.coeff / k),
            k,
            cost: v.cost,
            t_min: v.t_min,
            t_max: v.t_max,
        });
    }

    // Fixed variance from everything not being allocated.
    let alloc_idx: std::collections::BTreeSet<usize> = resolved.iter().map(|v| v.idx).collect();
    let fixed_var: f64 = s
        .contributors
        .iter()
        .enumerate()
        .filter(|(i, _)| !alloc_idx.contains(i))
        .map(|(_, c)| c.coeff * c.coeff * c.dist.variance())
        .sum();

    // Keep each allocated contributor's centering error in the mean.
    let mean = s.mean_gap();
    let sigma_max = sigma_for_yield(
        mean,
        s.requirement.lower_mm,
        s.requirement.upper_mm,
        target_yield,
    )?;
    let budget = sigma_max * sigma_max - fixed_var;
    let floor: f64 = resolved.iter().map(|v| v.w * v.t_min * v.t_min).sum();
    if budget < floor {
        // Even every allocatable at its tightest cannot reach the
        // target: report the best achievable yield, fail closed.
        let sigma_best = (fixed_var + floor).sqrt();
        return Err(StackupError::Infeasible {
            target_yield,
            best_yield: yield_within(
                mean,
                sigma_best,
                s.requirement.lower_mm,
                s.requirement.upper_mm,
            ),
        });
    }

    // S(λ): allocated variance with box clamping; strictly
    // nonincreasing in λ.
    let clamped = |v: &V, lambda: f64| -> f64 {
        if lambda <= 0.0 {
            return v.t_max;
        }
        v.cost.stationary_t(lambda, v.w).clamp(v.t_min, v.t_max)
    };
    let s_of = |lambda: f64| -> f64 {
        resolved
            .iter()
            .map(|v| {
                let t = clamped(v, lambda);
                v.w * t * t
            })
            .sum()
    };

    // Constraint inactive at λ = 0 (everything at t_max)?
    let (lambda, ts): (f64, Vec<f64>) = if s_of(0.0) <= budget {
        (0.0, resolved.iter().map(|v| v.t_max).collect())
    } else {
        // Bracket λ: double until S(λ) < budget.
        let mut lo = 0.0f64;
        let mut hi = 1e-9;
        while s_of(hi) > budget {
            lo = hi;
            hi *= 4.0;
            if hi > 1e30 {
                return Err(StackupError::BadRequirement(
                    "lambda bracket blew up (degenerate cost models?)".into(),
                ));
            }
        }
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            if s_of(mid) > budget {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let l = 0.5 * (lo + hi);
        (l, resolved.iter().map(|v| clamped(v, l)).collect())
    };

    // Build the allocated stackup: drawing limits and σ move together.
    let mut out = s.clone();
    for (v, &t) in resolved.iter().zip(&ts) {
        let c = &mut out.contributors[v.idx];
        c.tol_minus = t;
        c.tol_plus = t;
        if let Distribution::Normal { sigma, .. } = &mut c.dist {
            *sigma = t / v.k;
        }
    }
    let r = rss(&out)?;

    // Idealized proportional baseline: one scale factor on the current
    // tolerances to hit the same budget, no boxes (documented as the
    // policy every hand allocation actually uses).
    let current_alloc_var: f64 = resolved
        .iter()
        .map(|v| {
            let c = &s.contributors[v.idx];
            c.coeff * c.coeff * c.dist.variance()
        })
        .sum();
    let scale = (budget / current_alloc_var).sqrt();
    let cost_proportional_baseline: f64 = resolved
        .iter()
        .map(|v| {
            let c = &s.contributors[v.idx];
            // Current t from σ under the convention.
            let t0 = c.dist.sigma() * v.k;
            v.cost.cost(t0 * scale)
        })
        .sum();

    let tolerances: Vec<(String, f64, f64)> = resolved
        .iter()
        .zip(&ts)
        .map(|(v, &t)| (s.contributors[v.idx].name.clone(), t, v.cost.cost(t)))
        .collect();
    let cost = tolerances.iter().map(|(_, _, c)| c).sum();

    Ok(AllocationResult {
        tolerances,
        cost,
        cost_proportional_baseline,
        lambda,
        sigma_gap: r.sigma_gap,
        sigma_max,
        predicted_yield: r.yield_estimate,
        target_yield,
        stackup: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dist::SigmaConvention;
    use crate::stackup::{Contributor, Requirement};

    fn chain() -> Stackup {
        let conv = SigmaConvention::ThreeSigma;
        Stackup {
            name: "alloc".into(),
            contributors: vec![
                Contributor::normal("cheap", 1.0, 30.0, 0.3, conv),
                Contributor::normal("mid", -1.0, 20.0, 0.2, conv),
                Contributor::normal("pricey", -1.0, 9.0, 0.1, conv),
            ],
            requirement: Requirement::between("gap", 0.4, 1.6),
        }
    }

    fn vars() -> Vec<AllocationVar> {
        vec![
            AllocationVar {
                contributor: "cheap".into(),
                cost: CostModel::Reciprocal { a: 1.0, b: 0.05 },
                t_min: 0.02,
                t_max: 0.8,
            },
            AllocationVar {
                contributor: "mid".into(),
                cost: CostModel::Reciprocal { a: 1.0, b: 0.2 },
                t_min: 0.02,
                t_max: 0.8,
            },
            AllocationVar {
                contributor: "pricey".into(),
                cost: CostModel::Reciprocal { a: 1.0, b: 0.8 },
                t_min: 0.02,
                t_max: 0.8,
            },
        ]
    }

    #[test]
    fn cost_models_and_gradients() {
        let models = [
            CostModel::Reciprocal { a: 1.0, b: 0.5 },
            CostModel::ReciprocalSquared { a: 1.0, b: 0.05 },
            CostModel::Exponential {
                a: 1.0,
                b: 5.0,
                tau: 0.1,
            },
        ];
        for m in models {
            // Decreasing cost; exact gradient matches FD.
            let h = 1e-7;
            for t in [0.02, 0.1, 0.5] {
                assert!(m.cost(t) > m.cost(t + 0.01), "{m:?} not decreasing");
                let fd = (m.cost(t + h) - m.cost(t - h)) / (2.0 * h);
                let exact = m.d_cost_d_tol(t);
                assert!(
                    (fd - exact).abs() < 1e-4 * exact.abs(),
                    "{m:?} at {t}: fd {fd} vs exact {exact}"
                );
            }
        }
    }

    #[test]
    fn reciprocal_family_matches_closed_form_lagrange() {
        // Pure reciprocal costs, no boxes binding: KKT gives
        // tᵢ = (bᵢ/(2λwᵢ))^(1/3); with all aᵢ² = 1, k = 3:
        // w = 1/9 for every i, so tᵢ ∝ bᵢ^(1/3) and λ comes from
        // Σ w tᵢ² = budget. Verify the allocator lands on it.
        let s = chain();
        let target = 0.9973;
        let r = allocate(&s, &vars(), target).unwrap();
        assert!(r.lambda > 0.0, "constraint must bind");
        let w = 1.0 / 9.0;
        let bs = [0.05f64, 0.2, 0.8];
        // λ from the constraint in closed form:
        // Σ w (b/(2λw))^(2/3) = budget →
        // λ^(2/3) = Σ w (b/(2w))^(2/3) / budget.
        let budget = r.sigma_max * r.sigma_max;
        let num: f64 = bs.iter().map(|b| w * (b / (2.0 * w)).powf(2.0 / 3.0)).sum();
        let lambda_cf = (num / budget).powf(1.5);
        assert!(
            (r.lambda - lambda_cf).abs() / lambda_cf < 1e-6,
            "λ {} vs closed form {lambda_cf}",
            r.lambda
        );
        for ((_, t, _), b) in r.tolerances.iter().zip(bs) {
            let t_cf = (b / (2.0 * lambda_cf * w)).cbrt();
            assert!(
                (t - t_cf).abs() / t_cf < 1e-6,
                "t {t} vs closed form {t_cf}"
            );
        }
        // KKT stationarity residual ≈ 0 for interior points.
        for (v, (_, t, _)) in vars().iter().zip(&r.tolerances) {
            let resid = v.cost.d_cost_d_tol(*t) + 2.0 * r.lambda * w * t;
            assert!(
                resid.abs() < 1e-6 * v.cost.d_cost_d_tol(*t).abs(),
                "KKT residual {resid}"
            );
        }
        // The yield constraint is met to bisection accuracy.
        assert!(r.predicted_yield >= target - 1e-9, "{}", r.predicted_yield);
        assert!((r.sigma_gap - r.sigma_max).abs() < 1e-9 * r.sigma_max);
    }

    #[test]
    fn allocation_beats_proportional_scaling() {
        // With unequal b's the optimizer must beat the one-knob policy
        // at the same yield: it loosens the cheap dim and tightens the
        // pricey one.
        let s = chain();
        let r = allocate(&s, &vars(), 0.9973).unwrap();
        assert!(
            r.cost < r.cost_proportional_baseline - 1e-6,
            "allocated {} vs proportional {}",
            r.cost,
            r.cost_proportional_baseline
        );
        // The cheap contributor should get a looser tolerance than the
        // pricey one scaled by the b-ratio direction.
        let t_cheap = r.tolerances[0].1;
        let t_pricey = r.tolerances[2].1;
        assert!(
            t_cheap < t_pricey,
            "reciprocal optimum: t ∝ b^(1/3), so bigger b ⇒ looser: {t_cheap} vs {t_pricey}"
        );
    }

    #[test]
    fn boxes_clamp_and_the_rest_compensate() {
        let s = chain();
        let free = allocate(&s, &vars(), 0.9973).unwrap();
        // Free optimum (t ∝ b^(1/3)): cheap ≈ 0.19, pricey ≈ 0.48.

        // Case 1 — t_min binds: the cheap dim's process can't hold
        // tighter than 0.25 (free optimum wants 0.19). It's forced
        // looser than optimal, eating variance budget, so the others
        // must TIGHTEN to keep the yield.
        let mut v = vars();
        v[0].t_min = 0.25;
        let boxed = allocate(&s, &v, 0.9973).unwrap();
        assert!(free.tolerances[0].1 < 0.25, "sanity: floor binds");
        assert!((boxed.tolerances[0].1 - 0.25).abs() < 1e-9);
        assert!(boxed.tolerances[1].1 < free.tolerances[1].1);
        assert!(boxed.tolerances[2].1 < free.tolerances[2].1);
        assert!(boxed.predicted_yield >= 0.9973 - 1e-9);
        assert!(boxed.cost > free.cost, "boxes never help");

        // Case 2 — t_max binds: the pricey dim's drawing caps at 0.30
        // (free optimum wants ≈ 0.48). It's forced tighter than
        // optimal, freeing budget, so the others go LOOSER.
        let mut v = vars();
        v[2].t_max = 0.30;
        let boxed = allocate(&s, &v, 0.9973).unwrap();
        assert!(free.tolerances[2].1 > 0.30, "sanity: cap binds");
        assert!((boxed.tolerances[2].1 - 0.30).abs() < 1e-9);
        assert!(boxed.tolerances[0].1 > free.tolerances[0].1);
        assert!(boxed.tolerances[1].1 > free.tolerances[1].1);
        assert!(boxed.predicted_yield >= 0.9973 - 1e-9);
        assert!(boxed.cost > free.cost);
    }

    #[test]
    fn exponential_model_allocates_and_meets_kkt() {
        let s = chain();
        let v: Vec<AllocationVar> = vars()
            .into_iter()
            .map(|mut v| {
                v.cost = CostModel::Exponential {
                    a: 1.0,
                    b: match v.contributor.as_str() {
                        "cheap" => 2.0,
                        "mid" => 6.0,
                        _ => 20.0,
                    },
                    tau: 0.08,
                };
                v
            })
            .collect();
        let r = allocate(&s, &v, 0.9973).unwrap();
        assert!(r.predicted_yield >= 0.9973 - 1e-9);
        let w = 1.0 / 9.0;
        for (var, (_, t, _)) in v.iter().zip(&r.tolerances) {
            if *t > var.t_min + 1e-9 && *t < var.t_max - 1e-9 {
                let resid = var.cost.d_cost_d_tol(*t) + 2.0 * r.lambda * w * t;
                assert!(
                    resid.abs() < 1e-5 * var.cost.d_cost_d_tol(*t).abs().max(1e-12),
                    "KKT residual {resid} at t {t}"
                );
            }
        }
    }

    #[test]
    fn inactive_constraint_returns_all_loose() {
        // A very lax target: everything sits at t_max, λ = 0.
        let s = chain();
        let r = allocate(&s, &vars(), 0.5).unwrap();
        assert_eq!(r.lambda, 0.0);
        for (v, (_, t, _)) in vars().iter().zip(&r.tolerances) {
            assert_eq!(*t, v.t_max);
        }
        assert!(r.predicted_yield >= 0.5);
    }

    #[test]
    fn mixed_chain_allocates_only_the_normals() {
        // Fixed vendor bands stay fixed; their variance eats budget.
        let conv = SigmaConvention::ThreeSigma;
        let s = Stackup {
            name: "mixed".into(),
            contributors: vec![
                Contributor::normal("machined", 1.0, 30.0, 0.3, conv),
                Contributor::uniform("vendor", -1.0, 15.0, 0.12, 0.0),
                Contributor::normal("machined2", -1.0, 14.4, 0.2, conv),
            ],
            requirement: Requirement::between("gap", 0.2, 1.0),
        };
        let v = vec![
            AllocationVar {
                contributor: "machined".into(),
                cost: CostModel::Reciprocal { a: 0.0, b: 0.1 },
                t_min: 0.02,
                t_max: 0.6,
            },
            AllocationVar {
                contributor: "machined2".into(),
                cost: CostModel::Reciprocal { a: 0.0, b: 0.1 },
                t_min: 0.02,
                t_max: 0.6,
            },
        ];
        let r = allocate(&s, &v, 0.99).unwrap();
        assert!(r.predicted_yield >= 0.99 - 1e-9);
        // The vendor band is untouched.
        assert_eq!(r.stackup.contributors[1], s.contributors[1]);
        // Allocating the vendor band is a category error.
        let bad = vec![AllocationVar {
            contributor: "vendor".into(),
            cost: CostModel::Reciprocal { a: 0.0, b: 0.1 },
            t_min: 0.02,
            t_max: 0.6,
        }];
        assert!(matches!(
            allocate(&s, &bad, 0.99),
            Err(StackupError::NotAllocatable(_))
        ));
    }

    #[test]
    fn infeasible_targets_fail_closed_with_the_best_yield() {
        let s = chain();
        let mut v = vars();
        for var in &mut v {
            var.t_min = 0.25; // can't tighten below current-ish levels
            var.t_max = 0.4;
        }
        let err = allocate(&s, &v, 0.999999999).unwrap_err();
        match err {
            StackupError::Infeasible {
                target_yield,
                best_yield,
            } => {
                assert!(target_yield > best_yield);
                assert!(best_yield > 0.9, "still a real yield: {best_yield}");
            }
            other => panic!("wrong error: {other:?}"),
        }
        // A mean outside the limits can't reach any yield ≥ 0.5.
        let mut hopeless = chain();
        hopeless.requirement = Requirement::between("gap", 2.0, 3.0);
        assert!(matches!(
            allocate(&hopeless, &vars(), 0.9),
            Err(StackupError::Infeasible { .. })
        ));
    }
}
