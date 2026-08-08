//! The reporting vocabulary: one sensitivity, and a table of them.
//!
//! A number like `−8.1` is not a sensitivity. A sensitivity is a number,
//! a unit, the route that produced it, how complete the coupling behind
//! it was, and — the part everybody forgets — **the interval of the
//! parameter over which it is meaningful**.
//!
//! # Why the trust radius is not optional
//!
//! CAD is not smooth. A fillet whose radius exceeds the edge it sits on
//! stops existing; a boolean can change face count; a voxel solver's
//! material mask does not move at all until a parameter crosses half a
//! cell. Each of those makes `dJ/dθ` either undefined or, worse,
//! *defined and wrong*: the mask case returns a clean, confident,
//! spuriously-zero derivative, and nothing in the arithmetic notices.
//!
//! [`TrustRadius`] carries the interval and the reason it ends
//! ([`TrustLimit`]). Three constructors cover the ways vcad's gradients
//! actually stop being true:
//!
//! - [`TrustRadius::from_linearity`] — the second-derivative bound. The
//!   interval where a linear model stays within a stated relative error.
//! - [`TrustRadius::from_grid`] — the discretization floor. A parameter
//!   step smaller than this cannot move a voxel mask, so a gradient taken
//!   through the mask is noise below it.
//! - [`TrustRadius::from_bounds`] — the scrub range the author declared.
//!
//! # Ranking, and the °C trap
//!
//! The obvious way to rank knobs is elasticity, `(θ/J)·dJ/dθ`. Do not do
//! that here. Half of vcad's objectives are temperatures in °C, whose
//! zero is a historical accident about brine — an elasticity computed
//! against it is meaningless, and it silently changes if the user
//! switches to °F.
//!
//! [`Sensitivity::influence`] ranks by `|dJ/dθ| × span`, where the span
//! is how far the parameter is actually allowed to move. That is "how
//! much of the objective this knob commands", it carries the objective's
//! own units, and it is invariant to where the objective's zero sits.

use serde::{Deserialize, Serialize};
use vcad_receipt::{ClaimBasis, ClaimQuantity, ClaimVerdict, OracleRef, ReceiptClaim};

use crate::ledger::Completeness;

/// How a sensitivity was obtained.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "route", rename_all = "snake_case")]
pub enum Route {
    /// Closed form.
    Analytic,
    /// One adjoint (backward) solve of a single discipline.
    Adjoint,
    /// Forward-mode dual numbers through the differentiable geometry seam.
    Dual,
    /// A coupled adjoint across several disciplines. Carries the ledger's
    /// roll-up, because a coupled number without its completeness is not
    /// interpretable.
    Coupled {
        /// How complete the coupling was.
        completeness: Completeness,
    },
    /// Central finite differences at the given step.
    FiniteDifference {
        /// Step size, in the parameter's units.
        step: f64,
    },
}

impl Route {
    /// Short label.
    pub fn label(&self) -> &'static str {
        match self {
            Route::Analytic => "analytic",
            Route::Adjoint => "adjoint",
            Route::Dual => "dual",
            Route::Coupled { .. } => "coupled-adjoint",
            Route::FiniteDifference { .. } => "finite-difference",
        }
    }

    /// The strongest basis this route may claim, before any other
    /// consideration.
    pub fn max_basis(&self) -> ClaimBasis {
        match self {
            Route::Analytic | Route::Adjoint | Route::Dual => ClaimBasis::Verified,
            Route::Coupled { completeness } => completeness.max_basis(),
            Route::FiniteDifference { .. } => ClaimBasis::Predicted,
        }
    }
}

/// Why a trust radius ends where it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustLimit {
    /// Beyond this the linear model departs from the function by more
    /// than the stated relative tolerance (a curvature bound).
    Linearity,
    /// The frozen tessellation plan's topology stops being valid — a face
    /// appears or vanishes and the parameterization no longer describes
    /// the same solid.
    TopologyStable,
    /// Discretization: a parameter step this small cannot move a voxel
    /// mask or a mesh node, so a gradient taken through the discretization
    /// is below its own noise floor.
    GridResolution,
    /// The author's declared scrub range for the parameter.
    ParameterBounds,
    /// The physical model itself stops applying (a correlation leaves its
    /// Reynolds range, a material leaves its linear-elastic regime).
    ModelValidity,
}

impl TrustLimit {
    /// Short label.
    pub fn label(self) -> &'static str {
        match self {
            TrustLimit::Linearity => "linearity",
            TrustLimit::TopologyStable => "topology",
            TrustLimit::GridResolution => "grid",
            TrustLimit::ParameterBounds => "bounds",
            TrustLimit::ModelValidity => "model",
        }
    }
}

