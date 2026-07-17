//! Projected gradient ascent for two-arm splitter objectives.
//!
//! Maximizes the balanced figure of merit
//!
//! ```text
//! F = J_a + J_b − γ·(J_a − J_b)²/(J_a + J_b + ϵ)
//! ```
//!
//! (total mode power into both arms, penalized for imbalance; the
//! normalization in the penalty keeps γ scale-free) over densities
//! ρ ∈ [0, 1]ᶜ, using per-arm adjoint gradients chained through the
//! topology parameterization.
//!
//! Mechanics, each encoding a lesson:
//!
//! - **Scale-invariant steps**: the update is `ρ += α·g/‖g‖∞`, so `α` is
//!   a density step, independent of the objective's absolute scale.
//! - **Scale-invariant stopping**: relative F improvement over a trailing
//!   window, never an absolute threshold.
//! - **Monotone acceptance**: a step that lowers F is reverted and the
//!   step size halved; growth is slow (×1.2) — no line-search machinery,
//!   just never walking downhill.
//! - **β schedule**: the projection sharpness ramps through
//!   [`OptimizeOptions::betas`]; each ramp re-evaluates F under the new
//!   parameterization before comparing (an F change from re-projection is
//!   not a step failure).
//!
//! The evaluation callback owns the physics (and its frozen
//! discretization); this module never sees a `Simulation`.

use crate::design::TopologyParam;

/// Per-arm objective values and density-space gradients for the current
/// design.
#[derive(Debug, Clone)]
pub struct SplitEval {
    /// Arm-A objective J_a = |A_a|².
    pub j_a: f64,
    /// Arm-B objective J_b = |A_b|².
    pub j_b: f64,
    /// dJ_a/dρ (chained through the parameterization).
    pub grad_a: Vec<f64>,
    /// dJ_b/dρ.
    pub grad_b: Vec<f64>,
}

/// Optimizer configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct OptimizeOptions {
    /// Projection-sharpness schedule; each β gets up to
    /// `iters_per_beta` iterations.
    pub betas: Vec<f64>,
    /// Iteration cap per β stage.
    pub iters_per_beta: usize,
    /// Initial density step (per iteration, in ρ units).
    pub step0: f64,
    /// Imbalance penalty weight γ.
    pub balance_gamma: f64,
    /// Stop a stage early when the relative F improvement over the last
    /// three accepted steps falls below this.
    pub rel_tol: f64,
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            betas: vec![4.0, 8.0, 16.0, 32.0],
            iters_per_beta: 12,
            step0: 0.08,
            balance_gamma: 1.0,
            rel_tol: 1e-4,
        }
    }
}

/// One optimizer iteration record (for traces, tables, and the design
/// receipt's provenance).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IterRecord {
    /// Global iteration index.
    pub iter: usize,
    /// β in effect.
    pub beta: f64,
    /// Arm objectives.
    pub j_a: f64,
    /// Arm objectives.
    pub j_b: f64,
    /// Balanced figure of merit.
    pub fom: f64,
    /// Step size after this iteration's accept/reject.
    pub step: f64,
    /// Whether the step was accepted.
    pub accepted: bool,
}

fn fom(j_a: f64, j_b: f64, gamma: f64) -> f64 {
    let total = j_a + j_b;
    j_a + j_b - gamma * (j_a - j_b) * (j_a - j_b) / (total + f64::MIN_POSITIVE)
}

/// d(FoM)/dJ_a and d(FoM)/dJ_b (the penalty's quotient rule, exact).
fn fom_weights(j_a: f64, j_b: f64, gamma: f64) -> (f64, f64) {
    let t = j_a + j_b + f64::MIN_POSITIVE;
    let d = j_a - j_b;
    let common = gamma * d * d / (t * t);
    (
        1.0 - gamma * 2.0 * d / t + common,
        1.0 + gamma * 2.0 * d / t + common,
    )
}

