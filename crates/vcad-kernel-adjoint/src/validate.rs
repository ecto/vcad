//! Evidence: the finite-difference step sweep, and the ablation check.
//!
//! # The step sweep
//!
//! SU2's validation slide does not quote one finite difference. It quotes
//! six, from `h = 1e-1` down to `h = 1e-6`, and shows the estimate
//! settling — 3.48%, 0.38%, 0.039%, 0.0039%, 0.00039%, 0.00003% — *before*
//! comparing anything to the adjoint. That ordering matters. A single
//! finite difference is not a reference; it is a number with an unknown
//! truncation error and an unknown round-off error, and the only way to
//! know which regime you are in is to watch the estimate as `h` shrinks.
//!
//! [`fd_sweep`] runs the sweep and **fails closed if the estimate never
//! plateaus** ([`SweepError::NoPlateau`]). A reference that has not
//! demonstrated convergence is not allowed to validate a gradient. This
//! is the difference between "I checked it against FD" and "I have
//! evidence".
//!
//! # The ablation check
//!
//! A passing finite-difference comparison does **not** prove a cross term
//! is wired in. The term may be near zero in that fixture, in which case
//! the test passes identically whether the code is there or not. This is
//! the failure mode that lets a coupling regress silently for a year.
//!
//! [`ablation`] recomputes the gradient with the term deliberately
//! removed and requires the error to *grow*. A term that can be deleted
//! without moving the answer is either not wired in or not needed, and
//! either way the test should say so.
//!
//! **Honesty:** the sweep's plateau detector is a heuristic over a finite
//! list of steps. It can be fooled by a function that is piecewise-linear
//! at the scale probed (every step lands in the same linear patch, so the
//! estimates agree perfectly and mean nothing) — which is exactly what a
//! voxel mask does below one cell. That is why
//! [`crate::TrustRadius::from_grid`] exists, and why a sweep over a
//! mask-moving parameter should start above the cell size.

use serde::{Deserialize, Serialize};

/// One row of a finite-difference step sweep.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FdRow {
    /// The step.
    pub step: f64,
    /// `f(θ₀ − h)`.
    pub minus: f64,
    /// `f(θ₀ + h)`.
    pub plus: f64,
    /// Central difference `(f₊ − f₋) / 2h`.
    pub derivative: f64,
    /// Relative change from the previous (larger) step. `None` on the
    /// first row.
    pub rel_change: Option<f64>,
}

/// A completed, plateaued step sweep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FdSweep {
    /// Where the sweep was taken.
    pub at: f64,
    /// `f(θ₀)`.
    pub value: f64,
    /// One row per step, in the order supplied (largest step first by
    /// convention).
    pub rows: Vec<FdRow>,
    /// Index of the row taken as the reference — the tightest step inside
    /// the plateau, before round-off takes over.
    pub best: usize,
    /// The relative agreement achieved across the plateau.
    pub plateau_rel: f64,
}

impl FdSweep {
    /// The reference derivative.
    pub fn derivative(&self) -> f64 {
        self.rows[self.best].derivative
    }

    /// Relative error of a candidate derivative against this reference,
    /// scaled by the reference magnitude.
    ///
    /// When the reference is exactly zero there is no scale to normalize
    /// against: a zero candidate is exact and anything else is infinitely
    /// wrong. Inventing a scale there would let a wrong answer look
    /// small.
    pub fn rel_error(&self, candidate: f64) -> f64 {
        let d = self.derivative();
        if d == 0.0 {
            return if candidate == 0.0 { 0.0 } else { f64::INFINITY };
        }
        (candidate - d).abs() / d.abs()
    }

    /// SU2's table, rendered.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "f({:.6}) = {:.12}\n{:>10}  {:>22}  {:>14}\n",
            self.at, self.value, "step h", "central difference", "rel. change"
        ));
        for (i, r) in self.rows.iter().enumerate() {
            let mark = if i == self.best { " <- reference" } else { "" };
            let rc = match r.rel_change {
                Some(c) => format!("{:>13.6}%", c * 100.0),
                None => "             -".to_string(),
            };
            out.push_str(&format!(
                "{:>10.1e}  {:>22.12}  {rc}{mark}\n",
                r.step, r.derivative
            ));
        }
        out.push_str(&format!(
            "plateau agreement {:.3e} over the reference neighbourhood\n",
            self.plateau_rel
        ));
        out
    }
}