/// The interval of a parameter over which a derivative is meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrustRadius {
    /// Lower edge, in the parameter's units.
    pub lower: f64,
    /// Upper edge.
    pub upper: f64,
    /// Why it ends.
    pub limited_by: TrustLimit,
}

impl TrustRadius {
    /// An explicit interval.
    pub fn new(lower: f64, upper: f64, limited_by: TrustLimit) -> Option<Self> {
        if !lower.is_finite() || !upper.is_finite() || upper < lower {
            return None;
        }
        Some(TrustRadius {
            lower,
            upper,
            limited_by,
        })
    }

    /// Width of the interval.
    pub fn span(&self) -> f64 {
        self.upper - self.lower
    }

    /// Whether a parameter value sits inside.
    pub fn contains(&self, theta: f64) -> bool {
        theta >= self.lower && theta <= self.upper
    }

    /// The curvature bound: the interval around `theta0` on which the
    /// linear model `J + d1·Δ` stays within `rel_tol` relative error of
    /// the quadratic `J + d1·Δ + ½·d2·Δ²`.
    ///
    /// Solving `|½·d2·Δ²| ≤ rel_tol·|d1·Δ|` gives `|Δ| ≤ 2·rel_tol·|d1/d2|`.
    /// With `d2 ≈ 0` the linear model never departs and the radius is
    /// unbounded — reported as `None`, since an unbounded radius is better
    /// expressed by having no linearity limit at all than by an infinity
    /// that has to be special-cased downstream.
    pub fn from_linearity(theta0: f64, d1: f64, d2: f64, rel_tol: f64) -> Option<Self> {
        if !theta0.is_finite() || !d1.is_finite() || !d2.is_finite() || rel_tol <= 0.0 {
            return None;
        }
        if d2 == 0.0 {
            return None;
        }
        let delta = 2.0 * rel_tol * (d1 / d2).abs();
        if !delta.is_finite() || delta <= 0.0 {
            return None;
        }
        TrustRadius::new(theta0 - delta, theta0 + delta, TrustLimit::Linearity)
    }

    /// The discretization floor for a parameter that moves a voxel mask.
    ///
    /// A step smaller than half a cell may not flip a single voxel, in
    /// which case a finite-difference gradient reads exactly zero — a
    /// confident, clean, entirely fictional answer. This radius says "do
    /// not believe a step below this"; note that unlike the other
    /// constructors it is a *lower* bound on a usable step, so it is
    /// reported as an interval of width one cell centred on `theta0`.
    pub fn from_grid(theta0: f64, cell_size: f64) -> Option<Self> {
        if !theta0.is_finite() || !cell_size.is_finite() || cell_size <= 0.0 {
            return None;
        }
        TrustRadius::new(
            theta0 - 0.5 * cell_size,
            theta0 + 0.5 * cell_size,
            TrustLimit::GridResolution,
        )
    }

    /// The author's declared scrub range.
    pub fn from_bounds(min: f64, max: f64) -> Option<Self> {
        TrustRadius::new(min, max, TrustLimit::ParameterBounds)
    }

    /// The tighter of two radii — the binding constraint. When both are
    /// present the result keeps the limit of whichever side actually
    /// binds on each edge, preferring the narrower overall.
    pub fn tighter(a: Option<Self>, b: Option<Self>) -> Option<Self> {
        match (a, b) {
            (None, x) | (x, None) => x,
            (Some(x), Some(y)) => {
                let lower = x.lower.max(y.lower);
                let upper = x.upper.min(y.upper);
                let limited_by = if x.span() <= y.span() {
                    x.limited_by
                } else {
                    y.limited_by
                };
                TrustRadius::new(lower, upper.max(lower), limited_by)
            }
        }
    }
}

