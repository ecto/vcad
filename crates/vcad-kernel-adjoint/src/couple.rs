//! The block-Jacobi coupled-adjoint driver — SU2's four operations.
//!
//! SU2 builds its entire multiphysics adjoint on four high-level calls:
//! `ComputeAdjoints(j)`, `Iterate(k)`, `AddExternal(k)`, and
//! `UpdateCrossTerm(j,k)`. The payoff is that adding a discipline is O(1)
//! work rather than O(z²) pairwise integrations — no part of the driver
//! knows how many disciplines there are, what physics they solve, or what
//! kind of interface joins them.
//!
//! Here those four collapse into two traits and a loop:
//!
//! - [`AdjointDiscipline::iterate`] is `ComputeAdjoints` + `Iterate`: one
//!   sweep of `λ(k) ← (∂G(k)/∂u(k))ᵀ λ(k) + external`.
//! - [`CrossTerm::accumulate`] is `UpdateCrossTerm`, and summing every
//!   registered cross term into a discipline's right-hand side is
//!   `AddExternal`.
//!
//! The fixed-point form is deliberate. SU2 shows that a residual-based
//! adjoint preconditioned with `M = Pᵀ` — the transpose of the primal's
//! own preconditioner — produces *the same iteration* as the fixed-point
//! adjoint. Two consequences, both of which this module leans on:
//!
//! 1. The adjoint converges at roughly the primal's rate. That makes a
//!    rate mismatch a cheap wiring check rather than a mystery, and
//!    [`CoupledAdjoint::rate`] reports it.
//! 2. A solver that already iterates to a fixed point needs no
//!    checkpointing to be differentiated — only its transposed sweep.
//!
//! **Honesty:** this is block Jacobi with inner sweeps, not a Newton
//! solve on the coupled system. It converges when the coupled fixed-point
//! iteration converges, which for loosely-coupled interfaces is the same
//! condition the *primal* segregated loop already lives under. Strongly
//! coupled interfaces (added-mass FSI is the classic) can diverge here
//! exactly as they diverge in the primal, and the driver reports that as
//! [`CoupleError::NotConverged`] rather than returning a plausible
//! wrong answer.

use crate::ledger::{Completeness, CouplingLedger};

/// One discipline's own adjoint — the diagonal block.
pub trait AdjointDiscipline {
    /// Display name. Must match the ledger's name for this index.
    fn name(&self) -> &str;

    /// Dimension of this discipline's adjoint vector.
    fn dim(&self) -> usize;

    /// The objective seed `∂J/∂u(k)`, written into `out` (length
    /// [`Self::dim`]). Disciplines the objective does not touch write
    /// zeros.
    fn seed(&self, out: &mut [f64]);

    /// One fixed-point sweep of this discipline's own adjoint:
    ///
    /// ```text
    /// out ← (∂G(k)/∂u(k))ᵀ · lambda + external
    /// ```
    ///
    /// `external` already contains the objective seed plus every cross
    /// term from the other disciplines. Implementations must not assume
    /// `out` is zeroed.
    fn iterate(&self, lambda: &[f64], external: &[f64], out: &mut [f64]);
}

/// One off-diagonal block: how discipline `source`'s adjoint feeds
/// discipline `target`'s right-hand side.
pub trait CrossTerm {
    /// Source discipline index `j`.
    fn source(&self) -> usize;

    /// Target discipline index `k`.
    fn target(&self) -> usize;

    /// Accumulate `(∂G(source)/∂u(target))ᵀ · lambda_source` into `out`.
    ///
    /// Accumulate — do not overwrite. Several cross terms land in the same
    /// buffer.
    fn accumulate(&self, lambda_source: &[f64], out: &mut [f64]);
}

/// Driver options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoupleOptions {
    /// Outer iterations over the disciplines.
    pub max_outer: usize,
    /// Inner sweeps per discipline per outer iteration. More inner sweeps
    /// converge each discipline further before the cross terms are
    /// refreshed; SU2 exposes the same knob.
    pub inner_iters: usize,
    /// Convergence tolerance on the largest adjoint change between outer
    /// iterations, relative to the largest adjoint magnitude.
    pub tol: f64,
}

