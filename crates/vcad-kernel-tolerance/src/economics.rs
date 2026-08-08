//! The exchange rate between precision and money.
//!
//! Every other milestone in this ladder differentiates a physical
//! quantity. This one differentiates the number the decision is actually
//! made on: **cost per good part**.
//!
//! ```text
//! C_good(t) = Σ Cᵢ(tᵢ) / Y(t)
//! ```
//!
//! Loosening a tolerance makes the part cheaper and the yield worse. Both
//! effects are real, both are already exactly computable in this crate,
//! and neither one on its own tells you what to do. The quotient does:
//!
//! ```text
//! dC_good/dtᵢ = [ (dCᵢ/dtᵢ)·Y − C·(dY/dtᵢ) ] / Y²
//! ```
//!
//! - `dCᵢ/dtᵢ` is exact and closed-form ([`crate::allocate::CostModel::d_cost_d_tol`]).
//! - `dY/dtᵢ` chains the crate's exact yield sensitivity through the
//!   sigma convention: `σᵢ = tᵢ/k`, so `dY/dtᵢ = (dY/dσᵢ)/k`.
//!
//! Sign convention: `dC_good/dt < 0` means **loosen** — the saving beats
//! the scrap. `> 0` means **tighten** — you are paying more in scrap than
//! you are saving in machining. A sign change across the fleet of
//! contributors is a design at its economic optimum, and the row where
//! the magnitude is largest is where the next dollar should go.
//!
//! # The exchange rate
//!
//! [`ExchangeRow::dollars_per_yield_point`] is `(dC/dt)/(dY/dt)` — what
//! one point of yield costs on *this* dimension. It is the number that
//! makes two dimensions comparable when their tolerances are in different
//! units of difficulty, and it is the thing a manufacturing engineer
//! actually argues about.
//!
//! # Honesty
//!
//! - `dY/dσ` inherits the RSS **normal-gap** model. It is exact given
//!   that model and wrong to the extent the gap is not normal; a chain
//!   with a dominant uniform contributor is the case to distrust, and
//!   [`ToleranceEconomics::all_normal`] reports it.
//! - Only contributors with a normal distribution and a stated sigma
//!   convention can be priced this way — the tolerance→σ chain needs a
//!   `k`. Vendor-band uniforms and measured empirical distributions are
//!   skipped rather than given a fabricated `k`, and they are listed in
//!   [`ToleranceEconomics::skipped`].
//! - The cost model is whatever the caller supplies. `C(t) = a + b/t` is
//!   a fitted convenience, not a law of nature; the derivative is exact
//!   for the model, and the model is an assumption.
//! - Yield here is the *stackup's* yield. A real line has other scrap
//!   sources, and multiplying them in is the caller's job.

use serde::{Deserialize, Serialize};
use vcad_kernel_adjoint::{Route, Sensitivity, SensitivityTable, TrustLimit, TrustRadius};

use crate::allocate::CostModel;
use crate::analysis::rss;
use crate::dist::{Distribution, DistributionSource};
use crate::sensitivity::sensitivities;
use crate::stackup::{Stackup, StackupError};

/// What to do with a tolerance, economically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Loosening lowers cost per good part: the machining saving beats
    /// the scrap it causes.
    Loosen,
    /// Tightening lowers cost per good part: scrap costs more than the
    /// precision does.
    Tighten,
    /// The derivative is within tolerance of zero — this dimension is at
    /// its economic optimum and there is nothing to win here.
    AtOptimum,
}

impl Direction {
    /// Human label.
    pub fn label(self) -> &'static str {
        match self {
            Direction::Loosen => "loosen",
            Direction::Tighten => "tighten",
            Direction::AtOptimum => "at optimum",
        }
    }
}

