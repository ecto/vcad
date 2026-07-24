//! Derived physics: Creutz ratios, the static quark potential, and the
//! Cornell fit (M1).
//!
//! All inputs are jackknifed loop estimates; all outputs carry
//! first-order-propagated errors. Functions return `None` rather than a
//! number when the constituent loops are not statistically resolved —
//! the log of a value consistent with zero is not a measurement.

use serde::{Deserialize, Serialize};

use crate::spec::WilsonLoop;

/// Minimum |mean|/err for a loop to enter a logarithm.
pub const MIN_SIGNIFICANCE: f64 = 3.0;

fn find(loops: &[WilsonLoop], r: usize, t: usize) -> Option<&WilsonLoop> {
    loops
        .iter()
        .find(|w| (w.r == r && w.t == t) || (w.r == t && w.t == r))
}

fn resolved(w: &WilsonLoop) -> bool {
    w.value.err > 0.0 && w.value.mean > 0.0 && w.value.mean / w.value.err >= MIN_SIGNIFICANCE
}

fn rel2(w: &WilsonLoop) -> f64 {
    (w.value.err / w.value.mean).powi(2)
}

/// One Creutz ratio with propagated error.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CreutzRatio {
    /// Ratio size (χ(r,r)).
    pub r: usize,
    /// `χ(r,r) = −ln[W(r,r)W(r−1,r−1)/(W(r,r−1)W(r−1,r))]` — estimates
    /// the string tension `σa²`.
    pub chi: f64,
    /// First-order-propagated error.
    pub err: f64,
}

/// All resolvable Creutz ratios from a set of measured loops.
pub fn creutz_ratios(loops: &[WilsonLoop]) -> Vec<CreutzRatio> {
    let max_e = loops.iter().map(|w| w.t.max(w.r)).max().unwrap_or(0);
    let mut out = Vec::new();
    for r in 2..=max_e {
        let quad = [
            find(loops, r, r),
            find(loops, r - 1, r - 1),
            find(loops, r, r - 1),
            find(loops, r - 1, r),
        ];
        let [Some(a), Some(b), Some(c), Some(d)] = quad else {
            continue;
        };
        if ![a, b, c, d].iter().all(|w| resolved(w)) {
            continue;
        }
        let chi = -((a.value.mean * b.value.mean) / (c.value.mean * d.value.mean)).ln();
        let err = (rel2(a) + rel2(b) + rel2(c) + rel2(d)).sqrt();
        out.push(CreutzRatio { r, chi, err });
    }
    out
}

/// One point of the static potential in lattice units.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PotentialPoint {
    /// Quark separation (lattice units).
    pub r: usize,
    /// `V(r)·a = ln(W(r,t−1)/W(r,t))` at the largest available `t`.
    pub v: f64,
    /// Propagated error.
    pub err: f64,
    /// The temporal extent the effective mass was taken at.
    pub t: usize,
}

/// Static potential from temporal Wilson loops: for each spatial
/// extent `r`, the effective potential `ln(W(r,t−1)/W(r,t))` at the
/// largest `t` with both loops resolved. Excited-state contamination
/// falls with `t`; smear spatially before measuring to help.
pub fn static_potential(loops: &[WilsonLoop]) -> Vec<PotentialPoint> {
    let max_r = loops.iter().map(|w| w.r).max().unwrap_or(0);
    let max_t = loops.iter().map(|w| w.t).max().unwrap_or(0);
    let mut out = Vec::new();
    for r in 1..=max_r {
        // Largest t with W(r,t) and W(r,t−1) both resolved.
        for t in (2..=max_t).rev() {
            let (Some(wt), Some(wtm)) = (
                loops.iter().find(|w| w.r == r && w.t == t),
                loops.iter().find(|w| w.r == r && w.t == t - 1),
            ) else {
                continue;
            };
            if !(resolved(wt) && resolved(wtm)) {
                continue;
            }
            let v = (wtm.value.mean / wt.value.mean).ln();
            let err = (rel2(wt) + rel2(wtm)).sqrt();
            out.push(PotentialPoint { r, v, err, t });
            break;
        }
    }
    out
}