/// Why a sweep could not produce a reference.
#[derive(Debug, Clone, PartialEq)]
pub enum SweepError {
    /// Fewer than three steps — a plateau needs at least three estimates
    /// to be a plateau rather than a coincidence.
    TooFewSteps(usize),
    /// A step was zero, negative, or non-finite.
    BadStep(f64),
    /// The objective returned a non-finite value.
    NonFinite {
        /// The parameter value it was evaluated at.
        at: f64,
    },
    /// The estimate never settled: no two adjacent steps agreed to within
    /// the plateau tolerance. The finite differences are not a reference,
    /// and nothing may be validated against them.
    NoPlateau {
        /// The best adjacent agreement achieved.
        best_rel: f64,
        /// The tolerance it failed to meet.
        tol: f64,
    },
}

impl std::fmt::Display for SweepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SweepError::TooFewSteps(n) => {
                write!(
                    f,
                    "a finite-difference sweep needs at least 3 steps, got {n}"
                )
            }
            SweepError::BadStep(h) => write!(f, "invalid finite-difference step {h}"),
            SweepError::NonFinite { at } => {
                write!(f, "objective returned a non-finite value at {at}")
            }
            SweepError::NoPlateau { best_rel, tol } => write!(
                f,
                "finite differences never settled: best adjacent agreement {best_rel:.3e} \
                 > tol {tol:.3e} — this sweep is not a reference"
            ),
        }
    }
}

impl std::error::Error for SweepError {}

/// Run a central-difference sweep and require it to plateau.
///
/// `steps` should run from coarse to fine (`[1e-1, 1e-2, … 1e-6]`). The
/// reference is the finest step still inside the plateau — going finer
/// trades truncation error for round-off, and the sweep is how you see
/// the turn.
pub fn fd_sweep(
    mut f: impl FnMut(f64) -> f64,
    at: f64,
    steps: &[f64],
    plateau_tol: f64,
) -> Result<FdSweep, SweepError> {
    if steps.len() < 3 {
        return Err(SweepError::TooFewSteps(steps.len()));
    }
    for &h in steps {
        if !h.is_finite() || h <= 0.0 {
            return Err(SweepError::BadStep(h));
        }
    }

    let value = f(at);
    if !value.is_finite() {
        return Err(SweepError::NonFinite { at });
    }

    let mut rows: Vec<FdRow> = Vec::with_capacity(steps.len());
    for &h in steps {
        let minus = f(at - h);
        let plus = f(at + h);
        if !minus.is_finite() {
            return Err(SweepError::NonFinite { at: at - h });
        }
        if !plus.is_finite() {
            return Err(SweepError::NonFinite { at: at + h });
        }
        let derivative = (plus - minus) / (2.0 * h);
        let rel_change = rows.last().map(|p: &FdRow| {
            let scale = derivative.abs().max(p.derivative.abs());
            if scale == 0.0 {
                0.0
            } else {
                (derivative - p.derivative).abs() / scale
            }
        });
        rows.push(FdRow {
            step: h,
            minus,
            plus,
            derivative,
            rel_change,
        });
    }

    // The plateau: the finest row whose agreement with its predecessor is
    // within tolerance. Walking from the fine end backwards finds the
    // turn where round-off starts to dominate.
    let mut best: Option<(usize, f64)> = None;
    for i in (1..rows.len()).rev() {
        let rc = rows[i].rel_change.unwrap_or(f64::INFINITY);
        if rc <= plateau_tol {
            best = Some((i, rc));
            break;
        }
    }
    let (best, plateau_rel) = match best {
        Some(b) => b,
        None => {
            let best_rel = rows
                .iter()
                .filter_map(|r| r.rel_change)
                .fold(f64::INFINITY, f64::min);
            return Err(SweepError::NoPlateau {
                best_rel,
                tol: plateau_tol,
            });
        }
    };

    Ok(FdSweep {
        at,
        value,
        rows,
        best,
        plateau_rel,
    })
}