/// One dimension's economics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExchangeRow {
    /// Contributor name.
    pub contributor: String,
    /// Current ± tolerance, mm.
    pub tol_mm: f64,
    /// Current σ, mm.
    pub sigma_mm: f64,
    /// σ per unit tolerance (`1/k` for the stated convention).
    pub sigma_per_tol: f64,
    /// This dimension's cost at the current tolerance.
    pub cost: f64,
    /// `dC/dt`, currency per mm. Negative: looser is cheaper.
    pub d_cost_d_tol: f64,
    /// `dY/dt`, yield fraction per mm. Negative: looser scatters more.
    pub d_yield_d_tol: f64,
    /// `d(cost per good part)/dt`. **The decision number.** Negative
    /// means loosen.
    pub d_cost_per_good_d_tol: f64,
    /// `(dC/dt)/(dY/dt)` — currency per unit of yield fraction on this
    /// dimension. `None` when the yield does not respond (a dimension
    /// off the critical chain), which is precisely the case where
    /// loosening is free.
    pub dollars_per_yield_point: Option<f64>,
    /// Share of the gap variance this dimension owns, in [0, 1].
    pub variance_share: f64,
    /// What to do.
    pub direction: Direction,
}

impl ExchangeRow {
    /// Currency saved per *percentage point* of yield given up — the
    /// unit the argument is usually had in.
    pub fn dollars_per_percent_yield(&self) -> Option<f64> {
        self.dollars_per_yield_point.map(|r| r / 100.0)
    }
}

/// A contributor that could not be priced, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skipped {
    /// Contributor name.
    pub contributor: String,
    /// Why it was skipped.
    pub reason: String,
}

/// The economics of a whole stackup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToleranceEconomics {
    /// Stackup yield at the current tolerances.
    pub yield_fraction: f64,
    /// Σ Cᵢ(tᵢ) over priced contributors.
    pub unit_cost: f64,
    /// `unit_cost / yield` — what a *shippable* part costs.
    pub cost_per_good_part: f64,
    /// Scrap cost per part: `cost_per_good_part − unit_cost`.
    pub scrap_cost: f64,
    /// One row per priced contributor, largest `|d_cost_per_good_d_tol|`
    /// first — the order in which to spend attention.
    pub rows: Vec<ExchangeRow>,
    /// Contributors that could not be priced.
    pub skipped: Vec<Skipped>,
    /// Whether every contributor is normal (the RSS yield model's
    /// premise). False means the yield derivatives are approximations of
    /// unstated quality.
    pub all_normal: bool,
}

impl ToleranceEconomics {
    /// The single best move: the dimension with the largest economic
    /// gradient, or `None` when everything is at its optimum.
    pub fn best_move(&self) -> Option<&ExchangeRow> {
        self.rows
            .iter()
            .find(|r| r.direction != Direction::AtOptimum)
    }

    /// Render as the shared sensitivity vocabulary, so tolerance
    /// economics compose with geometric and physical sensitivities in one
    /// table.
    ///
    /// The trust radius is the allocation box: a cost curve fitted
    /// between `t_min` and `t_max` says nothing outside it, and the
    /// reciprocal families in particular blow up as `t → 0`.
    pub fn to_table(&self, bounds: impl Fn(&str) -> Option<(f64, f64)>) -> SensitivityTable {
        let mut table = SensitivityTable::new();
        for r in &self.rows {
            let trust = bounds(&r.contributor)
                .and_then(|(lo, hi)| TrustRadius::new(lo, hi, TrustLimit::ModelValidity));
            table.push(
                Sensitivity::new(
                    r.contributor.clone(),
                    "cost_per_good_part",
                    r.d_cost_per_good_d_tol,
                    "currency/mm",
                    r.tol_mm,
                    Route::Analytic,
                )
                .with_trust(trust)
                .with_note(format!(
                    "{}: dC/dt {:.4}, dY/dt {:.6}{}",
                    r.direction.label(),
                    r.d_cost_d_tol,
                    r.d_yield_d_tol,
                    match r.dollars_per_percent_yield() {
                        Some(v) => format!(", {v:.4} per point of yield"),
                        None => String::new(),
                    }
                )),
            );
        }
        table
    }