/// One derivative, fully described.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sensitivity {
    /// Parameter name — a named document parameter, not an internal index.
    pub parameter: String,
    /// Objective name.
    pub objective: String,
    /// `dJ/dθ`.
    pub value: f64,
    /// Unit of the derivative, e.g. `"K/mm"`, `"g/mm"`, `"USD/mm"`.
    pub unit: String,
    /// Current parameter value, so the reader can place the derivative.
    pub at: f64,
    /// How it was obtained.
    pub route: Route,
    /// Provenance.
    pub basis: ClaimBasis,
    /// Whether anything establishes it as the derivative.
    pub verdict: ClaimVerdict,
    /// Where it stops being meaningful.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<TrustRadius>,
    /// Anything the reader needs (a frozen assumption, a warning).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Sensitivity {
    /// A sensitivity whose basis and verdict follow from its route.
    pub fn new(
        parameter: impl Into<String>,
        objective: impl Into<String>,
        value: f64,
        unit: impl Into<String>,
        at: f64,
        route: Route,
    ) -> Self {
        let basis = route.max_basis();
        let verdict = match &route {
            Route::Coupled { completeness } => completeness.verdict(),
            _ if !value.is_finite() => ClaimVerdict::Unverifiable,
            _ => ClaimVerdict::Pass,
        };
        Sensitivity {
            parameter: parameter.into(),
            objective: objective.into(),
            value,
            unit: unit.into(),
            at,
            route,
            basis,
            verdict,
            trust: None,
            note: None,
        }
    }

    /// Attach a trust radius.
    pub fn with_trust(mut self, trust: Option<TrustRadius>) -> Self {
        self.trust = trust;
        self
    }

    /// Attach a note, accumulating rather than replacing. A row often has
    /// several things worth saying (a non-smooth quantity *and* a searched
    /// trust radius), and the second one must not silently delete the
    /// first.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        let note = note.into();
        self.note = Some(match self.note.take() {
            Some(existing) => format!("{existing}; {note}"),
            None => note,
        });
        self
    }

    /// How much of the objective this knob commands: `|dJ/dθ| × span`,
    /// in the objective's units.
    ///
    /// `None` without a trust radius — an influence computed over an
    /// unstated range is not comparable to one computed over a stated
    /// range, and quietly assuming a unit span would rank knobs by their
    /// units.
    pub fn influence(&self) -> Option<f64> {
        self.trust.map(|t| self.value.abs() * t.span())
    }

    /// Whether the current parameter value sits inside its own trust
    /// radius. False is a bug in whoever built the radius, and worth an
    /// assertion at the call site.
    pub fn in_trust(&self) -> bool {
        self.trust.map(|t| t.contains(self.at)).unwrap_or(true)
    }

    /// Render as a receipt claim.
    ///
    /// A sensitivity whose verdict is `Unverifiable` becomes an
    /// unverifiable claim carrying the reason — it never silently
    /// disappears from the receipt, and it never passes.
    pub fn to_claim(&self, oracle: OracleRef) -> ReceiptClaim {
        let id = format!("sensitivity/{}/{}", self.objective, self.parameter);
        let description = format!(
            "d({})/d({}) = {:.6e} {} at {} = {:.6}",
            self.objective, self.parameter, self.value, self.unit, self.parameter, self.at
        );
        let mut details = vec![format!("route: {}", self.route.label())];
        if let Route::Coupled { completeness } = &self.route {
            details.push(completeness.summary());
        }
        if let Route::FiniteDifference { step } = &self.route {
            details.push(format!("step: {step:e}"));
        }
        match self.trust {
            Some(t) => details.push(format!(
                "trust: [{:.6}, {:.6}] limited by {}",
                t.lower,
                t.upper,
                t.limited_by.label()
            )),
            None => details.push("trust: unbounded (no radius established)".into()),
        }
        if let Some(n) = &self.note {
            details.push(n.clone());
        }
        let detail = details.join("; ");

        let claim = match self.verdict {
            ClaimVerdict::Unverifiable => {
                ReceiptClaim::unverifiable(id, "sensitivity", description, oracle, detail.clone())
            }
            ClaimVerdict::Fail => ReceiptClaim::fail(id, "sensitivity", description, oracle)
                .with_details(detail.clone()),
            ClaimVerdict::Pass => ReceiptClaim::pass(id, "sensitivity", description, oracle)
                .with_details(detail.clone()),
        };
        claim
            .with_basis(self.basis)
            .with_subject(self.parameter.clone())
            .with_predicted(ClaimQuantity::new(self.value, self.unit.clone()))
    }
}

/// A table of sensitivities: rows are parameters, and every row shares
/// the objective set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SensitivityTable {
    /// Every sensitivity, in insertion order.
    pub rows: Vec<Sensitivity>,
}