/// The result of deleting one term and watching what happens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AblationReport {
    /// What was deleted.
    pub term: String,
    /// The finite-difference reference.
    pub reference: f64,
    /// The gradient with the term.
    pub full: f64,
    /// The gradient without it.
    pub ablated: f64,
    /// Relative error of the full gradient.
    pub full_rel_err: f64,
    /// Relative error of the ablated gradient.
    pub ablated_rel_err: f64,
    /// How much worse ablation makes it. Infinite when the full gradient
    /// is exact and the ablated one is not — the ideal outcome.
    pub degradation: f64,
    /// Whether ablation flips the sign relative to the reference. SU2's
    /// three-physics case does exactly this, and a sign flip is the
    /// failure that an optimizer cannot survive.
    pub sign_flip: bool,
}

impl AblationReport {
    /// Whether the term is demonstrably load-bearing: removing it
    /// degrades the gradient by at least `min_degradation`×.
    pub fn load_bearing(&self, min_degradation: f64) -> bool {
        self.degradation >= min_degradation
    }

    /// One-line summary for a test failure message.
    pub fn summary(&self) -> String {
        format!(
            "{}: reference {:.6e}, full {:.6e} ({:.3}% err), ablated {:.6e} ({:.3}% err), \
             {:.1}x worse{}",
            self.term,
            self.reference,
            self.full,
            self.full_rel_err * 100.0,
            self.ablated,
            self.ablated_rel_err * 100.0,
            self.degradation,
            if self.sign_flip { ", SIGN FLIP" } else { "" }
        )
    }
}