impl Default for CoupleOptions {
    fn default() -> Self {
        CoupleOptions {
            max_outer: 200,
            inner_iters: 1,
            tol: 1e-10,
        }
    }
}

/// Why a coupled adjoint solve failed. Fail-closed throughout.
#[derive(Debug, Clone, PartialEq)]
pub enum CoupleError {
    /// Discipline count disagrees with the ledger.
    ArityMismatch {
        /// Disciplines handed to the driver.
        disciplines: usize,
        /// Disciplines the ledger describes.
        ledger: usize,
    },
    /// A discipline's name does not match the ledger entry at its index.
    NameMismatch {
        /// Index.
        index: usize,
        /// Name the discipline reports.
        discipline: String,
        /// Name the ledger has.
        ledger: String,
    },
    /// The ledger and the registered cross terms disagree: a block marked
    /// `Implemented` has no `CrossTerm`, or a `CrossTerm` was registered
    /// for a block the ledger calls `Absent`/`Frozen`/`Missing`.
    ///
    /// This is the check that makes the ledger load-bearing rather than
    /// decorative.
    LedgerMismatch {
        /// Human description of every disagreement found.
        problems: Vec<String>,
    },
    /// A cross term's `target` buffer length disagrees with the target
    /// discipline's `dim()`.
    DimensionMismatch {
        /// Source index.
        source: usize,
        /// Target index.
        target: usize,
        /// What the discipline reports.
        expected: usize,
        /// What arrived.
        got: usize,
    },
    /// The coupled fixed point did not converge in budget.
    NotConverged {
        /// Outer iterations run.
        iters: usize,
        /// Final relative change.
        residual: f64,
        /// Tolerance it failed to meet.
        tol: f64,
    },
    /// An adjoint went non-finite — the coupled iteration diverged.
    Diverged {
        /// Discipline whose adjoint blew up.
        discipline: String,
        /// Outer iteration it happened on.
        iter: usize,
    },
    /// The ledger rolls up as [`Completeness::Incomplete`]. Refused: a
    /// gradient with a missing cross term can carry the wrong sign, and
    /// there is no safe way to hand one to a caller who asked for a
    /// derivative.
    IncompleteLedger(Completeness),
}

impl std::fmt::Display for CoupleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoupleError::ArityMismatch {
                disciplines,
                ledger,
            } => write!(
                f,
                "{disciplines} disciplines but the ledger describes {ledger}"
            ),
            CoupleError::NameMismatch {
                index,
                discipline,
                ledger,
            } => write!(
                f,
                "discipline {index} calls itself {discipline:?}, ledger says {ledger:?}"
            ),
            CoupleError::LedgerMismatch { problems } => {
                write!(
                    f,
                    "ledger does not match registered cross terms: {}",
                    problems.join("; ")
                )
            }
            CoupleError::DimensionMismatch {
                source,
                target,
                expected,
                got,
            } => write!(
                f,
                "cross term ({source} -> {target}) writes {got} values, target expects {expected}"
            ),
            CoupleError::NotConverged {
                iters,
                residual,
                tol,
            } => write!(
                f,
                "coupled adjoint not converged after {iters} outer iterations: \
                 residual {residual:.3e} > tol {tol:.3e}"
            ),
            CoupleError::Diverged { discipline, iter } => write!(
                f,
                "coupled adjoint diverged: {discipline} went non-finite at outer iteration {iter}"
            ),
            CoupleError::IncompleteLedger(c) => {
                write!(
                    f,
                    "refusing to solve an incomplete coupled adjoint — {}",
                    c.summary()
                )
            }
        }
    }
}

impl std::error::Error for CoupleError {}

/// A converged coupled adjoint.
#[derive(Debug, Clone, PartialEq)]
pub struct CoupledAdjoint {
    /// `λ(k)` per discipline, in ledger index order.
    pub lambdas: Vec<Vec<f64>>,
    /// Outer iterations to convergence.
    pub outer_iters: usize,
    /// Final relative change.
    pub residual: f64,
    /// Per-outer-iteration relative change, for the convergence-rate
    /// check against the primal.
    pub history: Vec<f64>,
    /// Completeness of the ledger this was solved under.
    pub completeness: Completeness,
}