    /// Fixed-width summary.
    pub fn render(&self) -> String {
        let mut out = format!(
            "yield {:.4}%  unit cost {:.4}  cost per good part {:.4}  (scrap {:.4})\n",
            self.yield_fraction * 100.0,
            self.unit_cost,
            self.cost_per_good_part,
            self.scrap_cost
        );
        let w = self
            .rows
            .iter()
            .map(|r| r.contributor.len())
            .max()
            .unwrap_or(10)
            .max(10);
        out.push_str(&format!(
            "{:w$}  {:>10}  {:>12}  {:>12}  {:>14}  {}\n",
            "dimension",
            "tol",
            "dC/dt",
            "dY/dt",
            "dC_good/dt",
            "verdict",
            w = w
        ));
        for r in &self.rows {
            out.push_str(&format!(
                "{:w$}  {:>10.4}  {:>12.4}  {:>12.6}  {:>14.4}  {}\n",
                r.contributor,
                r.tol_mm,
                r.d_cost_d_tol,
                r.d_yield_d_tol,
                r.d_cost_per_good_d_tol,
                r.direction.label(),
                w = w
            ));
        }
        for s in &self.skipped {
            out.push_str(&format!("  skipped {}: {}\n", s.contributor, s.reason));
        }
        if !self.all_normal {
            out.push_str(
                "  NOTE: not every contributor is normal — the yield derivatives inherit \
                 the RSS normal-gap model\n",
            );
        }
        out
    }
}

/// Cost model and box for one dimension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricedDimension {
    /// Contributor name in the stackup.
    pub contributor: String,
    /// Cost-vs-tolerance curve.
    pub cost: CostModel,
}

/// Threshold below which `dC_good/dt` counts as zero, as a fraction of
/// the largest magnitude in the fleet. Keeps a dimension that is three
/// orders of magnitude off the action from being labelled "loosen".
const OPTIMUM_REL: f64 = 1e-6;