/// Compare a full gradient and an ablated one against a reference.
///
/// Errors are scaled by the reference magnitude, falling back to the
/// largest magnitude in play when the reference is zero — so the case
/// that matters most here, *"the ablated gradient is exactly zero because
/// the parameter only reaches the objective through the deleted term"*,
/// reports a clean 100% error rather than a division by zero.
pub fn ablation(
    term: impl Into<String>,
    reference: f64,
    full: f64,
    ablated: f64,
) -> AblationReport {
    let scale = reference
        .abs()
        .max(full.abs())
        .max(ablated.abs())
        .max(f64::MIN_POSITIVE);
    let full_rel_err = (full - reference).abs() / scale;
    let ablated_rel_err = (ablated - reference).abs() / scale;
    let degradation = if full_rel_err == 0.0 {
        if ablated_rel_err == 0.0 {
            1.0
        } else {
            f64::INFINITY
        }
    } else {
        ablated_rel_err / full_rel_err
    };
    let sign_flip = reference != 0.0 && ablated != 0.0 && reference.signum() != ablated.signum();
    AblationReport {
        term: term.into(),
        reference,
        full,
        ablated,
        full_rel_err,
        ablated_rel_err,
        degradation,
        sign_flip,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEPS: [f64; 6] = [1e-1, 1e-2, 1e-3, 1e-4, 1e-5, 1e-6];

    #[test]
    fn a_smooth_function_plateaus_and_finds_its_derivative() {
        // f(x) = x^3 - 2x, f'(2) = 3*4 - 2 = 10.
        let sw = fd_sweep(|x| x * x * x - 2.0 * x, 2.0, &STEPS, 1e-6).unwrap();
        assert!((sw.derivative() - 10.0).abs() < 1e-6, "{}", sw.derivative());
        assert!(sw.rel_error(10.0) < 1e-8);
        // The coarse rows carry visible truncation error; the fine ones
        // do not. That's the whole point of showing the table.
        assert!(sw.rows[0].rel_change.is_none());
        assert!(sw.rows[1].rel_change.unwrap() > 1e-4);
        assert!(sw.render().contains("<- reference"));
    }

    #[test]
    fn a_noisy_objective_refuses_to_be_a_reference() {
        // A function with a jitter floor: differencing it never settles.
        let mut n = 0u32;
        let noisy = move |x: f64| {
            n = n.wrapping_mul(1664525).wrapping_add(1013904223);
            let jitter = (n as f64 / u32::MAX as f64 - 0.5) * 1e-6;
            x * x + jitter
        };
        let err = fd_sweep(noisy, 1.0, &STEPS, 1e-6).unwrap_err();
        assert!(matches!(err, SweepError::NoPlateau { .. }));
    }

    #[test]
    fn non_finite_and_bad_steps_fail_closed() {
        assert!(matches!(
            fd_sweep(|_| f64::NAN, 1.0, &STEPS, 1e-6),
            Err(SweepError::NonFinite { .. })
        ));
        assert!(matches!(
            fd_sweep(|x| x, 1.0, &[1e-1, 0.0, 1e-3], 1e-6),
            Err(SweepError::BadStep(_))
        ));
        assert!(matches!(
            fd_sweep(|x| x, 1.0, &[1e-1, 1e-2], 1e-6),
            Err(SweepError::TooFewSteps(2))
        ));
    }

    #[test]
    fn a_provably_zero_derivative_admits_only_zero() {
        // f'(0) = 0 for f = x², and every row of the sweep is exactly
        // zero — there is no scale to normalize against. A candidate of 0
        // is exact; any other candidate is infinitely wrong in relative
        // terms, and saying so is more useful than inventing a scale.
        let sw = fd_sweep(|x| x * x, 0.0, &STEPS, 1e-6).unwrap();
        assert_eq!(sw.derivative(), 0.0);
        assert_eq!(sw.rel_error(0.0), 0.0);
        assert!(sw.rel_error(1.0).is_infinite());
    }

    #[test]
    fn a_zero_candidate_against_a_real_reference_reads_100_percent() {
        // The case that actually matters: the ablated gradient collapses
        // to zero because the parameter only reaches the objective through
        // the deleted term. That must be a clean 100%, not a divide fault.
        let sw = fd_sweep(|x| x * x * x - 2.0 * x, 2.0, &STEPS, 1e-6).unwrap();
        assert!((sw.rel_error(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn ablation_catches_a_term_that_carries_the_whole_answer() {
        // The SU2 shape: the parameter reaches the objective only through
        // the coupling, so deleting it zeroes the gradient outright.
        let r = ablation("dG(thermal)/du(flow)", -3.2, -3.2, 0.0);
        assert!(r.load_bearing(10.0));
        assert!(r.degradation.is_infinite());
        assert!((r.ablated_rel_err - 1.0).abs() < 1e-12);
        assert!(!r.sign_flip); // zero has no sign
        assert!(r.summary().contains("load") || r.summary().contains("worse"));
    }

    #[test]
    fn ablation_reports_a_sign_flip() {
        // SU2's three-physics row: FD +0.251, ablated -0.525.
        let r = ablation("all coupling", 0.2510100394, 0.2510, -0.525212846);
        assert!(r.sign_flip, "must flag the sign flip");
        assert!(r.ablated_rel_err > 1.0, "{}", r.ablated_rel_err);
        assert!(r.load_bearing(100.0));
        assert!(r.summary().contains("SIGN FLIP"));
    }

    #[test]
    fn ablation_calls_out_a_term_that_does_nothing() {
        // Identical gradients: the term is not wired in, or not needed.
        let r = ablation("suspect", 1.0, 1.0, 1.0);
        assert!(!r.load_bearing(2.0));
        assert_eq!(r.degradation, 1.0);
    }

    #[test]
    fn su2_cht_numbers_reproduce_the_published_error() {
        // Coarse mesh row: FD 0.7047023658, disc-adj without coupling
        // 0.4269605577, published relative error 39.4%.
        let r = ablation("cht coupling", 0.7047023658, 0.7047023658, 0.4269605577);
        assert!(
            (r.ablated_rel_err - 0.394).abs() < 0.001,
            "expected SU2's 39.4%, got {:.4}%",
            r.ablated_rel_err * 100.0
        );
    }
}