/// Cornell-form fit `V(r) = c − a/r + σ·r` (lattice units).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CornellFit {
    /// Constant offset (self-energy).
    pub c: f64,
    /// Coulomb coefficient.
    pub a: f64,
    /// String tension `σa²`.
    pub sigma: f64,
}

/// Unweighted least-squares Cornell fit through ≥ 3 potential points.
/// Returns `None` when under-determined or numerically singular.
pub fn fit_cornell(points: &[PotentialPoint]) -> Option<CornellFit> {
    if points.len() < 3 {
        return None;
    }
    // Basis: [1, −1/r, r]; normal equations 3×3.
    let mut ata = [[0.0f64; 3]; 3];
    let mut atv = [0.0f64; 3];
    for p in points {
        let x = [1.0, -1.0 / p.r as f64, p.r as f64];
        for i in 0..3 {
            for j in 0..3 {
                ata[i][j] += x[i] * x[j];
            }
            atv[i] += x[i] * p.v;
        }
    }
    // Cramer's rule.
    let det3 = |m: &[[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det3(&ata);
    if d.abs() < 1e-12 {
        return None;
    }
    let mut sol = [0.0f64; 3];
    for (k, s) in sol.iter_mut().enumerate() {
        let mut m = ata;
        for i in 0..3 {
            m[i][k] = atv[i];
        }
        *s = det3(&m) / d;
    }
    Some(CornellFit {
        c: sol[0],
        a: sol[1],
        sigma: sol[2],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::Estimate;

    fn wl(r: usize, t: usize, mean: f64) -> WilsonLoop {
        WilsonLoop {
            r,
            t,
            value: Estimate {
                mean,
                err: mean * 0.001,
                n_bins: 10,
                bin_size: 5,
            },
        }
    }

    #[test]
    fn creutz_recovers_pure_area_law() {
        // W(r,t) = e^{−σrt} ⇒ χ(r,r) = σ exactly.
        let sigma = 0.3;
        let w = |r: usize, t: usize| wl(r, t, (-(sigma * (r * t) as f64)).exp());
        let loops = vec![w(1, 1), w(1, 2), w(2, 2)];
        let chis = creutz_ratios(&loops);
        assert_eq!(chis.len(), 1);
        assert!((chis[0].chi - sigma).abs() < 1e-10, "{}", chis[0].chi);
    }

    #[test]
    fn potential_recovers_linear_law() {
        // W(r,t) = e^{−V(r)t}, V(r) = σr ⇒ effective mass exact.
        let sigma = 0.4;
        let mut loops = Vec::new();
        for r in 1..=3 {
            for t in 1..=3 {
                loops.push(wl(r, t, (-(sigma * r as f64) * t as f64).exp()));
            }
        }
        let v = static_potential(&loops);
        assert_eq!(v.len(), 3);
        for p in &v {
            assert_eq!(p.t, 3, "should use largest t");
            assert!((p.v - sigma * p.r as f64).abs() < 1e-10);
        }
    }

    #[test]
    fn cornell_fit_recovers_exact_params() {
        let (c, a, sigma) = (0.5, 0.26, 0.15);
        let pts: Vec<PotentialPoint> = (1..=4)
            .map(|r| PotentialPoint {
                r,
                v: c - a / r as f64 + sigma * r as f64,
                err: 0.001,
                t: 3,
            })
            .collect();
        let fit = fit_cornell(&pts).unwrap();
        assert!((fit.c - c).abs() < 1e-9);
        assert!((fit.a - a).abs() < 1e-9);
        assert!((fit.sigma - sigma).abs() < 1e-9);
    }

    #[test]
    fn unresolved_loops_are_refused() {
        let mut bad = wl(2, 2, 1e-6);
        bad.value.err = 1e-5; // 0.1σ from zero
        let loops = vec![wl(1, 1, 0.5), wl(1, 2, 0.3), bad];
        assert!(creutz_ratios(&loops).is_empty());
    }
}