/// Price a stackup: what each tolerance costs, what it buys, and which
/// way to move it.
pub fn economics(
    s: &Stackup,
    priced: &[PricedDimension],
    scrap_is_total_loss: bool,
) -> Result<ToleranceEconomics, StackupError> {
    s.validate()?;
    let analysis = rss(s)?;
    let sens = sensitivities(s)?;
    let y = analysis.yield_estimate;

    let mut rows: Vec<ExchangeRow> = Vec::new();
    let mut skipped: Vec<Skipped> = Vec::new();
    let mut unit_cost = 0.0;

    // First pass: cost and the tolerance→σ chain per dimension.
    struct Priced<'a> {
        name: &'a str,
        model: CostModel,
        tol: f64,
        sigma: f64,
        k: f64,
    }
    let mut ready: Vec<Priced> = Vec::new();
    for p in priced {
        let Some(c) = s.contributors.iter().find(|c| c.name == p.contributor) else {
            skipped.push(Skipped {
                contributor: p.contributor.clone(),
                reason: "no such contributor in the stackup".into(),
            });
            continue;
        };
        if !matches!(c.dist, Distribution::Normal { .. }) {
            skipped.push(Skipped {
                contributor: p.contributor.clone(),
                reason: "not a normal contributor — a tolerance→sigma chain needs a convention"
                    .into(),
            });
            continue;
        }
        let k = match &c.source {
            DistributionSource::Assumed { convention, .. } => convention.k(),
            other => {
                skipped.push(Skipped {
                    contributor: p.contributor.clone(),
                    reason: format!(
                        "distribution is {other:?}, not derived from a drawing tolerance — \
                         there is no tolerance to price"
                    ),
                });
                continue;
            }
        };
        // The symmetric ± tolerance this contributor was built from.
        let tol = 0.5 * (c.tol_minus + c.tol_plus);
        if tol <= 0.0 || k <= 0.0 || !tol.is_finite() || !k.is_finite() {
            skipped.push(Skipped {
                contributor: p.contributor.clone(),
                reason: "degenerate tolerance or sigma convention".into(),
            });
            continue;
        }
        unit_cost += p.cost.cost(tol);
        ready.push(Priced {
            name: &c.name,
            model: p.cost,
            tol,
            sigma: c.dist.sigma(),
            k,
        });
    }

    let cost_per_good = if scrap_is_total_loss && y > 0.0 {
        unit_cost / y
    } else {
        unit_cost
    };

    for p in &ready {
        let srow = sens
            .iter()
            .find(|r| r.name == p.name)
            .expect("sensitivities cover every contributor");
        let d_cost_d_tol = p.model.d_cost_d_tol(p.tol);
        // σ = t/k  ⇒  dY/dt = (dY/dσ)·(1/k).
        let sigma_per_tol = 1.0 / p.k;
        let d_yield_d_tol = srow.d_yield_d_sigma * sigma_per_tol;

        // d(C/Y)/dt = (C'·Y − C·Y')/Y².
        let d_cost_per_good_d_tol = if scrap_is_total_loss && y > 0.0 {
            (d_cost_d_tol * y - unit_cost * d_yield_d_tol) / (y * y)
        } else {
            d_cost_d_tol
        };

        let dollars_per_yield_point = if d_yield_d_tol.abs() > 0.0 {
            Some(d_cost_d_tol / d_yield_d_tol)
        } else {
            None
        };

        rows.push(ExchangeRow {
            contributor: p.name.to_string(),
            tol_mm: p.tol,
            sigma_mm: p.sigma,
            sigma_per_tol,
            cost: p.model.cost(p.tol),
            d_cost_d_tol,
            d_yield_d_tol,
            d_cost_per_good_d_tol,
            dollars_per_yield_point,
            variance_share: srow.variance_share,
            // Filled in below, once the fleet's scale is known.
            direction: Direction::AtOptimum,
        });
    }

    // Direction, scaled against the fleet so a dimension that is
    // effectively inert does not get an action label.
    let scale = rows
        .iter()
        .map(|r| r.d_cost_per_good_d_tol.abs())
        .fold(0.0_f64, f64::max);
    for r in rows.iter_mut() {
        let d = r.d_cost_per_good_d_tol;
        r.direction = if scale == 0.0 || d.abs() <= OPTIMUM_REL * scale {
            Direction::AtOptimum
        } else if d < 0.0 {
            Direction::Loosen
        } else {
            Direction::Tighten
        };
    }
    rows.sort_by(|a, b| {
        b.d_cost_per_good_d_tol
            .abs()
            .partial_cmp(&a.d_cost_per_good_d_tol.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(ToleranceEconomics {
        yield_fraction: y,
        unit_cost,
        cost_per_good_part: cost_per_good,
        scrap_cost: cost_per_good - unit_cost,
        rows,
        skipped,
        all_normal: analysis.all_normal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dist::SigmaConvention;
    use crate::stackup::{Contributor, Requirement};

    /// Three machined dimensions closing on a gap requirement. The
    /// tolerances start tight enough that the yield is high, so the
    /// scrap term is small and machining cost dominates.
    fn chain(tol_a: f64) -> Stackup {
        Stackup {
            name: "housing".into(),
            contributors: vec![
                Contributor::normal("bore", 1.0, 50.0, tol_a, SigmaConvention::ThreeSigma),
                Contributor::normal("shaft", -1.0, 20.0, 0.05, SigmaConvention::ThreeSigma),
                Contributor::normal("spacer", -1.0, 9.7, 0.03, SigmaConvention::ThreeSigma),
            ],
            requirement: Requirement::between("gap", 20.0, 20.6),
        }
    }

    fn priced() -> Vec<PricedDimension> {
        vec![
            PricedDimension {
                contributor: "bore".into(),
                cost: CostModel::Reciprocal { a: 2.0, b: 0.02 },
            },
            PricedDimension {
                contributor: "shaft".into(),
                cost: CostModel::Reciprocal { a: 1.5, b: 0.01 },
            },
        ]
    }

    /// The decision number must equal a central finite difference of the
    /// thing it claims to be the derivative of — cost per good part.
    #[test]
    fn the_decision_number_is_the_derivative_of_cost_per_good_part() {
        let t0 = 0.08;
        let cost_per_good = |t: f64| {
            let e = economics(&chain(t), &priced(), true).unwrap();
            e.cost_per_good_part
        };
        let e = economics(&chain(t0), &priced(), true).unwrap();
        let bore = e.rows.iter().find(|r| r.contributor == "bore").unwrap();

        let h = 1e-5;
        let fd = (cost_per_good(t0 + h) - cost_per_good(t0 - h)) / (2.0 * h);
        let rel = (bore.d_cost_per_good_d_tol - fd).abs() / fd.abs();
        assert!(
            rel < 1e-4,
            "dC_good/dt: analytic {:.9e}, fd {fd:.9e} (rel {rel:.3e})",
            bore.d_cost_per_good_d_tol
        );
    }

    /// The two effects pull opposite ways, and both are real.
    #[test]
    fn loosening_is_cheaper_to_make_and_worse_to_yield() {
        let e = economics(&chain(0.08), &priced(), true).unwrap();
        for r in &e.rows {
            assert!(
                r.d_cost_d_tol < 0.0,
                "{} looser must be cheaper",
                r.contributor
            );
            assert!(
                r.d_yield_d_tol <= 0.0,
                "{} looser must not improve yield",
                r.contributor
            );
        }
    }

    /// The optimum is interior: tight tolerances say loosen, loose ones
    /// say tighten, and the sign flips in between. That crossing is the
    /// whole point — it is a number no single-discipline analysis can
    /// produce.
    #[test]
    fn the_economic_optimum_is_interior() {
        let bore_dir = |t: f64| {
            let e = economics(&chain(t), &priced(), true).unwrap();
            e.rows
                .iter()
                .find(|r| r.contributor == "bore")
                .unwrap()
                .d_cost_per_good_d_tol
        };
        // The window is +/-0.3 mm on a centred gap, so the yield stays
        // above 99.9% until the tolerance is a good fraction of it — the
        // crossing is out near 1 mm, not at the tenths a first guess
        // reaches for.
        let tight = bore_dir(0.02);
        let loose = bore_dir(1.0);
        assert!(
            tight < 0.0,
            "at a tight 0.02 the machining saving should dominate, got {tight:.6e}"
        );
        assert!(
            loose > 0.0,
            "at a loose 1.0 the scrap should dominate, got {loose:.6e}"
        );

        // Bisect the crossing and confirm it is a genuine optimum: cost
        // per good part there is below both ends.
        let (mut lo, mut hi) = (0.02_f64, 1.0_f64);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if bore_dir(mid) < 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let t_star = 0.5 * (lo + hi);
        let cost = |t: f64| {
            economics(&chain(t), &priced(), true)
                .unwrap()
                .cost_per_good_part
        };
        assert!(t_star > 0.02 && t_star < 1.0, "optimum at {t_star}");
        assert!(
            cost(t_star) < cost(0.02) && cost(t_star) < cost(1.0),
            "t* = {t_star:.5} should be cheaper than both ends: {:.6} vs {:.6} / {:.6}",
            cost(t_star),
            cost(0.02),
            cost(1.0)
        );
        println!(
            "economic optimum at +/-{t_star:.4} mm: {:.5} per good part \
             (vs {:.5} at +/-0.02, {:.5} at +/-1.0)",
            cost(t_star),
            cost(0.02),
            cost(1.0)
        );
    }

    /// Without the scrap coupling the answer is always "loosen" — the
    /// degenerate advice a cost model alone gives, and the reason the
    /// coupling exists.
    #[test]
    fn without_the_scrap_coupling_the_advice_is_always_loosen() {
        for t in [0.02, 0.08, 0.25] {
            let e = economics(&chain(t), &priced(), false).unwrap();
            for r in &e.rows {
                assert_eq!(
                    r.direction,
                    Direction::Loosen,
                    "uncoupled, {} at t={t} should always say loosen",
                    r.contributor
                );
            }
        }
    }

    /// The exchange rate is what one point of yield costs on a dimension.
    #[test]
    fn the_exchange_rate_is_dollars_per_point_of_yield() {
        let e = economics(&chain(0.08), &priced(), true).unwrap();
        let bore = e.rows.iter().find(|r| r.contributor == "bore").unwrap();
        let rate = bore.dollars_per_yield_point.expect("bore moves the yield");
        // Both derivatives are negative, so the ratio is positive: giving
        // up yield saves money.
        assert!(rate > 0.0, "exchange rate {rate}");
        assert!((bore.dollars_per_percent_yield().unwrap() - rate / 100.0).abs() < 1e-12);
    }

    /// A contributor with no tolerance to price is skipped, loudly, not
    /// given a fabricated sigma convention.
    #[test]
    fn unpriceable_contributors_are_skipped_with_a_reason() {
        let mut s = chain(0.08);
        s.contributors.push(Contributor {
            name: "vendor_shim".into(),
            coeff: -1.0,
            nominal: 0.0,
            tol_minus: 0.05,
            tol_plus: 0.05,
            dist: Distribution::Uniform {
                lo: -0.05,
                hi: 0.05,
            },
            source: DistributionSource::default(),
        });
        let mut p = priced();
        p.push(PricedDimension {
            contributor: "vendor_shim".into(),
            cost: CostModel::Reciprocal { a: 0.1, b: 0.001 },
        });
        p.push(PricedDimension {
            contributor: "ghost".into(),
            cost: CostModel::Reciprocal { a: 0.1, b: 0.001 },
        });
        let e = economics(&s, &p, true).unwrap();
        assert_eq!(e.skipped.len(), 2);
        assert!(e
            .skipped
            .iter()
            .any(|k| k.contributor == "vendor_shim" && k.reason.contains("normal")));
        assert!(e
            .skipped
            .iter()
            .any(|k| k.contributor == "ghost" && k.reason.contains("no such contributor")));
        // And the mixed chain flags that the yield model's premise broke.
        assert!(!e.all_normal);
        assert!(e.render().contains("NOTE"));
    }

    /// Rows compose into the shared sensitivity vocabulary, carrying the
    /// allocation box as their trust radius.
    #[test]
    fn rows_render_into_the_shared_sensitivity_table() {
        let e = economics(&chain(0.08), &priced(), true).unwrap();
        let table = e.to_table(|name| match name {
            "bore" => Some((0.01, 0.30)),
            _ => None,
        });
        assert_eq!(table.len(), 2);
        let bore = table.rows.iter().find(|r| r.parameter == "bore").unwrap();
        assert_eq!(bore.objective, "cost_per_good_part");
        assert_eq!(bore.route, Route::Analytic);
        assert_eq!(bore.trust.unwrap().limited_by, TrustLimit::ModelValidity);
        assert!(bore.influence().is_some());
        assert!(bore.note.as_ref().unwrap().contains("per point of yield"));
        assert!(table.all_usable());
    }

    /// The ranking puts the biggest economic gradient first, and
    /// `best_move` names it.
    #[test]
    fn the_best_move_is_the_largest_economic_gradient() {
        let e = economics(&chain(0.08), &priced(), true).unwrap();
        assert!(e.rows[0].d_cost_per_good_d_tol.abs() >= e.rows[1].d_cost_per_good_d_tol.abs());
        let best = e.best_move().expect("something to do");
        assert_eq!(best.contributor, e.rows[0].contributor);
        assert!(e.render().contains("cost per good part"));
    }
}