impl CoupledAdjoint {
    /// Geometric convergence rate estimated from the last half of the
    /// history — the ratio by which the residual shrinks per outer
    /// iteration.
    ///
    /// SU2's `M = Pᵀ` equivalence says this should track the *primal*
    /// coupled loop's rate. A coupled adjoint that converges markedly
    /// slower than its primal is the signature of a wrong or missing
    /// cross term, and this number is how you notice.
    pub fn rate(&self) -> Option<f64> {
        let h: Vec<f64> = self
            .history
            .iter()
            .copied()
            .filter(|r| r.is_finite() && *r > 0.0)
            .collect();
        if h.len() < 4 {
            return None;
        }
        let tail = &h[h.len() / 2..];
        let ratios: Vec<f64> = tail.windows(2).map(|w| w[1] / w[0]).collect();
        if ratios.is_empty() {
            return None;
        }
        Some(ratios.iter().sum::<f64>() / ratios.len() as f64)
    }
}

/// Solve the coupled discrete adjoint by block Jacobi over the
/// disciplines — SU2's multi-disciplinary algorithm.
///
/// ```text
/// for outer:
///   for k in disciplines:
///     external(k) = seed(k) + Σ_{j≠k} (∂G(j)/∂u(k))ᵀ λ(j)   // AddExternal
///     repeat inner_iters:
///       λ(k) ← (∂G(k)/∂u(k))ᵀ λ(k) + external(k)             // Iterate
/// ```
///
/// Refuses to run against an [`Completeness::Incomplete`] ledger, and
/// refuses to run when the ledger and the registered cross terms
/// disagree.
pub fn solve_coupled(
    disciplines: &[&dyn AdjointDiscipline],
    cross_terms: &[&dyn CrossTerm],
    ledger: &CouplingLedger,
    opts: &CoupleOptions,
) -> Result<CoupledAdjoint, CoupleError> {
    let z = disciplines.len();
    if z != ledger.len() {
        return Err(CoupleError::ArityMismatch {
            disciplines: z,
            ledger: ledger.len(),
        });
    }
    for (i, d) in disciplines.iter().enumerate() {
        if d.name() != ledger.disciplines[i] {
            return Err(CoupleError::NameMismatch {
                index: i,
                discipline: d.name().to_string(),
                ledger: ledger.disciplines[i].clone(),
            });
        }
    }

    let completeness = ledger.completeness();
    if !completeness.may_optimize() {
        return Err(CoupleError::IncompleteLedger(completeness));
    }

    // The ledger must describe exactly the cross terms that were handed
    // over. This is what stops the ledger from drifting into decoration.
    let mut registered = vec![false; z * z];
    let mut problems = Vec::new();
    for ct in cross_terms {
        let (s, t) = (ct.source(), ct.target());
        if s >= z || t >= z {
            problems.push(format!(
                "cross term ({s} -> {t}) out of range for {z} disciplines"
            ));
            continue;
        }
        if s == t {
            problems.push(format!(
                "cross term ({s} -> {t}) is a diagonal block; that belongs in AdjointDiscipline::iterate"
            ));
            continue;
        }
        if registered[s * z + t] {
            problems.push(format!(
                "duplicate cross term ({} -> {})",
                ledger.disciplines[s], ledger.disciplines[t]
            ));
        }
        registered[s * z + t] = true;
    }
    for (s, t, status) in ledger.cross_blocks() {
        let have = registered[s * z + t];
        let want = status.expects_registration();
        if want && !have {
            problems.push(format!(
                "ledger marks d{}/d{} implemented but no cross term was registered",
                ledger.disciplines[s], ledger.disciplines[t]
            ));
        }
        if !want && have {
            problems.push(format!(
                "a cross term was registered for d{}/d{}, which the ledger does not mark implemented",
                ledger.disciplines[s], ledger.disciplines[t]
            ));
        }
    }
    if !problems.is_empty() {
        return Err(CoupleError::LedgerMismatch { problems });
    }

    let dims: Vec<usize> = disciplines.iter().map(|d| d.dim()).collect();
    let mut lambdas: Vec<Vec<f64>> = dims.iter().map(|&n| vec![0.0; n]).collect();
    let mut seeds: Vec<Vec<f64>> = dims.iter().map(|&n| vec![0.0; n]).collect();
    for (k, d) in disciplines.iter().enumerate() {
        d.seed(&mut seeds[k]);
    }

    let mut external = vec![0.0_f64; *dims.iter().max().unwrap_or(&0)];
    let mut next = vec![0.0_f64; *dims.iter().max().unwrap_or(&0)];
    let mut history = Vec::with_capacity(opts.max_outer);
    let mut residual = f64::INFINITY;

    for outer in 1..=opts.max_outer {
        let mut max_delta = 0.0_f64;
        let mut max_mag = 0.0_f64;

        for k in 0..z {
            let nk = dims[k];
            // AddExternal(k): objective seed plus every cross term.
            external[..nk].copy_from_slice(&seeds[k][..nk]);
            for ct in cross_terms {
                if ct.target() != k {
                    continue;
                }
                let before = nk;
                let ext = &mut external[..nk];
                ct.accumulate(&lambdas[ct.source()], ext);
                if ext.len() != before {
                    return Err(CoupleError::DimensionMismatch {
                        source: ct.source(),
                        target: k,
                        expected: before,
                        got: ext.len(),
                    });
                }
            }

            // Iterate(k), inner_iters times.
            for _ in 0..opts.inner_iters {
                disciplines[k].iterate(&lambdas[k], &external[..nk], &mut next[..nk]);
                for (i, v) in next[..nk].iter().enumerate() {
                    if !v.is_finite() {
                        return Err(CoupleError::Diverged {
                            discipline: disciplines[k].name().to_string(),
                            iter: outer,
                        });
                    }
                    let d = (v - lambdas[k][i]).abs();
                    if d > max_delta {
                        max_delta = d;
                    }
                    if v.abs() > max_mag {
                        max_mag = v.abs();
                    }
                    lambdas[k][i] = *v;
                }
            }
        }

        residual = if max_mag > 0.0 {
            max_delta / max_mag
        } else {
            max_delta
        };
        history.push(residual);
        if residual < opts.tol {
            return Ok(CoupledAdjoint {
                lambdas,
                outer_iters: outer,
                residual,
                history,
                completeness,
            });
        }
    }

    Err(CoupleError::NotConverged {
        iters: opts.max_outer,
        residual,
        tol: opts.tol,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{BlockMethod, BlockStatus};

    /// A scalar discipline: `G(k) = a·u(k) + Σ c_j·u(j)`, so the
    /// diagonal adjoint sweep is `λ ← a·λ + external`.
    struct Scalar {
        name: String,
        a: f64,
        seed: f64,
    }

    impl AdjointDiscipline for Scalar {
        fn name(&self) -> &str {
            &self.name
        }
        fn dim(&self) -> usize {
            1
        }
        fn seed(&self, out: &mut [f64]) {
            out[0] = self.seed;
        }
        fn iterate(&self, lambda: &[f64], external: &[f64], out: &mut [f64]) {
            out[0] = self.a * lambda[0] + external[0];
        }
    }

    struct ScalarCross {
        s: usize,
        t: usize,
        c: f64,
    }

    impl CrossTerm for ScalarCross {
        fn source(&self) -> usize {
            self.s
        }
        fn target(&self) -> usize {
            self.t
        }
        fn accumulate(&self, lambda_source: &[f64], out: &mut [f64]) {
            out[0] += self.c * lambda_source[0];
        }
    }

    /// A 2x2 coupled system with a closed-form answer:
    ///   λ0 = a0 λ0 + c10 λ1 + s0
    ///   λ1 = a1 λ1 + c01 λ0 + s1
    /// Solving: (1-a0) λ0 - c10 λ1 = s0 ; -c01 λ0 + (1-a1) λ1 = s1.
    fn exact(a0: f64, a1: f64, c01: f64, c10: f64, s0: f64, s1: f64) -> (f64, f64) {
        let (p, q) = (1.0 - a0, 1.0 - a1);
        let det = p * q - c10 * c01;
        ((s0 * q + c10 * s1) / det, (p * s1 + c01 * s0) / det)
    }

    fn ledger_2x2_coupled() -> CouplingLedger {
        let mut l = CouplingLedger::new(["d0", "d1"], BlockMethod::Analytic).unwrap();
        l.set(0, 1, BlockStatus::implemented(BlockMethod::Analytic))
            .unwrap();
        l.set(1, 0, BlockStatus::implemented(BlockMethod::Analytic))
            .unwrap();
        l
    }

    #[test]
    fn coupled_fixed_point_hits_the_closed_form() {
        let (a0, a1, c01, c10, s0, s1) = (0.5, 0.4, 0.2, 0.3, 1.0, -2.0);
        let d0 = Scalar {
            name: "d0".into(),
            a: a0,
            seed: s0,
        };
        let d1 = Scalar {
            name: "d1".into(),
            a: a1,
            seed: s1,
        };
        // Cross term (source j -> target k): d1's adjoint feeds d0, with
        // coefficient c10, and vice versa.
        let x10 = ScalarCross { s: 1, t: 0, c: c10 };
        let x01 = ScalarCross { s: 0, t: 1, c: c01 };
        let r = solve_coupled(
            &[&d0, &d1],
            &[&x10, &x01],
            &ledger_2x2_coupled(),
            &CoupleOptions::default(),
        )
        .expect("converges");
        let (e0, e1) = exact(a0, a1, c01, c10, s0, s1);
        assert!(
            (r.lambdas[0][0] - e0).abs() < 1e-8,
            "{} vs {e0}",
            r.lambdas[0][0]
        );
        assert!(
            (r.lambdas[1][0] - e1).abs() < 1e-8,
            "{} vs {e1}",
            r.lambdas[1][0]
        );
        assert!(r.completeness.may_optimize());
    }

    /// The headline, in miniature: dropping the cross terms does not
    /// perturb the answer, it changes it qualitatively. The uncoupled
    /// solve cannot see that d1 exists, so it returns +2.0 where the
    /// coupled truth is −25.7 — SU2's sign flip, on two scalars.
    #[test]
    fn dropping_cross_terms_changes_the_answer_materially() {
        let (a0, a1, c01, c10, s0, s1) = (0.5, 0.4, 0.3, 0.3, 1.0, -20.0);
        let d0 = Scalar {
            name: "d0".into(),
            a: a0,
            seed: s0,
        };
        let d1 = Scalar {
            name: "d1".into(),
            a: a1,
            seed: s1,
        };
        let uncoupled = CouplingLedger::new(["d0", "d1"], BlockMethod::Analytic).unwrap();
        let r = solve_coupled(&[&d0, &d1], &[], &uncoupled, &CoupleOptions::default()).unwrap();
        let (e0, _) = exact(a0, a1, c01, c10, s0, s1);
        // Uncoupled: λ0 = s0/(1-a0) = 2.0. Coupled truth is negative.
        assert!((r.lambdas[0][0] - 2.0).abs() < 1e-8);
        assert!(e0 < 0.0, "coupled truth {e0} should flip sign");
        let report = crate::validate::ablation("cross terms", e0, e0, r.lambdas[0][0]);
        assert!(report.sign_flip, "{}", report.summary());
        assert!(
            report.ablated_rel_err > 1.0,
            "the uncoupled answer should be more than 100% off: {}",
            report.summary()
        );
    }

    #[test]
    fn an_unregistered_implemented_block_is_refused() {
        let d0 = Scalar {
            name: "d0".into(),
            a: 0.5,
            seed: 1.0,
        };
        let d1 = Scalar {
            name: "d1".into(),
            a: 0.4,
            seed: 1.0,
        };
        // Ledger claims both cross blocks; only one is registered.
        let x10 = ScalarCross { s: 1, t: 0, c: 0.3 };
        let err = solve_coupled(
            &[&d0, &d1],
            &[&x10],
            &ledger_2x2_coupled(),
            &CoupleOptions::default(),
        )
        .unwrap_err();
        match err {
            CoupleError::LedgerMismatch { problems } => {
                assert_eq!(problems.len(), 1);
                assert!(problems[0].contains("no cross term was registered"));
            }
            other => panic!("expected LedgerMismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_cross_term_the_ledger_does_not_declare_is_refused() {
        let d0 = Scalar {
            name: "d0".into(),
            a: 0.5,
            seed: 1.0,
        };
        let d1 = Scalar {
            name: "d1".into(),
            a: 0.4,
            seed: 1.0,
        };
        let x10 = ScalarCross { s: 1, t: 0, c: 0.3 };
        let bare = CouplingLedger::new(["d0", "d1"], BlockMethod::Analytic).unwrap();
        let err =
            solve_coupled(&[&d0, &d1], &[&x10], &bare, &CoupleOptions::default()).unwrap_err();
        assert!(matches!(err, CoupleError::LedgerMismatch { .. }));
    }

    #[test]
    fn an_incomplete_ledger_is_refused_outright() {
        let d0 = Scalar {
            name: "d0".into(),
            a: 0.5,
            seed: 1.0,
        };
        let d1 = Scalar {
            name: "d1".into(),
            a: 0.4,
            seed: 1.0,
        };
        let mut l = CouplingLedger::new(["d0", "d1"], BlockMethod::Analytic).unwrap();
        l.set(0, 1, BlockStatus::missing("not written yet"))
            .unwrap();
        let err = solve_coupled(&[&d0, &d1], &[], &l, &CoupleOptions::default()).unwrap_err();
        assert!(matches!(err, CoupleError::IncompleteLedger(_)));
    }

    #[test]
    fn divergence_is_reported_not_returned() {
        // Spectral radius > 1: the coupled iteration cannot converge.
        let d0 = Scalar {
            name: "d0".into(),
            a: 1.5,
            seed: 1.0,
        };
        let d1 = Scalar {
            name: "d1".into(),
            a: 1.4,
            seed: 1.0,
        };
        let err = solve_coupled(
            &[&d0, &d1],
            &[],
            &CouplingLedger::new(["d0", "d1"], BlockMethod::Analytic).unwrap(),
            &CoupleOptions {
                max_outer: 40,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CoupleError::NotConverged { .. } | CoupleError::Diverged { .. }
        ));
    }

    #[test]
    fn name_and_arity_mismatches_are_caught() {
        let d0 = Scalar {
            name: "wrong".into(),
            a: 0.5,
            seed: 1.0,
        };
        let d1 = Scalar {
            name: "d1".into(),
            a: 0.4,
            seed: 1.0,
        };
        let l = CouplingLedger::new(["d0", "d1"], BlockMethod::Analytic).unwrap();
        assert!(matches!(
            solve_coupled(&[&d0, &d1], &[], &l, &CoupleOptions::default()),
            Err(CoupleError::NameMismatch { .. })
        ));
        assert!(matches!(
            solve_coupled(&[&d1], &[], &l, &CoupleOptions::default()),
            Err(CoupleError::ArityMismatch { .. })
        ));
    }

    #[test]
    fn convergence_rate_is_reported() {
        let d0 = Scalar {
            name: "d0".into(),
            a: 0.5,
            seed: 1.0,
        };
        let d1 = Scalar {
            name: "d1".into(),
            a: 0.5,
            seed: 1.0,
        };
        let r = solve_coupled(
            &[&d0, &d1],
            &[],
            &CouplingLedger::new(["d0", "d1"], BlockMethod::Analytic).unwrap(),
            &CoupleOptions::default(),
        )
        .unwrap();
        let rate = r.rate().expect("enough history for a rate");
        // Contraction factor 0.5 per sweep.
        assert!(rate > 0.2 && rate < 0.8, "rate {rate}");
    }
}