impl SensitivityTable {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append.
    pub fn push(&mut self, s: Sensitivity) -> &mut Self {
        self.rows.push(s);
        self
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Row count.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Distinct objectives, in first-seen order.
    pub fn objectives(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for r in &self.rows {
            if !out.contains(&r.objective.as_str()) {
                out.push(&r.objective);
            }
        }
        out
    }

    /// Distinct parameters, in first-seen order.
    pub fn parameters(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for r in &self.rows {
            if !out.contains(&r.parameter.as_str()) {
                out.push(&r.parameter);
            }
        }
        out
    }

    /// Parameters ranked by influence on one objective, most influential
    /// first.
    ///
    /// This is the ordering the feature tree wants: which knob actually
    /// commands this number. Rows without a trust radius have no
    /// comparable influence and sort last, in their original order.
    pub fn ranked_for(&self, objective: &str) -> Vec<&Sensitivity> {
        let mut rows: Vec<&Sensitivity> = self
            .rows
            .iter()
            .filter(|r| r.objective == objective)
            .collect();
        rows.sort_by(|a, b| match (a.influence(), b.influence()) {
            (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        rows
    }

    /// Every row that may not steer an optimizer, with its reason.
    pub fn unusable(&self) -> Vec<(&Sensitivity, String)> {
        self.rows
            .iter()
            .filter_map(|r| match (&r.route, r.verdict) {
                (Route::Coupled { completeness }, _) if !completeness.may_optimize() => {
                    Some((r, completeness.summary()))
                }
                (_, ClaimVerdict::Unverifiable) => {
                    Some((r, "unverifiable sensitivity".to_string()))
                }
                _ => None,
            })
            .collect()
    }

    /// Whether every row is safe to hand an optimizer.
    pub fn all_usable(&self) -> bool {
        self.unusable().is_empty()
    }

    /// Render a fixed-width table for logs and terminal output.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for obj in self.objectives() {
            out.push_str(&format!("d({obj})/d(parameter)\n"));
            let rows = self.ranked_for(obj);
            let w = rows
                .iter()
                .map(|r| r.parameter.len())
                .max()
                .unwrap_or(9)
                .max(9);
            for r in rows {
                let infl = match r.influence() {
                    Some(v) => format!("{v:>10.4}"),
                    None => "         -".to_string(),
                };
                let trust = match r.trust {
                    Some(t) => format!("[{:.4}, {:.4}] {}", t.lower, t.upper, t.limited_by.label()),
                    None => "unbounded".to_string(),
                };
                out.push_str(&format!(
                    "  {:w$}  {:>12.5e} {:<10} influence {infl}  {:<16} {trust}\n",
                    r.parameter,
                    r.value,
                    r.unit,
                    r.route.label(),
                    w = w
                ));
            }
        }
        let bad = self.unusable();
        if !bad.is_empty() {
            out.push_str(&format!(
                "  {} row(s) may not steer an optimizer:\n",
                bad.len()
            ));
            for (r, why) in bad {
                out.push_str(&format!("    {} / {}: {why}\n", r.objective, r.parameter));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oracle() -> OracleRef {
        OracleRef::new("vcad-kernel-adjoint/test", "0")
    }

    fn s(name: &str, value: f64, lo: f64, hi: f64) -> Sensitivity {
        Sensitivity::new(
            name,
            "hotspot_c",
            value,
            "K/mm",
            (lo + hi) / 2.0,
            Route::Adjoint,
        )
        .with_trust(TrustRadius::from_bounds(lo, hi))
    }

    #[test]
    fn linearity_radius_follows_the_curvature() {
        // Strong curvature -> tight radius; weak curvature -> wide.
        let tight = TrustRadius::from_linearity(2.0, 1.0, 10.0, 0.05).unwrap();
        let wide = TrustRadius::from_linearity(2.0, 1.0, 0.1, 0.05).unwrap();
        assert!(tight.span() < wide.span());
        assert!(tight.contains(2.0) && wide.contains(2.0));
        assert_eq!(tight.limited_by, TrustLimit::Linearity);
        // Δ = 2·0.05·|1/10| = 0.01
        assert!((tight.span() - 0.02).abs() < 1e-12);
        // Zero curvature: no linearity limit at all.
        assert!(TrustRadius::from_linearity(2.0, 1.0, 0.0, 0.05).is_none());
    }

    #[test]
    fn grid_radius_is_half_a_cell_each_way() {
        let t = TrustRadius::from_grid(5.0, 1.0).unwrap();
        assert!((t.span() - 1.0).abs() < 1e-12);
        assert_eq!(t.limited_by, TrustLimit::GridResolution);
        assert!(TrustRadius::from_grid(5.0, 0.0).is_none());
    }

    #[test]
    fn tighter_takes_the_binding_constraint() {
        let bounds = TrustRadius::from_bounds(0.0, 10.0);
        let lin = TrustRadius::from_linearity(5.0, 1.0, 4.0, 0.02); // ±0.01
        let t = TrustRadius::tighter(bounds, lin).unwrap();
        assert!(t.span() < 0.1, "span {}", t.span());
        assert_eq!(t.limited_by, TrustLimit::Linearity);
        // One-sided cases pass through.
        assert_eq!(TrustRadius::tighter(bounds, None), bounds);
        assert_eq!(TrustRadius::tighter(None, lin), lin);
    }

    #[test]
    fn influence_ranks_by_command_over_the_objective() {
        // A big derivative over a tiny range commands less than a small
        // derivative over a wide one.
        let mut t = SensitivityTable::new();
        t.push(s("fin_thickness", 100.0, 0.99, 1.01)); // influence 2
        t.push(s("wall", 1.0, 0.0, 10.0)); // influence 10
        let ranked = t.ranked_for("hotspot_c");
        assert_eq!(ranked[0].parameter, "wall");
        assert_eq!(ranked[1].parameter, "fin_thickness");
        assert!((ranked[0].influence().unwrap() - 10.0).abs() < 1e-12);
    }

    #[test]
    fn rows_without_a_radius_have_no_influence_and_sort_last() {
        let mut t = SensitivityTable::new();
        t.push(Sensitivity::new(
            "unbounded",
            "hotspot_c",
            1e9,
            "K/mm",
            1.0,
            Route::Adjoint,
        ));
        t.push(s("wall", 1.0, 0.0, 10.0));
        let ranked = t.ranked_for("hotspot_c");
        assert_eq!(ranked[0].parameter, "wall");
        assert!(ranked[1].influence().is_none());
    }

    #[test]
    fn a_coupled_row_inherits_its_ledger_verdict() {
        let incomplete = Completeness::Incomplete {
            reasons: vec!["dflow/dthermal missing".into()],
        };
        let row = Sensitivity::new(
            "inlet_v",
            "hotspot_c",
            -3.0,
            "K/(m/s)",
            0.1,
            Route::Coupled {
                completeness: incomplete,
            },
        );
        assert_eq!(row.verdict, ClaimVerdict::Unverifiable);
        let mut t = SensitivityTable::new();
        t.push(row);
        assert!(!t.all_usable());
        assert_eq!(t.unusable().len(), 1);
    }

    #[test]
    fn an_unverifiable_row_becomes_an_unverifiable_claim() {
        let row = Sensitivity::new(
            "inlet_v",
            "hotspot_c",
            -3.0,
            "K/(m/s)",
            0.1,
            Route::Coupled {
                completeness: Completeness::Incomplete {
                    reasons: vec!["missing".into()],
                },
            },
        );
        let c = row.to_claim(oracle());
        assert_eq!(c.verdict, ClaimVerdict::Unverifiable);
        assert!(c.details.as_ref().unwrap().contains("INCOMPLETE"));
    }

    #[test]
    fn a_clean_adjoint_row_claims_verified() {
        let row = s("wall", -8.1, 1.0, 4.0);
        assert_eq!(row.basis, ClaimBasis::Verified);
        let c = row.to_claim(oracle());
        assert_eq!(c.verdict, ClaimVerdict::Pass);
        assert_eq!(c.effective_basis(), ClaimBasis::Verified);
        assert!(c.details.as_ref().unwrap().contains("trust: ["));
    }

    #[test]
    fn a_finite_difference_row_can_never_claim_verified() {
        let row = Sensitivity::new(
            "hole_d",
            "mass_g",
            -0.4,
            "g/mm",
            3.0,
            Route::FiniteDifference { step: 1e-4 },
        );
        assert_eq!(row.basis, ClaimBasis::Predicted);
        assert!(row.to_claim(oracle()).details.unwrap().contains("step:"));
    }

    #[test]
    fn non_finite_values_never_pass() {
        let row = Sensitivity::new("x", "J", f64::NAN, "1", 0.0, Route::Adjoint);
        assert_eq!(row.verdict, ClaimVerdict::Unverifiable);
    }

    #[test]
    fn render_lists_objectives_and_flags_unusable_rows() {
        let mut t = SensitivityTable::new();
        t.push(s("wall", 1.0, 0.0, 10.0));
        t.push(Sensitivity::new(
            "inlet_v",
            "hotspot_c",
            -3.0,
            "K/(m/s)",
            0.1,
            Route::Coupled {
                completeness: Completeness::Incomplete {
                    reasons: vec!["missing".into()],
                },
            },
        ));
        let r = t.render();
        assert!(r.contains("d(hotspot_c)/d(parameter)"));
        assert!(r.contains("may not steer"));
    }

    #[test]
    fn table_round_trips_through_json() {
        let mut t = SensitivityTable::new();
        t.push(s("wall", -8.1, 1.0, 4.0).with_note("film-averaged coupling"));
        let js = serde_json::to_string(&t).unwrap();
        let back: SensitivityTable = serde_json::from_str(&js).unwrap();
        assert_eq!(t, back);
    }
}