/// Run the ascent. `eval` computes per-arm objectives and density
/// gradients for the design it is handed (with the β currently set on
/// it). Returns the iteration trace; `topo` holds the optimized design.
pub fn maximize_split(
    eval: &mut dyn FnMut(&TopologyParam) -> SplitEval,
    topo: &mut TopologyParam,
    opts: &OptimizeOptions,
) -> Vec<IterRecord> {
    let mut trace = Vec::new();
    let mut step = opts.step0;
    let mut iter = 0usize;
    for &beta in &opts.betas {
        topo.beta = beta;
        let mut cur = eval(topo);
        let mut f_cur = fom(cur.j_a, cur.j_b, opts.balance_gamma);
        let mut recent: Vec<f64> = vec![f_cur];
        for _ in 0..opts.iters_per_beta {
            // Ascent direction under the balanced FoM.
            let (wa, wb) = fom_weights(cur.j_a, cur.j_b, opts.balance_gamma);
            let g: Vec<f64> = cur
                .grad_a
                .iter()
                .zip(cur.grad_b.iter())
                .map(|(a, b)| wa * a + wb * b)
                .collect();
            let gmax = g.iter().fold(0.0f64, |m, v| m.max(v.abs()));
            if gmax == 0.0 {
                break;
            }
            let old_rho = topo.rho.clone();
            for (r, gi) in topo.rho.iter_mut().zip(g.iter()) {
                *r = (*r + step * gi / gmax).clamp(0.0, 1.0);
            }
            let cand = eval(topo);
            let f_cand = fom(cand.j_a, cand.j_b, opts.balance_gamma);
            let accepted = f_cand > f_cur;
            if accepted {
                cur = cand;
                f_cur = f_cand;
                step = (step * 1.2).min(0.25);
            } else {
                topo.rho = old_rho;
                step *= 0.5;
            }
            trace.push(IterRecord {
                iter,
                beta,
                j_a: cur.j_a,
                j_b: cur.j_b,
                fom: f_cur,
                step,
                accepted,
            });
            iter += 1;
            recent.push(f_cur);
            if recent.len() > 4 {
                recent.remove(0);
            }
            let span = recent.last().unwrap() - recent.first().unwrap();
            if recent.len() == 4 && span.abs() <= opts.rel_tol * f_cur.abs() {
                break;
            }
            if step < 1e-4 {
                break;
            }
        }
    }
    trace
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adjoint::DesignRegion;

    /// A cheap surrogate with a known optimum: J_a and J_b are smooth
    /// quadratics of the mean density, maximized (and balanced) at
    /// ρ̄ = 0.7. The optimizer must climb there without any FDTD.
    #[test]
    fn optimizer_climbs_and_balances_a_surrogate() {
        let region = DesignRegion {
            i0: 0,
            i1: 4,
            j0: 0,
            j1: 4,
        };
        let mut topo = TopologyParam::uniform(region, 0.30, 2.0, 12.0);
        topo.filter_radius_cells = 0.0; // surrogate acts on raw ρ
        let mut eval = |t: &TopologyParam| {
            let n = t.rho.len() as f64;
            let mean: f64 = t.rho.iter().sum::<f64>() / n;
            let j_a = 1.0 - (mean - 0.7) * (mean - 0.7);
            let j_b = 0.98 - (mean - 0.7) * (mean - 0.7);
            // dJ/dρ_i = −2(mean − 0.7)/n for both arms; the chain through
            // projection is exercised in the FDTD tests, not here.
            let g = -2.0 * (mean - 0.7) / n;
            SplitEval {
                j_a,
                j_b,
                grad_a: vec![g; t.rho.len()],
                grad_b: vec![g; t.rho.len()],
            }
        };
        let opts = OptimizeOptions {
            betas: vec![4.0, 8.0],
            iters_per_beta: 40,
            step0: 0.05,
            balance_gamma: 1.0,
            rel_tol: 1e-9,
        };
        let trace = maximize_split(&mut eval, &mut topo, &opts);
        assert!(!trace.is_empty());
        let mean: f64 = topo.rho.iter().sum::<f64>() / topo.rho.len() as f64;
        assert!(
            (mean - 0.7).abs() < 0.02,
            "optimizer did not reach the surrogate optimum: mean ρ = {mean}"
        );
        // Monotone accepted FoM.
        let accepted: Vec<f64> = trace.iter().filter(|r| r.accepted).map(|r| r.fom).collect();
        for w in accepted.windows(2) {
            assert!(w[1] >= w[0] - 1e-12, "accepted FoM went downhill");
        }
    }

    #[test]
    fn fom_weights_match_fd() {
        let (ja, jb, gamma) = (0.83, 0.61, 1.3);
        let h = 1e-7;
        let (wa, wb) = fom_weights(ja, jb, gamma);
        let fa = (fom(ja + h, jb, gamma) - fom(ja - h, jb, gamma)) / (2.0 * h);
        let fb = (fom(ja, jb + h, gamma) - fom(ja, jb - h, gamma)) / (2.0 * h);
        assert!((wa - fa).abs() < 1e-6, "wa {wa} vs fd {fa}");
        assert!((wb - fb).abs() < 1e-6, "wb {wb} vs fd {fb}");
    }
}
