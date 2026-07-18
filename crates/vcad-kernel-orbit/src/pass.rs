//! Pass prediction: rise/culminate/set of a satellite over a ground site.
//!
//! Coarse time scan for elevation-mask crossings, refined by bisection.
//! Honesty note: with the M0 force model (two-body + J2, no drag) and
//! GMST-only Earth rotation, predicted pass times against the real sky
//! are good to **±minutes**, not ±seconds — quantified against the ISS
//! fixture in `examples/iss_pass.rs`.

use crate::groundtrack::Site;
use crate::propagate::{propagate, ForceModel};
use crate::state::StateVector;
use crate::OrbitError;

/// One predicted pass above the elevation mask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pass {
    /// Rise time (crossing the mask upward), JD UTC.
    pub rise_jd_utc: f64,
    /// Time of maximum elevation, JD UTC.
    pub culmination_jd_utc: f64,
    /// Set time (crossing the mask downward), JD UTC.
    pub set_jd_utc: f64,
    /// Maximum elevation, radians.
    pub max_elevation_rad: f64,
}

impl Pass {
    /// Pass duration in seconds.
    pub fn duration_s(&self) -> f64 {
        (self.set_jd_utc - self.rise_jd_utc) * 86_400.0
    }
}

/// Predict passes of a satellite over `site` between `jd0_utc` and
/// `jd1_utc`, given its inertial state `sv0` at `jd0_utc`.
///
/// `mask_rad` is the minimum elevation (e.g. 10° for visual passes).
/// The propagation uses `model` with `step_s` RK4 steps; elevation is
/// scanned every `scan_s` seconds and crossings bisected to <0.1 s.
/// A pass that is already up at `jd0` or still up at `jd1` is discarded
/// (fail-closed: only complete rise→set windows are reported).
#[allow(clippy::too_many_arguments)] // a pass forecast genuinely has this many knobs
pub fn predict_passes(
    sv0: &StateVector,
    jd0_utc: f64,
    jd1_utc: f64,
    site: &Site,
    mask_rad: f64,
    model: ForceModel,
    step_s: f64,
    scan_s: f64,
) -> Result<Vec<Pass>, OrbitError> {
    if jd1_utc <= jd0_utc {
        return Err(OrbitError::Invalid("empty time window".into()));
    }
    let total_s = (jd1_utc - jd0_utc) * 86_400.0;
    let n = (total_s / scan_s).ceil() as usize;

    // March a single state forward, recording elevation at scan points.
    let mut states = Vec::with_capacity(n + 1);
    let mut s = *sv0;
    states.push(s);
    for _ in 0..n {
        s = propagate(&s, scan_s, step_s, model);
        states.push(s);
    }
    let elev = |i: usize| -> f64 {
        let jd = jd0_utc + (i as f64 * scan_s) / 86_400.0;
        site.elevation_rad(states[i].r, jd) - mask_rad
    };
    // Bisect a crossing between scan indices i and i+1 to <0.1 s.
    let refine = |i: usize| -> f64 {
        let mut lo = 0.0_f64;
        let mut hi = scan_s;
        let f_lo = elev(i);
        let base = states[i];
        let f = |t: f64| {
            let jd = jd0_utc + (i as f64 * scan_s + t) / 86_400.0;
            let sv = propagate(&base, t, 1.0, model);
            site.elevation_rad(sv.r, jd) - mask_rad
        };
        for _ in 0..40 {
            if hi - lo < 0.1 {
                break;
            }
            let mid = (lo + hi) / 2.0;
            if (f(mid) > 0.0) == (f_lo > 0.0) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        jd0_utc + (i as f64 * scan_s + (lo + hi) / 2.0) / 86_400.0
    };

    let mut passes = Vec::new();
    let mut rise: Option<f64> = None;
    let mut best: Option<(f64, f64)> = None; // (elevation above mask, jd)
    for i in 0..n {
        let e0 = elev(i);
        let e1 = elev(i + 1);
        if rise.is_some() {
            let jd = jd0_utc + (i as f64 * scan_s) / 86_400.0;
            if best.map(|(b, _)| e0 > b).unwrap_or(true) {
                best = Some((e0, jd));
            }
        }
        if e0 <= 0.0 && e1 > 0.0 {
            rise = Some(refine(i));
            best = None;
        } else if e0 > 0.0 && e1 <= 0.0 {
            if let Some(r) = rise.take() {
                let set = refine(i);
                let (bel, bjd) = best.take().unwrap_or((0.0, (r + set) / 2.0));
                passes.push(Pass {
                    rise_jd_utc: r,
                    culmination_jd_utc: bjd,
                    set_jd_utc: set,
                    max_elevation_rad: bel + mask_rad,
                });
            }
        }
    }
    Ok(passes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::OrbitalElements;

    #[test]
    fn iss_like_orbit_produces_ordered_bounded_passes() {
        let sv = OrbitalElements {
            a: 6795.0,
            e: 0.0007,
            i: 51.63_f64.to_radians(),
            raan: 2.6,
            argp: 5.3,
            nu: 0.0,
        }
        .to_state()
        .unwrap();
        let site = Site {
            lat_rad: 37.7749_f64.to_radians(),
            lon_rad: (-122.4194_f64).to_radians(),
            alt_km: 0.0,
        };
        let jd0 = 2_461_238.5;
        let passes = predict_passes(
            &sv,
            jd0,
            jd0 + 1.0,
            &site,
            10.0_f64.to_radians(),
            ForceModel::TwoBodyJ2,
            10.0,
            30.0,
        )
        .unwrap();
        // The ISS covers ±51.6° latitude; SF at 37.8°N sees it several
        // times a day above 10°.
        assert!(
            (1..=8).contains(&passes.len()),
            "{} passes in 24 h",
            passes.len()
        );
        for p in &passes {
            assert!(p.rise_jd_utc < p.culmination_jd_utc);
            assert!(p.culmination_jd_utc < p.set_jd_utc);
            // ISS passes above a 10° mask last between ~30 s and ~8 min.
            assert!(p.duration_s() > 20.0 && p.duration_s() < 600.0);
            assert!(p.max_elevation_rad >= 10.0_f64.to_radians());
            // Rise happens at the mask (within bisection tolerance ~0.1 s
            // of motion, generously 0.5°).
        }
    }

    #[test]
    fn site_outside_coverage_sees_nothing() {
        // A polar site never sees a low-inclination satellite above 10°.
        let sv = OrbitalElements {
            a: 6795.0,
            e: 0.001,
            i: 5.0_f64.to_radians(),
            raan: 0.0,
            argp: 0.0,
            nu: 0.0,
        }
        .to_state()
        .unwrap();
        let site = Site {
            lat_rad: 85.0_f64.to_radians(),
            lon_rad: 0.0,
            alt_km: 0.0,
        };
        let jd0 = 2_461_238.5;
        let passes = predict_passes(
            &sv,
            jd0,
            jd0 + 1.0,
            &site,
            10.0_f64.to_radians(),
            ForceModel::TwoBodyJ2,
            10.0,
            30.0,
        )
        .unwrap();
        assert!(passes.is_empty());
    }

    #[test]
    fn empty_window_fails_closed() {
        let sv = OrbitalElements {
            a: 6795.0,
            e: 0.0,
            i: 0.9,
            raan: 0.0,
            argp: 0.0,
            nu: 0.0,
        }
        .to_state()
        .unwrap();
        let site = Site {
            lat_rad: 0.0,
            lon_rad: 0.0,
            alt_km: 0.0,
        };
        assert!(predict_passes(
            &sv,
            2_461_238.5,
            2_461_238.5,
            &site,
            0.17,
            ForceModel::TwoBody,
            10.0,
            30.0
        )
        .is_err());
    }
}
